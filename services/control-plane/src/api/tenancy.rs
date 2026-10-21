// SPDX-License-Identifier: AGPL-3.0-only
//! Tenancy isolation and quotas (ADR 0012, `docs/design/tenancy-isolation.md`).
//!
//! - **Preparing an environment's Incus project**: created (or adopted when it exists and is
//!   restricted) with the admin identity, given its own default profile, and added to the
//!   deploy identity's project list; then read back with the deploy identity. The admin
//!   identity is only ever used here, in the control plane, synchronously (ADR 0003).
//! - **Quotas** on organisations, projects and environments: stored in Apelora, checked when a
//!   deployment is planned (`check_deployment`), and mirrored as the tightest applicable limit
//!   onto each environment's Incus project (`mirror_limits`).
//!
//! Audit: an Incus change happens first, then the record and its audit row commit together
//! (the "Incus first" exception of IDENTITY-AND-AUTHORIZATION §8); a quota change and its row
//! are one transaction, and the mirror runs after the commit.

use std::collections::{BTreeMap, BTreeSet};

use apelora_auth::{EnvKind, Resource};
use apelora_domain::tenancy::{
    self as t, ProvisionSettings, QuotaCheck, QuotaLimits, QuotaScopeInput, QuotaScopeKind,
    QuotaUsage, TenancyFinding, TenancyFindingSeverity, TenancyIncusState, TenancyProjectUse,
};
use apelora_incus_adapter::{AdapterError, IncusProfile, Provider};
use apelora_persistence::identity as idb;
use apelora_persistence::tenancy as db;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::{Extension, Json};
use serde::Serialize;
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use super::authz::{allow, allow_platform, audit_alone, audit_scoped, begin, commit, filter_for};
use super::deploy::store;
use super::{ApiError, AppState, ErrorBody, Principal, Source, source};

pub(super) const NO_ADMIN: &str = "no administrator identity is configured (APELORA_INCUS_ADMIN_CLIENT_CERT): Apelora cannot create Incus projects or set their limits";

