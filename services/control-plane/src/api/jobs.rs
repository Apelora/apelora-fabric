// SPDX-License-Identifier: AGPL-3.0-only
//! Job control: retry a failed job, cancel a pending or running one, and a job's detail (its
//! attempts with their timings and Incus operations, who queued it, and the jobs linked to it).
//!
//! Both actions need the permission that queues that kind of job, checked on the job's own
//! target (its application, or the environment of its instance's Incus project): the people who
//! may ask for a change may also take it back or ask for it again, nobody else. Both are
//! audited, refusals included. The design and the safe points are in
//! `apelora_persistence::jobs` and `apelora_deployment_worker::cancelled_before_start`.

use std::collections::HashMap;

use apelora_auth::Resource;
use apelora_domain::is_valid_name;
use apelora_persistence::JobView;
use apelora_persistence::jobs::{AttemptView, Cancel, Retry, RetryRequest};
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use serde::Serialize;
use serde_json::{Value, json};
use utoipa::ToSchema;
use uuid::Uuid;

use super::authz::{allow, audit_scoped};
use super::deploy::{
    Job, JobAccepted, ensure_project_allowed, idempotency_key, instance_resource, job_visible,
    rfc3339, store,
};
use super::manage::ActionState;
use super::{ApiError, AppState, ErrorBody, Principal};

/// What the caller may do with a job.
#[derive(Serialize, ToSchema)]
pub struct JobActions {
    pub retry: ActionState,
    pub cancel: ActionState,
}

impl JobActions {
    /// Nothing allowed (before the caller's permissions are applied).
    pub fn none() -> Self {
        let no = || ActionState {
            allowed: false,
            reason: None,
        };
        JobActions {
            retry: no(),
            cancel: no(),
        }
    }
}

/// One attempt of a job.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobAttempt {
    pub attempt: i32,
    /// The worker that ran it.
    pub worker: String,
    /// RFC 3339.
    pub started_at: String,
    pub ended_at: Option<String>,
    /// `succeeded`, `failed`, `rejected`, `waiting`, `cancelled` or `lost` (its worker stopped
    /// renewing the lease and another worker took the job over); absent while it runs.
    pub outcome: Option<String>,
    pub error: Option<String>,
    /// Incus operations this attempt started, oldest first.
    pub incus_operations: Vec<String>,
}

impl From<AttemptView> for JobAttempt {
    fn from(a: AttemptView) -> Self {
        JobAttempt {
            attempt: a.attempt,
            worker: a.worker,
            started_at: rfc3339(a.started_at),
            ended_at: a.ended_at.map(rfc3339),
            outcome: a.outcome,
            error: a.error,
            incus_operations: a.incus_operations,
        }
    }
}

/// A job linked to the one shown.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct LinkedJob {
    pub id: Uuid,
    pub kind: String,
    pub status: String,
    /// `retry-of` (the failed job this one repeats), `retried-by`, or `same-request` (queued
    /// by the same API request, e.g. an application operation's instance jobs).
    pub relation: String,
    /// `project/name` or the application.
    pub target: String,
    /// RFC 3339.
    pub created_at: String,
}

/// A job with everything known about it.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobDetail {
    pub job: Job,
    /// The application it belongs to, if any.
    pub application: Option<String>,
    /// `project/name` or the application.
    pub target: String,
    /// RFC 3339: when the first attempt started, and when it reached a final status.
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub attempts: Vec<JobAttempt>,
    pub linked: Vec<LinkedJob>,
    /// Where a cancelled job of this kind stops.
    pub safe_points: String,
}

/// The answer to a cancellation request.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct JobCancellation {
    /// `cancelled` (it had not started: done) or `requested` (the worker stops at the next
    /// safe point).
    pub outcome: String,
    pub job: Job,
}

/// Where a job of `kind` stops when cancelled; the same rules as the worker
/// (`apelora_deployment_worker::cancelled_before_start` and the handlers).
pub fn safe_points(kind: &str) -> &'static str {
    match kind {
        "instance.start" | "instance.stop" | "instance.restart" | "instance.delete" => {
            "Before each change it asks Incus for. A change already sent is waited on to its end, so the instance is left running or stopped, never in between."
        }
        "instance.create" => {
            "Only before the instance is created. Once it exists, its volumes are attached and it is started, so a cancelled create never leaves a half-built instance."
        }
        "instance.move" => {
            "While the instance is still on its original member: it is started again there if the move had stopped it. Once it is on the new member, the move is finished."
        }
        "application.operation" => {
            "Between phases. The instance jobs of the running phase finish; later phases are not started."
        }
        _ => "Only before it starts. Once running, it finishes its single pass.",
    }
}

