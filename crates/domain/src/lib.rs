// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora domain types shared by the control plane and the deployment worker. No I/O.

use serde::{Deserialize, Serialize};

/// Version segment of the Apelora REST API (`/api/v1`).
pub const API_VERSION: &str = "v1";

/// How an appliance runs on Incus (ADR 0001).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ProviderMode {
    /// Fixture data for development and tests; never a real cluster.
    Mock,
    Incus,
}

/// How much evidence backs a claim about a feature (see the `testing-operations` skill).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Evidence {
    NotTested,
    MockVerified,
    ContractVerified,
    RealIncusTested,
}

/// Kind of Incus instance as reported by the cluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum InstanceKind {
    Container,
    VirtualMachine,
    Unknown,
}

/// Observed state of an Incus cluster member ("minion" in the UI).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct StoragePool {
    pub name: String,
    pub driver: String,
    pub status: String,
    pub description: String,
}

/// Facts about the Incus server the adapter is talking to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub api_version: String,
    /// `None` when unknown (e.g. the mock provider).
    #[cfg_attr(feature = "utoipa", schema(required = true))]
    pub server_version: Option<String>,
    pub api_extensions: Vec<String>,
    pub clustered: bool,
    pub auth: String,
}

/// Instance config key Apelora sets on every instance it creates. Apelora only changes or
/// deletes instances that carry it with the value `true`; everything else is left untouched.
pub const MANAGED_KEY: &str = "user.apelora.managed";

/// Whether `name` is a valid Incus instance or project name as Apelora uses them: 1–63
/// characters of ASCII letters, digits and `-`, starting with a letter and not ending with
/// `-`. Stricter than Incus where convenient, so a name can never inject into an API path.
pub fn is_valid_name(name: &str) -> bool {
    let b = name.as_bytes();
    (1..=63).contains(&b.len())
        && b[0].is_ascii_alphabetic()
        && b[b.len() - 1] != b'-'
        && b.iter().all(|c| c.is_ascii_alphanumeric() || *c == b'-')
}

/// How Incus fetches an image.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ImageProtocol {
    /// System-container and VM images from a simplestreams image server.
    Simplestreams,
    /// Application containers from an OCI registry (needs the `instance_oci` API extension).
    Oci,
}

/// Where an instance's root image comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ImageSource {
    pub protocol: ImageProtocol,
    /// Image server URL, e.g. a simplestreams server or an OCI registry.
    pub server: String,
    /// Alias such as `debian/13` or, for OCI, `repository:tag`.
    pub alias: String,
}

/// An instance Apelora should create. Apelora's ownership marker is added on creation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct InstanceSpec {
    pub project: String,
    pub name: String,
    /// `container` (system or OCI) or `virtual-machine`.
    pub kind: InstanceKind,
    pub source: ImageSource,
    #[serde(default)]
    pub profiles: Vec<String>,
    /// Extra instance config; keys under `user.apelora.` are reserved.
    #[serde(default)]
    pub config: std::collections::BTreeMap<String, String>,
    /// Start the instance once created.
    #[serde(default)]
    pub start: bool,
}

impl InstanceSpec {
    /// Checks names, kind and config before anything is sent to Incus.
    pub fn validate(&self) -> Result<(), String> {
        if !is_valid_name(&self.project) {
            return Err(format!("invalid project name {:?}", self.project));
        }
        if !is_valid_name(&self.name) {
            return Err(format!("invalid instance name {:?}", self.name));
        }
        if self.kind == InstanceKind::Unknown {
            return Err("kind must be container or virtual-machine".into());
        }
        if self.source.protocol == ImageProtocol::Oci && self.kind != InstanceKind::Container {
            return Err("OCI images run only as containers".into());
        }
        if !self.source.server.starts_with("https://") {
            return Err("the image server must use https://".into());
        }
        if self.source.alias.is_empty() {
            return Err("an image alias is required".into());
        }
        if let Some(k) = self.config.keys().find(|k| k.starts_with("user.apelora.")) {
            return Err(format!("config key {k} is reserved for Apelora"));
        }
        Ok(())
    }
}

/// A state change Apelora can request for one of its instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum InstanceAction {
    Start,
    Stop,
}

impl InstanceAction {
    pub fn as_str(self) -> &'static str {
        match self {
            InstanceAction::Start => "start",
            InstanceAction::Stop => "stop",
        }
    }

    /// The Incus status an instance has once the action has taken effect.
    pub fn target_status(self) -> &'static str {
        match self {
            InstanceAction::Start => "Running",
            InstanceAction::Stop => "Stopped",
        }
    }
}

/// Who may do what. Every API route names the permission it needs; roles map to permissions
/// here and nowhere else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
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
    fn names_are_safe_for_api_paths() {
        for ok in ["web", "web-01", "a", &"a".repeat(63)] {
            assert!(is_valid_name(ok), "{ok}");
        }
        for bad in [
            "",
            "1web",
            "-web",
            "web-",
            "web_01",
            "web/../x",
            "web?x",
            "wéb",
            &"a".repeat(64),
        ] {
            assert!(!is_valid_name(bad), "{bad}");
        }
    }

    fn spec() -> InstanceSpec {
        InstanceSpec {
            project: "apelora".into(),
            name: "web-01".into(),
            kind: InstanceKind::Container,
            source: ImageSource {
                protocol: ImageProtocol::Oci,
                server: "https://registry.example".into(),
                alias: "nginx:1.29".into(),
            },
            profiles: vec!["default".into()],
            config: Default::default(),
            start: true,
        }
    }

    #[test]
    fn instance_specs_are_validated_before_reaching_incus() {
        assert_eq!(spec().validate(), Ok(()));
        let vm_oci = InstanceSpec {
            kind: InstanceKind::VirtualMachine,
            ..spec()
        };
        assert!(vm_oci.validate().is_err());
        let http = InstanceSpec {
            source: ImageSource {
                server: "http://x".into(),
                ..spec().source
            },
            ..spec()
        };
        assert!(http.validate().is_err());
        let mut reserved = spec();
        reserved.config.insert(MANAGED_KEY.into(), "false".into());
        assert!(
            reserved
                .validate()
                .expect_err("reserved key")
                .contains("reserved")
        );
        assert!(
            InstanceSpec {
                project: "a/b".into(),
                ..spec()
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn evidence_labels_are_the_reporting_labels() {
        assert_eq!(json(Evidence::MockVerified), "\"MOCK_VERIFIED\"");
        assert_eq!(json(Evidence::RealIncusTested), "\"REAL_INCUS_TESTED\"");
    }
}