/// The deploy identity's fingerprint (SHA-256 of its certificate), as Incus names it in its
/// trust store: lowercase hexadecimal of the DER form.
pub fn certificate_fingerprint(pem: &[u8]) -> Result<String, String> {
    use rustls_pki_types::CertificateDer;
    use rustls_pki_types::pem::PemObject;
    use sha2::{Digest, Sha256};
    let der = CertificateDer::from_pem_slice(pem).map_err(|e| format!("certificate: {e}"))?;
    Ok(Sha256::digest(der.as_ref())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

pub(super) fn settings(state: &AppState) -> ProvisionSettings {
    ProvisionSettings {
        uplink: state.network_uplink.as_deref().map(str::to_owned),
        service_range: state.service_range.as_deref().map(str::to_owned),
    }
}

fn deploy_fingerprint(state: &AppState) -> Option<String> {
    state
        .deploy_fingerprint
        .as_deref()
        .map(str::to_owned)
        .or_else(|| state.provider.mock_deploy_fingerprint().map(str::to_owned))
}

fn env_resource(chain: &db::EnvironmentChain) -> Resource {
    Resource {
        organization: Some(chain.organization_id),
        project: Some(chain.environment.project_id),
        environment: EnvKind::parse(&chain.environment.kind),
        ..Resource::default()
    }
}

fn incus_state(chain: &db::EnvironmentChain) -> TenancyIncusState {
    TenancyIncusState::parse(&chain.environment.incus_state).unwrap_or(TenancyIncusState::Pending)
}

/// What to say about an environment's Incus project: the recorded detail, or, when nothing was
/// recorded yet, why it is not prepared.
pub(super) fn detail_of(state: &AppState, e: &idb::EnvironmentRow) -> String {
    if !e.incus_detail.is_empty() || e.incus_state != TenancyIncusState::Pending.as_str() {
        return e.incus_detail.clone();
    }
    if state.admin.is_none() {
        return NO_ADMIN.into();
    }
    if e.incus_project
        .as_deref()
        .is_some_and(|p| state.deploy_projects.iter().any(|d| d == p))
    {
        return "a platform deploy project (APELORA_DEPLOY_PROJECTS), set up by the operator; Prepare Incus project checks it and adopts it".into();
    }
    "Apelora prepares it at the first deployment, or now with Prepare Incus project on Tenancy & quotas".into()
}

fn rfc3339(t: time::OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

// ---- reading use --------------------------------------------------------------------------------

/// An Incus project's use: with the deploy identity, or with the admin identity when the deploy
/// identity may not see it (a project not granted yet). `None` when it does not exist.
async fn read_use(
    state: &AppState,
    project: &str,
) -> Result<Option<TenancyProjectUse>, AdapterError> {
    match state.provider.tenancy_project_use(project).await {
        Ok(u) => Ok(Some(u)),
        Err(AdapterError::NotFound(_)) => Ok(None),
        Err(AdapterError::Authentication(e)) => match state.admin.as_deref() {
            Some(admin) => match admin.tenancy_project_use(project).await {
                Ok(u) => Ok(Some(u)),
                Err(AdapterError::NotFound(_)) => Ok(None),
                Err(e) => Err(e),
            },
            None => Err(AdapterError::Authentication(e)),
        },
        Err(e) => Err(e),
    }
}

/// An Incus project's configuration, read the same way as [`read_use`].
async fn read_project(
    state: &AppState,
    project: &str,
) -> Result<Option<t::TenancyIncusProject>, AdapterError> {
    match state.provider.tenancy_project(project).await {
        Err(AdapterError::Authentication(e)) => match state.admin.as_deref() {
            Some(admin) => admin.tenancy_project(project).await,
            None => Err(AdapterError::Authentication(e)),
        },
        other => other,
    }
}

/// The quotas of an environment, its project and its organisation.
async fn chain_quotas(
    pool: &apelora_persistence::Pool,
    chain: &db::EnvironmentChain,
) -> Result<[Option<QuotaLimits>; 3], ApiError> {
    let q = |kind, id| async move { db::quota(pool, kind, id).await.map(|r| r.map(|r| r.limits)) };
    Ok([
        q(QuotaScopeKind::Organization, chain.organization_id).await?,
        q(QuotaScopeKind::Project, chain.environment.project_id).await?,
        q(QuotaScopeKind::Environment, chain.environment.id).await?,
    ])
}

fn effective(quotas: &[Option<QuotaLimits>; 3]) -> QuotaLimits {
    QuotaLimits::tightest(quotas.iter().flatten())
}

// ---- deployments ----------------------------------------------------------------------------------

/// What one instance to create brings: its config and the size of its volumes in MiB.
pub(super) struct Adding<'a> {
    pub config: &'a BTreeMap<String, String>,
    pub volume_mib: i64,
    /// For a resize of an existing instance: the increase it brings, instead of a new
    /// instance's whole use.
    pub resize: Option<QuotaUsage>,
}

/// Checks a deployment into `environment` against the quotas above it. `None` when no quota
/// applies (nothing is read from Incus then). Fails closed: if the use of a scope with a quota
/// cannot be read, the caller gets `503 quota_unverifiable`.
pub(super) async fn check_deployment(
    state: &AppState,
    environment: Uuid,
    adding: &[Adding<'_>],
) -> Result<Option<QuotaCheck>, ApiError> {
    let super::Auth::Tokens(pool) = &state.auth else {
        return Ok(None);
    };
    let Some(chain) = db::environment_chain(pool, environment).await? else {
        return Ok(None);
    };
    let quotas = chain_quotas(pool, &chain).await?;
    if quotas.iter().all(Option::is_none) {
        return Ok(None);
    }
    let root = if incus_state(&chain) == TenancyIncusState::Created {
        t::DEFAULT_ROOT_DISK_GIB * t::GIB
    } else {
        0
    };
    let add = QuotaUsage::sum(adding.iter().map(|a| {
        a.resize
            .unwrap_or_else(|| t::instance_usage(a.config, a.volume_mib, root))
    }));
    let scopes = [
        (
            QuotaScopeKind::Organization,
            chain.organization_id,
            chain.organization_name.clone(),
            quotas[0],
        ),
        (
            QuotaScopeKind::Project,
            chain.environment.project_id,
            chain.project_name.clone(),
            quotas[1],
        ),
        (
            QuotaScopeKind::Environment,
            chain.environment.id,
            chain.environment.name.clone(),
            quotas[2],
        ),
    ];
    let mut cache: BTreeMap<String, QuotaUsage> = BTreeMap::new();
    let mut inputs = Vec::new();
    for (kind, id, name, quota) in scopes {
        let Some(limits) = quota else { continue };
        let mut used = QuotaUsage::default();
        for env in db::environments_under(pool, kind, id).await? {
            let Some(project) = env.environment.incus_project.as_deref() else {
                continue;
            };
            if !cache.contains_key(project) {
                let u = read_use(state, project).await.map_err(|e| {
                    ApiError::new(
                        StatusCode::SERVICE_UNAVAILABLE,
                        "quota_unverifiable",
                        format!(
                            "the {} quota of {name} cannot be checked: Incus project {project}: {e}",
                            kind.as_str()
                        ),
                    )
                })?;
                cache.insert(project.to_owned(), u.map(|u| u.usage()).unwrap_or_default());
            }
            used = used.plus(cache[project]);
        }
        inputs.push(QuotaScopeInput {
            kind,
            id: id.to_string(),
            name,
            limits,
            used,
        });
    }
    let c = t::check(&inputs, add);
    tracing::debug!(environment = %environment, allowed = c.allowed, "quota: deployment checked");
    Ok(Some(c))
}

/// The refusal for a deployment over quota.
pub(super) fn refuse(check: &QuotaCheck) -> ApiError {
    ApiError::new(
        StatusCode::CONFLICT,
        "quota_exceeded",
        format!(
            "nothing was deployed; {}",
            check
                .refusal()
                .unwrap_or_else(|| "a quota would be exceeded".into())
        ),
    )
}

/// Before the first deployment into an environment whose Incus project Apelora has not prepared:
/// prepare it now. Projects the platform deploys into (`APELORA_DEPLOY_PROJECTS`) are the
/// operator's and are left as they are; without an admin identity nothing changes (Incus itself
/// refuses what the deploy identity may not do).
pub(super) async fn ensure_ready(
    state: &AppState,
    p: &Principal,
    environment: Uuid,
) -> Result<(), ApiError> {
    let super::Auth::Tokens(pool) = &state.auth else {
        return Ok(());
    };
    let Some(chain) = db::environment_chain(pool, environment).await? else {
        return Ok(());
    };
    let platform = chain
        .environment
        .incus_project
        .as_deref()
        .is_some_and(|ip| state.deploy_projects.iter().any(|d| d == ip));
    if incus_state(&chain) != TenancyIncusState::Pending || platform || state.admin.is_none() {
        return Ok(());
    }
    let r = prepare(state, p, &chain).await?;
    if r.state == TenancyIncusState::Pending {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "incus_project_not_ready",
            format!("the environment's Incus project is not ready: {}", r.detail),
        ));
    }
    Ok(())
}

// ---- preparing the Incus project -------------------------------------------------------------------

#[derive(Serialize, ToSchema, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "kebab-case")]
pub enum TenancyLimitResult {
    /// Incus has the new limits.
    Applied,
    /// Incus already had them.
    Unchanged,
    /// Not tried (the project is not prepared, or no admin identity).
    Skipped,
    /// Incus refused them; `detail` has its message. The quota still holds in Apelora.
    Failed,
}

/// What mirroring the quotas did to one environment's Incus project.
#[derive(Serialize, ToSchema, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct TenancyLimitOutcome {
    pub environment_id: Uuid,
    pub environment: String,
    pub incus_project: Option<String>,
    pub result: TenancyLimitResult,
    /// The limit keys asked for (`limits.cpu` = `8`, …).
    pub limits: BTreeMap<String, String>,
    pub detail: String,
}

