// SPDX-License-Identifier: AGPL-3.0-only
//! Incus projects for Apelora's environments (ADR 0012, `docs/design/tenancy-isolation.md`):
//! reading a project, its use and limits, its default profile; creating and updating projects
//! and profiles; and the project list of a restricted client certificate.
//!
//! Incus refuses a restricted certificate every project change and every certificate change
//! (`internal/server/auth/driver_tls.go`), so the writes here are called on the **admin**
//! provider, in the control plane only. Reading a project and its state works with the
//! restricted deploy identity for the projects it may use.
//!
//! Every call is synchronous in Incus (no operation to wait on). Wire shapes follow the pinned
//! release's `doc/rest-api.yaml`: `ProjectsPost`, `ProjectPut`, `ProjectState`, `ProfilePut`,
//! `CertificatePut`.

use std::collections::BTreeMap;

use apelora_domain::is_valid_name;
use apelora_domain::tenancy::{
    IncusTrustEntry, TenancyIncusProject, TenancyProjectUse, TenancyResource,
};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Value, json};

use crate::client::IncusClient;
use crate::{AdapterError, MockIncus, Provider};

/// The deploy identity's fingerprint as the mock knows it.
pub const MOCK_DEPLOY_FINGERPRINT: &str =
    "0000000000000000000000000000000000000000000000000000000000000d01";

/// A profile's configuration and devices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IncusProfile {
    pub description: String,
    pub config: BTreeMap<String, String>,
    pub devices: BTreeMap<String, BTreeMap<String, String>>,
}

fn check_project(name: &str) -> Result<(), AdapterError> {
    if is_valid_name(name) && name != "default" {
        Ok(())
    } else {
        Err(AdapterError::InvalidConfig(format!(
            "invalid project name {name:?}"
        )))
    }
}

fn check_fingerprint(fp: &str) -> Result<(), AdapterError> {
    if (12..=64).contains(&fp.len()) && fp.bytes().all(|b| b.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(AdapterError::InvalidConfig(
            "a certificate fingerprint is 12–64 hexadecimal characters".into(),
        ))
    }
}

/// Configuration keys and values must be printable and short; nothing else reaches Incus.
fn check_config(config: &BTreeMap<String, String>) -> Result<(), AdapterError> {
    for (k, v) in config {
        let key_ok = !k.is_empty()
            && k.len() <= 128
            && k.bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'));
        if !key_ok || v.len() > 4096 || v.chars().any(char::is_control) {
            return Err(AdapterError::InvalidConfig(format!(
                "invalid configuration key or value: {k:?}"
            )));
        }
    }
    Ok(())
}

// ---- wire ---------------------------------------------------------------------------------------

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct WireProject {
    name: String,
    description: String,
    config: BTreeMap<String, String>,
}

