// SPDX-License-Identifier: AGPL-3.0-only
//! Service identities: the credentials Apelora itself uses. Each kind may do one family of
//! things, with its own credential, so a leak of one does not open the others.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ServiceKind {
    /// Creates and changes instances in the tenants' Incus projects (restricted certificate).
    Workload,
    /// Manages networks, ACLs and load balancers.
    Network,
    /// Manages storage volumes.
    Storage,
    /// Commissions, installs and powers physical machines (MAAS) and issues join tokens.
    Provisioning,
    /// A script or pipeline calling the Apelora API with an API token and ordinary role assignments.
    Automation,
}

impl ServiceKind {
    pub const ALL: [ServiceKind; 5] = [ServiceKind::Workload, ServiceKind::Network, ServiceKind::Storage, ServiceKind::Provisioning, ServiceKind::Automation];

    pub fn as_str(self) -> &'static str {
        match self {
            ServiceKind::Workload => "workload",
            ServiceKind::Network => "network",
            ServiceKind::Storage => "storage",
            ServiceKind::Provisioning => "provisioning",
            ServiceKind::Automation => "automation",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.as_str() == s)
    }

    /// The system this identity authenticates *to*.
    pub fn backend(self) -> &'static str {
        match self {
            ServiceKind::Workload | ServiceKind::Network | ServiceKind::Storage => "incus",
            ServiceKind::Provisioning => "incus-admin and maas",
            ServiceKind::Automation => "apelora",
        }
    }

    /// Outbound identities hold a credential for a backend and can never call the Apelora API;
    /// only `Automation` identities have an API token.
    pub fn may_call_api(self) -> bool {
        self == ServiceKind::Automation
    }

    /// The operation families this kind may perform on its backend.
    pub fn operations(self) -> &'static [&'static str] {
        match self {
            ServiceKind::Workload => &["instance.create", "instance.state", "instance.delete", "instance.read"],
            ServiceKind::Network => &["network.create", "network.acl", "network.load_balancer", "network.read"],
            ServiceKind::Storage => &["volume.create", "volume.attach", "volume.snapshot", "volume.delete", "volume.read"],
            ServiceKind::Provisioning => &["cluster.join_token", "cluster.evacuate", "machine.commission", "machine.deploy", "machine.power", "machine.release"],
            ServiceKind::Automation => &[],
        }
    }

    pub fn may(self, operation: &str) -> bool {
        self.operations().contains(&operation)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_are_separated_by_purpose() {
        assert!(ServiceKind::Workload.may("instance.create"));
        assert!(!ServiceKind::Workload.may("network.acl"));
        assert!(!ServiceKind::Workload.may("machine.power"));
        assert!(!ServiceKind::Network.may("instance.create"));
        assert!(!ServiceKind::Storage.may("cluster.join_token"));
        assert!(ServiceKind::Provisioning.may("machine.power"));
        assert!(!ServiceKind::Provisioning.may("instance.create"));
    }

    #[test]
    fn only_automation_identities_call_the_api() {
        for k in ServiceKind::ALL {
            assert_eq!(k.may_call_api(), k == ServiceKind::Automation, "{k:?}");
            assert_eq!(ServiceKind::parse(k.as_str()), Some(k));
        }
        assert!(ServiceKind::Automation.operations().is_empty());
    }
}
