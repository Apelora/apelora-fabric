// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora domain types shared by the control plane and the deployment worker. No I/O.

use serde::{Deserialize, Serialize};

pub mod access;
pub mod enrolment;
pub mod evacuation;
pub mod health;
pub mod lifecycle;
pub mod network;
pub mod plan;
pub mod preflight;
pub mod removal;
pub mod storage;

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
    /// Incus's `scheduler.instance` for this member: `all` (the default), `manual` (only
    /// instances targeted at it are placed there) or `group`.
    #[serde(default = "default_scheduler")]
    pub scheduler: String,
}

fn default_scheduler() -> String {
    "all".into()
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
    /// Carries Apelora's ownership marker: only these can be stopped or deleted through Apelora.
    #[serde(default)]
    pub managed: bool,
    /// The application and appliance this instance was deployed for (Apelora's labels).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<Owner>,
    /// What Incus does with it when its member is evacuated (`cluster.evacuate`, with
    /// profiles applied); `None`: Incus's default, `auto`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evacuate: Option<String>,
    /// Incus cannot report its configuration because its member is unreachable, so its
    /// labels (and `managed`, `owner`) are unknown, not absent.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unreachable: bool,
}

/// Instance config keys Apelora writes so an instance can be traced back to the application
/// and the appliance (a node in the Studio design) it was deployed for.
pub const APPLICATION_KEY: &str = "user.apelora.application";
pub const APPLIANCE_KEY: &str = "user.apelora.appliance";
/// 1-based replica number of the instance within its appliance.
pub const REPLICA_KEY: &str = "user.apelora.replica";

fn first_replica() -> u8 {
    1
}

/// The application and appliance an instance belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct Owner {
    pub application: String,
    pub appliance: String,
    #[serde(default = "first_replica")]
    pub replica: u8,
}

impl Owner {
    pub fn validate(&self) -> Result<(), String> {
        for (what, v) in [
            ("application", &self.application),
            ("appliance", &self.appliance),
        ] {
            if !is_valid_name(v) {
                return Err(format!("invalid {what} name {v:?}"));
            }
        }
        if !(1..=plan::MAX_REPLICAS).contains(&self.replica) {
            return Err(format!("invalid replica number {}", self.replica));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Project {
    pub name: String,
    pub description: String,
    /// `restricted=true`: the project limits what its instances may do.
    pub restricted: bool,
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
    /// Cluster members it is created on; empty when not per member (or unknown).
    #[serde(default)]
    pub locations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct StoragePool {
    pub name: String,
    pub driver: String,
    pub status: String,
    pub description: String,
    /// Cluster members it is created on; empty when unknown.
    #[serde(default)]
    pub locations: Vec<String>,
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
    /// `None` when unknown (e.g. the mock provider).
    #[cfg_attr(feature = "utoipa", schema(required = true))]
    pub kernel_version: Option<String>,
    pub api_extensions: Vec<String>,
    /// Architectures this server can run (`environment.architectures`); empty when unknown.
    #[serde(default)]
    pub architectures: Vec<String>,
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
    /// Alias such as `debian/13` or, for OCI, `repository:tag`. Pin an OCI image by digest:
    /// `repository@sha256:<64 hex digits>`.
    pub alias: String,
    /// Pins a simplestreams image: the full SHA-256 fingerprint Incus gives it. The instance
    /// must then be built from exactly this image.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

/// Architecture names as Incus reports them.
pub const ARCHITECTURES: &[&str] = &[
    "i686",
    "x86_64",
    "armv6l",
    "armv7l",
    "armv8l",
    "aarch64",
    "ppc",
    "ppc64",
    "ppc64le",
    "s390x",
    "mips",
    "mips64",
    "riscv32",
    "riscv64",
    "loongarch64",
];

fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
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
    /// Profiles to apply, in order; empty applies the project's `default` profile.
    #[serde(default)]
    pub profiles: Vec<String>,
    /// Extra instance config; keys under `user.apelora.` are reserved.
    #[serde(default)]
    pub config: std::collections::BTreeMap<String, String>,
    /// Start the instance once created.
    #[serde(default)]
    pub start: bool,
    /// Architectures the image is known to run on (see [`ARCHITECTURES`]); empty allows any.
    /// Checked against the cluster before creating and against the instance Incus created.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub architectures: Vec<String>,
    /// Set by the control plane when the instance is part of an application deployment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<Owner>,
    /// Set by the control plane when the application's connections are enforced: the NIC
    /// joins the application's network with its own ACL (unmatched traffic rejected).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub network: Option<network::NetworkAttachment>,
    /// Persistent volumes, each created for this instance on the member it runs on.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<storage::VolumeMount>,
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
        if let Some(fp) = &self.source.fingerprint {
            if self.source.protocol != ImageProtocol::Simplestreams {
                return Err("pin an OCI image with a digest in the alias (name@sha256:…)".into());
            }
            if !is_sha256_hex(fp) {
                return Err("the image fingerprint must be 64 lowercase hex digits".into());
            }
            if self.architectures.len() > 1 {
                return Err("a fingerprint pins one image, which has one architecture".into());
            }
        }
        if let Some((_, digest)) = self.source.alias.split_once('@') {
            let pinned = self.source.protocol == ImageProtocol::Oci
                && digest.strip_prefix("sha256:").is_some_and(is_sha256_hex);
            if !pinned {
                return Err(
                    "an image digest must be @sha256:<64 lowercase hex digits> (OCI only)".into(),
                );
            }
        }
        if let Some(a) = self
            .architectures
            .iter()
            .find(|a| !ARCHITECTURES.contains(&a.as_str()))
        {
            return Err(format!("unknown architecture {a:?}"));
        }
        if let Some(o) = &self.owner {
            o.validate()?;
        }
        if let Some(n) = &self.network
            && let Some(bad) = [&n.network, &n.acl, &n.uplink]
                .into_iter()
                .find(|v| !is_valid_name(v))
        {
            return Err(format!("invalid network attachment name {bad:?}"));
        }
        if let Some(k) = self.config.keys().find(|k| k.starts_with("user.apelora.")) {
            return Err(format!("config key {k} is reserved for Apelora"));
        }
        storage::validate_volumes(&self.name, self.kind, &self.volumes)
    }

    /// [`validate`](Self::validate), plus what only Apelora may set: a caller's spec must not
    /// carry Apelora's discovery variables (`environment.APELORA_*`, ADR 0004).
    pub fn validate_from_caller(&self) -> Result<(), String> {
        self.validate()?;
        if let Some(k) = self
            .config
            .keys()
            .find(|k| k.starts_with("environment.APELORA_"))
        {
            return Err(format!("config key {k} is reserved for Apelora"));
        }
        if let Some(v) = self.volumes.iter().find(|v| v.pool.is_some()) {
            return Err(format!(
                "volume {}: the pool is chosen by Apelora from the volume class",
                v.name
            ));
        }
        Ok(())
    }
}

/// A single-use token that lets one new server join the Incus cluster. It carries a secret:
/// it is shown once to the administrator who asked for it and never stored or logged.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct JoinToken {
    /// Name the new member must join with.
    pub member_name: String,
    /// When Incus stops accepting it (RFC 3339, as Incus reports it).
    pub expires_at: String,
    /// The token to give `incus admin init` on the new server.
    pub token: String,
}

impl std::fmt::Debug for JoinToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JoinToken")
            .field("member_name", &self.member_name)
            .field("expires_at", &self.expires_at)
            .field("token", &"<redacted>")
            .finish()
    }
}

