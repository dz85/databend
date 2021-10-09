// Copyright 2020 Datafuse Labs.
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

use std::convert::TryInto;
use std::io::Cursor;

use common_arrow::arrow_flight::Action;
use common_exception::ErrorCode;
use prost::Message;
use tonic::Request;

use crate::impls::CreateDatabaseAction;
use crate::impls::CreateTableAction;
use crate::impls::DropDatabaseAction;
use crate::impls::DropTableAction;
use crate::impls::GetDatabaseAction;
use crate::impls::GetDatabasesAction;
use crate::impls::GetKVAction;
use crate::impls::GetTableAction;
use crate::impls::GetTableExtReq;
use crate::impls::GetTablesAction;
use crate::impls::KVMetaAction;
use crate::impls::MGetKVAction;
use crate::impls::PrefixListReq;
use crate::impls::UpsertKVAction;
use crate::protobuf::FlightMetaRequest;

pub trait RequestFor {
    type Reply;
}

#[macro_export]
macro_rules! action_declare {
    ($req:ident, $reply:tt, $enum_ctor:expr) => {
        impl RequestFor for $req {
            type Reply = $reply;
        }

        impl From<$req> for MetaFlightAction {
            fn from(act: $req) -> Self {
                $enum_ctor(act)
            }
        }
    };
}

// Action wrapper for do_action.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub enum MetaFlightAction {
    // database meta
    CreateDatabase(CreateDatabaseAction),
    GetDatabase(GetDatabaseAction),
    DropDatabase(DropDatabaseAction),
    CreateTable(CreateTableAction),
    DropTable(DropTableAction),
    GetTable(GetTableAction),
    GetTableExt(GetTableExtReq),
    GetTables(GetTablesAction),
    GetDatabases(GetDatabasesAction),

    // general purpose kv
    UpsertKV(UpsertKVAction),
    UpdateKVMeta(KVMetaAction),
    GetKV(GetKVAction),
    MGetKV(MGetKVAction),
    PrefixListKV(PrefixListReq),
}

/// Try convert tonic::Request<Action> to DoActionAction.
impl TryInto<MetaFlightAction> for Request<Action> {
    type Error = tonic::Status;

    fn try_into(self) -> Result<MetaFlightAction, Self::Error> {
        let action = self.into_inner();
        let mut buf = Cursor::new(&action.body);

        // Decode FlightRequest from buffer.
        let request: FlightMetaRequest = FlightMetaRequest::decode(&mut buf)
            .map_err(|e| tonic::Status::internal(e.to_string()))?;

        // Decode DoActionAction from flight request body.
        let json_str = request.body.as_str();
        let action = serde_json::from_str::<MetaFlightAction>(json_str)
            .map_err(|e| tonic::Status::internal(e.to_string()))?;
        Ok(action)
    }
}

/// Try convert DoActionAction to tonic::Request<Action>.
impl TryInto<Request<Action>> for &MetaFlightAction {
    type Error = ErrorCode;

    fn try_into(self) -> common_exception::Result<Request<Action>> {
        let flight_request = FlightMetaRequest {
            body: serde_json::to_string(&self)?,
        };
        let mut buf = vec![];
        flight_request.encode(&mut buf)?;
        let request = tonic::Request::new(Action {
            r#type: "".to_string(),
            body: buf,
        });
        Ok(request)
    }
}