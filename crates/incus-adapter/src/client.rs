// SPDX-License-Identifier: AGPL-3.0-only
//! Incus REST client over mutual TLS.

use std::time::Duration;

use apelora_domain::{
    APPLICATION_KEY, ClusterMember, ImageProtocol, Instance, InstanceAction, InstanceKind,
    InstanceSpec, MANAGED_KEY, MODULE_KEY, Network, Owner, Project, REPLICA_KEY, ServerInfo,
    StoragePool,
};
use reqwest::Method;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::wire::{self, Envelope, Operation, OperationList};
use crate::{AdapterError, InstanceState, SnapshotInfo, VolumeState};
use apelora_domain::inspect::{
    InstanceView, InstanceViewAddress, InstanceViewDisk, InstanceViewImage, InstanceViewLimits,
    InstanceViewSnapshot, InstanceViewUsage, LogTail,
};
use apelora_domain::network::{AclRule, LoadBalancer};
use apelora_domain::storage::{VOLUME_KEY, VolumeMount};
use apelora_domain::{JoinToken, PendingJoin};
use base64::Engine as _;

/// `path` and `project` for the instance file API. Slashes in the path are escaped.
fn file_query(path: &str, project: &str) -> String {
    fn enc(value: &str) -> String {
        let mut out = String::new();
        for b in value.bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                    out.push(b as char);
                }
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }
    format!("path={}&project={}", enc(path), enc(project))
}

/// The guest unit for the service-name resolver. It re-reads its records on every query.
const DNS_UNIT: &str = "\
[Unit]
Description=Apelora service names
After=network-online.target

[Service]
ExecStart=/usr/local/bin/apelora-dns
Restart=on-failure
RestartSec=1

[Install]
WantedBy=multi-user.target
";

/// Debian's stub listener owns `127.0.0.53:53`. The kernel then refuses `0.0.0.0:53`,
/// so the resolver cannot start until that listener is off. The container's own lookups
/// still go through the stub address, which this process answers once it has the port.
const RESOLVED_DROPIN: &str = "\
[Resolve]
DNSStubListener=no
";

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
        "description": format!("Apelora service {}", lb.module),
        "config": {
            MANAGED_KEY: "true",
            APPLICATION_KEY: application,
            MODULE_KEY: lb.module,
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

/// The `config` object of a resource-limit change: only the resizable keys, with plain values.
pub(crate) fn limits_body(
    changes: &std::collections::BTreeMap<String, Option<String>>,
) -> Result<serde_json::Map<String, Value>, AdapterError> {
    if changes.is_empty() {
        return Err(AdapterError::InvalidConfig("no limits to change".into()));
    }
    let mut out = serde_json::Map::new();
    for (key, value) in changes {
        if !apelora_domain::plan::RESIZABLE_KEYS.contains(&key.as_str()) {
            return Err(AdapterError::InvalidConfig(format!(
                "{key} is not a limit Apelora changes"
            )));
        }
        let v = value.as_deref().unwrap_or("");
        let plain = v.len() <= 32
            && v.bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'%' | b'/' | b'.' | b'-'));
        if !plain {
            return Err(AdapterError::InvalidConfig(format!(
                "{key}: {v:?} is not a plain limit value"
            )));
        }
        out.insert(key.clone(), json!(v));
    }
    Ok(out)
}

/// The `config` object of a settings change, after checking every key and value.
pub(crate) fn settings_body(
    changes: &std::collections::BTreeMap<String, Option<String>>,
) -> Result<serde_json::Map<String, Value>, AdapterError> {
    if changes.is_empty() {
        return Err(AdapterError::InvalidConfig("no settings to change".into()));
    }
    let mut out = serde_json::Map::new();
    for (key, value) in changes {
        apelora_domain::clustering::check_setting(key, value.as_deref().unwrap_or(""))
            .map_err(AdapterError::InvalidConfig)?;
        out.insert(key.clone(), json!(value.as_deref().unwrap_or("")));
    }
    Ok(out)
}

fn owned_by_config(config: &std::collections::BTreeMap<String, String>, application: &str) -> bool {
    config.get(MANAGED_KEY).is_some_and(|v| v == "true")
        && config
            .get(APPLICATION_KEY)
            .is_some_and(|v| v == application)
}

/// The application an object was created for, when it carries Apelora's marker.
fn owner_of(config: &std::collections::BTreeMap<String, String>) -> Option<String> {
    config
        .get(MANAGED_KEY)
        .is_some_and(|v| v == "true")
        .then(|| config.get(APPLICATION_KEY).cloned())
        .flatten()
}

fn owned_by(o: &wire::Owned, application: &str) -> bool {
    owned_by_config(&o.config, application)
}

fn already_exists(e: &AdapterError) -> bool {
    matches!(e, AdapterError::Api { message, .. } if message.to_ascii_lowercase().contains("already exists"))
}

fn volume_path(project: &str, pool: &str, member: Option<&str>, name: &str) -> String {
    let mut path = format!("/1.0/storage-pools/{pool}/volumes/custom/{name}?project={project}");
    if let Some(m) = member {
        path.push_str(&format!("&target={m}"));
    }
    path
}

/// A volume size as Incus stores it (`256MiB`, `1GiB`, `1TiB`) in MiB; `None` for other units.
pub(crate) fn parse_mib(size: &str) -> Option<u32> {
    let (n, unit) = size.split_at(size.find(|c: char| !c.is_ascii_digit())?);
    let n: u64 = n.parse().ok()?;
    let mib = match unit {
        "MiB" => n,
        "GiB" => n * 1024,
        "TiB" => n * 1024 * 1024,
        _ => return None,
    };
    u32::try_from(mib).ok()
}

/// The most a raw download (a log) may be before it is refused: 64 MiB.
pub const RAW_LIMIT: u64 = 64 << 20;

/// Incus reports "never" as the zero time (`0001-01-01T00:00:00Z`): that is no value.
fn timestamp(s: String) -> Option<String> {
    (!s.is_empty() && !s.starts_with("0001-01-01")).then_some(s)
}

