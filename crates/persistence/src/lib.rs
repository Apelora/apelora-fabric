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

mod health;
pub mod identity;
mod machines;
pub use health::{HealthRecord, NewHealth, application_health, forget_health, record_health};
mod tokens;
pub use machines::{
    MachineRecord, admit_machine, approve_machine, machine, machines, register_machine,
    remove_machine, withdraw_approval,
};
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

/// The application with this name, created if it does not exist yet.
pub async fn ensure_application(pool: &PgPool, name: &str) -> Result<Application> {
    sqlx::query(
        "INSERT INTO applications (id, name) VALUES ($1, $2) ON CONFLICT (name) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(name)
    .execute(pool)
    .await?;
    Ok(
        sqlx::query_as::<_, Application>("SELECT id, name FROM applications WHERE name = $1")
            .bind(name)
            .fetch_one(pool)
            .await?,
    )
}

/// An application and its latest revision number, if it has one.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct ApplicationSummary {
    pub id: Uuid,
    pub name: String,
    pub latest_revision: Option<i32>,
    pub created_at: time::OffsetDateTime,
}

pub async fn list_applications(pool: &PgPool) -> Result<Vec<ApplicationSummary>> {
    Ok(sqlx::query_as::<_, ApplicationSummary>(
        "SELECT a.id, a.name, max(r.revision) AS latest_revision, a.created_at
         FROM applications a LEFT JOIN application_revisions r ON r.application_id = a.id
         GROUP BY a.id ORDER BY a.name",
    )
    .fetch_all(pool)
    .await?)
}

/// Saves a new immutable revision numbered one above the latest. Concurrent saves of the same
/// application are serialised by locking the application row.
pub async fn add_revision(
    pool: &PgPool,
    application_id: Uuid,
    document: &Value,
    created_by: &str,
) -> Result<Revision> {
    Ok(
        add_revision_once(pool, application_id, document, created_by, None)
            .await?
            .0,
    )
}

/// Like [`add_revision`], but a repeated `idempotency_key` returns the revision saved the first
/// time; `true` means the revision was created now.
pub async fn add_revision_once(
    pool: &PgPool,
    application_id: Uuid,
    document: &Value,
    created_by: &str,
    idempotency_key: Option<&str>,
) -> Result<(Revision, bool)> {
    if let Some(key) = idempotency_key
        && let Some(existing) = sqlx::query_as::<_, Revision>(
            "SELECT id, application_id, revision, document, created_by FROM application_revisions
             WHERE idempotency_key = $1",
        )
        .bind(key)
        .fetch_optional(pool)
        .await?
    {
        return Ok((existing, false));
    }
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
        "INSERT INTO application_revisions (id, application_id, revision, document, created_by, idempotency_key)
         VALUES ($1, $2, COALESCE((SELECT max(revision) FROM application_revisions WHERE application_id = $2), 0) + 1, $3, $4, $5)
         RETURNING id, application_id, revision, document, created_by",
    )
    .bind(Uuid::new_v4())
    .bind(application_id)
    .bind(document)
    .bind(created_by)
    .bind(idempotency_key)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok((rev, true))
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

/// What an application should run in a project, as last deployed.
#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct DeploymentRecord {
    pub application_id: Uuid,
    pub application: String,
    pub project: String,
    pub revision: i32,
    pub desired: Value,
    pub connections: Option<Value>,
    pub uplink: Option<String>,
    pub service_range: Option<String>,
    pub published: Option<Value>,
}

/// The desired state of one deployment, as [`record_deployment`] stores it.
#[derive(Debug, Clone, Copy)]
pub struct NewDeployment<'a> {
    pub application_id: Uuid,
    pub project: &'a str,
    pub revision: i32,
    pub desired: &'a Value,
    pub connections: Option<&'a Value>,
    /// Set when connections are enforced.
    pub uplink: Option<&'a str>,
    /// Set when stateless appliances get service addresses.
    pub service_range: Option<&'a str>,
    /// Ports published outside the application.
    pub published: Option<&'a Value>,
}

