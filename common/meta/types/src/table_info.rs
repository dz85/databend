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

use std::collections::HashMap;

use common_datavalues::DataSchemaRef;

use crate::MetaVersion;

#[derive(serde::Serialize, serde::Deserialize, Clone, Debug, Eq, PartialEq)]
pub struct TableInfo {
    pub database_id: u64,
    pub table_id: u64,

    /// version of this table snapshot.
    ///
    /// Any change to a table causes the version to increment, e.g. insert or delete rows, update schema etc.
    /// But renaming a table should not affect the version, since the table itself does not change.
    /// The tuple (database_id, table_id, version) identifies a unique and consistent table snapshot.
    ///
    /// A version is not guaranteed to be consecutive.
    ///
    pub version: MetaVersion,

    pub db: String,
    pub name: String,

    pub schema: DataSchemaRef,
    pub engine: String,
    pub options: HashMap<String, String>,
}