/// The result of preparing an environment's Incus project.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IncusProjectPrepared {
    pub environment_id: Uuid,
    pub incus_project: Option<String>,
    pub state: TenancyIncusState,
    pub detail: String,
    pub findings: Vec<TenancyFinding>,
    /// The quotas mirrored right after (absent when the project is not ready).
    pub limits: Option<TenancyLimitOutcome>,
}

/// Steps on the Incus side; `Err` is the reason the environment stays pending.
async fn attempt(
    state: &AppState,
    chain: &db::EnvironmentChain,
) -> Result<(TenancyIncusState, String, Vec<TenancyFinding>), String> {
    let admin = state.admin.as_deref().ok_or(NO_ADMIN)?;
    let env = &chain.environment;
    let name = env
        .incus_project
        .as_deref()
        .ok_or("the environment names no Incus project")?;
    let s = settings(state);
    let incus = |e: AdapterError| format!("Incus: {e}");
    let existing = admin.tenancy_project(name).await.map_err(incus)?;
    let ours = |c: &BTreeMap<String, String>| {
        c.get(t::ENVIRONMENT_KEY)
            .is_some_and(|v| *v == env.id.to_string())
    };
    let (st, config) = match existing {
        Some(p) if ours(&p.config) => {
            ensure_profile(state, admin, name).await?;
            (TenancyIncusState::Created, p.config)
        }
        Some(p) => {
            if p.config.get("restricted").map(String::as_str) != Some("true") {
                return Err(format!(
                    "Incus project {name} exists but is not restricted; Apelora adopts only restricted projects (set restricted=true, or map the environment to another project)"
                ));
            }
            (TenancyIncusState::Adopted, p.config)
        }
        None => {
            let config = t::baseline_config(&env.id.to_string(), &s);
            let description = format!(
                "Apelora: {} / {} / {}",
                chain.organization_name, chain.project_name, env.name
            );
            let description: String = description
                .chars()
                .filter(|c| !c.is_control())
                .take(200)
                .collect();
            admin
                .create_tenancy_project(name, &description, &config)
                .await
                .map_err(incus)?;
            tracing::info!(environment = %env.id, project = name, "tenancy: Incus project created");
            ensure_profile(state, admin, name).await?;
            (TenancyIncusState::Created, config)
        }
    };
    let mut findings = t::findings(&config, &s);
    if let Some(f) = grant(state, admin, name).await? {
        findings.push(f);
    }
    // The deploy identity must now see the project, or nothing can be deployed into it.
    match state.provider.tenancy_project(name).await {
        Ok(Some(_)) => {}
        Ok(None) => {
            return Err(format!(
                "the deploy identity cannot find Incus project {name}"
            ));
        }
        Err(e) => {
            return Err(format!(
                "the deploy identity cannot use Incus project {name} yet: {e}"
            ));
        }
    }
    let detail = match st {
        TenancyIncusState::Created => {
            format!("Incus project {name} created by Apelora with the baseline restrictions")
        }
        _ => format!("Incus project {name} existed and is restricted; Apelora uses it as it is"),
    };
    Ok((st, detail, findings))
}

/// Gives a project Apelora created its default profile, unless it already has a root disk.
async fn ensure_profile(state: &AppState, admin: &Provider, name: &str) -> Result<(), String> {
    let incus = |e: AdapterError| format!("Incus: {e}");
    let current = admin
        .tenancy_profile(name, "default")
        .await
        .map_err(incus)?
        .ok_or_else(|| format!("Incus project {name} has no default profile"))?;
    let has_root = current.devices.values().any(|d| {
        d.get("type").is_some_and(|t| t == "disk") && d.get("path").is_some_and(|p| p == "/")
    });
    if has_root {
        return Ok(());
    }
    let base = admin
        .tenancy_profile("default", "default")
        .await
        .map_err(incus)?
        .unwrap_or_default();
    let root = base.devices.values().find(|d| {
        d.get("type").is_some_and(|t| t == "disk") && d.get("path").is_some_and(|p| p == "/")
    });
    let eth0 = base.devices.get("eth0");
    let spec = t::default_profile(
        root,
        eth0,
        state.volume_pool.as_deref(),
        state.network_uplink.is_some(),
    )?;
    admin
        .update_tenancy_profile(
            name,
            "default",
            &IncusProfile {
                description: "Default profile of an Apelora environment".into(),
                config: spec.config,
                devices: spec.devices,
            },
        )
        .await
        .map_err(|e| {
            format!(
                "Incus project {name} was created but its default profile could not be set: {e}"
            )
        })
}

