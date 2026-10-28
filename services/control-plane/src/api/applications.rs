// SPDX-License-Identifier: AGPL-3.0-only
//! Applications deployed from the Studio (docs/design/application-deployment.md).
//!
//! The Studio sends its document and one `InstanceSpec` per deployable module. The plan is
//! computed here from current inventory, for the preview and again when applying (never taken
//! from the caller). Applying saves an immutable revision and queues ordinary instance jobs.

use apelora_domain::InstanceSpec;
use apelora_domain::network::{
    Connection, Internet, Published, SyncRequest, discovery_env, with_discovery,
};
use apelora_domain::plan::{
    DesiredModule, Plan, PlanAction, deploy_spec, owned_spec, plan, update_name,
};
use apelora_domain::{Permission, is_valid_name};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::authz::{allow, allow_any};
use super::deploy::{bad_request, ensure_project_allowed, idempotency_key, store};
use super::{ApiError, AppState, Auth, ErrorBody, Principal, Source, require, source};
use apelora_auth::{EnvKind, Resource};
use apelora_persistence::identity::AppContext;

/// Application names become instance-name prefixes, so they are short.
const MAX_NAME: usize = 40;
const MAX_MODULES: usize = 200;

#[derive(Deserialize, ToSchema)]
pub struct PlanRequest {
    pub project: String,
    #[serde(alias = "appliances")]
    pub modules: Vec<DesiredModule>,
    /// The design's connections; enforced when the control plane has a network uplink.
    #[serde(default)]
    pub connections: Option<Vec<Connection>>,
    /// Ports reachable from outside the application (needs connections to be enforced).
    #[serde(default)]
    pub published: Vec<Published>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeployRequest {
    pub project: String,
    /// The Studio document, saved as an immutable revision. Ignored when `version` is given.
    #[schema(value_type = Object)]
    pub document: Value,
    /// Deploy this published version (its stored document) instead of saving `document` as a
    /// new revision. Drafts can never be deployed.
    #[serde(default)]
    pub version: Option<i32>,
    #[serde(alias = "appliances")]
    pub modules: Vec<DesiredModule>,
    /// The design's connections; enforced when the control plane has a network uplink.
    #[serde(default)]
    pub connections: Option<Vec<Connection>>,
    /// Ports reachable from outside the application (needs connections to be enforced).
    #[serde(default)]
    pub published: Vec<Published>,
    /// Also delete the application's instances that are no longer in the design.
    #[serde(default)]
    pub remove_missing: bool,
    /// Required when the plan rebuilds a singleton or stateful module (`rebuild`): it
    /// acknowledges that the module is stopped while it is rebuilt. Data on volumes is kept.
    #[serde(default)]
    pub confirm_disruption: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueuedChange {
    pub module: String,
    pub instance: String,
    pub action: PlanAction,
    pub job_id: Uuid,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeployResponse {
    pub application: String,
    pub revision: i32,
    /// The plan that was applied (recomputed when applying).
    pub plan: Plan,
    pub jobs: Vec<QueuedChange>,
    /// The job that applies the connections to the application's network, when enforced.
    pub network_job: Option<Uuid>,
    /// The rolling update that replaces the plan's `replace` instances one replica at a time,
    /// each new one healthy before the old one goes (an application operation of kind
    /// `update`); absent when nothing is replaced.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation: Option<Uuid>,
}

#[derive(Serialize, ToSchema)]
pub struct ModuleStatus {
    pub module: String,
    pub instance: String,
    pub project: String,
    /// Incus status, e.g. `Running`.
    pub status: String,
    pub location: String,
    /// What the worker decided from the instance's port check, with hysteresis; absent for
    /// instances that are not checked (no service address, not running, or no database).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub health: Option<InstanceHealth>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstanceHealth {
    pub health: apelora_domain::health::HealthState,
    /// RFC 3339: since when it has had this health.
    pub since: String,
    /// What the decision rests on, e.g. `port check failing`.
    pub detail: String,
    /// RFC 3339: when the worker last restarted it for failing its check.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restarted_at: Option<String>,
}

/// A stateless module's service address: a load balancer over its healthy replicas.
#[derive(Serialize, ToSchema)]
pub struct ServiceStatus {
    pub module: String,
    pub project: String,
    pub address: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationStatus {
    pub source: Source,
    pub items: Vec<ModuleStatus>,
    /// Allocated service addresses (empty without a database or service range).
    pub services: Vec<ServiceStatus>,
    /// Present while the application's service addresses cannot move traffic to replacement
    /// replicas because Incus refuses load balancer changes (a cluster member is unreachable):
    /// what happens, which member, and what to do.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub traffic_blocked: Option<String>,
}

#[derive(Deserialize, IntoParams)]
pub struct StatusQuery {
    /// Only this Incus project.
    pub project: Option<String>,
}

/// The spec a replica is created with; with enforced connections its NIC joins the
/// application's network with its own ACL.
fn spec_for(
    state: &AppState,
    name: &str,
    project: &str,
    d: &DesiredModule,
    replica: u8,
    enforce: bool,
) -> InstanceSpec {
    let uplink = state.network_uplink.as_deref().filter(|_| enforce);
    deploy_spec(name, project, d, replica, uplink)
}

/// Allocates (or finds) the service address of every module that gets one — stateless ones
/// that are connected to, and published ones — when connections are enforced and a service
/// range is configured. Returns module → address.
async fn allocate_services(
    state: &AppState,
    tx: &mut apelora_persistence::PgConnection,
    application_id: Uuid,
    name: &str,
    req: &DeployRequest,
) -> Result<std::collections::BTreeMap<String, String>, ApiError> {
    let (Some(uplink), Some(range), Some(connections)) = (
        state.network_uplink.as_deref(),
        state.service_range.as_deref(),
        req.connections.as_deref(),
    ) else {
        return Ok(Default::default());
    };
    let sync = SyncRequest::for_deployment(
        name,
        &req.project,
        uplink,
        &req.modules,
        connections,
        Some(range),
    )
    .with_published(&req.published);
    let candidates: Vec<String> = apelora_domain::network::service_addresses(range)
        .map_err(bad_request)?
        .iter()
        .map(ToString::to_string)
        .collect();
    let mut out = std::collections::BTreeMap::new();
    for a in sync.services() {
        match apelora_persistence::service_address_in(
            &mut *tx,
            application_id,
            &req.project,
            &a,
            &candidates,
        )
        .await?
        {
            Some(address) => {
                out.insert(a, address);
            }
            None => {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "service_range_exhausted",
                    format!("no free service address for {a} in {range}"),
                ));
            }
        }
    }
    Ok(out)
}

fn check_connections(
    modules: &[DesiredModule],
    connections: Option<&[Connection]>,
) -> Result<(), ApiError> {
    for c in connections.unwrap_or_default() {
        c.validate().map_err(bad_request)?;
        for end in [&c.from, &c.to] {
            if !modules.iter().any(|d| &d.module == end) {
                return Err(bad_request(format!(
                    "connection {} → {}: {end} is not a deployed module",
                    c.from, c.to
                )));
            }
        }
    }
    Ok(())
}

fn check_published(modules: &[DesiredModule], published: &[Published]) -> Result<(), ApiError> {
    for p in published {
        p.validate().map_err(bad_request)?;
        if !modules.iter().any(|d| d.module == p.module) {
            return Err(bad_request(format!(
                "published {}: not a deployed module",
                p.module
            )));
        }
    }
    Ok(())
}

/// Adds what the designer must know about connections to the plan's warnings.
fn network_warnings(
    state: &AppState,
    plan: &mut Plan,
    desired: &[DesiredModule],
    connections: Option<&[Connection]>,
    published: &[Published],
) {
    if !published.is_empty() {
        let names: Vec<&str> = published.iter().map(|p| p.module.as_str()).collect();
        plan.warnings.push(match (&state.network_uplink, &state.service_range, connections) {
            (Some(_), Some(_), Some(_)) => format!(
                "published to outside clients: {}; the network outside must route the service addresses to the application's network",
                names.join(", ")
            ),
            _ => format!(
                "not published: {} (publishing needs enforced connections and APELORA_SERVICE_ADDRESSES)",
                names.join(", ")
            ),
        });
    }
    if state.network_uplink.is_none() || connections.is_none() {
        let mut open: Vec<&str> = desired
            .iter()
            .filter(|d| d.internet != Internet::None)
            .map(|d| d.module.as_str())
            .collect();
        open.sort_unstable();
        if !open.is_empty() {
            plan.warnings.push(format!(
                "internet access is not enforced for {}: without enforced connections instances reach whatever the project's network reaches",
                open.join(", ")
            ));
        }
    }
    match (connections, &state.network_uplink) {
        (Some(c), None) if !c.is_empty() => plan.warnings.push(
            "connections are not enforced: the control plane has no network uplink (APELORA_NETWORK_UPLINK); instances share the project's network".into(),
        ),
        (Some(_), Some(_)) if plan.count(PlanAction::Keep) > 0 => plan.warnings.push(
            "kept instances stay on the network they were created on; connections apply to new instances".into(),
        ),
        _ => {}
    }
}

fn check(name: &str, project: &str, modules: &[DesiredModule]) -> Result<(), ApiError> {
    if !is_valid_name(name) || name.len() > MAX_NAME {
        return Err(bad_request(format!(
            "invalid application name {name:?} (1–{MAX_NAME} letters, digits and hyphens, starting with a letter)"
        )));
    }
    if !is_valid_name(project) {
        return Err(bad_request(format!("invalid project name {project:?}")));
    }
    if modules.len() > MAX_MODULES {
        return Err(bad_request(format!(
            "at most {MAX_MODULES} modules per deployment"
        )));
    }
    let mut seen = std::collections::HashSet::new();
    for d in modules {
        if !seen.insert(d.module.as_str()) {
            return Err(bad_request(format!("module {} is listed twice", d.module)));
        }
        d.validate().map_err(bad_request)?;
        // The spec Apelora will create (its name, project and owner), checked with the caller's
        // own `owner` put back: what only Apelora sets is refused as the caller sent it.
        InstanceSpec {
            owner: d.spec.owner.clone(),
            ..owned_spec(name, project, d, d.replicas)
        }
        .validate_from_caller()
        .map_err(|e| bad_request(format!("module {}: {e}", d.module)))?;
    }
    Ok(())
}

/// Puts the pool behind each volume's class into the spec, so the worker (and the replica
/// supervisor, which reads the saved revision) creates volumes where the administrator said.
/// Refuses when no pool is configured for the class or Incus does not have it: a design never
/// silently falls back to another pool.
async fn resolve_volume_pools(
    state: &AppState,
    modules: &mut [DesiredModule],
) -> Result<(), ApiError> {
    use apelora_domain::storage::{VolumeClass, is_shared_driver, restarts_elsewhere};
    if modules.iter().all(|d| d.spec.volumes.is_empty()) {
        return Ok(());
    }
    let pools = state.provider.storage_pools().await?;
    let wants = |class: VolumeClass| {
        modules
            .iter()
            .flat_map(|d| d.spec.volumes.iter())
            .any(|v| v.class == class)
    };
    let local = if wants(VolumeClass::DataLocal) {
        let Some(pool) = state.volume_pool.as_deref() else {
            return Err(bad_request(
                "this design has volumes, but no storage pool is configured for volume class data-local (APELORA_VOLUME_POOL)",
            ));
        };
        if !pools.iter().any(|p| p.name == pool) {
            return Err(bad_request(format!(
                "volume class data-local: storage pool {pool} does not exist in Incus"
            )));
        }
        Some(pool)
    } else {
        None
    };
    let shared = if wants(VolumeClass::DataReplicated) {
        let Some(pool) = state.shared_pool.as_deref() else {
            return Err(bad_request(
                "this design has data-replicated volumes, but no shared storage pool is configured (APELORA_SHARED_POOL)",
            ));
        };
        match pools.iter().find(|p| p.name == pool) {
            None => {
                return Err(bad_request(format!(
                    "volume class data-replicated: storage pool {pool} does not exist in Incus"
                )));
            }
            Some(p) if !is_shared_driver(&p.driver) => {
                return Err(bad_request(format!(
                    "volume class data-replicated: storage pool {pool} uses driver {}, which is not shared by all servers",
                    p.driver
                )));
            }
            Some(_) => Some(pool),
        }
    } else {
        None
    };
    for d in modules.iter_mut() {
        for v in d.spec.volumes.iter_mut() {
            v.pool = match v.class {
                VolumeClass::DataLocal => local,
                VolumeClass::DataReplicated => shared,
            }
            .map(str::to_owned);
        }
        // Everything on the shared pool: the root disk goes there too, so the module can
        // restart on another server after a failure (ADR 0013).
        if restarts_elsewhere(&d.spec.volumes) {
            d.spec.root_pool = shared.map(str::to_owned);
        }
    }
    Ok(())
}

async fn current_plan(
    state: &AppState,
    name: &str,
    project: &str,
    modules: &[DesiredModule],
) -> Result<(Plan, Vec<apelora_domain::Instance>), ApiError> {
    let observed = state.provider.instances().await?;
    let p = plan(name, project, modules, &observed);
    for i in &p.items {
        tracing::debug!(application = name, instance = %format!("{project}/{}", i.instance), module = %i.module, replica = i.replica, action = ?i.action, reason = %i.reason, "plan: decision");
    }
    Ok((p, observed))
}

/// Where an application operation lands, resolved from the database (never from client claims
/// alone): the application's own project and environment, or, for a new application, the
/// environment that owns the requested Incus project.
struct Target {
    resource: Resource,
    existing: Option<AppContext>,
    project_id: Option<Uuid>,
    environment_id: Option<Uuid>,
}

async fn resolve_target(
    state: &AppState,
    p: &Principal,
    name: &str,
    incus_project: &str,
) -> Result<Target, ApiError> {
    let Auth::Tokens(pool) = &state.auth else {
        return Ok(Target {
            resource: Resource::platform(),
            existing: None,
            project_id: None,
            environment_id: None,
        });
    };
    let existing = apelora_persistence::identity::application_context(pool, name).await?;
    if let Some(ctx) = existing {
        // People and services deploy an application only into its own environment.
        if p.caller.is_some()
            && ctx
                .incus_project
                .as_deref()
                .is_some_and(|ip| ip != incus_project)
        {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "project_mismatch",
                format!(
                    "{name} belongs to a different environment than Incus project {incus_project}"
                ),
            ));
        }
        return Ok(Target {
            resource: ctx.resource(),
            project_id: Some(ctx.project_id),
            environment_id: ctx.environment_id,
            existing: Some(ctx),
        });
    }
    Ok(
        match apelora_persistence::identity::environment_by_incus_project(pool, incus_project)
            .await?
        {
            Some((env, org)) => Target {
                resource: Resource {
                    organization: Some(org),
                    project: Some(env.project_id),
                    environment: EnvKind::parse(&env.kind),
                    ..Resource::default()
                },
                existing: None,
                project_id: Some(env.project_id),
                environment_id: Some(env.id),
            },
            // An Incus project no environment owns is infrastructure: only platform-wide grants reach it.
            None => Target {
                resource: Resource::platform(),
                existing: None,
                project_id: None,
                environment_id: None,
            },
        },
    )
}

