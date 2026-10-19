// SPDX-License-Identifier: AGPL-3.0-only
//! PostgreSQL persistence for Apelora's own state: applications and their immutable revisions,
//! durable jobs with leases and fencing generations, and the audit log.
//!
//! Incus keeps its own database; nothing here mirrors or edits it.

use serde_json::Value;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{FromRow, Row};
use uuid::Uuid;

pub use sqlx::PgPool as Pool;

mod tokens;
pub use tokens::{TokenRecord, authenticate, create_token, list_tokens, revoke_token};

/// Embedded migrations (`crates/persistence/migrations`).
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("not found: {0}")]
    NotFound(String),
    #[error("conflict: {0}")]
    Conflict(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// Connects and applies pending migrations.
pub async fn connect(database_url: &str) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(database_url)
        .await?;
    MIGRATOR.run(&pool).await?;
    Ok(pool)
}

#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct Application {
    pub id: Uuid,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct Revision {
    pub id: Uuid,
    pub application_id: Uuid,
    pub revision: i32,
    pub document: Value,
    pub created_by: String,
}

pub async fn create_application(pool: &PgPool, name: &str) -> Result<Application> {
    let app = sqlx::query_as::<_, Application>(
        "INSERT INTO applications (id, name) VALUES ($1, $2) RETURNING id, name",
    )
    .bind(Uuid::new_v4())
    .bind(name)
    .fetch_one(pool)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            StoreError::Conflict(format!("application {name} already exists"))
        }
        _ => StoreError::Database(e),
    })?;
    Ok(app)
}

/// Saves a new immutable revision numbered one above the latest. Concurrent saves of the same
/// application are serialised by locking the application row.
pub async fn add_revision(
    pool: &PgPool,
    application_id: Uuid,
    document: &Value,
    created_by: &str,
) -> Result<Revision> {
    let mut tx = pool.begin().await?;
    let exists = sqlx::query("SELECT 1 FROM applications WHERE id = $1 FOR UPDATE")
        .bind(application_id)
        .fetch_optional(&mut *tx)
        .await?;
    if exists.is_none() {
        return Err(StoreError::NotFound(format!(
            "application {application_id}"
        )));
    }
    let rev = sqlx::query_as::<_, Revision>(
        "INSERT INTO application_revisions (id, application_id, revision, document, created_by)
         VALUES ($1, $2, COALESCE((SELECT max(revision) FROM application_revisions WHERE application_id = $2), 0) + 1, $3, $4)
         RETURNING id, application_id, revision, document, created_by",
    )
    .bind(Uuid::new_v4())
    .bind(application_id)
    .bind(document)
    .bind(created_by)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(rev)
}

pub async fn latest_revision(pool: &PgPool, application_id: Uuid) -> Result<Option<Revision>> {
    Ok(sqlx::query_as::<_, Revision>(
        "SELECT id, application_id, revision, document, created_by FROM application_revisions
         WHERE application_id = $1 ORDER BY revision DESC LIMIT 1",
    )
    .bind(application_id)
    .fetch_optional(pool)
    .await?)
}

/// A job a worker currently owns. Every write about it must quote `generation`.
#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct ClaimedJob {
    pub id: Uuid,
    pub kind: String,
    pub application_id: Option<Uuid>,
    pub payload: Value,
    pub attempts: i32,
    pub max_attempts: i32,
    pub generation: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Enqueued {
    Created(Uuid),
    /// A job with the same idempotency key already exists; nothing new was created.
    Existing(Uuid),
}

impl Enqueued {
    pub fn id(&self) -> Uuid {
        match self {
            Enqueued::Created(id) | Enqueued::Existing(id) => *id,
        }
    }
}

pub async fn enqueue_job(
    pool: &PgPool,
    kind: &str,
    application_id: Option<Uuid>,
    payload: &Value,
    idempotency_key: Option<&str>,
) -> Result<Enqueued> {
    let id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO jobs (id, kind, application_id, payload, idempotency_key) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (idempotency_key) DO NOTHING RETURNING id",
    )
    .bind(id)
    .bind(kind)
    .bind(application_id)
    .bind(payload)
    .bind(idempotency_key)
    .fetch_optional(pool)
    .await?;
    if inserted.is_some() {
        return Ok(Enqueued::Created(id));
    }
    let existing: Uuid = sqlx::query_scalar("SELECT id FROM jobs WHERE idempotency_key = $1")
        .bind(idempotency_key)
        .fetch_one(pool)
        .await?;
    Ok(Enqueued::Existing(existing))
}