/// Adds the project to the deploy identity's project list, under a lock shared by every control
/// plane (two changes at once could otherwise lose one), and reads the list back.
async fn grant(
    state: &AppState,
    admin: &Provider,
    name: &str,
) -> Result<Option<TenancyFinding>, String> {
    let Some(fp) = deploy_fingerprint(state) else {
        return Ok(Some(TenancyFinding {
            severity: TenancyFindingSeverity::Warning,
            key: None,
            message: "the deploy identity's certificate is not known to the control plane, so Apelora could not check that it may use this project".into(),
        }));
    };
    let pool = match &state.auth {
        super::Auth::Tokens(pool) => pool,
        super::Auth::DisabledForDevelopment => return Err("needs a database".into()),
    };
    let lock = db::lock_trust(pool)
        .await
        .map_err(|e| format!("cannot take the trust lock: {e}"))?;
    let result = async {
        let incus = |e: AdapterError| format!("Incus: {e}");
        let entry = admin
            .trust_entry(&fp)
            .await
            .map_err(incus)?
            .ok_or("the deploy identity is not in Incus's trust store")?;
        if !entry.restricted {
            return Ok(Some(TenancyFinding {
                severity: TenancyFindingSeverity::Warning,
                key: None,
                message: "the deploy identity is not restricted: it can use every Incus project (ADR 0003 wants it restricted)".into(),
            }));
        }
        if entry.projects.iter().any(|p| p == name) {
            return Ok(None);
        }
        let mut list = entry.projects.clone();
        list.push(name.to_owned());
        list.sort();
        list.dedup();
        admin.set_trust_projects(&fp, &list).await.map_err(incus)?;
        let after = admin
            .trust_entry(&fp)
            .await
            .map_err(incus)?
            .unwrap_or_default();
        if !after.projects.iter().any(|p| p == name) {
            return Err(format!(
                "Incus did not keep {name} in the deploy identity's project list"
            ));
        }
        tracing::info!(project = name, "tenancy: project granted to the deploy identity");
        Ok(None)
    }
    .await;
    if let Err(e) = db::unlock_trust(lock).await {
        tracing::warn!(error = %e, "tenancy: releasing the trust lock failed (the connection closes it)");
    }
    result
}

/// Prepares an environment's Incus project now (create or adopt, profile, grant, read back),
/// records the state with its audit row, then mirrors the quotas.
pub(super) async fn prepare(
    state: &AppState,
    p: &Principal,
    chain: &db::EnvironmentChain,
) -> Result<IncusProjectPrepared, ApiError> {
    let pool = store(state)?;
    let env = &chain.environment;
    let (st, detail, findings) = match attempt(state, chain).await {
        Ok(x) => x,
        Err(why) => {
            tracing::warn!(environment = %env.id, project = ?env.incus_project, reason = %why, "tenancy: Incus project not prepared");
            (TenancyIncusState::Pending, why, Vec::new())
        }
    };
    let target = env
        .incus_project
        .clone()
        .unwrap_or_else(|| env.slug.clone());
    let mut tx = begin(pool).await?;
    db::set_incus_state(&mut *tx, env.id, st, &detail).await?;
    audit_scoped(
        &mut tx,
        p,
        "environment.incus_project.prepare",
        &target,
        if st == TenancyIncusState::Pending {
            "failed"
        } else {
            "success"
        },
        Some(&env_resource(chain)),
        &json!({ "environment": env.id, "incusProject": env.incus_project, "state": st.as_str(), "detail": detail }),
    )
    .await?;
    commit(tx).await?;
    let limits = if st == TenancyIncusState::Pending {
        None
    } else {
        let fresh = db::environment_chain(pool, env.id).await?.ok_or_else(|| {
            ApiError::new(StatusCode::NOT_FOUND, "not_found", "not found: environment")
        })?;
        Some(mirror_limits(state, p, &fresh).await?)
    };
    Ok(IncusProjectPrepared {
        environment_id: env.id,
        incus_project: env.incus_project.clone(),
        state: st,
        detail,
        findings,
        limits,
    })
}

/// Writes the tightest applicable quota as the environment's Incus project limits.
pub(super) async fn mirror_limits(
    state: &AppState,
    p: &Principal,
    chain: &db::EnvironmentChain,
) -> Result<TenancyLimitOutcome, ApiError> {
    let pool = store(state)?;
    let env = &chain.environment;
    let desired = t::limit_config(&effective(&chain_quotas(pool, chain).await?));
    let outcome = |result, detail: String| TenancyLimitOutcome {
        environment_id: env.id,
        environment: env.name.clone(),
        incus_project: env.incus_project.clone(),
        result,
        limits: desired.clone(),
        detail,
    };
    let Some(name) = env.incus_project.as_deref() else {
        return Ok(outcome(
            TenancyLimitResult::Skipped,
            "the environment names no Incus project".into(),
        ));
    };
    if incus_state(chain) == TenancyIncusState::Pending {
        return Ok(outcome(
            TenancyLimitResult::Skipped,
            "the Incus project is not prepared yet; Apelora still checks the quota when deploying"
                .into(),
        ));
    }
    let Some(admin) = state.admin.as_deref() else {
        return Ok(outcome(TenancyLimitResult::Skipped, NO_ADMIN.into()));
    };
    let res = env_resource(chain);
    let current = match admin.tenancy_project(name).await {
        Ok(Some(c)) => c,
        Ok(None) => {
            return Ok(outcome(
                TenancyLimitResult::Failed,
                format!("Incus project {name} does not exist"),
            ));
        }
        Err(e) => return Ok(outcome(TenancyLimitResult::Failed, format!("Incus: {e}"))),
    };
    let next = t::with_limits(&current.config, &desired, &chain.incus_limits);
    let incus_changed = next != current.config;
    if !incus_changed && desired == chain.incus_limits {
        return Ok(outcome(
            TenancyLimitResult::Unchanged,
            "Incus already has these limits".into(),
        ));
    }
    if incus_changed
        && let Err(e) = admin
            .update_tenancy_project(name, &current.description, &next)
            .await
    {
        tracing::warn!(environment = %env.id, project = name, error = %e, "tenancy: Incus refused the limits");
        // Nothing changed in Apelora's database: the refusal is recorded on its own.
        audit_alone(
            pool,
            p,
            "environment.incus_limits.apply",
            name,
            "failed",
            Some(&res),
            &json!({ "environment": env.id, "limits": desired, "error": e.to_string() }),
        )
        .await?;
        return Ok(outcome(TenancyLimitResult::Failed, format!("Incus: {e}")));
    }
    let mut tx = begin(pool).await?;
    db::set_incus_limits(&mut *tx, env.id, &desired).await?;
    audit_scoped(
        &mut tx,
        p,
        "environment.incus_limits.apply",
        name,
        "success",
        Some(&res),
        &json!({ "environment": env.id, "limits": desired, "before": chain.incus_limits, "incusChanged": incus_changed }),
    )
    .await?;
    commit(tx).await?;
    Ok(outcome(
        if incus_changed {
            TenancyLimitResult::Applied
        } else {
            TenancyLimitResult::Unchanged
        },
        if desired.is_empty() {
            "no quota applies: Apelora's limits removed".into()
        } else {
            "Incus enforces these limits".into()
        },
    ))
}

