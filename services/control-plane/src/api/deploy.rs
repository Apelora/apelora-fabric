// SPDX-License-Identifier: AGPL-3.0-only
//! Instance lifecycle requests. The API validates, checks the caller and the project, records
//! the request in the audit log and queues a job; the deployment worker makes the Incus calls.
//! Nothing here talks to Incus directly or waits on it.

use apelora_domain::{InstanceAction, InstanceSpec, Permission, is_valid_name};
use apelora_persistence::{Enqueued, JobView};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::{ApiError, AppState, Auth, ErrorBody, Principal, require};

/// Header that makes a request safe to repeat: the same key from the same caller returns the
/// job created the first time.
pub const IDEMPOTENCY_KEY: &str = "idempotency-key";

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobAccepted {
    pub job_id: Uuid,
    /// False when an earlier request with the same idempotency key already created the job.
    pub created: bool,
}

#[derive(Deserialize, ToSchema)]
pub struct StateChange {
    pub action: InstanceAction,
}

#[derive(Deserialize, IntoParams)]
pub struct InstancePath {
    /// Incus project.
    pub project: String,
    /// Instance name.
    pub name: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Job {
    pub id: Uuid,
    /// `instance.create`, `instance.start`, `instance.stop` or `instance.delete`.
    pub kind: String,
    /// `pending`, `running`, `succeeded`, `failed` or `cancelled`.
    pub status: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    /// The Incus operation the current attempt is waiting on, if any.
    pub incus_operation: Option<String>,
    #[schema(value_type = Object)]
    pub request: Value,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub updated_at: String,
}

fn rfc3339(t: time::OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

impl From<JobView> for Job {
    fn from(j: JobView) -> Self {
        Job {
            id: j.id,
            kind: j.kind,
            status: j.status,
            attempts: j.attempts,
            max_attempts: j.max_attempts,
            last_error: j.last_error,
            incus_operation: j.external_operation,
            request: j.payload,
            created_at: rfc3339(j.created_at),
            updated_at: rfc3339(j.updated_at),
        }
    }
}

pub(super) fn bad_request(msg: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", msg)
}

pub(super) fn store(state: &AppState) -> Result<&apelora_persistence::Pool, ApiError> {
    match &state.auth {
        Auth::Tokens(pool) => Ok(pool),
        Auth::DisabledForDevelopment => Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "store_unavailable",
            "deployments need a database (DATABASE_URL)",
        )),
    }
}

/// Apelora only changes projects it was configured for (`APELORA_DEPLOY_PROJECTS`).
pub(super) fn allowed_project(state: &AppState, project: &str) -> Result<(), ApiError> {
    if !is_valid_name(project) {
        return Err(bad_request(format!("invalid project name {project:?}")));
    }
    if state.deploy_projects.iter().any(|p| p == project) {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "project_not_allowed",
            format!(
                "Apelora is not configured to change project {project}; allowed: {}",
                state.deploy_projects.join(", ")
            ),
        ))
    }
}

pub(super) fn idempotency_key(
    headers: &HeaderMap,
    p: &Principal,
) -> Result<Option<String>, ApiError> {
    let Some(v) = headers.get(IDEMPOTENCY_KEY) else {
        return Ok(None);
    };
    let key = v
        .to_str()
        .ok()
        .filter(|k| (1..=128).contains(&k.len()) && k.bytes().all(|b| b.is_ascii_graphic()))
        .ok_or_else(|| bad_request("Idempotency-Key must be 1–128 visible ASCII characters"))?;
    // Scoped to the caller so two callers can never collide on a key.
    Ok(Some(format!("{}:{key}", p.name)))
}

async fn queue(
    state: &AppState,
    p: &Principal,
    headers: &HeaderMap,
    kind: &str,
    target: &str,
    payload: Value,
) -> Result<(StatusCode, Json<JobAccepted>), ApiError> {
    let pool = store(state)?;
    let key = idempotency_key(headers, p)?;
    let enqueued =
        apelora_persistence::enqueue_job(pool, kind, None, &payload, key.as_deref()).await?;
    let (job_id, created) = match enqueued {
        Enqueued::Created(id) => (id, true),
        Enqueued::Existing(id) => (id, false),
    };
    if created {
        apelora_persistence::audit(
            pool,
            &p.name,
            &format!("{kind}.requested"),
            target,
            &json!({ "jobId": job_id, "role": p.role }),
        )
        .await?;
    }
    Ok((StatusCode::ACCEPTED, Json(JobAccepted { job_id, created })))
}

