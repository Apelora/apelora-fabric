// SPDX-License-Identifier: AGPL-3.0-only
//! Boundary between Apelora and the Incus REST API.
//!
//! [`Provider`] is either the real read-only [`IncusClient`] (mutual TLS, pinned server
//! certificate) or [`MockIncus`], fixture data for development and tests that never
//! pretends to be a real cluster. Write operations arrive in Phase 04.

mod client;
mod wire;

pub use client::{IncusClient, IncusConfig};
pub use wire::Operation;

use apelora_domain::{
    ClusterMember, Instance, InstanceKind, Network, Project, ProviderMode, ServerInfo, StoragePool,
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

/// Where inventory comes from: a real Incus server or labelled mock data.
#[derive(Debug, Clone)]
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
            Provider::Mock(m) => Ok(m.instances.clone()),
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
}

/// Fixture data for development and tests. Every name says "mock"; nothing here comes from a
/// real cluster and the server version is unknown.
#[derive(Debug, Clone)]
pub struct MockIncus {
    pub server: ServerInfo,
    pub members: Vec<ClusterMember>,
    pub instances: Vec<Instance>,
    pub projects: Vec<Project>,
    pub networks: Vec<Network>,
    pub pools: Vec<StoragePool>,
}

impl MockIncus {
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
        };
        Self {
            server: ServerInfo {
                api_version: "1.0".into(),
                server_version: None,
                api_extensions: Vec::new(),
                clustered: true,
                auth: "mock".into(),
            },
            members: vec![
                member(1, &["database-leader", "database"]),
                member(2, &["database"]),
                member(3, &["database"]),
            ],
            instances: vec![
                instance("mock-web-01", InstanceKind::Container, 1),
                instance("mock-web-02", InstanceKind::Container, 2),
                instance("mock-db-01", InstanceKind::Container, 3),
                instance("mock-vm-01", InstanceKind::VirtualMachine, 2),
            ],
            projects: vec![Project {
                name: "default".into(),
                description: "mock data".into(),
            }],
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