// ---- API: scopes ------------------------------------------------------------------------------------

/// A quota scope resolved from the database.
struct ScopeRef {
    kind: QuotaScopeKind,
    id: Uuid,
    name: String,
    organization: Uuid,
    project: Option<Uuid>,
    resource: Resource,
    /// The scopes above it, nearest last.
    parents: Vec<(QuotaScopeKind, Uuid)>,
}

async fn resolve(
    pool: &apelora_persistence::Pool,
    kind: QuotaScopeKind,
    id: Uuid,
) -> Result<Option<ScopeRef>, ApiError> {
    Ok(match kind {
        QuotaScopeKind::Organization => idb::organization(pool, id).await?.map(|o| ScopeRef {
            kind,
            id,
            name: o.name,
            organization: id,
            project: None,
            resource: Resource::organization(id),
            parents: vec![],
        }),
        QuotaScopeKind::Project => idb::project(pool, id).await?.map(|p| ScopeRef {
            kind,
            id,
            name: p.name,
            organization: p.organization_id,
            project: Some(id),
            resource: Resource::project(p.organization_id, id),
            parents: vec![(QuotaScopeKind::Organization, p.organization_id)],
        }),
        QuotaScopeKind::Environment => db::environment_chain(pool, id).await?.map(|c| ScopeRef {
            kind,
            id,
            name: c.environment.name.clone(),
            organization: c.organization_id,
            project: Some(c.environment.project_id),
            resource: env_resource(&c),
            parents: vec![
                (QuotaScopeKind::Organization, c.organization_id),
                (QuotaScopeKind::Project, c.environment.project_id),
            ],
        }),
    })
}

fn view_permission(kind: QuotaScopeKind) -> &'static str {
    match kind {
        QuotaScopeKind::Organization => "organization.view",
        _ => "project.view",
    }
}

/// May the caller set this scope's quota? `Ok(true)` through `quota.manage` (no bound),
/// `Ok(false)` as a tenant dividing what it was given (bounded by the quotas above).
fn may_set(p: &Principal, s: &ScopeRef) -> Result<bool, ApiError> {
    if allow_platform(p, "quota.manage").is_ok() {
        return Ok(true);
    }
    let org = Resource::organization(s.organization);
    let ok = match s.kind {
        QuotaScopeKind::Organization => false,
        QuotaScopeKind::Project => allow(p, "organization.update", &org).is_ok(),
        QuotaScopeKind::Environment => {
            allow(p, "organization.update", &org).is_ok()
                || s.project.is_some_and(|proj| {
                    allow(
                        p,
                        "project.update",
                        &Resource::project(s.organization, proj),
                    )
                    .is_ok()
                })
        }
    };
    if ok {
        Ok(false)
    } else {
        allow_platform(p, "quota.manage").map(|()| true)
    }
}

fn parse_scope(scope: &str) -> Result<QuotaScopeKind, ApiError> {
    QuotaScopeKind::parse(scope).ok_or_else(|| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "the scope is organization, project or environment",
        )
    })
}

/// Resolves a scope the caller may see with `perm`; a missing one answers like a forbidden one
/// unless the caller could see everything.
async fn visible(
    pool: &apelora_persistence::Pool,
    p: &Principal,
    kind: QuotaScopeKind,
    id: Uuid,
) -> Result<ScopeRef, ApiError> {
    let perm = view_permission(kind);
    match resolve(pool, kind, id).await? {
        Some(s) => {
            allow(p, perm, &s.resource).or_else(|_| allow_platform(p, "quota.manage"))?;
            Ok(s)
        }
        None if allow_platform(p, perm).is_ok() || allow_platform(p, "quota.manage").is_ok() => {
            Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("not found: {}", kind.as_str()),
            ))
        }
        None => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            format!("you may not do this ({perm})"),
        )),
    }
}

