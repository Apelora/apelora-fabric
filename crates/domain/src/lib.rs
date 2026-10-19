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

/// Who may do what. Every API route names the permission it needs; roles map to permissions
/// here and nowhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    /// Manages the platform: members, networks, storage, tokens.
    FabricAdmin,
    /// Runs workloads: deploys and operates applications.
    Operator,
    /// Designs applications in Studio; cannot change infrastructure.
    StudioDesigner,
    /// Reads everything, changes nothing.
    Auditor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    ReadInventory,
    ReadAudit,
    ManageTokens,
    ManageInfrastructure,
    DesignApplications,
    Deploy,
}

impl Role {
    pub const ALL: [Role; 4] = [
        Role::FabricAdmin,
        Role::Operator,
        Role::StudioDesigner,
        Role::Auditor,
    ];

    pub fn allows(self, p: Permission) -> bool {
        use Permission::*;
        match self {
            Role::FabricAdmin => true,
            Role::Operator => matches!(p, ReadInventory | DesignApplications | Deploy),
            Role::StudioDesigner => matches!(p, ReadInventory | DesignApplications),
            Role::Auditor => matches!(p, ReadInventory | ReadAudit),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Role::FabricAdmin => "fabric-admin",
            Role::Operator => "operator",
            Role::StudioDesigner => "studio-designer",
            Role::Auditor => "auditor",
        }
    }

    pub fn parse(s: &str) -> Option<Role> {
        Role::ALL.into_iter().find(|r| r.as_str() == s)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn roles_follow_least_privilege() {
        use super::{Permission::*, Role};
        assert!(Role::FabricAdmin.allows(ManageTokens));
        assert!(!Role::Operator.allows(ManageInfrastructure));
        assert!(
            !Role::StudioDesigner.allows(Deploy),
            "designers cannot change running infrastructure"
        );
        assert!(Role::Auditor.allows(ReadAudit) && !Role::Auditor.allows(DesignApplications));
        for r in Role::ALL {
            assert!(r.allows(ReadInventory));
            assert_eq!(Role::parse(r.as_str()), Some(r));
        }
        assert_eq!(Role::parse("root"), None);
    }

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