/// Records the desired state of the latest deployment; an empty `desired` removes the record.
pub async fn record_deployment(pool: &PgPool, d: &NewDeployment<'_>) -> Result<()> {
    let NewDeployment {
        application_id,
        project,
        revision,
        desired,
        connections,
        uplink,
        service_range,
        published,
    } = *d;
    if desired.as_array().is_some_and(Vec::is_empty) {
        sqlx::query(
            "DELETE FROM application_deployments WHERE application_id = $1 AND project = $2",
        )
        .bind(application_id)
        .bind(project)
        .execute(pool)
        .await?;
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO application_deployments (application_id, project, revision, desired, connections, uplink, service_range, published)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (application_id, project) DO UPDATE
         SET revision = $3, desired = $4, connections = $5, uplink = $6, service_range = $7, published = $8, updated_at = now()",
    )
    .bind(application_id)
    .bind(project)
    .bind(revision)
    .bind(desired)
    .bind(connections)
    .bind(uplink)
    .bind(service_range)
    .bind(published)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn list_deployments(pool: &PgPool) -> Result<Vec<DeploymentRecord>> {
    Ok(sqlx::query_as::<_, DeploymentRecord>(
        "SELECT d.application_id, a.name AS application, d.project, d.revision, d.desired, d.connections, d.uplink,
                d.service_range, d.published
         FROM application_deployments d JOIN applications a ON a.id = d.application_id
         ORDER BY a.name, d.project",
    )
    .fetch_all(pool)
    .await?)
}

/// The service address of an appliance: the one it already has, otherwise the first of
/// `candidates` no appliance holds. `None` when every candidate is taken.
pub async fn service_address(
    pool: &PgPool,
    application_id: Uuid,
    project: &str,
    appliance: &str,
    candidates: &[String],
) -> Result<Option<String>> {
    let held = |pool| {
        sqlx::query_scalar::<_, String>(
            "SELECT address FROM service_addresses
             WHERE application_id = $1 AND project = $2 AND appliance = $3",
        )
        .bind(application_id)
        .bind(project)
        .bind(appliance)
        .fetch_optional(pool)
    };
    if let Some(a) = held(pool).await? {
        return Ok(Some(a));
    }
    for c in candidates {
        // Both the address and the appliance are unique: a concurrent allocation for the
        // same appliance makes this insert a no-op, and the held address is returned below.
        let inserted = sqlx::query(
            "INSERT INTO service_addresses (address, application_id, project, appliance)
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        )
        .bind(c)
        .bind(application_id)
        .bind(project)
        .bind(appliance)
        .execute(pool)
        .await?
        .rows_affected();
        if inserted == 1 {
            return Ok(Some(c.clone()));
        }
        if let Some(a) = held(pool).await? {
            return Ok(Some(a));
        }
    }
    Ok(None)
}

/// The application's service addresses in a project, as `(appliance, address)`.
pub async fn service_addresses(
    pool: &PgPool,
    application_id: Uuid,
    project: &str,
) -> Result<Vec<(String, String)>> {
    Ok(sqlx::query_as::<_, (String, String)>(
        "SELECT appliance, address FROM service_addresses
         WHERE application_id = $1 AND project = $2 ORDER BY appliance",
    )
    .bind(application_id)
    .bind(project)
    .fetch_all(pool)
    .await?)
}

/// An application's service addresses in every project, as `(project, appliance, address)`.
pub async fn application_services(
    pool: &PgPool,
    application: &str,
) -> Result<Vec<(String, String, String)>> {
    Ok(sqlx::query_as::<_, (String, String, String)>(
        "SELECT s.project, s.appliance, s.address
         FROM service_addresses s JOIN applications a ON a.id = s.application_id
         WHERE a.name = $1 ORDER BY s.project, s.appliance",
    )
    .bind(application)
    .fetch_all(pool)
    .await?)
}

/// Frees the service addresses of the application's appliances not in `keep`.
pub async fn release_service_addresses(
    pool: &PgPool,
    application_id: Uuid,
    project: &str,
    keep: &[String],
) -> Result<u64> {
    Ok(sqlx::query(
        "DELETE FROM service_addresses
         WHERE application_id = $1 AND project = $2 AND NOT (appliance = ANY($3))",
    )
    .bind(application_id)
    .bind(project)
    .bind(keep)
    .execute(pool)
    .await?
    .rows_affected())
}

/// Whether a job of `kind` for the instance `project/name` is waiting or running.
pub async fn job_in_flight(pool: &PgPool, kind: &str, project: &str, name: &str) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM jobs WHERE kind = $1 AND status IN ('pending', 'running')
                        AND payload->>'project' = $2 AND payload->>'name' = $3)",
    )
    .bind(kind)
    .bind(project)
    .bind(name)
    .fetch_one(pool)
    .await?)
}

