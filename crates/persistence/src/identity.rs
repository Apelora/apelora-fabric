// SPDX-License-Identifier: AGPL-3.0-only
//! Organisations, projects, environments, people, groups, roles, assignments, service
//! identities, access policies, approvals and the audit trail.
//!
//! Every list function takes an [`AccessFilter`] computed by the authorisation engine and
//! applies it in SQL, so a caller can never read rows outside the scopes they hold. Writes that
//! change who may do what also take the acting principal's name and are audited by the caller.

use std::collections::{BTreeSet, HashMap};

use apelora_auth::{
    AccessFilter, Approval, ApprovalState, Assignment, AssignmentKind, Conditions, CustomRole,
    Engine, EnvKind, Policies, PrincipalRef, Resource, RoleRef, Scope,
};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{FromRow, PgPool, Row};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::{Result, StoreError};

pub const DEFAULT_ORGANIZATION: Uuid = Uuid::from_u128(1);
pub const DEFAULT_PROJECT: Uuid = Uuid::from_u128(2);
pub const DEFAULT_ENVIRONMENT: Uuid = Uuid::from_u128(3);

/// `(organisations, projects, applications)` ids as SQL arrays.
fn arrays(f: &AccessFilter) -> (Vec<Uuid>, Vec<Uuid>, Vec<Uuid>) {
    match f {
        AccessFilter::All => (vec![], vec![], vec![]),
        AccessFilter::Limited {
            organizations,
            projects,
            applications,
        } => (
            organizations.clone(),
            projects.clone(),
            applications.clone(),
        ),
    }
}
fn all(f: &AccessFilter) -> bool {
    matches!(f, AccessFilter::All)
}

fn db_conflict(e: sqlx::Error, what: &str) -> StoreError {
    match &e {
        sqlx::Error::Database(d) if d.is_unique_violation() => {
            StoreError::Conflict(format!("{what} already exists"))
        }
        sqlx::Error::Database(d)
            if d.is_foreign_key_violation() || d.code().as_deref() == Some("23001") =>
        {
            StoreError::Conflict(format!(
                "{what} is still in use or refers to something that does not exist"
            ))
        }
        _ => StoreError::Database(e),
    }
}

// ---- organisations, projects, environments --------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
pub struct OrganizationRow {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub disabled_at: Option<OffsetDateTime>,
}

pub async fn create_organization(pool: &PgPool, slug: &str, name: &str) -> Result<OrganizationRow> {
    sqlx::query_as("INSERT INTO organizations (id, slug, name) VALUES ($1, $2, $3) RETURNING id, slug, name, created_at, disabled_at")
        .bind(Uuid::new_v4())
        .bind(slug)
        .bind(name)
        .fetch_one(pool)
        .await
        .map_err(|e| db_conflict(e, "an organisation with that identifier"))
}

