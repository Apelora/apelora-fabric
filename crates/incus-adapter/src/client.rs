// SPDX-License-Identifier: AGPL-3.0-only
//! Read-only Incus REST client over mutual TLS.

use std::time::Duration;

use apelora_domain::{
    ClusterMember, Instance, InstanceKind, Network, Project, ServerInfo, StoragePool,
};
use serde::de::DeserializeOwned;

use crate::AdapterError;
use crate::wire::{self, Envelope, Operation, OperationList};

/// Connection settings. Certificates are PEM bytes loaded by the caller from outside the
/// repository; the server certificate is pinned (no public CA roots are trusted).
#[derive(Clone)]
pub struct IncusConfig {
    pub url: String,
    pub client_cert_pem: Vec<u8>,
    pub client_key_pem: Vec<u8>,
    pub server_cert_pem: Vec<u8>,
    /// Name to verify the server certificate against when it differs from the URL host.
    /// `None` = automatic: the URL host if the certificate covers it, else `unspecified`.
    pub tls_server_name: Option<String>,
}

impl std::fmt::Debug for IncusConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never print key material.
        f.debug_struct("IncusConfig")
            .field("url", &self.url)
            .finish_non_exhaustive()
    }
}

/// The TLS name to verify the pinned server certificate against, or `None` when the URL host
/// itself is covered by the certificate.
fn tls_name(
    server_pem: &[u8],
    host: &str,
    explicit: Option<&str>,
) -> Result<Option<String>, AdapterError> {
    use rustls_pki_types::pem::PemObject;
    use rustls_pki_types::{CertificateDer, ServerName};
    let der = CertificateDer::from_pem_slice(server_pem)
        .map_err(|e| AdapterError::InvalidConfig(format!("server certificate: {e}")))?;
    let cert = webpki::EndEntityCert::try_from(&der)
        .map_err(|e| AdapterError::InvalidConfig(format!("server certificate: {e}")))?;
    let covers = |name: &str| {
        ServerName::try_from(name.to_owned())
            .is_ok_and(|n| cert.verify_is_valid_for_subject_name(&n).is_ok())
    };
    if let Some(name) = explicit.filter(|n| !n.is_empty()) {
        return if covers(name) {
            Ok((name != host).then(|| name.to_owned()))
        } else {
            Err(AdapterError::InvalidConfig(format!(
                "the server certificate is not valid for {name}"
            )))
        };
    }
    if covers(host) {
        Ok(None)
    } else if covers("unspecified") {
        Ok(Some("unspecified".to_owned()))
    } else {
        Err(AdapterError::InvalidConfig(format!(
            "the server certificate is valid neither for {host} nor for \"unspecified\"; set the TLS server name explicitly"
        )))
    }
}

/// An error with its causes, e.g. "builder error: no crypto provider".
fn chain(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut cur = e.source();
    while let Some(c) = cur {
        out.push_str(": ");
        out.push_str(&c.to_string());
        cur = c.source();
    }
    out
}

#[derive(Debug, Clone)]
pub struct IncusClient {
    base: String,
    http: reqwest::Client,
}