/// What deploying the design would change, without changing anything. Needs `deploy`.
#[utoipa::path(post, path = "/api/v1/applications/{name}/plan", tag = "applications", operation_id = "plan_application",
    params(("name" = String, Path, description = "Application name")), request_body = PlanRequest,
    responses((status = 200, body = Plan), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn preview(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
    body: Result<Json<PlanRequest>, JsonRejection>,
) -> Result<Json<Plan>, ApiError> {
    let Json(mut req) = body.map_err(|e| bad_request(e.body_text()))?;
    check(&name, &req.project, &req.modules)?;
    let target = resolve_target(&state, &p, &name, &req.project).await?;
    if p.caller.is_some() {
        allow_any(
            &p,
            &["application.design", "application.deploy"],
            &target.resource,
        )?;
    } else {
        require(&p, Permission::Deploy)?;
    }
    ensure_project_allowed(&state, &req.project).await?;
    check_connections(&req.modules, req.connections.as_deref())?;
    check_published(&req.modules, &req.published)?;
    resolve_volume_pools(&state, &mut req.modules).await?;
    let (mut p, _) = current_plan(&state, &name, &req.project, &req.modules).await?;
    network_warnings(
        &state,
        &mut p,
        &req.modules,
        req.connections.as_deref(),
        &req.published,
    );
    if let Some(env) = target.environment_id {
        p.quota =
            super::tenancy::check_deployment(&state, env, &creating(&p, &req.modules)).await?;
        if let Some(q) = p.quota.as_ref().and_then(|q| q.refusal()) {
            p.warnings
                .push(format!("over quota, applying would be refused: {q}"));
        }
    }
    Ok(Json(p))
}

/// What the plan adds, per instance: a created instance's config and volume sizes, and the
/// increase of a resized one's CPU and memory (quota accounting).
fn creating<'a>(plan: &Plan, modules: &'a [DesiredModule]) -> Vec<super::tenancy::Adding<'a>> {
    plan.items
        .iter()
        .filter(|i| matches!(i.action, PlanAction::Create | PlanAction::Resize))
        .filter_map(|i| Some((i, modules.iter().find(|d| d.module == i.module)?)))
        .map(|(i, d)| super::tenancy::Adding {
            config: &d.spec.config,
            volume_mib: d.spec.volumes.iter().map(|v| i64::from(v.size_mib)).sum(),
            resize: (i.action == PlanAction::Resize)
                .then(|| apelora_domain::tenancy::resize_usage(&i.changes)),
        })
        .collect()
}