/// The tightest quota of the scopes above `s`.
async fn bound(pool: &apelora_persistence::Pool, s: &ScopeRef) -> Result<QuotaLimits, ApiError> {
    let mut above = Vec::new();
    for (kind, id) in &s.parents {
        if let Some(q) = db::quota(pool, *kind, *id).await? {
            above.push(q.limits);
        }
    }
    Ok(QuotaLimits::tightest(above.iter()))
}

/// Use beneath a scope, summed from Incus. `Err` names the first project that could not be read.
async fn usage_under(
    state: &AppState,
    pool: &apelora_persistence::Pool,
    kind: QuotaScopeKind,
    id: Uuid,
) -> Result<Result<QuotaUsage, String>, ApiError> {
    let envs = db::environments_under(pool, kind, id).await?;
    let projects: BTreeSet<String> = envs
        .iter()
        .filter_map(|e| e.environment.incus_project.clone())
        .collect();
    let reads = futures_util::future::join_all(projects.iter().map(|p| read_use(state, p))).await;
    let mut total = QuotaUsage::default();
    for (project, r) in projects.iter().zip(reads) {
        match r {
            Ok(u) => total = total.plus(u.map(|u| u.usage()).unwrap_or_default()),
            Err(e) => return Ok(Err(format!("Incus project {project}: {e}"))),
        }
    }
    Ok(Ok(total))
}

/// A scope's quota, its use and the quotas above it.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuotaView {
    pub kind: QuotaScopeKind,
    pub id: Uuid,
    pub name: String,
    /// `None`: this scope has no quota of its own.
    pub quota: Option<QuotaLimits>,
    /// The tightest quota of the scopes above (a tenant may not set more).
    pub bound: QuotaLimits,
    /// Use beneath this scope as Incus reports it; `None` with `usageError` when it could not be read.
    pub usage: Option<QuotaUsage>,
    pub usage_error: Option<String>,
    pub updated_by: Option<String>,
    pub updated_at: Option<String>,
    /// The caller may change it.
    pub can_set: bool,
}

/// A scope's quota and use. Needs `organization.view` (organisation) or `project.view` (project,
/// environment) on it, or `quota.manage`.
#[utoipa::path(get, path = "/api/v1/identity/quotas/{scope}/{id}", tag = "identity", operation_id = "quota",
    params(("scope" = String, Path, description = "organization, project or environment"), ("id" = Uuid, Path, description = "Its id")),
    responses((status = 200, body = QuotaView), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub(super) async fn quota(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((scope, id)): Path<(String, Uuid)>,
) -> Result<Json<QuotaView>, ApiError> {
    let pool = store(&state)?;
    let kind = parse_scope(&scope)?;
    let s = visible(pool, &p, kind, id).await?;
    let row = db::quota(pool, kind, id).await?;
    let usage = usage_under(&state, pool, kind, id).await?;
    Ok(Json(QuotaView {
        kind,
        id: s.id,
        name: s.name.clone(),
        quota: row.as_ref().map(|r| r.limits),
        bound: bound(pool, &s).await?,
        usage: usage.as_ref().ok().copied(),
        usage_error: usage.err(),
        updated_by: row.as_ref().map(|r| r.updated_by.clone()),
        updated_at: row.as_ref().map(|r| rfc3339(r.updated_at)),
        can_set: may_set(&p, &s).is_ok(),
    }))
}

/// The result of changing a quota.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QuotaChange {
    pub quota: Option<QuotaLimits>,
    /// Each environment beneath the scope: what mirroring the change to Incus did.
    pub environments: Vec<TenancyLimitOutcome>,
}

