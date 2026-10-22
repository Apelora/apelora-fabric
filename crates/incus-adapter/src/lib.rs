// SPDX-License-Identifier: AGPL-3.0-only
//! Boundary between Apelora and the Incus REST API.
//!
//! [`Provider`] is either the real read-only [`IncusClient`] (mutual TLS, pinned server
//! certificate) or [`MockIncus`], fixture data for development and tests that never
//! pretends to be a real cluster.
//!
//! Writes are instance lifecycle calls that start asynchronous Incus operations; callers record
//! the returned operation id and wait on it with [`Provider::wait_operation`]. Ownership checks
//! (only instances carrying [`apelora_domain::MANAGED_KEY`]) are made by the caller after
//! reading [`Provider::instance`], so the decision and its evidence stay together.

mod client;
pub mod config;
mod shell;
pub mod tenancy;
mod wire;

pub use client::{CertificateExpiry, IncusClient, IncusConfig, certificate_not_after};
pub use shell::{MAX_CHUNK, MockShell, ShellInput, ShellRequest, ShellSession};
pub use tenancy::{IncusProfile, MOCK_DEPLOY_FINGERPRINT, MockTenancy};
pub use wire::Operation;

use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};

use apelora_domain::capacity::HealthServerResources;
use apelora_domain::clustering::ClusterGroupInfo;
use apelora_domain::enrolment::HELD;
use apelora_domain::inspect::{
    InstanceView, InstanceViewAddress, InstanceViewDisk, InstanceViewImage, InstanceViewLimits,
    InstanceViewUsage, LogTail,
};
use apelora_domain::network::{AclRule, LoadBalancer};
use apelora_domain::storage::{VolumeMount, volume_name};
use apelora_domain::{
    ClusterMember, Instance, InstanceAction, InstanceKind, InstanceSpec, JoinToken, Network, Owner,
    PendingJoin, Project, ProviderMode, ServerInfo, StoragePool, is_valid_name,
};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AdapterError {
    #[error("not connected to an Incus server")]
    NotConfigured,
    #[error("invalid Incus connection settings: {0}")]
    InvalidConfig(String),
    #[error("cannot reach the Incus server: {0}")]
    Connectivity(String),
    #[error("the Incus server rejected our credentials: {0}")]
    Authentication(String),
    #[error("not found on the Incus server: {0}")]
    NotFound(String),
    #[error("Incus API error {status}: {message}")]
    Api { status: u16, message: String },
    #[error("unexpected response from the Incus server: {0}")]
    Decode(String),
    #[error("the Incus server does not support the `{0}` API extension")]
    UnsupportedCapability(String),
}

/// What lifecycle decisions need to know about an existing instance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstanceState {
    /// Incus status, e.g. `Running` or `Stopped`.
    pub status: String,
    /// Carries Apelora's ownership marker.
    pub managed: bool,
    /// Incus architecture name, e.g. `x86_64`.
    pub architecture: String,
    /// Fingerprint of the image the instance was built from (`volatile.base_image`).
    pub base_image: Option<String>,
    /// The member it runs on; empty (or `none`) on a standalone server.
    pub location: String,
    /// Names of the instance's own disk devices backed by a pool volume.
    pub volume_devices: Vec<String>,
    pub kind: InstanceKind,
    /// `migration.stateful=true`: a VM can move while running.
    pub stateful: bool,
    /// The resource limits set on the instance itself (`apelora_domain::plan::RESIZABLE_KEYS`).
    pub limits: std::collections::BTreeMap<String, String>,
}

/// A snapshot of a volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotInfo {
    pub name: String,
    /// RFC 3339, as Incus reports it.
    pub created_at: String,
}

/// What Apelora needs to know about an existing custom volume.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VolumeState {
    /// Carries Apelora's ownership marker.
    pub managed: bool,
    /// The application it was created for (`user.apelora.application`).
    pub application: Option<String>,
    /// `filesystem` or `block`.
    pub content: String,
    /// Size in MiB as configured (`size`), when it is set in a unit Apelora reads.
    pub size_mib: Option<u32>,
    pub location: String,
    /// Instances using it, as Incus API paths.
    pub used_by: Vec<String>,
}

/// A custom volume as Incus lists it in one pool (read-only views: storage, orphans).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CustomVolume {
    pub name: String,
    pub pool: String,
    /// Member of a member-local pool; `none` (or empty) otherwise.
    pub location: String,
    /// `filesystem` or `block`.
    pub content: String,
    pub size_mib: Option<u32>,
    /// Carries Apelora's ownership marker.
    pub managed: bool,
    /// `user.apelora.application`.
    pub application: Option<String>,
    /// `user.apelora.backup-of`: the volume it is a backup copy of.
    pub backup_of: Option<String>,
    /// Instances using it, as Incus API paths.
    pub used_by: Vec<String>,
    /// When Incus created it (RFC 3339), empty when not reported.
    pub created_at: String,
}

/// A network ACL as Incus reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedAcl {
    pub name: String,
    /// `user.apelora.application` when Apelora created it.
    pub application: Option<String>,
    pub ingress: Vec<AclRule>,
    pub egress: Vec<AclRule>,
}

/// A network address set as Incus reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedAddressSet {
    pub name: String,
    pub application: Option<String>,
    pub addresses: Vec<String>,
}

/// One backend of a load balancer as Incus reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedBackend {
    pub name: String,
    pub target_address: String,
    pub description: String,
}

/// A network load balancer (a service or published address) as Incus reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedLoadBalancer {
    pub listen_address: String,
    pub application: Option<String>,
    /// `user.apelora.module`.
    pub module: Option<String>,
    pub backends: Vec<ObservedBackend>,
    /// `(protocol, listen port)`.
    pub ports: Vec<(String, String)>,
}

/// A network as Incus reports it, with its configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedNetwork {
    pub name: String,
    pub kind: String,
    pub status: String,
    pub config: std::collections::BTreeMap<String, String>,
}

/// The result of waiting on an Incus operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationOutcome {
    Succeeded,
    Failed(String),
    /// Not finished within the wait budget; wait again later rather than repeating the call.
    StillRunning,
}

/// How long creating or attaching a volume may take before the job retries.
const VOLUME_BUDGET: Duration = Duration::from_secs(300);

/// Server-side wait per request; below the client's 20 s request timeout.
const WAIT_STEP_SECS: u32 = 15;

/// Image mirrors for this process (ADR 0010), set once at startup from the environment.
static IMAGE_MIRRORS: RwLock<Option<apelora_domain::images::ImageMirrors>> = RwLock::new(None);

/// Sets the image mirrors every instance creation goes through.
pub fn set_image_mirrors(m: apelora_domain::images::ImageMirrors) {
    *IMAGE_MIRRORS.write().unwrap_or_else(|e| e.into_inner()) = Some(m);
}

/// The spec with its image server replaced by this installation's mirror, or refused.
fn mirrored(spec: &InstanceSpec) -> Result<std::borrow::Cow<'_, InstanceSpec>, AdapterError> {
    let guard = IMAGE_MIRRORS.read().unwrap_or_else(|e| e.into_inner());
    let Some(m) = guard.as_ref().filter(|m| !m.is_empty()) else {
        return Ok(std::borrow::Cow::Borrowed(spec));
    };
    let source = m.apply(&spec.source).map_err(AdapterError::InvalidConfig)?;
    if source.server != spec.source.server {
        tracing::debug!(instance = %format!("{}/{}", spec.project, spec.name), from = %spec.source.server, to = %source.server, "image from mirror");
    }
    let mut out = spec.clone();
    out.source = source;
    Ok(std::borrow::Cow::Owned(out))
}

/// Where inventory comes from: a real Incus server or labelled mock data.
// `allow`, not `expect`: which variant is larger changes whenever either side gains a field.
#[allow(
    clippy::large_enum_variant,
    reason = "built once at startup and shared; its size does not matter"
)]
#[derive(Debug, Clone)]
pub enum Provider {
    Mock(MockIncus),
    Incus(IncusClient),
}

impl Provider {
    /// When the client and pinned server certificates expire (none for the mock).
    pub fn certificate_expiry(&self) -> Option<CertificateExpiry> {
        match self {
            Provider::Mock(_) => None,
            Provider::Incus(c) => c.certificate_expiry(),
        }
    }

    /// Starts an interactive program in a running instance Apelora owns (the mock answers with
    /// a labelled fake shell). Nothing here decides who may do it: the control plane does.
    pub async fn open_shell(&self, req: &ShellRequest) -> Result<ShellSession, AdapterError> {
        match self {
            Provider::Mock(m) => m.open_shell(req),
            Provider::Incus(c) => c.open_shell(req).await,
        }
    }

    pub fn mode(&self) -> ProviderMode {
        match self {
            Provider::Mock(_) => ProviderMode::Mock,
            Provider::Incus(_) => ProviderMode::Incus,
        }
    }

    pub async fn server_info(&self) -> Result<ServerInfo, AdapterError> {
        match self {
            Provider::Mock(m) => Ok(m.server.clone()),
            Provider::Incus(c) => c.server_info().await,
        }
    }

    /// Fails with [`AdapterError::UnsupportedCapability`] instead of assuming a feature exists.
    pub async fn require_extension(&self, name: &str) -> Result<(), AdapterError> {
        let info = self.server_info().await?;
        if info.api_extensions.iter().any(|e| e == name) {
            Ok(())
        } else {
            Err(AdapterError::UnsupportedCapability(name.to_owned()))
        }
    }

    /// Cluster members; a standalone server is reported as having none.
    pub async fn cluster_members(&self) -> Result<Vec<ClusterMember>, AdapterError> {
        match self {
            Provider::Mock(m) => Ok(m.members.lock().unwrap_or_else(|e| e.into_inner()).clone()),
            Provider::Incus(c) => {
                if c.server_info().await?.clustered {
                    c.cluster_members().await
                } else {
                    Ok(Vec::new())
                }
            }
        }
    }

    /// Cluster groups and their members; none on a standalone server.
    pub async fn cluster_groups(&self) -> Result<Vec<ClusterGroupInfo>, AdapterError> {
        match self {
            Provider::Mock(m) => Ok(m.groups()),
            Provider::Incus(c) => {
                if c.server_info().await?.clustered {
                    c.cluster_groups().await
                } else {
                    Ok(Vec::new())
                }
            }
        }
    }

