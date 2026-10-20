// SPDX-License-Identifier: AGPL-3.0-only
//! Applications deployed from the Studio (docs/design/application-deployment.md).
//!
//! The Studio sends its document and one `InstanceSpec` per deployable appliance. The plan is
//! computed here from current inventory, for the preview and again when applying (never taken
//! from the caller). Applying saves an immutable revision and queues ordinary instance jobs.

use apelora_domain::plan::{DesiredAppliance, Plan, PlanAction, owned_spec, plan};
use apelora_domain::{Permission, is_valid_name};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::deploy::{allowed_project, bad_request, idempotency_key, store};
use super::{ApiError, AppState, ErrorBody, Principal, Source, require, source};

/// Application names become instance-name prefixes, so they are short.
const MAX_NAME: usize = 40;
const MAX_APPLIANCES: usize = 200;

#[derive(Deserialize, ToSchema)]
pub struct PlanRequest {
    pub project: String,
    pub appliances: Vec<DesiredAppliance>,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct DeployRequest {
    pub project: String,
    /// The Studio document, saved as an immutable revision.
    #[schema(value_type = Object)]
    pub document: Value,
    pub appliances: Vec<DesiredAppliance>,
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
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationInfo {
    pub name: String,
    pub latest_revision: Option<i32>,
    /// RFC 3339.
    pub created_at: String,
}

#[derive(Serialize, ToSchema)]
pub struct ApplicationList {
    pub items: Vec<ApplicationInfo>,
}

#[derive(Serialize, ToSchema)]
pub struct ApplianceStatus {
    pub appliance: String,
    pub instance: String,
    pub project: String,
    /// Incus status, e.g. `Running`.
    pub status: String,
    pub location: String,
}

#[derive(Serialize, ToSchema)]
pub struct ApplicationStatus {
    pub source: Source,
    pub items: Vec<ApplianceStatus>,
}

#[derive(Deserialize, IntoParams)]
pub struct StatusQuery {
    /// Only this Incus project.
    pub project: Option<String>,
}

fn check(
    state: &AppState,
    name: &str,
    project: &str,
    appliances: &[DesiredAppliance],
) -> Result<(), ApiError> {
    if !is_valid_name(name) || name.len() > MAX_NAME {
        return Err(bad_request(format!(
            "invalid application name {name:?} (1–{MAX_NAME} letters, digits and hyphens, starting with a letter)"
        )));
    }
    allowed_project(state, project)?;
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
        owned_spec(name, project, d)
            .validate()
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

/// Applications that have been deployed, with their latest revision. Needs `read-inventory`.
#[utoipa::path(get, path = "/api/v1/applications", tag = "applications", operation_id = "applications",
    responses((status = 200, body = ApplicationList), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<ApplicationList>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let items = apelora_persistence::list_applications(store(&state)?)
        .await?
        .into_iter()
        .map(|a| ApplicationInfo {
            name: a.name,
            latest_revision: a.latest_revision,
            created_at: a
                .created_at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
        })
        .collect();
    Ok(Json(ApplicationList { items }))
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
    require(&p, Permission::Deploy)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    check(&state, &name, &req.project, &req.appliances)?;
    Ok(Json(
        current_plan(&state, &name, &req.project, &req.appliances).await?,
    ))
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
    require(&p, Permission::Deploy)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    if !req.document.is_object() {
        return Err(bad_request(
            "the document must be a Studio application document",
        ));
    }
    check(&state, &name, &req.project, &req.appliances)?;
    let pool = store(&state)?;
    let key = idempotency_key(&headers, &p)?;
    let applied = current_plan(&state, &name, &req.project, &req.appliances).await?;
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

    let app = apelora_persistence::ensure_application(pool, &name).await?;
    // A repeated request (same key) finds its revision and its jobs: nothing happens twice.
    let (revision, new_revision) = apelora_persistence::add_revision_once(
        pool,
        app.id,
        &req.document,
        &p.name,
        key.as_ref().map(|k| format!("{k}:{name}")).as_deref(),
    )
    .await?;
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
                let spec = owned_spec(&name, &req.project, desired);
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
    if new_revision {
        apelora_persistence::audit(
            pool,
            &p.name,
            "application.deploy.requested",
            &format!("{}/{name}", req.project),
            &json!({
                "revision": revision.revision,
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
            revision: revision.revision,
            plan: applied,
            jobs,
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
    require(&p, Permission::ReadInventory)?;
    if !is_valid_name(&name) {
        return Err(bad_request(format!("invalid application name {name:?}")));
    }
    let items = state
        .provider
        .instances()
        .await?
        .into_iter()
        .filter(|i| i.managed && q.project.as_ref().is_none_or(|p| &i.project == p))
        .filter_map(|i| {
            let o = i.owner.filter(|o| o.application == name)?;
            Some(ApplianceStatus {
                appliance: o.appliance,
                instance: i.name,
                project: i.project,
                status: i.status,
                location: i.location,
            })
        })
        .collect();
    Ok(Json(ApplicationStatus {
        source: source(state.provider.mode()),
        items,
    }))
}