pub async fn organization(pool: &PgPool, id: Uuid) -> Result<Option<OrganizationRow>> {
    Ok(sqlx::query_as(
        "SELECT id, slug, name, created_at, disabled_at FROM organizations WHERE id = $1",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

/// Organisations the filter reaches: those named directly, and those that contain a reachable project.
pub async fn list_organizations(pool: &PgPool, f: &AccessFilter) -> Result<Vec<OrganizationRow>> {
    let (orgs, projects, _) = arrays(f);
    Ok(sqlx::query_as(
        "SELECT o.id, o.slug, o.name, o.created_at, o.disabled_at FROM organizations o
         WHERE $1 OR o.id = ANY($2) OR EXISTS (SELECT 1 FROM projects p WHERE p.organization_id = o.id AND p.id = ANY($3))
         ORDER BY o.slug",
    )
    .bind(all(f))
    .bind(orgs)
    .bind(projects)
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
pub struct ProjectRow {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub organization_slug: String,
    pub slug: String,
    pub name: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

const PROJECT_COLS: &str =
    "p.id, p.organization_id, o.slug AS organization_slug, p.slug, p.name, p.created_at";

pub async fn create_project(
    pool: &PgPool,
    organization_id: Uuid,
    slug: &str,
    name: &str,
) -> Result<ProjectRow> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO projects (id, organization_id, slug, name) VALUES ($1, $2, $3, $4)")
        .bind(id)
        .bind(organization_id)
        .bind(slug)
        .bind(name)
        .execute(pool)
        .await
        .map_err(|e| db_conflict(e, "a project with that identifier"))?;
    project(pool, id)
        .await?
        .ok_or_else(|| StoreError::NotFound(format!("project {id}")))
}

pub async fn project(pool: &PgPool, id: Uuid) -> Result<Option<ProjectRow>> {
    Ok(sqlx::query_as(&format!("SELECT {PROJECT_COLS} FROM projects p JOIN organizations o ON o.id = p.organization_id WHERE p.id = $1")).bind(id).fetch_optional(pool).await?)
}

pub async fn list_projects(pool: &PgPool, f: &AccessFilter) -> Result<Vec<ProjectRow>> {
    let (orgs, projects, apps) = arrays(f);
    Ok(sqlx::query_as(&format!(
        "SELECT {PROJECT_COLS} FROM projects p JOIN organizations o ON o.id = p.organization_id
         WHERE $1 OR p.organization_id = ANY($2) OR p.id = ANY($3)
            OR EXISTS (SELECT 1 FROM applications a WHERE a.project_id = p.id AND a.id = ANY($4))
         ORDER BY o.slug, p.slug"
    ))
    .bind(all(f))
    .bind(orgs)
    .bind(projects)
    .bind(apps)
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
pub struct EnvironmentRow {
    pub id: Uuid,
    pub project_id: Uuid,
    pub slug: String,
    pub name: String,
    pub kind: String,
    pub incus_project: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

pub async fn create_environment(
    pool: &PgPool,
    project_id: Uuid,
    slug: &str,
    name: &str,
    kind: EnvKind,
    incus_project: Option<&str>,
) -> Result<EnvironmentRow> {
    sqlx::query_as("INSERT INTO environments (id, project_id, slug, name, kind, incus_project) VALUES ($1,$2,$3,$4,$5,$6) RETURNING id, project_id, slug, name, kind, incus_project, created_at")
        .bind(Uuid::new_v4())
        .bind(project_id)
        .bind(slug)
        .bind(name)
        .bind(kind.as_str())
        .bind(incus_project)
        .fetch_one(pool)
        .await
        .map_err(|e| db_conflict(e, "an environment with that identifier or Incus project"))
}

pub async fn list_environments(pool: &PgPool, project_id: Uuid) -> Result<Vec<EnvironmentRow>> {
    Ok(sqlx::query_as("SELECT id, project_id, slug, name, kind, incus_project, created_at FROM environments WHERE project_id = $1 ORDER BY slug").bind(project_id).fetch_all(pool).await?)
}

/// The environment that owns an Incus project, with the ancestry needed to authorise against it.
pub async fn environment_by_incus_project(
    pool: &PgPool,
    incus_project: &str,
) -> Result<Option<(EnvironmentRow, Uuid)>> {
    let row = sqlx::query("SELECT e.id, e.project_id, e.slug, e.name, e.kind, e.incus_project, e.created_at, p.organization_id FROM environments e JOIN projects p ON p.id = e.project_id WHERE e.incus_project = $1")
        .bind(incus_project)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| {
        (
            EnvironmentRow {
                id: r.get("id"),
                project_id: r.get("project_id"),
                slug: r.get("slug"),
                name: r.get("name"),
                kind: r.get("kind"),
                incus_project: r.get("incus_project"),
                created_at: r.get("created_at"),
            },
            r.get("organization_id"),
        )
    }))
}

/// Everything needed to authorise a request about an application, resolved from the database.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppContext {
    pub application_id: Uuid,
    pub name: String,
    pub organization_id: Uuid,
    pub project_id: Uuid,
    pub environment_id: Option<Uuid>,
    pub environment_kind: Option<EnvKind>,
    pub incus_project: Option<String>,
}

impl AppContext {
    pub fn resource(&self) -> Resource {
        Resource::application(
            self.organization_id,
            self.project_id,
            self.application_id,
            self.environment_kind,
        )
    }
}

pub async fn application_context(pool: &PgPool, name: &str) -> Result<Option<AppContext>> {
    let row = sqlx::query(
        "SELECT a.id, a.name, p.organization_id, a.project_id, a.environment_id, e.kind, e.incus_project
         FROM applications a JOIN projects p ON p.id = a.project_id LEFT JOIN environments e ON e.id = a.environment_id WHERE a.name = $1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| AppContext {
        application_id: r.get("id"),
        name: r.get("name"),
        organization_id: r.get("organization_id"),
        project_id: r.get("project_id"),
        environment_id: r.get("environment_id"),
        environment_kind: r
            .get::<Option<String>, _>("kind")
            .and_then(|k| EnvKind::parse(&k)),
        incus_project: r.get("incus_project"),
    }))
}

pub async fn application_context_by_id(pool: &PgPool, id: Uuid) -> Result<Option<AppContext>> {
    let name: Option<String> = sqlx::query_scalar("SELECT name FROM applications WHERE id = $1")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    match name {
        Some(n) => application_context(pool, &n).await,
        None => Ok(None),
    }
}

/// Recent jobs of the applications the filter reaches (newest first).
pub async fn recent_application_jobs(
    pool: &PgPool,
    f: &AccessFilter,
    limit: i64,
) -> Result<Vec<crate::JobView>> {
    let (orgs, projects, apps) = arrays(f);
    Ok(sqlx::query_as(
        "SELECT j.id, j.kind, j.application_id, j.status, j.attempts, j.max_attempts, j.last_error, j.external_operation, j.payload, j.created_at, j.updated_at
         FROM jobs j JOIN applications a ON a.id = j.application_id JOIN projects p ON p.id = a.project_id
         WHERE $1 OR p.organization_id = ANY($2) OR a.project_id = ANY($3) OR a.id = ANY($4)
         ORDER BY j.created_at DESC, j.id LIMIT $5",
    )
    .bind(all(f))
    .bind(orgs)
    .bind(projects)
    .bind(apps)
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?)
}

/// Creates the application inside a project and environment, or returns the existing one if it
/// is already there. An application with that name in another project is a conflict.
pub async fn ensure_application_in(
    pool: &PgPool,
    name: &str,
    project_id: Uuid,
    environment_id: Option<Uuid>,
) -> Result<crate::Application> {
    sqlx::query("INSERT INTO applications (id, name, project_id, environment_id) VALUES ($1, $2, $3, $4) ON CONFLICT (name) DO NOTHING")
        .bind(Uuid::new_v4())
        .bind(name)
        .bind(project_id)
        .bind(environment_id)
        .execute(pool)
        .await?;
    let row = sqlx::query("SELECT id, name, project_id FROM applications WHERE name = $1")
        .bind(name)
        .fetch_one(pool)
        .await?;
    if row.get::<Uuid, _>("project_id") != project_id {
        return Err(StoreError::Conflict(format!(
            "an application named {name} already exists"
        )));
    }
    Ok(crate::Application {
        id: row.get("id"),
        name: row.get("name"),
    })
}

/// Applications the filter reaches, with their latest revision.
pub async fn list_applications_scoped(
    pool: &PgPool,
    f: &AccessFilter,
) -> Result<Vec<crate::ApplicationSummary>> {
    let (orgs, projects, apps) = arrays(f);
    Ok(sqlx::query_as(
        "SELECT a.id, a.name, max(r.revision) AS latest_revision, a.created_at
         FROM applications a JOIN projects p ON p.id = a.project_id LEFT JOIN application_revisions r ON r.application_id = a.id
         WHERE $1 OR p.organization_id = ANY($2) OR a.project_id = ANY($3) OR a.id = ANY($4)
         GROUP BY a.id ORDER BY a.name",
    )
    .bind(all(f))
    .bind(orgs)
    .bind(projects)
    .bind(apps)
    .fetch_all(pool)
    .await?)
}

// ---- users and groups -----------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
pub struct UserRow {
    pub id: Uuid,
    pub issuer: String,
    pub subject: String,
    pub username: Option<String>,
    pub email: Option<String>,
    pub display_name: Option<String>,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_seen_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub disabled_at: Option<OffsetDateTime>,
}

const USER_COLS: &str =
    "id, issuer, subject, username, email, display_name, created_at, last_seen_at, disabled_at";

/// First sign-in creates the user; later ones refresh the profile and the last-seen time.
pub async fn upsert_user(
    pool: &PgPool,
    issuer: &str,
    subject: &str,
    username: Option<&str>,
    email: Option<&str>,
    display_name: Option<&str>,
) -> Result<UserRow> {
    Ok(sqlx::query_as(&format!(
        "INSERT INTO users (id, issuer, subject, username, email, display_name, last_seen_at) VALUES ($1,$2,$3,$4,$5,$6, now())
         ON CONFLICT (issuer, subject) DO UPDATE SET username = EXCLUDED.username, email = EXCLUDED.email, display_name = EXCLUDED.display_name, last_seen_at = now()
         RETURNING {USER_COLS}"
    ))
    .bind(Uuid::new_v4())
    .bind(issuer)
    .bind(subject)
    .bind(username)
    .bind(email)
    .bind(display_name)
    .fetch_one(pool)
    .await?)
}

pub async fn user(pool: &PgPool, id: Uuid) -> Result<Option<UserRow>> {
    Ok(
        sqlx::query_as(&format!("SELECT {USER_COLS} FROM users WHERE id = $1"))
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// A person by exact user name or e-mail address (case-insensitive), for adding them to a scope.
pub async fn find_user(pool: &PgPool, query: &str) -> Result<Option<UserRow>> {
    Ok(sqlx::query_as(&format!("SELECT {USER_COLS} FROM users WHERE lower(username) = lower($1) OR lower(email) = lower($1) ORDER BY created_at LIMIT 1")).bind(query).fetch_optional(pool).await?)
}

pub async fn set_user_disabled(pool: &PgPool, id: Uuid, disabled: bool) -> Result<()> {
    let n = sqlx::query("UPDATE users SET disabled_at = CASE WHEN $2 THEN COALESCE(disabled_at, now()) ELSE NULL END WHERE id = $1").bind(id).bind(disabled).execute(pool).await?.rows_affected();
    if n == 0 {
        Err(StoreError::NotFound(format!("user {id}")))
    } else {
        Ok(())
    }
}

/// Users visible under the filter: everyone for a platform-wide grant, otherwise people who
/// hold an assignment (directly or through a group) in a scope the filter reaches.
pub async fn list_users(pool: &PgPool, f: &AccessFilter) -> Result<Vec<UserRow>> {
    let (orgs, projects, apps) = arrays(f);
    Ok(sqlx::query_as(&format!(
        "SELECT {USER_COLS} FROM users u WHERE $1 OR EXISTS (
            SELECT 1 FROM role_assignments ra
            LEFT JOIN projects rp ON rp.id = ra.project_id
            LEFT JOIN applications rapp ON rapp.id = ra.application_id
            LEFT JOIN projects ap ON ap.id = rapp.project_id
            WHERE ra.revoked_at IS NULL
              AND ((ra.principal_type = 'user' AND ra.principal_id = u.id)
                   OR (ra.principal_type = 'group' AND ra.principal_id IN (SELECT group_id FROM group_members WHERE user_id = u.id)))
              AND (ra.organization_id = ANY($2) OR rp.organization_id = ANY($2) OR ap.organization_id = ANY($2)
                   OR ra.project_id = ANY($3) OR rapp.project_id = ANY($3)
                   OR ra.application_id = ANY($4)))
         ORDER BY u.username NULLS LAST, u.created_at"
    ))
    .bind(all(f))
    .bind(orgs)
    .bind(projects)
    .bind(apps)
    .fetch_all(pool)
    .await?)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
pub struct GroupRow {
    pub id: Uuid,
    pub organization_id: Option<Uuid>,
    pub name: String,
    pub external_name: Option<String>,
    pub description: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
}

const GROUP_COLS: &str = "id, organization_id, name, external_name, description, created_at";

pub async fn create_group(
    pool: &PgPool,
    organization_id: Option<Uuid>,
    name: &str,
    external_name: Option<&str>,
    description: &str,
) -> Result<GroupRow> {
    sqlx::query_as(&format!("INSERT INTO groups (id, organization_id, name, external_name, description) VALUES ($1,$2,$3,$4,$5) RETURNING {GROUP_COLS}"))
        .bind(Uuid::new_v4())
        .bind(organization_id)
        .bind(name)
        .bind(external_name)
        .bind(description)
        .fetch_one(pool)
        .await
        .map_err(|e| db_conflict(e, "a group with that name or identity-provider name"))
}

pub async fn group(pool: &PgPool, id: Uuid) -> Result<Option<GroupRow>> {
    Ok(
        sqlx::query_as(&format!("SELECT {GROUP_COLS} FROM groups WHERE id = $1"))
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// Platform groups are visible only to a platform-wide grant; organisation groups to the
/// organisation's scope.
pub async fn list_groups(pool: &PgPool, f: &AccessFilter) -> Result<Vec<GroupRow>> {
    let (orgs, projects, _) = arrays(f);
    Ok(sqlx::query_as(&format!(
        "SELECT {GROUP_COLS} FROM groups g WHERE $1 OR g.organization_id = ANY($2)
            OR g.organization_id IN (SELECT organization_id FROM projects WHERE id = ANY($3)) ORDER BY g.name"
    ))
    .bind(all(f))
    .bind(orgs)
    .bind(projects)
    .fetch_all(pool)
    .await?)
}

pub async fn add_group_member(
    pool: &PgPool,
    group_id: Uuid,
    user_id: Uuid,
    by: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO group_members (group_id, user_id, added_by) VALUES ($1,$2,$3) ON CONFLICT DO NOTHING").bind(group_id).bind(user_id).bind(by).execute(pool).await.map_err(|e| db_conflict(e, "that membership"))?;
    Ok(())
}

pub async fn remove_group_member(pool: &PgPool, group_id: Uuid, user_id: Uuid) -> Result<()> {
    sqlx::query("DELETE FROM group_members WHERE group_id = $1 AND user_id = $2")
        .bind(group_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn group_members(pool: &PgPool, group_id: Uuid) -> Result<Vec<UserRow>> {
    Ok(sqlx::query_as(&format!("SELECT {USER_COLS} FROM users WHERE id IN (SELECT user_id FROM group_members WHERE group_id = $1) ORDER BY username NULLS LAST")).bind(group_id).fetch_all(pool).await?)
}

/// Groups a person belongs to: by local membership and by the group names in their token.
pub async fn groups_of(
    pool: &PgPool,
    user_id: Uuid,
    token_groups: &BTreeSet<String>,
) -> Result<BTreeSet<Uuid>> {
    let names: Vec<String> = token_groups.iter().cloned().collect();
    let rows = sqlx::query("SELECT id FROM groups WHERE id IN (SELECT group_id FROM group_members WHERE user_id = $1) OR external_name = ANY($2)").bind(user_id).bind(names).fetch_all(pool).await?;
    Ok(rows.into_iter().map(|r| r.get("id")).collect())
}

// ---- roles and assignments ------------------------------------------------------------------

pub async fn create_custom_role(
    pool: &PgPool,
    name: &str,
    description: &str,
    permissions: &BTreeSet<String>,
    by: &str,
) -> Result<CustomRole> {
    let id = Uuid::new_v4();
    sqlx::query("INSERT INTO custom_roles (id, name, description, permissions, created_by) VALUES ($1,$2,$3,$4,$5)")
        .bind(id)
        .bind(name)
        .bind(description)
        .bind(permissions.iter().cloned().collect::<Vec<_>>())
        .bind(by)
        .execute(pool)
        .await
        .map_err(|e| db_conflict(e, "a role with that name"))?;
    Ok(CustomRole {
        id,
        name: name.to_owned(),
        description: description.to_owned(),
        permissions: permissions.clone(),
    })
}

pub async fn list_custom_roles(pool: &PgPool) -> Result<Vec<CustomRole>> {
    let rows =
        sqlx::query("SELECT id, name, description, permissions FROM custom_roles ORDER BY name")
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|r| CustomRole {
            id: r.get("id"),
            name: r.get("name"),
            description: r.get("description"),
            permissions: r.get::<Vec<String>, _>("permissions").into_iter().collect(),
        })
        .collect())
}

pub async fn custom_role(pool: &PgPool, id: Uuid) -> Result<Option<CustomRole>> {
    Ok(list_custom_roles(pool)
        .await?
        .into_iter()
        .find(|r| r.id == id))
}

/// Refuses while any assignment (even a revoked one, which the audit trail still refers to) uses the role.
pub async fn delete_custom_role(pool: &PgPool, id: Uuid) -> Result<()> {
    let n = sqlx::query("DELETE FROM custom_roles WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await
        .map_err(|e| db_conflict(e, "the role"))?
        .rows_affected();
    if n == 0 {
        Err(StoreError::NotFound(format!("role {id}")))
    } else {
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct NewAssignment {
    pub principal: PrincipalRef,
    pub role: RoleRef,
    pub scope: Scope,
    pub conditions: Conditions,
    pub kind: AssignmentKind,
    pub expires_at: Option<OffsetDateTime>,
    pub reason: Option<String>,
    pub created_by: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AssignmentRow {
    pub id: Uuid,
    pub principal_type: String,
    pub principal_id: Uuid,
    pub builtin_role: Option<String>,
    pub custom_role_id: Option<Uuid>,
    pub scope: Scope,
    pub conditions: Value,
    pub kind: String,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    pub reason: Option<String>,
    pub created_by: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub revoked_at: Option<OffsetDateTime>,
}

fn scope_columns(
    s: &Scope,
) -> (
    &'static str,
    Option<Uuid>,
    Option<Uuid>,
    Option<Uuid>,
    Option<String>,
) {
    match s {
        Scope::Platform => ("platform", None, None, None, None),
        Scope::Organization { id } => ("organization", Some(*id), None, None, None),
        Scope::Project { id } => ("project", None, Some(*id), None, None),
        Scope::Application { id } => ("application", None, None, Some(*id), None),
        Scope::Pool { name } => ("pool", None, None, None, Some(name.clone())),
    }
}

fn scope_from(row: &sqlx::postgres::PgRow) -> Option<Scope> {
    match row.get::<String, _>("scope_type").as_str() {
        "platform" => Some(Scope::Platform),
        "organization" => row
            .get::<Option<Uuid>, _>("organization_id")
            .map(|id| Scope::Organization { id }),
        "project" => row
            .get::<Option<Uuid>, _>("project_id")
            .map(|id| Scope::Project { id }),
        "application" => row
            .get::<Option<Uuid>, _>("application_id")
            .map(|id| Scope::Application { id }),
        "pool" => row
            .get::<Option<String>, _>("pool")
            .map(|name| Scope::Pool { name }),
        _ => None,
    }
}

fn principal_columns(p: &PrincipalRef) -> (&'static str, Uuid) {
    match p {
        PrincipalRef::User { id } => ("user", *id),
        PrincipalRef::Group { id } => ("group", *id),
        PrincipalRef::Service { id } => ("service", *id),
    }
}

pub async fn insert_assignment(pool: &PgPool, a: &NewAssignment) -> Result<Uuid> {
    let id = Uuid::new_v4();
    let (st, org, proj, app, pool_name) = scope_columns(&a.scope);
    let (pt, pid) = principal_columns(&a.principal);
    let (builtin, custom) = match &a.role {
        RoleRef::Builtin { key } => (Some(key.clone()), None),
        RoleRef::Custom { id } => (None, Some(*id)),
    };
    sqlx::query(
        "INSERT INTO role_assignments (id, principal_type, principal_id, builtin_role, custom_role_id, scope_type, organization_id, project_id, application_id, pool, conditions, kind, expires_at, reason, created_by)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)",
    )
    .bind(id)
    .bind(pt)
    .bind(pid)
    .bind(builtin)
    .bind(custom)
    .bind(st)
    .bind(org)
    .bind(proj)
    .bind(app)
    .bind(pool_name)
    .bind(serde_json::to_value(&a.conditions).unwrap_or_else(|_| json!({})))
    .bind(if a.kind == AssignmentKind::Emergency { "emergency" } else { "standard" })
    .bind(a.expires_at)
    .bind(&a.reason)
    .bind(&a.created_by)
    .execute(pool)
    .await
    .map_err(|e| db_conflict(e, "that assignment"))?;
    Ok(id)
}

pub async fn revoke_assignment(pool: &PgPool, id: Uuid, by: &str) -> Result<()> {
    let n = sqlx::query("UPDATE role_assignments SET revoked_at = now(), revoked_by = $2 WHERE id = $1 AND revoked_at IS NULL").bind(id).bind(by).execute(pool).await?.rows_affected();
    if n == 0 {
        Err(StoreError::NotFound(format!("assignment {id}")))
    } else {
        Ok(())
    }
}

fn to_row(r: &sqlx::postgres::PgRow) -> Option<AssignmentRow> {
    Some(AssignmentRow {
        id: r.get("id"),
        principal_type: r.get("principal_type"),
        principal_id: r.get("principal_id"),
        builtin_role: r.get("builtin_role"),
        custom_role_id: r.get("custom_role_id"),
        scope: scope_from(r)?,
        conditions: r.get("conditions"),
        kind: r.get("kind"),
        expires_at: r.get("expires_at"),
        reason: r.get("reason"),
        created_by: r.get("created_by"),
        created_at: r.get("created_at"),
        revoked_at: r.get("revoked_at"),
    })
}

const ASSIGNMENT_COLS: &str = "id, principal_type, principal_id, builtin_role, custom_role_id, scope_type, organization_id, project_id, application_id, pool, conditions, kind, expires_at, reason, created_by, created_at, revoked_at";

pub async fn assignment(pool: &PgPool, id: Uuid) -> Result<Option<AssignmentRow>> {
    let row = sqlx::query(&format!(
        "SELECT {ASSIGNMENT_COLS} FROM role_assignments WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.as_ref().and_then(to_row))
}

/// Assignments in scopes the filter reaches (including those of a project's applications),
/// optionally only those of one principal. Revoked ones are included for the audit view.
pub async fn list_assignments(
    pool: &PgPool,
    f: &AccessFilter,
    principal: Option<PrincipalRef>,
    include_revoked: bool,
) -> Result<Vec<AssignmentRow>> {
    let (orgs, projects, apps) = arrays(f);
    let (pt, pid) = principal
        .map(|p| principal_columns(&p))
        .map_or((None, None), |(t, i)| (Some(t), Some(i)));
    let rows = sqlx::query(&format!(
        "SELECT {ASSIGNMENT_COLS} FROM role_assignments ra
         WHERE ($5 OR ra.revoked_at IS NULL)
           AND ($6::text IS NULL OR (ra.principal_type = $6 AND ra.principal_id = $7))
           AND ($1 OR ra.organization_id = ANY($2) OR ra.project_id = ANY($3) OR ra.application_id = ANY($4)
                OR ra.project_id IN (SELECT id FROM projects WHERE organization_id = ANY($2))
                OR ra.application_id IN (SELECT id FROM applications WHERE project_id = ANY($3) OR project_id IN (SELECT id FROM projects WHERE organization_id = ANY($2))))
         ORDER BY ra.created_at DESC LIMIT 1000"
    ))
    .bind(all(f))
    .bind(orgs)
    .bind(projects)
    .bind(apps)
    .bind(include_revoked)
    .bind(pt)
    .bind(pid)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().filter_map(to_row).collect())
}

/// Builds the decision engine for a caller: their live assignments (their own and their
/// groups'), the custom roles those use, and the enabled access policies.
pub async fn load_engine(pool: &PgPool, principals: &[PrincipalRef]) -> Result<Engine> {
    let (types, ids): (Vec<String>, Vec<Uuid>) = principals
        .iter()
        .map(|p| {
            let (t, i) = principal_columns(p);
            (t.to_owned(), i)
        })
        .unzip();
    let rows = sqlx::query(&format!(
        "SELECT {ASSIGNMENT_COLS} FROM role_assignments ra
         WHERE ra.revoked_at IS NULL AND (ra.expires_at IS NULL OR ra.expires_at > now())
           AND EXISTS (SELECT 1 FROM unnest($1::text[], $2::uuid[]) AS p(t, i) WHERE p.t = ra.principal_type AND p.i = ra.principal_id)"
    ))
    .bind(types)
    .bind(ids)
    .fetch_all(pool)
    .await?;
    let mut assignments = Vec::new();
    for r in &rows {
        let Some(scope) = scope_from(r) else { continue };
        let principal = match (
            r.get::<String, _>("principal_type").as_str(),
            r.get::<Uuid, _>("principal_id"),
        ) {
            ("user", id) => PrincipalRef::User { id },
            ("group", id) => PrincipalRef::Group { id },
            ("service", id) => PrincipalRef::Service { id },
            _ => continue,
        };
        let role = match (
            r.get::<Option<String>, _>("builtin_role"),
            r.get::<Option<Uuid>, _>("custom_role_id"),
        ) {
            (Some(key), None) => RoleRef::Builtin { key },
            (None, Some(id)) => RoleRef::Custom { id },
            _ => continue,
        };
        assignments.push(Assignment {
            id: r.get("id"),
            principal,
            role,
            scope,
            conditions: serde_json::from_value(r.get::<Value, _>("conditions")).unwrap_or_default(),
            expires_at: r
                .get::<Option<OffsetDateTime>, _>("expires_at")
                .map(|t| t.unix_timestamp()),
            revoked: false,
            kind: if r.get::<String, _>("kind") == "emergency" {
                AssignmentKind::Emergency
            } else {
                AssignmentKind::Standard
            },
        });
    }
    let custom_ids: Vec<Uuid> = assignments
        .iter()
        .filter_map(|a| {
            if let RoleRef::Custom { id } = a.role {
                Some(id)
            } else {
                None
            }
        })
        .collect();
    let mut custom_roles = HashMap::new();
    if !custom_ids.is_empty() {
        for c in list_custom_roles(pool).await? {
            if custom_ids.contains(&c.id) {
                custom_roles.insert(c.id, c);
            }
        }
    }
    Ok(Engine {
        assignments,
        custom_roles,
        policies: load_policies(pool).await?,
    })
}

// ---- access policies ------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
pub struct PolicyRow {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub parameters: Value,
    pub enabled: bool,
    pub updated_by: String,
    #[serde(with = "time::serde::rfc3339")]
    pub updated_at: OffsetDateTime,
}

pub async fn list_policies(pool: &PgPool) -> Result<Vec<PolicyRow>> {
    Ok(sqlx::query_as("SELECT id, name, kind, parameters, enabled, updated_by, updated_at FROM access_policies ORDER BY name").fetch_all(pool).await?)
}

pub async fn save_policy(
    pool: &PgPool,
    name: &str,
    kind: &str,
    parameters: &Value,
    enabled: bool,
    by: &str,
) -> Result<PolicyRow> {
    sqlx::query_as(
        "INSERT INTO access_policies (id, name, kind, parameters, enabled, updated_by) VALUES ($1,$2,$3,$4,$5,$6)
         ON CONFLICT (name) DO UPDATE SET kind = EXCLUDED.kind, parameters = EXCLUDED.parameters, enabled = EXCLUDED.enabled, updated_by = EXCLUDED.updated_by, updated_at = now()
         RETURNING id, name, kind, parameters, enabled, updated_by, updated_at",
    )
    .bind(Uuid::new_v4())
    .bind(name)
    .bind(kind)
    .bind(parameters)
    .bind(enabled)
    .bind(by)
    .fetch_one(pool)
    .await
    .map_err(|e| db_conflict(e, "that policy"))
}

pub async fn delete_policy(pool: &PgPool, id: Uuid) -> Result<()> {
    let n = sqlx::query("DELETE FROM access_policies WHERE id = $1")
        .bind(id)
        .execute(pool)
        .await?
        .rows_affected();
    if n == 0 {
        Err(StoreError::NotFound(format!("policy {id}")))
    } else {
        Ok(())
    }
}

pub async fn load_policies(pool: &PgPool) -> Result<Policies> {
    let mut p = Policies::default();
    for row in list_policies(pool).await?.into_iter().filter(|r| r.enabled) {
        match row.kind.as_str() {
            "mfa" => {
                if let Some(list) = row.parameters.get("environments").and_then(Value::as_array) {
                    p.mfa_for_environments.extend(
                        list.iter()
                            .filter_map(Value::as_str)
                            .filter_map(EnvKind::parse),
                    );
                }
                if row
                    .parameters
                    .get("infrastructure")
                    .and_then(Value::as_bool)
                    == Some(true)
                {
                    p.mfa_for_infrastructure = true;
                }
            }
            "approval" => {
                if let Some(list) = row.parameters.get("actions").and_then(Value::as_array) {
                    p.extra_approval
                        .extend(list.iter().filter_map(Value::as_str).map(str::to_owned));
                }
            }
            _ => {}
        }
    }
    Ok(p)
}

// ---- service identities ---------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
pub struct ServiceIdentityRow {
    pub id: Uuid,
    pub name: String,
    pub kind: String,
    pub credential_ref: Option<String>,
    pub restrictions: Value,
    pub created_by: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339::option")]
    pub expires_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub disabled_at: Option<OffsetDateTime>,
    #[serde(with = "time::serde::rfc3339::option")]
    pub last_used_at: Option<OffsetDateTime>,
}

const SERVICE_COLS: &str = "id, name, kind, credential_ref, restrictions, created_by, created_at, expires_at, disabled_at, last_used_at";

fn hash(token: &str) -> Vec<u8> {
    Sha256::digest(token.as_bytes()).to_vec()
}

/// Registers a service identity. Automation identities also get an API token, returned once.
pub async fn create_service_identity(
    pool: &PgPool,
    name: &str,
    kind: &str,
    credential_ref: Option<&str>,
    restrictions: &Value,
    expires_at: Option<OffsetDateTime>,
    by: &str,
) -> Result<(ServiceIdentityRow, Option<String>)> {
    let token = if kind == "automation" {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|e| StoreError::Conflict(format!("no randomness available: {e}")))?;
        Some(format!(
            "aps_{}",
            bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()
        ))
    } else {
        None
    };
    let row = sqlx::query_as(&format!(
        "INSERT INTO service_identities (id, name, kind, credential_ref, restrictions, token_hash, created_by, expires_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8) RETURNING {SERVICE_COLS}"
    ))
    .bind(Uuid::new_v4())
    .bind(name)
    .bind(kind)
    .bind(credential_ref)
    .bind(restrictions)
    .bind(token.as_deref().map(hash))
    .bind(by)
    .bind(expires_at)
    .fetch_one(pool)
    .await
    .map_err(|e| db_conflict(e, "a service identity with that name"))?;
    Ok((row, token))
}

pub async fn list_service_identities(pool: &PgPool) -> Result<Vec<ServiceIdentityRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {SERVICE_COLS} FROM service_identities ORDER BY name"
    ))
    .fetch_all(pool)
    .await?)
}

pub async fn set_service_identity_disabled(pool: &PgPool, id: Uuid, disabled: bool) -> Result<()> {
    let n = sqlx::query("UPDATE service_identities SET disabled_at = CASE WHEN $2 THEN COALESCE(disabled_at, now()) ELSE NULL END WHERE id = $1").bind(id).bind(disabled).execute(pool).await?.rows_affected();
    if n == 0 {
        Err(StoreError::NotFound(format!("service identity {id}")))
    } else {
        Ok(())
    }
}

/// The automation identity a presented API token belongs to, if it is live.
pub async fn authenticate_service(
    pool: &PgPool,
    token: &str,
) -> Result<Option<ServiceIdentityRow>> {
    let row = sqlx::query_as::<_, ServiceIdentityRow>(&format!(
        "UPDATE service_identities SET last_used_at = now() WHERE token_hash = $1 AND kind = 'automation' AND disabled_at IS NULL AND (expires_at IS NULL OR expires_at > now()) RETURNING {SERVICE_COLS}"
    ))
    .bind(hash(token))
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

// ---- approvals ------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, FromRow)]
pub struct ApprovalRow {
    pub id: Uuid,
    pub action: String,
    pub target: String,
    pub requester: String,
    pub approver: Option<String>,
    pub state: String,
    pub reason: String,
    #[serde(with = "time::serde::rfc3339")]
    pub created_at: OffsetDateTime,
    #[serde(with = "time::serde::rfc3339")]
    pub expires_at: OffsetDateTime,
}

const APPROVAL_COLS: &str =
    "id, action, target, requester, approver, state, reason, created_at, expires_at";

fn state_of(s: &str) -> ApprovalState {
    match s {
        "approved" => ApprovalState::Approved,
        "rejected" => ApprovalState::Rejected,
        "cancelled" => ApprovalState::Cancelled,
        "consumed" => ApprovalState::Consumed,
        _ => ApprovalState::Pending,
    }
}
fn state_name(s: ApprovalState) -> &'static str {
    match s {
        ApprovalState::Pending => "pending",
        ApprovalState::Approved => "approved",
        ApprovalState::Rejected => "rejected",
        ApprovalState::Cancelled => "cancelled",
        ApprovalState::Consumed => "consumed",
    }
}
fn domain(r: &ApprovalRow) -> Approval {
    Approval {
        action: r.action.clone(),
        target: r.target.clone(),
        requester: r.requester.clone(),
        approver: r.approver.clone(),
        state: state_of(&r.state),
        expires_at: r.expires_at.unix_timestamp(),
    }
}

pub async fn request_approval(
    pool: &PgPool,
    action: &str,
    target: &str,
    requester: &str,
    reason: &str,
) -> Result<ApprovalRow> {
    let a = Approval::request(
        action,
        target,
        requester,
        OffsetDateTime::now_utc().unix_timestamp(),
    );
    Ok(sqlx::query_as(&format!("INSERT INTO approvals (id, action, target, requester, reason, expires_at) VALUES ($1,$2,$3,$4,$5, to_timestamp($6)) RETURNING {APPROVAL_COLS}"))
        .bind(Uuid::new_v4())
        .bind(action)
        .bind(target)
        .bind(requester)
        .bind(reason)
        .bind(a.expires_at as f64)
        .fetch_one(pool)
        .await?)
}

pub async fn approval(pool: &PgPool, id: Uuid) -> Result<Option<ApprovalRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {APPROVAL_COLS} FROM approvals WHERE id = $1"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?)
}

pub async fn list_approvals(
    pool: &PgPool,
    requester: Option<&str>,
    only_open: bool,
) -> Result<Vec<ApprovalRow>> {
    Ok(sqlx::query_as(&format!(
        "SELECT {APPROVAL_COLS} FROM approvals WHERE ($1::text IS NULL OR requester = $1) AND (NOT $2 OR (state = 'pending' AND expires_at > now())) ORDER BY created_at DESC LIMIT 200"
    ))
    .bind(requester)
    .bind(only_open)
    .fetch_all(pool)
    .await?)
}

/// Approves or rejects. `approver_may` is the caller's authorisation check for the action.
pub async fn decide_approval(
    pool: &PgPool,
    id: Uuid,
    approver: &str,
    approver_may: bool,
    approve: bool,
) -> Result<std::result::Result<ApprovalRow, apelora_auth::ApprovalError>> {
    let mut tx = pool.begin().await?;
    let Some(row) = sqlx::query_as::<_, ApprovalRow>(&format!(
        "SELECT {APPROVAL_COLS} FROM approvals WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Err(StoreError::NotFound(format!("approval {id}")));
    };
    let now = OffsetDateTime::now_utc().unix_timestamp();
    match domain(&row).decide(approver, approver_may, approve, now) {
        Err(e) => Ok(Err(e)),
        Ok(next) => {
            let updated = sqlx::query_as(&format!("UPDATE approvals SET state = $2, approver = $3, decided_at = now(), expires_at = to_timestamp($4) WHERE id = $1 RETURNING {APPROVAL_COLS}"))
                .bind(id)
                .bind(state_name(next.state))
                .bind(&next.approver)
                .bind(next.expires_at as f64)
                .fetch_one(&mut *tx)
                .await?;
            tx.commit().await?;
            Ok(Ok(updated))
        }
    }
}

pub async fn cancel_approval(
    pool: &PgPool,
    id: Uuid,
    who: &str,
) -> Result<std::result::Result<ApprovalRow, apelora_auth::ApprovalError>> {
    let mut tx = pool.begin().await?;
    let Some(row) = sqlx::query_as::<_, ApprovalRow>(&format!(
        "SELECT {APPROVAL_COLS} FROM approvals WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Err(StoreError::NotFound(format!("approval {id}")));
    };
    match domain(&row).cancel(who) {
        Err(e) => Ok(Err(e)),
        Ok(next) => {
            let updated = sqlx::query_as(&format!(
                "UPDATE approvals SET state = $2 WHERE id = $1 RETURNING {APPROVAL_COLS}"
            ))
            .bind(id)
            .bind(state_name(next.state))
            .fetch_one(&mut *tx)
            .await?;
            tx.commit().await?;
            Ok(Ok(updated))
        }
    }
}

/// Spends an approval for exactly this action and target, once. Atomic: two concurrent
/// attempts cannot both succeed.
pub async fn consume_approval(
    pool: &PgPool,
    id: Uuid,
    action: &str,
    target: &str,
    who: &str,
) -> Result<std::result::Result<ApprovalRow, apelora_auth::ApprovalError>> {
    let mut tx = pool.begin().await?;
    let Some(row) = sqlx::query_as::<_, ApprovalRow>(&format!(
        "SELECT {APPROVAL_COLS} FROM approvals WHERE id = $1 FOR UPDATE"
    ))
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?
    else {
        return Err(StoreError::NotFound(format!("approval {id}")));
    };
    let now = OffsetDateTime::now_utc().unix_timestamp();
    match domain(&row).consume(action, target, who, now) {
        Err(e) => Ok(Err(e)),
        Ok(next) => {
            let updated = sqlx::query_as(&format!("UPDATE approvals SET state = $2, consumed_at = now() WHERE id = $1 RETURNING {APPROVAL_COLS}")).bind(id).bind(state_name(next.state)).fetch_one(&mut *tx).await?;
            tx.commit().await?;
            Ok(Ok(updated))
        }
    }
}

// ---- audit ----------------------------------------------------------------------------------

/// One audit record. `scope` names the organisation, project and application involved so the
/// trail can be filtered by what a reader is allowed to see.
pub struct AuditEntry<'a> {
    pub actor: &'a str,
    pub actor_type: &'a str,
    pub action: &'a str,
    pub target: &'a str,
    pub outcome: &'a str,
    pub scope: Option<&'a Resource>,
    pub detail: &'a Value,
}

pub async fn audit_event(pool: &PgPool, e: &AuditEntry<'_>) -> Result<()> {
    let scope = e.scope.map(|r| json!({ "organization": r.organization, "project": r.project, "application": r.application }));
    sqlx::query("INSERT INTO audit_events (actor, actor_type, action, target, outcome, scope, detail) VALUES ($1,$2,$3,$4,$5,$6,$7)")
        .bind(e.actor)
        .bind(e.actor_type)
        .bind(e.action)
        .bind(e.target)
        .bind(e.outcome)
        .bind(scope)
        .bind(e.detail)
        .execute(pool)
        .await?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, FromRow)]
pub struct AuditRow {
    pub id: i64,
    #[serde(with = "time::serde::rfc3339")]
    pub at: OffsetDateTime,
    pub actor: String,
    pub actor_type: String,
    pub action: String,
    pub target: String,
    pub outcome: String,
    pub scope: Option<Value>,
    pub detail: Value,
}

/// Newest first. Platform-wide readers see everything (including events with no scope); others
/// see only events that name an organisation, project or application they may read.
pub async fn audit_history(
    pool: &PgPool,
    f: &AccessFilter,
    action_prefix: Option<&str>,
    outcome: Option<&str>,
    before_id: Option<i64>,
    limit: i64,
) -> Result<Vec<AuditRow>> {
    let (orgs, projects, apps) = arrays(f);
    Ok(sqlx::query_as(
        "SELECT id, at, actor, actor_type, action, target, outcome, scope, detail FROM audit_events e
         WHERE ($1 OR (e.scope IS NOT NULL AND ((e.scope->>'organization')::uuid = ANY($2) OR (e.scope->>'project')::uuid = ANY($3) OR (e.scope->>'application')::uuid = ANY($4))))
           AND ($5::text IS NULL OR e.action LIKE $5 || '%')
           AND ($6::text IS NULL OR e.outcome = $6)
           AND ($7::bigint IS NULL OR e.id < $7)
         ORDER BY e.id DESC LIMIT $8",
    )
    .bind(all(f))
    .bind(orgs)
    .bind(projects)
    .bind(apps)
    .bind(action_prefix)
    .bind(outcome)
    .bind(before_id)
    .bind(limit.clamp(1, 500))
    .fetch_all(pool)
    .await?)
}

/// Count of live platform-super-admin assignments; used to refuse revoking the last one.
pub async fn super_admin_count(pool: &PgPool) -> Result<i64> {
    Ok(sqlx::query_scalar("SELECT count(*) FROM role_assignments WHERE builtin_role = 'platform-super-admin' AND scope_type = 'platform' AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at > now()) AND kind = 'standard'").fetch_one(pool).await?)
}