/// Deploys the design: saves it as a new revision and queues the planned changes. Refuses
/// the whole deployment if any module conflicts with an existing instance. Needs `deploy`.
#[utoipa::path(post, path = "/api/v1/applications/{name}/deployments", tag = "applications", operation_id = "deploy_application",
    params(("name" = String, Path, description = "Application name"),
        ("Idempotency-Key" = Option<String>, Header, description = "Makes the request safe to repeat")),
    request_body = DeployRequest,
    responses((status = 202, body = DeployResponse), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 409, body = ErrorBody), (status = 502, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn deploy(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Result<Json<DeployRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<DeployResponse>), ApiError> {
    let Json(mut req) = body.map_err(|e| bad_request(e.body_text()))?;
    if !req.document.is_object() {
        return Err(bad_request(
            "the document must be a Studio application document",
        ));
    }
    check(&name, &req.project, &req.modules)?;
    let target = resolve_target(&state, &p, &name, &req.project).await?;
    if p.caller.is_some() {
        if target.existing.is_none() {
            allow(&p, "application.create", &target.resource)?;
        }
        allow(&p, "application.deploy", &target.resource)?;
    } else {
        require(&p, Permission::Deploy)?;
    }
    ensure_project_allowed(&state, &req.project).await?;
    check_connections(&req.modules, req.connections.as_deref())?;
    check_published(&req.modules, &req.published)?;
    resolve_volume_pools(&state, &mut req.modules).await?;
    let pool = store(&state)?;
    let key = idempotency_key(&headers, &p)?;
    // Removing every module also removes the application's network objects, even when the
    // request leaves `connections` out (there is nothing left to connect).
    let cleanup = req.modules.is_empty()
        && req.connections.is_none()
        && state.network_uplink.is_some()
        && apelora_persistence::deployment_enforced(pool, &name, &req.project).await?;
    let enforce = (req.connections.is_some() && state.network_uplink.is_some()) || cleanup;
    // A rolling update holds two instances in a replica's slot and works from the deployment
    // record it started with: nothing else is deployed until it ends.
    if let Some(ctx) = &target.existing
        && apelora_persistence::lifecycle::active_operation_kind(pool, ctx.application_id)
            .await?
            .as_deref()
            == Some("update")
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "operation_in_progress",
            "a rolling update of this application is still in progress; nothing was deployed. Deploy again once it has finished or stopped",
        ));
    }
    let (mut applied, observed) = current_plan(&state, &name, &req.project, &req.modules).await?;
    network_warnings(
        &state,
        &mut applied,
        &req.modules,
        req.connections.as_deref(),
        &req.published,
    );
    if applied.has_conflicts() {
        let conflicts: Vec<String> = applied
            .items
            .iter()
            .filter(|i| i.action == PlanAction::Conflict)
            .map(|i| format!("{}: {}", i.module, i.reason))
            .collect();
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "plan_conflict",
            format!("nothing was deployed; {}", conflicts.join("; ")),
        ));
    }
    // A rebuilt singleton or stateful module is down while it is rebuilt: the caller says so.
    let rebuilt: Vec<&str> = applied
        .items
        .iter()
        .filter(|i| i.action == PlanAction::Rebuild)
        .map(|i| i.instance.as_str())
        .collect();
    if !rebuilt.is_empty() && !req.confirm_disruption {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "confirmation_required",
            format!(
                "nothing was deployed: {} {} stopped while rebuilt with the design's image and settings, so {} clients lose service meanwhile. Data on volumes is kept. Deploy again with confirmDisruption",
                rebuilt.join(", "),
                if rebuilt.len() == 1 { "is" } else { "are each" },
                if rebuilt.len() == 1 { "its" } else { "their" },
            ),
        ));
    }
    // The environment's Incus project is prepared on first use, and the quotas above it are
    // checked before anything is saved or queued (ADR 0012).
    if let Some(env) = target.environment_id {
        super::tenancy::ensure_ready(&state, &p, env).await?;
        applied.quota =
            super::tenancy::check_deployment(&state, env, &creating(&applied, &req.modules))
                .await?;
        if let Some(q) = applied.quota.as_ref().filter(|q| !q.allowed) {
            return Err(super::tenancy::refuse(q));
        }
    }

    // A pinned version must be a published one of an application that already exists: a draft
    // can never be deployed, and deploying does not create revisions of its own.
    if let Some(v) = req.version {
        let Some(ctx) = &target.existing else {
            return Err(ApiError::new(
                StatusCode::NOT_FOUND,
                "not_found",
                format!("not found: application {name}; create it and publish a version first"),
            ));
        };
        match apelora_persistence::lifecycle::version_document(pool, ctx.application_id, v).await? {
            None => {
                return Err(ApiError::new(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    format!("not found: version {v}"),
                ));
            }
            Some((_, false)) => {
                return Err(ApiError::new(
                    StatusCode::CONFLICT,
                    "not_published",
                    format!(
                        "version {v} is not published; only published versions can be deployed"
                    ),
                ));
            }
            Some(_) => {}
        }
    }
    let mut tx = super::authz::begin(pool).await?;
    let app = match target.project_id {
        Some(project) => {
            apelora_persistence::identity::ensure_application_in_tx(
                &mut tx,
                &name,
                project,
                target.environment_id,
            )
            .await?
        }
        None => apelora_persistence::ensure_application_tx(&mut tx, &name).await?,
    };
    // A repeated request (same key) finds its revision and its jobs: nothing happens twice.
    let (revision_no, new_revision) = match req.version {
        Some(v) => (v, true),
        None => {
            let (r, new) = apelora_persistence::add_revision_once_in(
                &mut tx,
                app.id,
                &req.document,
                &p.actor_ref(),
                key.as_ref().map(|k| format!("{k}:{name}")).as_deref(),
            )
            .await?;
            (r.revision, new)
        }
    };
    // Service addresses are allocated before any instance is created, so consumers start with
    // their discovery environment (ADR 0004).
    let addresses = allocate_services(&state, &mut tx, app.id, &name, &req).await?;
    let mut jobs = Vec::new();
    for item in &applied.items {
        let (kind, payload) = match item.action {
            PlanAction::Create => {
                let desired = req
                    .modules
                    .iter()
                    .find(|d| d.module == item.module)
                    .ok_or_else(|| bad_request(format!("module {} disappeared", item.module)))?;
                let env = discovery_env(
                    &name,
                    &desired.module,
                    req.connections.as_deref().unwrap_or_default(),
                    &addresses,
                );
                let spec = with_discovery(
                    spec_for(&state, &name, &req.project, desired, item.replica, enforce),
                    &env,
                );
                (
                    "instance.create",
                    serde_json::to_value(&spec).map_err(|e| bad_request(e.to_string()))?,
                )
            }
            PlanAction::Resize => (
                "instance.resize",
                json!({
                    "project": req.project,
                    "name": item.instance,
                    "limits": item.changes.iter()
                        .map(|c| (c.key.clone(), c.to.clone()))
                        .collect::<std::collections::BTreeMap<_, _>>(),
                }),
            ),
            PlanAction::Remove if req.remove_missing => (
                "instance.delete",
                json!({ "project": req.project, "name": item.instance }),
            ),
            _ => continue,
        };
        let job_key = key
            .as_ref()
            .map(|k| format!("{k}:{}:{kind}", item.instance));
        let job_id = apelora_persistence::enqueue_job_from(
            &mut tx,
            kind,
            Some(app.id),
            &payload,
            job_key.as_deref(),
            &apelora_persistence::JobOrigin {
                request_id: super::requests::current().as_deref(),
                queued_by: Some(&p.actor_ref()),
                retry_of: None,
            },
        )
        .await?
        .id();
        jobs.push(QueuedChange {
            module: item.module.clone(),
            instance: item.instance.clone(),
            action: item.action,
            job_id,
        });
    }
    // Outdated stateless replicas are replaced one at a time by a rolling update: for each, its
    // update (`<replica>-u<n>`) is created beside it, must be healthy, then the old one leaves
    // its service address and is deleted (docs/design/rolling-updates.md).
    let mut phases = Vec::new();
    for item in applied
        .items
        .iter()
        .filter(|i| matches!(i.action, PlanAction::Replace | PlanAction::Rebuild))
    {
        let desired = req
            .modules
            .iter()
            .find(|d| d.module == item.module)
            .ok_or_else(|| bad_request(format!("module {} disappeared", item.module)))?;
        // A singleton or stateful module keeps its instance: rebuilt in place, in this order.
        if item.action == PlanAction::Rebuild {
            let env = discovery_env(
                &name,
                &desired.module,
                req.connections.as_deref().unwrap_or_default(),
                &addresses,
            );
            let spec = with_discovery(
                spec_for(&state, &name, &req.project, desired, item.replica, enforce),
                &env,
            );
            let spec = serde_json::to_value(&spec).map_err(|e| bad_request(e.to_string()))?;
            phases.push(json!({ "action": "rebuild", "state": "pending", "steps": [
                { "project": req.project, "name": item.instance, "module": item.module, "state": "pending", "payload": spec }
            ] }));
            continue;
        }
        let Some(new) = (1..=9)
            .map(|n| update_name(&name, &item.module, item.replica, n))
            .find(|n| {
                !observed
                    .iter()
                    .any(|i| i.project == req.project && i.name == *n)
            })
        else {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "plan_conflict",
                format!(
                    "nothing was deployed; {}: no name is left for its update",
                    item.instance
                ),
            ));
        };
        if !is_valid_name(&new) {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "plan_conflict",
                format!(
                    "nothing was deployed; {}: the name of its update, {new:?}, is too long (at most 63 letters, digits and hyphens)",
                    item.instance
                ),
            ));
        }
        let env = discovery_env(
            &name,
            &desired.module,
            req.connections.as_deref().unwrap_or_default(),
            &addresses,
        );
        let mut spec = with_discovery(
            spec_for(&state, &name, &req.project, desired, item.replica, enforce),
            &env,
        );
        spec.name = new.clone();
        let spec = serde_json::to_value(&spec).map_err(|e| bad_request(e.to_string()))?;
        let step = |instance: &str, payload: Option<&Value>| json!({ "project": req.project, "name": instance, "module": item.module, "state": "pending", "payload": payload });
        for (action, instance, payload) in [
            ("create", new.as_str(), Some(&spec)),
            ("healthy", new.as_str(), None),
            ("drain", item.instance.as_str(), None),
            ("delete", item.instance.as_str(), None),
        ] {
            phases.push(
                json!({ "action": action, "state": "pending", "steps": [step(instance, payload)] }),
            );
        }
    }
    let operation = if phases.is_empty() {
        None
    } else {
        let request = super::requests::current();
        let (op, _) = apelora_persistence::lifecycle::create_operation_in(
            &mut tx,
            &apelora_persistence::lifecycle::NewOperation {
                application_id: app.id,
                project: &req.project,
                kind: "update",
                phases: &json!(phases),
                requested_by: &p.actor_ref(),
                confirmed: req.confirm_disruption,
                key: key
                    .as_ref()
                    .map(|k| format!("{k}:{name}:update"))
                    .as_deref(),
                request_id: request.as_deref(),
            },
        )
        .await
        .map_err(|e| match e {
            apelora_persistence::StoreError::Conflict(m) => ApiError::new(
                StatusCode::CONFLICT,
                "operation_in_progress",
                format!("nothing was deployed: {m}"),
            ),
            other => other.into(),
        })?;
        Some(op.id)
    };
    // The supervisor restores lost stateless replicas from this record.
    let service_range = state.service_range.as_deref().filter(|_| enforce);
    let desired = serde_json::to_value(&req.modules).map_err(|e| bad_request(e.to_string()))?;
    let connections = req
        .connections
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(|e| bad_request(e.to_string()))?;
    // Published ports need a service address, so only with a service range.
    let published: &[Published] = if service_range.is_some() {
        &req.published
    } else {
        &[]
    };
    let published_json = (!published.is_empty())
        .then(|| serde_json::to_value(published))
        .transpose()
        .map_err(|e| bad_request(e.to_string()))?;
    apelora_persistence::record_deployment_in(
        &mut tx,
        &apelora_persistence::NewDeployment {
            application_id: app.id,
            project: &req.project,
            revision: revision_no,
            desired: &desired,
            connections: connections.as_ref(),
            uplink: state.network_uplink.as_deref().filter(|_| enforce),
            service_range,
            published: published_json.as_ref(),
        },
    )
    .await?;
    let mut network_job = None;
    if let (true, Some(uplink)) = (enforce, &state.network_uplink) {
        let payload = serde_json::to_value(
            SyncRequest::for_deployment(
                &name,
                &req.project,
                uplink,
                &req.modules,
                req.connections.as_deref().unwrap_or_default(),
                service_range,
            )
            .with_published(published),
        )
        .map_err(|e| bad_request(e.to_string()))?;
        let job_key = key.as_ref().map(|k| format!("{k}:{name}:network.sync"));
        let job_id = apelora_persistence::enqueue_job_from(
            &mut tx,
            "network.sync",
            Some(app.id),
            &payload,
            job_key.as_deref(),
            &apelora_persistence::JobOrigin {
                request_id: super::requests::current().as_deref(),
                queued_by: Some(&p.actor_ref()),
                retry_of: None,
            },
        )
        .await?
        .id();
        network_job = Some(job_id);
    }
    if new_revision {
        super::authz::audit_scoped(
            &mut tx,
            &p,
            "application.deploy.requested",
            &format!("{}/{name}", req.project),
            "success",
            Some(&target.resource),
            &json!({
                "revision": revision_no,
                "create": applied.count(PlanAction::Create),
                "keep": applied.count(PlanAction::Keep),
                "resize": applied.count(PlanAction::Resize),
                "replace": applied.count(PlanAction::Replace),
                "rebuild": applied.count(PlanAction::Rebuild),
                "confirmedDisruption": req.confirm_disruption,
                "operation": operation,
                "remove": if req.remove_missing { applied.count(PlanAction::Remove) } else { 0 },
                "role": p.role,
            }),
        )
        .await?;
    }
    super::authz::commit(tx).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(DeployResponse {
            application: name,
            revision: revision_no,
            plan: applied,
            jobs,
            network_job,
            operation,
        }),
    ))
}

