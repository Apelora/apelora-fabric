// SPDX-License-Identifier: AGPL-3.0-only
//! Application metadata, drafts, published versions and lifecycle operations.

use apelora_auth::{AccessFilter, EnvKind};
use serde::Serialize;
use serde_json::Value;
use sqlx::{FromRow, PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::identity::{AppContext, application_context};
use crate::{Result, StoreError};

fn arrays(f: &AccessFilter) -> (bool, Vec<Uuid>, Vec<Uuid>, Vec<Uuid>) {
    match f {
        AccessFilter::All => (true, vec![], vec![], vec![]),
        AccessFilter::Limited {
            organizations,
            projects,
            applications,
        } => (
            false,
            organizations.clone(),
            projects.clone(),
            applications.clone(),
        ),
    }
}

// ---- metadata -------------------------------------------------------------------------------

/// Creates an application in a project and environment. The name is unique across the platform
/// and cannot change later (instance names are derived from it).
pub async fn create_application_full(
    pool: &PgPool,
    name: &str,
    project_id: Uuid,
    environment_id: Option<Uuid>,
    description: &str,
    icon: &str,
    created_by: &str,
) -> Result<crate::Application> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO applications (id, name, project_id, environment_id, description, icon, created_by) VALUES ($1,$2,$3,$4,$5,$6,$7)")
        .bind(id)
        .bind(name)
        .bind(project_id)
        .bind(environment_id)
        .bind(description)
        .bind(icon)
        .bind(created_by)
        .execute(pool)
        .await
        .map_err(|e| match &e {
            sqlx::Error::Database(d) if d.is_unique_violation() => StoreError::Conflict(format!("an application named {name} already exists")),
            _ => StoreError::Database(e),
        })?;
    Ok(crate::Application {
        id,
        name: name.to_owned(),
    })
}

pub async fn update_application_meta(
    pool: &PgPool,
    id: Uuid,
    description: Option<&str>,
    icon: Option<&str>,
) -> Result<()> {
    let n = sqlx::query("UPDATE applications SET description = COALESCE($2, description), icon = COALESCE($3, icon), updated_at = now() WHERE id = $1")
        .bind(id)
        .bind(description)
        .bind(icon)
        .execute(pool)
        .await?
        .rows_affected();
    if n == 0 {
        Err(StoreError::NotFound(format!("application {id}")))
    } else {
        Ok(())
    }
}

pub async fn set_archived(pool: &PgPool, id: Uuid, archived: bool) -> Result<()> {
    sqlx::query("UPDATE applications SET archived_at = CASE WHEN $2 THEN now() ELSE NULL END, updated_at = now() WHERE id = $1").bind(id).bind(archived).execute(pool).await?;
    Ok(())
}

/// What the application list and the overview page show, before runtime status is added.
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct ApplicationRow {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub icon: String,
    pub project_id: Uuid,
    pub project_slug: String,
    pub project_name: String,
    pub organization_id: Uuid,
    pub organization_slug: String,
    pub environment_id: Option<Uuid>,
    pub environment_slug: Option<String>,
    pub environment_name: Option<String>,
    pub environment_kind: Option<String>,
    pub incus_project: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
    pub archived: bool,
    pub created_by: Option<String>,
    /// Highest revision of any kind.
    pub latest_revision: Option<i32>,
    /// Highest published revision.
    pub published_revision: Option<i32>,
    /// The draft version that revision was published from.
    pub published_from_draft: Option<i32>,
    pub draft_version: Option<i32>,
    pub draft_errors: Option<i32>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub draft_updated_at: Option<OffsetDateTime>,
}

const ROW_SELECT: &str = "SELECT a.id, a.name, a.description, a.icon, a.project_id, p.slug AS project_slug, p.name AS project_name, p.organization_id, o.slug AS organization_slug,
        a.environment_id, e.slug AS environment_slug, e.name AS environment_name, e.kind AS environment_kind, e.incus_project,
        a.created_at, a.updated_at, (a.archived_at IS NOT NULL) AS archived, a.created_by,
        (SELECT max(revision) FROM application_revisions r WHERE r.application_id = a.id) AS latest_revision,
        (SELECT max(revision) FROM application_revisions r WHERE r.application_id = a.id AND r.published) AS published_revision,
        (SELECT draft_version FROM application_revisions r WHERE r.application_id = a.id AND r.published ORDER BY revision DESC LIMIT 1) AS published_from_draft,
        d.version AS draft_version, jsonb_array_length(d.validation_errors)::int AS draft_errors, d.updated_at AS draft_updated_at
     FROM applications a JOIN projects p ON p.id = a.project_id JOIN organizations o ON o.id = p.organization_id
     LEFT JOIN environments e ON e.id = a.environment_id LEFT JOIN application_drafts d ON d.application_id = a.id";