/// `ProjectStateResource` has no JSON tags in Incus, so its fields are `Limit` and `Usage`;
/// lower case is accepted too.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct WireResource {
    #[serde(alias = "limit", rename = "Limit")]
    limit: i64,
    #[serde(alias = "usage", rename = "Usage")]
    usage: i64,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct WireProjectState {
    resources: BTreeMap<String, WireResource>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct WireProfile {
    description: String,
    config: BTreeMap<String, String>,
    devices: BTreeMap<String, BTreeMap<String, String>>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct WireCertificate {
    fingerprint: String,
    name: String,
    restricted: bool,
    /// `null` for an unrestricted certificate.
    projects: Option<Vec<String>>,
}

fn resource(r: Option<&WireResource>) -> TenancyResource {
    r.map_or_else(TenancyResource::default, |r| TenancyResource {
        used: r.usage.max(0),
        limit: (r.limit >= 0).then_some(r.limit),
    })
}

fn project_use(s: &WireProjectState) -> TenancyProjectUse {
    TenancyProjectUse {
        instances: resource(s.resources.get("instances")),
        cpu: resource(s.resources.get("cpu")),
        memory: resource(s.resources.get("memory")),
        disk: resource(s.resources.get("disk")),
    }
}

async fn metadata<T: serde::de::DeserializeOwned>(
    c: &IncusClient,
    path: &str,
) -> Result<T, AdapterError> {
    c.call::<T>(Method::GET, path, None)
        .await?
        .metadata
        .ok_or_else(|| AdapterError::Decode(format!("{path}: response has no metadata")))
}

impl IncusClient {
    async fn tenancy_project(
        &self,
        name: &str,
    ) -> Result<Option<TenancyIncusProject>, AdapterError> {
        match metadata::<WireProject>(self, &format!("/1.0/projects/{name}")).await {
            Ok(p) => Ok(Some(TenancyIncusProject {
                name: p.name,
                description: p.description,
                config: p.config,
            })),
            Err(AdapterError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn create_tenancy_project(
        &self,
        name: &str,
        description: &str,
        config: &BTreeMap<String, String>,
    ) -> Result<(), AdapterError> {
        let body = json!({ "name": name, "description": description, "config": config });
        self.call::<Value>(Method::POST, "/1.0/projects", Some(&body))
            .await
            .map(|_| ())
    }

    async fn put_tenancy_project(
        &self,
        name: &str,
        description: &str,
        config: &BTreeMap<String, String>,
    ) -> Result<(), AdapterError> {
        let body = json!({ "description": description, "config": config });
        self.call::<Value>(Method::PUT, &format!("/1.0/projects/{name}"), Some(&body))
            .await
            .map(|_| ())
    }

    async fn delete_tenancy_project(&self, name: &str) -> Result<(), AdapterError> {
        self.call::<Value>(Method::DELETE, &format!("/1.0/projects/{name}"), None)
            .await
            .map(|_| ())
    }

    async fn tenancy_project_use(&self, name: &str) -> Result<TenancyProjectUse, AdapterError> {
        let s: WireProjectState = metadata(self, &format!("/1.0/projects/{name}/state")).await?;
        Ok(project_use(&s))
    }

    async fn tenancy_profile(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Option<IncusProfile>, AdapterError> {
        match metadata::<WireProfile>(self, &format!("/1.0/profiles/{name}?project={project}"))
            .await
        {
            Ok(p) => Ok(Some(IncusProfile {
                description: p.description,
                config: p.config,
                devices: p.devices,
            })),
            Err(AdapterError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn put_tenancy_profile(
        &self,
        project: &str,
        name: &str,
        profile: &IncusProfile,
    ) -> Result<(), AdapterError> {
        let body = json!({ "description": profile.description, "config": profile.config, "devices": profile.devices });
        self.call::<Value>(
            Method::PUT,
            &format!("/1.0/profiles/{name}?project={project}"),
            Some(&body),
        )
        .await
        .map(|_| ())
    }

    async fn trust_entry(
        &self,
        fingerprint: &str,
    ) -> Result<Option<IncusTrustEntry>, AdapterError> {
        match metadata::<WireCertificate>(self, &format!("/1.0/certificates/{fingerprint}")).await {
            Ok(c) => Ok(Some(IncusTrustEntry {
                fingerprint: c.fingerprint,
                name: c.name,
                restricted: c.restricted,
                projects: c.projects.unwrap_or_default(),
            })),
            Err(AdapterError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn set_trust_projects(
        &self,
        fingerprint: &str,
        projects: &[String],
    ) -> Result<(), AdapterError> {
        // Incus merges a PATCH into the stored entry: only the project list changes.
        let body = json!({ "projects": projects });
        self.call::<Value>(
            Method::PATCH,
            &format!("/1.0/certificates/{fingerprint}"),
            Some(&body),
        )
        .await
        .map(|_| ())
    }
}

// ---- the mock -------------------------------------------------------------------------------------

/// What the mock knows about projects Apelora manages, profiles, the trust store and use.
#[derive(Debug, Clone, Default)]
pub struct MockTenancy {
    pub projects: BTreeMap<String, TenancyIncusProject>,
    /// `(project, profile)` → profile.
    pub profiles: BTreeMap<(String, String), IncusProfile>,
    /// Fingerprint → trust entry; seeded with the deploy identity restricted to `apelora`.
    pub trust: BTreeMap<String, IncusTrustEntry>,
    /// Extra use per project that tests set (CPUs, memory, disk), on top of the instances
    /// created through the mock (counted from `instances`).
    pub usage: BTreeMap<String, apelora_domain::tenancy::QuotaUsage>,
    /// `(project, instance)` → what that instance counts against its project's limits.
    pub instances: BTreeMap<(String, String), apelora_domain::tenancy::QuotaUsage>,
    /// Makes the next project creation fail (tests of the failure path).
    pub fail_next_create: Option<String>,
    /// Every change, in order: `"POST /1.0/projects acme-shop-dev"` and the like.
    pub calls: Vec<String>,
}

impl MockTenancy {
    pub fn seeded() -> Self {
        let mut t = MockTenancy::default();
        t.trust.insert(
            MOCK_DEPLOY_FINGERPRINT.into(),
            IncusTrustEntry {
                fingerprint: MOCK_DEPLOY_FINGERPRINT.into(),
                name: "mock-deploy".into(),
                restricted: true,
                projects: vec!["apelora".into()],
            },
        );
        let root: BTreeMap<String, String> =
            [("type", "disk"), ("path", "/"), ("pool", "mock-local")]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect();
        let eth0: BTreeMap<String, String> =
            [("type", "nic"), ("name", "eth0"), ("network", "mock-br0")]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect();
        t.profiles.insert(
            ("default".into(), "default".into()),
            IncusProfile {
                description: "mock data".into(),
                config: BTreeMap::new(),
                devices: [("root".to_owned(), root), ("eth0".to_owned(), eth0)].into(),
            },
        );
        t
    }

    /// CPUs, memory and disk in use in a project (instances are counted separately).
    fn used(&self, project: &str) -> apelora_domain::tenancy::QuotaUsage {
        apelora_domain::tenancy::QuotaUsage::sum(
            self.instances
                .iter()
                .filter(|((p, _), _)| p == project)
                .map(|(_, u)| *u)
                .chain(self.usage.get(project).copied()),
        )
    }

    /// The limit a project sets for `key`, if any (`limits.instances`, …).
    fn limit(&self, project: &str, key: &str) -> Option<i64> {
        let v = self.projects.get(project)?.config.get(key)?;
        match key {
            "limits.memory" | "limits.disk" => apelora_domain::tenancy::parse_bytes(v),
            _ => v.parse().ok(),
        }
    }
}

impl MockIncus {
    pub fn tenancy(&self) -> std::sync::MutexGuard<'_, MockTenancy> {
        self.tenancy.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Like Incus, refuses an instance its project's limits do not leave room for, and
    /// counts what a created one uses.
    pub(crate) fn tenancy_admit(
        &self,
        spec: &apelora_domain::InstanceSpec,
        existing: usize,
    ) -> Result<(), AdapterError> {
        let mut t = self.tenancy();
        let p = spec.project.as_str();
        let add = apelora_domain::tenancy::instance_usage(
            &spec.config,
            spec.volumes.iter().map(|v| i64::from(v.size_mib)).sum(),
            if t.projects.contains_key(p) {
                apelora_domain::tenancy::DEFAULT_ROOT_DISK_GIB * apelora_domain::tenancy::GIB
            } else {
                0
            },
        );
        let used = t.used(p);
        if let Some(max) = t.limit(p, "limits.instances")
            && i64::try_from(existing).unwrap_or(i64::MAX) + 1 > max
        {
            return Err(AdapterError::Api {
                status: 403,
                message: format!("Reached maximum number of instances in project {p:?}"),
            });
        }
        for (key, have, more) in [
            ("limits.cpu", used.cpu, add.cpu),
            ("limits.memory", used.memory_bytes, add.memory_bytes),
            ("limits.disk", used.disk_bytes, add.disk_bytes),
        ] {
            if let Some(max) = t.limit(p, key)
                && have + more > max
            {
                return Err(AdapterError::Api {
                    status: 403,
                    message: format!(
                        "Reached maximum aggregate value for {key:?} in project {p:?}"
                    ),
                });
            }
        }
        t.instances.insert(
            (p.to_owned(), spec.name.clone()),
            apelora_domain::tenancy::QuotaUsage {
                instances: 0,
                ..add
            },
        );
        Ok(())
    }

    /// An instance was deleted: it no longer counts.
    pub(crate) fn tenancy_release(&self, project: &str, name: &str) {
        self.tenancy()
            .instances
            .remove(&(project.to_owned(), name.to_owned()));
    }

    fn mock_project_use(&self, name: &str) -> TenancyProjectUse {
        let instances = self
            .lock()
            .iter()
            .filter(|i| i.instance.project == name)
            .count();
        let t = self.tenancy();
        let u = t.used(name);
        let r = |used: i64, key: &str| TenancyResource {
            used,
            limit: t.limit(name, key),
        };
        TenancyProjectUse {
            instances: r(
                i64::try_from(instances).unwrap_or(i64::MAX),
                "limits.instances",
            ),
            cpu: r(u.cpu, "limits.cpu"),
            memory: r(u.memory_bytes, "limits.memory"),
            disk: r(u.disk_bytes, "limits.disk"),
        }
    }
}

// ---- the provider ---------------------------------------------------------------------------------

impl Provider {
    /// The fingerprint the mock uses for the deploy identity; real fingerprints are computed
    /// by the caller from the certificate (SHA-256 of its DER form).
    pub fn mock_deploy_fingerprint(&self) -> Option<&'static str> {
        matches!(self, Provider::Mock(_)).then_some(MOCK_DEPLOY_FINGERPRINT)
    }

    /// A project and its configuration; `None` when it does not exist. With the restricted
    /// identity, a project it may not use answers [`AdapterError::Authentication`].
    pub async fn tenancy_project(
        &self,
        name: &str,
    ) -> Result<Option<TenancyIncusProject>, AdapterError> {
        if !is_valid_name(name) {
            return Err(AdapterError::InvalidConfig(format!(
                "invalid project name {name:?}"
            )));
        }
        match self {
            Provider::Mock(m) => {
                if let Some(p) = m.tenancy().projects.get(name) {
                    return Ok(Some(p.clone()));
                }
                // The seeded projects (`default`, `apelora`) as the mock lists them.
                Ok(m.projects.iter().find(|p| p.name == name).map(|p| {
                    let mut config = BTreeMap::new();
                    if p.restricted {
                        config.insert("restricted".to_owned(), "true".to_owned());
                        config.insert("restricted.snapshots".to_owned(), "allow".to_owned());
                        config.insert("restricted.cluster.target".to_owned(), "allow".to_owned());
                    }
                    TenancyIncusProject {
                        name: p.name.clone(),
                        description: p.description.clone(),
                        config,
                    }
                }))
            }
            Provider::Incus(c) => c.tenancy_project(name).await,
        }
    }

    /// Creates a project (admin identity). Fails if it exists.
    pub async fn create_tenancy_project(
        &self,
        name: &str,
        description: &str,
        config: &BTreeMap<String, String>,
    ) -> Result<(), AdapterError> {
        check_project(name)?;
        check_config(config)?;
        match self {
            Provider::Mock(m) => {
                let mut t = m.tenancy();
                if let Some(msg) = t.fail_next_create.take() {
                    return Err(AdapterError::Api {
                        status: 500,
                        message: msg,
                    });
                }
                if t.projects.contains_key(name) || m.projects.iter().any(|p| p.name == name) {
                    return Err(AdapterError::Api {
                        status: 409,
                        message:
                            "Error inserting into database: UNIQUE constraint failed: projects.name"
                                .into(),
                    });
                }
                t.calls.push(format!("POST /1.0/projects {name}"));
                t.projects.insert(
                    name.to_owned(),
                    TenancyIncusProject {
                        name: name.to_owned(),
                        description: description.to_owned(),
                        config: config.clone(),
                    },
                );
                // Incus gives a project with its own profiles an empty `default` profile.
                if config.get("features.profiles").is_some_and(|v| v == "true") {
                    t.profiles.insert(
                        (name.to_owned(), "default".to_owned()),
                        IncusProfile::default(),
                    );
                }
                Ok(())
            }
            Provider::Incus(c) => c.create_tenancy_project(name, description, config).await,
        }
    }

    /// Replaces a project's description and configuration (admin identity). Incus refuses a
    /// limit below what the project already uses.
    pub async fn update_tenancy_project(
        &self,
        name: &str,
        description: &str,
        config: &BTreeMap<String, String>,
    ) -> Result<(), AdapterError> {
        check_project(name)?;
        check_config(config)?;
        match self {
            Provider::Mock(m) => {
                let using = m.mock_project_use(name);
                let mut t = m.tenancy();
                if !t.projects.contains_key(name) {
                    // A seeded project becomes a managed one the first time it is changed.
                    let Some(seed) = m.projects.iter().find(|p| p.name == name) else {
                        return Err(AdapterError::NotFound(format!("/1.0/projects/{name}")));
                    };
                    t.projects.insert(
                        name.to_owned(),
                        TenancyIncusProject {
                            name: seed.name.clone(),
                            description: seed.description.clone(),
                            config: BTreeMap::new(),
                        },
                    );
                }
                let limit = |k: &str| {
                    config.get(k).and_then(|v| match k {
                        "limits.memory" | "limits.disk" => apelora_domain::tenancy::parse_bytes(v),
                        _ => v.parse::<i64>().ok(),
                    })
                };
                for (key, used) in [
                    ("limits.instances", using.instances.used),
                    ("limits.cpu", using.cpu.used),
                    ("limits.memory", using.memory.used),
                    ("limits.disk", using.disk.used),
                ] {
                    if let Some(l) = limit(key)
                        && l < used
                    {
                        return Err(AdapterError::Api {
                            status: 400,
                            message: format!("{key:?} is too low: current total is {used}"),
                        });
                    }
                }
                t.calls.push(format!("PUT /1.0/projects/{name}"));
                let p = t.projects.get_mut(name).expect("inserted above");
                p.description = description.to_owned();
                p.config = config.clone();
                Ok(())
            }
            Provider::Incus(c) => c.put_tenancy_project(name, description, config).await,
        }
    }

    /// Deletes an empty project (admin identity). Used by lab tests to clean up.
    pub async fn delete_tenancy_project(&self, name: &str) -> Result<(), AdapterError> {
        check_project(name)?;
        match self {
            Provider::Mock(m) => {
                let mut t = m.tenancy();
                t.calls.push(format!("DELETE /1.0/projects/{name}"));
                t.projects.remove(name);
                t.profiles.retain(|(p, _), _| p != name);
                for e in t.trust.values_mut() {
                    e.projects.retain(|p| p != name);
                }
                Ok(())
            }
            Provider::Incus(c) => c.delete_tenancy_project(name).await,
        }
    }

    /// A project's use and limits (`GET /1.0/projects/<name>/state`). The deploy identity may
    /// read it for the projects it can use.
    pub async fn tenancy_project_use(&self, name: &str) -> Result<TenancyProjectUse, AdapterError> {
        if !is_valid_name(name) {
            return Err(AdapterError::InvalidConfig(format!(
                "invalid project name {name:?}"
            )));
        }
        match self {
            Provider::Mock(m) => {
                let known = m.tenancy().projects.contains_key(name)
                    || m.projects.iter().any(|p| p.name == name);
                if !known {
                    return Err(AdapterError::NotFound(format!(
                        "/1.0/projects/{name}/state"
                    )));
                }
                Ok(m.mock_project_use(name))
            }
            Provider::Incus(c) => c.tenancy_project_use(name).await,
        }
    }

    /// A profile of a project; `None` when it does not exist.
    pub async fn tenancy_profile(
        &self,
        project: &str,
        name: &str,
    ) -> Result<Option<IncusProfile>, AdapterError> {
        if !is_valid_name(project) || !is_valid_name(name) {
            return Err(AdapterError::InvalidConfig(
                "invalid project or profile name".into(),
            ));
        }
        match self {
            Provider::Mock(m) => Ok(m
                .tenancy()
                .profiles
                .get(&(project.to_owned(), name.to_owned()))
                .cloned()),
            Provider::Incus(c) => c.tenancy_profile(project, name).await,
        }
    }

    /// Replaces a profile's description, configuration and devices (admin identity).
    pub async fn update_tenancy_profile(
        &self,
        project: &str,
        name: &str,
        profile: &IncusProfile,
    ) -> Result<(), AdapterError> {
        check_project(project)?;
        if !is_valid_name(name) {
            return Err(AdapterError::InvalidConfig("invalid profile name".into()));
        }
        check_config(&profile.config)?;
        for d in profile.devices.values() {
            check_config(d)?;
        }
        match self {
            Provider::Mock(m) => {
                let mut t = m.tenancy();
                let key = (project.to_owned(), name.to_owned());
                if !t.profiles.contains_key(&key) {
                    return Err(AdapterError::NotFound(format!(
                        "/1.0/profiles/{name}?project={project}"
                    )));
                }
                t.calls
                    .push(format!("PUT /1.0/profiles/{name}?project={project}"));
                t.profiles.insert(key, profile.clone());
                Ok(())
            }
            Provider::Incus(c) => c.put_tenancy_profile(project, name, profile).await,
        }
    }

    /// A trusted certificate by fingerprint (admin identity); `None` when it is not trusted.
    pub async fn trust_entry(
        &self,
        fingerprint: &str,
    ) -> Result<Option<IncusTrustEntry>, AdapterError> {
        check_fingerprint(fingerprint)?;
        match self {
            Provider::Mock(m) => Ok(m.tenancy().trust.get(fingerprint).cloned()),
            Provider::Incus(c) => c.trust_entry(fingerprint).await,
        }
    }

    /// Sets the projects a restricted certificate may use (admin identity). The caller passes
    /// the whole list: read it with [`Provider::trust_entry`] and add to it.
    pub async fn set_trust_projects(
        &self,
        fingerprint: &str,
        projects: &[String],
    ) -> Result<(), AdapterError> {
        check_fingerprint(fingerprint)?;
        if let Some(bad) = projects.iter().find(|p| !is_valid_name(p)) {
            return Err(AdapterError::InvalidConfig(format!(
                "invalid project name {bad:?}"
            )));
        }
        match self {
            Provider::Mock(m) => {
                let mut t = m.tenancy();
                let e = t.trust.get_mut(fingerprint).ok_or_else(|| {
                    AdapterError::NotFound(format!("/1.0/certificates/{fingerprint}"))
                })?;
                e.projects = projects.to_vec();
                t.calls
                    .push(format!("PATCH /1.0/certificates/{fingerprint}"));
                Ok(())
            }
            Provider::Incus(c) => c.set_trust_projects(fingerprint, projects).await,
        }
    }
}

#[cfg(test)]
mod tests {
    //! Contract tests: the real client against a fixture server answering with the shapes of
    //! the pinned release's REST API. CONTRACT VERIFIED, not a real Incus.

    use std::sync::{Arc, Mutex};

    use axum::Router;
    use axum::body::Bytes;
    use axum::http::{Method as HttpMethod, StatusCode, Uri};
    use axum::response::IntoResponse;

    use super::*;

    type Seen = Arc<Mutex<Vec<(String, String, Value)>>>;

    fn sync(metadata: Value) -> (u16, Value) {
        (
            200,
            json!({ "type": "sync", "status": "Success", "status_code": 200, "operation": "", "error_code": 0, "error": "", "metadata": metadata }),
        )
    }

    /// Answers `"METHOD /path?query"` from the table, records every request with its body.
    async fn fixture(routes: Vec<(&'static str, (u16, Value))>) -> (String, Seen) {
        let table: Arc<BTreeMap<String, (u16, Value)>> =
            Arc::new(routes.into_iter().map(|(k, v)| (k.to_owned(), v)).collect());
        let seen: Seen = Arc::default();
        let log = seen.clone();
        let app = Router::new().fallback(move |method: HttpMethod, uri: Uri, body: Bytes| {
            let (table, log) = (table.clone(), log.clone());
            async move {
                let path = uri
                    .path_and_query()
                    .map(|p| p.as_str().to_owned())
                    .unwrap_or_default();
                let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
                log.lock()
                    .expect("log")
                    .push((method.to_string(), path.clone(), body));
                let (status, reply) = table.get(&format!("{method} {path}")).cloned().unwrap_or((
                    404,
                    json!({ "type": "error", "error": "not found", "error_code": 404 }),
                ));
                (
                    StatusCode::from_u16(status).expect("status"),
                    axum::Json(reply),
                )
                    .into_response()
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        (format!("http://{addr}"), seen)
    }

    #[tokio::test]
    async fn reads_a_project_its_state_with_incus_field_names_and_a_profile() {
        let (base, _) = fixture(vec![
            ("GET /1.0/projects/acme-shop-prod", sync(json!({ "name": "acme-shop-prod", "description": "d", "config": { "restricted": "true", "limits.cpu": "8" }, "used_by": [] }))),
            // `ProjectStateResource` has no JSON tags: `Limit`/`Usage`; -1 is "no limit".
            ("GET /1.0/projects/acme-shop-prod/state", sync(json!({ "resources": {
                "instances": { "Limit": 10, "Usage": 3 }, "cpu": { "Limit": 8, "Usage": 5 },
                "memory": { "Limit": -1, "Usage": 3_221_225_472_i64 }, "disk": { "limit": -1, "usage": 0 },
                "networks": { "Limit": -1, "Usage": 1 } } }))),
            ("GET /1.0/profiles/default?project=default", sync(json!({ "name": "default", "description": "Default profile", "config": {},
                "devices": { "root": { "type": "disk", "path": "/", "pool": "local" }, "eth0": { "type": "nic", "network": "incusbr0", "name": "eth0" } } }))),
        ])
        .await;
        let p = Provider::Incus(IncusClient::plain_for_tests(&base));
        let project = p
            .tenancy_project("acme-shop-prod")
            .await
            .expect("read")
            .expect("exists");
        assert_eq!(project.config["limits.cpu"], "8");
        assert_eq!(
            p.tenancy_project("acme-shop-dev").await.expect("read"),
            None
        );
        let u = p
            .tenancy_project_use("acme-shop-prod")
            .await
            .expect("state");
        assert_eq!((u.instances.used, u.instances.limit), (3, Some(10)));
        assert_eq!((u.cpu.used, u.cpu.limit), (5, Some(8)));
        assert_eq!(
            (u.memory.used, u.memory.limit),
            (3 * 1024 * 1024 * 1024, None)
        );
        assert_eq!(
            u.disk,
            TenancyResource {
                used: 0,
                limit: None
            }
        );
        let profile = p
            .tenancy_profile("default", "default")
            .await
            .expect("profile")
            .expect("exists");
        assert_eq!(profile.devices["root"]["pool"], "local");
    }

    #[tokio::test]
    async fn writes_projects_profiles_and_the_trust_list_with_the_documented_bodies() {
        let fp = "a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2";
        let (base, seen) = fixture(vec![
            ("POST /1.0/projects", sync(json!({}))),
            ("PUT /1.0/projects/acme-shop-prod", sync(json!({}))),
            ("PUT /1.0/profiles/default?project=acme-shop-prod", sync(json!({}))),
            ("GET /1.0/certificates/a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2",
                sync(json!({ "fingerprint": fp, "name": "apelora-deploy", "type": "client", "restricted": true, "projects": ["apelora"], "certificate": "-----BEGIN CERTIFICATE-----…" }))),
            ("PATCH /1.0/certificates/a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2c3d4e5f6a1b2", sync(json!({}))),
            ("DELETE /1.0/projects/acme-shop-prod", sync(json!({}))),
        ])
        .await;
        let p = Provider::Incus(IncusClient::plain_for_tests(&base));
        let config: BTreeMap<String, String> =
            [("restricted".to_owned(), "true".to_owned())].into();
        p.create_tenancy_project("acme-shop-prod", "Acme / Shop / Production", &config)
            .await
            .expect("create");
        p.update_tenancy_project("acme-shop-prod", "Acme / Shop / Production", &config)
            .await
            .expect("update");
        let profile = IncusProfile {
            description: "d".into(),
            config: [("limits.cpu".to_owned(), "1".to_owned())].into(),
            devices: [(
                "root".to_owned(),
                [("type".to_owned(), "disk".to_owned())].into(),
            )]
            .into(),
        };
        p.update_tenancy_profile("acme-shop-prod", "default", &profile)
            .await
            .expect("profile");
        let entry = p.trust_entry(fp).await.expect("trust").expect("trusted");
        assert!(entry.restricted);
        assert_eq!(entry.projects, vec!["apelora"]);
        p.set_trust_projects(fp, &["apelora".into(), "acme-shop-prod".into()])
            .await
            .expect("grant");
        p.delete_tenancy_project("acme-shop-prod")
            .await
            .expect("delete");
        let seen = seen.lock().expect("seen").clone();
        let bodies: Vec<(String, String, Value)> =
            seen.into_iter().filter(|(m, _, _)| m != "GET").collect();
        assert_eq!(
            bodies[0],
            (
                "POST".into(),
                "/1.0/projects".into(),
                json!({ "name": "acme-shop-prod", "description": "Acme / Shop / Production", "config": { "restricted": "true" } })
            )
        );
        assert_eq!(
            bodies[1].2,
            json!({ "description": "Acme / Shop / Production", "config": { "restricted": "true" } })
        );
        assert_eq!(bodies[2].2["devices"]["root"]["type"], "disk");
        assert_eq!(
            bodies[3],
            (
                "PATCH".into(),
                format!("/1.0/certificates/{fp}"),
                json!({ "projects": ["apelora", "acme-shop-prod"] })
            )
        );
        assert_eq!(
            (bodies[4].0.as_str(), bodies[4].1.as_str()),
            ("DELETE", "/1.0/projects/acme-shop-prod")
        );
    }

    #[tokio::test]
    async fn refuses_names_and_values_that_must_never_reach_incus() {
        let p = Provider::Incus(IncusClient::plain_for_tests("http://127.0.0.1:9"));
        let ok: BTreeMap<String, String> = BTreeMap::new();
        assert!(matches!(
            p.create_tenancy_project("default", "", &ok).await,
            Err(AdapterError::InvalidConfig(_))
        ));
        assert!(matches!(
            p.create_tenancy_project("../x", "", &ok).await,
            Err(AdapterError::InvalidConfig(_))
        ));
        let bad: BTreeMap<String, String> = [("limits cpu".to_owned(), "1".to_owned())].into();
        assert!(matches!(
            p.create_tenancy_project("a-b", "", &bad).await,
            Err(AdapterError::InvalidConfig(_))
        ));
        let ctl: BTreeMap<String, String> = [("user.x".to_owned(), "a\nb".to_owned())].into();
        assert!(matches!(
            p.update_tenancy_project("a-b", "", &ctl).await,
            Err(AdapterError::InvalidConfig(_))
        ));
        assert!(matches!(
            p.trust_entry("xyz").await,
            Err(AdapterError::InvalidConfig(_))
        ));
        assert!(matches!(
            p.set_trust_projects("abcdefabcdef", &["no/pe".into()])
                .await,
            Err(AdapterError::InvalidConfig(_))
        ));
    }

    #[tokio::test]
    async fn the_mock_enforces_limits_like_incus() {
        let m = MockIncus::default();
        let p = Provider::Mock(m.clone());
        let config: BTreeMap<String, String> =
            [("features.profiles", "true"), ("limits.instances", "1")]
                .into_iter()
                .map(|(k, v)| (k.to_owned(), v.to_owned()))
                .collect();
        p.create_tenancy_project("acme-shop-dev", "", &config)
            .await
            .expect("create");
        assert!(
            p.create_tenancy_project("acme-shop-dev", "", &config)
                .await
                .is_err(),
            "exists"
        );
        assert!(
            p.tenancy_profile("acme-shop-dev", "default")
                .await
                .expect("read")
                .is_some()
        );
        let mut spec = apelora_domain::InstanceSpec {
            project: "acme-shop-dev".into(),
            name: "one".into(),
            kind: apelora_domain::InstanceKind::Container,
            source: apelora_domain::ImageSource {
                protocol: apelora_domain::ImageProtocol::Simplestreams,
                server: "https://images.example".into(),
                alias: "debian/13".into(),
                fingerprint: None,
            },
            profiles: vec![],
            config: BTreeMap::new(),
            start: true,
            architectures: vec![],
            owner: None,
            network: None,
            volumes: vec![],
        };
        p.create_instance(&spec).await.expect("first fits");
        spec.name = "two".into();
        let refused = p.create_instance(&spec).await.expect_err("over the limit");
        assert!(
            refused
                .to_string()
                .contains("Reached maximum number of instances"),
            "{refused}"
        );
        let u = p.tenancy_project_use("acme-shop-dev").await.expect("use");
        assert_eq!(
            (u.instances.used, u.instances.limit, u.cpu.used),
            (1, Some(1), 1)
        );
        // A limit below use is refused, as Incus does.
        let lower: BTreeMap<String, String> =
            [("limits.instances".to_owned(), "0".to_owned())].into();
        assert!(
            p.update_tenancy_project("acme-shop-dev", "", &lower)
                .await
                .is_err()
        );
        // Grants and deletion.
        let mut list = p
            .trust_entry(MOCK_DEPLOY_FINGERPRINT)
            .await
            .expect("trust")
            .expect("seeded")
            .projects;
        list.push("acme-shop-dev".into());
        p.set_trust_projects(MOCK_DEPLOY_FINGERPRINT, &list)
            .await
            .expect("grant");
        p.delete_tenancy_project("acme-shop-dev")
            .await
            .expect("delete");
        assert_eq!(
            p.trust_entry(MOCK_DEPLOY_FINGERPRINT)
                .await
                .expect("trust")
                .expect("seeded")
                .projects,
            vec!["apelora"],
            "Incus removes a deleted project from restricted certificates"
        );
    }
}