/// `project/name` for instance jobs, the application otherwise.
pub(super) fn target_of(job: &JobView, application: Option<&str>) -> String {
    let field = |k: &str| job.payload.get(k).and_then(Value::as_str);
    match (field("project"), field("name")) {
        (Some(p), Some(n)) => format!("{p}/{n}"),
        _ => application
            .or_else(|| field("application"))
            .unwrap_or("-")
            .to_owned(),
    }
}

/// The permission that queues a job of this kind, and the resource it is checked on. The
/// kinds a person starts on an application need `application.operate` (start, stop, restart,
/// backups) or `application.deploy` (create, delete, networks); single instances outside an
/// application need `instance.manage` on the environment that owns the project.
struct Authority {
    permission: &'static str,
    resource: Resource,
    /// The Incus project, for instance jobs (must still be one Apelora may change).
    project: Option<String>,
}

async fn authority(
    state: &AppState,
    pool: &apelora_persistence::Pool,
    job: &JobView,
    contexts: &mut HashMap<String, Option<Resource>>,
) -> Result<Authority, ApiError> {
    let operate = matches!(
        job.kind.as_str(),
        "application.operation"
            | "instance.start"
            | "instance.stop"
            | "instance.restart"
            | "volume.backup"
    );
    let app_permission = if operate {
        "application.operate"
    } else {
        "application.deploy"
    };
    let field = |k: &str| job.payload.get(k).and_then(Value::as_str);
    let app_resource = |ctx: Option<apelora_persistence::identity::AppContext>| {
        ctx.map_or_else(Resource::platform, |c| c.resource())
    };
    if let Some(app) = job.application_id {
        let key = app.to_string();
        if !contexts.contains_key(&key) {
            let ctx = apelora_persistence::identity::application_context_by_id(pool, app).await?;
            contexts.insert(key.clone(), Some(app_resource(ctx)));
        }
        return Ok(Authority {
            permission: app_permission,
            resource: contexts[&key].clone().unwrap_or_else(Resource::platform),
            project: None,
        });
    }
    if let (Some(project), Some(_)) = (field("project"), field("name"))
        && job.kind != "volume.backup"
    {
        let key = format!("project:{project}");
        if !contexts.contains_key(&key) {
            let r = if is_valid_name(project) {
                Some(instance_resource(state, project).await?)
            } else {
                None
            };
            contexts.insert(key.clone(), r);
        }
        return Ok(Authority {
            permission: "instance.manage",
            resource: contexts[&key].clone().unwrap_or_else(Resource::platform),
            project: Some(project.to_owned()),
        });
    }
    if let Some(name) = field("application") {
        let key = format!("app:{name}");
        if !contexts.contains_key(&key) {
            let ctx = apelora_persistence::identity::application_context(pool, name).await?;
            contexts.insert(key.clone(), Some(app_resource(ctx)));
        }
        return Ok(Authority {
            permission: app_permission,
            resource: contexts[&key].clone().unwrap_or_else(Resource::platform),
            project: None,
        });
    }
    Ok(Authority {
        permission: "cluster.manage",
        resource: Resource::platform(),
        project: None,
    })
}

fn state(allowed: bool, reason: impl Into<String>) -> ActionState {
    ActionState {
        allowed,
        reason: (!allowed).then(|| reason.into()),
    }
}

fn actions(job: &Job, permitted: Result<(), String>) -> JobActions {
    let retry = if job.status != "failed" {
        state(false, "Only failed jobs can be retried.")
    } else if job.kind == "application.operation" {
        state(
            false,
            "Start, stop or restart the application again from its page: an operation is planned anew from what runs now.",
        )
    } else if let Some(newer) = job.retried_by {
        state(false, format!("Retried already by job {newer}."))
    } else if let Err(why) = &permitted {
        state(false, why.clone())
    } else {
        state(true, "")
    };
    let cancel = if !matches!(job.status.as_str(), "pending" | "running") {
        state(false, "Only waiting or running jobs can be cancelled.")
    } else if let Some(by) = &job.cancel_requested_by {
        state(
            false,
            format!("Cancellation was requested by {by}; it stops at the next safe point."),
        )
    } else if let Err(why) = &permitted {
        state(false, why.clone())
    } else {
        state(true, "")
    };
    JobActions { retry, cancel }
}

/// API jobs with what the caller may do with each.
pub(super) async fn with_actions(
    state: &AppState,
    p: &Principal,
    rows: Vec<JobView>,
) -> Result<Vec<Job>, ApiError> {
    let pool = store(state)?;
    let mut contexts = HashMap::new();
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let auth = authority(state, pool, &row, &mut contexts).await?;
        let permitted = allow(p, auth.permission, &auth.resource)
            .map_err(|_| format!("You may not do this ({}).", auth.permission));
        let mut job = Job::from(row);
        job.actions = actions(&job, permitted);
        out.push(job);
    }
    Ok(out)
}

