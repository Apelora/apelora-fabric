// SPDX-License-Identifier: AGPL-3.0-only
//! Instance lifecycle requests. The API validates, checks the caller and the project, records
//! the request in the audit log and queues a job; the deployment worker makes the Incus calls.
//! Nothing here talks to Incus directly or waits on it.

use apelora_auth::{EnvKind, Resource};
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

use super::authz::{allow, filter_for};
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
    /// `instance.create`, `instance.start`, `instance.stop`, `instance.restart`,
    /// `instance.delete`, `instance.rebuild`, `instance.resize`, `instance.move`,
    /// `instance.recover`, `volume.backup`, `volume.restore`, `network.sync`, `network.resync` or
    /// `application.operation`.
    pub kind: String,
    /// `pending`, `running`, `succeeded`, `failed` or `cancelled`.
    pub status: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    /// The Incus operation the current attempt is waiting on, if any.
    pub incus_operation: Option<String>,
    /// Every Incus operation the job started, oldest first (kept after they finish).
    pub incus_operations: Vec<String>,
    /// The `x-request-id` of the API request that queued the job; absent for jobs the
    /// supervisor or the scheduler queued. The request's audit rows carry it as `requestId`.
    pub request_id: Option<String>,
    /// What the job was asked to do; its fields depend on `kind` (an instance specification
    /// for `instance.create`, the instance's project and name for `instance.start`, …).
    #[schema(value_type = Object)]
    pub request: Value,
    /// RFC 3339.
    pub created_at: String,
    /// RFC 3339.
    pub updated_at: String,
    /// Who asked for it (`user:…`, `service:…`, `token:…`); absent for jobs the supervisor or
    /// the scheduler queued, and for jobs queued before this was recorded.
    pub queued_by: Option<String>,
    /// The failed job this one retries.
    pub retry_of: Option<Uuid>,
    /// The job that retries this one.
    pub retried_by: Option<Uuid>,
    /// RFC 3339; set once someone asked to cancel it.
    pub cancel_requested_at: Option<String>,
    pub cancel_requested_by: Option<String>,
    /// What the caller may do with it, each with the reason when not.
    pub actions: super::jobs::JobActions,
}

