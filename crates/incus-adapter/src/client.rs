// SPDX-License-Identifier: AGPL-3.0-only
//! Incus REST client over mutual TLS.

use std::time::Duration;

use apelora_domain::{
    APPLIANCE_KEY, APPLICATION_KEY, ClusterMember, ImageProtocol, Instance, InstanceAction,
    InstanceKind, InstanceSpec, MANAGED_KEY, Network, Owner, Project, REPLICA_KEY, ServerInfo,
    StoragePool,
};
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::wire::{self, Envelope, Operation, OperationList};
use crate::{AdapterError, InstanceState};
use apelora_domain::network::{AclRule, LoadBalancer};
use apelora_domain::{JoinToken, PendingJoin};
use base64::Engine as _;

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

/// TLS libraries reject some client certificates only when the client is built, with an
/// error that blames the peer. Name the actual problem instead (e.g. an X.509 v1 certificate,
/// which some `openssl req -x509` versions produce without extensions).
fn check_client_certificate(pem: &[u8]) -> Result<(), AdapterError> {
    use rustls_pki_types::CertificateDer;
    use rustls_pki_types::pem::PemObject;
    let der = CertificateDer::from_pem_slice(pem)
        .map_err(|e| AdapterError::InvalidConfig(format!("client certificate: {e}")))?;
    webpki::EndEntityCert::try_from(&der)
        .map(|_| ())
        .map_err(|e| {
            AdapterError::InvalidConfig(match e {
                webpki::Error::UnsupportedCertVersion => {
                    "client certificate: X.509 v3 is required (this one is an older version)".into()
                }
                e => format!("client certificate: {e}"),
            })
        })
}

/// Created by Apelora for exactly this application (marker and label both match).
/// Health checks on every service: a backend that stops answering on its port is taken out
/// within about ten seconds and put back once it answers again.
fn load_balancer_body(lb: &LoadBalancer, application: &str) -> Value {
    let backends: Vec<String> = lb.backends.iter().map(|b| b.name.clone()).collect();
    json!({
        "description": format!("Apelora service {}", lb.appliance),
        "config": {
            MANAGED_KEY: "true",
            APPLICATION_KEY: application,
            APPLIANCE_KEY: lb.appliance,
            "healthcheck": "true",
            "healthcheck.interval": "5",
            "healthcheck.failure_count": "2",
            "healthcheck.success_count": "2",
        },
        "backends": lb.backends.iter().map(|b| json!({
            "name": b.name,
            "description": b.description,
            "target_address": b.address.to_string(),
            "target_port": "",
        })).collect::<Vec<_>>(),
        "ports": lb.ports.iter().map(|(transport, port)| json!({
            "description": "",
            "protocol": transport,
            "listen_port": port.to_string(),
            "target_backend": backends,
        })).collect::<Vec<_>>(),
    })
}

fn owned_by_config(config: &std::collections::BTreeMap<String, String>, application: &str) -> bool {
    config.get(MANAGED_KEY).is_some_and(|v| v == "true")
        && config
            .get(APPLICATION_KEY)
            .is_some_and(|v| v == application)
}

fn owned_by(o: &wire::Owned, application: &str) -> bool {
    owned_by_config(&o.config, application)
}

/// Operation ids are UUIDs; anything else must never reach an API path.
fn is_operation_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-')
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

/// When the certificates a client was built with stop being valid (Unix seconds).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CertificateExpiry {
    /// Apelora's client certificate (its Incus identity).
    pub client: i64,
    /// The pinned Incus server certificate.
    pub server: i64,
}

/// When a PEM certificate stops being valid (Unix seconds).
pub fn certificate_not_after(pem: &[u8]) -> Result<i64, AdapterError> {
    use rustls_pki_types::CertificateDer;
    use rustls_pki_types::pem::PemObject;
    let der = CertificateDer::from_pem_slice(pem)
        .map_err(|e| AdapterError::InvalidConfig(format!("certificate: {e}")))?;
    let (_, cert) = x509_parser::parse_x509_certificate(&der)
        .map_err(|e| AdapterError::InvalidConfig(format!("certificate: {e}")))?;
    Ok(cert.validity().not_after.timestamp())
}