fn not_found(id: Uuid) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "not_found",
        format!("not found: job {id}"),
    )
}

/// The job, or the same 404 as for a job that does not exist when the caller may not see it.
pub(super) async fn visible_job(
    state: &AppState,
    p: &Principal,
    pool: &apelora_persistence::Pool,
    id: Uuid,
) -> Result<JobView, ApiError> {
    let job = match apelora_persistence::get_job(pool, id).await {
        Err(apelora_persistence::StoreError::NotFound(_)) => return Err(not_found(id)),
        other => other?,
    };
    if job_visible(state, p, &job).await? {
        Ok(job)
    } else {
        Err(not_found(id))
    }
}

/// Checks the permission that queued the job (403, audited as `access.denied` by the layer) and
/// that an instance job's project is still one Apelora may change.
async fn authorize(
    state: &AppState,
    p: &Principal,
    pool: &apelora_persistence::Pool,
    job: &JobView,
) -> Result<(), ApiError> {
    let auth = authority(state, pool, job, &mut HashMap::new()).await?;
    allow(p, auth.permission, &auth.resource)?;
    if let Some(project) = &auth.project {
        ensure_project_allowed(state, project).await?;
    }
    Ok(())
}

/// Audits a refusal (outcome `denied`) and returns it.
async fn refuse(
    pool: &apelora_persistence::Pool,
    p: &Principal,
    action: &str,
    job: &JobView,
    code: &'static str,
    message: String,
) -> ApiError {
    // A refusal changes nothing: its row is written on its own.
    let result = super::authz::audit_alone(
        pool,
        p,
        action,
        &target_of(job, None),
        "denied",
        None,
        &json!({ "jobId": job.id, "kind": job.kind, "code": code, "reason": message }),
    )
    .await;
    super::authz::audit_failed(action, result);
    ApiError::new(StatusCode::CONFLICT, code, message)
}

