// SPDX-License-Identifier: AGPL-3.0-only
//! Job control: cancelling, retrying, the attempt history and the jobs linked to a job.
//!
//! **Retry** queues a *new* job linked to the failed one (`retry_of`), never the failed row
//! re-queued: the failed job keeps its attempts, error and Incus operations as evidence, the new
//! job starts with fencing generation 0 and its own request id, and a worker still holding a
//! stale claim on the old row cannot write to the new one. A failed job is retried at most once
//! (unique index); to try again, retry the newer job.
//!
//! **Cancel** ends a pending job at once when nothing of it is in flight. Otherwise (it is
//! running, an earlier attempt's Incus operation is still unfinished, or it is a coordinating
//! `application.operation`) the request is recorded and the worker stops at the job kind's next
//! safe point, never in the middle of an Incus change.

use sqlx::{Connection, FromRow, PgConnection, PgPool};
use uuid::Uuid;

use crate::{Enqueued, JOB_COLS, JobOrigin, JobView, Result, StoreError};

/// What a cancellation request did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Cancel {
    /// The job had not started (and nothing of it was in flight): it is cancelled.
    Cancelled,
    /// Recorded; the worker stops at the next safe point.
    Requested,
    /// Someone had already asked; nothing changed.
    AlreadyRequested,
    /// The job is finished (`succeeded`, `failed` or `cancelled`); nothing to cancel.
    Finished(String),
}

/// Asks for a job to be cancelled (see the module documentation for when it ends at once).
pub async fn request_cancel(pool: &PgPool, id: Uuid, by: &str) -> Result<Cancel> {
    let mut tx = pool.begin().await?;
    let out = request_cancel_in(&mut tx, id, by).await?;
    tx.commit().await?;
    Ok(out)
}

/// [`request_cancel`] inside the caller's transaction (with the request's audit row).
pub async fn request_cancel_in(conn: &mut PgConnection, id: Uuid, by: &str) -> Result<Cancel> {
    let mut tx = conn.begin().await?;
    // Waits for a claim in progress, so the status read here is the one that counts.
    let row: Option<(String, String, Option<String>, bool)> = sqlx::query_as(
        "SELECT status, kind, external_operation, cancel_requested_at IS NOT NULL
         FROM jobs WHERE id = $1 FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((status, kind, external, requested)) = row else {
        return Err(StoreError::NotFound(format!("job {id}")));
    };
    if !matches!(status.as_str(), "pending" | "running") {
        return Ok(Cancel::Finished(status));
    }
    if requested {
        return Ok(Cancel::AlreadyRequested);
    }
    let at_once = status == "pending" && external.is_none() && kind != "application.operation";
    sqlx::query(
        "UPDATE jobs SET cancel_requested_at = now(), cancel_requested_by = $2, updated_at = now(),
                status = CASE WHEN $3 THEN 'cancelled' ELSE status END
         WHERE id = $1",
    )
    .bind(id)
    .bind(by)
    .bind(at_once)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(if at_once {
        Cancel::Cancelled
    } else {
        Cancel::Requested
    })
}

/// Whether someone has asked to cancel the job since it was claimed: handlers ask this at their
/// safe points. Returns who asked.
pub async fn cancel_requested(pool: &PgPool, id: Uuid) -> Result<Option<String>> {
    Ok(sqlx::query_scalar::<_, Option<String>>(
        "SELECT cancel_requested_by FROM jobs WHERE id = $1 AND cancel_requested_at IS NOT NULL",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .flatten())
}

/// What a retry request did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Retry {
    /// A new job was queued.
    Queued(Uuid),
    /// The same idempotency key queued this job earlier.
    Repeated(Uuid),
    /// Only failed jobs are retried; this one is in the given status.
    NotFailed(String),
    /// The failed job was retried already, by this job.
    AlreadyRetried(Uuid),
    /// A newer job for the same target was queued since; retrying would undo it.
    Superseded(Uuid),
}

/// Who asks for a retry, and how the new job is identified.
#[derive(Debug, Clone, Copy)]
pub struct RetryRequest<'a> {
    pub queued_by: &'a str,
    pub request_id: Option<&'a str>,
    pub idempotency_key: Option<&'a str>,
}