/// Queues creation of an instance. Needs `deploy`. Apelora marks it as its own; only such
/// instances can later be stopped or deleted through Apelora.
#[utoipa::path(post, path = "/api/v1/instances", tag = "deployment", request_body = InstanceSpec,
    params(("Idempotency-Key" = Option<String>, Header, description = "Makes the request safe to repeat")),
    responses((status = 202, body = JobAccepted), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn create_instance(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    body: Result<Json<InstanceSpec>, JsonRejection>,
) -> Result<(StatusCode, Json<JobAccepted>), ApiError> {
    require(&p, Permission::Deploy)?;
    let Json(spec) = body.map_err(|e| bad_request(e.body_text()))?;
    spec.validate_from_caller().map_err(bad_request)?;
    allowed_project(&state, &spec.project)?;
    let target = format!("{}/{}", spec.project, spec.name);
    let payload = serde_json::to_value(&spec).map_err(|e| bad_request(e.to_string()))?;
    queue(&state, &p, &headers, "instance.create", &target, payload).await
}

/// Queues starting or stopping an Apelora instance. Needs `deploy`.
#[utoipa::path(post, path = "/api/v1/instances/{project}/{name}/state", tag = "deployment",
    params(InstancePath, ("Idempotency-Key" = Option<String>, Header, description = "Makes the request safe to repeat")),
    request_body = StateChange,
    responses((status = 202, body = JobAccepted), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn change_state(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(InstancePath { project, name }): Path<InstancePath>,
    headers: HeaderMap,
    body: Result<Json<StateChange>, JsonRejection>,
) -> Result<(StatusCode, Json<JobAccepted>), ApiError> {
    require(&p, Permission::Deploy)?;
    let Json(change) = body.map_err(|e| bad_request(e.body_text()))?;
    allowed_project(&state, &project)?;
    if !is_valid_name(&name) {
        return Err(bad_request(format!("invalid instance name {name:?}")));
    }
    let kind = format!("instance.{}", change.action.as_str());
    let target = format!("{project}/{name}");
    let payload = json!({ "project": project, "name": name });
    queue(&state, &p, &headers, &kind, &target, payload).await
}

/// Queues deletion of an Apelora instance (stopped first if running). Needs `deploy`.
#[utoipa::path(delete, path = "/api/v1/instances/{project}/{name}", tag = "deployment",
    params(InstancePath, ("Idempotency-Key" = Option<String>, Header, description = "Makes the request safe to repeat")),
    responses((status = 202, body = JobAccepted), (status = 400, body = ErrorBody),
        (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn delete_instance(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(InstancePath { project, name }): Path<InstancePath>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<JobAccepted>), ApiError> {
    require(&p, Permission::Deploy)?;
    allowed_project(&state, &project)?;
    if !is_valid_name(&name) {
        return Err(bad_request(format!("invalid instance name {name:?}")));
    }
    let target = format!("{project}/{name}");
    let payload = json!({ "project": project, "name": name });
    queue(&state, &p, &headers, "instance.delete", &target, payload).await
}

#[derive(Serialize, ToSchema)]
pub struct JobList {
    pub items: Vec<Job>,
}

/// The 100 most recent jobs, newest first. Needs `read-inventory`.
#[utoipa::path(get, path = "/api/v1/jobs", tag = "deployment", operation_id = "jobs",
    responses((status = 200, body = JobList), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn jobs(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<JobList>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let pool = store(&state)?;
    let items = apelora_persistence::recent_jobs(pool, 100)
        .await?
        .into_iter()
        .map(Job::from)
        .collect();
    Ok(Json(JobList { items }))
}

/// A queued job and its progress. Needs `read-inventory`.
#[utoipa::path(get, path = "/api/v1/jobs/{id}", tag = "deployment",
    params(("id" = Uuid, Path, description = "Job id")),
    responses((status = 200, body = Job), (status = 401, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn job(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<Json<Job>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let pool = store(&state)?;
    Ok(Json(apelora_persistence::get_job(pool, id).await?.into()))
}