    /// The values of `keys` in the server configuration; unset keys are absent. Incus shows
    /// the configuration only to an unrestricted identity (the restricted one gets none), so
    /// call this on the admin provider. Keys outside `keys` never leave the adapter.
    pub async fn server_config(
        &self,
        keys: &[&str],
    ) -> Result<std::collections::BTreeMap<String, String>, AdapterError> {
        match self {
            Provider::Mock(m) => Ok(m
                .config
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .filter(|(k, _)| keys.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect()),
            Provider::Incus(c) => c.server_config(keys).await,
        }
    }

    /// Changes cluster settings (a `None` removes the key, so Incus's default applies). Only the
    /// settings in `CLUSTER_SETTINGS`, with values `check_setting` accepts; call this on the admin
    /// provider. The mock changes its own configuration and nothing else.
    pub async fn set_server_config(
        &self,
        changes: &std::collections::BTreeMap<String, Option<String>>,
    ) -> Result<(), AdapterError> {
        match self {
            Provider::Mock(m) => {
                let body = client::settings_body(changes)?;
                let mut config = m.config.lock().unwrap_or_else(|e| e.into_inner());
                for (key, value) in body {
                    match value.as_str().filter(|v| !v.is_empty()) {
                        Some(v) => {
                            config.insert(key, v.to_owned());
                        }
                        None => {
                            config.remove(&key);
                        }
                    }
                }
                Ok(())
            }
            Provider::Incus(c) => c.set_server_config(changes).await,
        }
    }

    /// Incus's defaults for server configuration keys, as the server documents them.
    pub async fn config_defaults(
        &self,
    ) -> Result<std::collections::BTreeMap<String, String>, AdapterError> {
        match self {
            Provider::Mock(_) => Ok(mock_config_defaults()),
            Provider::Incus(c) => c.config_defaults().await,
        }
    }

    pub async fn instances(&self) -> Result<Vec<Instance>, AdapterError> {
        match self {
            Provider::Mock(m) => {
                // Like Incus: an instance on an offline member is listed without its
                // configuration, so its labels are unknown.
                let offline: Vec<String> = m
                    .members
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .filter(|x| x.status == "Offline")
                    .map(|x| x.name.clone())
                    .collect();
                Ok(m.lock()
                    .iter()
                    .map(|i| {
                        let mut out = i.instance.clone();
                        if offline.contains(&out.location) {
                            out.status = "Error".into();
                            out.managed = false;
                            out.owner = None;
                            out.unreachable = true;
                        }
                        out
                    })
                    .collect())
            }
            Provider::Incus(c) => c.instances().await,
        }
    }

    pub async fn projects(&self) -> Result<Vec<Project>, AdapterError> {
        match self {
            Provider::Mock(m) => Ok(m.projects.clone()),
            Provider::Incus(c) => c.projects().await,
        }
    }

    pub async fn networks(&self) -> Result<Vec<Network>, AdapterError> {
        match self {
            // Like a join: managed networks and pools are created on every member.
            Provider::Mock(m) => Ok(m
                .networks
                .iter()
                .cloned()
                .map(|mut n| {
                    n.locations = m.member_names();
                    n
                })
                .collect()),
            Provider::Incus(c) => c.networks().await,
        }
    }

    pub async fn storage_pools(&self) -> Result<Vec<StoragePool>, AdapterError> {
        match self {
            Provider::Mock(m) => Ok(m
                .pools
                .iter()
                .cloned()
                .map(|mut p| {
                    p.locations = m.member_names();
                    p
                })
                .collect()),
            Provider::Incus(c) => c.storage_pools().await,
        }
    }

    /// `None` when the instance does not exist.
    pub async fn instance(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Option<InstanceState>, AdapterError> {
        match self {
            Provider::Mock(m) => Ok(m.find(project, name).map(|i| InstanceState {
                status: i.instance.status.clone(),
                managed: i.instance.managed,
                architecture: i.instance.architecture.clone(),
                base_image: i.base_image.clone(),
                location: i.instance.location.clone(),
                volume_devices: i.volume_devices.clone(),
                kind: i.instance.kind,
                stateful: i.stateful,
                limits: i.instance.limits.clone(),
            })),
            Provider::Incus(c) => c.instance(project, name).await,
        }
    }

    /// One instance in detail for its page; `None` when it does not exist. Names must already
    /// be validated.
    pub async fn instance_view(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Option<InstanceView>, AdapterError> {
        match self {
            Provider::Mock(m) => Ok(m.find(project, name).map(|i| m.view(&i))),
            Provider::Incus(c) => c.instance_view(project, name).await,
        }
    }

    /// The names of the instance's log files, as Incus lists them.
    pub async fn instance_logs(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Vec<String>, AdapterError> {
        match self {
            Provider::Mock(m) => m
                .find(project, name)
                .map(|i| vec![mock_log_name(&i).to_owned()])
                .ok_or_else(|| AdapterError::NotFound(format!("instance {project}/{name}"))),
            Provider::Incus(c) => c.instance_logs(project, name).await,
        }
    }

    /// The last `max` bytes of one of the instance's log files.
    pub async fn instance_log(
        &self,
        project: &str,
        name: &str,
        file: &str,
        max: usize,
    ) -> Result<LogTail, AdapterError> {
        match self {
            Provider::Mock(m) => {
                let i = m
                    .find(project, name)
                    .ok_or_else(|| AdapterError::NotFound(format!("instance {project}/{name}")))?;
                if file != mock_log_name(&i) {
                    return Err(AdapterError::NotFound(format!("log {file}")));
                }
                let text = format!(
                    "Mock · fixture log of {project}/{name}; not read from Incus\n\
                     Mock · {} is {}\n",
                    i.instance.name, i.instance.status
                );
                Ok(LogTail::from_bytes(text.as_bytes(), text.len() as u64, max))
            }
            Provider::Incus(c) => c.instance_log(project, name, file, max).await,
        }
    }

    /// The last `max` bytes of the instance's console log.
    pub async fn console_log(
        &self,
        project: &str,
        name: &str,
        max: usize,
    ) -> Result<LogTail, AdapterError> {
        match self {
            Provider::Mock(m) => {
                let i = m
                    .find(project, name)
                    .ok_or_else(|| AdapterError::NotFound(format!("instance {project}/{name}")))?;
                let text = format!(
                    "Mock · console of {project}/{name}; not read from Incus\nMock · {} reached {}\n",
                    i.instance.name,
                    if i.instance.status == "Running" {
                        "its login prompt"
                    } else {
                        "shutdown"
                    }
                );
                Ok(LogTail::from_bytes(text.as_bytes(), text.len() as u64, max))
            }
            Provider::Incus(c) => c.console_log(project, name, max).await,
        }
    }

    /// Starts creating an instance (validated spec) and returns the Incus operation id.
    pub async fn create_instance(&self, spec: &InstanceSpec) -> Result<String, AdapterError> {
        spec.validate().map_err(AdapterError::InvalidConfig)?;
        let spec = mirrored(spec)?;
        match self {
            Provider::Mock(m) => m.create(&spec),
            Provider::Incus(c) => c.create_instance(&spec).await,
        }
    }

    /// Changes an Apelora-owned instance's resource limits in place and returns the operation, or
    /// `None` when Incus has already applied it (it answers at once for a running container):
    /// `Some` sets a limit, `None` removes it so none applies. Only the keys in
    /// `apelora_domain::plan::RESIZABLE_KEYS` with plain values are accepted; nothing else on
    /// the instance can be changed through this call.
    pub async fn set_instance_limits(
        &self,
        project: &str,
        name: &str,
        changes: &std::collections::BTreeMap<String, Option<String>>,
    ) -> Result<Option<String>, AdapterError> {
        let config = client::limits_body(changes)?;
        match self {
            Provider::Mock(m) => {
                let mut all = m.lock();
                let i = all
                    .iter_mut()
                    .find(|i| i.instance.project == project && i.instance.name == name)
                    .ok_or_else(|| AdapterError::NotFound(format!("instance {project}/{name}")))?;
                if !i.instance.managed {
                    return Err(AdapterError::Api {
                        status: 409,
                        message: "the instance is not managed by Apelora".into(),
                    });
                }
                for (k, v) in config {
                    match v.as_str().filter(|v| !v.is_empty()) {
                        Some(v) => {
                            i.instance.limits.insert(k, v.to_owned());
                        }
                        None => {
                            i.instance.limits.remove(&k);
                        }
                    }
                }
                drop(all);
                Ok(Some(m.op()))
            }
            Provider::Incus(c) => c.set_instance_limits(project, name, config).await,
        }
    }

    pub async fn change_state(
        &self,
        project: &str,
        name: &str,
        action: InstanceAction,
        force: bool,
    ) -> Result<String, AdapterError> {
        match self {
            Provider::Mock(m) => {
                if action == InstanceAction::Restart {
                    m.net().restarts.push(name.to_owned());
                }
                if action == InstanceAction::Start
                    && let Some(n) = m
                        .lock_refusals
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .get_mut(name)
                        .filter(|n| **n > 0)
                {
                    *n -= 1;
                    return Err(AdapterError::Api {
                        status: 500,
                        message: format!(
                            "Failed to activate LVM logical volume: exit status 5 (LV locked by other host: {name})"
                        ),
                    });
                }
                m.set_status(project, name, action.target_status())
            }
            Provider::Incus(c) => c.change_state(project, name, action, force).await,
        }
    }

    pub async fn delete_instance(&self, project: &str, name: &str) -> Result<String, AdapterError> {
        match self {
            Provider::Mock(m) => m.delete(project, name),
            Provider::Incus(c) => c.delete_instance(project, name).await,
        }
    }

    /// Asks Incus for a single-use token letting `member` join the cluster. Needs an
    /// unrestricted identity (see `config::admin_provider_from_env`). The token is a secret.
    pub async fn create_join_token(&self, member: &str) -> Result<JoinToken, AdapterError> {
        if !is_valid_name(member) {
            return Err(AdapterError::InvalidConfig(format!(
                "invalid member name {member:?}"
            )));
        }
        match self {
            Provider::Mock(m) => m.create_join_token(member),
            Provider::Incus(c) => c.create_join_token(member).await,
        }
    }

    /// Join tokens issued and not yet used, revoked or expired.
    pub async fn join_tokens(&self) -> Result<Vec<PendingJoin>, AdapterError> {
        match self {
            Provider::Mock(m) => Ok(m
                .join_tokens
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()),
            Provider::Incus(c) => c.join_tokens().await,
        }
    }

    /// Revokes the pending join token for `member`; `false` when there is none.
    pub async fn revoke_join_token(&self, member: &str) -> Result<bool, AdapterError> {
        if !is_valid_name(member) {
            return Err(AdapterError::InvalidConfig(format!(
                "invalid member name {member:?}"
            )));
        }
        match self {
            Provider::Mock(m) => {
                let mut all = m.join_tokens.lock().unwrap_or_else(|e| e.into_inner());
                let before = all.len();
                all.retain(|t| t.member_name != member);
                Ok(all.len() != before)
            }
            Provider::Incus(c) => c.revoke_join_token(member).await,
        }
    }

    /// Publishes service names on an application network: `dns.domain` is
    /// `<application>.internal` and `user.apelora.dns` is one `name address` per line.
    /// The existing configuration is kept (a replace would drop the uplink and the addresses
    /// Incus recorded). NOT REAL INCUS TESTED.
    pub async fn publish_service_dns(
        &self,
        project: &str,
        network: &str,
        application: &str,
        records: &str,
    ) -> Result<(), AdapterError> {
        valid_names(&[project, network, application])?;
        match self {
            Provider::Mock(m) => {
                m.net()
                    .dns
                    .insert(format!("{project}/{network}"), records.to_owned());
                Ok(())
            }
            Provider::Incus(c) => {
                c.publish_service_dns(project, network, application, records)
                    .await
            }
        }
    }

    /// Network objects of an application (OVN network, module ACLs and address sets).
    /// Each is created only when missing and labelled with its application; one that exists
    /// for another application or without Apelora's marker is refused, never adopted.
    pub async fn ensure_network(
        &self,
        project: &str,
        name: &str,
        uplink: &str,
        application: &str,
    ) -> Result<(), AdapterError> {
        match self {
            Provider::Mock(m) => m
                .net()
                .ensure(NetworkObject::Network, project, name, application),
            Provider::Incus(c) => c.ensure_network(project, name, uplink, application).await,
        }
    }

    pub async fn ensure_acl(
        &self,
        project: &str,
        name: &str,
        application: &str,
    ) -> Result<(), AdapterError> {
        match self {
            Provider::Mock(m) => m
                .net()
                .ensure(NetworkObject::Acl, project, name, application),
            Provider::Incus(c) => c.ensure_acl(project, name, application).await,
        }
    }

    pub async fn set_acl_rules(
        &self,
        project: &str,
        name: &str,
        ingress: &[AclRule],
        egress: &[AclRule],
        application: &str,
    ) -> Result<(), AdapterError> {
        match self {
            Provider::Mock(m) => {
                let mut net = m.net();
                net.ensure(NetworkObject::Acl, project, name, application)?;
                net.acls.insert(
                    format!("{project}/{name}"),
                    (ingress.to_vec(), egress.to_vec()),
                );
                Ok(())
            }
            Provider::Incus(c) => {
                c.set_acl_rules(project, name, ingress, egress, application)
                    .await
            }
        }
    }

    pub async fn ensure_address_set(
        &self,
        project: &str,
        name: &str,
        application: &str,
    ) -> Result<(), AdapterError> {
        match self {
            Provider::Mock(m) => {
                m.net()
                    .ensure(NetworkObject::AddressSet, project, name, application)
            }
            Provider::Incus(c) => c.ensure_address_set(project, name, application).await,
        }
    }

    pub async fn set_addresses(
        &self,
        project: &str,
        name: &str,
        addresses: &[String],
        application: &str,
    ) -> Result<(), AdapterError> {
        match self {
            Provider::Mock(m) => {
                let mut net = m.net();
                net.ensure(NetworkObject::AddressSet, project, name, application)?;
                net.address_sets
                    .insert(format!("{project}/{name}"), addresses.to_vec());
                Ok(())
            }
            Provider::Incus(c) => c.set_addresses(project, name, addresses, application).await,
        }
    }

    /// Names of the network objects of one kind that Apelora created for `application`.
    pub async fn owned_network_objects(
        &self,
        kind: NetworkObject,
        project: &str,
        application: &str,
    ) -> Result<Vec<String>, AdapterError> {
        match self {
            Provider::Mock(m) => {
                let prefix = format!("{kind:?}:{project}/");
                Ok(m.net()
                    .owners
                    .iter()
                    .filter(|(k, app)| k.starts_with(&prefix) && app.as_str() == application)
                    .map(|(k, _)| k[prefix.len()..].to_owned())
                    .collect())
            }
            Provider::Incus(c) => c.owned_names(kind.collection(), project, application).await,
        }
    }

    /// Deletes a network object Apelora created for `application`; `false` when it did not
    /// exist. Incus refuses while it is in use (an ACL referencing an address set, a NIC on a
    /// network).
    pub async fn delete_network_object(
        &self,
        kind: NetworkObject,
        project: &str,
        name: &str,
        application: &str,
    ) -> Result<bool, AdapterError> {
        match self {
            Provider::Mock(m) => m.net().delete(kind, project, name, application),
            Provider::Incus(c) => {
                c.delete_owned(kind.collection(), project, name, application)
                    .await
            }
        }
    }

    /// One member's own server facts (Incus and kernel version). Needs a valid member name.
    pub async fn member_server_info(&self, member: &str) -> Result<ServerInfo, AdapterError> {
        valid_member(member)?;
        match self {
            Provider::Mock(m) => {
                m.member(member)?;
                let mut info = m.server.clone();
                if let Some(v) = m
                    .member_versions
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(member)
                {
                    info.server_version = Some(v.clone());
                }
                Ok(info)
            }
            Provider::Incus(c) => c.member_server_info(member).await,
        }
    }

    /// A server's CPU, memory, load and storage-pool space, read-only. `member` is the cluster
    /// member to read; `None` reads a standalone server. The mock answers with labelled
    /// fixture numbers and fails for an offline member, as Incus does.
    pub async fn server_resources(
        &self,
        member: Option<&str>,
    ) -> Result<apelora_domain::capacity::HealthServerResources, AdapterError> {
        if let Some(m) = member {
            valid_member(m)?;
        }
        match self {
            Provider::Mock(m) => m.server_resources(member),
            Provider::Incus(c) => c.server_resources(member).await,
        }
    }

    /// Holds a member out of automatic placement, or makes it eligible again (Phase 09).
    /// Needs an unrestricted identity.
    pub async fn set_member_held(&self, member: &str, held: bool) -> Result<(), AdapterError> {
        valid_member(member)?;
        match self {
            Provider::Mock(m) => {
                let mut members = m.members.lock().unwrap_or_else(|e| e.into_inner());
                let found = members
                    .iter_mut()
                    .find(|x| x.name == member)
                    .ok_or_else(|| AdapterError::NotFound(format!("cluster member {member}")))?;
                found.scheduler = if held { HELD } else { "all" }.into();
                Ok(())
            }
            Provider::Incus(c) => c.set_member_held(member, held).await,
        }
    }

    /// Evacuates (or restores) a cluster member through Incus; returns the operation.
    pub async fn member_state(&self, member: &str, restore: bool) -> Result<String, AdapterError> {
        match self {
            Provider::Mock(m) => {
                let mut members = m.members.lock().unwrap_or_else(|e| e.into_inner());
                let found = members
                    .iter_mut()
                    .find(|x| x.name == member)
                    .ok_or_else(|| AdapterError::NotFound(format!("cluster member {member}")))?;
                found.status = if restore { "Online" } else { "Evacuated" }.into();
                drop(members);
                Ok(m.op())
            }
            Provider::Incus(c) => c.member_state(member, restore).await,
        }
    }

    /// Moves an instance to another member (`live`: while it runs; Incus refuses unless it is a
    /// VM with `migration.stateful`). The project must allow member targeting
    /// (`restricted.cluster.target=allow`) for a restricted identity. Returns the operation.
    pub async fn move_instance(
        &self,
        project: &str,
        name: &str,
        target: &str,
        live: bool,
    ) -> Result<String, AdapterError> {
        valid_names(&[project, name])?;
        valid_member(target)?;
        match self {
            Provider::Mock(m) => {
                let offline: Vec<String> = m
                    .members
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .filter(|x| x.status == "Offline")
                    .map(|x| x.name.clone())
                    .collect();
                let shared = |pool: &str| {
                    m.pools.iter().any(|p| {
                        p.name == pool && apelora_domain::storage::is_shared_driver(&p.driver)
                    })
                };
                let user = format!("/1.0/instances/{name}?project={project}");
                let local_volume = m
                    .storage()
                    .volumes
                    .iter()
                    .find(|(k, v)| {
                        v.used_by.contains(&user)
                            && !shared(k.split('/').nth(1).unwrap_or_default())
                    })
                    .map(|(k, _)| k.clone());
                let mut all = m.lock();
                let i = all
                    .iter_mut()
                    .find(|i| i.instance.project == project && i.instance.name == name)
                    .ok_or_else(|| AdapterError::NotFound(format!("instance {project}/{name}")))?;
                if offline.contains(&i.instance.location) {
                    // Like Incus: away from an offline member only on shared storage, and only
                    // its records move (it is not running anywhere afterwards).
                    if !i.root_pool.as_deref().is_some_and(shared) || local_volume.is_some() {
                        return Err(AdapterError::Api {
                            status: 500,
                            message: format!(
                                "Failed to migrate instance {name}: the member {} is offline and its storage is not shared",
                                i.instance.location
                            ),
                        });
                    }
                    if live || offline.iter().any(|o| o == target) {
                        return Err(AdapterError::Api {
                            status: 400,
                            message: "Target cluster member is offline or a live move was asked"
                                .into(),
                        });
                    }
                    i.instance.location = target.to_owned();
                    i.instance.status = "Stopped".into();
                    drop(all);
                    return Ok(m.op());
                }
                let running = i.instance.status == "Running";
                if running
                    && !(live && i.stateful && i.instance.kind == InstanceKind::VirtualMachine)
                {
                    return Err(AdapterError::Api {
                        status: 400,
                        message: "Instance must be stopped to be moved".into(),
                    });
                }
                let from = std::mem::replace(&mut i.instance.location, target.to_owned());
                drop(all);
                // Its devices now resolve to the same-named volumes on the new member.
                let user = format!("/1.0/instances/{name}?project={project}");
                let mut st = m.storage();
                let moved: Vec<String> = st
                    .volumes
                    .iter_mut()
                    .filter(|(k, v)| {
                        k.starts_with(&format!("{project}/"))
                            && k.contains(&format!("/{from}/"))
                            && v.used_by.contains(&user)
                    })
                    .map(|(k, v)| {
                        v.used_by.retain(|u| *u != user);
                        k.replacen(&format!("/{from}/"), &format!("/{target}/"), 1)
                    })
                    .collect();
                for k in moved {
                    if let Some(v) = st.volumes.get_mut(&k) {
                        v.used_by.push(user.clone());
                    }
                }
                drop(st);
                Ok(m.op())
            }
            Provider::Incus(c) => c.move_instance(project, name, target, live).await,
        }
    }

    /// Removes a member from the cluster for good. Needs an unrestricted identity. The caller
    /// has checked that nothing is left on it (see `apelora_domain::removal`).
    pub async fn remove_member(&self, member: &str) -> Result<(), AdapterError> {
        valid_member(member)?;
        let op = match self {
            Provider::Mock(m) => {
                let mut members = m.members.lock().unwrap_or_else(|e| e.into_inner());
                let before = members.len();
                members.retain(|x| x.name != member);
                if members.len() == before {
                    return Err(AdapterError::NotFound(format!("cluster member {member}")));
                }
                None
            }
            Provider::Incus(c) => c.remove_member(member).await?,
        };
        self.finish(op, &format!("removing member {member}")).await
    }

    /// The instance's identity (`volatile.uuid`); `None` when Incus does not report one.
    pub async fn instance_uuid(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Option<String>, AdapterError> {
        match self {
            Provider::Mock(m) => m
                .find(project, name)
                .map(|i| Some(i.uuid))
                .ok_or_else(|| AdapterError::NotFound(format!("instance {project}/{name}"))),
            Provider::Incus(c) => c.instance_uuid(project, name).await,
        }
    }

    /// Creates or updates a module's service address (an OVN load balancer); `true` when
    /// it changed.
    pub async fn ensure_load_balancer(
        &self,
        project: &str,
        network: &str,
        lb: &LoadBalancer,
        application: &str,
    ) -> Result<bool, AdapterError> {
        match self {
            Provider::Mock(m) => {
                let mut net = m.net();
                let key = format!("{project}/{network}/{}", lb.listen_address);
                if let Some((app, _)) = net.load_balancers.get(&key)
                    && app != application
                {
                    return Err(AdapterError::Api {
                        status: 409,
                        message: format!(
                            "load balancer {} belongs to application {app}",
                            lb.listen_address
                        ),
                    });
                }
                let changed = net
                    .load_balancers
                    .get(&key)
                    .is_none_or(|(_, old)| old != lb);
                net.load_balancers
                    .insert(key, (application.to_owned(), lb.clone()));
                Ok(changed)
            }
            Provider::Incus(c) => {
                c.ensure_load_balancer(project, network, lb, application)
                    .await
            }
        }
    }

    /// Each backend's health check status, as `(backend, status)`.
    pub async fn load_balancer_health(
        &self,
        project: &str,
        network: &str,
        address: &str,
    ) -> Result<Vec<(String, String)>, AdapterError> {
        match self {
            Provider::Mock(m) => {
                let net = m.net();
                Ok(net
                    .load_balancers
                    .get(&format!("{project}/{network}/{address}"))
                    .map(|(_, lb)| {
                        let failing = &net.failing;
                        lb.backends
                            .iter()
                            .map(|b| {
                                let instance = b.description.split(' ').next().unwrap_or_default();
                                let status = if failing.contains(instance) {
                                    "offline"
                                } else {
                                    "online"
                                };
                                (b.name.clone(), status.to_owned())
                            })
                            .collect()
                    })
                    .unwrap_or_default())
            }
            Provider::Incus(c) => c.load_balancer_health(project, network, address).await,
        }
    }

    /// Re-applies a load balancer's health check (see the client's documentation).
    pub async fn reapply_health_check(
        &self,
        project: &str,
        network: &str,
        address: &str,
        application: &str,
    ) -> Result<(), AdapterError> {
        match self {
            Provider::Mock(m) => {
                m.net().health_check_repairs += 1;
                Ok(())
            }
            Provider::Incus(c) => {
                c.reapply_health_check(project, network, address, application)
                    .await
            }
        }
    }

    /// The application's load balancers on `network`, as `(listen address, module)`.
    pub async fn owned_load_balancers(
        &self,
        project: &str,
        network: &str,
        application: &str,
    ) -> Result<Vec<(String, String)>, AdapterError> {
        match self {
            Provider::Mock(m) => {
                let prefix = format!("{project}/{network}/");
                Ok(m.net()
                    .load_balancers
                    .iter()
                    .filter(|(k, (app, _))| k.starts_with(&prefix) && app == application)
                    .map(|(k, (_, lb))| (k[prefix.len()..].to_owned(), lb.module.clone()))
                    .collect())
            }
            Provider::Incus(c) => c.owned_load_balancers(project, network, application).await,
        }
    }

    pub async fn delete_load_balancer(
        &self,
        project: &str,
        network: &str,
        address: &str,
        application: &str,
    ) -> Result<bool, AdapterError> {
        match self {
            Provider::Mock(m) => {
                let mut net = m.net();
                let key = format!("{project}/{network}/{address}");
                match net.load_balancers.get(&key) {
                    None => Ok(false),
                    Some((app, _)) if app != application => Err(AdapterError::Api {
                        status: 409,
                        message: format!("load balancer {address} belongs to application {app}"),
                    }),
                    Some(_) => Ok(net.load_balancers.remove(&key).is_some()),
                }
            }
            Provider::Incus(c) => {
                c.delete_load_balancer(project, network, address, application)
                    .await
            }
        }
    }

    /// Runs `command` (an argument list, no shell) inside a running, Apelora-owned instance as the
    /// numeric user `uid`, waits at most `timeout_secs` (1–15) and reports how it ended. The
    /// command's output is discarded. The caller decides what the exit status means.
    pub async fn run_probe(
        &self,
        project: &str,
        name: &str,
        command: &[String],
        uid: u32,
        timeout_secs: u8,
    ) -> Result<ProbeRun, AdapterError> {
        if !is_valid_name(project) || !is_valid_name(name) || command.is_empty() {
            return Err(AdapterError::InvalidConfig(
                "invalid project, instance or command".into(),
            ));
        }
        let timeout_secs = timeout_secs.clamp(1, 15);
        match self {
            Provider::Mock(m) => {
                let found = m
                    .instances
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .iter()
                    .find(|i| i.instance.project == project && i.instance.name == name)
                    .map(|i| (i.instance.managed, i.instance.status.clone()));
                match found {
                    None => {
                        return Err(AdapterError::NotFound(format!("instance {project}/{name}")));
                    }
                    Some((false, _)) => {
                        return Err(AdapterError::Api {
                            status: 409,
                            message: "the instance is not managed by Apelora".into(),
                        });
                    }
                    Some((true, status)) if status != "Running" => {
                        return Err(AdapterError::Api {
                            status: 409,
                            message: format!("the instance is {}", status.to_lowercase()),
                        });
                    }
                    Some(_) => {}
                }
                m.probes_run
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push((name.to_owned(), command.to_vec()));
                let behaviour = m
                    .probe_results
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .get(name)
                    .cloned()
                    .unwrap_or(ProbeBehaviour::Exit(0));
                Ok(match behaviour {
                    ProbeBehaviour::Exit(code) => ProbeRun {
                        exit: Some(code),
                        timed_out: false,
                        error: None,
                    },
                    ProbeBehaviour::TimesOut => ProbeRun {
                        exit: None,
                        timed_out: true,
                        error: None,
                    },
                    ProbeBehaviour::CannotRun(why) => ProbeRun {
                        exit: None,
                        timed_out: false,
                        error: Some(why),
                    },
                })
            }
            Provider::Incus(c) => c.run_probe(project, name, command, uid, timeout_secs).await,
        }
    }

    /// A running instance's global addresses; empty while it has none (e.g. stopped).
    pub async fn instance_addresses(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Vec<String>, AdapterError> {
        match self {
            // Documentation-range addresses so mock data can never be mistaken for real ones.
            Provider::Mock(m) => {
                let found = m.lock().iter().position(|i| {
                    i.instance.project == project
                        && i.instance.name == name
                        && i.instance.status == "Running"
                });
                let net = m.net();
                Ok(found
                    .map(|n| {
                        let mut a = vec![format!("198.51.100.{}/32", n + 1)];
                        if net.ipv6 && !net.no_ipv6_yet.contains(name) {
                            a.push(format!("2001:db8:a::{:x}/128", n + 1));
                        }
                        a.sort();
                        a
                    })
                    .unwrap_or_default())
            }
            Provider::Incus(c) => c.instance_addresses(project, name).await,
        }
    }

    /// The volume `mount` asks for, for `instance` on `member` (where the instance runs):
    /// created when missing and labelled with its owner. One that exists for another
    /// application or without Apelora's marker is refused, never adopted.
    pub async fn ensure_volume(
        &self,
        project: &str,
        member: &str,
        instance: &str,
        mount: &VolumeMount,
        owner: &Owner,
    ) -> Result<(), AdapterError> {
        let (pool, name, member) = volume_target(project, member, instance, mount)?;
        match self.volume(project, &pool, member, &name).await? {
            Some(v) if v.managed && v.application.as_deref() == Some(&owner.application) => {
                return Ok(());
            }
            Some(_) => {
                return Err(AdapterError::Api {
                    status: 409,
                    message: format!(
                        "volume {name} exists but was not created by Apelora for application {}",
                        owner.application
                    ),
                });
            }
            None => {}
        }
        let op = match self {
            Provider::Mock(m) => m.create_volume(project, member, &name, mount, owner)?,
            Provider::Incus(c) => {
                c.create_volume(project, member, &name, mount, owner)
                    .await?
            }
        };
        self.finish(op, &format!("creating volume {name}")).await
    }

    /// Attaches the instance's volume as its disk device (see [`VolumeMount::device`]).
    pub async fn attach_volume(
        &self,
        project: &str,
        instance: &str,
        mount: &VolumeMount,
    ) -> Result<(), AdapterError> {
        let (_, name, _) = volume_target(project, "", instance, mount)?;
        let op = match self {
            Provider::Mock(m) => m.attach_volume(project, instance, &name, mount)?,
            Provider::Incus(c) => c.attach_volume(project, instance, &name, mount).await?,
        };
        self.finish(op, &format!("attaching volume {name}")).await
    }

    /// A custom volume, `None` if it does not exist. `member` only for member-local pools.
    pub async fn volume(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
    ) -> Result<Option<VolumeState>, AdapterError> {
        valid_names(&[project, pool, name])?;
        if let Some(m) = member {
            valid_member(m)?;
        }
        match self {
            Provider::Mock(m) => Ok(m
                .storage()
                .volumes
                .get(&mock_key(project, pool, member, name))
                .cloned()),
            Provider::Incus(c) => c.volume(project, pool, member, name).await,
        }
    }

    /// Grows a volume Apelora created for `application` to `size_mib`. Grow-only: Incus accepts
    /// a shrink, which can destroy data, so a smaller size is refused here. Works while the
    /// volume is attached and its instance runs.
    pub async fn grow_volume(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
        application: &str,
        size_mib: u32,
    ) -> Result<(), AdapterError> {
        let v = self
            .volume(project, pool, member, name)
            .await?
            .ok_or_else(|| AdapterError::NotFound(format!("volume {name}")))?;
        if !v.managed || v.application.as_deref() != Some(application) {
            return Err(AdapterError::Api {
                status: 409,
                message: format!("volume {name} does not belong to application {application}"),
            });
        }
        let current = v.size_mib.ok_or_else(|| {
            AdapterError::Decode(format!("volume {name} has a size Apelora cannot read"))
        })?;
        apelora_domain::storage::check_resize(current, size_mib).map_err(|message| {
            AdapterError::Api {
                status: 400,
                message,
            }
        })?;
        if size_mib == current {
            return Ok(());
        }
        let op = match self {
            Provider::Mock(m) => {
                if let Some(v) = m
                    .storage()
                    .volumes
                    .get_mut(&mock_key(project, pool, member, name))
                {
                    v.size_mib = Some(size_mib);
                }
                None
            }
            Provider::Incus(c) => {
                c.set_volume_size(project, pool, member, name, size_mib)
                    .await?
            }
        };
        self.finish(op, &format!("growing volume {name}")).await
    }

    /// The volume's snapshots, oldest first.
    pub async fn volume_snapshots(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
    ) -> Result<Vec<SnapshotInfo>, AdapterError> {
        valid_names(&[project, pool, name])?;
        if let Some(m) = member {
            valid_member(m)?;
        }
        match self {
            Provider::Mock(m) => Ok(m
                .storage()
                .snapshots
                .get(&mock_key(project, pool, member, name))
                .cloned()
                .unwrap_or_default()),
            Provider::Incus(c) => c.volume_snapshots(project, pool, member, name).await,
        }
    }

    /// Takes a snapshot of a volume Apelora created for `application`. Incus refuses unless
    /// the project allows snapshots (`restricted.snapshots=allow`). While its instance runs the
    /// snapshot is only crash-consistent.
    pub async fn snapshot_volume(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
        application: &str,
        snapshot: &str,
    ) -> Result<(), AdapterError> {
        self.owned_volume(project, pool, member, name, application)
            .await?;
        valid_names(&[snapshot])?;
        let op = match self {
            Provider::Mock(m) => {
                let mut st = m.storage();
                let list = st
                    .snapshots
                    .entry(mock_key(project, pool, member, name))
                    .or_default();
                if list.iter().any(|s| s.name == snapshot) {
                    return Err(AdapterError::Api {
                        status: 409,
                        message: "Snapshot already exists".into(),
                    });
                }
                list.push(SnapshotInfo {
                    name: snapshot.to_owned(),
                    created_at: format!("mock: snapshot {}", list.len() + 1),
                });
                None
            }
            Provider::Incus(c) => {
                c.snapshot_volume(project, pool, member, name, snapshot)
                    .await?
            }
        };
        self.finish(op, &format!("snapshotting volume {name}"))
            .await
    }

    /// Puts a volume back to one of its snapshots: its data since then is lost. Incus refuses
    /// while a running instance uses the volume.
    pub async fn restore_volume(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
        application: &str,
        snapshot: &str,
    ) -> Result<(), AdapterError> {
        let v = self
            .owned_volume(project, pool, member, name, application)
            .await?;
        valid_names(&[snapshot])?;
        if !self
            .volume_snapshots(project, pool, member, name)
            .await?
            .iter()
            .any(|s| s.name == snapshot)
        {
            return Err(AdapterError::NotFound(format!(
                "snapshot {snapshot} of volume {name}"
            )));
        }
        let op = match self {
            Provider::Mock(m) => {
                if !v.used_by.is_empty() && m.storage().fail_attach.contains("running") {
                    return Err(AdapterError::Api {
                        status: 400,
                        message: "Cannot restore custom volume used by running instances".into(),
                    });
                }
                m.storage().restores.push(format!("{name}/{snapshot}"));
                None
            }
            Provider::Incus(c) => {
                c.restore_volume(project, pool, member, name, snapshot)
                    .await?
            }
        };
        self.finish(op, &format!("restoring volume {name}")).await
    }

    /// Deletes one snapshot of a volume Apelora created for `application`.
    pub async fn delete_snapshot(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
        application: &str,
        snapshot: &str,
    ) -> Result<(), AdapterError> {
        self.owned_volume(project, pool, member, name, application)
            .await?;
        valid_names(&[snapshot])?;
        let op = match self {
            Provider::Mock(m) => {
                let mut st = m.storage();
                let list = st
                    .snapshots
                    .entry(mock_key(project, pool, member, name))
                    .or_default();
                let before = list.len();
                list.retain(|s| s.name != snapshot);
                if list.len() == before {
                    return Err(AdapterError::NotFound(format!("snapshot {snapshot}")));
                }
                None
            }
            Provider::Incus(c) => {
                c.delete_snapshot(project, pool, member, name, snapshot)
                    .await?
            }
        };
        self.finish(op, &format!("deleting snapshot {snapshot}"))
            .await
    }

    /// Copies the application's volume, as it was at `snapshot`, to a new volume `to` in
    /// `to_pool` (on `to_member` for member-local pools), labelled like the source plus
    /// `user.apelora.backup-of`. Used for backups and for moving a volume to another member.
    #[allow(clippy::too_many_arguments)]
    pub async fn copy_volume(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
        application: &str,
        snapshot: Option<&str>,
        to_pool: &str,
        to_member: Option<&str>,
        to: &str,
        backup_of: Option<&str>,
    ) -> Result<(), AdapterError> {
        let v = self
            .owned_volume(project, pool, member, name, application)
            .await?;
        valid_names(&[to_pool, to])?;
        if let Some(s) = snapshot {
            valid_names(&[s])?;
        }
        if let Some(m) = to_member {
            valid_member(m)?;
        }
        let size = v.size_mib.ok_or_else(|| {
            AdapterError::Decode(format!("volume {name} has a size Apelora cannot read"))
        })?;
        let mut config = std::collections::BTreeMap::from([
            ("size".to_owned(), format!("{size}MiB")),
            (apelora_domain::MANAGED_KEY.to_owned(), "true".to_owned()),
            (
                apelora_domain::APPLICATION_KEY.to_owned(),
                application.to_owned(),
            ),
        ]);
        if let Some(b) = backup_of {
            config.insert(
                apelora_domain::storage::BACKUP_OF_KEY.to_owned(),
                b.to_owned(),
            );
        }
        let op = match self {
            Provider::Mock(m) => {
                let mut st = m.storage();
                if let Some(s) = snapshot
                    && !st
                        .snapshots
                        .get(&mock_key(project, pool, member, name))
                        .is_some_and(|l| l.iter().any(|x| x.name == s))
                {
                    return Err(AdapterError::NotFound(format!(
                        "snapshot {s} of volume {name}"
                    )));
                }
                if st.fail_create.contains(to) {
                    return Err(AdapterError::Api {
                        status: 500,
                        message: format!("mock: pool {to_pool} is full"),
                    });
                }
                let key = mock_key(project, to_pool, to_member, to);
                if st.volumes.contains_key(&key) {
                    return Err(AdapterError::Api {
                        status: 409,
                        message: "Volume by that name already exists".into(),
                    });
                }
                st.volumes.insert(
                    key.clone(),
                    VolumeState {
                        used_by: Vec::new(),
                        location: to_member.unwrap_or("none").into(),
                        ..v.clone()
                    },
                );
                if let Some(b) = backup_of {
                    st.backup_of.insert(key.clone(), b.to_owned());
                }
                st.copies.push(format!(
                    "{name}{} -> {to_pool}/{}/{to}",
                    snapshot.map(|s| format!("/{s}")).unwrap_or_default(),
                    to_member.unwrap_or("-")
                ));
                None
            }
            Provider::Incus(c) => {
                let from = match snapshot {
                    Some(s) => format!("{name}/{s}"),
                    None => name.to_owned(),
                };
                let op = c
                    .copy_volume(
                        project, pool, member, &from, to_pool, to_member, to, &v.content, &config,
                        false,
                    )
                    .await?;
                let what = format!("copying volume {name} to {to_pool}/{to}");
                match self.finish(op, &what).await {
                    // Incus 7.0.1: a restricted identity may not read the copy's operation on
                    // the receiving member (docs/development/TEST-CLUSTER.md). Its other
                    // operation is listed in the project and finishes with the copy.
                    Err(AdapterError::Authentication(_)) => {
                        let started = Instant::now();
                        while c.project_operation_running(project, &[name, to]).await? {
                            if started.elapsed() >= VOLUME_BUDGET {
                                return Err(AdapterError::Api {
                                    status: 504,
                                    message: format!("{what} is still running"),
                                });
                            }
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        }
                        if self
                            .volume(project, to_pool, to_member, to)
                            .await?
                            .is_none()
                        {
                            return Err(AdapterError::Api {
                                status: 500,
                                message: format!("{what} failed: the copy does not exist"),
                            });
                        }
                        return Ok(());
                    }
                    other => return other,
                }
            }
        };
        self.finish(op, &format!("copying volume {name} to {to_pool}/{to}"))
            .await
    }

    /// The volume, when it exists and Apelora created it for `application`.
    async fn owned_volume(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
        application: &str,
    ) -> Result<VolumeState, AdapterError> {
        let v = self
            .volume(project, pool, member, name)
            .await?
            .ok_or_else(|| AdapterError::NotFound(format!("volume {name}")))?;
        if !v.managed || v.application.as_deref() != Some(application) {
            return Err(AdapterError::Api {
                status: 409,
                message: format!("volume {name} does not belong to application {application}"),
            });
        }
        Ok(v)
    }

    /// Deletes a volume Apelora created for `application`, with its snapshots; `false` when it
    /// did not exist. Incus refuses while an instance uses it.
    pub async fn delete_volume(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
        application: &str,
    ) -> Result<bool, AdapterError> {
        valid_names(&[project, pool, name])?;
        if let Some(m) = member {
            valid_member(m)?;
        }
        match self {
            Provider::Mock(m) => m.delete_volume(project, pool, member, name, application),
            Provider::Incus(c) => {
                c.delete_volume(project, pool, member, name, application)
                    .await
            }
        }
    }

    /// The custom volumes of `project` in `pool`, on every member. Read-only.
    pub async fn custom_volumes(
        &self,
        project: &str,
        pool: &str,
    ) -> Result<Vec<CustomVolume>, AdapterError> {
        valid_names(&[project, pool])?;
        match self {
            Provider::Mock(m) => {
                let prefix = format!("{project}/{pool}/");
                let st = m.storage();
                Ok(st
                    .volumes
                    .iter()
                    .filter_map(|(k, v)| {
                        let rest = k.strip_prefix(&prefix)?;
                        let (_, name) = rest.split_once('/')?;
                        Some(CustomVolume {
                            name: name.to_owned(),
                            pool: pool.to_owned(),
                            location: v.location.clone(),
                            content: v.content.clone(),
                            size_mib: v.size_mib,
                            managed: v.managed,
                            application: v.application.clone(),
                            backup_of: st.backup_of.get(k).cloned(),
                            used_by: v.used_by.clone(),
                            created_at: String::new(),
                        })
                    })
                    .collect())
            }
            Provider::Incus(c) => c.custom_volumes(project, pool).await,
        }
    }

    /// Brings a copy's data back into an existing volume: Incus copies `from` (in `from_pool`,
    /// on `from_member`) over the volume `to` with `refresh`. Both must be Apelora's volumes of
    /// `application`. Incus deletes the target's own snapshots and accepts this while an
    /// instance runs (REAL INCUS TESTED, TEST-CLUSTER.md): callers stop or refuse first.
    #[allow(clippy::too_many_arguments)]
    pub async fn refresh_volume(
        &self,
        project: &str,
        from_pool: &str,
        from_member: Option<&str>,
        from: &str,
        to_pool: &str,
        to_member: Option<&str>,
        to: &str,
        application: &str,
    ) -> Result<(), AdapterError> {
        let source = self
            .owned_volume(project, from_pool, from_member, from, application)
            .await?;
        let target = self
            .owned_volume(project, to_pool, to_member, to, application)
            .await?;
        let op = match self {
            Provider::Mock(m) => {
                let mut st = m.storage();
                if st.fail_create.contains(to) {
                    return Err(AdapterError::Api {
                        status: 500,
                        message: format!("mock: pool {to_pool} is full"),
                    });
                }
                st.snapshots
                    .remove(&mock_key(project, to_pool, to_member, to));
                st.copies.push(format!(
                    "{from} -> {to_pool}/{}/{to} (refresh)",
                    to_member.unwrap_or("-")
                ));
                None
            }
            Provider::Incus(c) => {
                let size = target.size_mib.or(source.size_mib).ok_or_else(|| {
                    AdapterError::Decode(format!("volume {to} has a size Apelora cannot read"))
                })?;
                let config = std::collections::BTreeMap::from([
                    ("size".to_owned(), format!("{size}MiB")),
                    (apelora_domain::MANAGED_KEY.to_owned(), "true".to_owned()),
                    (
                        apelora_domain::APPLICATION_KEY.to_owned(),
                        application.to_owned(),
                    ),
                ]);
                let op = c
                    .copy_volume(
                        project,
                        from_pool,
                        from_member,
                        from,
                        to_pool,
                        to_member,
                        to,
                        &target.content,
                        &config,
                        true,
                    )
                    .await?;
                let what = format!("restoring volume {to} from {from_pool}/{from}");
                return match self.finish(op, &what).await {
                    // As for copies: the receiving member's operation may not be readable for a
                    // restricted identity; watch the project's operations instead.
                    Err(AdapterError::Authentication(_)) => {
                        let started = Instant::now();
                        while c.project_operation_running(project, &[from, to]).await? {
                            if started.elapsed() >= VOLUME_BUDGET {
                                return Err(AdapterError::Api {
                                    status: 504,
                                    message: format!("{what} is still running"),
                                });
                            }
                            tokio::time::sleep(Duration::from_secs(2)).await;
                        }
                        Ok(())
                    }
                    other => other,
                };
            }
        };
        self.finish(op, &format!("restoring volume {to}")).await
    }

    /// The network ACLs of `project`. Read-only.
    pub async fn network_acls(&self, project: &str) -> Result<Vec<ObservedAcl>, AdapterError> {
        valid_names(&[project])?;
        match self {
            Provider::Mock(m) => {
                let net = m.net();
                let prefix = format!("{project}/");
                Ok(net
                    .acls
                    .iter()
                    .filter_map(|(k, (ingress, egress))| {
                        let name = k.strip_prefix(&prefix)?;
                        Some(ObservedAcl {
                            name: name.to_owned(),
                            application: net.owners.get(&format!("Acl:{k}")).cloned(),
                            ingress: ingress.clone(),
                            egress: egress.clone(),
                        })
                    })
                    .collect())
            }
            Provider::Incus(c) => c.network_acls(project).await,
        }
    }

    /// The network address sets of `project`. Read-only.
    pub async fn address_sets(
        &self,
        project: &str,
    ) -> Result<Vec<ObservedAddressSet>, AdapterError> {
        valid_names(&[project])?;
        match self {
            Provider::Mock(m) => {
                let net = m.net();
                let prefix = format!("{project}/");
                Ok(net
                    .address_sets
                    .iter()
                    .filter_map(|(k, addresses)| {
                        let name = k.strip_prefix(&prefix)?;
                        Some(ObservedAddressSet {
                            name: name.to_owned(),
                            application: net.owners.get(&format!("AddressSet:{k}")).cloned(),
                            addresses: addresses.clone(),
                        })
                    })
                    .collect())
            }
            Provider::Incus(c) => c.address_sets(project).await,
        }
    }

    /// Every load balancer on `network` (service and published addresses); empty when the
    /// network does not exist. Read-only.
    pub async fn load_balancers(
        &self,
        project: &str,
        network: &str,
    ) -> Result<Vec<ObservedLoadBalancer>, AdapterError> {
        valid_names(&[project, network])?;
        match self {
            Provider::Mock(m) => {
                let prefix = format!("{project}/{network}/");
                Ok(m.net()
                    .load_balancers
                    .iter()
                    .filter(|(k, _)| k.starts_with(&prefix))
                    .map(|(_, (app, lb))| ObservedLoadBalancer {
                        listen_address: lb.listen_address.to_string(),
                        application: Some(app.clone()),
                        module: Some(lb.module.clone()),
                        backends: lb
                            .backends
                            .iter()
                            .map(|b| ObservedBackend {
                                name: b.name.clone(),
                                target_address: b.address.to_string(),
                                description: b.description.clone(),
                            })
                            .collect(),
                        ports: lb
                            .ports
                            .iter()
                            .map(|(t, p)| (t.clone(), p.to_string()))
                            .collect(),
                    })
                    .collect())
            }
            Provider::Incus(c) => c.load_balancers(project, network).await,
        }
    }

    /// One network with its configuration, `None` when it does not exist (or the identity may
    /// not see it). Read-only.
    pub async fn network(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Option<ObservedNetwork>, AdapterError> {
        valid_names(&[project, name])?;
        match self {
            Provider::Mock(m) => {
                if let Some(n) = m
                    .networks
                    .iter()
                    .find(|n| n.project == project && n.name == name)
                {
                    return Ok(Some(ObservedNetwork {
                        name: n.name.clone(),
                        kind: n.kind.clone(),
                        status: n.status.clone(),
                        config: std::collections::BTreeMap::from([
                            ("ipv4.address".to_owned(), "192.0.2.1/24".to_owned()),
                            ("ipv4.routes".to_owned(), "198.18.100.0/28".to_owned()),
                            (
                                "ipv4.ovn.ranges".to_owned(),
                                "192.0.2.100-192.0.2.150".to_owned(),
                            ),
                        ]),
                    }));
                }
                let key = format!("{project}/{name}");
                let net = m.net();
                Ok(net.networks.contains(&key).then(|| ObservedNetwork {
                    name: name.to_owned(),
                    kind: "ovn".into(),
                    status: "Created".into(),
                    config: std::collections::BTreeMap::from([
                        ("network".to_owned(), "mock-br0".to_owned()),
                        ("ipv4.address".to_owned(), "10.99.0.1/24".to_owned()),
                        (
                            "ipv6.address".to_owned(),
                            if net.ipv6 { "2001:db8:a::1/64" } else { "none" }.to_owned(),
                        ),
                        (
                            "volatile.network.ipv4.address".to_owned(),
                            "192.0.2.100".to_owned(),
                        ),
                        (apelora_domain::MANAGED_KEY.to_owned(), "true".to_owned()),
                        (
                            apelora_domain::APPLICATION_KEY.to_owned(),
                            net.owners
                                .get(&format!("Network:{key}"))
                                .cloned()
                                .unwrap_or_default(),
                        ),
                    ]),
                }))
            }
            Provider::Incus(c) => c.network(project, name).await,
        }
    }

    /// A project's configuration (its `restricted.*` limits and features), `None` when it does
    /// not exist. Read-only.
    pub async fn project_config(
        &self,
        project: &str,
    ) -> Result<Option<std::collections::BTreeMap<String, String>>, AdapterError> {
        valid_names(&[project])?;
        match self {
            Provider::Mock(m) => Ok(m.projects.iter().find(|p| p.name == project).map(|p| {
                let mut c = std::collections::BTreeMap::new();
                if p.restricted {
                    c.insert("restricted".to_owned(), "true".to_owned());
                    c.insert("features.networks".to_owned(), "true".to_owned());
                    c.insert(
                        "restricted.networks.uplinks".to_owned(),
                        "mock-br0".to_owned(),
                    );
                    c.insert(
                        "restricted.networks.subnets".to_owned(),
                        "mock-br0:198.18.100.0/28".to_owned(),
                    );
                    c.insert("restricted.snapshots".to_owned(), "allow".to_owned());
                }
                c
            })),
            Provider::Incus(c) => c.project_config(project).await,
        }
    }

    /// Waits on an operation a synchronous-looking call started, if it started one.
    async fn finish(&self, op: Option<String>, what: &str) -> Result<(), AdapterError> {
        let Some(op) = op else { return Ok(()) };
        match self.wait_operation(&op, VOLUME_BUDGET).await? {
            OperationOutcome::Succeeded => Ok(()),
            OperationOutcome::Failed(e) => Err(AdapterError::Api {
                status: 500,
                message: format!("{what} failed: {e}"),
            }),
            OperationOutcome::StillRunning => Err(AdapterError::Api {
                status: 504,
                message: format!("{what} is still running (operation {op})"),
            }),
        }
    }

    /// Waits for an operation to finish, for at most about `budget`.
    pub async fn wait_operation(
        &self,
        id: &str,
        budget: Duration,
    ) -> Result<OperationOutcome, AdapterError> {
        let Provider::Incus(c) = self else {
            // Mock operations complete when they are started.
            return Ok(OperationOutcome::Succeeded);
        };
        let started = Instant::now();
        loop {
            let op = c.wait_operation(id, WAIT_STEP_SECS).await.inspect_err(
                |e| tracing::warn!(operation = id, error = %e, "incus: cannot read the operation"),
            )?;
            let secs = started.elapsed().as_secs();
            match op.status.as_str() {
                "Success" => {
                    tracing::debug!(operation = id, description = %op.description, secs, "incus: operation succeeded");
                    return Ok(OperationOutcome::Succeeded);
                }
                "Failure" => {
                    tracing::warn!(operation = id, description = %op.description, error = %op.err, secs, "incus: operation failed");
                    return Ok(OperationOutcome::Failed(op.err));
                }
                "Cancelled" => {
                    tracing::warn!(operation = id, description = %op.description, secs, "incus: operation was cancelled");
                    return Ok(OperationOutcome::Failed("cancelled".into()));
                }
                _ if started.elapsed() >= budget => {
                    tracing::warn!(operation = id, description = %op.description, status = %op.status, secs, "incus: operation still running when the wait ended");
                    return Ok(OperationOutcome::StillRunning);
                }
                _ => {}
            }
        }
    }
}

/// Fixture data for development and tests. Every name says "mock"; nothing here comes from a
/// real cluster and the server version is unknown. Instances are shared between clones, so a
/// control plane and a worker in one test see the same mock cluster; the seeded instances are
/// not Apelora's (no ownership marker).
#[derive(Debug, Clone)]
pub struct MockIncus {
    pub server: ServerInfo,
    pub members: Arc<Mutex<Vec<ClusterMember>>>,
    pub instances: Arc<Mutex<Vec<MockInstance>>>,
    pub join_tokens: Arc<Mutex<Vec<PendingJoin>>>,
    pub network: Arc<Mutex<MockNetwork>>,
    pub storage: Arc<Mutex<MockStorage>>,
    pub projects: Vec<Project>,
    pub networks: Vec<Network>,
    pub pools: Vec<StoragePool>,
    /// Resources per member name (fixture numbers, labelled mock by the API); a member without
    /// an entry gets [`mock_resources`] for its position.
    pub resources: Arc<Mutex<std::collections::BTreeMap<String, HealthServerResources>>>,
    /// Server configuration (cluster-wide keys), as the admin identity would read it.
    pub config: Arc<Mutex<std::collections::BTreeMap<String, String>>>,
    /// Incus release per member name (tests); a member without one reports the server's
    /// (unknown for the mock).
    pub member_versions: Arc<Mutex<std::collections::BTreeMap<String, String>>>,
    /// Projects Apelora created or changed, profiles, the trust store and project use
    /// (`tenancy.rs`, ADR 0012).
    pub tenancy: Arc<Mutex<tenancy::MockTenancy>>,
    /// Starts of an instance (by name) refused as Incus does while the shared storage's lock is
    /// still held by another host, this many times (tests).
    pub lock_refusals: Arc<Mutex<std::collections::BTreeMap<String, u32>>>,
    /// What a readiness command inside an instance (by name) ends with (tests); unlisted
    /// instances exit 0. `Err` makes the command unable to run at all.
    pub probe_results: Arc<Mutex<std::collections::BTreeMap<String, ProbeBehaviour>>>,
    /// Every readiness command run on the mock: (instance, command), oldest first.
    pub probes_run: Arc<Mutex<Vec<ProbeCall>>>,
    next_op: Arc<Mutex<u64>>,
}

/// One readiness command the mock ran: the instance, and the argument list.
pub type ProbeCall = (String, Vec<String>);

/// How a readiness command ends on the mock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeBehaviour {
    Exit(i32),
    TimesOut,
    CannotRun(String),
}

/// How a command run inside an instance ended. Only its exit status is read, never its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeRun {
    /// The command's exit status, when it ran to the end.
    pub exit: Option<i32>,
    /// It was still running when the time limit passed (and was stopped).
    pub timed_out: bool,
    /// Why it could not be run, when Incus refused.
    pub error: Option<String>,
}

/// Kinds of network objects Apelora owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkObject {
    Network,
    Acl,
    AddressSet,
}

impl NetworkObject {
    fn collection(self) -> &'static str {
        match self {
            NetworkObject::Network => "/1.0/networks",
            NetworkObject::Acl => "/1.0/network-acls",
            NetworkObject::AddressSet => "/1.0/network-address-sets",
        }
    }
}

/// Network objects the mock has been asked to create, keyed `project/name`; `owners` maps
/// `<kind>:project/name` to the application that owns the object.
#[derive(Debug, Clone, Default)]
pub struct MockNetwork {
    pub networks: std::collections::BTreeSet<String>,
    pub acls: std::collections::BTreeMap<String, (Vec<AclRule>, Vec<AclRule>)>,
    pub address_sets: std::collections::BTreeMap<String, Vec<String>>,
    pub owners: std::collections::BTreeMap<String, String>,
    /// `project/network/address` → (application, load balancer).
    pub load_balancers: std::collections::BTreeMap<String, (String, LoadBalancer)>,
    /// How often a health check was re-applied.
    pub health_check_repairs: u32,
    /// Instances whose load balancer check fails (tests): their backends report `offline`.
    pub failing: std::collections::BTreeSet<String>,
    /// Instances restarted, in order (tests).
    pub restarts: Vec<String>,
    /// `project/network` → the service-name records (`user.apelora.dns`).
    pub dns: std::collections::BTreeMap<String, String>,
    /// Application networks get an IPv6 subnet and running instances an IPv6 address too, as
    /// Incus gives an OVN network one of its own (tests).
    pub ipv6: bool,
    /// Instances whose IPv6 address has not arrived yet (it comes after IPv4) (tests).
    pub no_ipv6_yet: std::collections::BTreeSet<String>,
}

impl MockNetwork {
    fn ensure(
        &mut self,
        kind: NetworkObject,
        project: &str,
        name: &str,
        application: &str,
    ) -> Result<(), AdapterError> {
        let owner = format!("{kind:?}:{project}/{name}");
        match self.owners.get(&owner) {
            Some(app) if app != application => {
                return Err(AdapterError::Api {
                    status: 409,
                    message: format!("{name} belongs to application {app}"),
                });
            }
            Some(_) => return Ok(()),
            None => {}
        }
        self.owners.insert(owner, application.to_owned());
        let key = format!("{project}/{name}");
        match kind {
            NetworkObject::Network => {
                self.networks.insert(key);
            }
            NetworkObject::Acl => {
                self.acls.entry(key).or_default();
            }
            NetworkObject::AddressSet => {
                self.address_sets.entry(key).or_default();
            }
        }
        Ok(())
    }

    fn delete(
        &mut self,
        kind: NetworkObject,
        project: &str,
        name: &str,
        application: &str,
    ) -> Result<bool, AdapterError> {
        let owner = format!("{kind:?}:{project}/{name}");
        match self.owners.get(&owner) {
            None => return Ok(false),
            Some(app) if app != application => {
                return Err(AdapterError::Api {
                    status: 409,
                    message: format!("{name} belongs to application {app}"),
                });
            }
            Some(_) => {}
        }
        self.owners.remove(&owner);
        let key = format!("{project}/{name}");
        Ok(match kind {
            NetworkObject::Network => self.networks.remove(&key),
            NetworkObject::Acl => self.acls.remove(&key).is_some(),
            NetworkObject::AddressSet => self.address_sets.remove(&key).is_some(),
        })
    }
}

/// Custom volumes the mock holds, keyed `project/pool/member/name`, and faults to inject.
#[derive(Debug, Clone, Default)]
pub struct MockStorage {
    pub volumes: std::collections::BTreeMap<String, VolumeState>,
    /// Volumes (Incus names) whose creation fails, as on a full pool (tests).
    pub fail_create: std::collections::BTreeSet<String>,
    /// Volumes whose attachment fails (tests); `running` makes restores of used volumes fail
    /// as Incus does while their instance runs.
    pub fail_attach: std::collections::BTreeSet<String>,
    /// Snapshots per volume (same keys as `volumes`).
    pub snapshots: std::collections::BTreeMap<String, Vec<SnapshotInfo>>,
    /// Restores carried out, `volume/snapshot` (tests).
    pub restores: Vec<String>,
    /// Copies made, `source[/snapshot] -> pool/member/name` (tests); a refresh ends in
    /// ` (refresh)`.
    pub copies: Vec<String>,
    /// `user.apelora.backup-of` of backup copies (same keys as `volumes`).
    pub backup_of: std::collections::BTreeMap<String, String>,
}

#[derive(Debug, Clone)]
pub struct MockInstance {
    pub instance: Instance,
    pub base_image: Option<String>,
    /// Disk devices backed by a volume.
    pub volume_devices: Vec<String>,
    /// `migration.stateful` was set at creation.
    pub stateful: bool,
    /// New for every created instance, like Incus's `volatile.uuid`.
    pub uuid: String,
    /// The root disk's pool when the spec chose one (`rootPool`); `None`: the profile's.
    pub root_pool: Option<String>,
}

/// The mock's one log file per instance, named like Incus's for its kind.
fn mock_log_name(i: &MockInstance) -> &'static str {
    match i.instance.kind {
        InstanceKind::VirtualMachine => "qemu.log",
        _ => "lxc.log",
    }
}

impl MockIncus {
    /// The detail page's view of a mock instance: fixture limits and use, labelled as mock by
    /// the API's source.
    fn view(&self, i: &MockInstance) -> InstanceView {
        let running = i.instance.status == "Running";
        let n = self
            .lock()
            .iter()
            .position(|x| {
                x.instance.project == i.instance.project && x.instance.name == i.instance.name
            })
            .unwrap_or(0) as u8;
        InstanceView {
            name: i.instance.name.clone(),
            project: i.instance.project.clone(),
            kind: i.instance.kind,
            status: i.instance.status.clone(),
            location: i.instance.location.clone(),
            architecture: i.instance.architecture.clone(),
            description: i.instance.description.clone(),
            managed: i.instance.managed,
            owner: i.instance.owner.clone(),
            created_at: Some("2026-10-01T09:00:00Z".into()),
            last_used_at: Some("2026-10-20T08:00:00Z".into()),
            started_at: running.then(|| "2026-10-20T08:00:00Z".into()),
            profiles: vec!["default".into()],
            image: InstanceViewImage {
                description: Some("Mock image".into()),
                os: Some("Debian".into()),
                release: Some("13".into()),
                fingerprint: i.base_image.clone(),
            },
            limits: InstanceViewLimits {
                cpu: Some("2".into()),
                memory: Some("2GiB".into()),
                root_disk: Some("10GiB".into()),
            },
            usage: running.then_some(InstanceViewUsage {
                cpu_ns: Some(42_000_000_000),
                memory_bytes: Some(268_435_456),
                memory_peak_bytes: Some(301_989_888),
                memory_total_bytes: Some(2_147_483_648),
                processes: Some(23),
                root_disk_bytes: Some(1_073_741_824),
            }),
            addresses: if running {
                vec![InstanceViewAddress {
                    interface: "eth0".into(),
                    family: "inet".into(),
                    address: format!("10.0.0.{}", 10 + n),
                    netmask: "24".into(),
                    scope: "global".into(),
                }]
            } else {
                Vec::new()
            },
            disks: std::iter::once(InstanceViewDisk {
                device: "root".into(),
                path: Some("/".into()),
                pool: Some("mock-local".into()),
                volume: None,
                size: Some("10GiB".into()),
            })
            .chain(i.volume_devices.iter().map(|d| InstanceViewDisk {
                device: d.clone(),
                path: None,
                pool: Some("local".into()),
                volume: Some(format!(
                    "{}-{}",
                    i.instance.name,
                    d.trim_start_matches("vol-")
                )),
                size: None,
            }))
            .collect(),
            snapshots: Vec::new(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<MockInstance>> {
        // A panic while holding the lock leaves plain data behind; keep using it.
        self.instances.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn net(&self) -> std::sync::MutexGuard<'_, MockNetwork> {
        self.network.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn find(&self, project: &str, name: &str) -> Option<MockInstance> {
        self.lock()
            .iter()
            .find(|i| i.instance.project == project && i.instance.name == name)
            .cloned()
    }

    fn op(&self) -> String {
        let mut n = self.next_op.lock().unwrap_or_else(|e| e.into_inner());
        *n += 1;
        format!("00000000-0000-4000-8000-{:012x}", *n)
    }

    fn create(&self, spec: &InstanceSpec) -> Result<String, AdapterError> {
        if self.find(&spec.project, &spec.name).is_some() {
            return Err(AdapterError::Api {
                status: 409,
                message: "Instance already exists".into(),
            });
        }
        let in_project = self
            .lock()
            .iter()
            .filter(|i| i.instance.project == spec.project)
            .count();
        self.tenancy_admit(spec, in_project)?;
        let uuid = self.op();
        // Like Incus, never place on a member that is not Online.
        let location = self
            .members
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|m| m.status == "Online")
            .map_or_else(|| "mock-member-01".to_owned(), |m| m.name.clone());
        self.lock().push(MockInstance {
            uuid,
            instance: Instance {
                name: spec.name.clone(),
                project: spec.project.clone(),
                kind: spec.kind,
                status: if spec.start { "Running" } else { "Stopped" }.into(),
                location,
                architecture: "x86_64".into(),
                description: "mock data".into(),
                managed: true,
                owner: spec.owner.clone(),
                evacuate: spec.config.get("cluster.evacuate").cloned(),
                unreachable: false,
                limits: apelora_domain::plan::RESIZABLE_KEYS
                    .iter()
                    .filter_map(|k| spec.config.get(*k).map(|v| ((*k).to_owned(), v.clone())))
                    .collect(),
            },
            base_image: spec.source.fingerprint.clone(),
            volume_devices: Vec::new(),
            stateful: spec
                .config
                .get("migration.stateful")
                .is_some_and(|v| v == "true"),
            root_pool: spec.root_pool.clone(),
        });
        Ok(self.op())
    }

    pub fn storage(&self) -> std::sync::MutexGuard<'_, MockStorage> {
        self.storage.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn create_volume(
        &self,
        project: &str,
        member: Option<&str>,
        name: &str,
        mount: &VolumeMount,
        owner: &Owner,
    ) -> Result<Option<String>, AdapterError> {
        let pool = mount.pool.as_deref().unwrap_or_default();
        let mut st = self.storage();
        if st.fail_create.contains(name) {
            return Err(AdapterError::Api {
                status: 500,
                message: format!("mock: pool {pool} is full"),
            });
        }
        st.volumes.insert(
            mock_key(project, pool, member, name),
            VolumeState {
                managed: true,
                application: Some(owner.application.clone()),
                content: mount.content.as_str().into(),
                size_mib: Some(mount.size_mib),
                location: member.unwrap_or("none").into(),
                used_by: Vec::new(),
            },
        );
        Ok(None)
    }

    fn attach_volume(
        &self,
        project: &str,
        instance: &str,
        name: &str,
        mount: &VolumeMount,
    ) -> Result<Option<String>, AdapterError> {
        let pool = mount.pool.as_deref().unwrap_or_default();
        let mut st = self.storage();
        if st.fail_attach.contains(name) {
            return Err(AdapterError::Api {
                status: 500,
                message: format!("mock: cannot attach {name}"),
            });
        }
        let mut all = self.lock();
        let i = all
            .iter_mut()
            .find(|i| i.instance.project == project && i.instance.name == instance)
            .ok_or_else(|| AdapterError::NotFound(format!("instance {project}/{instance}")))?;
        // Like Incus: the volume must be on the instance's member, unless the pool is shared
        // (then it belongs to no member).
        let shared = self
            .pools
            .iter()
            .any(|p| p.name == pool && apelora_domain::storage::is_shared_driver(&p.driver));
        let at = (!shared).then_some(i.instance.location.as_str());
        let key = mock_key(project, pool, at, name);
        let v = st.volumes.get_mut(&key).ok_or_else(|| AdapterError::Api {
            status: 404,
            message: "Storage volume not found".into(),
        })?;
        v.used_by
            .push(format!("/1.0/instances/{instance}?project={project}"));
        let dev = mount.device();
        if !i.volume_devices.contains(&dev) {
            i.volume_devices.push(dev);
        }
        Ok(None)
    }

    fn delete_volume(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
        application: &str,
    ) -> Result<bool, AdapterError> {
        let key = mock_key(project, pool, member, name);
        let mut st = self.storage();
        match st.volumes.get(&key) {
            None => Ok(false),
            Some(v) if !v.managed || v.application.as_deref() != Some(application) => {
                Err(AdapterError::Api {
                    status: 409,
                    message: format!("volume {name} does not belong to application {application}"),
                })
            }
            Some(v) if !v.used_by.is_empty() => Err(AdapterError::Api {
                status: 400,
                message: "The storage volume is still in use".into(),
            }),
            Some(_) => Ok(st.volumes.remove(&key).is_some()),
        }
    }

    fn set_status(&self, project: &str, name: &str, status: &str) -> Result<String, AdapterError> {
        let mut all = self.lock();
        let i = all
            .iter_mut()
            .find(|i| i.instance.project == project && i.instance.name == name)
            .ok_or_else(|| AdapterError::NotFound(format!("instance {project}/{name}")))?;
        i.instance.status = status.into();
        drop(all);
        Ok(self.op())
    }

    fn delete(&self, project: &str, name: &str) -> Result<String, AdapterError> {
        let mut all = self.lock();
        let at = all
            .iter()
            .position(|i| i.instance.project == project && i.instance.name == name)
            .ok_or_else(|| AdapterError::NotFound(format!("instance {project}/{name}")))?;
        if all[at].instance.status == "Running" {
            return Err(AdapterError::Api {
                status: 400,
                message: "Instance is running".into(),
            });
        }
        let gone = all.remove(at);
        drop(all);
        let used = format!("/1.0/instances/{name}?project={project}");
        for v in self.storage().volumes.values_mut() {
            v.used_by.retain(|u| *u != used);
        }
        drop(gone);
        self.tenancy_release(project, name);
        Ok(self.op())
    }

    /// Groups as the members name them (Incus puts every member in `default`).
    fn groups(&self) -> Vec<ClusterGroupInfo> {
        let mut by: std::collections::BTreeMap<String, Vec<String>> = Default::default();
        for m in self
            .members
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
        {
            for g in &m.groups {
                by.entry(g.clone()).or_default().push(m.name.clone());
            }
        }
        by.into_iter()
            .map(|(name, members)| ClusterGroupInfo {
                description: "mock data".into(),
                name,
                members,
            })
            .collect()
    }

    fn member_names(&self) -> Vec<String> {
        self.members
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .map(|m| m.name.clone())
            .collect()
    }

    fn member(&self, name: &str) -> Result<ClusterMember, AdapterError> {
        self.members
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .find(|m| m.name == name)
            .cloned()
            .ok_or_else(|| AdapterError::NotFound(format!("cluster member {name}")))
    }

    /// Simulates a server joining with its pending token (as `incus admin init` would), held
    /// out of placement when `held` (as the enrolment script does). `false` without a token.
    pub fn join(&self, member: &str, held: bool) -> bool {
        let mut tokens = self.join_tokens.lock().unwrap_or_else(|e| e.into_inner());
        let before = tokens.len();
        tokens.retain(|t| t.member_name != member);
        if tokens.len() == before {
            return false;
        }
        drop(tokens);
        let mut members = self.members.lock().unwrap_or_else(|e| e.into_inner());
        let mut m = members[0].clone();
        m.name = member.to_owned();
        m.url = format!("https://{member}.invalid:8443");
        m.roles = Vec::new();
        m.database = false;
        m.scheduler = if held { HELD } else { "all" }.into();
        members.push(m);
        true
    }

    fn create_join_token(&self, member: &str) -> Result<JoinToken, AdapterError> {
        if self
            .members
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .any(|m| m.name == member)
        {
            return Err(AdapterError::Api {
                status: 500,
                message: format!("The cluster already has a member with name: {member}"),
            });
        }
        let pending = PendingJoin {
            member_name: member.to_owned(),
            expires_at: "mock: never used".into(),
        };
        let mut all = self.join_tokens.lock().unwrap_or_else(|e| e.into_inner());
        all.retain(|t| t.member_name != member);
        all.push(pending.clone());
        Ok(JoinToken {
            member_name: pending.member_name,
            expires_at: pending.expires_at,
            token: "mock-join-token-not-usable".into(),
        })
    }

    fn server_resources(
        &self,
        member: Option<&str>,
    ) -> Result<HealthServerResources, AdapterError> {
        let members = self
            .members
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let (at, m) = match member {
            Some(name) => members
                .iter()
                .enumerate()
                .find(|(_, x)| x.name == name)
                .ok_or_else(|| AdapterError::NotFound(format!("cluster member {name}")))?,
            None => members
                .iter()
                .enumerate()
                .next()
                .ok_or_else(|| AdapterError::NotFound("mock server".into()))?,
        };
        if m.status == "Offline" {
            return Err(AdapterError::Connectivity(format!(
                "GET /1.0/resources?target={}: mock: the member does not answer",
                m.name
            )));
        }
        Ok(self
            .resources
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&m.name)
            .cloned()
            .unwrap_or_else(|| mock_resources(at)))
    }

    pub fn with_extensions(extensions: &[&str]) -> Self {
        Self {
            server: ServerInfo {
                api_extensions: extensions.iter().map(|e| (*e).to_owned()).collect(),
                ..Self::default().server
            },
            ..Self::default()
        }
    }
}

/// Incus 7.0.1's documented defaults for the cluster settings Apelora shows (read from
/// `/1.0/metadata/configuration` on the test cluster, 2026-10-21), for the mock.
fn mock_config_defaults() -> std::collections::BTreeMap<String, String> {
    [
        ("cluster.offline_threshold", "20"),
        ("cluster.healing_threshold", "0"),
        ("cluster.max_voters", "3"),
        ("cluster.max_standby", "2"),
        ("cluster.join_token_expiry", "3H"),
        ("cluster.images_minimal_replica", "3"),
        ("cluster.rebalance.interval", "0"),
        ("images.auto_update_interval", "6"),
        ("images.auto_update_cached", "true"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

impl Default for MockIncus {
    fn default() -> Self {
        let member = |n: u8, roles: &[&str]| ClusterMember {
            name: format!("mock-member-0{n}"),
            url: format!("https://mock-member-0{n}.invalid:8443"),
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
            status: "Online".into(),
            message: "Fully operational".into(),
            architecture: "x86_64".into(),
            failure_domain: "default".into(),
            database: true,
            groups: vec!["default".into()],
            scheduler: "all".into(),
        };
        let instance = |name: &str, kind: InstanceKind, loc: u8| Instance {
            name: name.into(),
            project: "default".into(),
            kind,
            status: "Running".into(),
            location: format!("mock-member-0{loc}"),
            architecture: "x86_64".into(),
            description: "mock data".into(),
            managed: false,
            owner: None,
            evacuate: None,
            unreachable: false,
            limits: Default::default(),
        };
        Self {
            server: ServerInfo {
                api_version: "1.0".into(),
                server_version: None,
                kernel_version: None,
                api_extensions: Vec::new(),
                architectures: vec!["x86_64".into()],
                clustered: true,
                auth: "mock".into(),
            },
            members: Arc::new(Mutex::new(vec![
                member(1, &["database-leader", "database"]),
                member(2, &["database"]),
                member(3, &["database"]),
            ])),
            instances: Arc::new(Mutex::new(
                [
                    instance("mock-web-01", InstanceKind::Container, 1),
                    instance("mock-web-02", InstanceKind::Container, 2),
                    instance("mock-db-01", InstanceKind::Container, 3),
                    instance("mock-vm-01", InstanceKind::VirtualMachine, 2),
                ]
                .into_iter()
                .map(|instance| MockInstance {
                    uuid: format!("seed-{}", instance.name),
                    instance,
                    base_image: None,
                    volume_devices: Vec::new(),
                    stateful: false,
                    root_pool: None,
                })
                .collect(),
            )),
            next_op: Arc::new(Mutex::new(0)),
            resources: Arc::default(),
            config: Arc::new(Mutex::new(
                [("cluster.join_token_expiry", "1H")]
                    .into_iter()
                    .map(|(k, v)| (k.to_owned(), v.to_owned()))
                    .collect(),
            )),
            member_versions: Arc::default(),
            tenancy: Arc::new(Mutex::new(tenancy::MockTenancy::seeded())),
            lock_refusals: Arc::default(),
            probe_results: Arc::default(),
            probes_run: Arc::default(),
            join_tokens: Arc::new(Mutex::new(Vec::new())),
            network: Arc::default(),
            storage: Arc::default(),
            projects: vec![
                Project {
                    name: "default".into(),
                    description: "mock data".into(),
                    restricted: false,
                },
                Project {
                    name: "apelora".into(),
                    description: "mock data".into(),
                    restricted: true,
                },
            ],
            networks: vec![Network {
                name: "mock-br0".into(),
                project: "default".into(),
                kind: "bridge".into(),
                managed: true,
                status: "Created".into(),
                description: "mock data".into(),
                locations: Vec::new(),
            }],
            pools: vec![StoragePool {
                name: "mock-local".into(),
                driver: "zfs".into(),
                status: "Created".into(),
                description: "mock data".into(),
                locations: Vec::new(),
            }],
        }
    }
}

/// Fixture resources for the mock's `n`-th member (from 0): 8 threads, 32 GiB, a 500 GiB pool,
/// moderately used. Never shown without the mock label.
pub fn mock_resources(n: usize) -> HealthServerResources {
    const GIB: u64 = 1024 * 1024 * 1024;
    let used = [9, 13, 6][n % 3];
    let disk = [120, 210, 80][n % 3];
    let load = [0.6, 1.4, 0.3][n % 3];
    HealthServerResources {
        cpu_threads: Some(8),
        load_averages: vec![load, load * 0.8, load * 0.6],
        memory: Some(apelora_domain::capacity::HealthUsage {
            used_bytes: used * GIB,
            total_bytes: 32 * GIB,
        }),
        pools: vec![apelora_domain::capacity::HealthPoolUsage {
            name: "mock-local".into(),
            used_bytes: disk * GIB,
            total_bytes: 500 * GIB,
        }],
        uptime_secs: Some(3 * 86_400 + 3_600 * n as u64),
    }
}

fn valid_names(names: &[&str]) -> Result<(), AdapterError> {
    match names.iter().find(|n| !is_valid_name(n)) {
        Some(bad) => Err(AdapterError::InvalidConfig(format!("invalid name {bad:?}"))),
        None => Ok(()),
    }
}

/// The pool, Incus volume name and target member of an instance's volume, all checked.
/// A standalone server reports its location as `none` (or nothing): no target then.
fn volume_target<'a>(
    project: &str,
    member: &'a str,
    instance: &str,
    mount: &VolumeMount,
) -> Result<(String, String, Option<&'a str>), AdapterError> {
    let pool = mount
        .pool
        .clone()
        .ok_or_else(|| AdapterError::InvalidConfig(format!("volume {} has no pool", mount.name)))?;
    let name = volume_name(instance, &mount.name);
    valid_names(&[project, &pool, &name, instance])?;
    let member = Some(member).filter(|m| !m.is_empty() && *m != "none");
    if let Some(m) = member {
        valid_member(m)?;
    }
    Ok((pool, name, member))
}

fn mock_key(project: &str, pool: &str, member: Option<&str>, name: &str) -> String {
    format!("{project}/{pool}/{}/{name}", member.unwrap_or("-"))
}

fn valid_member(name: &str) -> Result<(), AdapterError> {
    if is_valid_name(name) {
        Ok(())
    } else {
        Err(AdapterError::InvalidConfig(format!(
            "invalid member name {name:?}"
        )))
    }
}

#[cfg(test)]
mod inspect_tests;
#[cfg(test)]
mod tests;