impl IncusClient {
    /// Builds a client that authenticates with a client certificate and trusts only the
    /// given server certificate. Plain HTTP is refused.
    pub fn mtls(cfg: &IncusConfig) -> Result<Self, AdapterError> {
        if !cfg.url.starts_with("https://") {
            return Err(AdapterError::InvalidConfig(
                "Incus URL must use https://".into(),
            ));
        }
        let mut pem = cfg.client_cert_pem.clone();
        pem.push(b'\n');
        pem.extend_from_slice(&cfg.client_key_pem);
        let identity = reqwest::Identity::from_pem(&pem)
            .map_err(|e| AdapterError::InvalidConfig(format!("client certificate/key: {e}")))?;
        let server = reqwest::Certificate::from_pem(&cfg.server_cert_pem)
            .map_err(|e| AdapterError::InvalidConfig(format!("server certificate: {e}")))?;
        let mut url = reqwest::Url::parse(&cfg.url)
            .map_err(|e| AdapterError::InvalidConfig(format!("Incus URL: {e}")))?;
        let host = url
            .host_str()
            .ok_or_else(|| AdapterError::InvalidConfig("Incus URL has no host".into()))?
            .trim_matches(|c| c == '[' || c == ']')
            .to_owned();
        let port = url.port_or_known_default().unwrap_or(8443);
        let mut builder = reqwest::Client::builder()
            .https_only(true)
            .tls_certs_only([server])
            .identity(identity)
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(20));
        // Incus server certificates often name only "unspecified". Verify against the name the
        // pinned certificate actually carries, while still connecting to the configured address.
        if let Some(name) = tls_name(&cfg.server_cert_pem, &host, cfg.tls_server_name.as_deref())? {
            let addr = std::net::ToSocketAddrs::to_socket_addrs(&(host.as_str(), port))
                .map_err(|e| AdapterError::InvalidConfig(format!("cannot resolve {host}: {e}")))?
                .next()
                .ok_or_else(|| AdapterError::InvalidConfig(format!("cannot resolve {host}")))?;
            url.set_host(Some(&name))
                .map_err(|e| AdapterError::InvalidConfig(format!("TLS server name {name}: {e}")))?;
            builder = builder.resolve(&name, addr);
        }
        let http = builder
            .build()
            .map_err(|e| AdapterError::InvalidConfig(chain(&e)))?;
        Ok(Self {
            base: url.as_str().trim_end_matches('/').to_owned(),
            http,
        })
    }

    /// Test-only constructor for contract tests against a local plain-HTTP fixture server.
    /// Not compiled into release builds (needs `cfg(test)` or the `test-util` feature).
    #[cfg(any(test, feature = "test-util"))]
    pub fn plain_for_tests(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            http: reqwest::Client::new(),
        }
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, AdapterError> {
        let res = self
            .http
            .get(format!("{}{}", self.base, path))
            .send()
            .await
            // Keep the cause chain: it says whether TLS verification, the network or a timeout failed.
            .map_err(|e| AdapterError::Connectivity(chain(&e)))?;
        let status = res.status().as_u16();
        let env: Envelope<T> = res
            .json()
            .await
            .map_err(|e| AdapterError::Decode(format!("{path}: {e}")))?;
        if env.kind == "error" || status >= 400 {
            let code = if env.error_code != 0 {
                env.error_code
            } else {
                status
            };
            return Err(match code {
                401 | 403 => AdapterError::Authentication(env.error),
                404 => AdapterError::NotFound(path.to_owned()),
                _ => AdapterError::Api {
                    status: code,
                    message: env.error,
                },
            });
        }
        env.metadata
            .ok_or_else(|| AdapterError::Decode(format!("{path}: response has no metadata")))
    }

    pub async fn server_info(&self) -> Result<ServerInfo, AdapterError> {
        let s: wire::Server = self.get("/1.0").await?;
        Ok(ServerInfo {
            api_version: s.api_version,
            server_version: Some(s.environment.server_version).filter(|v| !v.is_empty()),
            api_extensions: s.api_extensions,
            clustered: s.environment.server_clustered,
            auth: s.auth,
        })
    }

    pub async fn cluster_members(&self) -> Result<Vec<ClusterMember>, AdapterError> {
        let members: Vec<wire::ClusterMember> =
            self.get("/1.0/cluster/members?recursion=1").await?;
        Ok(members
            .into_iter()
            .map(|m| ClusterMember {
                name: m.server_name,
                url: m.url,
                roles: m.roles,
                status: m.status,
                message: m.message,
                architecture: m.architecture,
                failure_domain: m.failure_domain,
                database: m.database,
                groups: m.groups,
            })
            .collect())
    }

    pub async fn instances(&self) -> Result<Vec<Instance>, AdapterError> {
        let list: Vec<wire::Instance> = self
            .get("/1.0/instances?recursion=1&all-projects=true")
            .await?;
        Ok(list
            .into_iter()
            .map(|i| Instance {
                kind: match i.kind.as_str() {
                    "container" => InstanceKind::Container,
                    "virtual-machine" => InstanceKind::VirtualMachine,
                    _ => InstanceKind::Unknown,
                },
                name: i.name,
                project: i.project,
                status: i.status,
                location: i.location,
                architecture: i.architecture,
                description: i.description,
            })
            .collect())
    }

    pub async fn projects(&self) -> Result<Vec<Project>, AdapterError> {
        let list: Vec<wire::Project> = self.get("/1.0/projects?recursion=1").await?;
        Ok(list
            .into_iter()
            .map(|p| Project {
                name: p.name,
                description: p.description,
            })
            .collect())
    }

    pub async fn networks(&self) -> Result<Vec<Network>, AdapterError> {
        let list: Vec<wire::Network> = self
            .get("/1.0/networks?recursion=1&all-projects=true")
            .await?;
        Ok(list
            .into_iter()
            .map(|n| Network {
                name: n.name,
                project: n.project,
                kind: n.kind,
                managed: n.managed,
                status: n.status,
                description: n.description,
            })
            .collect())
    }

    pub async fn storage_pools(&self) -> Result<Vec<StoragePool>, AdapterError> {
        let list: Vec<wire::StoragePool> = self.get("/1.0/storage-pools?recursion=1").await?;
        Ok(list
            .into_iter()
            .map(|p| StoragePool {
                name: p.name,
                driver: p.driver,
                status: p.status,
                description: p.description,
            })
            .collect())
    }

    pub async fn operations(&self) -> Result<Vec<Operation>, AdapterError> {
        let list: OperationList = self
            .get("/1.0/operations?recursion=1&all-projects=true")
            .await?;
        Ok(list.into_vec())
    }
}
