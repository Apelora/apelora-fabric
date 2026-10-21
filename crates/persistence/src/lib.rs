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

pub mod auth_settings;
pub mod coordination;
pub mod directories;
mod health;
pub mod identity;
pub mod jobs;
pub mod lifecycle;
pub mod local_accounts;
mod machines;
pub mod monitoring;
pub mod outbox;
pub mod personal_tokens;
pub mod status;
pub use health::{HealthRecord, NewHealth, application_health, forget_health, record_health};
mod tokens;
pub mod user_sessions;
mod volumes;
pub use coordination::{
    JOBS_CHANNEL, Leadership, acquire_leadership, begin_fenced, leader_of, listen_for_jobs,
    release_leadership, renew_leadership,
};
pub use machines::{
    MachineRecord, admit_machine, approve_machine, machine, machines, register_machine,
    remove_machine, withdraw_approval,
};
pub use sqlx::postgres::PgListener;
pub use sqlx::{PgConnection, PgExecutor};
pub use tokens::{TokenRecord, authenticate, create_token, list_tokens, revoke_token};
pub use volumes::{
    BackupRecord, DueVolume, NewBackup, ScheduleKind, claim_due, forget_backup, instance_volumes,
    record_backup, record_scheduled, set_volume_member, set_volume_schedules, volume_backups,
};
pub use volumes::{
    NewVolume, VolumeRecord, all_volumes, application_volume, application_volumes, forget_volume,
    record_volume, set_volume_size,
};

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