#[derive(Debug, Clone)]
pub struct IncusClient {
    base: String,
    http: reqwest::Client,
    expiry: Option<CertificateExpiry>,
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
        check_client_certificate(&cfg.client_cert_pem)?;
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
        let expiry = CertificateExpiry {
            client: certificate_not_after(&cfg.client_cert_pem)?,
            server: certificate_not_after(&cfg.server_cert_pem)?,
        };
        Ok(Self {
            base: url.as_str().trim_end_matches('/').to_owned(),
            http,
            expiry: Some(expiry),
        })
    }

    /// When this client's certificates expire; `None` for test clients without TLS.
    pub fn certificate_expiry(&self) -> Option<CertificateExpiry> {
        self.expiry
    }

    /// Test-only constructor for contract tests against a local plain-HTTP fixture server.
    /// Not compiled into release builds (needs `cfg(test)` or the `test-util` feature).
    #[cfg(any(test, feature = "test-util"))]
    pub fn plain_for_tests(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            http: reqwest::Client::new(),
            expiry: None,
        }
    }

    /// Sends a request and checks the Incus envelope. 404s become [`AdapterError::NotFound`].
    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Envelope<T>, AdapterError> {
        let mut req = self.http.request(method, format!("{}{}", self.base, path));
        if let Some(b) = body {
            req = req.json(b);
        }
        let res = req
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
        Ok(env)
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, AdapterError> {
        self.call(Method::GET, path, None)
            .await?
            .metadata
            .ok_or_else(|| AdapterError::Decode(format!("{path}: response has no metadata")))
    }

    /// Starts an asynchronous Incus operation and returns its id.
    async fn start_operation(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<String, AdapterError> {
        let env: Envelope<Value> = self.call(method, path, body).await?;
        let id = env
            .operation
            .rsplit('/')
            .next()
            .filter(|id| !id.is_empty() && env.kind == "async")
            .ok_or_else(|| {
                AdapterError::Decode(format!("{path}: expected an asynchronous operation"))
            })?;
        Ok(id.to_owned())
    }

    /// The instance's status and whether Apelora owns it; `None` if it does not exist.
    pub async fn instance(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Option<InstanceState>, AdapterError> {
        let path = format!("/1.0/instances/{name}?project={project}");
        match self.get::<wire::InstanceDetail>(&path).await {
            Ok(d) => Ok(Some(InstanceState {
                managed: d.config.get(MANAGED_KEY).is_some_and(|v| v == "true"),
                base_image: d.config.get("volatile.base_image").cloned(),
                status: d.status,
                architecture: d.architecture,
            })),
            Err(AdapterError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `POST /1.0/instances`; the spec must already be validated. Adds the ownership marker.
    pub async fn create_instance(&self, spec: &InstanceSpec) -> Result<String, AdapterError> {
        let mut config = spec.config.clone();
        config.insert(MANAGED_KEY.into(), "true".into());
        if let Some(o) = &spec.owner {
            config.insert(APPLICATION_KEY.into(), o.application.clone());
            config.insert(APPLIANCE_KEY.into(), o.appliance.clone());
            config.insert(REPLICA_KEY.into(), o.replica.to_string());
        }
        let mut source = json!({
            "type": "image",
            "protocol": match spec.source.protocol {
                ImageProtocol::Simplestreams => "simplestreams",
                ImageProtocol::Oci => "oci",
            },
            "server": spec.source.server,
            "alias": spec.source.alias,
        });
        // Incus prefers the fingerprint to the alias, so only the pinned image can be used.
        if let Some(fp) = &spec.source.fingerprint {
            source["fingerprint"] = json!(fp);
        }
        let mut devices = json!({});
        if let Some(n) = &spec.network {
            devices["eth0"] = json!({
                "type": "nic", "name": "eth0", "network": n.network,
                "security.acls": n.acl,
                "security.acls.default.ingress.action": "reject",
                "security.acls.default.egress.action": "reject",
            });
        }
        let mut body = json!({
            "name": spec.name,
            "devices": devices,
            "type": match spec.kind {
                InstanceKind::VirtualMachine => "virtual-machine",
                _ => "container",
            },
            "source": source,
            "config": config,
            "start": spec.start,
        });
        // Incus applies the `default` profile only when the list is absent; an empty list
        // means no profiles at all (and so no root disk).
        if !spec.profiles.is_empty() {
            body["profiles"] = json!(spec.profiles);
        }
        self.start_operation(
            Method::POST,
            &format!("/1.0/instances?project={}", spec.project),
            Some(&body),
        )
        .await
    }

    /// `PUT /1.0/instances/<name>/state`.
    pub async fn change_state(
        &self,
        project: &str,
        name: &str,
        action: InstanceAction,
        force: bool,
    ) -> Result<String, AdapterError> {
        let body = json!({ "action": action.as_str(), "timeout": 30, "force": force });
        self.start_operation(
            Method::PUT,
            &format!("/1.0/instances/{name}/state?project={project}"),
            Some(&body),
        )
        .await
    }

    /// Asks Incus to evacuate (`restore = false`) or restore a cluster member; returns the
    /// operation. Incus moves or stops each instance per its `cluster.evacuate` mode.
    pub async fn member_state(&self, member: &str, restore: bool) -> Result<String, AdapterError> {
        let body = json!({ "action": if restore { "restore" } else { "evacuate" } });
        self.start_operation(
            Method::POST,
            &format!("/1.0/cluster/members/{member}/state"),
            Some(&body),
        )
        .await
    }

    /// `DELETE /1.0/instances/<name>`; Incus refuses to delete a running instance.
    pub async fn delete_instance(&self, project: &str, name: &str) -> Result<String, AdapterError> {
        self.start_operation(
            Method::DELETE,
            &format!("/1.0/instances/{name}?project={project}"),
            None,
        )
        .await
    }

    /// Creates an Apelora-owned object at `collection` (e.g. `/1.0/network-acls`) unless one
    /// with that name exists; refuses one that exists without Apelora's marker.
    async fn ensure_owned(
        &self,
        collection: &str,
        project: &str,
        name: &str,
        application: &str,
        mut body: Value,
    ) -> Result<(), AdapterError> {
        match self
            .get::<wire::Owned>(&format!("{collection}/{name}?project={project}"))
            .await
        {
            Ok(o) if owned_by(&o, application) => Ok(()),
            Ok(_) => Err(AdapterError::Api {
                status: 409,
                message: format!(
                    "{collection}/{name} exists and does not belong to application {application}"
                ),
            }),
            Err(AdapterError::NotFound(_)) => {
                body["name"] = json!(name);
                body["config"][MANAGED_KEY] = json!("true");
                body["config"][APPLICATION_KEY] = json!(application);
                self.call::<Value>(
                    Method::POST,
                    &format!("{collection}?project={project}"),
                    Some(&body),
                )
                .await
                .map(|_| ())
            }
            Err(e) => Err(e),
        }
    }

    /// Names of the objects in `collection` (e.g. `/1.0/network-acls`) that Apelora created for
    /// `application`.
    pub async fn owned_names(
        &self,
        collection: &str,
        project: &str,
        application: &str,
    ) -> Result<Vec<String>, AdapterError> {
        let all: Vec<wire::Owned> = self
            .get(&format!("{collection}?recursion=1&project={project}"))
            .await?;
        Ok(all
            .into_iter()
            .filter(|o| owned_by(o, application))
            .map(|o| o.name)
            .collect())
    }

    /// Deletes an object Apelora created for `application`; `false` when it did not exist.
    pub async fn delete_owned(
        &self,
        collection: &str,
        project: &str,
        name: &str,
        application: &str,
    ) -> Result<bool, AdapterError> {
        match self
            .get::<wire::Owned>(&format!("{collection}/{name}?project={project}"))
            .await
        {
            Ok(o) if owned_by(&o, application) => self
                .call::<Value>(
                    Method::DELETE,
                    &format!("{collection}/{name}?project={project}"),
                    None,
                )
                .await
                .map(|_| true),
            Ok(_) => Err(AdapterError::Api {
                status: 409,
                message: format!(
                    "{collection}/{name} does not belong to application {application}"
                ),
            }),
            Err(AdapterError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// The application's OVN network on `uplink`, created if missing.
    pub async fn ensure_network(
        &self,
        project: &str,
        name: &str,
        uplink: &str,
        application: &str,
    ) -> Result<(), AdapterError> {
        let body = json!({ "type": "ovn", "description": "Apelora application network", "config": { "network": uplink } });
        self.ensure_owned("/1.0/networks", project, name, application, body)
            .await
    }

    pub async fn ensure_acl(
        &self,
        project: &str,
        name: &str,
        application: &str,
    ) -> Result<(), AdapterError> {
        let body = json!({ "description": "Apelora appliance", "config": {}, "ingress": [], "egress": [] });
        self.ensure_owned("/1.0/network-acls", project, name, application, body)
            .await
    }

    /// Replaces the ACL's rules (Apelora owns the whole ACL).
    pub async fn set_acl_rules(
        &self,
        project: &str,
        name: &str,
        ingress: &[AclRule],
        egress: &[AclRule],
        application: &str,
    ) -> Result<(), AdapterError> {
        let body = json!({ "description": "Apelora appliance", "config": { MANAGED_KEY: "true", APPLICATION_KEY: application }, "ingress": ingress, "egress": egress });
        self.call::<Value>(
            Method::PUT,
            &format!("/1.0/network-acls/{name}?project={project}"),
            Some(&body),
        )
        .await
        .map(|_| ())
    }

    pub async fn ensure_address_set(
        &self,
        project: &str,
        name: &str,
        application: &str,
    ) -> Result<(), AdapterError> {
        let body =
            json!({ "description": "Apelora appliance addresses", "config": {}, "addresses": [] });
        self.ensure_owned(
            "/1.0/network-address-sets",
            project,
            name,
            application,
            body,
        )
        .await
    }

    /// Replaces the address set's members.
    pub async fn set_addresses(
        &self,
        project: &str,
        name: &str,
        addresses: &[String],
        application: &str,
    ) -> Result<(), AdapterError> {
        let body = json!({ "description": "Apelora appliance addresses", "config": { MANAGED_KEY: "true", APPLICATION_KEY: application }, "addresses": addresses });
        self.call::<Value>(
            Method::PUT,
            &format!("/1.0/network-address-sets/{name}?project={project}"),
            Some(&body),
        )
        .await
        .map(|_| ())
    }

    /// The instance's identity (`volatile.uuid`); a recreated instance gets a new one.
    pub async fn instance_uuid(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Option<String>, AdapterError> {
        let i: wire::Owned = self
            .get(&format!("/1.0/instances/{name}?project={project}"))
            .await?;
        Ok(i.config.get("volatile.uuid").cloned())
    }

    /// Creates or updates the appliance's load balancer on `network` (Apelora owns it whole);
    /// refuses one at that address that is not this application's. `true` when it changed.
    pub async fn ensure_load_balancer(
        &self,
        project: &str,
        network: &str,
        lb: &LoadBalancer,
        application: &str,
    ) -> Result<bool, AdapterError> {
        let path = format!(
            "/1.0/networks/{network}/load-balancers/{}?project={project}",
            lb.listen_address
        );
        let desired = load_balancer_body(lb, application);
        match self.get::<wire::LoadBalancer>(&path).await {
            Ok(found) if owned_by_config(&found.config, application) => {
                if found.normalised() == wire::LoadBalancer::from_body(&desired)?.normalised() {
                    return Ok(false);
                }
                self.call::<Value>(Method::PUT, &path, Some(&desired))
                    .await
                    .map(|_| true)
            }
            Ok(_) => Err(AdapterError::Api {
                status: 409,
                message: format!(
                    "load balancer {} on {network} does not belong to application {application}",
                    lb.listen_address
                ),
            }),
            Err(AdapterError::NotFound(_)) => {
                let mut body = desired;
                body["listen_address"] = json!(lb.listen_address.to_string());
                self.call::<Value>(
                    Method::POST,
                    &format!("/1.0/networks/{network}/load-balancers?project={project}"),
                    Some(&body),
                )
                .await
                .map(|_| true)
            }
            Err(e) => Err(e),
        }
    }

    /// Each backend's health check status (`online`, `offline`, or `unknown` when no check
    /// runs), as `(backend, status)`.
    pub async fn load_balancer_health(
        &self,
        project: &str,
        network: &str,
        address: &str,
    ) -> Result<Vec<(String, String)>, AdapterError> {
        let state: wire::LoadBalancerState = self
            .get(&format!(
                "/1.0/networks/{network}/load-balancers/{address}/state?project={project}"
            ))
            .await?;
        Ok(state
            .backend_health
            .into_iter()
            .map(|(name, b)| {
                let status = b
                    .ports
                    .first()
                    .map_or_else(|| "unknown".to_owned(), |p| p.status.clone());
                (name, status)
            })
            .collect())
    }

    /// Re-applies the load balancer's health check (off, then on): Incus 7.0.1 can lose it in
    /// OVN on some updates while still reporting it configured (docs/development/TEST-CLUSTER.md).
    pub async fn reapply_health_check(
        &self,
        project: &str,
        network: &str,
        address: &str,
        application: &str,
    ) -> Result<(), AdapterError> {
        let path = format!("/1.0/networks/{network}/load-balancers/{address}?project={project}");
        let lb: wire::LoadBalancer = self.get(&path).await?;
        if !owned_by_config(&lb.config, application) {
            return Err(AdapterError::Api {
                status: 409,
                message: format!(
                    "load balancer {address} on {network} does not belong to application {application}"
                ),
            });
        }
        let mut body = json!({ "description": lb.description, "config": lb.config, "backends": lb.backends.iter().map(|b| json!({ "name": b.name, "description": b.description, "target_address": b.target_address, "target_port": b.target_port })).collect::<Vec<_>>(), "ports": lb.ports.iter().map(|p| json!({ "description": p.description, "protocol": p.protocol, "listen_port": p.listen_port, "target_backend": p.target_backend })).collect::<Vec<_>>() });
        body["config"]["healthcheck"] = json!("false");
        self.call::<Value>(Method::PUT, &path, Some(&body)).await?;
        body["config"]["healthcheck"] = json!("true");
        self.call::<Value>(Method::PUT, &path, Some(&body))
            .await
            .map(|_| ())
    }

    /// The application's load balancers on `network`, as `(listen address, appliance)`;
    /// empty when the network does not exist.
    pub async fn owned_load_balancers(
        &self,
        project: &str,
        network: &str,
        application: &str,
    ) -> Result<Vec<(String, String)>, AdapterError> {
        let all: Vec<wire::LoadBalancer> = match self
            .get(&format!(
                "/1.0/networks/{network}/load-balancers?recursion=1&project={project}"
            ))
            .await
        {
            Ok(all) => all,
            Err(AdapterError::NotFound(_)) => return Ok(Vec::new()),
            Err(e) => return Err(e),
        };
        Ok(all
            .into_iter()
            .filter(|lb| owned_by_config(&lb.config, application))
            .map(|lb| {
                let appliance = lb.config.get(APPLIANCE_KEY).cloned().unwrap_or_default();
                (lb.listen_address, appliance)
            })
            .collect())
    }

    /// Deletes the application's load balancer at `address`; `false` when there was none.
    pub async fn delete_load_balancer(
        &self,
        project: &str,
        network: &str,
        address: &str,
        application: &str,
    ) -> Result<bool, AdapterError> {
        let path = format!("/1.0/networks/{network}/load-balancers/{address}?project={project}");
        match self.get::<wire::LoadBalancer>(&path).await {
            Ok(lb) if owned_by_config(&lb.config, application) => self
                .call::<Value>(Method::DELETE, &path, None)
                .await
                .map(|_| true),
            Ok(_) => Err(AdapterError::Api {
                status: 409,
                message: format!(
                    "load balancer {address} on {network} does not belong to application {application}"
                ),
            }),
            Err(AdapterError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// The instance's global addresses on `eth0` (IPv4 and IPv6), as `address/32` or `/128`.
    pub async fn instance_addresses(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Vec<String>, AdapterError> {
        let state: wire::InstanceStateDetail = self
            .get(&format!("/1.0/instances/{name}/state?project={project}"))
            .await?;
        let mut out: Vec<String> = state
            .network
            .as_ref()
            .and_then(|n| n.get("eth0"))
            .map(|n| {
                n.addresses
                    .iter()
                    .filter(|a| a.scope == "global" && (a.family == "inet" || a.family == "inet6"))
                    .map(|a| {
                        format!(
                            "{}/{}",
                            a.address,
                            if a.family == "inet" { 32 } else { 128 }
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        out.sort();
        Ok(out)
    }

    /// Waits up to `timeout_secs` (kept below the request timeout) for an operation to finish
    /// and returns its state, which may still be running.
    pub async fn wait_operation(
        &self,
        id: &str,
        timeout_secs: u32,
    ) -> Result<Operation, AdapterError> {
        if !is_operation_id(id) {
            return Err(AdapterError::InvalidConfig(format!(
                "invalid operation id {id:?}"
            )));
        }
        self.get(&format!("/1.0/operations/{id}/wait?timeout={timeout_secs}"))
            .await
    }

    pub async fn server_info(&self) -> Result<ServerInfo, AdapterError> {
        self.server_info_at("/1.0").await
    }

    /// One member's own server facts (`GET /1.0?target=<member>`): its Incus and kernel version.
    pub async fn member_server_info(&self, member: &str) -> Result<ServerInfo, AdapterError> {
        self.server_info_at(&format!("/1.0?target={member}")).await
    }

    /// Holds a member out of automatic placement (`scheduler.instance=manual`) or returns it to
    /// the default. Incus reads a member update without roles as dropping them, so the
    /// member's roles, groups, failure domain and description are sent back unchanged.
    pub async fn set_member_held(&self, member: &str, held: bool) -> Result<(), AdapterError> {
        let path = format!("/1.0/cluster/members/{member}");
        let m: wire::ClusterMember = self.get(&path).await?;
        let body = json!({
            "roles": m.roles,
            "groups": m.groups,
            "failure_domain": m.failure_domain,
            "description": m.description,
            // An empty value removes the key: the member goes back to Incus's default.
            "config": { "scheduler.instance": if held { apelora_domain::enrolment::HELD } else { "" } },
        });
        self.call::<Value>(Method::PATCH, &path, Some(&body))
            .await
            .map(|_| ())
    }

    async fn server_info_at(&self, path: &str) -> Result<ServerInfo, AdapterError> {
        let s: wire::Server = self.get(path).await?;
        Ok(ServerInfo {
            api_version: s.api_version,
            server_version: Some(s.environment.server_version).filter(|v| !v.is_empty()),
            kernel_version: Some(s.environment.kernel_version).filter(|v| !v.is_empty()),
            api_extensions: s.api_extensions,
            architectures: s.environment.architectures,
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
                scheduler: m
                    .config
                    .as_ref()
                    .and_then(|c| c.get("scheduler.instance"))
                    .filter(|s| !s.is_empty())
                    .cloned()
                    .unwrap_or_else(|| "all".into()),
            })
            .collect())
    }

    pub async fn instances(&self) -> Result<Vec<Instance>, AdapterError> {
        let list: Vec<wire::Instance> = self
            .get("/1.0/instances?recursion=1&all-projects=true")
            .await?;
        Ok(list
            .into_iter()
            .map(|i| {
                let unreachable = i.config.is_none();
                let config = i.config.unwrap_or_default();
                let expanded = i.expanded_config.unwrap_or_default();
                Instance {
                    unreachable,
                    kind: match i.kind.as_str() {
                        "container" => InstanceKind::Container,
                        "virtual-machine" => InstanceKind::VirtualMachine,
                        _ => InstanceKind::Unknown,
                    },
                    managed: config.get(MANAGED_KEY).is_some_and(|v| v == "true"),
                    owner: match (config.get(APPLICATION_KEY), config.get(APPLIANCE_KEY)) {
                        (Some(application), Some(appliance)) => Some(Owner {
                            application: application.clone(),
                            appliance: appliance.clone(),
                            replica: config
                                .get(REPLICA_KEY)
                                .and_then(|r| r.parse().ok())
                                .unwrap_or(1),
                        }),
                        _ => None,
                    },
                    evacuate: expanded.get("cluster.evacuate").cloned(),
                    name: i.name,
                    project: i.project,
                    status: i.status,
                    location: i.location,
                    architecture: i.architecture,
                    description: i.description,
                }
            })
            .collect())
    }

    pub async fn projects(&self) -> Result<Vec<Project>, AdapterError> {
        let list: Vec<wire::Project> = self.get("/1.0/projects?recursion=1").await?;
        Ok(list
            .into_iter()
            .map(|p| Project {
                restricted: p.config.get("restricted").is_some_and(|v| v == "true"),
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
                locations: n.locations.unwrap_or_default(),
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
                locations: p.locations.unwrap_or_default(),
            })
            .collect())
    }

    /// `POST /1.0/cluster/members`: asks Incus for a single-use join token for `member`.
    /// The returned token carries the join secret; callers must not store or log it.
    pub async fn create_join_token(&self, member: &str) -> Result<JoinToken, AdapterError> {
        let body = json!({ "server_name": member });
        let env: Envelope<wire::TokenOperation> = self
            .call(Method::POST, "/1.0/cluster/members", Some(&body))
            .await?;
        let meta = env
            .metadata
            .and_then(|op| op.metadata)
            .filter(|m| !m.secret.is_empty() && m.server_name == member)
            .ok_or_else(|| {
                AdapterError::Decode("/1.0/cluster/members: no join token in the response".into())
            })?;
        let wire = wire::JoinTokenWire {
            server_name: &meta.server_name,
            fingerprint: &meta.fingerprint,
            addresses: &meta.addresses,
            secret: &meta.secret,
            expires_at: &meta.expires_at,
        };
        let json = serde_json::to_vec(&wire).map_err(|e| AdapterError::Decode(e.to_string()))?;
        Ok(JoinToken {
            member_name: meta.server_name.clone(),
            expires_at: meta.expires_at.clone(),
            token: base64::engine::general_purpose::STANDARD.encode(json),
        })
    }

    /// Join tokens still waiting to be used, with their operation ids (no secrets).
    async fn join_token_operations(&self) -> Result<Vec<(String, PendingJoin)>, AdapterError> {
        let list: OperationList<wire::TokenOperation> =
            self.get("/1.0/operations?recursion=1").await?;
        Ok(list
            .into_vec()
            .into_iter()
            .filter(|op| op.class == "token" && op.status == "Running")
            .filter_map(|op| {
                let m = op.metadata.filter(|m| !m.server_name.is_empty())?;
                let pending = PendingJoin {
                    member_name: m.server_name,
                    expires_at: m.expires_at,
                };
                Some((op.id, pending))
            })
            .collect())
    }

    pub async fn join_tokens(&self) -> Result<Vec<PendingJoin>, AdapterError> {
        let mut out: Vec<PendingJoin> = self
            .join_token_operations()
            .await?
            .into_iter()
            .map(|(_, p)| p)
            .collect();
        out.sort_by(|a, b| a.member_name.cmp(&b.member_name));
        Ok(out)
    }

    /// Cancels the pending join token for `member`; `false` when there is none.
    pub async fn revoke_join_token(&self, member: &str) -> Result<bool, AdapterError> {
        let Some((id, _)) = self
            .join_token_operations()
            .await?
            .into_iter()
            .find(|(_, p)| p.member_name == member)
        else {
            return Ok(false);
        };
        if !is_operation_id(&id) {
            return Err(AdapterError::Decode(format!(
                "unexpected operation id {id:?}"
            )));
        }
        self.call::<Value>(Method::DELETE, &format!("/1.0/operations/{id}"), None)
            .await?;
        Ok(true)
    }

    pub async fn operations(&self) -> Result<Vec<Operation>, AdapterError> {
        let list: OperationList = self
            .get("/1.0/operations?recursion=1&all-projects=true")
            .await?;
        Ok(list.into_vec())
    }
}