/// Sets a scope's quota (all limits empty removes it), then mirrors the tightest applicable quota
/// onto every environment beneath it. Needs `quota.manage`; or, below the organisation,
/// `organization.update` on the organisation (projects, environments) or `project.update` on the
/// project (environments), and then never more than a quota above allows.
#[utoipa::path(put, path = "/api/v1/identity/quotas/{scope}/{id}", tag = "identity", operation_id = "set_quota",
    params(("scope" = String, Path, description = "organization, project or environment"), ("id" = Uuid, Path, description = "Its id")),
    request_body = QuotaLimits,
    responses((status = 200, body = QuotaChange), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub(super) async fn set_quota(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((scope, id)): Path<(String, Uuid)>,
    body: Result<Json<QuotaLimits>, JsonRejection>,
) -> Result<Json<QuotaChange>, ApiError> {
    let pool = store(&state)?;
    let kind = parse_scope(&scope)?;
    let s = visible(pool, &p, kind, id).await?;
    let unbounded = may_set(&p, &s)?;
    let Json(limits) =
        body.map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", e.body_text()))?;
    limits
        .validate()
        .map_err(|e| ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", e))?;
    if !unbounded && !limits.is_empty() {
        let above = limits.above(&bound(pool, &s).await?);
        if !above.is_empty() {
            let dims: Vec<&str> = above.iter().map(|d| d.label()).collect();
            return Err(ApiError::new(
                StatusCode::BAD_REQUEST,
                "quota_above_parent",
                format!(
                    "a quota may not allow more than the quota above it: {}",
                    dims.join(", ")
                ),
            ));
        }
    }
    let mut tx = begin(pool).await?;
    let before = db::set_quota_in(&mut tx, kind, id, &limits, &p.actor_ref()).await?;
    audit_scoped(
        &mut tx,
        &p,
        "quota.set",
        &format!("{}:{id}", kind.as_str()),
        "success",
        Some(&s.resource),
        &json!({ "scope": kind.as_str(), "id": id, "name": s.name, "before": before, "after": (!limits.is_empty()).then_some(limits) }),
    )
    .await?;
    commit(tx).await?;
    let mut environments = Vec::new();
    for chain in db::environments_under(pool, kind, id).await? {
        environments.push(mirror_limits(&state, &p, &chain).await?);
    }
    Ok(Json(QuotaChange {
        quota: (!limits.is_empty()).then_some(limits),
        environments,
    }))
}

/// Creates or adopts the environment's Incus project now (idempotent): the restricted project
/// with its default profile, granted to the deploy identity, then the quotas mirrored. Needs
/// `environment.manage` on the environment's project; 503 without an admin identity.
#[utoipa::path(post, path = "/api/v1/identity/environments/{id}/incus-project", tag = "identity", operation_id = "prepare_incus_project",
    params(("id" = Uuid, Path, description = "Environment id")),
    responses((status = 200, body = IncusProjectPrepared), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn prepare_project(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<Json<IncusProjectPrepared>, ApiError> {
    let pool = store(&state)?;
    let Some(chain) = db::environment_chain(pool, id).await? else {
        return Err(if allow_platform(&p, "environment.manage").is_ok() {
            ApiError::new(StatusCode::NOT_FOUND, "not_found", "not found: environment")
        } else {
            ApiError::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                "you may not do this (environment.manage)",
            )
        });
    };
    allow(&p, "environment.manage", &env_resource(&chain))?;
    if state.admin.is_none() {
        return Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "cluster_admin_not_configured",
            NO_ADMIN,
        ));
    }
    Ok(Json(prepare(&state, &p, &chain).await?))
}

// ---- API: the Tenancy page ---------------------------------------------------------------------------

/// What Incus reports for an environment's project.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TenancyIncusView {
    /// `restricted*` and `features.*` keys as set.
    pub restrictions: BTreeMap<String, String>,
    /// `limits.*` keys as set.
    pub limits: BTreeMap<String, String>,
    pub usage: TenancyProjectUse,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TenancyEnvironment {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub kind: EnvKind,
    pub incus_project: Option<String>,
    pub incus_state: TenancyIncusState,
    pub incus_detail: String,
    pub checked_at: Option<String>,
    /// This environment's own quota.
    pub quota: Option<QuotaLimits>,
    /// The tightest of its own, its project's and its organisation's quota: what Incus gets.
    pub effective: QuotaLimits,
    /// The limit keys Apelora last wrote to the Incus project.
    pub apelora_limits: BTreeMap<String, String>,
    /// As Incus reports it; `None` when the project does not exist or could not be read.
    pub incus: Option<TenancyIncusView>,
    pub incus_error: Option<String>,
    pub findings: Vec<TenancyFinding>,
    pub can_set_quota: bool,
    /// The caller may create or adopt the Incus project now.
    pub can_prepare: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TenancyProject {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub quota: Option<QuotaLimits>,
    /// Use of its environments; `None` when one could not be read.
    pub usage: Option<QuotaUsage>,
    pub can_set_quota: bool,
    pub environments: Vec<TenancyEnvironment>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TenancyOrganization {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub quota: Option<QuotaLimits>,
    /// Use of all its environments; only for callers who see the whole organisation.
    pub usage: Option<QuotaUsage>,
    pub can_set_quota: bool,
    pub projects: Vec<TenancyProject>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct TenancyOverview {
    pub source: Source,
    /// An admin identity is configured: Apelora can prepare Incus projects and set limits.
    pub admin_configured: bool,
    /// The configuration of a project Apelora would create now (restrictions and features).
    pub baseline: BTreeMap<String, String>,
    pub organizations: Vec<TenancyOrganization>,
}

/// Organisations, projects and environments the caller can see, with quotas, use and each
/// environment's Incus project as Incus reports it. Needs `organization.view` or `project.view`
/// somewhere (or `quota.manage`).
#[utoipa::path(get, path = "/api/v1/identity/tenancy", tag = "identity", operation_id = "tenancy",
    responses((status = 200, body = TenancyOverview), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody)))]
pub(super) async fn overview(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<TenancyOverview>, ApiError> {
    let pool = store(&state)?;
    let platform = allow_platform(&p, "quota.manage").is_ok();
    let f = if platform {
        apelora_auth::AccessFilter::All
    } else {
        filter_for(&p, "project.view")
    };
    let projects = idb::list_projects(pool, &f).await?;
    let orgs = idb::list_organizations(
        pool,
        &if platform {
            apelora_auth::AccessFilter::All
        } else {
            filter_for(&p, "organization.view")
        },
    )
    .await?;
    let envs =
        db::environments_of_projects(pool, &projects.iter().map(|x| x.id).collect::<Vec<_>>())
            .await?;
    let quotas: BTreeMap<(QuotaScopeKind, Uuid), QuotaLimits> = db::all_quotas(pool)
        .await?
        .into_iter()
        .map(|q| ((q.kind, q.scope_id), q.limits))
        .collect();
    // Read every visible environment's Incus project once, concurrently.
    let names: BTreeSet<String> = envs
        .iter()
        .filter_map(|e| e.environment.incus_project.clone())
        .collect();
    let reads = futures_util::future::join_all(names.iter().map(|n| {
        let state = &state;
        async move { (read_project(state, n).await, read_use(state, n).await) }
    }))
    .await;
    let incus: BTreeMap<&str, _> = names.iter().map(String::as_str).zip(reads).collect();
    let s = settings(&state);

    let mut organizations = Vec::new();
    // Organisations reached only through a project still appear, with their projects.
    let mut org_ids: Vec<Uuid> = orgs.iter().map(|o| o.id).collect();
    for x in &projects {
        if !org_ids.contains(&x.organization_id) {
            org_ids.push(x.organization_id);
        }
    }
    for org_id in org_ids {
        let Some(org) = idb::organization(pool, org_id).await? else {
            continue;
        };
        let org_quota = quotas.get(&(QuotaScopeKind::Organization, org.id)).copied();
        let mut tenancy_projects = Vec::new();
        let mut org_usage: Option<QuotaUsage> = Some(QuotaUsage::default());
        for proj in projects.iter().filter(|x| x.organization_id == org.id) {
            let proj_quota = quotas.get(&(QuotaScopeKind::Project, proj.id)).copied();
            let mut proj_usage: Option<QuotaUsage> = Some(QuotaUsage::default());
            let mut environments = Vec::new();
            for chain in envs.iter().filter(|e| e.environment.project_id == proj.id) {
                let env = &chain.environment;
                let env_quota = quotas.get(&(QuotaScopeKind::Environment, env.id)).copied();
                let eff = effective(&[org_quota, proj_quota, env_quota]);
                let (view, error, findings) =
                    match env.incus_project.as_deref().and_then(|n| incus.get(n)) {
                        None => (None, None, Vec::new()),
                        Some((Ok(Some(project)), Ok(usage))) => (
                            Some(TenancyIncusView {
                                restrictions: t::restrictions(&project.config),
                                limits: project
                                    .config
                                    .iter()
                                    .filter(|(k, _)| k.starts_with("limits."))
                                    .map(|(k, v)| (k.clone(), v.clone()))
                                    .collect(),
                                usage: usage.unwrap_or_default(),
                            }),
                            None,
                            t::findings(&project.config, &s),
                        ),
                        Some((Ok(None), _)) => (
                            None,
                            Some("the Incus project does not exist yet".to_owned()),
                            Vec::new(),
                        ),
                        Some((Err(e), _)) | Some((_, Err(e))) => {
                            (None, Some(e.to_string()), Vec::new())
                        }
                    };
                let used = match (&view, &error) {
                    (Some(v), _) => Some(v.usage.usage()),
                    // A project that does not exist uses nothing.
                    (None, Some(e)) if e.contains("does not exist") => Some(QuotaUsage::default()),
                    (None, None) => Some(QuotaUsage::default()),
                    _ => None,
                };
                proj_usage = proj_usage.zip(used).map(|(a, b)| a.plus(b));
                let res = env_resource(chain);
                let chain_scope = ScopeRef {
                    kind: QuotaScopeKind::Environment,
                    id: env.id,
                    name: env.name.clone(),
                    organization: org.id,
                    project: Some(proj.id),
                    resource: res.clone(),
                    parents: vec![],
                };
                environments.push(TenancyEnvironment {
                    id: env.id,
                    slug: env.slug.clone(),
                    name: env.name.clone(),
                    kind: EnvKind::parse(&env.kind).unwrap_or(EnvKind::Other),
                    incus_project: env.incus_project.clone(),
                    incus_state: incus_state(chain),
                    incus_detail: detail_of(&state, env),
                    checked_at: chain.incus_checked_at.map(rfc3339),
                    quota: env_quota,
                    effective: eff,
                    apelora_limits: chain.incus_limits.clone(),
                    incus: view,
                    incus_error: error,
                    findings,
                    can_set_quota: may_set(&p, &chain_scope).is_ok(),
                    can_prepare: state.admin.is_some()
                        && allow(&p, "environment.manage", &res).is_ok(),
                });
            }
            org_usage = org_usage.zip(proj_usage).map(|(a, b)| a.plus(b));
            let proj_scope = ScopeRef {
                kind: QuotaScopeKind::Project,
                id: proj.id,
                name: proj.name.clone(),
                organization: org.id,
                project: Some(proj.id),
                resource: Resource::project(org.id, proj.id),
                parents: vec![],
            };
            tenancy_projects.push(TenancyProject {
                id: proj.id,
                slug: proj.slug.clone(),
                name: proj.name.clone(),
                quota: proj_quota,
                usage: proj_usage,
                can_set_quota: may_set(&p, &proj_scope).is_ok(),
                environments,
            });
        }
        // The organisation's use is shown only to those who see all of it.
        let whole =
            platform || allow(&p, "organization.view", &Resource::organization(org.id)).is_ok();
        let org_scope = ScopeRef {
            kind: QuotaScopeKind::Organization,
            id: org.id,
            name: org.name.clone(),
            organization: org.id,
            project: None,
            resource: Resource::organization(org.id),
            parents: vec![],
        };
        organizations.push(TenancyOrganization {
            id: org.id,
            slug: org.slug,
            name: org.name,
            quota: org_quota,
            usage: org_usage.filter(|_| whole),
            can_set_quota: may_set(&p, &org_scope).is_ok(),
            projects: tenancy_projects,
        });
    }
    Ok(Json(TenancyOverview {
        source: source(state.provider.mode()),
        admin_configured: state.admin.is_some(),
        baseline: t::restrictions(&t::baseline_config("", &s)),
        organizations,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fingerprint_is_the_sha256_of_the_certificate() {
        let key = rcgen::KeyPair::generate().expect("key");
        let cert = rcgen::CertificateParams::new(vec!["apelora".into()])
            .expect("params")
            .self_signed(&key)
            .expect("cert");
        let fp = certificate_fingerprint(cert.pem().as_bytes()).expect("fingerprint");
        use sha2::{Digest, Sha256};
        let want: String = Sha256::digest(cert.der().as_ref())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        assert_eq!(fp, want);
        assert_eq!(fp.len(), 64);
        assert!(certificate_fingerprint(b"not a certificate").is_err());
    }
}