/// Whether a job of `kind` for the instance failed within the last `secs` seconds (used to
/// back off instead of re-queuing a change that keeps failing).
pub async fn job_failed_recently(
    pool: &PgPool,
    kind: &str,
    project: &str,
    name: &str,
    secs: i64,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM jobs WHERE kind = $1 AND status = 'failed'
                        AND payload->>'project' = $2 AND payload->>'name' = $3
                        AND updated_at > now() - make_interval(secs => $4))",
    )
    .bind(kind)
    .bind(project)
    .bind(name)
    .bind(secs as f64)
    .fetch_one(pool)
    .await?)
}

/// The kind of the latest lifecycle job (`instance.create|start|stop|delete`) for an instance.
pub async fn last_lifecycle_job(
    pool: &PgPool,
    project: &str,
    name: &str,
) -> Result<Option<String>> {
    Ok(sqlx::query_scalar::<_, String>(
        "SELECT kind FROM jobs
         WHERE kind IN ('instance.create', 'instance.start', 'instance.stop', 'instance.restart', 'instance.delete')
           AND payload->>'project' = $1 AND payload->>'name' = $2
         ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(project)
    .bind(name)
    .fetch_optional(pool)
    .await?)
}

/// The status of the application's latest job of `kind` and how many seconds ago it last
/// changed.
pub async fn latest_application_job(
    pool: &PgPool,
    kind: &str,
    application_id: Uuid,
) -> Result<Option<(String, f64)>> {
    Ok(sqlx::query_as::<_, (String, f64)>(
        "SELECT status, EXTRACT(EPOCH FROM now() - updated_at)::float8 FROM jobs
         WHERE kind = $1 AND application_id = $2 ORDER BY created_at DESC, id DESC LIMIT 1",
    )
    .bind(kind)
    .bind(application_id)
    .fetch_optional(pool)
    .await?)
}

/// Whether a job of `kind` for the application is waiting or running.
pub async fn application_job_in_flight(
    pool: &PgPool,
    kind: &str,
    application_id: Uuid,
) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM jobs WHERE kind = $1 AND status IN ('pending', 'running') AND application_id = $2)",
    )
    .bind(kind)
    .bind(application_id)
    .fetch_one(pool)
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
    /// Incus operation started by an earlier attempt, if any.
    pub external_operation: Option<String>,
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
         RETURNING id, kind, application_id, payload, attempts, max_attempts, generation, external_operation",
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
    /// Marked failed at once: retrying cannot help (e.g. the target is not Apelora's).
    Rejected(String),
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
        Outcome::Rejected(err) => sqlx::query(
            "UPDATE jobs SET status = 'failed', lease_owner = NULL, lease_until = NULL, last_error = $4, updated_at = now()
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

/// Records the Incus operation this attempt started, fenced like every other job write. Call it
/// before waiting on the operation.
pub async fn record_operation(
    pool: &PgPool,
    job: &ClaimedJob,
    worker: &str,
    operation: Option<&str>,
) -> Result<()> {
    let n = sqlx::query(
        "UPDATE jobs SET external_operation = $4, updated_at = now()
         WHERE id = $1 AND lease_owner = $2 AND generation = $3 AND status = 'running'",
    )
    .bind(job.id)
    .bind(worker)
    .bind(job.generation)
    .bind(operation)
    .execute(pool)
    .await?
    .rows_affected();
    fenced(n, job)
}

/// A job as shown to API callers.
#[derive(Debug, Clone, PartialEq, FromRow)]
pub struct JobView {
    pub id: Uuid,
    pub kind: String,
    pub application_id: Option<Uuid>,
    pub status: String,
    pub attempts: i32,
    pub max_attempts: i32,
    pub last_error: Option<String>,
    pub external_operation: Option<String>,
    pub payload: Value,
    pub created_at: time::OffsetDateTime,
    pub updated_at: time::OffsetDateTime,
}

pub async fn get_job(pool: &PgPool, id: Uuid) -> Result<JobView> {
    sqlx::query_as::<_, JobView>(
        "SELECT id, kind, application_id, status, attempts, max_attempts, last_error, external_operation, payload,
                created_at, updated_at
         FROM jobs WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?
    .ok_or_else(|| StoreError::NotFound(format!("job {id}")))
}

/// The most recently created jobs, newest first.
pub async fn recent_jobs(pool: &PgPool, limit: i64) -> Result<Vec<JobView>> {
    Ok(sqlx::query_as::<_, JobView>(
        "SELECT id, kind, application_id, status, attempts, max_attempts, last_error, external_operation, payload,
                created_at, updated_at
         FROM jobs ORDER BY created_at DESC, id LIMIT $1",
    )
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?)
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
mod identity_tests;
#[cfg(test)]
mod tests;
