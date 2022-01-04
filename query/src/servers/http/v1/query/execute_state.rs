// Copyright 2021 Datafuse Labs.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use common_base::tokio;
use common_base::tokio::sync::mpsc;
use common_base::tokio::sync::RwLock;
use common_base::ProgressValues;
use common_base::TrySpawn;
use common_datablocks::DataBlock;
use common_datavalues::DataSchemaRef;
use common_exception::ErrorCode;
use common_exception::Result;
use common_tracing::tracing;
use futures::StreamExt;
use serde::Deserialize;
use serde::Serialize;
use ExecuteState::*;

use crate::interpreters::Interpreter;
use crate::interpreters::InterpreterFactory;
use crate::sessions::QueryContext;
use crate::sessions::SessionManager;
use crate::sessions::SessionRef;
use crate::sql::PlanParser;

#[derive(Deserialize, Debug)]
pub struct HttpQueryRequest {
    #[serde(default)]
    pub session: HttpSessionConf,
    pub sql: String,
}

#[derive(Deserialize, Debug, Default)]
pub struct HttpSessionConf {
    pub database: Option<String>,
    pub user: Option<String>,
}

#[derive(Serialize, Deserialize, Debug, Copy, Clone, PartialEq)]
pub enum ExecuteStateName {
    Running,
    Failed,
    Succeeded,
}

pub(crate) enum ExecuteState {
    Running(ExecuteRunning),
    Stopped(ExecuteStopped),
}

impl ExecuteState {
    pub(crate) fn extract(&self) -> (ExecuteStateName, Option<ErrorCode>) {
        match self {
            ExecuteState::Running(_) => (ExecuteStateName::Running, None),
            ExecuteState::Stopped(v) => match &v.reason {
                Ok(_) => (ExecuteStateName::Succeeded, None),
                Err(e) => (ExecuteStateName::Failed, Some(e.clone())),
            },
        }
    }
}

pub(crate) type ExecutorRef = Arc<RwLock<Executor>>;

pub(crate) struct ExecuteStopped {
    progress: Option<ProgressValues>,
    reason: Result<()>,
    stop_time: Instant,
}

pub(crate) struct Executor {
    start_time: Instant,
    pub(crate) state: ExecuteState,
}

impl Executor {
    pub(crate) fn get_progress(&self) -> Option<ProgressValues> {
        match &self.state {
            Running(r) => Some(r.context.get_scan_progress_value()),
            Stopped(f) => f.progress.clone(),
        }
    }
    pub(crate) fn elapsed(&self) -> Duration {
        match &self.state {
            Running(_) => Instant::now() - self.start_time,
            Stopped(f) => f.stop_time - self.start_time,
        }
    }
    pub(crate) async fn stop(this: &ExecutorRef, reason: Result<()>, kill: bool) {
        let mut guard = this.write().await;
        if let Running(r) = &guard.state {
            // release session
            let progress = Some(r.context.get_scan_progress_value());
            if kill {
                r.session.force_kill_query();
            }
            // Write Finish to query log table.
            let _ = r
                .interpreter
                .finish()
                .await
                .map_err(|e| tracing::error!("interpreter.finish error: {:?}", e));
            guard.state = Stopped(ExecuteStopped {
                progress,
                reason,
                stop_time: Instant::now(),
            });
        };
    }
}

pub struct HttpQueryHandle {
    pub abort_sender: mpsc::Sender<()>,
}

impl HttpQueryHandle {
    pub fn abort(&self) {
        let sender = self.abort_sender.clone();
        tokio::spawn(async move {
            sender.send(()).await.ok();
        });
    }
}

pub(crate) struct ExecuteRunning {
    // used to kill query
    session: SessionRef,
    // mainly used to get progress for now
    context: Arc<QueryContext>,
    interpreter: Arc<dyn Interpreter>,
}

impl ExecuteState {
    pub(crate) async fn try_create(
        request: &HttpQueryRequest,
        session_manager: &Arc<SessionManager>,
        block_tx: mpsc::Sender<DataBlock>,
    ) -> Result<(ExecutorRef, DataSchemaRef)> {
        let sql = &request.sql;
        let session = session_manager.create_session("http-statement")?;
        let context = session.create_context().await?;
        if let Some(db) = &request.session.database {
            context.set_current_database(db.clone()).await?;
        };
        context.attach_query_str(sql);
        let default_user = "root".to_string();
        let user_name = request.session.user.as_ref().unwrap_or(&default_user);
        let user_manager = session.get_user_manager();

        // TODO: list user's grant list and check client address
        let ctx = session.create_context().await?;
        let user_info = user_manager
            .get_user(ctx.get_tenant(), user_name, "%")
            .await?;
        session.set_current_user(user_info);

        let plan = PlanParser::parse(sql, context.clone()).await?;
        let schema = plan.schema();

        let interpreter = InterpreterFactory::get(context.clone(), plan.clone())?;
        // Write Start to query log table.
        let _ = interpreter
            .start()
            .await
            .map_err(|e| tracing::error!("interpreter.start.error: {:?}", e));

        let data_stream = interpreter.execute(None).await?;
        let mut data_stream = context.try_create_abortable(data_stream)?;

        let (abort_tx, mut abort_rx) = mpsc::channel(2);
        context.attach_http_query(HttpQueryHandle {
            abort_sender: abort_tx,
        });

        let running_state = ExecuteRunning {
            session,
            context: context.clone(),
            interpreter: interpreter.clone(),
        };
        let executor = Arc::new(RwLock::new(Executor {
            start_time: Instant::now(),
            state: Running(running_state),
        }));

        let executor_clone = executor.clone();
        context
            .try_spawn(async move {
                loop {
                    if let Some(block_r) = data_stream.next().await {
                        match block_r {
                            Ok(block) => tokio::select! {
                                _ = block_tx.send(block) => { },
                                _ = abort_rx.recv() => {
                                    Executor::stop(&executor, Err(ErrorCode::AbortedQuery("query aborted")), true).await;
                                    break;
                                },
                            },
                            Err(err) => {
                                Executor::stop(&executor, Err(err), false).await;
                                break;
                            }
                        };
                    } else {
                        Executor::stop(&executor, Ok(()), false).await;
                        break;
                    }
                }
                tracing::debug!("drop block sender!");
            })?;

        Ok((executor_clone, schema))
    }
}
