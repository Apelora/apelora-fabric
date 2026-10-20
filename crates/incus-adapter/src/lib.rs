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
mod wire;

pub use client::{IncusClient, IncusConfig};
pub use wire::Operation;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apelora_domain::network::{AclRule, LoadBalancer};
use apelora_domain::{
    ClusterMember, Instance, InstanceAction, InstanceKind, InstanceSpec, JoinToken, Network,
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
}

/// The result of waiting on an Incus operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationOutcome {
    Succeeded,
    Failed(String),
    /// Not finished within the wait budget; wait again later rather than repeating the call.
    StillRunning,
}

/// Server-side wait per request; below the client's 20 s request timeout.
const WAIT_STEP_SECS: u32 = 15;

/// Where inventory comes from: a real Incus server or labelled mock data.
#[derive(Debug, Clone)]
#[expect(
    clippy::large_enum_variant,
    reason = "built once at startup and shared behind an Arc; its size does not matter"
)]
pub enum Provider {
    Mock(MockIncus),
    Incus(IncusClient),
}

impl Provider {
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
            Provider::Mock(m) => Ok(m.members.clone()),
            Provider::Incus(c) => {
                if c.server_info().await?.clustered {
                    c.cluster_members().await
                } else {
                    Ok(Vec::new())
                }
            }
        }
    }

    pub async fn instances(&self) -> Result<Vec<Instance>, AdapterError> {
        match self {
            Provider::Mock(m) => Ok(m.lock().iter().map(|i| i.instance.clone()).collect()),
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
            Provider::Mock(m) => Ok(m.networks.clone()),
            Provider::Incus(c) => c.networks().await,
        }
    }

    pub async fn storage_pools(&self) -> Result<Vec<StoragePool>, AdapterError> {
        match self {
            Provider::Mock(m) => Ok(m.pools.clone()),
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
            })),
            Provider::Incus(c) => c.instance(project, name).await,
        }
    }

    /// Starts creating an instance (validated spec) and returns the Incus operation id.
    pub async fn create_instance(&self, spec: &InstanceSpec) -> Result<String, AdapterError> {
        spec.validate().map_err(AdapterError::InvalidConfig)?;
        match self {
            Provider::Mock(m) => m.create(spec),
            Provider::Incus(c) => c.create_instance(spec).await,
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
            Provider::Mock(m) => m.set_status(project, name, action.target_status()),
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

    /// Network objects of an application (OVN network, appliance ACLs and address sets).
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

    /// Creates or updates an appliance's service address (an OVN load balancer).
    pub async fn ensure_load_balancer(
        &self,
        project: &str,
        network: &str,
        lb: &LoadBalancer,
        application: &str,
    ) -> Result<(), AdapterError> {
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
                net.load_balancers
                    .insert(key, (application.to_owned(), lb.clone()));
                Ok(())
            }
            Provider::Incus(c) => {
                c.ensure_load_balancer(project, network, lb, application)
                    .await
            }
        }
    }

    /// The application's load balancers on `network`, as `(listen address, appliance)`.
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
                    .map(|(k, (_, lb))| (k[prefix.len()..].to_owned(), lb.appliance.clone()))
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

    /// A running instance's global addresses; empty while it has none (e.g. stopped).
    pub async fn instance_addresses(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Vec<String>, AdapterError> {
        match self {
            // Documentation-range addresses so mock data can never be mistaken for real ones.
            Provider::Mock(m) => Ok(m
                .lock()
                .iter()
                .position(|i| {
                    i.instance.project == project
                        && i.instance.name == name
                        && i.instance.status == "Running"
                })
                .map(|n| vec![format!("198.51.100.{}/32", n + 1)])
                .unwrap_or_default()),
            Provider::Incus(c) => c.instance_addresses(project, name).await,
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
            let op = c.wait_operation(id, WAIT_STEP_SECS).await?;
            match op.status.as_str() {
                "Success" => return Ok(OperationOutcome::Succeeded),
                "Failure" => return Ok(OperationOutcome::Failed(op.err)),
                "Cancelled" => return Ok(OperationOutcome::Failed("cancelled".into())),
                _ if started.elapsed() >= budget => return Ok(OperationOutcome::StillRunning),
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
    pub members: Vec<ClusterMember>,
    pub instances: Arc<Mutex<Vec<MockInstance>>>,
    pub join_tokens: Arc<Mutex<Vec<PendingJoin>>>,
    pub network: Arc<Mutex<MockNetwork>>,
    pub projects: Vec<Project>,
    pub networks: Vec<Network>,
    pub pools: Vec<StoragePool>,
    next_op: Arc<Mutex<u64>>,
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

#[derive(Debug, Clone)]
pub struct MockInstance {
    pub instance: Instance,
    pub base_image: Option<String>,
    /// New for every created instance, like Incus's `volatile.uuid`.
    pub uuid: String,
}

impl MockIncus {
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
        let uuid = self.op();
        self.lock().push(MockInstance {
            uuid,
            instance: Instance {
                name: spec.name.clone(),
                project: spec.project.clone(),
                kind: spec.kind,
                status: if spec.start { "Running" } else { "Stopped" }.into(),
                location: "mock-member-01".into(),
                architecture: "x86_64".into(),
                description: "mock data".into(),
                managed: true,
                owner: spec.owner.clone(),
            },
            base_image: spec.source.fingerprint.clone(),
        });
        Ok(self.op())
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
        all.remove(at);
        drop(all);
        Ok(self.op())
    }

    fn create_join_token(&self, member: &str) -> Result<JoinToken, AdapterError> {
        if self.members.iter().any(|m| m.name == member) {
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
            members: vec![
                member(1, &["database-leader", "database"]),
                member(2, &["database"]),
                member(3, &["database"]),
            ],
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
                })
                .collect(),
            )),
            next_op: Arc::new(Mutex::new(0)),
            join_tokens: Arc::new(Mutex::new(Vec::new())),
            network: Arc::default(),
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
            }],
            pools: vec![StoragePool {
                name: "mock-local".into(),
                driver: "zfs".into(),
                status: "Created".into(),
                description: "mock data".into(),
            }],
        }
    }
}

#[cfg(test)]
mod tests;