/// The detail page's view of an instance from Incus's three answers.
pub(crate) fn instance_view_from(
    full: wire::InstanceFull,
    state: Option<wire::InstanceStateFull>,
    snapshots: Vec<wire::InstanceSnapshot>,
) -> InstanceView {
    let config = &full.config;
    let expanded = &full.expanded_config;
    let get = |k: &str| expanded.get(k).or_else(|| config.get(k)).cloned();
    let root = full.expanded_devices.values().find(|d| {
        d.get("type").is_some_and(|t| t == "disk") && d.get("path").is_some_and(|p| p == "/")
    });
    let disks = full
        .expanded_devices
        .iter()
        .filter(|(_, d)| d.get("type").is_some_and(|t| t == "disk"))
        .map(|(device, d)| InstanceViewDisk {
            device: device.clone(),
            path: d.get("path").cloned(),
            pool: d.get("pool").cloned(),
            volume: d.get("source").filter(|_| d.contains_key("pool")).cloned(),
            size: d.get("size").cloned(),
        })
        .collect();
    let running = state.as_ref().is_some_and(|s| s.status == "Running");
    let usage = state
        .as_ref()
        .filter(|_| running)
        .map(|s| InstanceViewUsage {
            cpu_ns: s.cpu.as_ref().and_then(|c| c.usage),
            memory_bytes: s.memory.as_ref().and_then(|m| m.usage),
            memory_peak_bytes: s.memory.as_ref().and_then(|m| m.usage_peak),
            memory_total_bytes: s.memory.as_ref().and_then(|m| m.total).filter(|t| *t > 0),
            processes: s.processes.filter(|p| *p >= 0),
            root_disk_bytes: s
                .disk
                .as_ref()
                .and_then(|d| d.get("root"))
                .and_then(|d| d.usage),
        });
    let mut addresses: Vec<InstanceViewAddress> = state
        .as_ref()
        .and_then(|s| s.network.as_ref())
        .map(|n| {
            n.iter()
                .filter(|(iface, _)| iface.as_str() != "lo")
                .flat_map(|(iface, n)| {
                    n.addresses.iter().map(move |a| InstanceViewAddress {
                        interface: iface.clone(),
                        family: a.family.clone(),
                        address: a.address.clone(),
                        netmask: a.netmask.clone(),
                        scope: a.scope.clone(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    addresses.sort_by(|a, b| {
        (&a.interface, &a.family, &a.address).cmp(&(&b.interface, &b.family, &b.address))
    });
    let owner = match (
        config.get(APPLICATION_KEY),
        apelora_domain::module_label(config),
    ) {
        (Some(application), Some(module)) => Some(Owner {
            application: application.clone(),
            module: module.clone(),
            replica: config
                .get(REPLICA_KEY)
                .and_then(|r| r.parse().ok())
                .unwrap_or(1),
        }),
        _ => None,
    };
    InstanceView {
        kind: match full.kind.as_str() {
            "container" => InstanceKind::Container,
            "virtual-machine" => InstanceKind::VirtualMachine,
            _ => InstanceKind::Unknown,
        },
        managed: config.get(MANAGED_KEY).is_some_and(|v| v == "true"),
        owner,
        image: InstanceViewImage {
            description: get("image.description"),
            os: get("image.os"),
            release: get("image.release"),
            fingerprint: config.get("volatile.base_image").cloned(),
        },
        limits: InstanceViewLimits {
            cpu: get("limits.cpu"),
            memory: get("limits.memory"),
            root_disk: root.and_then(|r| r.get("size").cloned()),
        },
        usage,
        addresses,
        disks,
        snapshots: snapshots
            .into_iter()
            .map(|s| InstanceViewSnapshot {
                name: s.name.rsplit('/').next().unwrap_or_default().to_owned(),
                created_at: timestamp(s.created_at),
                expires_at: timestamp(s.expires_at),
                stateful: s.stateful,
            })
            .collect(),
        started_at: state
            .filter(|_| running)
            .and_then(|s| timestamp(s.started_at)),
        created_at: timestamp(full.created_at),
        last_used_at: timestamp(full.last_used_at),
        profiles: full.profiles,
        name: full.name,
        project: full.project,
        status: full.status,
        location: full.location,
        architecture: full.architecture,
        description: full.description,
    }
}

/// Operation ids are UUIDs; anything else must never reach an API path.
pub(crate) fn is_operation_id(id: &str) -> bool {
    !id.is_empty() && id.bytes().all(|c| c.is_ascii_hexdigit() || c == b'-')
}

/// A request path for messages and logs: the query keeps only the parameters Apelora sends that
/// name things (`project`, `target`, `recursion`, `all-projects`, `timeout`), so a parameter
/// that could ever carry a secret is not repeated.
pub(crate) fn loggable(path: &str) -> String {
    let Some((base, query)) = path.split_once('?') else {
        return path.to_owned();
    };
    let kept: Vec<&str> = query
        .split('&')
        .filter(|kv| {
            matches!(
                kv.split_once('=').map_or(*kv, |(k, _)| k),
                "project" | "target" | "recursion" | "all-projects" | "timeout"
            )
        })
        .collect();
    if kept.is_empty() {
        base.to_owned()
    } else {
        format!("{base}?{}", kept.join("&"))
    }
}

/// A request path as a metrics label: names and ids replaced by `{id}`, the query dropped, so
/// the label has as many values as the adapter has endpoints, not as many as there are
/// instances (`/1.0/instances/{id}/state`).
pub fn endpoint_template(path: &str) -> String {
    /// Collections whose next segment names one object.
    const COLLECTIONS: &[&str] = &[
        "instances",
        "members",
        "network-acls",
        "network-address-sets",
        "networks",
        "load-balancers",
        "forwards",
        "operations",
        "projects",
        "storage-pools",
        "custom",
        "snapshots",
        "backups",
        "certificates",
        "images",
        "profiles",
        "warnings",
        "groups",
    ];
    /// Fixed segments that do not name an object.
    const FIXED: &[&str] = &[
        "1.0",
        "cluster",
        "volumes",
        "state",
        "exec",
        "wait",
        "websocket",
        "metrics",
        "resources",
        "events",
        "console",
        "logs",
        "files",
        "metadata",
        "configuration",
    ];
    let base = path.split('?').next().unwrap_or("");
    let mut out = String::new();
    let mut expect_id = false;
    for seg in base.split('/').filter(|s| !s.is_empty()) {
        out.push('/');
        if expect_id {
            out.push_str("{id}");
            expect_id = false;
        } else if COLLECTIONS.contains(&seg) {
            out.push_str(seg);
            expect_id = true;
        } else if FIXED.contains(&seg) {
            out.push_str(seg);
        } else {
            out.push_str("{id}");
        }
    }
    if out.is_empty() { "/".into() } else { out }
}

/// The server-configuration defaults in a `/1.0/metadata/configuration` answer:
/// `configs.server.<section>.keys[]`, each `{ "<key>": { "defaultdesc": "`20`", … } }`.
pub(crate) fn config_defaults_from(meta: &Value) -> std::collections::BTreeMap<String, String> {
    let mut out = std::collections::BTreeMap::new();
    let Some(sections) = meta.pointer("/configs/server").and_then(Value::as_object) else {
        return out;
    };
    for section in sections.values() {
        let Some(keys) = section.get("keys").and_then(Value::as_array) else {
            continue;
        };
        for entry in keys.iter().filter_map(Value::as_object) {
            for (key, about) in entry {
                if let Some(d) = about.get("defaultdesc").and_then(Value::as_str) {
                    let d = d.trim().trim_matches('`').trim();
                    if !d.is_empty() {
                        out.insert(key.clone(), d.to_owned());
                    }
                }
            }
        }
    }
    out
}

/// Counts one Incus request by endpoint template and outcome class (never the error text), and
/// keeps the reachability gauge: any HTTP answer means Incus is reachable.
fn record_call<T>(path: &str, method: &str, secs: f64, result: &Result<T, AdapterError>) {
    let outcome = match result {
        Ok(_) => "ok",
        Err(AdapterError::NotFound(_)) => "not_found",
        Err(AdapterError::Authentication(_)) => "auth_error",
        Err(AdapterError::Api { status, .. }) if *status >= 500 => "server_error",
        Err(AdapterError::Api { .. }) => "client_error",
        Err(AdapterError::Connectivity(_)) => "unreachable",
        Err(AdapterError::Decode(_)) => "bad_response",
        Err(_) => "other",
    };
    let endpoint = endpoint_template(path);
    apelora_metrics::INCUS_REQUESTS.inc(&[&endpoint, method, outcome]);
    apelora_metrics::INCUS_DURATION.observe(&[&endpoint, method], secs);
    let reachable = outcome != "unreachable";
    apelora_metrics::INCUS_UP.set(&[], if reachable { 1.0 } else { 0.0 });
    if outcome == "ok" {
        apelora_metrics::INCUS_LAST_SUCCESS.set(&[], apelora_metrics::now_secs());
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
    /// How to open websockets (exec sessions); absent for plain-HTTP test clients.
    pub(crate) ws: Option<crate::shell::WsTarget>,
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
        let name = tls_name(&cfg.server_cert_pem, &host, cfg.tls_server_name.as_deref())?;
        let ws = crate::shell::WsTarget::build(
            &cfg.client_cert_pem,
            &cfg.client_key_pem,
            &cfg.server_cert_pem,
            host.clone(),
            port,
            name.clone().unwrap_or_else(|| host.clone()),
        )?;
        if let Some(name) = name {
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
            ws: Some(ws),
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
            ws: None,
        }
    }

    /// Sends a request and checks the Incus envelope. 404s become [`AdapterError::NotFound`].
    /// Errors name the request (`GET /1.0/...`) and the HTTP status; a reply that is not JSON
    /// (e.g. a proxy's error page) keeps its status. Request bodies are never logged: they can
    /// carry join secrets or certificates.
    pub(crate) async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Envelope<T>, AdapterError> {
        let started = std::time::Instant::now();
        let m = apelora_metrics::method_label(method.as_str());
        let result = self.send(method, path, body).await;
        record_call(path, m, started.elapsed().as_secs_f64(), &result);
        result
    }

    async fn send<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Envelope<T>, AdapterError> {
        let what = format!("{method} {}", loggable(path));
        let started = std::time::Instant::now();
        let mut req = self.http.request(method, format!("{}{}", self.base, path));
        if let Some(b) = body {
            req = req.json(b);
        }
        let res = match req.send().await {
            Ok(r) => r,
            Err(e) => {
                // Keep the cause chain: it says whether TLS verification, the network or a timeout failed.
                let e = chain(&e);
                tracing::debug!(request = %what, error = %e, "incus: request not sent or not answered");
                return Err(AdapterError::Connectivity(format!("{what}: {e}")));
            }
        };
        let status = res.status().as_u16();
        let ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
        tracing::debug!(request = %what, status, latency_ms = ms, "incus: response");
        let bytes = res.bytes().await.map_err(|e| {
            AdapterError::Connectivity(format!(
                "{what}: HTTP {status}, reading the reply: {}",
                chain(&e)
            ))
        })?;
        let env: Envelope<T> = match serde_json::from_slice(&bytes) {
            Ok(env) => env,
            Err(e) if status >= 400 => {
                let message = format!(
                    "{what}: HTTP {status} with a reply that is not from the Incus API (a proxy or load balancer in between?)"
                );
                return Err(match status {
                    401 | 403 => AdapterError::Authentication(message),
                    _ => {
                        tracing::debug!(request = %what, status, error = %e, "incus: unreadable error reply");
                        AdapterError::Api { status, message }
                    }
                });
            }
            Err(e) => return Err(AdapterError::Decode(format!("{what}: HTTP {status}: {e}"))),
        };
        if env.kind == "error" || status >= 400 {
            let code = if env.error_code != 0 {
                env.error_code
            } else {
                status
            };
            return Err(match code {
                401 | 403 => {
                    AdapterError::Authentication(format!("{what}: HTTP {code}: {}", env.error))
                }
                404 => AdapterError::NotFound(path.to_owned()),
                _ => AdapterError::Api {
                    status: code,
                    message: format!("{what}: {}", env.error),
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

    /// Like `start_operation`, for a call Incus may answer at once (a configuration change of an
    /// instance is answered synchronously on a running container): `None` when it already
    /// finished.
    async fn start_operation_or_done(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Option<String>, AdapterError> {
        let env: Envelope<Value> = self.call(method, path, body).await?;
        if env.kind == "sync" {
            return Ok(None);
        }
        env.operation
            .rsplit('/')
            .next()
            .filter(|id| !id.is_empty() && env.kind == "async")
            .map(|id| Some(id.to_owned()))
            .ok_or_else(|| {
                AdapterError::Decode(format!("{path}: expected an asynchronous operation"))
            })
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
                location: d.location,
                volume_devices: d
                    .devices
                    .into_iter()
                    .filter(|(_, dev)| {
                        dev.get("type").is_some_and(|t| t == "disk")
                            && dev.contains_key("pool")
                            && dev.get("path").is_none_or(|p| p != "/")
                    })
                    .map(|(name, _)| name)
                    .collect(),
                kind: match d.kind.as_str() {
                    "virtual-machine" => InstanceKind::VirtualMachine,
                    "container" => InstanceKind::Container,
                    _ => InstanceKind::Unknown,
                },
                limits: apelora_domain::plan::RESIZABLE_KEYS
                    .iter()
                    .filter_map(|k| d.config.get(*k).map(|v| ((*k).to_owned(), v.clone())))
                    .collect(),
                stateful: d
                    .config
                    .get("migration.stateful")
                    .is_some_and(|v| v == "true"),
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
            config.insert(MODULE_KEY.into(), o.module.clone());
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
        // The root disk on a chosen pool (the shared one, ADR 0013) instead of the profile's.
        if let Some(pool) = &spec.root_pool {
            devices["root"] = json!({ "type": "disk", "path": "/", "pool": pool });
        }
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

    /// A custom volume in `pool`, on `member` when the pool is member-local; `None` if missing.
    pub async fn volume(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
    ) -> Result<Option<VolumeState>, AdapterError> {
        let path = volume_path(project, pool, member, name);
        match self.get::<wire::Volume>(&path).await {
            Ok(v) => Ok(Some(VolumeState {
                managed: v.config.get(MANAGED_KEY).is_some_and(|x| x == "true"),
                application: v.config.get(APPLICATION_KEY).cloned(),
                size_mib: v.config.get("size").and_then(|s| parse_mib(s)),
                content: v.content_type,
                location: v.location,
                used_by: v.used_by,
            })),
            Err(AdapterError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// `POST /1.0/storage-pools/<pool>/volumes/custom`, labelled with its owner. Returns the
    /// operation id when Incus creates it asynchronously.
    pub async fn create_volume(
        &self,
        project: &str,
        member: Option<&str>,
        name: &str,
        mount: &VolumeMount,
        owner: &Owner,
    ) -> Result<Option<String>, AdapterError> {
        let pool = mount.pool.as_deref().unwrap_or_default();
        let body = json!({
            "name": name,
            "content_type": mount.content.as_str(),
            "description": "Apelora volume",
            "config": {
                "size": format!("{}MiB", mount.size_mib),
                MANAGED_KEY: "true",
                APPLICATION_KEY: owner.application,
                MODULE_KEY: owner.module,
                VOLUME_KEY: mount.name,
            },
        });
        let mut path = format!("/1.0/storage-pools/{pool}/volumes/custom?project={project}");
        if let Some(m) = member {
            path.push_str(&format!("&target={m}"));
        }
        self.maybe_operation(Method::POST, &path, Some(&body)).await
    }

    /// Adds the volume as a disk device of the instance. Incus merges the devices of a `PATCH`
    /// with the instance's existing ones.
    pub async fn attach_volume(
        &self,
        project: &str,
        instance: &str,
        name: &str,
        mount: &VolumeMount,
    ) -> Result<Option<String>, AdapterError> {
        let mut dev = json!({
            "type": "disk",
            "pool": mount.pool.as_deref().unwrap_or_default(),
            "source": name,
        });
        if let Some(p) = &mount.path {
            dev["path"] = json!(p);
        }
        let body = json!({ "devices": { mount.device(): dev } });
        self.maybe_operation(
            Method::PATCH,
            &format!("/1.0/instances/{instance}?project={project}"),
            Some(&body),
        )
        .await
    }

    /// Sets a volume's size (`PATCH`, merged with its config). The caller has checked that the
    /// volume is the application's and that the size only grows: Incus would shrink it.
    pub async fn set_volume_size(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
        size_mib: u32,
    ) -> Result<Option<String>, AdapterError> {
        let body = json!({ "config": { "size": format!("{size_mib}MiB") } });
        self.maybe_operation(
            Method::PATCH,
            &volume_path(project, pool, member, name),
            Some(&body),
        )
        .await
    }

    /// Deletes a volume Apelora created for `application` (with its snapshots); `false` when
    /// it did not exist. Incus refuses while an instance uses it.
    pub async fn delete_volume(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
        application: &str,
    ) -> Result<bool, AdapterError> {
        let path = volume_path(project, pool, member, name);
        match self.get::<wire::Volume>(&path).await {
            Ok(v) if owned_by_config(&v.config, application) => self
                .maybe_operation(Method::DELETE, &path, None)
                .await
                .map(|_| true),
            Ok(_) => Err(AdapterError::Api {
                status: 409,
                message: format!("volume {name} does not belong to application {application}"),
            }),
            Err(AdapterError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// The volume's snapshots, oldest first.
    pub async fn volume_snapshots(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
    ) -> Result<Vec<SnapshotInfo>, AdapterError> {
        let path = volume_path(project, pool, member, &format!("{name}/snapshots")).replacen(
            '?',
            "?recursion=1&",
            1,
        );
        let mut all: Vec<wire::VolumeSnapshot> = self.get(&path).await?;
        all.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(all
            .into_iter()
            .map(|s| SnapshotInfo {
                // Some releases report `<volume>/<snapshot>`.
                name: s.name.rsplit('/').next().unwrap_or_default().to_owned(),
                created_at: s.created_at,
            })
            .collect())
    }

    /// `POST …/volumes/custom/<name>/snapshots`.
    pub async fn snapshot_volume(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
        snapshot: &str,
    ) -> Result<Option<String>, AdapterError> {
        let path = volume_path(project, pool, member, &format!("{name}/snapshots"));
        self.maybe_operation(Method::POST, &path, Some(&json!({ "name": snapshot })))
            .await
    }

    /// Restores a volume from a snapshot: `PUT` of the volume with `restore`, keeping its
    /// config and description.
    pub async fn restore_volume(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
        snapshot: &str,
    ) -> Result<Option<String>, AdapterError> {
        let path = volume_path(project, pool, member, name);
        let v: wire::Volume = self.get(&path).await?;
        let body = json!({ "config": v.config, "description": v.description, "restore": snapshot });
        self.maybe_operation(Method::PUT, &path, Some(&body)).await
    }

    /// `DELETE …/volumes/custom/<name>/snapshots/<snapshot>`.
    pub async fn delete_snapshot(
        &self,
        project: &str,
        pool: &str,
        member: Option<&str>,
        name: &str,
        snapshot: &str,
    ) -> Result<Option<String>, AdapterError> {
        let path = volume_path(
            project,
            pool,
            member,
            &format!("{name}/snapshots/{snapshot}"),
        );
        self.maybe_operation(Method::DELETE, &path, None).await
    }

    /// Copies a volume (or one of its snapshots, `name/snapshot`) to `to` in `to_pool`, on
    /// `to_member` for member-local pools. Incus carries neither the size nor the labels over:
    /// both are set here.
    #[allow(clippy::too_many_arguments)]
    pub async fn copy_volume(
        &self,
        project: &str,
        from_pool: &str,
        from_member: Option<&str>,
        from: &str,
        to_pool: &str,
        to_member: Option<&str>,
        to: &str,
        content: &str,
        config: &std::collections::BTreeMap<String, String>,
        refresh: bool,
    ) -> Result<Option<String>, AdapterError> {
        let mut source = json!({ "type": "copy", "pool": from_pool, "name": from });
        if let Some(m) = from_member {
            source["location"] = json!(m);
        }
        // Copies over an existing volume `to` (its own snapshots are deleted by Incus).
        if refresh {
            source["refresh"] = json!(true);
        }
        let body = json!({ "name": to, "type": "custom", "content_type": content, "config": config, "source": source });
        let mut path = format!("/1.0/storage-pools/{to_pool}/volumes/custom?project={project}");
        if let Some(m) = to_member {
            path.push_str(&format!("&target={m}"));
        }
        self.maybe_operation(Method::POST, &path, Some(&body)).await
    }

    /// The custom volumes of `project` in `pool` (every member of a member-local pool).
    pub async fn custom_volumes(
        &self,
        project: &str,
        pool: &str,
    ) -> Result<Vec<crate::CustomVolume>, AdapterError> {
        let list: Vec<wire::Volume> = self
            .get(&format!(
                "/1.0/storage-pools/{pool}/volumes/custom?project={project}&recursion=1"
            ))
            .await?;
        Ok(list
            .into_iter()
            .map(|v| crate::CustomVolume {
                managed: v.config.get(MANAGED_KEY).is_some_and(|x| x == "true"),
                application: v.config.get(APPLICATION_KEY).cloned(),
                backup_of: v
                    .config
                    .get(apelora_domain::storage::BACKUP_OF_KEY)
                    .cloned(),
                size_mib: v.config.get("size").and_then(|s| parse_mib(s)),
                name: v.name,
                pool: pool.to_owned(),
                location: v.location,
                content: v.content_type,
                used_by: v.used_by,
                created_at: v.created_at,
            })
            .collect())
    }

    /// The network ACLs of `project`.
    pub async fn network_acls(
        &self,
        project: &str,
    ) -> Result<Vec<crate::ObservedAcl>, AdapterError> {
        let list: Vec<wire::Acl> = self
            .get(&format!("/1.0/network-acls?recursion=1&project={project}"))
            .await?;
        let rules = |r: Vec<wire::AclRuleWire>| {
            r.into_iter()
                .map(|r| AclRule {
                    action: r.action,
                    source: r.source,
                    destination: r.destination,
                    protocol: r.protocol,
                    destination_port: r.destination_port,
                    state: r.state,
                    description: r.description,
                })
                .collect()
        };
        Ok(list
            .into_iter()
            .map(|a| crate::ObservedAcl {
                application: owner_of(&a.config),
                name: a.name,
                ingress: rules(a.ingress),
                egress: rules(a.egress),
            })
            .collect())
    }

    /// The network address sets of `project`.
    pub async fn address_sets(
        &self,
        project: &str,
    ) -> Result<Vec<crate::ObservedAddressSet>, AdapterError> {
        let list: Vec<wire::AddressSet> = self
            .get(&format!(
                "/1.0/network-address-sets?recursion=1&project={project}"
            ))
            .await?;
        Ok(list
            .into_iter()
            .map(|a| crate::ObservedAddressSet {
                application: owner_of(&a.config),
                name: a.name,
                addresses: a.addresses,
            })
            .collect())
    }

    /// Every load balancer on `network`; empty when the network does not exist.
    pub async fn load_balancers(
        &self,
        project: &str,
        network: &str,
    ) -> Result<Vec<crate::ObservedLoadBalancer>, AdapterError> {
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
            .map(|lb| crate::ObservedLoadBalancer {
                application: owner_of(&lb.config),
                module: apelora_domain::module_label(&lb.config).cloned(),
                listen_address: lb.listen_address,
                backends: lb
                    .backends
                    .into_iter()
                    .map(|b| crate::ObservedBackend {
                        name: b.name,
                        target_address: b.target_address,
                        description: b.description,
                    })
                    .collect(),
                ports: lb
                    .ports
                    .into_iter()
                    .map(|p| (p.protocol, p.listen_port))
                    .collect(),
            })
            .collect())
    }

    /// One network with its configuration; `None` when it does not exist or is not visible.
    pub async fn network(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Option<crate::ObservedNetwork>, AdapterError> {
        match self
            .get::<wire::NetworkFull>(&format!("/1.0/networks/{name}?project={project}"))
            .await
        {
            Ok(n) => Ok(Some(crate::ObservedNetwork {
                name: n.name,
                kind: n.kind,
                status: n.status,
                config: n.config,
            })),
            Err(AdapterError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Adds the application's service names to the network without dropping the configuration
    /// already there. Reads the network, then writes `config` and `description` back.
    pub async fn publish_service_dns(
        &self,
        project: &str,
        name: &str,
        application: &str,
        records: &str,
        nameservers: Option<&str>,
        upstream: Option<&str>,
    ) -> Result<(), AdapterError> {
        let current = self
            .get::<Value>(&format!("/1.0/networks/{name}?project={project}"))
            .await?;
        let mut config = current
            .get("config")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        config.insert(
            "dns.domain".into(),
            Value::String(format!("{application}.internal")),
        );
        // DHCP search list. `incus` stays so a short instance name still finds the record
        // OVN registered under that domain; the application's domain is tried first.
        config.insert(
            "dns.search".into(),
            Value::String(format!("{application}.internal,incus")),
        );
        config.insert("user.apelora.dns".into(), Value::String(records.to_owned()));
        // `None` leaves dns.nameservers alone (the resolver is not ready yet). An empty
        // list removes one Apelora installed. A list makes the instances ask the resolver.
        match nameservers {
            None => {}
            Some("") => {
                if config.get("user.apelora.dns.via").and_then(Value::as_str) == Some("resolver") {
                    config.remove("dns.nameservers");
                    config.remove("user.apelora.dns.via");
                    config.remove("user.apelora.dns.upstream");
                }
            }
            Some(list) => {
                config.insert("dns.nameservers".into(), Value::String(list.to_owned()));
                config.insert(
                    "user.apelora.dns.via".into(),
                    Value::String("resolver".into()),
                );
                if let Some(up) = upstream {
                    config.insert(
                        "user.apelora.dns.upstream".into(),
                        Value::String(up.to_owned()),
                    );
                }
            }
        }
        config.insert(MANAGED_KEY.into(), Value::String("true".into()));
        config.insert(
            APPLICATION_KEY.into(),
            Value::String(application.to_owned()),
        );
        let description = current
            .get("description")
            .cloned()
            .unwrap_or(Value::String("Apelora application network".into()));
        self.call::<Value>(
            Method::PUT,
            &format!("/1.0/networks/{name}?project={project}"),
            Some(&json!({ "config": config, "description": description })),
        )
        .await
        .map(|_| ())
    }

    /// A project's configuration; `None` when it does not exist or is not visible.
    pub async fn project_config(
        &self,
        project: &str,
    ) -> Result<Option<std::collections::BTreeMap<String, String>>, AdapterError> {
        match self
            .get::<wire::Project>(&format!("/1.0/projects/{project}"))
            .await
        {
            Ok(p) => Ok(Some(p.config)),
            Err(AdapterError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Whether an operation of `project` that mentions any of `names` is still running. Used
    /// when Incus refuses a restricted identity the wait on a copy's own operation (it lives on
    /// the receiving member and is checked against the default project); the copy's other
    /// operation is listed in the project and finishes when the copy does.
    pub async fn project_operation_running(
        &self,
        project: &str,
        names: &[&str],
    ) -> Result<bool, AdapterError> {
        let all: std::collections::BTreeMap<String, Vec<Value>> = self
            .get(&format!("/1.0/operations?project={project}&recursion=1"))
            .await?;
        Ok(all.values().flatten().any(|op| {
            op["status"].as_str() == Some("Running") && {
                let text = op.to_string();
                names.iter().any(|n| text.contains(n))
            }
        }))
    }

    /// Sends a request that Incus answers either synchronously or with an operation.
    async fn maybe_operation(
        &self,
        method: Method,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Option<String>, AdapterError> {
        let env: Envelope<Value> = self.call(method, path, body).await?;
        if env.kind != "async" {
            return Ok(None);
        }
        let id = env.operation.rsplit('/').next().unwrap_or_default();
        if is_operation_id(id) {
            Ok(Some(id.to_owned()))
        } else {
            Err(AdapterError::Decode(format!(
                "{path}: invalid operation id"
            )))
        }
    }

    /// `PUT /1.0/instances/<name>/state`.
    /// `PATCH /1.0/instances/<name>`: merges the given limits into the instance's own
    /// configuration (an empty value removes a key). Refuses an instance Apelora does not own.
    pub(crate) async fn set_instance_limits(
        &self,
        project: &str,
        name: &str,
        config: serde_json::Map<String, Value>,
    ) -> Result<Option<String>, AdapterError> {
        match self.instance(project, name).await? {
            None => return Err(AdapterError::NotFound(format!("instance {project}/{name}"))),
            Some(s) if !s.managed => {
                return Err(AdapterError::Api {
                    status: 409,
                    message: "the instance is not managed by Apelora".into(),
                });
            }
            Some(_) => {}
        }
        self.start_operation_or_done(
            Method::PATCH,
            &format!("/1.0/instances/{name}?project={project}"),
            Some(&json!({ "config": config })),
        )
        .await
    }

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

    /// `POST /1.0/instances/<name>?target=<member>`: moves the instance within the cluster.
    pub async fn move_instance(
        &self,
        project: &str,
        name: &str,
        target: &str,
        live: bool,
    ) -> Result<String, AdapterError> {
        self.start_operation(
            Method::POST,
            &format!("/1.0/instances/{name}?project={project}&target={target}"),
            Some(&json!({ "name": name, "migration": true, "live": live })),
        )
        .await
    }

    /// `DELETE /1.0/cluster/members/<name>`: removes the member from the cluster for good.
    /// Incus refuses while instances or volumes remain on it.
    pub async fn remove_member(&self, member: &str) -> Result<Option<String>, AdapterError> {
        self.maybe_operation(
            Method::DELETE,
            &format!("/1.0/cluster/members/{member}"),
            None,
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
                match self
                    .call::<Value>(
                        Method::POST,
                        &format!("{collection}?project={project}"),
                        Some(&body),
                    )
                    .await
                {
                    Ok(_) => Ok(()),
                    // Another request created it first (two workers, or a GET that lagged
                    // behind a POST on another member). If it is ours, the ensure succeeded.
                    Err(e) if already_exists(&e) => {
                        self.require_owned(collection, project, name, application)
                            .await
                    }
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
        }
    }

    async fn require_owned(
        &self,
        collection: &str,
        project: &str,
        name: &str,
        application: &str,
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
        let body =
            json!({ "description": "Apelora module", "config": {}, "ingress": [], "egress": [] });
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
        let body = json!({ "description": "Apelora module", "config": { MANAGED_KEY: "true", APPLICATION_KEY: application }, "ingress": ingress, "egress": egress });
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
            json!({ "description": "Apelora module addresses", "config": {}, "addresses": [] });
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
        let body = json!({ "description": "Apelora module addresses", "config": { MANAGED_KEY: "true", APPLICATION_KEY: application }, "addresses": addresses });
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

    /// Creates or updates the module's load balancer on `network` (Apelora owns it whole);
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

    /// The application's load balancers on `network`, as `(listen address, module)`;
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
                let module = apelora_domain::module_label(&lb.config)
                    .cloned()
                    .unwrap_or_default();
                (lb.listen_address, module)
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

    /// One instance in detail: three reads (the instance, its state, its snapshots). `None`
    /// when it does not exist. The state and snapshots are best effort: a VM whose agent does
    /// not answer still has a detail page.
    pub async fn instance_view(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Option<InstanceView>, AdapterError> {
        let full: wire::InstanceFull = match self
            .get(&format!("/1.0/instances/{name}?project={project}"))
            .await
        {
            Ok(f) => f,
            Err(AdapterError::NotFound(_)) => return Ok(None),
            Err(e) => return Err(e),
        };
        let state: Option<wire::InstanceStateFull> = match self
            .get(&format!("/1.0/instances/{name}/state?project={project}"))
            .await
        {
            Ok(s) => Some(s),
            Err(AdapterError::NotFound(_)) => return Ok(None),
            Err(e) => {
                tracing::debug!(instance = %format!("{project}/{name}"), error = %e, "incus: instance state not readable; shown without it");
                None
            }
        };
        let snapshots: Vec<wire::InstanceSnapshot> = self
            .get(&format!(
                "/1.0/instances/{name}/snapshots?project={project}&recursion=1"
            ))
            .await
            .unwrap_or_default();
        Ok(Some(instance_view_from(full, state, snapshots)))
    }

    /// The instance's log files, as names (Incus lists them as URLs).
    pub async fn instance_logs(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Vec<String>, AdapterError> {
        let urls: Vec<String> = self
            .get(&format!("/1.0/instances/{name}/logs?project={project}"))
            .await?;
        let mut names: Vec<String> = urls
            .iter()
            .filter_map(|u| apelora_domain::inspect::log_file_name(u))
            .map(str::to_owned)
            .collect();
        names.sort();
        names.dedup();
        Ok(names)
    }

    /// The end of one log file (raw content, not an envelope), at most `max` bytes.
    pub async fn instance_log(
        &self,
        project: &str,
        name: &str,
        file: &str,
        max: usize,
    ) -> Result<LogTail, AdapterError> {
        let file = apelora_domain::inspect::log_file_name(file)
            .filter(|f| *f == file)
            .ok_or_else(|| {
                AdapterError::InvalidConfig(format!("invalid log file name {file:?}"))
            })?;
        self.get_tail(
            &format!("/1.0/instances/{name}/logs/{file}?project={project}"),
            max,
        )
        .await
    }

    /// The end of the instance's console log (`type=log`), at most `max` bytes.
    pub async fn console_log(
        &self,
        project: &str,
        name: &str,
        max: usize,
    ) -> Result<LogTail, AdapterError> {
        self.get_tail(
            &format!("/1.0/instances/{name}/console?project={project}&type=log"),
            max,
        )
        .await
    }

    /// A raw (non-envelope) download, keeping only its last `max` bytes in memory. Refuses
    /// more than [`RAW_LIMIT`] in total. Error replies are envelopes and are read as such.
    async fn get_tail(&self, path: &str, max: usize) -> Result<LogTail, AdapterError> {
        let started = std::time::Instant::now();
        let result = self.send_tail(path, max).await;
        record_call(path, "GET", started.elapsed().as_secs_f64(), &result);
        result
    }

    async fn send_tail(&self, path: &str, max: usize) -> Result<LogTail, AdapterError> {
        let what = format!("GET {}", loggable(path));
        let mut res = self
            .http
            .get(format!("{}{}", self.base, path))
            .send()
            .await
            .map_err(|e| {
                let e = chain(&e);
                tracing::debug!(request = %what, error = %e, "incus: request not sent or not answered");
                AdapterError::Connectivity(format!("{what}: {e}"))
            })?;
        let status = res.status().as_u16();
        tracing::debug!(request = %what, status, "incus: response");
        let read_err = |e: reqwest::Error| {
            AdapterError::Connectivity(format!(
                "{what}: HTTP {status}, reading the reply: {}",
                chain(&e)
            ))
        };
        if status >= 400 {
            let bytes = res.bytes().await.map_err(read_err)?;
            let env: Option<Envelope<Value>> = serde_json::from_slice(&bytes).ok();
            let message = env.map_or_else(
                || format!("{what}: HTTP {status} with a reply that is not from the Incus API"),
                |e| format!("{what}: {}", e.error),
            );
            return Err(match status {
                401 | 403 => AdapterError::Authentication(message),
                404 => AdapterError::NotFound(path.to_owned()),
                _ => AdapterError::Api { status, message },
            });
        }
        let mut kept: Vec<u8> = Vec::new();
        let mut size: u64 = 0;
        while let Some(chunk) = res.chunk().await.map_err(read_err)? {
            size += chunk.len() as u64;
            if size > RAW_LIMIT {
                return Err(AdapterError::Api {
                    status: 413,
                    message: format!(
                        "{what}: larger than {} MiB; read it on the server",
                        RAW_LIMIT >> 20
                    ),
                });
            }
            kept.extend_from_slice(&chunk);
            if kept.len() > max.saturating_mul(2) {
                kept.drain(..kept.len() - max);
            }
        }
        Ok(LogTail::from_bytes(&kept, size, max))
    }

    async fn require_running(&self, project: &str, name: &str) -> Result<(), AdapterError> {
        match self.instance(project, name).await? {
            None => Err(AdapterError::NotFound(format!("instance {project}/{name}"))),
            Some(s) if !s.managed => Err(AdapterError::Api {
                status: 409,
                message: "the instance is not managed by Apelora".into(),
            }),
            Some(s) if s.status != "Running" => Err(AdapterError::Api {
                status: 409,
                message: format!("the instance is {}", s.status.to_lowercase()),
            }),
            Some(_) => Ok(()),
        }
    }

    /// Reads a small file from a running instance Apelora owns.
    pub(crate) async fn read_instance_file(
        &self,
        project: &str,
        name: &str,
        path: &str,
    ) -> Result<String, AdapterError> {
        self.require_running(project, name).await?;
        let what = format!("GET file {path}");
        let res = self
            .http
            .get(format!(
                "{}/1.0/instances/{name}/files?{}",
                self.base,
                file_query(path, project)
            ))
            .send()
            .await
            .map_err(|e| AdapterError::Connectivity(format!("{what}: {}", chain(&e))))?;
        let status = res.status().as_u16();
        let bytes = res.bytes().await.map_err(|e| {
            AdapterError::Connectivity(format!("{what}: HTTP {status}: {}", chain(&e)))
        })?;
        if status >= 400 {
            return Err(AdapterError::Api {
                status,
                message: format!("{what}: HTTP {status}"),
            });
        }
        if bytes.len() > 8192 {
            return Err(AdapterError::Api {
                status: 413,
                message: format!("{what}: larger than 8192 bytes"),
            });
        }
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    }

    /// The `error` field of an Incus JSON body, or the body itself, shortened.
    fn body_error(body: &str) -> String {
        let text = serde_json::from_str::<serde_json::Value>(body)
            .ok()
            .and_then(|v| {
                v.get("error")
                    .and_then(|e| e.as_str())
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| body.trim().to_owned());
        let mut short: String = text.chars().take(240).collect();
        if text.chars().count() > 240 {
            short.push('…');
        }
        if short.is_empty() {
            "no detail".into()
        } else {
            short
        }
    }

    /// Writes a file into a running instance Apelora owns, replacing it when it exists.
    async fn put_instance_file(
        &self,
        project: &str,
        name: &str,
        path: &str,
        mode: u32,
        bytes: &[u8],
    ) -> Result<(), AdapterError> {
        self.require_running(project, name).await?;
        let what = format!("POST file {path}");
        let res = self
            .http
            .post(format!(
                "{}/1.0/instances/{name}/files?{}",
                self.base,
                file_query(path, project)
            ))
            .header("X-Incus-uid", "0")
            .header("X-Incus-gid", "0")
            .header("X-Incus-mode", format!("{mode:04o}"))
            .header("X-Incus-type", "file")
            .header("X-Incus-write", "overwrite")
            .header("Content-Type", "application/octet-stream")
            .body(bytes.to_vec())
            .send()
            .await
            .map_err(|e| AdapterError::Connectivity(format!("{what}: {}", chain(&e))))?;
        let status = res.status().as_u16();
        if status >= 400 {
            let body = res.text().await.unwrap_or_default();
            let detail = Self::body_error(&body);
            return Err(AdapterError::Api {
                status,
                message: format!("{what}: HTTP {status}: {detail}"),
            });
        }
        Ok(())
    }

    /// Puts the resolver binary, its records and its forwarders in place, and the unit that
    /// runs it. The caller starts the unit.
    pub(crate) async fn install_dns_resolver(
        &self,
        project: &str,
        name: &str,
        records: &str,
        upstream: &str,
        binary: &[u8],
    ) -> Result<(), AdapterError> {
        if binary.len() < 4 || &binary[..4] != b"\x7fELF" {
            return Err(AdapterError::InvalidConfig(
                "apelora-dns is not a Linux executable".into(),
            ));
        }
        // A running executable cannot be replaced in place ("text file busy"). The caller
        // renames this onto `/usr/local/bin/apelora-dns` after the write.
        self.put_instance_file(
            project,
            name,
            "/usr/local/bin/apelora-dns.new",
            0o755,
            binary,
        )
        .await?;
        self.put_instance_file(
            project,
            name,
            "/etc/apelora-dns.records",
            0o644,
            records.as_bytes(),
        )
        .await?;
        self.put_instance_file(
            project,
            name,
            "/etc/apelora-dns.upstream",
            0o644,
            upstream.as_bytes(),
        )
        .await?;
        self.put_instance_file(
            project,
            name,
            "/etc/systemd/system/apelora-dns.service",
            0o644,
            DNS_UNIT.as_bytes(),
        )
        .await?;
        // The directory is created by the caller before this write.
        self.put_instance_file(
            project,
            name,
            "/etc/systemd/resolved.conf.d/apelora.conf",
            0o644,
            RESOLVED_DROPIN.as_bytes(),
        )
        .await
    }

    /// `POST /1.0/instances/<name>/exec` without a terminal, then waits for the operation and
    /// reads the command's exit status from its metadata. The instance must be Apelora's and
    /// running (checked first). A command still running at the time limit is cancelled.
    pub(crate) async fn run_probe(
        &self,
        project: &str,
        name: &str,
        command: &[String],
        uid: u32,
        timeout_secs: u8,
    ) -> Result<crate::ProbeRun, AdapterError> {
        self.require_running(project, name).await?;
        let body = json!({
            "command": command,
            "environment": { "PATH": "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin" },
            "wait-for-websocket": false,
            "interactive": false,
            "record-output": false,
            "user": uid,
            "group": uid,
            "cwd": "/",
        });
        let id = self
            .start_operation(
                Method::POST,
                &format!("/1.0/instances/{name}/exec?project={project}"),
                Some(&body),
            )
            .await?;
        if !is_operation_id(&id) {
            return Err(AdapterError::Decode("the exec operation has no id".into()));
        }
        let waited: Value = self
            .get(&format!(
                "/1.0/operations/{id}/wait?timeout={}",
                timeout_secs.clamp(1, 15)
            ))
            .await?;
        match waited["status"].as_str().unwrap_or_default() {
            "Success" => Ok(crate::ProbeRun {
                exit: waited["metadata"]["return"]
                    .as_i64()
                    .and_then(|r| i32::try_from(r).ok()),
                timed_out: false,
                error: None,
            }),
            "Failure" | "Cancelled" => Ok(crate::ProbeRun {
                exit: None,
                timed_out: false,
                error: Some(
                    waited["err"]
                        .as_str()
                        .filter(|e| !e.is_empty())
                        .unwrap_or("Incus could not run the command")
                        .to_owned(),
                ),
            }),
            // Still running at the limit: stop it, and say it was too slow.
            _ => {
                let _ = self
                    .call::<Value>(Method::DELETE, &format!("/1.0/operations/{id}"), None)
                    .await;
                Ok(crate::ProbeRun {
                    exit: None,
                    timed_out: true,
                    error: None,
                })
            }
        }
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
        let started = std::time::Instant::now();
        let result = self
            .get(&format!("/1.0/operations/{id}/wait?timeout={timeout_secs}"))
            .await;
        apelora_metrics::INCUS_OPERATION_WAIT.observe(&[], started.elapsed().as_secs_f64());
        result
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

    /// Cluster groups with their members (`GET /1.0/cluster/groups?recursion=1`).
    pub async fn cluster_groups(
        &self,
    ) -> Result<Vec<apelora_domain::clustering::ClusterGroupInfo>, AdapterError> {
        let groups: Vec<wire::ClusterGroup> = self.get("/1.0/cluster/groups?recursion=1").await?;
        Ok(groups
            .into_iter()
            .map(|g| apelora_domain::clustering::ClusterGroupInfo {
                name: g.name,
                description: g.description,
                members: g.members.unwrap_or_default(),
            })
            .collect())
    }

    /// The values of `keys` in the server configuration (`GET /1.0`); keys that are not set
    /// are absent. Only an unrestricted identity sees the configuration. Nothing outside
    /// `keys` is returned: the configuration also holds secrets.
    pub async fn server_config(
        &self,
        keys: &[&str],
    ) -> Result<std::collections::BTreeMap<String, String>, AdapterError> {
        let s: wire::ServerConfig = self.get("/1.0").await?;
        Ok(s.config
            .unwrap_or_default()
            .into_iter()
            .filter(|(k, _)| keys.contains(&k.as_str()))
            .map(|(k, v)| {
                let v = match v {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                (k, v)
            })
            .collect())
    }

    /// Changes cluster settings: `PATCH /1.0` with only these keys. A `None` removes the key so
    /// Incus's default applies again. Every key must be one Apelora changes and every value
    /// must pass `check_setting`; anything else is refused before a request is made. Needs an
    /// unrestricted identity.
    pub async fn set_server_config(
        &self,
        changes: &std::collections::BTreeMap<String, Option<String>>,
    ) -> Result<(), AdapterError> {
        let config = settings_body(changes)?;
        self.call::<Value>(Method::PATCH, "/1.0", Some(&json!({ "config": config })))
            .await
            .map(|_| ())
    }

    /// Incus's documented defaults for server configuration keys
    /// (`GET /1.0/metadata/configuration`, readable by the restricted identity), with the
    /// formatting marks removed: `` `20` `` becomes `20`.
    pub async fn config_defaults(
        &self,
    ) -> Result<std::collections::BTreeMap<String, String>, AdapterError> {
        let meta: Value = self.get("/1.0/metadata/configuration").await?;
        Ok(config_defaults_from(&meta))
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
                    owner: match (
                        config.get(APPLICATION_KEY),
                        apelora_domain::module_label(&config),
                    ) {
                        (Some(application), Some(module)) => Some(Owner {
                            application: application.clone(),
                            module: module.clone(),
                            replica: config
                                .get(REPLICA_KEY)
                                .and_then(|r| r.parse().ok())
                                .unwrap_or(1),
                        }),
                        _ => None,
                    },
                    evacuate: expanded.get("cluster.evacuate").cloned(),
                    limits: apelora_domain::plan::RESIZABLE_KEYS
                        .iter()
                        .filter_map(|k| config.get(*k).map(|v| ((*k).to_owned(), v.clone())))
                        .collect(),
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

    /// One server's resources, read-only: `GET /1.0/resources` (CPU threads, memory) and, for a
    /// cluster member, `GET /1.0/cluster/members/<m>/state` (load, uptime, each pool's space);
    /// a standalone server's pools are read with `GET /1.0/storage-pools/<p>/resources`.
    pub async fn server_resources(
        &self,
        member: Option<&str>,
    ) -> Result<apelora_domain::capacity::HealthServerResources, AdapterError> {
        use apelora_domain::capacity::{HealthPoolUsage, HealthServerResources, HealthUsage};
        let pool = |name: String, s: wire::PoolState| HealthPoolUsage {
            name,
            used_bytes: s.space.used,
            total_bytes: s.space.total,
        };
        let (res, load_averages, uptime_secs, pools) = match member {
            Some(m) => {
                let (res_path, state_path) = (
                    format!("/1.0/resources?target={m}"),
                    format!("/1.0/cluster/members/{m}/state"),
                );
                let (res, state) = tokio::try_join!(
                    self.get::<wire::Resources>(&res_path),
                    self.get::<wire::MemberState>(&state_path),
                )?;
                let pools = state
                    .storage_pools
                    .into_iter()
                    .map(|(n, s)| pool(n, s))
                    .collect();
                (
                    res,
                    state.sysinfo.load_averages,
                    u64::try_from(state.sysinfo.uptime).ok(),
                    pools,
                )
            }
            None => {
                let res: wire::Resources = self.get("/1.0/resources").await?;
                let mut pools = Vec::new();
                for p in self.storage_pools().await? {
                    let s: wire::PoolState = self
                        .get(&format!("/1.0/storage-pools/{}/resources", p.name))
                        .await?;
                    pools.push(pool(p.name, s));
                }
                (res, Vec::new(), None, pools)
            }
        };
        Ok(HealthServerResources {
            cpu_threads: Some(res.cpu.total).filter(|t| *t > 0),
            load_averages,
            memory: Some(HealthUsage {
                used_bytes: res.memory.used,
                total_bytes: res.memory.total,
            })
            .filter(|m| m.total_bytes > 0),
            pools,
            uptime_secs,
        })
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