pub(super) fn rfc3339(t: time::OffsetDateTime) -> String {
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
            incus_operations: j.incus_operations,
            request_id: j.request_id,
            request: j.payload,
            created_at: rfc3339(j.created_at),
            updated_at: rfc3339(j.updated_at),
            queued_by: j.queued_by,
            retry_of: j.retry_of,
            retried_by: j.retried_by,
            cancel_requested_at: j.cancel_requested_at.map(rfc3339),
            cancel_requested_by: j.cancel_requested_by,
            actions: super::jobs::JobActions::none(),
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
            "database_not_configured",
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

/// Like [`allowed_project`], and also accepts an Incus project that an Apelora environment is
/// mapped to (the deploy identity's own restrictions in Incus still apply).
pub(super) async fn ensure_project_allowed(
    state: &AppState,
    project: &str,
) -> Result<(), ApiError> {
    if allowed_project(state, project).is_ok() {
        return Ok(());
    }
    if let Auth::Tokens(pool) = &state.auth
        && is_valid_name(project)
        && apelora_persistence::identity::environment_by_incus_project(pool, project)
            .await?
            .is_some()
    {
        return Ok(());
    }
    allowed_project(state, project)
}

/// What a request about a raw instance in an Incus project is authorised against: the
/// environment that owns the project, or the platform when no environment does.
pub(super) async fn instance_resource(
    state: &AppState,
    project: &str,
) -> Result<Resource, ApiError> {
    let Auth::Tokens(pool) = &state.auth else {
        return Ok(Resource::platform());
    };
    Ok(
        match apelora_persistence::identity::environment_by_incus_project(pool, project).await? {
            Some((env, org)) => Resource {
                organization: Some(org),
                project: Some(env.project_id),
                environment: EnvKind::parse(&env.kind),
                ..Resource::default()
            },
            None => Resource::platform(),
        },
    )
}

/// The one authorisation step for raw-instance routes: legacy tokens need `deploy`; people and
/// service identities need `instance.manage` on the environment that owns the project.
pub(super) async fn authorize_instance(
    state: &AppState,
    p: &Principal,
    project: &str,
) -> Result<(), ApiError> {
    if p.caller.is_none() {
        return require(p, Permission::Deploy);
    }
    if !is_valid_name(project) {
        return Err(bad_request(format!("invalid project name {project:?}")));
    }
    allow(
        p,
        "instance.manage",
        &instance_resource(state, project).await?,
    )
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
    Ok(Some(format!("{}:{key}", p.actor_ref())))
}

pub(super) async fn queue(
    state: &AppState,
    p: &Principal,
    headers: &HeaderMap,
    kind: &str,
    target: &str,
    payload: Value,
) -> Result<(StatusCode, Json<JobAccepted>), ApiError> {
    let pool = store(state)?;
    let key = idempotency_key(headers, p)?;
    let request = super::requests::current();
    let actor = p.actor_ref();
    // The job and the request's audit row are queued together: no job without its record.
    let mut tx = super::authz::begin(pool).await?;
    let enqueued = apelora_persistence::enqueue_job_from(
        &mut tx,
        kind,
        None,
        &payload,
        key.as_deref(),
        &apelora_persistence::JobOrigin {
            request_id: request.as_deref(),
            queued_by: Some(&actor),
            retry_of: None,
        },
    )
    .await?;
    let (job_id, created) = match enqueued {
        Enqueued::Created(id) => (id, true),
        Enqueued::Existing(id) => (id, false),
    };
    if created {
        super::authz::audit_as(
            &mut tx,
            p,
            &format!("{kind}.requested"),
            target,
            &json!({ "jobId": job_id, "role": p.role }),
        )
        .await?;
    }
    super::authz::commit(tx).await?;
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
    let Json(spec) = body.map_err(|e| bad_request(e.body_text()))?;
    authorize_instance(&state, &p, &spec.project).await?;
    spec.validate_from_caller().map_err(bad_request)?;
    if !spec.volumes.is_empty() {
        return Err(bad_request(
            "volumes belong to an application's module; deploy them with the application",
        ));
    }
    ensure_project_allowed(&state, &spec.project).await?;
    // In an environment's project: prepared on first use, and its quotas checked before the
    // job is queued (ADR 0012). A repeated request for an instance that exists adds nothing.
    if let Auth::Tokens(pool) = &state.auth
        && let Some((env, _)) =
            apelora_persistence::identity::environment_by_incus_project(pool, &spec.project).await?
    {
        super::tenancy::ensure_ready(&state, &p, env.id).await?;
        let exists = state
            .provider
            .instance(&spec.project, &spec.name)
            .await
            .ok()
            .flatten()
            .is_some();
        let adding = [super::tenancy::Adding {
            config: &spec.config,
            volume_mib: 0,
            resize: None,
        }];
        let adding: &[super::tenancy::Adding<'_>] = if exists { &[] } else { &adding };
        if let Some(q) = super::tenancy::check_deployment(&state, env.id, adding).await?
            && !q.allowed
        {
            return Err(super::tenancy::refuse(&q));
        }
    }
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
    authorize_instance(&state, &p, &project).await?;
    let Json(change) = body.map_err(|e| bad_request(e.body_text()))?;
    ensure_project_allowed(&state, &project).await?;
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
    authorize_instance(&state, &p, &project).await?;
    ensure_project_allowed(&state, &project).await?;
    if !is_valid_name(&name) {
        return Err(bad_request(format!("invalid instance name {name:?}")));
    }
    let target = format!("{project}/{name}");
    let payload = json!({ "project": project, "name": name });
    queue(&state, &p, &headers, "instance.delete", &target, payload).await
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobList {
    pub items: Vec<Job>,
    /// Pass this as `after` for the next, older page; absent on the last page.
    pub next: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct JobFilter {
    /// `pending`, `running`, `succeeded`, `failed` or `cancelled`.
    pub status: Option<String>,
    /// Only kinds starting with this, e.g. `instance.` or `volume.restore`.
    pub kind: Option<String>,
    /// Only the jobs of this application.
    pub application: Option<String>,
    /// Paging: the `next` of the previous page.
    pub after: Option<String>,
    /// Jobs per page, 1 to 200 (default 100).
    pub limit: Option<i64>,
}

/// Largest `limit` `GET /api/v1/jobs` accepts, and the largest page `with_actions` will allocate.
pub(super) const JOB_LIST_LIMIT: i64 = 200;

fn invalid(message: &str) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", message)
}

fn trimmed(v: Option<String>, what: &str) -> Result<Option<String>, ApiError> {
    let Some(v) = v.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    if v.chars().count() > 120 || v.chars().any(char::is_control) {
        return Err(invalid(&format!(
            "{what} is at most 120 characters, without control characters"
        )));
    }
    Ok(Some(v))
}

/// A page position: the creation time (RFC 3339) and id of the last job shown, joined by `|`.
fn cursor(j: &JobView) -> String {
    let at = j
        .created_at
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    format!("{at}|{}", j.id)
}

fn parse_cursor(v: &str) -> Option<(time::OffsetDateTime, Uuid)> {
    let (at, id) = v.split_once('|')?;
    Some((
        time::OffsetDateTime::parse(at, &time::format_description::well_known::Rfc3339).ok()?,
        Uuid::parse_str(id).ok()?,
    ))
}

impl JobFilter {
    fn query(self) -> Result<apelora_persistence::identity::JobQuery, ApiError> {
        let status = trimmed(self.status, "status")?;
        if status.as_deref().is_some_and(|s| {
            !matches!(
                s,
                "pending" | "running" | "succeeded" | "failed" | "cancelled"
            )
        }) {
            return Err(invalid(
                "status is pending, running, succeeded, failed or cancelled",
            ));
        }
        let after = match trimmed(self.after, "after")? {
            None => None,
            Some(c) => {
                Some(parse_cursor(&c).ok_or_else(|| invalid("after is not a page position"))?)
            }
        };
        Ok(apelora_persistence::identity::JobQuery {
            status,
            kind_prefix: trimmed(self.kind, "kind")?,
            application: trimmed(self.application, "application")?,
            after,
        })
    }
}

/// Jobs newest first, filtered by status, kind and application and paged with `after`. Whole
/// platform readers (`read-inventory`) see every job; anyone else the jobs of applications they
/// may view.
#[utoipa::path(get, path = "/api/v1/jobs", tag = "deployment", operation_id = "jobs", params(JobFilter),
    responses((status = 200, body = JobList), (status = 400, body = ErrorBody), (status = 401, body = ErrorBody),
        (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn jobs(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    axum::extract::Query(f): axum::extract::Query<JobFilter>,
) -> Result<Json<JobList>, ApiError> {
    let pool = store(&state)?;
    let limit = f.limit.unwrap_or(100);
    if !(1..=JOB_LIST_LIMIT).contains(&limit) {
        return Err(invalid("limit is 1 to 200"));
    }
    let q = f.query()?;
    // Whole-platform readers see every job; anyone else sees the jobs of applications they may view.
    let scope = if require(&p, Permission::ReadInventory).is_ok() {
        None
    } else {
        let filter = filter_for(&p, "application.view");
        if filter.is_empty() {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                "you may not see jobs (cluster.view or application.view)",
            ));
        }
        Some(filter)
    };
    // One more than shown tells whether another page exists.
    let mut rows =
        apelora_persistence::identity::job_history(pool, scope.as_ref(), &q, limit + 1).await?;
    let next = (rows.len() as i64 > limit).then(|| {
        rows.truncate(limit as usize);
        rows.last().map(cursor)
    });
    Ok(Json(JobList {
        items: super::jobs::with_actions(&state, &p, rows).await?,
        next: next.flatten(),
    }))
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
    let pool = store(&state)?;
    let job = super::jobs::visible_job(&state, &p, pool, id).await?;
    let mut out = super::jobs::with_actions(&state, &p, vec![job]).await?;
    Ok(Json(out.remove(0)))
}

/// Whether the caller may see a job: whole-platform readers see every job; anyone else the jobs
/// of applications they may view, or of instances in environments they manage.
pub(super) async fn job_visible(
    state: &AppState,
    p: &Principal,
    job: &JobView,
) -> Result<bool, ApiError> {
    if require(p, Permission::ReadInventory).is_ok() {
        return Ok(true);
    }
    let pool = store(state)?;
    Ok(match job.application_id {
        Some(app) => {
            match apelora_persistence::identity::application_context_by_id(pool, app).await? {
                Some(ctx) => allow(p, "application.view", &ctx.resource()).is_ok(),
                None => false,
            }
        }
        None => match job.payload.get("project").and_then(Value::as_str) {
            Some(project) if is_valid_name(project) => allow(
                p,
                "instance.manage",
                &instance_resource(state, project).await?,
            )
            .is_ok(),
            _ => false,
        },
    })
}
