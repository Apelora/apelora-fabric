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
    /// For `async` responses: `/1.0/operations/<id>`.
    #[serde(default)]
    pub operation: String,
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
    pub kernel_version: String,
    pub architectures: Vec<String>,
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
    /// `null` from some servers; treated as empty.
    pub config: Option<BTreeMap<String, String>>,
    pub description: String,
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
    /// `null` for an instance on an unreachable member: Incus cannot report its configuration.
    /// A missing field is an empty configuration.
    #[serde(default = "known_config")]
    pub config: Option<BTreeMap<String, String>>,
    #[serde(default = "known_config")]
    pub expanded_config: Option<BTreeMap<String, String>>,
}

fn known_config() -> Option<BTreeMap<String, String>> {
    Some(BTreeMap::new())
}

/// Any Apelora-owned object (network, ACL, address set): only its config is read.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Owned {
    pub name: String,
    pub config: BTreeMap<String, String>,
}

/// A network load balancer (`/1.0/networks/<network>/load-balancers/<address>`).
#[derive(Debug, Default, Clone, PartialEq, Eq, Deserialize)]
#[serde(default)]
pub(crate) struct LoadBalancer {
    pub listen_address: String,
    pub description: String,
    pub config: BTreeMap<String, String>,
    pub backends: Vec<LoadBalancerBackend>,
    pub ports: Vec<LoadBalancerPort>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(default)]
pub(crate) struct LoadBalancerBackend {
    pub name: String,
    pub description: String,
    pub target_address: String,
    pub target_port: String,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(default)]
pub(crate) struct LoadBalancerPort {
    pub description: String,
    pub protocol: String,
    pub listen_port: String,
    pub target_backend: Vec<String>,
}

/// `GET /1.0/networks/<network>/load-balancers/<address>/state`.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct LoadBalancerState {
    pub backend_health: BTreeMap<String, BackendHealth>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct BackendHealth {
    pub ports: Vec<BackendPortHealth>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct BackendPortHealth {
    pub status: String,
}

impl LoadBalancer {
    pub fn from_body(body: &serde_json::Value) -> Result<Self, crate::AdapterError> {
        serde_json::from_value(body.clone())
            .map_err(|e| crate::AdapterError::Decode(format!("load balancer body: {e}")))
    }

    /// The fields Apelora sets, in a comparable order (the listen address is the identity).
    pub fn normalised(
        &self,
    ) -> (
        String,
        BTreeMap<String, String>,
        Vec<LoadBalancerBackend>,
        Vec<LoadBalancerPort>,
    ) {
        let mut backends = self.backends.clone();
        backends.sort();
        let mut ports = self.ports.clone();
        for p in &mut ports {
            p.target_backend.sort();
        }
        ports.sort();
        (
            self.description.clone(),
            self.config.clone(),
            backends,
            ports,
        )
    }
}

/// `GET /1.0/instances/<name>/state`: the addresses.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct InstanceStateDetail {
    /// `null` while the instance is stopped.
    pub network: Option<BTreeMap<String, NetworkState>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct NetworkState {
    pub addresses: Vec<NetworkAddress>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct NetworkAddress {
    pub family: String,
    pub address: String,
    pub scope: String,
}

/// `GET /1.0/instances/<name>`: only what lifecycle decisions need.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct InstanceDetail {
    pub status: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub architecture: String,
    pub config: BTreeMap<String, String>,
    pub location: String,
    /// The instance's own devices (not its profiles').
    pub devices: BTreeMap<String, BTreeMap<String, String>>,
}

/// A snapshot of a custom volume (`…/volumes/custom/<name>/snapshots/<snapshot>`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct VolumeSnapshot {
    pub name: String,
    pub created_at: String,
}

/// A custom storage volume (`/1.0/storage-pools/<pool>/volumes/custom/<name>`).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Volume {
    pub config: BTreeMap<String, String>,
    pub description: String,
    pub content_type: String,
    pub location: String,
    pub used_by: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct Project {
    pub name: String,
    pub description: String,
    pub config: BTreeMap<String, String>,
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
    /// `null` for unmanaged host interfaces.
    pub locations: Option<Vec<String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub(crate) struct StoragePool {
    pub name: String,
    pub driver: String,
    pub status: String,
    pub description: String,
    /// `null` for unmanaged host interfaces.
    pub locations: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
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

/// A cluster join token operation (`class: token`). Its metadata holds the join secret.
#[derive(Default, Deserialize)]
#[serde(default)]
pub(crate) struct TokenOperation {
    pub id: String,
    pub class: String,
    pub status: String,
    pub metadata: Option<JoinTokenMeta>,
}

/// Metadata of a join token operation (`POST /1.0/cluster/members`).
#[derive(Default, Deserialize, serde::Serialize)]
#[serde(default, rename_all = "camelCase")]
pub(crate) struct JoinTokenMeta {
    pub server_name: String,
    pub secret: String,
    pub fingerprint: String,
    pub addresses: Vec<String>,
    pub expires_at: String,
}

/// The token a joining server is given: Incus encodes these fields as JSON, then base64.
#[derive(serde::Serialize)]
pub(crate) struct JoinTokenWire<'a> {
    pub server_name: &'a str,
    pub fingerprint: &'a str,
    pub addresses: &'a [String],
    pub secret: &'a str,
    pub expires_at: &'a str,
}

/// `GET /1.0/operations?recursion=1`: the specification describes a list, while servers have
/// grouped operations by status; accept both.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub(crate) enum OperationList<T = Operation> {
    Flat(Vec<T>),
    ByStatus(BTreeMap<String, Vec<T>>),
}

impl<T> OperationList<T> {
    pub fn into_vec(self) -> Vec<T> {
        match self {
            OperationList::Flat(v) => v,
            OperationList::ByStatus(m) => m.into_values().flatten().collect(),
        }
    }
}