/// Applications the filter reaches. Archived ones only when asked.
pub async fn list_application_rows(
    pool: &PgPool,
    f: &AccessFilter,
    include_archived: bool,
) -> Result<Vec<ApplicationRow>> {
    let (all, orgs, projects, apps) = arrays(f);
    Ok(sqlx::query_as(&format!(
        "{ROW_SELECT} WHERE ($1 OR p.organization_id = ANY($2) OR a.project_id = ANY($3) OR a.id = ANY($4)) AND ($5 OR a.archived_at IS NULL) ORDER BY a.name"
    ))
    .bind(all)
    .bind(orgs)
    .bind(projects)
    .bind(apps)
    .bind(include_archived)
    .fetch_all(pool)
    .await?)
}

pub async fn application_row(pool: &PgPool, name: &str) -> Result<Option<ApplicationRow>> {
    Ok(sqlx::query_as(&format!("{ROW_SELECT} WHERE a.name = $1"))
        .bind(name)
        .fetch_optional(pool)
        .await?)
}

// ---- drafts and versions --------------------------------------------------------------------

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DraftRow {
    pub application_id: Uuid,
    pub document: Value,
    pub version: i32,
    pub validation_errors: Value,
    pub updated_by: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

pub async fn get_draft(pool: &PgPool, application_id: Uuid) -> Result<Option<DraftRow>> {
    Ok(sqlx::query_as("SELECT application_id, document, version, validation_errors, updated_by, updated_at FROM application_drafts WHERE application_id = $1").bind(application_id).fetch_optional(pool).await?)
}

/// Saves a new draft version. `base` is the version the caller edited (`None` only for the very
/// first save). Returns `Err(current)` when someone else saved in between, changing nothing.
pub async fn put_draft(
    pool: &PgPool,
    application_id: Uuid,
    document: &Value,
    base: Option<i32>,
    errors: &Value,
    by: &str,
) -> Result<std::result::Result<DraftRow, i32>> {
    let mut tx = pool.begin().await?;
    // Serialise saves of one application by locking its row.
    sqlx::query("SELECT id FROM applications WHERE id = $1 FOR UPDATE")
        .bind(application_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StoreError::NotFound(format!("application {application_id}")))?;
    let current: Option<i32> =
        sqlx::query_scalar("SELECT version FROM application_drafts WHERE application_id = $1")
            .bind(application_id)
            .fetch_optional(&mut *tx)
            .await?;
    if current != base {
        return Ok(Err(current.unwrap_or(0)));
    }
    let next = current.unwrap_or(0) + 1;
    let row = sqlx::query_as::<_, DraftRow>(
        "INSERT INTO application_drafts (application_id, document, version, validation_errors, updated_by) VALUES ($1,$2,$3,$4,$5)
         ON CONFLICT (application_id) DO UPDATE SET document = $2, version = $3, validation_errors = $4, updated_by = $5, updated_at = now()
         RETURNING application_id, document, version, validation_errors, updated_by, updated_at",
    )
    .bind(application_id)
    .bind(document)
    .bind(next)
    .bind(errors)
    .bind(by)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query("UPDATE applications SET updated_at = now() WHERE id = $1")
        .bind(application_id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(Ok(row))
}

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct VersionRow {
    pub revision: i32,
    pub published: bool,
    pub label: Option<String>,
    pub draft_version: Option<i32>,
    pub created_by: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

pub async fn list_versions(pool: &PgPool, application_id: Uuid) -> Result<Vec<VersionRow>> {
    Ok(sqlx::query_as("SELECT revision, published, label, draft_version, created_by, created_at FROM application_revisions WHERE application_id = $1 ORDER BY revision DESC").bind(application_id).fetch_all(pool).await?)
}

pub async fn version_document(
    pool: &PgPool,
    application_id: Uuid,
    revision: i32,
) -> Result<Option<(Value, bool)>> {
    let row = sqlx::query("SELECT document, published FROM application_revisions WHERE application_id = $1 AND revision = $2").bind(application_id).bind(revision).fetch_optional(pool).await?;
    Ok(row.map(|r| (r.get("document"), r.get("published"))))
}

/// Releases the current draft as the next immutable, published version. Publishing the same
/// draft version twice is refused, so a version number always means one distinct design.
pub async fn publish_draft(
    pool: &PgPool,
    application_id: Uuid,
    label: Option<&str>,
    by: &str,
) -> Result<std::result::Result<VersionRow, PublishRefusal>> {
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT id FROM applications WHERE id = $1 FOR UPDATE")
        .bind(application_id)
        .fetch_optional(&mut *tx)
        .await?
        .ok_or_else(|| StoreError::NotFound(format!("application {application_id}")))?;
    let Some(draft) = sqlx::query_as::<_, DraftRow>("SELECT application_id, document, version, validation_errors, updated_by, updated_at FROM application_drafts WHERE application_id = $1").bind(application_id).fetch_optional(&mut *tx).await? else {
        return Ok(Err(PublishRefusal::NoDraft));
    };
    if draft
        .validation_errors
        .as_array()
        .is_some_and(|e| !e.is_empty())
    {
        return Ok(Err(PublishRefusal::Invalid(draft.validation_errors)));
    }
    let already: Option<i32> = sqlx::query_scalar("SELECT revision FROM application_revisions WHERE application_id = $1 AND published AND draft_version = $2").bind(application_id).bind(draft.version).fetch_optional(&mut *tx).await?;
    if let Some(r) = already {
        return Ok(Err(PublishRefusal::AlreadyPublished(r)));
    }
    let next: i32 = sqlx::query_scalar("SELECT COALESCE(max(revision), 0) + 1 FROM application_revisions WHERE application_id = $1").bind(application_id).fetch_one(&mut *tx).await?;
    let row = sqlx::query_as::<_, VersionRow>(
        "INSERT INTO application_revisions (id, application_id, revision, document, created_by, published, label, draft_version) VALUES ($1,$2,$3,$4,$5,true,$6,$7)
         RETURNING revision, published, label, draft_version, created_by, created_at",
    )
    .bind(Uuid::new_v4())
    .bind(application_id)
    .bind(next)
    .bind(&draft.document)
    .bind(by)
    .bind(label)
    .bind(draft.version)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Ok(row))
}

#[derive(Debug, Clone, PartialEq)]
pub enum PublishRefusal {
    NoDraft,
    Invalid(Value),
    AlreadyPublished(i32),
}

// ---- operations -----------------------------------------------------------------------------

#[derive(Debug, Clone, FromRow, Serialize)]
pub struct OperationRow {
    pub id: Uuid,
    pub application_id: Uuid,
    pub project: String,
    pub kind: String,
    pub state: String,
    pub requested_by: String,
    pub confirmed_disruption: bool,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub started_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub finished_at: Option<OffsetDateTime>,
    pub phases: Value,
    pub error: Option<String>,
}

const OP_COLS: &str = "id, application_id, project, kind, state, requested_by, confirmed_disruption, created_at, started_at, finished_at, phases, error";

/// A requested operation: what to do to which application, in which phases, and by whom.
#[derive(Debug, Clone, Copy)]
pub struct NewOperation<'a> {
    pub application_id: Uuid,
    pub project: &'a str,
    pub kind: &'a str,
    pub phases: &'a Value,
    pub requested_by: &'a str,
    pub confirmed: bool,
    pub key: Option<&'a str>,
    /// The API request that asked for it; stored on the coordinating job (and inherited by the
    /// jobs it queues).
    pub request_id: Option<&'a str>,
}

/// Creates a pending operation and queues the job that carries it out, in one transaction.
/// `Conflict` when another operation of the application is still active, or when the same
/// idempotency key was used for a different request.
pub async fn create_operation(pool: &PgPool, n: &NewOperation<'_>) -> Result<(OperationRow, bool)> {
    let NewOperation {
        application_id,
        project,
        kind,
        phases,
        requested_by,
        confirmed,
        key,
        request_id,
    } = *n;
    let mut tx = pool.begin().await?;
    if let Some(k) = key
        && let Some(existing) = sqlx::query_as::<_, OperationRow>(&format!(
            "SELECT {OP_COLS} FROM application_operations WHERE idempotency_key = $1"
        ))
        .bind(k)
        .fetch_optional(&mut *tx)
        .await?
    {
        return if existing.application_id == application_id && existing.kind == kind {
            Ok((existing, false))
        } else {
            Err(StoreError::Conflict(
                "that Idempotency-Key was used for a different request".into(),
            ))
        };
    }
    let id = Uuid::new_v4();
    let row = sqlx::query_as::<_, OperationRow>(&format!(
        "INSERT INTO application_operations (id, application_id, project, kind, phases, requested_by, confirmed_disruption, idempotency_key) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING {OP_COLS}"
    ))
    .bind(id)
    .bind(application_id)
    .bind(project)
    .bind(kind)
    .bind(phases)
    .bind(requested_by)
    .bind(confirmed)
    .bind(key)
    .fetch_one(&mut *tx)
    .await
    .map_err(|e| match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => StoreError::Conflict("another operation on this application is still in progress".into()),
        _ => StoreError::Database(e),
    })?;
    crate::enqueue_job_from(
        &mut tx,
        "application.operation",
        Some(application_id),
        &serde_json::json!({ "operation": id }),
        Some(&format!("application.operation:{id}")),
        &crate::JobOrigin {
            request_id,
            queued_by: Some(requested_by),
            retry_of: None,
        },
    )
    .await?;
    tx.commit().await?;
    Ok((row, true))
}