/// Claims one runnable job: pending and due, or running with an expired lease (its worker is
/// presumed dead). Concurrent workers never claim the same job (`FOR UPDATE SKIP LOCKED`).
pub async fn claim_job(pool: &PgPool, worker: &str, lease_secs: i64) -> Result<Option<ClaimedJob>> {
    Ok(sqlx::query_as::<_, ClaimedJob>(
        "UPDATE jobs SET status = 'running', lease_owner = $1, lease_until = now() + make_interval(secs => $2),
                attempts = attempts + 1, generation = generation + 1, updated_at = now()
         WHERE id = (
             SELECT id FROM jobs
             WHERE (status = 'pending' AND run_after <= now())
                OR (status = 'running' AND lease_until < now())
             ORDER BY run_after, created_at
             FOR UPDATE SKIP LOCKED
             LIMIT 1
         )
         RETURNING id, kind, application_id, payload, attempts, max_attempts, generation",
    )
    .bind(worker)
    .bind(lease_secs as f64)
    .fetch_optional(pool)
    .await?)
}

/// Extends a lease. Fails with `Conflict` if the job was claimed again since (stale generation).
pub async fn renew_lease(
    pool: &PgPool,
    job: &ClaimedJob,
    worker: &str,
    lease_secs: i64,
) -> Result<()> {
    let n = sqlx::query(
        "UPDATE jobs SET lease_until = now() + make_interval(secs => $4), updated_at = now()
         WHERE id = $1 AND lease_owner = $2 AND generation = $3 AND status = 'running'",
    )
    .bind(job.id)
    .bind(worker)
    .bind(job.generation)
    .bind(lease_secs as f64)
    .execute(pool)
    .await?
    .rows_affected();
    fenced(n, job)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Succeeded,
    /// Retried with backoff until `max_attempts`, then marked failed.
    Failed(String),
}

/// Records the result. Rejected with `Conflict` when another worker has claimed the job since.
pub async fn complete_job(
    pool: &PgPool,
    job: &ClaimedJob,
    worker: &str,
    outcome: &Outcome,
) -> Result<()> {
    let n = match outcome {
        Outcome::Succeeded => sqlx::query(
            "UPDATE jobs SET status = 'succeeded', lease_owner = NULL, lease_until = NULL, last_error = NULL, updated_at = now()
             WHERE id = $1 AND lease_owner = $2 AND generation = $3 AND status = 'running'",
        )
        .bind(job.id)
        .bind(worker)
        .bind(job.generation)
        .execute(pool)
        .await?
        .rows_affected(),
        Outcome::Failed(err) => sqlx::query(
            "UPDATE jobs SET
                status = CASE WHEN attempts >= max_attempts THEN 'failed' ELSE 'pending' END,
                run_after = now() + make_interval(secs => least(300, 2 ^ attempts)::double precision),
                lease_owner = NULL, lease_until = NULL, last_error = $4, updated_at = now()
             WHERE id = $1 AND lease_owner = $2 AND generation = $3 AND status = 'running'",
        )
        .bind(job.id)
        .bind(worker)
        .bind(job.generation)
        .bind(err)
        .execute(pool)
        .await?
        .rows_affected(),
    };
    fenced(n, job)
}

fn fenced(rows: u64, job: &ClaimedJob) -> Result<()> {
    if rows == 1 {
        Ok(())
    } else {
        Err(StoreError::Conflict(format!(
            "job {} generation {} is no longer owned by this worker",
            job.id, job.generation
        )))
    }
}

pub async fn job_status(pool: &PgPool, id: Uuid) -> Result<(String, i32, Option<String>)> {
    let row = sqlx::query("SELECT status, attempts, last_error FROM jobs WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| StoreError::NotFound(format!("job {id}")))?;
    Ok((
        row.get("status"),
        row.get("attempts"),
        row.get("last_error"),
    ))
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, FromRow)]
pub struct AuditEvent {
    pub id: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub at: time::OffsetDateTime,
    pub actor: String,
    pub action: String,
    pub target: String,
    pub detail: Value,
}

/// Most recent audit events first.
pub async fn recent_audit(pool: &PgPool, limit: i64) -> Result<Vec<AuditEvent>> {
    Ok(sqlx::query_as::<_, AuditEvent>(
        "SELECT id, at, actor, action, target, detail FROM audit_events ORDER BY id DESC LIMIT $1",
    )
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?)
}

pub async fn audit(
    pool: &PgPool,
    actor: &str,
    action: &str,
    target: &str,
    detail: &Value,
) -> Result<()> {
    sqlx::query("INSERT INTO audit_events (actor, action, target, detail) VALUES ($1, $2, $3, $4)")
        .bind(actor)
        .bind(action)
        .bind(target)
        .bind(detail)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
