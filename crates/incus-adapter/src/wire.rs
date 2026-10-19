// SPDX-License-Identifier: AGPL-3.0-only
//! Incus REST wire types (subset), field names as in Incus's own API specification
//! (`doc/rest-api.yaml` at the pinned release). Unknown fields are ignored and missing
//! fields default, so newer servers with extra fields still parse.

use std::collections::BTreeMap;

use serde::Deserialize;

/// Every Incus response is wrapped in this envelope (`type` is sync, async or error).
#[derive(Debug, Deserialize)]
pub(crate) struct Envelope<T> {
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub error_code: u16,
    pub metadata: Option<T>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Server {
    pub api_extensions: Vec<String>,
    pub api_version: String,
    pub auth: String,
    pub environment: ServerEnvironment,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ServerEnvironment {
    pub server_clustered: bool,
    pub server_version: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct ClusterMember {
    pub server_name: String,
    pub url: String,
    pub roles: Vec<String>,
    pub status: String,
    pub message: String,
    pub architecture: String,
    pub failure_domain: String,
    pub database: bool,
    pub groups: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Instance {
    pub name: String,
    pub project: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub status: String,
    pub location: String,
    pub architecture: String,
    pub description: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Project {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Network {
    pub name: String,
    pub project: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub managed: bool,
    pub status: String,
    pub description: String,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct StoragePool {
    pub name: String,
    pub driver: String,
    pub status: String,
    pub description: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Operation {
    pub id: String,
    pub class: String,
    pub description: String,
    pub status: String,
    pub err: String,
    pub location: String,
    pub may_cancel: bool,
    pub created_at: String,
    pub updated_at: String,
}

/// `GET /1.0/operations?recursion=1`: the specification describes a list, while servers have
/// grouped operations by status; accept both.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum OperationList {
    Flat(Vec<Operation>),
    ByStatus(BTreeMap<String, Vec<Operation>>),
}

impl OperationList {
    pub fn into_vec(self) -> Vec<Operation> {
        match self {
            OperationList::Flat(v) => v,
            OperationList::ByStatus(m) => m.into_values().flatten().collect(),
        }
    }
}