pub async fn get_operation(pool: &PgPool, id: Uuid) -> Result<Option<OperationRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {OP_COLS} FROM application_operations WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

pub async fn list_operations(
    pool: &PgPool,
    application_id: Uuid,
    limit: i64,
) -> Result<Vec<OperationRow>> {
    Ok(sqlx::query_as(&format!("SELECT {OP_COLS} FROM application_operations WHERE application_id = $1 ORDER BY created_at DESC LIMIT $2")).bind(application_id).bind(limit.clamp(1, 100)).fetch_all(pool).await?)
}

pub async fn latest_operations_by_application(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<Vec<OperationRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT DISTINCT ON (application_id) {OP_COLS} FROM application_operations WHERE application_id = ANY($1) ORDER BY application_id, created_at DESC"
    ))
    .bind(ids.to_vec())
    .fetch_all(pool)
    .await?)
}

/// Records progress. Phases are rewritten whole by the worker, which is the only writer.
pub async fn update_operation(
    pool: &PgPool,
    id: Uuid,
    state: &str,
    phases: &Value,
    error: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "UPDATE application_operations SET state = $2, phases = $3, error = $4,
            started_at = CASE WHEN started_at IS NULL AND $2 <> 'pending' THEN now() ELSE started_at END,
            finished_at = CASE WHEN $2 IN ('succeeded', 'failed', 'cancelled') THEN now() ELSE NULL END
         WHERE id = $1",
    )
    .bind(id)
    .bind(state)
    .bind(phases)
    .bind(error)
    .execute(pool)
    .await?;
    Ok(())
}