/// Queues a new job that repeats a failed one (same kind, application and payload), linked to
/// it. Refused when the job did not fail, was retried already, or a newer job for the same
/// target exists.
pub async fn retry_job(pool: &PgPool, id: Uuid, r: &RetryRequest<'_>) -> Result<Retry> {
    let mut tx = pool.begin().await?;
    let out = retry_job_in(&mut tx, id, r).await?;
    tx.commit().await?;
    Ok(out)
}

/// [`retry_job`] inside the caller's transaction (with the request's audit row). Its own work
/// runs in a savepoint, so a lost race on the unique `retry_of` index leaves the caller's
/// transaction usable.
pub async fn retry_job_in(
    conn: &mut PgConnection,
    id: Uuid,
    r: &RetryRequest<'_>,
) -> Result<Retry> {
    if let Some(key) = r.idempotency_key
        && let Some(existing) =
            sqlx::query_scalar::<_, Uuid>("SELECT id FROM jobs WHERE idempotency_key = $1")
                .bind(key)
                .fetch_optional(&mut *conn)
                .await?
    {
        return Ok(Retry::Repeated(existing));
    }
    let mut tx = conn.begin().await?;
    let job = sqlx::query_as::<_, JobView>(&format!(
        "SELECT {JOB_COLS} FROM jobs j WHERE j.id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    .ok_or_else(|| StoreError::NotFound(format!("job {id}")))?;
    if job.status != "failed" {
        return Ok(Retry::NotFailed(job.status));
    }
    if let Some(newer) = job.retried_by {
        return Ok(Retry::AlreadyRetried(newer));
    }
    if let Some(newer) = newer_for_target(&mut tx, &job).await? {
        return Ok(Retry::Superseded(newer));
    }
    let origin = JobOrigin {
        request_id: r.request_id,
        queued_by: Some(r.queued_by),
        retry_of: Some(job.id),
    };
    let queued = crate::enqueue_job_from(
        &mut tx,
        &job.kind,
        job.application_id,
        &job.payload,
        r.idempotency_key,
        &origin,
    )
    .await;
    let queued = match queued {
        Ok(q) => q,
        // Two retries at once: the unique index on `retry_of` lets one through.
        Err(StoreError::Database(sqlx::Error::Database(d))) if d.is_unique_violation() => {
            tx.rollback().await?;
            let newer = sqlx::query_scalar::<_, Uuid>("SELECT id FROM jobs WHERE retry_of = $1")
                .bind(id)
                .fetch_one(&mut *conn)
                .await?;
            return Ok(Retry::AlreadyRetried(newer));
        }
        Err(e) => return Err(e),
    };
    sqlx::query("UPDATE jobs SET max_attempts = $2 WHERE id = $1")
        .bind(queued.id())
        .bind(job.max_attempts)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(match queued {
        Enqueued::Created(new) => Retry::Queued(new),
        Enqueued::Existing(new) => Retry::Repeated(new),
    })
}

/// The newest job for the same target created after `job`: the same instance (`project`/`name`
/// in the payload), or the same kind for the same application otherwise.
async fn newer_for_target(conn: &mut sqlx::PgConnection, job: &JobView) -> Result<Option<Uuid>> {
    let field = |k: &str| job.payload.get(k).and_then(serde_json::Value::as_str);
    if let (Some(project), Some(name)) = (field("project"), field("name")) {
        return Ok(sqlx::query_scalar(
            "SELECT id FROM jobs WHERE payload->>'project' = $1 AND payload->>'name' = $2
                                  AND created_at > $3 AND id <> $4
             ORDER BY created_at DESC LIMIT 1",
        )
        .bind(project)
        .bind(name)
        .bind(job.created_at)
        .bind(job.id)
        .fetch_optional(&mut *conn)
        .await?);
    }
    let Some(app) = job.application_id else {
        return Ok(None);
    };
    Ok(sqlx::query_scalar(
        "SELECT id FROM jobs WHERE application_id = $1 AND kind = $2 AND created_at > $3 AND id <> $4
         ORDER BY created_at DESC LIMIT 1",
    )
    .bind(app)
    .bind(&job.kind)
    .bind(job.created_at)
    .bind(job.id)
    .fetch_optional(&mut *conn)
    .await?)
}

/// One attempt of a job, oldest first.
#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct AttemptView {
    pub attempt: i32,
    pub worker: String,
    pub started_at: time::OffsetDateTime,
    pub ended_at: Option<time::OffsetDateTime>,
    /// `succeeded`, `failed`, `rejected`, `waiting`, `cancelled`, `lost`; `None` while it runs.
    pub outcome: Option<String>,
    pub error: Option<String>,
    pub incus_operations: Vec<String>,
}

/// Jobs that failed (finally) in the last `hours`, newest first, at most `limit` (support bundle).
pub async fn failed_jobs(pool: &PgPool, hours: i64, limit: i64) -> Result<Vec<JobView>> {
    Ok(sqlx::query_as::<_, JobView>(&format!(
        "SELECT {JOB_COLS} FROM jobs j
         WHERE j.status = 'failed' AND j.updated_at > now() - make_interval(hours => $1)
         ORDER BY j.updated_at DESC, j.id LIMIT $2"
    ))
    .bind(i32::try_from(hours).unwrap_or(i32::MAX))
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?)
}

pub async fn job_attempts(pool: &PgPool, id: Uuid) -> Result<Vec<AttemptView>> {
    Ok(sqlx::query_as::<_, AttemptView>(
        "SELECT attempt, worker, started_at, ended_at, outcome, error, incus_operations
         FROM job_attempts WHERE job_id = $1 ORDER BY attempt",
    )
    .bind(id)
    .fetch_all(pool)
    .await?)
}

/// Jobs linked to `job`: the one it retries, the one retrying it, and other jobs the same API
/// request queued (an application operation's instance jobs, a deployment's jobs), oldest first.
pub async fn linked_jobs(pool: &PgPool, job: &JobView) -> Result<Vec<JobView>> {
    Ok(sqlx::query_as::<_, JobView>(&format!(
        "SELECT {JOB_COLS} FROM jobs j
         WHERE j.id <> $1 AND (j.id = $2 OR j.retry_of = $1 OR ($3::text IS NOT NULL AND j.request_id = $3))
         ORDER BY j.created_at, j.id LIMIT 50"
    ))
    .bind(job.id)
    .bind(job.retry_of)
    .bind(job.request_id.as_deref())
    .fetch_all(pool)
    .await?)
}

/// The name of the application a job belongs to.
pub async fn application_name(pool: &PgPool, id: Uuid) -> Result<Option<String>> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT name FROM applications WHERE id = $1")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// The latest jobs for one instance, newest first.
pub async fn instance_jobs(
    pool: &PgPool,
    project: &str,
    name: &str,
    limit: i64,
) -> Result<Vec<JobView>> {
    Ok(sqlx::query_as::<_, JobView>(&format!(
        "SELECT {JOB_COLS} FROM jobs j
         WHERE j.payload ? 'name' AND j.payload->>'project' = $1 AND j.payload->>'name' = $2
         ORDER BY j.created_at DESC, j.id LIMIT $3"
    ))
    .bind(project)
    .bind(name)
    .bind(limit.clamp(1, 100))
    .fetch_all(pool)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        MIGRATOR, Outcome, claim_job, complete_job, enqueue_job, get_job, record_operation,
    };
    use serde_json::json;

    async fn queued(pool: &PgPool, kind: &str, name: &str) -> Uuid {
        enqueue_job(
            pool,
            kind,
            None,
            &json!({ "project": "apelora", "name": name }),
            None,
        )
        .await
        .expect("enqueue")
        .id()
    }

    async fn fail_for_good(pool: &PgPool, id: Uuid) {
        sqlx::query("UPDATE jobs SET max_attempts = 1 WHERE id = $1")
            .bind(id)
            .execute(pool)
            .await
            .expect("cap");
        let job = claim_job(pool, "w", 30).await.expect("claim").expect("job");
        assert_eq!(job.id, id);
        complete_job(pool, &job, "w", &Outcome::Failed("Incus said no".into()))
            .await
            .expect("complete");
        assert_eq!(get_job(pool, id).await.expect("job").status, "failed");
    }

    fn by<'a>(key: Option<&'a str>) -> RetryRequest<'a> {
        RetryRequest {
            queued_by: "user:alice",
            request_id: Some("req-retry"),
            idempotency_key: key,
        }
    }

    #[sqlx::test(migrator = "MIGRATOR")]
    async fn a_pending_job_is_cancelled_at_once_and_never_claimed(pool: PgPool) {
        let id = queued(&pool, "instance.stop", "web").await;
        assert_eq!(
            request_cancel(&pool, id, "user:alice")
                .await
                .expect("cancel"),
            Cancel::Cancelled
        );
        let j = get_job(&pool, id).await.expect("job");
        assert_eq!(
            (j.status.as_str(), j.cancel_requested_by.as_deref()),
            ("cancelled", Some("user:alice"))
        );
        assert!(claim_job(&pool, "w", 30).await.expect("claim").is_none());
        assert_eq!(
            request_cancel(&pool, id, "user:bob").await.expect("again"),
            Cancel::Finished("cancelled".into())
        );
    }

    #[sqlx::test(migrator = "MIGRATOR")]
    async fn a_running_job_gets_a_request_its_worker_sees_and_finishes_as_cancelled(pool: PgPool) {
        let id = queued(&pool, "instance.stop", "web").await;
        let job = claim_job(&pool, "w", 30)
            .await
            .expect("claim")
            .expect("job");
        assert!(!job.cancel_requested);
        assert_eq!(
            request_cancel(&pool, id, "user:alice")
                .await
                .expect("cancel"),
            Cancel::Requested
        );
        assert_eq!(
            request_cancel(&pool, id, "user:bob").await.expect("again"),
            Cancel::AlreadyRequested
        );
        assert_eq!(get_job(&pool, id).await.expect("job").status, "running");
        assert_eq!(
            cancel_requested(&pool, id).await.expect("ask").as_deref(),
            Some("user:alice")
        );
        complete_job(
            &pool,
            &job,
            "w",
            &Outcome::Cancelled("stopped before stopping web".into()),
        )
        .await
        .expect("complete");
        assert_eq!(get_job(&pool, id).await.expect("job").status, "cancelled");
        let attempts = job_attempts(&pool, id).await.expect("attempts");
        assert_eq!(attempts.len(), 1);
        assert_eq!(
            (attempts[0].outcome.as_deref(), attempts[0].error.as_deref()),
            (Some("cancelled"), Some("stopped before stopping web"))
        );
    }

    #[sqlx::test(migrator = "MIGRATOR")]
    async fn a_pending_job_with_an_operation_in_flight_or_a_coordinator_waits_for_its_worker(
        pool: PgPool,
    ) {
        let waiting = queued(&pool, "instance.start", "web").await;
        sqlx::query("UPDATE jobs SET external_operation = 'op-1', attempts = 1 WHERE id = $1")
            .bind(waiting)
            .execute(&pool)
            .await
            .expect("an attempt left its operation unfinished");
        assert_eq!(
            request_cancel(&pool, waiting, "user:alice")
                .await
                .expect("cancel"),
            Cancel::Requested
        );
        let coordinator = enqueue_job(&pool, "application.operation", None, &json!({}), None)
            .await
            .expect("enqueue")
            .id();
        assert_eq!(
            request_cancel(&pool, coordinator, "user:alice")
                .await
                .expect("cancel"),
            Cancel::Requested
        );
        for id in [waiting, coordinator] {
            assert_eq!(get_job(&pool, id).await.expect("job").status, "pending");
        }
        // The next claim carries the request to the worker.
        let next = claim_job(&pool, "w", 30)
            .await
            .expect("claim")
            .expect("job");
        assert!(next.cancel_requested);
    }

    #[sqlx::test(migrator = "MIGRATOR")]
    async fn a_retry_is_a_new_job_linked_to_the_failed_one_which_keeps_its_history(pool: PgPool) {
        let id = queued(&pool, "instance.start", "web").await;
        fail_for_good(&pool, id).await;
        let Retry::Queued(new) = retry_job(&pool, id, &by(Some("k1"))).await.expect("retry") else {
            panic!("not queued")
        };
        let (old, fresh) = (
            get_job(&pool, id).await.expect("old"),
            get_job(&pool, new).await.expect("new"),
        );
        assert_eq!(
            (
                old.status.as_str(),
                old.attempts,
                old.last_error.as_deref(),
                old.retried_by
            ),
            ("failed", 1, Some("Incus said no"), Some(new)),
            "the failed job is evidence and is not changed"
        );
        assert_eq!(
            (
                fresh.status.as_str(),
                fresh.attempts,
                fresh.kind.as_str(),
                fresh.retry_of,
                fresh.queued_by.as_deref(),
                fresh.request_id.as_deref(),
                fresh.max_attempts
            ),
            (
                "pending",
                0,
                "instance.start",
                Some(id),
                Some("user:alice"),
                Some("req-retry"),
                1
            )
        );
        assert_eq!(fresh.payload, old.payload);
        assert_eq!(
            retry_job(&pool, id, &by(Some("k1")))
                .await
                .expect("same key"),
            Retry::Repeated(new)
        );
        assert_eq!(
            retry_job(&pool, id, &by(None)).await.expect("again"),
            Retry::AlreadyRetried(new)
        );
        assert_eq!(
            retry_job(&pool, new, &by(None)).await.expect("not failed"),
            Retry::NotFailed("pending".into())
        );
        let linked = linked_jobs(&pool, &fresh).await.expect("linked");
        assert_eq!(linked.iter().map(|j| j.id).collect::<Vec<_>>(), vec![id]);
    }

    #[sqlx::test(migrator = "MIGRATOR")]
    async fn a_retry_that_would_undo_a_newer_request_is_refused(pool: PgPool) {
        let stop = queued(&pool, "instance.stop", "web").await;
        fail_for_good(&pool, stop).await;
        let start = queued(&pool, "instance.start", "web").await;
        assert_eq!(
            retry_job(&pool, stop, &by(None)).await.expect("retry"),
            Retry::Superseded(start)
        );
        let other = queued(&pool, "instance.start", "db").await;
        assert_ne!(other, start);
    }

    #[sqlx::test(migrator = "MIGRATOR")]
    async fn attempts_record_each_claim_its_operations_and_a_lost_lease(pool: PgPool) {
        let id = queued(&pool, "instance.start", "web").await;
        let first = claim_job(&pool, "w1", 30)
            .await
            .expect("claim")
            .expect("job");
        record_operation(&pool, &first, "w1", Some("op-a"))
            .await
            .expect("record");
        // The first worker stalls; its lease expires and another worker takes the job over.
        sqlx::query("UPDATE jobs SET lease_until = now() - interval '1 second' WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .expect("expire");
        let second = claim_job(&pool, "w2", 30)
            .await
            .expect("claim")
            .expect("job");
        assert!(
            complete_job(&pool, &first, "w1", &Outcome::Succeeded)
                .await
                .is_err(),
            "the stale worker's result is refused"
        );
        complete_job(&pool, &second, "w2", &Outcome::Succeeded)
            .await
            .expect("complete");
        let a = job_attempts(&pool, id).await.expect("attempts");
        assert_eq!(
            a.iter()
                .map(|x| (x.attempt, x.worker.as_str(), x.outcome.as_deref()))
                .collect::<Vec<_>>(),
            vec![(1, "w1", Some("lost")), (2, "w2", Some("succeeded"))]
        );
        assert_eq!(a[0].incus_operations, vec!["op-a".to_owned()]);
        assert!(a.iter().all(|x| x.ended_at.is_some()));
    }

    #[sqlx::test(migrator = "MIGRATOR")]
    async fn a_waiting_coordinator_keeps_one_attempt_row(pool: PgPool) {
        let id = enqueue_job(&pool, "application.operation", None, &json!({}), None)
            .await
            .expect("enqueue")
            .id();
        for _ in 0..3 {
            sqlx::query("UPDATE jobs SET run_after = now() WHERE id = $1")
                .bind(id)
                .execute(&pool)
                .await
                .expect("due");
            let job = claim_job(&pool, "w", 30)
                .await
                .expect("claim")
                .expect("job");
            complete_job(&pool, &job, "w", &Outcome::Waiting(2))
                .await
                .expect("wait");
        }
        let a = job_attempts(&pool, id).await.expect("attempts");
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].outcome.as_deref(), Some("waiting"));
    }

    #[sqlx::test(migrator = "MIGRATOR")]
    async fn instance_jobs_are_found_by_their_target(pool: PgPool) {
        let a = queued(&pool, "instance.start", "web").await;
        let _ = queued(&pool, "instance.start", "db").await;
        let b = queued(&pool, "instance.stop", "web").await;
        let jobs = instance_jobs(&pool, "apelora", "web", 10)
            .await
            .expect("jobs");
        assert_eq!(jobs.iter().map(|j| j.id).collect::<Vec<_>>(), vec![b, a]);
    }
}
