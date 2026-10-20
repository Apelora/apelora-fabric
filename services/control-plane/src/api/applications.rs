// SPDX-License-Identifier: AGPL-3.0-only
//! Applications deployed from the Studio (docs/design/application-deployment.md).
//!
//! The Studio sends its document and one `InstanceSpec` per deployable appliance. The plan is
//! computed here from current inventory, for the preview and again when applying (never taken
//! from the caller). Applying saves an immutable revision and queues ordinary instance jobs.

use apelora_domain::InstanceSpec;
use apelora_domain::network::{
    Connection, Internet, Published, SyncRequest, discovery_env, with_discovery,
};
use apelora_domain::plan::{DesiredAppliance, Plan, PlanAction, deploy_spec, owned_spec, plan};
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
const MAX_APPLIANCES: usize = 200;

#[derive(Deserialize, ToSchema)]
pub struct PlanRequest {
    pub project: String,
    pub appliances: Vec<DesiredAppliance>,
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
    pub appliances: Vec<DesiredAppliance>,
    /// The design's connections; enforced when the control plane has a network uplink.
    #[serde(default)]
    pub connections: Option<Vec<Connection>>,
    /// Ports reachable from outside the application (needs connections to be enforced).
    #[serde(default)]
    pub published: Vec<Published>,
    /// Also delete the application's instances that are no longer in the design.
    #[serde(default)]
    pub remove_missing: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct QueuedChange {
    pub appliance: String,
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
}

#[derive(Serialize, ToSchema)]
pub struct ApplianceStatus {
    pub appliance: String,
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

/// A stateless appliance's service address: a load balancer over its healthy replicas.
#[derive(Serialize, ToSchema)]
pub struct ServiceStatus {
    pub appliance: String,
    pub project: String,
    pub address: String,
}

#[derive(Serialize, ToSchema)]
pub struct ApplicationStatus {
    pub source: Source,
    pub items: Vec<ApplianceStatus>,
    /// Allocated service addresses (empty without a database or service range).
    pub services: Vec<ServiceStatus>,
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
    d: &DesiredAppliance,
    replica: u8,
    enforce: bool,
) -> InstanceSpec {
    let uplink = state.network_uplink.as_deref().filter(|_| enforce);
    deploy_spec(name, project, d, replica, uplink)
}

/// Allocates (or finds) the service address of every appliance that gets one — stateless ones
/// that are connected to, and published ones — when connections are enforced and a service
/// range is configured. Returns appliance → address.
async fn allocate_services(
    state: &AppState,
    pool: &apelora_persistence::Pool,
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
        &req.appliances,
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
        match apelora_persistence::service_address(
            pool,
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
    appliances: &[DesiredAppliance],
    connections: Option<&[Connection]>,
) -> Result<(), ApiError> {
    for c in connections.unwrap_or_default() {
        c.validate().map_err(bad_request)?;
        for end in [&c.from, &c.to] {
            if !appliances.iter().any(|d| &d.appliance == end) {
                return Err(bad_request(format!(
                    "connection {} → {}: {end} is not a deployed appliance",
                    c.from, c.to
                )));
            }
        }
    }
    Ok(())
}

fn check_published(
    appliances: &[DesiredAppliance],
    published: &[Published],
) -> Result<(), ApiError> {
    for p in published {
        p.validate().map_err(bad_request)?;
        if !appliances.iter().any(|d| d.appliance == p.appliance) {
            return Err(bad_request(format!(
                "published {}: not a deployed appliance",
                p.appliance
            )));
        }
    }
    Ok(())
}

/// Adds what the designer must know about connections to the plan's warnings.
fn network_warnings(
    state: &AppState,
    plan: &mut Plan,
    desired: &[DesiredAppliance],
    connections: Option<&[Connection]>,
    published: &[Published],
) {
    if !published.is_empty() {
        let names: Vec<&str> = published.iter().map(|p| p.appliance.as_str()).collect();
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
            .map(|d| d.appliance.as_str())
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

fn check(name: &str, project: &str, appliances: &[DesiredAppliance]) -> Result<(), ApiError> {
    if !is_valid_name(name) || name.len() > MAX_NAME {
        return Err(bad_request(format!(
            "invalid application name {name:?} (1–{MAX_NAME} letters, digits and hyphens, starting with a letter)"
        )));
    }
    if !is_valid_name(project) {
        return Err(bad_request(format!("invalid project name {project:?}")));
    }
    if appliances.len() > MAX_APPLIANCES {
        return Err(bad_request(format!(
            "at most {MAX_APPLIANCES} appliances per deployment"
        )));
    }
    let mut seen = std::collections::HashSet::new();
    for d in appliances {
        if !seen.insert(d.appliance.as_str()) {
            return Err(bad_request(format!(
                "appliance {} is listed twice",
                d.appliance
            )));
        }
        d.validate().map_err(bad_request)?;
        owned_spec(name, project, d, d.replicas)
            .validate_from_caller()
            .map_err(|e| bad_request(format!("appliance {}: {e}", d.appliance)))?;
    }
    Ok(())
}

async fn current_plan(
    state: &AppState,
    name: &str,
    project: &str,
    appliances: &[DesiredAppliance],
) -> Result<Plan, ApiError> {
    let observed = state.provider.instances().await?;
    Ok(plan(name, project, appliances, &observed))
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
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    check(&name, &req.project, &req.appliances)?;
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
    check_connections(&req.appliances, req.connections.as_deref())?;
    check_published(&req.appliances, &req.published)?;
    let mut p = current_plan(&state, &name, &req.project, &req.appliances).await?;
    network_warnings(
        &state,
        &mut p,
        &req.appliances,
        req.connections.as_deref(),
        &req.published,
    );
    Ok(Json(p))
}

/// Deploys the design: saves it as a new revision and queues the planned changes. Refuses
/// the whole deployment if any appliance conflicts with an existing instance. Needs `deploy`.
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
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    if !req.document.is_object() {
        return Err(bad_request(
            "the document must be a Studio application document",
        ));
    }
    check(&name, &req.project, &req.appliances)?;
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
    check_connections(&req.appliances, req.connections.as_deref())?;
    check_published(&req.appliances, &req.published)?;
    let pool = store(&state)?;
    let key = idempotency_key(&headers, &p)?;
    // Removing every appliance also removes the application's network objects, even when the
    // request leaves `connections` out (there is nothing left to connect).
    let cleanup = req.appliances.is_empty()
        && req.connections.is_none()
        && state.network_uplink.is_some()
        && apelora_persistence::deployment_enforced(pool, &name, &req.project).await?;
    let enforce = (req.connections.is_some() && state.network_uplink.is_some()) || cleanup;
    let mut applied = current_plan(&state, &name, &req.project, &req.appliances).await?;
    network_warnings(
        &state,
        &mut applied,
        &req.appliances,
        req.connections.as_deref(),
        &req.published,
    );
    if applied.has_conflicts() {
        let conflicts: Vec<String> = applied
            .items
            .iter()
            .filter(|i| i.action == PlanAction::Conflict)
            .map(|i| format!("{}: {}", i.appliance, i.reason))
            .collect();
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "plan_conflict",
            format!("nothing was deployed; {}", conflicts.join("; ")),
        ));
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
    let app = match target.project_id {
        Some(project) => {
            apelora_persistence::identity::ensure_application_in(
                pool,
                &name,
                project,
                target.environment_id,
            )
            .await?
        }
        None => apelora_persistence::ensure_application(pool, &name).await?,
    };
    // A repeated request (same key) finds its revision and its jobs: nothing happens twice.
    let (revision_no, new_revision) = match req.version {
        Some(v) => (v, true),
        None => {
            let (r, new) = apelora_persistence::add_revision_once(
                pool,
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
    let addresses = allocate_services(&state, pool, app.id, &name, &req).await?;
    let mut jobs = Vec::new();
    for item in &applied.items {
        let (kind, payload) = match item.action {
            PlanAction::Create => {
                let desired = req
                    .appliances
                    .iter()
                    .find(|d| d.appliance == item.appliance)
                    .ok_or_else(|| {
                        bad_request(format!("appliance {} disappeared", item.appliance))
                    })?;
                let env = discovery_env(
                    &name,
                    &desired.appliance,
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
            PlanAction::Remove if req.remove_missing => (
                "instance.delete",
                json!({ "project": req.project, "name": item.instance }),
            ),
            _ => continue,
        };
        let job_key = key
            .as_ref()
            .map(|k| format!("{k}:{}:{kind}", item.instance));
        let job_id = apelora_persistence::enqueue_job(
            pool,
            kind,
            Some(app.id),
            &payload,
            job_key.as_deref(),
        )
        .await?
        .id();
        jobs.push(QueuedChange {
            appliance: item.appliance.clone(),
            instance: item.instance.clone(),
            action: item.action,
            job_id,
        });
    }
    // The supervisor restores lost stateless replicas from this record.
    let service_range = state.service_range.as_deref().filter(|_| enforce);
    let desired = serde_json::to_value(&req.appliances).map_err(|e| bad_request(e.to_string()))?;
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
    apelora_persistence::record_deployment(
        pool,
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
                &req.appliances,
                req.connections.as_deref().unwrap_or_default(),
                service_range,
            )
            .with_published(published),
        )
        .map_err(|e| bad_request(e.to_string()))?;
        let job_key = key.as_ref().map(|k| format!("{k}:{name}:network.sync"));
        let job_id = apelora_persistence::enqueue_job(
            pool,
            "network.sync",
            Some(app.id),
            &payload,
            job_key.as_deref(),
        )
        .await?
        .id();
        network_job = Some(job_id);
    }
    if new_revision {
        super::authz::audit_scoped(
            pool,
            &p,
            "application.deploy.requested",
            &format!("{}/{name}", req.project),
            "success",
            Some(&target.resource),
            &json!({
                "revision": revision_no,
                "create": applied.count(PlanAction::Create),
                "keep": applied.count(PlanAction::Keep),
                "remove": if req.remove_missing { applied.count(PlanAction::Remove) } else { 0 },
                "role": p.role,
            }),
        )
        .await?;
    }
    Ok((
        StatusCode::ACCEPTED,
        Json(DeployResponse {
            application: name,
            revision: revision_no,
            plan: applied,
            jobs,
            network_job,
        }),
    ))
}

/// The application's instances as Incus reports them, by appliance. Needs `read-inventory`.
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
            Some(ApplianceStatus {
                appliance: o.appliance,
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
            .map(|(project, appliance, address)| ServiceStatus {
                appliance,
                project,
                address,
            })
            .collect(),
        Auth::DisabledForDevelopment => Vec::new(),
    };
    Ok(Json(ApplicationStatus {
        source: source(state.provider.mode()),
        items,
        services,
    }))
}
