// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora domain types shared by the control plane and the deployment worker. No I/O.

use serde::{Deserialize, Serialize};

/// Version segment of the Apelora REST API (`/api/v1`).
pub const API_VERSION: &str = "v1";

/// How an appliance runs on Incus (ADR 0001).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Runtime {
    IncusSystemContainer,
    IncusOciContainer,
    IncusVm,
    /// A Studio grouping of other appliances; not an Incus instance type.
    Composite,
}

impl Runtime {
    pub fn is_incus_instance(self) -> bool {
        !matches!(self, Runtime::Composite)
    }
}

/// Where the backend's view of the infrastructure comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderMode {
    /// Fixture data for development and tests; never a real cluster.
    Mock,
    Incus,
}

/// How much evidence backs a claim about a feature (see the `testing-operations` skill).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Evidence {
    NotTested,
    MockVerified,
    ContractVerified,
    RealIncusTested,
}

/// Kind of Incus instance as reported by the cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstanceKind {
    Container,
    VirtualMachine,
    Unknown,
}

/// Observed state of an Incus cluster member ("minion" in the UI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterMember {
    pub name: String,
    pub url: String,
    pub roles: Vec<String>,
    pub status: String,
    pub message: String,
    pub architecture: String,
    pub failure_domain: String,
    pub database: bool,
    pub groups: Vec<String>,
}

/// Observed state of one Incus instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    pub name: String,
    pub project: String,
    pub kind: InstanceKind,
    pub status: String,
    pub location: String,
    pub architecture: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Network {
    pub name: String,
    pub project: String,
    pub kind: String,
    pub managed: bool,
    pub status: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoragePool {
    pub name: String,
    pub driver: String,
    pub status: String,
    pub description: String,
}

/// Facts about the Incus server the adapter is talking to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub api_version: String,
    /// `None` when unknown (e.g. the mock provider).
    pub server_version: Option<String>,
    pub api_extensions: Vec<String>,
    pub clustered: bool,
    pub auth: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json<T: Serialize>(v: T) -> String {
        serde_json::to_string(&v).expect("serialisable")
    }

    #[test]
    fn runtime_names_match_the_catalog_spelling() {
        assert_eq!(
            json(Runtime::IncusSystemContainer),
            "\"incus-system-container\""
        );
        assert_eq!(json(Runtime::IncusOciContainer), "\"incus-oci-container\"");
        assert_eq!(json(Runtime::IncusVm), "\"incus-vm\"");
        assert!(!Runtime::Composite.is_incus_instance());
        assert!(Runtime::IncusVm.is_incus_instance());
    }

    #[test]
    fn evidence_labels_are_the_reporting_labels() {
        assert_eq!(json(Evidence::MockVerified), "\"MOCK_VERIFIED\"");
        assert_eq!(json(Evidence::RealIncusTested), "\"REAL_INCUS_TESTED\"");
    }
}