impl StoreError {
    /// The database cannot be reached or has no connection to spare (it is down, restarting,
    /// refusing connections, or the pool timed out), as opposed to refusing one statement.
    pub fn is_unavailable(&self) -> bool {
        match self {
            StoreError::Database(e) => match e {
                sqlx::Error::Io(_)
                | sqlx::Error::Tls(_)
                | sqlx::Error::PoolTimedOut
                | sqlx::Error::PoolClosed
                | sqlx::Error::WorkerCrashed => true,
                // SQLSTATE class 08 (connection exception), 57P01-57P03 (shutting down or
                // starting up), 53300 (too many connections).
                sqlx::Error::Database(db) => db.code().is_some_and(|c| {
                    c.starts_with("08") || matches!(&*c, "57P01" | "57P02" | "57P03" | "53300")
                }),
                _ => false,
            },
            StoreError::Migration(_) | StoreError::NotFound(_) | StoreError::Conflict(_) => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, StoreError>;

/// A transaction. A change, its audit row and its outbox messages are written in one, so they
/// commit or roll back together (EVERY-SIZE.md §2.5). Functions that take
/// `&mut PgConnection` run inside the caller's transaction (`&mut tx`); the ones named `…_in`
/// are the in-transaction forms of a function that otherwise opens its own.
pub type Tx = sqlx::Transaction<'static, sqlx::Postgres>;

/// Begins a transaction on `pool`.
pub async fn begin(pool: &PgPool) -> Result<Tx> {
    Ok(pool.begin().await?)
}

/// Commits a transaction begun with [`begin`].
pub async fn commit(tx: Tx) -> Result<()> {
    Ok(tx.commit().await?)
}

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
    ensure_application_tx(&mut *pool.acquire().await?, name).await
}

/// [`ensure_application`] inside the caller's transaction (`&mut tx`; named `_tx` because
/// `identity::ensure_application_in` already means "in a project"). It commits or rolls back
/// with the caller's change and audit row; its own transaction becomes a savepoint.
pub async fn ensure_application_tx(
    conn: &mut sqlx::PgConnection,
    name: &str,
) -> Result<Application> {
    sqlx::query(
        "INSERT INTO applications (id, name) VALUES ($1, $2) ON CONFLICT (name) DO NOTHING",
    )
    .bind(Uuid::new_v4())
    .bind(name)
    .execute(&mut *conn)
    .await?;
    Ok(
        sqlx::query_as::<_, Application>("SELECT id, name FROM applications WHERE name = $1")
            .bind(name)
            .fetch_one(&mut *conn)
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
    add_revision_once_in(
        &mut *pool.acquire().await?,
        application_id,
        document,
        created_by,
        idempotency_key,
    )
    .await
}

/// [`add_revision_once`] inside the caller's transaction (`&mut tx`); its own transaction
/// becomes a savepoint.
pub async fn add_revision_once_in(
    conn: &mut sqlx::PgConnection,
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
        .fetch_optional(&mut *conn)
        .await?
    {
        return Ok((existing, false));
    }
    let mut tx = sqlx::Connection::begin(&mut *conn).await?;
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
        "INSERT INTO application_revisions (id, application_id, revision, document, created_by, idempotency_key, published)
         VALUES ($1, $2, COALESCE((SELECT max(revision) FROM application_revisions WHERE application_id = $2), 0) + 1, $3, $4, $5, true)
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
    /// Set when stateless modules get service addresses.
    pub service_range: Option<&'a str>,
    /// Ports published outside the application.
    pub published: Option<&'a Value>,
}

/// Records the desired state of the latest deployment; an empty `desired` removes the record.
pub async fn record_deployment(pool: &PgPool, d: &NewDeployment<'_>) -> Result<()> {
    record_deployment_in(&mut *pool.acquire().await?, d).await
}

/// [`record_deployment`] inside the caller's transaction (`&mut tx`), so it commits or rolls back
/// with the caller's change and audit row; its own transaction becomes a savepoint.
pub async fn record_deployment_in(
    conn: &mut sqlx::PgConnection,
    d: &NewDeployment<'_>,
) -> Result<()> {
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
        .execute(&mut *conn)
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
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Whether the application's recorded deployment in `project` enforces its connections (it
/// has an application network).
pub async fn deployment_enforced(pool: &PgPool, application: &str, project: &str) -> Result<bool> {
    Ok(sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS (SELECT 1 FROM application_deployments d JOIN applications a ON a.id = d.application_id
                        WHERE a.name = $1 AND d.project = $2 AND d.uplink IS NOT NULL)",
    )
    .bind(application)
    .bind(project)
    .fetch_one(pool)
    .await?)
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

/// The service address of a module: the one it already has, otherwise the first of
/// `candidates` no module holds. `None` when every candidate is taken.
pub async fn service_address(
    pool: &PgPool,
    application_id: Uuid,
    project: &str,
    module: &str,
    candidates: &[String],
) -> Result<Option<String>> {
    service_address_in(
        &mut *pool.acquire().await?,
        application_id,
        project,
        module,
        candidates,
    )
    .await
}

/// [`service_address`] inside the caller's transaction (`&mut tx`).
pub async fn service_address_in(
    conn: &mut sqlx::PgConnection,
    application_id: Uuid,
    project: &str,
    module: &str,
    candidates: &[String],
) -> Result<Option<String>> {
    async fn held(
        conn: &mut sqlx::PgConnection,
        application_id: Uuid,
        project: &str,
        module: &str,
    ) -> Result<Option<String>> {
        Ok(sqlx::query_scalar::<_, String>(
            "SELECT address FROM service_addresses
             WHERE application_id = $1 AND project = $2 AND module = $3",
        )
        .bind(application_id)
        .bind(project)
        .bind(module)
        .fetch_optional(conn)
        .await?)
    }
    if let Some(a) = held(&mut *conn, application_id, project, module).await? {
        return Ok(Some(a));
    }
    for c in candidates {
        // Both the address and the module are unique: a concurrent allocation for the
        // same module makes this insert a no-op, and the held address is returned below.
        let inserted = sqlx::query(
            "INSERT INTO service_addresses (address, application_id, project, module)
             VALUES ($1, $2, $3, $4) ON CONFLICT DO NOTHING",
        )
        .bind(c)
        .bind(application_id)
        .bind(project)
        .bind(module)
        .execute(&mut *conn)
        .await?
        .rows_affected();
        if inserted == 1 {
            return Ok(Some(c.clone()));
        }
        if let Some(a) = held(&mut *conn, application_id, project, module).await? {
            return Ok(Some(a));
        }
    }
    Ok(None)
}

/// The application's service addresses in a project, as `(module, address)`.
pub async fn service_addresses(
    pool: &PgPool,
    application_id: Uuid,
    project: &str,
) -> Result<Vec<(String, String)>> {
    Ok(sqlx::query_as::<_, (String, String)>(
        "SELECT module, address FROM service_addresses
         WHERE application_id = $1 AND project = $2 ORDER BY module",
    )
    .bind(application_id)
    .bind(project)
    .fetch_all(pool)
    .await?)
}

/// An application's service addresses in every project, as `(project, module, address)`.
pub async fn application_services(
    pool: &PgPool,
    application: &str,
) -> Result<Vec<(String, String, String)>> {
    Ok(sqlx::query_as::<_, (String, String, String)>(
        "SELECT s.project, s.module, s.address
         FROM service_addresses s JOIN applications a ON a.id = s.application_id
         WHERE a.name = $1 ORDER BY s.project, s.module",
    )
    .bind(application)
    .fetch_all(pool)
    .await?)
}

/// Frees the service addresses of the application's modules not in `keep`.
pub async fn release_service_addresses(
    pool: &PgPool,
    application_id: Uuid,
    project: &str,
    keep: &[String],
) -> Result<u64> {
    Ok(sqlx::query(
        "DELETE FROM service_addresses
         WHERE application_id = $1 AND project = $2 AND NOT (module = ANY($3))",
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
    /// The API request that queued the job (`x-request-id`), if a request did.
    pub request_id: Option<String>,
    /// Someone asked to cancel the job (as of the claim). Handlers stop at their next safe
    /// point; long handlers ask again with [`jobs::cancel_requested`].
    pub cancel_requested: bool,
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

/// Queues a job that no API request asked for (the supervisor, the scheduler, tests). Workers are
/// notified when the insert commits (trigger `jobs_queued`).
pub async fn enqueue_job(
    pool: &PgPool,
    kind: &str,
    application_id: Option<Uuid>,
    payload: &Value,
    idempotency_key: Option<&str>,
) -> Result<Enqueued> {
    enqueue_job_for_request(pool, kind, application_id, payload, idempotency_key, None).await
}

/// Like [`enqueue_job`], recording the API request that asked for it (`x-request-id`), so the
/// job, its log lines and the request's audit rows share one id.
pub async fn enqueue_job_for_request(
    pool: &PgPool,
    kind: &str,
    application_id: Option<Uuid>,
    payload: &Value,
    idempotency_key: Option<&str>,
    request_id: Option<&str>,
) -> Result<Enqueued> {
    let mut conn = pool.acquire().await?;
    enqueue_job_in(
        &mut conn,
        kind,
        application_id,
        payload,
        idempotency_key,
        request_id,
    )
    .await
}

/// Like [`enqueue_job_for_request`], on the caller's connection or inside its transaction
/// (`&mut tx`): the job, and the notification that wakes workers, exist only if it commits.
pub async fn enqueue_job_in(
    conn: &mut sqlx::PgConnection,
    kind: &str,
    application_id: Option<Uuid>,
    payload: &Value,
    idempotency_key: Option<&str>,
    request_id: Option<&str>,
) -> Result<Enqueued> {
    let origin = JobOrigin {
        request_id,
        ..JobOrigin::default()
    };
    enqueue_job_from(
        conn,
        kind,
        application_id,
        payload,
        idempotency_key,
        &origin,
    )
    .await
}

/// Where a job comes from: the API request, the person or service that asked, and the failed
/// job it retries.
#[derive(Debug, Clone, Copy, Default)]
pub struct JobOrigin<'a> {
    /// `x-request-id` of the request that queued it.
    pub request_id: Option<&'a str>,
    /// Who asked (`user:…`, `service:…`, `token:…`); `None` for the supervisor and the scheduler.
    pub queued_by: Option<&'a str>,
    /// The failed job this one retries.
    pub retry_of: Option<Uuid>,
}

/// Queues a job with its origin, on the caller's connection or inside its transaction.
pub async fn enqueue_job_from(
    conn: &mut sqlx::PgConnection,
    kind: &str,
    application_id: Option<Uuid>,
    payload: &Value,
    idempotency_key: Option<&str>,
    origin: &JobOrigin<'_>,
) -> Result<Enqueued> {
    let id = Uuid::new_v4();
    let inserted = sqlx::query(
        "INSERT INTO jobs (id, kind, application_id, payload, idempotency_key, request_id, queued_by, retry_of)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)
         ON CONFLICT (idempotency_key) DO NOTHING RETURNING id",
    )
    .bind(id)
    .bind(kind)
    .bind(application_id)
    .bind(payload)
    .bind(idempotency_key)
    .bind(origin.request_id)
    .bind(origin.queued_by)
    .bind(origin.retry_of)
    .fetch_optional(&mut *conn)
    .await?;
    if inserted.is_some() {
        return Ok(Enqueued::Created(id));
    }
    let existing: Uuid = sqlx::query_scalar("SELECT id FROM jobs WHERE idempotency_key = $1")
        .bind(idempotency_key)
        .fetch_one(&mut *conn)
        .await?;
    Ok(Enqueued::Existing(existing))
}

/// Claims one runnable job: pending and due, or running with an expired lease (its worker is
/// presumed dead). Concurrent workers never claim the same job (`FOR UPDATE SKIP LOCKED`).
/// The attempt is recorded in `job_attempts` in the same statement; an earlier attempt that
/// never finished (its worker stopped renewing the lease) is closed as `lost`.
pub async fn claim_job(pool: &PgPool, worker: &str, lease_secs: i64) -> Result<Option<ClaimedJob>> {
    Ok(sqlx::query_as::<_, ClaimedJob>(
        "WITH claimed AS (
             UPDATE jobs SET status = 'running', lease_owner = $1, lease_until = now() + make_interval(secs => $2),
                    attempts = attempts + 1, generation = generation + 1, updated_at = now()
             WHERE id = (
                 SELECT id FROM jobs
                 WHERE (status = 'pending' AND run_after <= now())
                    OR (status = 'running' AND lease_until < now())
                 ORDER BY run_after, created_at
                 FOR UPDATE SKIP LOCKED
                 LIMIT 1
             )
             RETURNING id, kind, application_id, payload, attempts, max_attempts, generation, external_operation,
                       request_id, cancel_requested_at IS NOT NULL AS cancel_requested
         ),
         lost AS (
             UPDATE job_attempts a SET outcome = 'lost', ended_at = now()
             FROM claimed c
             WHERE a.job_id = c.id AND a.ended_at IS NULL AND a.attempt <> c.attempts
         ),
         attempt AS (
             INSERT INTO job_attempts (job_id, attempt, generation, worker)
             SELECT id, attempts, generation, $1 FROM claimed
             ON CONFLICT (job_id, attempt) DO UPDATE
                 SET generation = EXCLUDED.generation, worker = EXCLUDED.worker,
                     ended_at = NULL, outcome = NULL, error = NULL
         )
         SELECT * FROM claimed",
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
    /// Not finished and not failing: run again after this many seconds without using up an
    /// attempt (a coordinating job waiting for the jobs it queued).
    Waiting(u32),
    /// Stopped on request at a safe point; the text says where (kept on the attempt).
    Cancelled(String),
}

impl Outcome {
    /// The attempt's outcome as recorded in `job_attempts`.
    fn attempt_label(&self) -> &'static str {
        match self {
            Outcome::Succeeded => "succeeded",
            Outcome::Failed(_) => "failed",
            Outcome::Rejected(_) => "rejected",
            Outcome::Waiting(_) => "waiting",
            Outcome::Cancelled(_) => "cancelled",
        }
    }

    fn attempt_error(&self) -> Option<&str> {
        match self {
            Outcome::Failed(e) | Outcome::Rejected(e) | Outcome::Cancelled(e) => Some(e),
            Outcome::Succeeded | Outcome::Waiting(_) => None,
        }
    }
}

/// Records the result. Rejected with `Conflict` when another worker has claimed the job since.
/// The attempt's row is closed in the same statement, fenced the same way.
pub async fn complete_job(
    pool: &PgPool,
    job: &ClaimedJob,
    worker: &str,
    outcome: &Outcome,
) -> Result<()> {
    let update = match outcome {
        Outcome::Succeeded => {
            "UPDATE jobs SET status = 'succeeded', lease_owner = NULL, lease_until = NULL, last_error = NULL, updated_at = now()"
        }
        Outcome::Failed(_) => {
            "UPDATE jobs SET
                status = CASE WHEN attempts >= max_attempts THEN 'failed' ELSE 'pending' END,
                run_after = now() + make_interval(secs => least(300, 2 ^ attempts)::double precision),
                lease_owner = NULL, lease_until = NULL, last_error = $4, updated_at = now()"
        }
        Outcome::Waiting(_) => {
            "UPDATE jobs SET status = 'pending', attempts = greatest(attempts - 1, 0),
                run_after = now() + make_interval(secs => $5::double precision),
                lease_owner = NULL, lease_until = NULL, updated_at = now()"
        }
        Outcome::Rejected(_) => {
            "UPDATE jobs SET status = 'failed', lease_owner = NULL, lease_until = NULL, last_error = $4, updated_at = now()"
        }
        // An earlier attempt's error stays: it is history. Why it stopped is on the attempt.
        Outcome::Cancelled(_) => {
            "UPDATE jobs SET status = 'cancelled', lease_owner = NULL, lease_until = NULL, updated_at = now()"
        }
    };
    let waiting_secs = match outcome {
        Outcome::Waiting(secs) => f64::from(*secs),
        _ => 0.0,
    };
    let n: i64 = sqlx::query_scalar(&format!(
        "WITH done AS (
             {update}
             WHERE id = $1 AND lease_owner = $2 AND generation = $3 AND status = 'running'
             RETURNING id
         ),
         attempt AS (
             UPDATE job_attempts SET ended_at = now(), outcome = $6, error = $4
             WHERE job_id IN (SELECT id FROM done) AND generation = $3
         )
         SELECT count(*) FROM done"
    ))
    .bind(job.id)
    .bind(worker)
    .bind(job.generation)
    .bind(outcome.attempt_error())
    .bind(waiting_secs)
    .bind(outcome.attempt_label())
    .fetch_one(pool)
    .await?;
    fenced(u64::try_from(n).unwrap_or(0), job)
}

/// Records the Incus operation this attempt started, fenced like every other job write. Call it
/// before waiting on the operation; `None` clears the in-flight marker once the wait ended. A
/// started operation is also appended to the job's history (`incus_operations`), which is kept.
pub async fn record_operation(
    pool: &PgPool,
    job: &ClaimedJob,
    worker: &str,
    operation: Option<&str>,
) -> Result<()> {
    let n: i64 = sqlx::query_scalar(
        "WITH done AS (
             UPDATE jobs SET external_operation = $4, updated_at = now(),
                    incus_operations = CASE WHEN $4::text IS NULL OR $4 = ANY(incus_operations)
                                            THEN incus_operations ELSE array_append(incus_operations, $4) END
             WHERE id = $1 AND lease_owner = $2 AND generation = $3 AND status = 'running'
             RETURNING id
         ),
         attempt AS (
             UPDATE job_attempts SET incus_operations = array_append(incus_operations, $4)
             WHERE job_id IN (SELECT id FROM done) AND generation = $3
               AND $4::text IS NOT NULL AND NOT ($4 = ANY(incus_operations))
         )
         SELECT count(*) FROM done",
    )
    .bind(job.id)
    .bind(worker)
    .bind(job.generation)
    .bind(operation)
    .fetch_one(pool)
    .await?;
    fenced(u64::try_from(n).unwrap_or(0), job)
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
    /// The API request that queued the job, if one did.
    pub request_id: Option<String>,
    /// Every Incus operation the job started, oldest first.
    pub incus_operations: Vec<String>,
    pub payload: Value,
    pub created_at: time::OffsetDateTime,
    pub updated_at: time::OffsetDateTime,
    /// Who asked for it; `None` for the supervisor and the scheduler.
    pub queued_by: Option<String>,
    /// The failed job this one retries.
    pub retry_of: Option<Uuid>,
    /// The job that retries this one, if any.
    pub retried_by: Option<Uuid>,
    pub cancel_requested_at: Option<time::OffsetDateTime>,
    pub cancel_requested_by: Option<String>,
}

/// The columns of [`JobView`], for `FROM jobs j`.
pub(crate) const JOB_COLS: &str = "j.id, j.kind, j.application_id, j.status, j.attempts, j.max_attempts, j.last_error,
    j.external_operation, j.request_id, j.incus_operations, j.payload, j.created_at, j.updated_at, j.queued_by,
    j.retry_of, (SELECT r.id FROM jobs r WHERE r.retry_of = j.id) AS retried_by, j.cancel_requested_at,
    j.cancel_requested_by";

pub async fn get_job(pool: &PgPool, id: Uuid) -> Result<JobView> {
    sqlx::query_as::<_, JobView>(&format!("SELECT {JOB_COLS} FROM jobs j WHERE j.id = $1"))
        .bind(id)
        .fetch_optional(pool)
        .await?
        .ok_or_else(|| StoreError::NotFound(format!("job {id}")))
}

/// The most recently created jobs, newest first.
pub async fn recent_jobs(pool: &PgPool, limit: i64) -> Result<Vec<JobView>> {
    Ok(sqlx::query_as::<_, JobView>(&format!(
        "SELECT {JOB_COLS} FROM jobs j ORDER BY j.created_at DESC, j.id LIMIT $1"
    ))
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

/// Records a successful audit event of Apelora itself (`supervisor`, `scheduler`, `cli`, …) with
/// its outbox message; takes a pool or, for a change, the change's transaction (`&mut *tx`).
pub async fn audit<'e>(
    db: impl sqlx::PgExecutor<'e>,
    actor: &str,
    action: &str,
    target: &str,
    detail: &Value,
) -> Result<()> {
    identity::audit_event(
        db,
        &identity::AuditEntry {
            actor,
            actor_type: if actor == "cli" { "cli" } else { "system" },
            actor_name: None,
            action,
            target,
            outcome: "success",
            scope: None,
            detail,
        },
    )
    .await
}

#[cfg(test)]
mod identity_tests;
#[cfg(test)]
mod tests;
