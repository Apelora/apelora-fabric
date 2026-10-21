// SPDX-License-Identifier: AGPL-3.0-only
//! Moving an instance to another member (docs/design/mobility-and-ha.md, slice A): a dry run
//! that says whether it moves live or must stop, and the request, which runs the check again
//! and queues an `instance.move` job. The project must allow member targeting
//! (`restricted.cluster.target=allow`); Incus refuses the move otherwise.

use apelora_domain::is_valid_name;
use apelora_domain::mobility::{MovePlan, MoveSubject, plan_move};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::{IntoParams, ToSchema};

use super::deploy::{
    InstancePath, JobAccepted, authorize_instance, bad_request, ensure_project_allowed, queue,
    store,
};
use super::{ApiError, AppState, ErrorBody, Principal, Source, source};

#[derive(Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub(super) struct MoveQuery {
    /// The member to move to.
    target: String,
    /// Whether a short stop is acceptable when it cannot move while running.
    #[serde(default)]
    allow_stop: bool,
}

#[derive(Serialize, ToSchema)]
pub struct MovePreview {
    pub source: Source,
    pub plan: MovePlan,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct MoveRequest {
    pub target: String,
    /// The instance's name again, typed by the person moving it.
    pub confirm: String,
    #[serde(default)]
    pub allow_stop: bool,
}

/// The plan, from what Incus reports now and the instance's recorded volumes. Also returns
/// the application the instance belongs to.
pub(super) async fn plan(
    state: &AppState,
    project: &str,
    name: &str,
    target: &str,
    allow_stop: bool,
) -> Result<(MovePlan, String), ApiError> {
    if !is_valid_name(name) || !is_valid_name(target) {
        return Err(bad_request("invalid instance or member name"));
    }
    let not_found = || {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            format!("not found: instance {project}/{name}"),
        )
    };
    let now = state
        .provider
        .instance(project, name)
        .await?
        .ok_or_else(not_found)?;
    let application = state
        .provider
        .instances()
        .await?
        .into_iter()
        .find(|i| i.project == project && i.name == name)
        .and_then(|i| i.owner.map(|o| o.application))
        .unwrap_or_default();
    let members = state.provider.cluster_members().await?;
    let volumes: Vec<(String, u32)> =
        apelora_persistence::instance_volumes(store(state)?, project, name)
            .await?
            .into_iter()
            // Volumes on the shared pool are reachable from every member: nothing to copy.
            .filter(|v| !v.member.is_empty() && v.member != "none")
            .map(|v| (v.incus_name, u32::try_from(v.size_mib).unwrap_or(0)))
            .collect();
    let plan = plan_move(
        &MoveSubject {
            name,
            kind: now.kind,
            status: &now.status,
            location: &now.location,
            architecture: &now.architecture,
            stateful: now.stateful,
            managed: now.managed && !application.is_empty(),
            volumes: &volumes,
        },
        target,
        &members,
        allow_stop,
    );
    tracing::debug!(instance = %format!("{project}/{name}"), application = %application, from = %plan.from, to = %plan.to, live = plan.live, stops = plan.stops, copy_mib = plan.copy_mib, blocked = ?plan.blocked, "move: plan");
    Ok((plan, application))
}

/// Dry run of moving an instance to another member: whether it keeps running, what it copies,
/// what blocks it. Changes nothing. Needs `deploy` (or `instance.manage`).
#[utoipa::path(get, path = "/api/v1/instances/{project}/{name}/move", tag = "deployment", operation_id = "preview_instance_move",
    params(InstancePath, MoveQuery),
    responses((status = 200, body = MovePreview), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn preview(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(InstancePath { project, name }): Path<InstancePath>,
    Query(q): Query<MoveQuery>,
) -> Result<Json<MovePreview>, ApiError> {
    authorize_instance(&state, &p, &project).await?;
    ensure_project_allowed(&state, &project).await?;
    let (plan, _) = plan(&state, &project, &name, &q.target, q.allow_stop).await?;
    Ok(Json(MovePreview {
        source: source(state.provider.mode()),
        plan,
    }))
}

/// Moves an Apelora instance to another member, after the check runs again and its name is
/// typed: live when it can, otherwise only with `allowStop`, copying its volumes there.
/// Queues an `instance.move` job. Needs `deploy` (or `instance.manage`); audited.
#[utoipa::path(post, path = "/api/v1/instances/{project}/{name}/move", tag = "deployment", operation_id = "move_instance",
    params(InstancePath, ("Idempotency-Key" = Option<String>, Header, description = "Makes the request safe to repeat")),
    request_body = MoveRequest,
    responses((status = 202, body = JobAccepted), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn request(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(InstancePath { project, name }): Path<InstancePath>,
    headers: HeaderMap,
    body: Result<Json<MoveRequest>, JsonRejection>,
) -> Result<(StatusCode, Json<JobAccepted>), ApiError> {
    authorize_instance(&state, &p, &project).await?;
    ensure_project_allowed(&state, &project).await?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    if req.confirm != name {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "confirmation_mismatch",
            format!("type the instance's name ({name}) to confirm"),
        ));
    }
    let (plan, application) = plan(&state, &project, &name, &req.target, req.allow_stop).await?;
    if !plan.blocked.is_empty() {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "move_blocked",
            format!("nothing was moved: {}", plan.blocked.join("; ")),
        ));
    }
    let payload = move_payload(&state, &project, &name, &application, &plan).await?;
    let target = format!("{project}/{name}");
    queue(&state, &p, &headers, "instance.move", &target, payload).await
}

/// The `instance.move` job payload for a plan that is not blocked: where it is and goes, whether
/// it moves live, whether it was running (so it is started again), and its volumes on
/// member-local pools. Also what an application's migration records for each of its instances.
pub(super) async fn move_payload(
    state: &AppState,
    project: &str,
    name: &str,
    application: &str,
    plan: &MovePlan,
) -> Result<serde_json::Value, ApiError> {
    let start_after = state
        .provider
        .instance(project, name)
        .await?
        .is_some_and(|s| s.status == "Running");
    let records: Vec<_> = apelora_persistence::instance_volumes(store(state)?, project, name)
        .await?
        .into_iter()
        .filter(|v| !v.member.is_empty() && v.member != "none")
        .collect();
    Ok(json!({
        "project": project, "name": name, "application": application,
        "from": plan.from, "target": plan.to, "live": plan.live, "startAfter": start_after,
        "volumes": records.iter().map(|v| json!({ "pool": v.storage_pool, "name": v.incus_name })).collect::<Vec<_>>(),
    }))
}