/// Queues a new job that repeats a failed one, linked to it (`retryOf`); the failed job is kept
/// unchanged as evidence. Needs the permission that queues that kind of job on its target.
/// Refused (409) when the job did not fail (`not_failed`), was retried already
/// (`already_retried`), a newer job for the same target was queued since (`superseded`), or it
/// is an application operation (`not_retryable`: start it again from the application). Audited.
#[utoipa::path(post, path = "/api/v1/jobs/{id}/retry", tag = "deployment", operation_id = "retry_job",
    params(("id" = Uuid, Path, description = "Job id"),
        ("Idempotency-Key" = Option<String>, Header, description = "Makes the request safe to repeat")),
    responses((status = 202, body = JobAccepted), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 409, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn retry(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<JobAccepted>), ApiError> {
    let pool = store(&state)?;
    let job = visible_job(&state, &p, pool, id).await?;
    authorize(&state, &p, pool, &job).await?;
    if job.kind == "application.operation" {
        return Err(refuse(pool, &p, "job.retry", &job, "not_retryable",
            "an application operation is not retried: start, stop or restart the application again; it is planned anew from what runs now".into()).await);
    }
    let key = idempotency_key(&headers, &p)?;
    let actor = p.actor_ref();
    let request = super::requests::current();
    // The new job and its audit row commit together; a refusal rolls back and is audited alone.
    let mut tx = super::authz::begin(pool).await?;
    let result = apelora_persistence::jobs::retry_job_in(
        &mut tx,
        id,
        &RetryRequest {
            queued_by: &actor,
            request_id: request.as_deref(),
            idempotency_key: key.as_deref(),
        },
    )
    .await?;
    let (new, created) = match result {
        Retry::Queued(new) => (new, true),
        Retry::Repeated(new) => (new, false),
        Retry::NotFailed(status) => {
            return Err(refuse(
                pool,
                &p,
                "job.retry",
                &job,
                "not_failed",
                format!("only failed jobs can be retried; this one is {status}"),
            )
            .await);
        }
        Retry::AlreadyRetried(newer) => {
            return Err(refuse(
                pool,
                &p,
                "job.retry",
                &job,
                "already_retried",
                format!("this job was retried already by job {newer}; retry that one if it failed"),
            )
            .await);
        }
        Retry::Superseded(newer) => {
            return Err(refuse(pool, &p, "job.retry", &job, "superseded",
                format!("a newer job for {} was queued since (job {newer}); retrying this one would undo it", target_of(&job, None))).await);
        }
    };
    if created {
        audit_scoped(
            &mut tx,
            &p,
            "job.retry",
            &target_of(&job, None),
            "success",
            None,
            &json!({ "jobId": new, "retryOf": id, "kind": job.kind }),
        )
        .await?;
        tracing::info!(job = %id, new_job = %new, kind = %job.kind, "job: retry queued");
    }
    super::authz::commit(tx).await?;
    Ok((
        StatusCode::ACCEPTED,
        Json(JobAccepted {
            job_id: new,
            created,
        }),
    ))
}

/// Cancels a job. A waiting job with nothing in flight is cancelled at once (`cancelled`);
/// otherwise the request is recorded (`requested`) and the worker stops at the job kind's next
/// safe point, never in the middle of an Incus change (see `safePoints` on the job's detail).
/// Repeating the request changes nothing. Refused (409 `job_finished`) for a finished job. Needs
/// the permission that queues that kind of job on its target; audited.
#[utoipa::path(post, path = "/api/v1/jobs/{id}/cancel", tag = "deployment", operation_id = "cancel_job",
    params(("id" = Uuid, Path, description = "Job id")),
    responses((status = 200, body = JobCancellation), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 409, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn cancel(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<Json<JobCancellation>, ApiError> {
    let pool = store(&state)?;
    let job = visible_job(&state, &p, pool, id).await?;
    authorize(&state, &p, pool, &job).await?;
    // The cancellation and its audit row commit together; a refusal is audited alone.
    let mut tx = super::authz::begin(pool).await?;
    let outcome =
        match apelora_persistence::jobs::request_cancel_in(&mut tx, id, &p.actor_ref()).await? {
            Cancel::Finished(status) => {
                drop(tx);
                return Err(refuse(
                    pool,
                    &p,
                    "job.cancel",
                    &job,
                    "job_finished",
                    format!("the job is {status}; only waiting or running jobs can be cancelled"),
                )
                .await);
            }
            Cancel::AlreadyRequested => "requested",
            c @ (Cancel::Cancelled | Cancel::Requested) => {
                let outcome = if c == Cancel::Cancelled {
                    "cancelled"
                } else {
                    "requested"
                };
                audit_scoped(
                &mut tx,
                &p,
                "job.cancel",
                &target_of(&job, None),
                "success",
                None,
                &json!({ "jobId": id, "kind": job.kind, "outcome": outcome, "status": job.status }),
            )
            .await?;
                tracing::info!(job = %id, kind = %job.kind, outcome, "job: cancellation");
                outcome
            }
        };
    super::authz::commit(tx).await?;
    let now = apelora_persistence::get_job(pool, id).await?;
    let mut out = with_actions(&state, &p, vec![now]).await?;
    Ok(Json(JobCancellation {
        outcome: outcome.into(),
        job: out.remove(0),
    }))
}

/// A job with its attempts (worker, timings, outcome, error and the Incus operations each
/// started), who queued it, its request id, and the jobs linked to it (retries, and other jobs
/// the same request queued). Same visibility as `GET /api/v1/jobs/{id}`.
#[utoipa::path(get, path = "/api/v1/jobs/{id}/detail", tag = "deployment", operation_id = "job_detail",
    params(("id" = Uuid, Path, description = "Job id")),
    responses((status = 200, body = JobDetail), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn detail(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Result<Json<JobDetail>, ApiError> {
    let pool = store(&state)?;
    let row = visible_job(&state, &p, pool, id).await?;
    let attempts: Vec<JobAttempt> = apelora_persistence::jobs::job_attempts(pool, id)
        .await?
        .into_iter()
        .map(JobAttempt::from)
        .collect();
    let application = match row.application_id {
        Some(app) => apelora_persistence::jobs::application_name(pool, app).await?,
        None => None,
    };
    let mut linked = Vec::new();
    for other in apelora_persistence::jobs::linked_jobs(pool, &row).await? {
        if !job_visible(&state, &p, &other).await? {
            continue;
        }
        let relation = if Some(other.id) == row.retry_of {
            "retry-of"
        } else if other.retry_of == Some(row.id) {
            "retried-by"
        } else {
            "same-request"
        };
        linked.push(LinkedJob {
            id: other.id,
            kind: other.kind.clone(),
            status: other.status.clone(),
            relation: relation.into(),
            target: target_of(&other, None),
            created_at: rfc3339(other.created_at),
        });
    }
    let finished = matches!(row.status.as_str(), "succeeded" | "failed" | "cancelled");
    let (target, kind) = (target_of(&row, application.as_deref()), row.kind.clone());
    let finished_at = finished.then(|| rfc3339(row.updated_at));
    let mut job = with_actions(&state, &p, vec![row]).await?;
    Ok(Json(JobDetail {
        job: job.remove(0),
        application,
        target,
        started_at: attempts.first().map(|a| a.started_at.clone()),
        finished_at,
        attempts,
        linked,
        safe_points: safe_points(&kind).into(),
    }))
}