/// The deployment record of an application (there is one per Incus project it is deployed in).
pub async fn deployments_of(
    pool: &PgPool,
    application_id: Uuid,
) -> Result<Vec<crate::DeploymentRecord>> {
    Ok(sqlx::query_as::<_, crate::DeploymentRecord>(
        "SELECT d.application_id, a.name AS application, d.project, d.revision, d.desired, d.connections, d.uplink, d.service_range, d.published
         FROM application_deployments d JOIN applications a ON a.id = d.application_id WHERE d.application_id = $1 ORDER BY d.project",
    )
    .bind(application_id)
    .fetch_all(pool)
    .await?)
}

pub async fn latest_deployed_at(
    pool: &PgPool,
    ids: &[Uuid],
) -> Result<Vec<(Uuid, OffsetDateTime)>> {
    let rows = sqlx::query("SELECT application_id, max(updated_at) AS at FROM application_deployments WHERE application_id = ANY($1) GROUP BY application_id").bind(ids.to_vec()).fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|r| (r.get("application_id"), r.get("at")))
        .collect())
}

/// Audit events about one application (newest first): the events carry its id in their scope.
pub async fn application_events(
    pool: &PgPool,
    application_id: Uuid,
    limit: i64,
) -> Result<Vec<crate::identity::AuditRow>> {
    Ok(sqlx::query_as(
        "SELECT id, at, actor, actor_type, action, target, outcome, scope, detail FROM audit_events
         WHERE (scope->>'application')::uuid = $1 ORDER BY id DESC LIMIT $2",
    )
    .bind(application_id)
    .bind(limit.clamp(1, 200))
    .fetch_all(pool)
    .await?)
}

/// The `Resource` for an application by name, plus the row, in one place for handlers.
pub async fn resolve(pool: &PgPool, name: &str) -> Result<Option<AppContext>> {
    application_context(pool, name).await
}

pub fn env_kind(s: &Option<String>) -> Option<EnvKind> {
    s.as_deref().and_then(EnvKind::parse)
}