/// The application's instances as Incus reports them, by module. Needs `read-inventory`.
#[utoipa::path(get, path = "/api/v1/applications/{name}/status", tag = "applications", operation_id = "application_status",
    params(("name" = String, Path, description = "Application name"), StatusQuery),
    responses((status = 200, body = ApplicationStatus), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn status(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
    Query(q): Query<StatusQuery>,
) -> Result<Json<ApplicationStatus>, ApiError> {
    if !is_valid_name(&name) {
        return Err(bad_request(format!("invalid application name {name:?}")));
    }
    match &state.auth {
        Auth::Tokens(pool) if p.caller.is_some() => {
            // An application the caller may not view answers exactly like one that does not exist.
            let ctx = apelora_persistence::identity::application_context(pool, &name).await?;
            if !ctx
                .as_ref()
                .is_some_and(|c| allow(&p, "application.view", &c.resource()).is_ok())
            {
                return Err(ApiError::new(
                    StatusCode::NOT_FOUND,
                    "not_found",
                    format!("not found: application {name}"),
                ));
            }
        }
        _ => require(&p, Permission::ReadInventory)?,
    }
    let mut health = match &state.auth {
        Auth::Tokens(pool) => apelora_persistence::application_health(pool, &name).await?,
        Auth::DisabledForDevelopment => Vec::new(),
    };
    let rfc3339 = |t: time::OffsetDateTime| {
        t.format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default()
    };
    let items = state
        .provider
        .instances()
        .await?
        .into_iter()
        .filter(|i| i.managed && q.project.as_ref().is_none_or(|p| &i.project == p))
        .filter_map(|i| {
            let o = i.owner.filter(|o| o.application == name)?;
            // Only a running instance's recorded health is current.
            let h = health
                .iter()
                .position(|h| h.project == i.project && h.instance == i.name)
                .map(|at| health.swap_remove(at))
                .filter(|_| i.status == "Running");
            Some(ModuleStatus {
                module: o.module,
                instance: i.name,
                project: i.project,
                status: i.status,
                location: i.location,
                health: h.map(|h| InstanceHealth {
                    health: apelora_domain::health::HealthState::parse(&h.health),
                    since: rfc3339(h.since),
                    detail: h.detail,
                    restarted_at: h.restarted_at.map(rfc3339),
                }),
            })
        })
        .collect();
    let services = match &state.auth {
        Auth::Tokens(pool) => apelora_persistence::application_services(pool, &name)
            .await?
            .into_iter()
            .filter(|(project, _, _)| q.project.as_ref().is_none_or(|p| p == project))
            .map(|(project, module, address)| ServiceStatus {
                module,
                project,
                address,
            })
            .collect(),
        Auth::DisabledForDevelopment => Vec::new(),
    };
    let traffic_blocked = match &state.auth {
        Auth::Tokens(pool) => apelora_persistence::jobs::failing_network_sync(pool, &name)
            .await?
            .filter(|e| e.starts_with(apelora_domain::network::TRAFFIC_BLOCKED)),
        Auth::DisabledForDevelopment => None,
    };
    Ok(Json(ApplicationStatus {
        source: source(state.provider.mode()),
        items,
        services,
        traffic_blocked,
    }))
}