/// A join token that has been issued and not yet used, revoked or expired (no secret).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct PendingJoin {
    pub member_name: String,
    /// RFC 3339, as Incus reports it.
    pub expires_at: String,
}

/// A state change Apelora can request for one of its instances.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum InstanceAction {
    Start,
    Stop,
    /// Stop and start again (a running instance) in one Incus operation.
    Restart,
}

impl InstanceAction {
    pub fn as_str(self) -> &'static str {
        match self {
            InstanceAction::Start => "start",
            InstanceAction::Stop => "stop",
            InstanceAction::Restart => "restart",
        }
    }

    /// The Incus status an instance has once the action has taken effect.
    pub fn target_status(self) -> &'static str {
        match self {
            InstanceAction::Start | InstanceAction::Restart => "Running",
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
                fingerprint: None,
            },
            profiles: vec!["default".into()],
            config: Default::default(),
            start: true,
            architectures: vec![],
            owner: None,
            network: None,
            volumes: Vec::new(),
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
    fn images_are_pinned_by_fingerprint_or_digest() {
        let hex = "ab".repeat(32);
        let with = |protocol, alias: &str, fingerprint: Option<&str>, arch: &[&str]| {
            InstanceSpec {
                source: ImageSource {
                    protocol,
                    server: "https://images.example".into(),
                    alias: alias.into(),
                    fingerprint: fingerprint.map(str::to_owned),
                },
                architectures: arch.iter().map(|a| (*a).to_owned()).collect(),
                ..spec()
            }
            .validate()
        };
        use ImageProtocol::{Oci, Simplestreams};
        assert_eq!(
            with(Simplestreams, "debian/13", Some(&hex), &["aarch64"]),
            Ok(())
        );
        assert_eq!(
            with(
                Oci,
                "library/nginx:1.29",
                None,
                &["aarch64", "x86_64", "loongarch64"]
            ),
            Ok(())
        );
        assert_eq!(
            with(Oci, &format!("library/nginx@sha256:{hex}"), None, &[]),
            Ok(())
        );
        for bad in [
            with(Oci, "nginx:1.29", Some(&hex), &[]),
            with(Simplestreams, "debian/13", Some(&hex.to_uppercase()), &[]),
            with(Simplestreams, "debian/13", Some("abc"), &[]),
            with(Oci, "library/nginx@sha256:abc", None, &[]),
            with(Oci, &format!("library/nginx@md5:{hex}"), None, &[]),
            with(Simplestreams, &format!("debian@sha256:{hex}"), None, &[]),
            with(Simplestreams, "debian/13", None, &["amd64"]),
            with(
                Simplestreams,
                "debian/13",
                Some(&hex),
                &["aarch64", "x86_64"],
            ),
        ] {
            assert!(bad.is_err(), "{bad:?}");
        }
    }

    #[test]
    fn join_tokens_never_show_their_secret_in_debug_output() {
        let t = JoinToken {
            member_name: "m2".into(),
            expires_at: "2026-10-20T03:00:00Z".into(),
            token: "c2VjcmV0".into(),
        };
        let shown = format!("{t:?}");
        assert!(shown.contains("m2") && shown.contains("<redacted>"));
        assert!(!shown.contains("c2VjcmV0"));
    }

    #[test]
    fn evidence_labels_are_the_reporting_labels() {
        assert_eq!(json(Evidence::MockVerified), "\"MOCK_VERIFIED\"");
        assert_eq!(json(Evidence::RealIncusTested), "\"REAL_INCUS_TESTED\"");
    }
}
