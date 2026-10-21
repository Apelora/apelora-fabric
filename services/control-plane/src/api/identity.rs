// SPDX-License-Identifier: AGPL-3.0-only
//! Identity and access management API: organisations, projects, environments, people, groups,
//! roles, role assignments, service identities, access policies, approvals and the audit trail.
//!
//! Every handler authorises on the server against the exact scope it touches; the route gate in
//! `authz.rs` is only the coarse outer check. Reads are limited in SQL to what the caller's
//! scopes reach. Anything that grants access goes through `apelora_auth::check_assignment`,
//! so nobody can grant more than they hold.

use std::collections::{BTreeMap, BTreeSet};

use apelora_auth::service::ServiceKind;
use apelora_auth::{
    AccessFilter, AssignRequest, Assignment, AssignmentKind, Builtin, Class, Conditions,
    CustomRole, Engine, EnvKind, Grant, Level, PERMISSIONS, PrincipalRef, ResolvedRole, Resource,
    RoleRef, Scope, Subject, check_assignment, check_revocation, permission, validate_custom,
};
use apelora_persistence::identity as db;
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use time::OffsetDateTime;
use utoipa::{IntoParams, ToSchema};
use uuid::Uuid;

use super::authz::{allow, allow_any, allow_platform, audit_scoped, filter_for, now};
use super::deploy::store;
use super::{ApiError, AppState, AuditItem, AuditList, ErrorBody, Principal};

type Res<T> = Result<T, ApiError>;

/// Requests one person may have waiting for a decision at once.
const MAX_OPEN_REQUESTS: i64 = 20;

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::BAD_REQUEST, "invalid_request", msg)
}
fn not_found(what: &str) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "not_found",
        format!("not found: {what}"),
    )
}
fn conflict(code: &'static str, msg: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, code, msg)
}
fn json_body<T>(b: Result<Json<T>, JsonRejection>) -> Res<T> {
    b.map(|Json(v)| v).map_err(|e| bad(e.body_text()))
}
fn rfc3339(t: OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}
fn parse_time(s: &str) -> Res<OffsetDateTime> {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339)
        .map_err(|_| bad("times are RFC 3339, e.g. 2026-12-31T00:00:00Z"))
}

fn valid_slug(s: &str) -> bool {
    (2..=40).contains(&s.len())
        && s.starts_with(|c: char| c.is_ascii_lowercase())
        && !s.ends_with('-')
        && s.chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}
fn valid_name(s: &str) -> Res<&str> {
    let n = s.trim();
    if n.is_empty() || n.chars().count() > 80 || n.chars().any(char::is_control) {
        return Err(bad("a name is 1–80 characters"));
    }
    Ok(n)
}

/// The engine and subject the caller acts as. Legacy platform administrators (and development
/// mode) act as platform super admins, as they always could; other legacy roles hold nothing here.
fn actor(p: &Principal) -> (Engine, Subject) {
    if let Some(c) = &p.caller {
        return (c.engine.clone(), c.subject.clone());
    }
    let me = Uuid::nil();
    let mut e = Engine::default();
    if p.role
        .is_some_and(|r| r.allows(apelora_domain::Permission::ManageTokens))
    {
        e.assignments.push(Assignment {
            id: Uuid::nil(),
            principal: PrincipalRef::User { id: me },
            role: RoleRef::Builtin {
                key: Builtin::PlatformSuperAdmin.key().into(),
            },
            scope: Scope::Platform,
            conditions: Conditions::default(),
            expires_at: None,
            revoked: false,
            kind: AssignmentKind::Standard,
        });
    }
    (
        e,
        Subject {
            user: Some(me),
            mfa: true,
            ..Subject::default()
        },
    )
}

async fn resource_of(pool: &apelora_persistence::Pool, scope: &Scope) -> Res<Resource> {
    Ok(match scope {
        Scope::Platform => Resource::platform(),
        Scope::Pool { name } => {
            if name.is_empty()
                || name.len() > 80
                || !name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
            {
                return Err(bad(
                    "a hardware pool name is 1–80 letters, digits, . _ or -",
                ));
            }
            Resource::pool(name)
        }
        Scope::Organization { id } => {
            db::organization(pool, *id)
                .await?
                .ok_or_else(|| not_found("organisation"))?;
            Resource::organization(*id)
        }
        Scope::Project { id } => {
            let p = db::project(pool, *id)
                .await?
                .ok_or_else(|| not_found("project"))?;
            Resource::project(p.organization_id, p.id)
        }
        Scope::Application { id } => db::application_context_by_id(pool, *id)
            .await?
            .ok_or_else(|| not_found("application"))?
            .resource(),
    })
}

// ---- DTOs -------------------------------------------------------------------------------------

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OrganizationDto {
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub created_at: String,
    pub disabled: bool,
}
#[derive(Serialize, ToSchema)]
pub struct OrganizationList {
    pub items: Vec<OrganizationDto>,
}
#[derive(Deserialize, ToSchema)]
pub struct NewOrganization {
    pub slug: String,
    pub name: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProjectDto {
    pub id: Uuid,
    pub organization_id: Uuid,
    pub organization_slug: String,
    pub slug: String,
    pub name: String,
    pub created_at: String,
}
/// Identity projects (`/api/v1/identity/projects`); not the Incus projects of
/// `/api/v1/projects`, whose schema is `ProjectList`.
#[derive(Serialize, ToSchema)]
#[schema(as = IdentityProjectList)]
pub struct ProjectList {
    pub items: Vec<ProjectDto>,
}
#[derive(Deserialize, ToSchema)]
pub struct NewProject {
    pub slug: String,
    pub name: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentDto {
    pub id: Uuid,
    pub project_id: Uuid,
    pub slug: String,
    pub name: String,
    pub kind: EnvKind,
    /// The restricted Incus project this environment deploys into.
    pub incus_project: Option<String>,
    /// The settings that project needs (restricted, unprivileged, no host devices). Apelora does
    /// not create or verify the Incus project yet: an administrator does, with these settings.
    pub incus_project_config: BTreeMap<String, String>,
    pub incus_provisioned: bool,
}
#[derive(Serialize, ToSchema)]
pub struct EnvironmentList {
    pub items: Vec<EnvironmentDto>,
}
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewEnvironment {
    pub slug: String,
    pub name: String,
    pub kind: EnvKind,
    /// Platform administrators only: map an existing Incus project instead of the derived name.
    pub incus_project: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct UserDto {
    pub id: Uuid,
    pub username: Option<String>,
    pub email: Option<String>,
    pub display_name: Option<String>,
    pub issuer: String,
    pub created_at: String,
    pub last_seen_at: Option<String>,
    pub disabled: bool,
}
#[derive(Serialize, ToSchema)]
pub struct UserList {
    pub items: Vec<UserDto>,
}
pub(super) fn user_dto(u: db::UserRow) -> UserDto {
    UserDto {
        id: u.id,
        username: u.username,
        email: u.email,
        display_name: u.display_name,
        issuer: u.issuer,
        created_at: rfc3339(u.created_at),
        last_seen_at: u.last_seen_at.map(rfc3339),
        disabled: u.disabled_at.is_some(),
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct GroupDto {
    pub id: Uuid,
    /// `null` = a platform group.
    pub organization_id: Option<Uuid>,
    pub name: String,
    /// The group's name in the identity provider's tokens, when members come from there.
    pub external_name: Option<String>,
    pub description: String,
}
#[derive(Serialize, ToSchema)]
pub struct GroupList {
    pub items: Vec<GroupDto>,
}
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewGroup {
    pub organization_id: Option<Uuid>,
    pub name: String,
    pub external_name: Option<String>,
    #[serde(default)]
    pub description: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RoleDto {
    /// `builtin:<key>` or `custom:<uuid>`.
    pub id: String,
    pub reference: RoleRef,
    pub name: String,
    pub description: String,
    pub builtin: bool,
    pub permissions: Vec<String>,
    /// Scope levels it can be assigned at.
    pub scopes: Vec<Level>,
}
#[derive(Serialize, ToSchema)]
pub struct RoleList {
    pub items: Vec<RoleDto>,
}
#[derive(Deserialize, ToSchema)]
pub struct NewRole {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub permissions: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PermissionDto {
    pub id: String,
    pub class: Class,
    /// The narrowest scope level at which a grant takes effect.
    pub floor: Level,
    pub needs_approval: bool,
    pub description: String,
}
#[derive(Serialize, ToSchema)]
pub struct PermissionList {
    pub items: Vec<PermissionDto>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssignmentDto {
    pub id: Uuid,
    pub principal: PrincipalRef,
    pub role: RoleRef,
    pub role_name: String,
    pub scope: Scope,
    pub conditions: Conditions,
    pub kind: AssignmentKind,
    pub expires_at: Option<String>,
    pub reason: Option<String>,
    pub created_by: String,
    pub created_at: String,
    pub revoked_at: Option<String>,
}
#[derive(Serialize, ToSchema)]
pub struct AssignmentList {
    pub items: Vec<AssignmentDto>,
}

/// The role an assignment row refers to (the table holds exactly one of the two columns).
fn assignment_role(r: &db::AssignmentRow) -> RoleRef {
    match (&r.builtin_role, r.custom_role_id) {
        (None, Some(id)) => RoleRef::Custom { id },
        (key, _) => RoleRef::Builtin {
            key: key.clone().unwrap_or_default(),
        },
    }
}

fn assignment_dto(r: db::AssignmentRow, customs: &[CustomRole]) -> AssignmentDto {
    let principal = match r.principal_type.as_str() {
        "group" => PrincipalRef::Group { id: r.principal_id },
        "service" => PrincipalRef::Service { id: r.principal_id },
        _ => PrincipalRef::User { id: r.principal_id },
    };
    let (role, role_name) = match (&r.builtin_role, r.custom_role_id) {
        (Some(k), _) => (
            RoleRef::Builtin { key: k.clone() },
            Builtin::parse(k)
                .map(|b| b.name().to_owned())
                .unwrap_or_else(|| k.clone()),
        ),
        (None, Some(id)) => (
            RoleRef::Custom { id },
            customs
                .iter()
                .find(|c| c.id == id)
                .map(|c| c.name.clone())
                .unwrap_or_default(),
        ),
        (None, None) => (RoleRef::Builtin { key: String::new() }, String::new()),
    };
    AssignmentDto {
        id: r.id,
        principal,
        role,
        role_name,
        scope: r.scope,
        conditions: serde_json::from_value(r.conditions).unwrap_or_default(),
        kind: if r.kind == "emergency" {
            AssignmentKind::Emergency
        } else {
            AssignmentKind::Standard
        },
        expires_at: r.expires_at.map(rfc3339),
        reason: r.reason,
        created_by: r.created_by,
        created_at: rfc3339(r.created_at),
        revoked_at: r.revoked_at.map(rfc3339),
    }
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewAssignment {
    pub principal: PrincipalRef,
    pub role: RoleRef,
    pub scope: Scope,
    #[serde(default)]
    pub conditions: Conditions,
    /// RFC 3339. Assignments may expire; emergency grants must.
    pub expires_at: Option<String>,
    pub reason: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EffectivePermission {
    pub permission: String,
    pub class: Class,
    pub description: String,
    /// Where each part of this permission comes from.
    pub grants: Vec<Grant>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AssignmentPreview {
    /// Could the signed-in administrator save this assignment?
    pub allowed: bool,
    /// Why not, when `allowed` is false.
    pub error: Option<String>,
    /// What the person can already do at that scope.
    pub before: Vec<EffectivePermission>,
    /// What they could do once this assignment exists (the new grant has the nil assignment id).
    pub after: Vec<EffectivePermission>,
    /// Permissions the assignment adds.
    pub added: Vec<String>,
    /// Permissions in the role that do nothing at this scope.
    pub inert: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ServiceIdentityDto {
    pub id: Uuid,
    pub name: String,
    pub kind: ServiceKind,
    /// The system it authenticates to.
    pub backend: String,
    /// The operation families this kind may perform.
    pub operations: Vec<String>,
    /// Name of the credential in the backend's secret store (never the secret).
    pub credential_ref: Option<String>,
    #[schema(value_type = Object)]
    pub restrictions: Value,
    pub created_by: String,
    pub created_at: String,
    pub expires_at: Option<String>,
    pub disabled: bool,
    pub last_used_at: Option<String>,
    /// Can call the Apelora API (automation identities only).
    pub api_access: bool,
}
#[derive(Serialize, ToSchema)]
pub struct ServiceIdentityList {
    pub items: Vec<ServiceIdentityDto>,
}
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewServiceIdentity {
    pub name: String,
    pub kind: ServiceKind,
    pub credential_ref: Option<String>,
    #[serde(default)]
    #[schema(value_type = Object)]
    pub restrictions: Value,
    /// Automation identities only: days until their token expires (1–365, default 90).
    pub expires_in_days: Option<i64>,
}
#[derive(Serialize, ToSchema)]
pub struct ServiceIdentityCreated {
    /// The API token of an automation identity, shown only here.
    pub token: Option<String>,
    pub identity: ServiceIdentityDto,
}
fn service_dto(r: db::ServiceIdentityRow) -> ServiceIdentityDto {
    let kind = ServiceKind::parse(&r.kind).unwrap_or(ServiceKind::Automation);
    ServiceIdentityDto {
        id: r.id,
        name: r.name,
        kind,
        backend: kind.backend().to_owned(),
        operations: kind.operations().iter().map(|s| (*s).to_owned()).collect(),
        credential_ref: r.credential_ref,
        restrictions: r.restrictions,
        created_by: r.created_by,
        created_at: rfc3339(r.created_at),
        expires_at: r.expires_at.map(rfc3339),
        disabled: r.disabled_at.is_some(),
        last_used_at: r.last_used_at.map(rfc3339),
        api_access: kind.may_call_api(),
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct PolicyDto {
    pub id: Uuid,
    pub name: String,
    /// `mfa` or `approval`.
    pub kind: String,
    #[schema(value_type = Object)]
    pub parameters: Value,
    pub enabled: bool,
    pub updated_by: String,
    pub updated_at: String,
}
#[derive(Serialize, ToSchema)]
pub struct PolicyList {
    pub items: Vec<PolicyDto>,
}
#[derive(Deserialize, ToSchema)]
pub struct SavePolicy {
    pub name: String,
    pub kind: String,
    #[schema(value_type = Object)]
    pub parameters: Value,
    #[serde(default = "yes")]
    pub enabled: bool,
}
fn yes() -> bool {
    true
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApprovalDto {
    pub id: Uuid,
    pub action: String,
    pub target: String,
    pub requester: String,
    pub approver: Option<String>,
    /// `pending`, `approved`, `rejected`, `cancelled` or `consumed`.
    pub state: String,
    pub reason: String,
    pub created_at: String,
    pub expires_at: String,
}
#[derive(Serialize, ToSchema)]
pub struct ApprovalList {
    pub items: Vec<ApprovalDto>,
}
fn approval_dto(a: db::ApprovalRow) -> ApprovalDto {
    ApprovalDto {
        id: a.id,
        action: a.action,
        target: a.target,
        requester: a.requester,
        approver: a.approver,
        state: a.state,
        reason: a.reason,
        created_at: rfc3339(a.created_at),
        expires_at: rfc3339(a.expires_at),
    }
}
#[derive(Deserialize, ToSchema)]
pub struct NewApproval {
    /// The permission to be exercised, e.g. `hardware.delete`.
    pub action: String,
    /// What it will be done to, e.g. `machine:rack1-07`.
    pub target: String,
    #[serde(default)]
    pub reason: String,
}

// ---- organisations, projects, environments ----------------------------------------------------

/// Organisations the caller may see. Needs `organization.view` (or a grant below that reaches them).
#[utoipa::path(get, path = "/api/v1/identity/organizations", tag = "identity", operation_id = "organizations",
    responses((status = 200, body = OrganizationList), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn organizations(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Res<Json<OrganizationList>> {
    let pool = store(&state)?;
    let f = filter_for(&p, "organization.view");
    let items = db::list_organizations(pool, &f)
        .await?
        .into_iter()
        .map(|o| OrganizationDto {
            id: o.id,
            slug: o.slug,
            name: o.name,
            created_at: rfc3339(o.created_at),
            disabled: o.disabled_at.is_some(),
        })
        .collect();
    Ok(Json(OrganizationList { items }))
}

/// Creates an organisation. Needs `organization.create` (platform).
#[utoipa::path(post, path = "/api/v1/identity/organizations", tag = "identity", operation_id = "create_organization", request_body = NewOrganization,
    responses((status = 201, body = OrganizationDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn create_organization(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<NewOrganization>, JsonRejection>,
) -> Res<(StatusCode, Json<OrganizationDto>)> {
    allow_platform(&p, "organization.create")?;
    let req = json_body(body)?;
    if !valid_slug(&req.slug) {
        return Err(bad(
            "an organisation identifier is 2–40 lowercase letters, digits and hyphens, starting with a letter",
        ));
    }
    let pool = store(&state)?;
    let o = db::create_organization(pool, &req.slug, valid_name(&req.name)?)
        .await
        .map_err(map_conflict)?;
    audit_scoped(
        pool,
        &p,
        "organization.create",
        &o.slug,
        "success",
        Some(&Resource::organization(o.id)),
        &json!({ "id": o.id }),
    )
    .await?;
    Ok((
        StatusCode::CREATED,
        Json(OrganizationDto {
            id: o.id,
            slug: o.slug,
            name: o.name,
            created_at: rfc3339(o.created_at),
            disabled: false,
        }),
    ))
}

fn map_conflict(e: apelora_persistence::StoreError) -> ApiError {
    e.into()
}

#[derive(Deserialize, IntoParams)]
pub struct ProjectQuery {
    pub organization: Option<Uuid>,
}

fn project_dto(p: db::ProjectRow) -> ProjectDto {
    ProjectDto {
        id: p.id,
        organization_id: p.organization_id,
        organization_slug: p.organization_slug,
        slug: p.slug,
        name: p.name,
        created_at: rfc3339(p.created_at),
    }
}

/// Projects the caller may see, optionally of one organisation. Needs `project.view`.
#[utoipa::path(get, path = "/api/v1/identity/projects", tag = "identity", operation_id = "projects_list", params(ProjectQuery),
    responses((status = 200, body = ProjectList), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody), (status = 503, body = ErrorBody)))]
pub(super) async fn projects(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Query(q): Query<ProjectQuery>,
) -> Res<Json<ProjectList>> {
    let pool = store(&state)?;
    let f = filter_for(&p, "project.view");
    let items = db::list_projects(pool, &f)
        .await?
        .into_iter()
        .filter(|x| q.organization.is_none_or(|o| o == x.organization_id))
        .map(project_dto)
        .collect();
    Ok(Json(ProjectList { items }))
}

/// Creates a project in an organisation. Needs `project.create` on the organisation.
#[utoipa::path(post, path = "/api/v1/identity/organizations/{org}/projects", tag = "identity", operation_id = "create_project", request_body = NewProject,
    params(("org" = Uuid, Path, description = "Organisation id")),
    responses((status = 201, body = ProjectDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn create_project(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(org): Path<Uuid>,
    body: Result<Json<NewProject>, JsonRejection>,
) -> Res<(StatusCode, Json<ProjectDto>)> {
    let pool = store(&state)?;
    let res = resource_of(pool, &Scope::Organization { id: org })
        .await
        .map_err(|e| hide_missing(&p, e, "project.view", &Resource::platform()))?;
    allow(&p, "project.create", &res)?;
    let req = json_body(body)?;
    if !valid_slug(&req.slug) {
        return Err(bad(
            "a project identifier is 2–40 lowercase letters, digits and hyphens, starting with a letter",
        ));
    }
    let row = db::create_project(pool, org, &req.slug, valid_name(&req.name)?).await?;
    audit_scoped(
        pool,
        &p,
        "project.create",
        &row.slug,
        "success",
        Some(&Resource::project(org, row.id)),
        &json!({ "id": row.id }),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(project_dto(row))))
}

/// A missing scope looks the same as one the caller may not see, unless they could see everything.
fn hide_missing(p: &Principal, e: ApiError, perm: &str, res: &Resource) -> ApiError {
    if e.status == StatusCode::NOT_FOUND && allow(p, perm, res).is_err() {
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            format!("you may not do this ({perm})"),
        );
    }
    e
}

fn env_dto(e: db::EnvironmentRow) -> EnvironmentDto {
    let kind = EnvKind::parse(&e.kind).unwrap_or(EnvKind::Other);
    EnvironmentDto {
        id: e.id,
        project_id: e.project_id,
        slug: e.slug,
        name: e.name,
        kind,
        incus_project_config: apelora_auth::backend::incus_project_config(
            kind,
            &apelora_auth::backend::Limits::default(),
        ),
        incus_project: e.incus_project,
        incus_provisioned: false,
    }
}

async fn project_resource(
    pool: &apelora_persistence::Pool,
    id: Uuid,
) -> Res<(Resource, db::ProjectRow)> {
    let row = db::project(pool, id)
        .await?
        .ok_or_else(|| not_found("project"))?;
    Ok((Resource::project(row.organization_id, row.id), row))
}

/// Environments of a project. Needs `project.view` on it.
#[utoipa::path(get, path = "/api/v1/identity/projects/{project}/environments", tag = "identity", operation_id = "environments",
    params(("project" = Uuid, Path, description = "Project id")),
    responses((status = 200, body = EnvironmentList), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub(super) async fn environments(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(project): Path<Uuid>,
) -> Res<Json<EnvironmentList>> {
    let pool = store(&state)?;
    let Ok((res, _)) = project_resource(pool, project).await else {
        return Err(forbidden_view("project.view"));
    };
    allow(&p, "project.view", &res)?;
    Ok(Json(EnvironmentList {
        items: db::list_environments(pool, project)
            .await?
            .into_iter()
            .map(env_dto)
            .collect(),
    }))
}

fn forbidden_view(perm: &str) -> ApiError {
    ApiError::new(
        StatusCode::FORBIDDEN,
        "forbidden",
        format!("you may not do this ({perm})"),
    )
}

/// Creates an environment. The Incus project it maps to is derived from the names (tenants cannot
/// choose it); platform administrators may map an existing one. Needs `environment.manage`.
#[utoipa::path(post, path = "/api/v1/identity/projects/{project}/environments", tag = "identity", operation_id = "create_environment", request_body = NewEnvironment,
    params(("project" = Uuid, Path, description = "Project id")),
    responses((status = 201, body = EnvironmentDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn create_environment(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(project): Path<Uuid>,
    body: Result<Json<NewEnvironment>, JsonRejection>,
) -> Res<(StatusCode, Json<EnvironmentDto>)> {
    let pool = store(&state)?;
    let Ok((res, row)) = project_resource(pool, project).await else {
        return Err(forbidden_view("environment.manage"));
    };
    allow(&p, "environment.manage", &res)?;
    let req = json_body(body)?;
    if !valid_slug(&req.slug) {
        return Err(bad(
            "an environment identifier is 2–40 lowercase letters, digits and hyphens, starting with a letter",
        ));
    }
    let incus = match &req.incus_project {
        Some(name) => {
            allow_platform(&p, "cluster.manage")?;
            if !apelora_domain::is_valid_name(name) {
                return Err(bad("invalid Incus project name"));
            }
            name.clone()
        }
        None => {
            let derived = apelora_auth::backend::incus_project_name(&row.organization_slug, &row.slug, &req.slug).ok_or_else(|| bad("the names do not make a valid Incus project name (63 characters at most, starting with a letter)"))?;
            // The tenant namespace may not collide with projects the platform itself deploys into.
            if state.deploy_projects.contains(&derived) {
                return Err(conflict(
                    "reserved_name",
                    "that name is reserved for a platform project; choose another environment identifier",
                ));
            }
            derived
        }
    };
    let env = db::create_environment(
        pool,
        project,
        &req.slug,
        valid_name(&req.name)?,
        req.kind,
        Some(&incus),
    )
    .await?;
    audit_scoped(pool, &p, "environment.create", &env.slug, "success", Some(&res), &json!({ "id": env.id, "kind": req.kind.as_str(), "incusProject": incus, "chosenByPlatformAdmin": req.incus_project.is_some() })).await?;
    Ok((StatusCode::CREATED, Json(env_dto(env))))
}

// ---- users and groups -------------------------------------------------------------------------

/// People who have signed in, as far as the caller may see them: everyone for a platform
/// administrator, otherwise those with a role in the caller's scopes. Needs `user.view`.
#[utoipa::path(get, path = "/api/v1/identity/users", tag = "identity", operation_id = "users",
    responses((status = 200, body = UserList), (status = 403, body = ErrorBody)))]
pub(super) async fn users(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Res<Json<UserList>> {
    let pool = store(&state)?;
    let f = if allow_platform(&p, "user.manage").is_ok() {
        AccessFilter::All
    } else {
        filter_for(&p, "user.view")
    };
    Ok(Json(UserList {
        items: db::list_users(pool, &f)
            .await?
            .into_iter()
            .map(user_dto)
            .collect(),
    }))
}

#[derive(Deserialize, IntoParams)]
pub struct LookupQuery {
    /// Exact user name or e-mail address.
    pub q: String,
}

/// Finds one person by exact user name or e-mail so a role can be given to them. Someone who has
/// never signed in does not exist yet. Needs `role.assign` or `group.manage`.
#[utoipa::path(get, path = "/api/v1/identity/users/lookup", tag = "identity", operation_id = "lookup_user", params(LookupQuery),
    responses((status = 200, body = UserList), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody)))]
pub(super) async fn lookup_user(
    State(state): State<AppState>,
    Extension(_p): Extension<Principal>,
    Query(q): Query<LookupQuery>,
) -> Res<Json<UserList>> {
    if q.q.trim().chars().count() < 3 {
        return Err(bad(
            "type at least 3 characters of the exact user name or e-mail",
        ));
    }
    let found = db::find_user(store(&state)?, q.q.trim()).await?;
    Ok(Json(UserList {
        items: found.into_iter().map(user_dto).collect(),
    }))
}

/// Refuses, on the record, a change that would leave no enabled platform super admin.
async fn last_super_admin(
    pool: &apelora_persistence::Pool,
    p: &Principal,
    action: &str,
    target: &str,
    scope: Option<&Resource>,
    what: &str,
) -> ApiError {
    let recorded = audit_scoped(
        pool,
        p,
        action,
        target,
        "denied",
        scope,
        &json!({ "code": "last_super_admin" }),
    )
    .await;
    super::authz::audit_failed(action, recorded);
    conflict(
        "last_super_admin",
        format!(
            "this would leave no enabled platform super admin, so it cannot be {what}; give the role to someone else first"
        ),
    )
}

async fn set_disabled(
    state: &AppState,
    p: &Principal,
    id: Uuid,
    disabled: bool,
) -> Res<StatusCode> {
    allow_platform(p, "user.manage")?;
    let pool = store(state)?;
    if p.caller.as_ref().is_some_and(|c| c.id == id) && disabled {
        return Err(conflict(
            "self_lockout",
            "you cannot disable your own account",
        ));
    }
    let revoked = if disabled {
        let Some(revoked) = db::guarded_change(pool, db::SuperAdminChange::DisableUser(id)).await?
        else {
            return Err(last_super_admin(
                pool,
                p,
                "user.disable",
                &id.to_string(),
                None,
                "disabled",
            )
            .await);
        };
        revoked
    } else {
        db::set_user_disabled(pool, id, false).await?
    };
    audit_scoped(
        pool,
        p,
        if disabled {
            "user.disable"
        } else {
            "user.enable"
        },
        &id.to_string(),
        "success",
        None,
        &if disabled {
            json!({ "revokedSessions": revoked })
        } else {
            json!({})
        },
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Disables a person: their tokens and Apelora sessions stop working at once. Needs
/// `user.manage`.
#[utoipa::path(post, path = "/api/v1/identity/users/{user}/disable", tag = "identity", operation_id = "disable_user",
    params(("user" = Uuid, Path, description = "User id")),
    responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn disable_user(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(user): Path<Uuid>,
) -> Res<StatusCode> {
    set_disabled(&state, &p, user, true).await
}

/// Re-enables a person. Needs `user.manage`.
#[utoipa::path(post, path = "/api/v1/identity/users/{user}/enable", tag = "identity", operation_id = "enable_user",
    params(("user" = Uuid, Path, description = "User id")),
    responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub(super) async fn enable_user(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(user): Path<Uuid>,
) -> Res<StatusCode> {
    set_disabled(&state, &p, user, false).await
}

fn group_dto(g: db::GroupRow) -> GroupDto {
    GroupDto {
        id: g.id,
        organization_id: g.organization_id,
        name: g.name,
        external_name: g.external_name,
        description: g.description,
    }
}

fn group_resource(g: &db::GroupRow) -> Resource {
    match g.organization_id {
        Some(o) => Resource::organization(o),
        None => Resource::platform(),
    }
}

/// Groups the caller may see. Needs `user.view`.
#[utoipa::path(get, path = "/api/v1/identity/groups", tag = "identity", operation_id = "groups",
    responses((status = 200, body = GroupList), (status = 403, body = ErrorBody)))]
pub(super) async fn groups(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Res<Json<GroupList>> {
    let f = filter_for(&p, "user.view");
    Ok(Json(GroupList {
        items: db::list_groups(store(&state)?, &f)
            .await?
            .into_iter()
            .map(group_dto)
            .collect(),
    }))
}

/// Creates a group: an organisation's (needs `group.manage` there) or a platform group (platform
/// `group.manage`). Mapping a group to an identity-provider group name is platform-only.
#[utoipa::path(post, path = "/api/v1/identity/groups", tag = "identity", operation_id = "create_group", request_body = NewGroup,
    responses((status = 201, body = GroupDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn create_group(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<NewGroup>, JsonRejection>,
) -> Res<(StatusCode, Json<GroupDto>)> {
    let pool = store(&state)?;
    let req = json_body(body)?;
    let res = match req.organization_id {
        Some(o) => resource_of(pool, &Scope::Organization { id: o })
            .await
            .map_err(|_| forbidden_view("group.manage"))?,
        None => Resource::platform(),
    };
    allow(&p, "group.manage", &res)?;
    if req.external_name.is_some() {
        allow_platform(&p, "group.manage")?;
    }
    if req.description.chars().count() > 200 {
        return Err(bad("a description is at most 200 characters"));
    }
    let g = db::create_group(
        pool,
        req.organization_id,
        valid_name(&req.name)?,
        req.external_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty()),
        &req.description,
    )
    .await?;
    audit_scoped(
        pool,
        &p,
        "group.create",
        &g.name,
        "success",
        Some(&res),
        &json!({ "id": g.id, "externalName": g.external_name }),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(group_dto(g))))
}

async fn load_group(pool: &apelora_persistence::Pool, id: Uuid, perm: &str) -> Res<db::GroupRow> {
    db::group(pool, id)
        .await?
        .ok_or_else(|| forbidden_view(perm))
}

/// Members of a group. Needs `user.view` or `group.manage` on the group's organisation.
#[utoipa::path(get, path = "/api/v1/identity/groups/{group}/members", tag = "identity", operation_id = "group_members",
    params(("group" = Uuid, Path, description = "Group id")),
    responses((status = 200, body = UserList), (status = 403, body = ErrorBody)))]
pub(super) async fn group_members(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(group): Path<Uuid>,
) -> Res<Json<UserList>> {
    let pool = store(&state)?;
    let g = load_group(pool, group, "user.view").await?;
    allow_any(&p, &["user.view", "group.manage"], &group_resource(&g))?;
    Ok(Json(UserList {
        items: db::group_members(pool, group)
            .await?
            .into_iter()
            .map(user_dto)
            .collect(),
    }))
}

async fn change_member(
    state: &AppState,
    p: &Principal,
    group: Uuid,
    user: Uuid,
    add: bool,
) -> Res<StatusCode> {
    let pool = store(state)?;
    let g = load_group(pool, group, "group.manage").await?;
    let res = group_resource(&g);
    allow(p, "group.manage", &res)?;
    if g.external_name.is_some() {
        return Err(conflict(
            "managed_by_provider",
            "this group's members come from the identity provider; change them there",
        ));
    }
    db::user(pool, user)
        .await?
        .ok_or_else(|| not_found("user"))?;
    if add {
        db::add_group_member(pool, group, user, &p.actor_ref()).await?;
    } else if !db::change_keeping_a_super_admin(
        pool,
        db::SuperAdminChange::RemoveGroupMember { group, user },
    )
    .await?
    {
        return Err(last_super_admin(
            pool,
            p,
            "group.member.remove",
            &g.name,
            Some(&res),
            "removed from the group",
        )
        .await);
    }
    audit_scoped(
        pool,
        p,
        if add {
            "group.member.add"
        } else {
            "group.member.remove"
        },
        &g.name,
        "success",
        Some(&res),
        &json!({ "user": user }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Adds a person to a group. Needs `group.manage`.
#[utoipa::path(put, path = "/api/v1/identity/groups/{group}/members/{user}", tag = "identity", operation_id = "add_group_member",
    params(("group" = Uuid, Path, description = "Group id"), ("user" = Uuid, Path, description = "User id")),
    responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn add_member(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((group, user)): Path<(Uuid, Uuid)>,
) -> Res<StatusCode> {
    change_member(&state, &p, group, user, true).await
}

/// Removes a person from a group; their access through it ends on their next request. Needs `group.manage`.
#[utoipa::path(delete, path = "/api/v1/identity/groups/{group}/members/{user}", tag = "identity", operation_id = "remove_group_member",
    params(("group" = Uuid, Path, description = "Group id"), ("user" = Uuid, Path, description = "User id")),
    responses((status = 204), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn remove_member(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((group, user)): Path<(Uuid, Uuid)>,
) -> Res<StatusCode> {
    change_member(&state, &p, group, user, false).await
}

// ---- roles and permissions --------------------------------------------------------------------

fn builtin_dto(b: Builtin) -> RoleDto {
    let r = ResolvedRole::builtin(b);
    RoleDto {
        id: format!("builtin:{}", b.key()),
        reference: r.reference,
        name: b.name().into(),
        description: b.description().into(),
        builtin: true,
        permissions: r.permissions.into_iter().collect(),
        scopes: r.scopes,
    }
}
fn custom_dto(c: &CustomRole) -> RoleDto {
    let r = ResolvedRole::custom(c);
    RoleDto {
        id: format!("custom:{}", c.id),
        reference: r.reference,
        name: c.name.clone(),
        description: c.description.clone(),
        builtin: false,
        permissions: c.permissions.iter().cloned().collect(),
        scopes: r.scopes,
    }
}

/// The predefined and custom roles with their permissions.
#[utoipa::path(get, path = "/api/v1/identity/roles", tag = "identity", operation_id = "roles",
    responses((status = 200, body = RoleList), (status = 403, body = ErrorBody)))]
pub(super) async fn roles(
    State(state): State<AppState>,
    Extension(_p): Extension<Principal>,
) -> Res<Json<RoleList>> {
    let mut items: Vec<RoleDto> = Builtin::ALL.into_iter().map(builtin_dto).collect();
    items.extend(
        db::list_custom_roles(store(&state)?)
            .await?
            .iter()
            .map(custom_dto),
    );
    Ok(Json(RoleList { items }))
}

/// Defines a custom role from explicit permissions. Needs `role.manage` (platform), and you may
/// only include permissions you hold yourself.
#[utoipa::path(post, path = "/api/v1/identity/roles", tag = "identity", operation_id = "create_role", request_body = NewRole,
    responses((status = 201, body = RoleDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn create_role(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<NewRole>, JsonRejection>,
) -> Res<(StatusCode, Json<RoleDto>)> {
    allow_platform(&p, "role.manage")?;
    let req = json_body(body)?;
    let perms: BTreeSet<String> = req.permissions.iter().cloned().collect();
    validate_custom(&req.name, &perms).map_err(|e| bad(e.to_string()))?;
    let (engine, subject) = actor(&p);
    let missing: Vec<&String> = perms
        .iter()
        .filter(|x| {
            !engine
                .check(&subject, x, &Resource::platform(), now())
                .allowed
        })
        .collect();
    if !missing.is_empty() {
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "would_escalate",
            format!(
                "a role may only contain permissions you hold: {}",
                missing
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ));
    }
    if req.description.chars().count() > 200 {
        return Err(bad("a description is at most 200 characters"));
    }
    let pool = store(&state)?;
    let role = db::create_custom_role(
        pool,
        req.name.trim(),
        &req.description,
        &perms,
        &p.actor_ref(),
    )
    .await?;
    audit_scoped(
        pool,
        &p,
        "role.create",
        &role.name,
        "success",
        None,
        &json!({ "id": role.id, "permissions": role.permissions }),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(custom_dto(&role))))
}

/// Deletes a custom role that no assignment uses. Needs `role.manage`.
#[utoipa::path(delete, path = "/api/v1/identity/roles/{role}", tag = "identity", operation_id = "delete_role",
    params(("role" = Uuid, Path, description = "Custom role id")),
    responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn delete_role(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(role): Path<Uuid>,
) -> Res<StatusCode> {
    allow_platform(&p, "role.manage")?;
    let pool = store(&state)?;
    let name = db::custom_role(pool, role)
        .await?
        .map(|r| r.name)
        .unwrap_or_default();
    db::delete_custom_role(pool, role).await?;
    audit_scoped(
        pool,
        &p,
        "role.delete",
        &name,
        "success",
        None,
        &json!({ "id": role }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Every permission, its class and the narrowest scope at which a grant takes effect.
#[utoipa::path(get, path = "/api/v1/identity/permissions", tag = "identity", operation_id = "permissions",
    responses((status = 200, body = PermissionList)))]
pub(super) async fn permissions() -> Json<PermissionList> {
    Json(PermissionList {
        items: PERMISSIONS
            .iter()
            .map(|d| PermissionDto {
                id: d.id.into(),
                class: d.class,
                floor: d.floor,
                needs_approval: d.approval,
                description: d.description.into(),
            })
            .collect(),
    })
}

// ---- assignments ------------------------------------------------------------------------------

#[derive(Deserialize, IntoParams)]
#[serde(rename_all = "camelCase")]
pub struct AssignmentQuery {
    pub principal_type: Option<String>,
    pub principal_id: Option<Uuid>,
    #[serde(default)]
    pub include_revoked: bool,
}

/// Role assignments in the caller's scopes. Needs `user.view`.
#[utoipa::path(get, path = "/api/v1/identity/assignments", tag = "identity", operation_id = "assignments", params(AssignmentQuery),
    responses((status = 200, body = AssignmentList), (status = 403, body = ErrorBody)))]
pub(super) async fn assignments(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Query(q): Query<AssignmentQuery>,
) -> Res<Json<AssignmentList>> {
    let pool = store(&state)?;
    let f = filter_for(&p, "user.view");
    let principal = match (q.principal_type.as_deref(), q.principal_id) {
        (Some("user"), Some(id)) => Some(PrincipalRef::User { id }),
        (Some("group"), Some(id)) => Some(PrincipalRef::Group { id }),
        (Some("service"), Some(id)) => Some(PrincipalRef::Service { id }),
        (None, None) => None,
        _ => {
            return Err(bad(
                "principalType (user, group or service) and principalId go together",
            ));
        }
    };
    let customs = db::list_custom_roles(pool).await?;
    let items = db::list_assignments(pool, &f, principal, q.include_revoked)
        .await?
        .into_iter()
        .map(|r| assignment_dto(r, &customs))
        .collect();
    Ok(Json(AssignmentList { items }))
}

async fn resolve_role(pool: &apelora_persistence::Pool, r: &RoleRef) -> Res<ResolvedRole> {
    match r {
        RoleRef::Builtin { key } => Builtin::parse(key)
            .map(ResolvedRole::builtin)
            .ok_or_else(|| bad("unknown predefined role")),
        RoleRef::Custom { id } => db::custom_role(pool, *id)
            .await?
            .map(|c| ResolvedRole::custom(&c))
            .ok_or_else(|| bad("unknown custom role")),
    }
}

/// Checks that the principal exists and that a group of one organisation is not used in another's tree.
async fn check_principal(
    pool: &apelora_persistence::Pool,
    principal: &PrincipalRef,
    scope_res: &Resource,
) -> Res<()> {
    match principal {
        PrincipalRef::User { id } => {
            let u = db::user(pool, *id)
                .await?
                .ok_or_else(|| bad("that person has not signed in yet"))?;
            if u.disabled_at.is_some() {
                return Err(conflict("user_disabled", "that account is disabled"));
            }
        }
        PrincipalRef::Group { id } => {
            let g = db::group(pool, *id)
                .await?
                .ok_or_else(|| bad("unknown group"))?;
            if let Some(org) = g.organization_id
                && scope_res.organization != Some(org)
            {
                return Err(bad(
                    "an organisation's group can only be given roles inside that organisation",
                ));
            }
        }
        PrincipalRef::Service { id } => {
            let s = db::list_service_identities(pool)
                .await?
                .into_iter()
                .find(|s| s.id == *id)
                .ok_or_else(|| bad("unknown service identity"))?;
            if s.disabled_at.is_some()
                || ServiceKind::parse(&s.kind).is_none_or(|k| !k.may_call_api())
            {
                return Err(bad(
                    "only enabled automation identities can hold role assignments",
                ));
            }
        }
    }
    Ok(())
}

async fn assign_request<'a>(
    role: &'a ResolvedRole,
    scope: &'a Scope,
    res: &'a Resource,
    req: &NewAssignment,
    emergency: bool,
    expires: Option<i64>,
) -> AssignRequest<'a> {
    AssignRequest {
        role,
        scope,
        resource: res,
        principal: req.principal,
        emergency,
        expires_at: expires,
    }
}

fn validate_conditions(c: &Conditions) -> Res<()> {
    if c.environments.as_ref().is_some_and(Vec::is_empty) {
        return Err(bad(
            "an environment condition needs at least one environment kind",
        ));
    }
    Ok(())
}

/// Grants a role to a person, group or automation identity at a scope. Needs `role.assign` there
/// and every permission the role holds (no escalation); audited.
#[utoipa::path(post, path = "/api/v1/identity/assignments", tag = "identity", operation_id = "create_assignment", request_body = NewAssignment,
    responses((status = 201, body = AssignmentDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn create_assignment(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<NewAssignment>, JsonRejection>,
) -> Res<(StatusCode, Json<AssignmentDto>)> {
    let pool = store(&state)?;
    let req = json_body(body)?;
    validate_conditions(&req.conditions)?;
    let res = resource_of(pool, &req.scope).await.map_err(|e| {
        if e.status == StatusCode::NOT_FOUND {
            forbidden_view("role.assign")
        } else {
            e
        }
    })?;
    let role = resolve_role(pool, &req.role).await?;
    let expires = req.expires_at.as_deref().map(parse_time).transpose()?;
    if expires.is_some_and(|t| t <= OffsetDateTime::now_utc()) {
        return Err(bad("the expiry is in the past"));
    }
    let (engine, subject) = actor(&p);
    let areq = assign_request(
        &role,
        &req.scope,
        &res,
        &req,
        false,
        expires.map(|t| t.unix_timestamp()),
    )
    .await;
    if let Err(e) = check_assignment(&engine, &subject, &areq, now()) {
        audit_scoped(
            pool,
            &p,
            "role.assign",
            &role.name,
            "denied",
            Some(&res),
            &json!({ "reason": e.to_string(), "scope": req.scope }),
        )
        .await?;
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "assignment_refused",
            e.to_string(),
        ));
    }
    check_principal(pool, &req.principal, &res).await?;
    let id = db::insert_assignment(
        pool,
        &db::NewAssignment {
            principal: req.principal,
            role: req.role.clone(),
            scope: req.scope.clone(),
            conditions: req.conditions.clone(),
            kind: AssignmentKind::Standard,
            expires_at: expires,
            reason: req.reason.clone().filter(|r| !r.trim().is_empty()),
            created_by: p.actor_ref(),
        },
    )
    .await?;
    audit_scoped(pool, &p, "role.assign", &role.name, "success", Some(&res), &json!({ "assignment": id, "principal": req.principal, "scope": req.scope, "conditions": req.conditions, "expiresAt": req.expires_at })).await?;
    let customs = db::list_custom_roles(pool).await?;
    let row = db::assignment(pool, id)
        .await?
        .ok_or_else(|| not_found("assignment"))?;
    Ok((StatusCode::CREATED, Json(assignment_dto(row, &customs))))
}

/// Revokes an assignment; the person loses it on their next request. Needs `role.assign` in its
/// scope and every permission the role confers there (you can only take away what you could
/// grant). Refused when it would leave no enabled platform super admin.
#[utoipa::path(delete, path = "/api/v1/identity/assignments/{assignment}", tag = "identity", operation_id = "revoke_assignment",
    params(("assignment" = Uuid, Path, description = "Assignment id")),
    responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn revoke_assignment(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Res<StatusCode> {
    let pool = store(&state)?;
    let row = db::assignment(pool, id)
        .await?
        .filter(|r| r.revoked_at.is_none())
        .ok_or_else(|| forbidden_view("role.assign"))?;
    let res = resource_of(pool, &row.scope).await?;
    // Ending an emergency grant is allowed to whoever may grant emergency access.
    if row.kind == "emergency" {
        allow_any(&p, &["role.assign", "platform.emergency_access"], &res)?;
    } else {
        allow(&p, "role.assign", &res)?;
        let role = resolve_role(pool, &assignment_role(&row)).await?;
        let (engine, subject) = actor(&p);
        if let Err(e) = check_revocation(&engine, &subject, &role, &row.scope, &res, now()) {
            audit_scoped(
                pool,
                &p,
                "role.revoke",
                &id.to_string(),
                "denied",
                Some(&res),
                &json!({ "reason": e.to_string(), "role": role.name, "scope": row.scope }),
            )
            .await?;
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "revocation_refused",
                format!("you could not grant this role here, so you may not revoke it: {e}"),
            ));
        }
    }
    let actor_ref = p.actor_ref();
    if !db::change_keeping_a_super_admin(
        pool,
        db::SuperAdminChange::RevokeAssignment { id, by: &actor_ref },
    )
    .await?
    {
        return Err(last_super_admin(
            pool,
            &p,
            "role.revoke",
            &id.to_string(),
            Some(&res),
            "revoked",
        )
        .await);
    }
    audit_scoped(pool, &p, "role.revoke", &id.to_string(), "success", Some(&res), &json!({ "principalId": row.principal_id, "scope": row.scope, "role": row.builtin_role, "emergency": row.kind == "emergency" })).await?;
    Ok(StatusCode::NO_CONTENT)
}

fn effective_dtos(m: BTreeMap<&'static str, Vec<Grant>>) -> Vec<EffectivePermission> {
    m.into_iter()
        .filter_map(|(id, grants)| {
            permission(id).map(|d| EffectivePermission {
                permission: id.into(),
                class: d.class,
                description: d.description.into(),
                grants,
            })
        })
        .collect()
}

/// Shows what an assignment would give before it is saved: what the person can already do at the
/// scope, what they could do afterwards, which permissions would be inert, and whether you may
/// save it. Changes nothing. Needs `role.assign`.
#[utoipa::path(post, path = "/api/v1/identity/assignments/preview", tag = "identity", operation_id = "preview_assignment", request_body = NewAssignment,
    responses((status = 200, body = AssignmentPreview), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody)))]
pub(super) async fn preview_assignment(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<NewAssignment>, JsonRejection>,
) -> Res<Json<AssignmentPreview>> {
    let pool = store(&state)?;
    let req = json_body(body)?;
    validate_conditions(&req.conditions)?;
    let res = resource_of(pool, &req.scope).await.map_err(|e| {
        if e.status == StatusCode::NOT_FOUND {
            forbidden_view("role.assign")
        } else {
            e
        }
    })?;
    let (engine, actor_subject) = actor(&p);
    // Only people who could grant something at this scope may look at another person's access.
    if !engine
        .check(&actor_subject, "role.assign", &res, now())
        .allowed
    {
        return Err(forbidden_view("role.assign"));
    }
    let role = resolve_role(pool, &req.role).await?;
    let expires = req.expires_at.as_deref().map(parse_time).transpose()?;

    // The target's current assignments, plus the proposed one.
    let mut refs = vec![req.principal];
    let mut subject = Subject {
        mfa: true,
        ..Subject::default()
    };
    match req.principal {
        PrincipalRef::User { id } => {
            subject.user = Some(id);
            let groups = db::groups_of(pool, id, &BTreeSet::new()).await?;
            refs.extend(groups.iter().map(|g| PrincipalRef::Group { id: *g }));
            subject.groups = groups;
        }
        PrincipalRef::Group { id } => subject.groups = [id].into(),
        PrincipalRef::Service { id } => subject.service = Some(id),
    }
    let mut target = db::load_engine(pool, &refs).await?;
    let before = effective_dtos(target.effective(&subject, &res, now()));
    target.assignments.push(Assignment {
        id: Uuid::nil(),
        principal: req.principal,
        role: req.role.clone(),
        scope: req.scope.clone(),
        conditions: req.conditions.clone(),
        expires_at: expires.map(|t| t.unix_timestamp()),
        revoked: false,
        kind: AssignmentKind::Standard,
    });
    if let RoleRef::Custom { id } = &req.role
        && let Some(c) = db::custom_role(pool, *id).await?
    {
        target.custom_roles.insert(c.id, c);
    }
    let after_map = target.effective(&subject, &res, now());
    let before_ids: BTreeSet<&str> = before.iter().map(|e| e.permission.as_str()).collect();
    let added: Vec<String> = after_map
        .keys()
        .filter(|k| !before_ids.contains(**k))
        .map(|k| (*k).to_owned())
        .collect();
    let inert: Vec<String> = role
        .permissions
        .iter()
        .filter(|x| !permission(x).is_some_and(|d| apelora_auth::confers(&req.scope, d)))
        .cloned()
        .collect();
    let mut warnings = Vec::new();
    if req.conditions.require_mfa {
        warnings.push(
            "Takes effect only when the person signed in with multi-factor authentication."
                .to_owned(),
        );
    }
    if added
        .iter()
        .any(|a| permission(a).is_some_and(|d| d.approval))
    {
        warnings.push("Includes actions that always need another person's approval.".to_owned());
    }
    if !inert.is_empty() {
        warnings.push(format!(
            "{} permission(s) in this role have no effect at this scope.",
            inert.len()
        ));
    }
    let areq = assign_request(
        &role,
        &req.scope,
        &res,
        &req,
        false,
        expires.map(|t| t.unix_timestamp()),
    )
    .await;
    let verdict = check_assignment(&engine, &actor_subject, &areq, now());
    Ok(Json(AssignmentPreview {
        allowed: verdict.is_ok(),
        error: verdict.err().map(|e| e.to_string()),
        before,
        after: effective_dtos(after_map),
        added,
        inert,
        warnings,
    }))
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct EmergencyAccess {
    pub user_id: Uuid,
    /// A predefined role key, e.g. `fabric-administrator`.
    pub role: String,
    /// Why; at least 10 characters. Recorded in the audit trail.
    pub reason: String,
    /// 5–240 minutes.
    pub minutes: i64,
}

/// Grants a person a platform role for a short time, with a recorded reason (break-glass). You
/// cannot grant it to yourself; every request made with it is audited. Needs `platform.emergency_access`.
#[utoipa::path(post, path = "/api/v1/identity/emergency-access", tag = "identity", operation_id = "grant_emergency_access", request_body = EmergencyAccess,
    responses((status = 201, body = AssignmentDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn emergency_access(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<EmergencyAccess>, JsonRejection>,
) -> Res<(StatusCode, Json<AssignmentDto>)> {
    let pool = store(&state)?;
    let req = json_body(body)?;
    if req.reason.trim().chars().count() < 10 {
        return Err(bad("give a reason of at least 10 characters"));
    }
    if !(5..=240).contains(&req.minutes) {
        return Err(bad("emergency access lasts 5–240 minutes"));
    }
    if p.caller.as_ref().is_some_and(|c| c.id == req.user_id) {
        return Err(conflict(
            "self_grant",
            "you cannot grant yourself emergency access; ask another administrator",
        ));
    }
    let builtin = Builtin::parse(&req.role).ok_or_else(|| bad("unknown predefined role"))?;
    let role = ResolvedRole::builtin(builtin);
    let expires = OffsetDateTime::now_utc() + time::Duration::minutes(req.minutes);
    let scope = Scope::Platform;
    let res = Resource::platform();
    let principal = PrincipalRef::User { id: req.user_id };
    let (engine, subject) = actor(&p);
    let areq = AssignRequest {
        role: &role,
        scope: &scope,
        resource: &res,
        principal,
        emergency: true,
        expires_at: Some(expires.unix_timestamp()),
    };
    if let Err(e) = check_assignment(&engine, &subject, &areq, now()) {
        audit_scoped(
            pool,
            &p,
            "emergency_access.grant",
            &req.user_id.to_string(),
            "denied",
            None,
            &json!({ "reason": e.to_string(), "role": req.role }),
        )
        .await?;
        return Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "assignment_refused",
            e.to_string(),
        ));
    }
    check_principal(pool, &principal, &res).await?;
    let id = db::insert_assignment(
        pool,
        &db::NewAssignment {
            principal,
            role: RoleRef::Builtin {
                key: builtin.key().into(),
            },
            scope,
            conditions: Conditions::default(),
            kind: AssignmentKind::Emergency,
            expires_at: Some(expires),
            reason: Some(req.reason.trim().to_owned()),
            created_by: p.actor_ref(),
        },
    )
    .await?;
    audit_scoped(pool, &p, "emergency_access.grant", &req.user_id.to_string(), "success", None, &json!({ "assignment": id, "role": req.role, "reason": req.reason.trim(), "expiresAt": rfc3339(expires) })).await?;
    let customs = db::list_custom_roles(pool).await?;
    let row = db::assignment(pool, id)
        .await?
        .ok_or_else(|| not_found("assignment"))?;
    Ok((StatusCode::CREATED, Json(assignment_dto(row, &customs))))
}

// ---- service identities -----------------------------------------------------------------------

/// Registered service identities (the credentials Apelora uses for Incus and MAAS, and automation
/// identities that call this API). Needs `service_identity.manage`.
#[utoipa::path(get, path = "/api/v1/identity/service-identities", tag = "identity", operation_id = "service_identities",
    responses((status = 200, body = ServiceIdentityList), (status = 403, body = ErrorBody)))]
pub(super) async fn service_identities(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Res<Json<ServiceIdentityList>> {
    allow_platform(&p, "service_identity.manage")?;
    Ok(Json(ServiceIdentityList {
        items: db::list_service_identities(store(&state)?)
            .await?
            .into_iter()
            .map(service_dto)
            .collect(),
    }))
}

/// Registers a service identity. Outbound kinds (workload, network, storage, provisioning) record
/// only the name of a credential held in the secret store; automation identities get an API token,
/// shown once. Needs `service_identity.manage`.
#[utoipa::path(post, path = "/api/v1/identity/service-identities", tag = "identity", operation_id = "create_service_identity", request_body = NewServiceIdentity,
    responses((status = 201, body = ServiceIdentityCreated), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn create_service_identity(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<NewServiceIdentity>, JsonRejection>,
) -> Res<Response> {
    allow_platform(&p, "service_identity.manage")?;
    let req = json_body(body)?;
    let name = valid_name(&req.name)?;
    // A credential reference is a name, never the secret itself.
    if let Some(r) = &req.credential_ref
        && (r.len() > 80
            || r.is_empty()
            || !r
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/')))
    {
        return Err(bad(
            "a credential reference is the name of a secret in the secret store (letters, digits and . _ - /), not the secret",
        ));
    }
    if req.kind == ServiceKind::Automation && req.credential_ref.is_some() {
        return Err(bad(
            "automation identities authenticate with an API token, not a stored credential",
        ));
    }
    if !req.restrictions.is_object() && !req.restrictions.is_null() {
        return Err(bad(
            "restrictions is an object such as {\"projects\": [\"acme-prod\"]}",
        ));
    }
    // Refused, not shortened: whoever asked for longer should know they did not get it.
    let expires = if req.kind == ServiceKind::Automation {
        let days = super::admin::token_lifetime_days(req.expires_in_days).map_err(bad)?;
        Some(OffsetDateTime::now_utc() + time::Duration::days(days))
    } else {
        None
    };
    let restrictions = if req.restrictions.is_null() {
        json!({})
    } else {
        req.restrictions.clone()
    };
    let pool = store(&state)?;
    let (row, token) = db::create_service_identity(
        pool,
        name,
        req.kind.as_str(),
        req.credential_ref.as_deref(),
        &restrictions,
        expires,
        &p.actor_ref(),
    )
    .await?;
    audit_scoped(pool, &p, "service_identity.create", &row.name, "success", None, &json!({ "id": row.id, "kind": row.kind, "credentialRef": row.credential_ref, "restrictions": row.restrictions })).await?;
    let mut res = (
        StatusCode::CREATED,
        Json(ServiceIdentityCreated {
            token,
            identity: service_dto(row),
        }),
    )
        .into_response();
    res.headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    Ok(res)
}

async fn set_service_disabled(
    state: &AppState,
    p: &Principal,
    id: Uuid,
    disabled: bool,
) -> Res<StatusCode> {
    allow_platform(p, "service_identity.manage")?;
    let pool = store(state)?;
    db::set_service_identity_disabled(pool, id, disabled).await?;
    audit_scoped(
        pool,
        p,
        if disabled {
            "service_identity.disable"
        } else {
            "service_identity.enable"
        },
        &id.to_string(),
        "success",
        None,
        &json!({}),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Disables a service identity at once. Needs `service_identity.manage`.
#[utoipa::path(post, path = "/api/v1/identity/service-identities/{id}/disable", tag = "identity", operation_id = "disable_service_identity",
    params(("id" = Uuid, Path, description = "Service identity id")), responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub(super) async fn disable_service(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Res<StatusCode> {
    set_service_disabled(&state, &p, id, true).await
}

/// Re-enables a service identity. Needs `service_identity.manage`.
#[utoipa::path(post, path = "/api/v1/identity/service-identities/{id}/enable", tag = "identity", operation_id = "enable_service_identity",
    params(("id" = Uuid, Path, description = "Service identity id")), responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub(super) async fn enable_service(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Res<StatusCode> {
    set_service_disabled(&state, &p, id, false).await
}

// ---- access policies --------------------------------------------------------------------------

fn policy_dto(r: db::PolicyRow) -> PolicyDto {
    PolicyDto {
        id: r.id,
        name: r.name,
        kind: r.kind,
        parameters: r.parameters,
        enabled: r.enabled,
        updated_by: r.updated_by,
        updated_at: rfc3339(r.updated_at),
    }
}

/// Access policies (MFA and approval rules). Needs `access_policy.manage` or `audit.read`.
#[utoipa::path(get, path = "/api/v1/identity/policies", tag = "identity", operation_id = "policies",
    responses((status = 200, body = PolicyList), (status = 403, body = ErrorBody)))]
pub(super) async fn policies(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Res<Json<PolicyList>> {
    allow_any(
        &p,
        &["access_policy.manage", "audit.read"],
        &Resource::platform(),
    )?;
    Ok(Json(PolicyList {
        items: db::list_policies(store(&state)?)
            .await?
            .into_iter()
            .map(policy_dto)
            .collect(),
    }))
}

/// Creates or updates a policy by name. `mfa`: `{"environments": ["production"], "infrastructure": true}`.
/// `approval`: `{"actions": ["cluster.evacuate"]}` (extra actions that need approval where the API
/// already asks for it). Needs `access_policy.manage`.
#[utoipa::path(put, path = "/api/v1/identity/policies", tag = "identity", operation_id = "save_policy", request_body = SavePolicy,
    responses((status = 200, body = PolicyDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody)))]
pub(super) async fn save_policy(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<SavePolicy>, JsonRejection>,
) -> Res<Json<PolicyDto>> {
    allow_platform(&p, "access_policy.manage")?;
    let req = json_body(body)?;
    let name = valid_name(&req.name)?;
    match req.kind.as_str() {
        "mfa" => {
            let envs = req
                .parameters
                .get("environments")
                .map(|v| {
                    v.as_array().map(|a| {
                        a.iter()
                            .all(|x| x.as_str().and_then(EnvKind::parse).is_some())
                    })
                })
                .unwrap_or(Some(true));
            if envs != Some(true)
                || req
                    .parameters
                    .get("infrastructure")
                    .is_some_and(|v| !v.is_boolean())
            {
                return Err(bad(
                    "an mfa policy has environments (production, staging, development, other) and/or infrastructure (true/false)",
                ));
            }
        }
        "approval" => {
            let ok = req
                .parameters
                .get("actions")
                .and_then(Value::as_array)
                .is_some_and(|a| {
                    !a.is_empty() && a.iter().all(|x| x.as_str().and_then(permission).is_some())
                });
            if !ok {
                return Err(bad(
                    "an approval policy lists actions (permission ids) that need approval",
                ));
            }
        }
        _ => return Err(bad("a policy kind is mfa or approval")),
    }
    let pool = store(&state)?;
    let row = db::save_policy(
        pool,
        name,
        &req.kind,
        &req.parameters,
        req.enabled,
        &p.actor_ref(),
    )
    .await?;
    audit_scoped(
        pool,
        &p,
        "access_policy.save",
        &row.name,
        "success",
        None,
        &json!({ "kind": row.kind, "parameters": row.parameters, "enabled": row.enabled }),
    )
    .await?;
    Ok(Json(policy_dto(row)))
}

/// Deletes a policy. Needs `access_policy.manage`.
#[utoipa::path(delete, path = "/api/v1/identity/policies/{id}", tag = "identity", operation_id = "delete_policy",
    params(("id" = Uuid, Path, description = "Policy id")), responses((status = 204), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody)))]
pub(super) async fn delete_policy(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Res<StatusCode> {
    allow_platform(&p, "access_policy.manage")?;
    let pool = store(&state)?;
    db::delete_policy(pool, id).await?;
    audit_scoped(
        pool,
        &p,
        "access_policy.delete",
        &id.to_string(),
        "success",
        None,
        &json!({}),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---- audit ------------------------------------------------------------------------------------

#[derive(Deserialize, IntoParams)]
pub struct AuditQuery {
    /// Only actions starting with this, e.g. `role.` or `emergency_access.`.
    pub action: Option<String>,
    /// `success`, `denied` or `failed`.
    pub outcome: Option<String>,
    /// Only events older than this id (paging).
    pub before: Option<i64>,
    pub limit: Option<i64>,
}

/// Audit history the caller may read, filtered and paged. Needs `audit.read`.
#[utoipa::path(get, path = "/api/v1/identity/audit", tag = "identity", operation_id = "identity_audit", params(AuditQuery),
    responses((status = 200, body = AuditList), (status = 403, body = ErrorBody)))]
pub(super) async fn audit_history(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Query(q): Query<AuditQuery>,
) -> Res<Json<AuditList>> {
    let f = filter_for(&p, "audit.read");
    if f.is_empty() {
        return Err(forbidden_view("audit.read"));
    }
    if q.outcome
        .as_deref()
        .is_some_and(|o| !matches!(o, "success" | "denied" | "failed"))
    {
        return Err(bad("outcome is success, denied or failed"));
    }
    let rows = db::audit_history(
        store(&state)?,
        &f,
        q.action.as_deref().filter(|a| a.len() <= 64),
        q.outcome.as_deref(),
        q.before,
        q.limit.unwrap_or(100),
    )
    .await?;
    Ok(Json(AuditList {
        items: rows.into_iter().map(AuditItem::from_row).collect(),
    }))
}

// ---- approvals --------------------------------------------------------------------------------

fn approval_error(e: apelora_auth::ApprovalError) -> ApiError {
    use apelora_auth::ApprovalError as E;
    let (status, code) = match e {
        E::SameAsRequester => (StatusCode::FORBIDDEN, "self_approval"),
        E::ApproverNotPermitted | E::RequesterOnly => (StatusCode::FORBIDDEN, "forbidden"),
        E::Expired => (StatusCode::GONE, "approval_expired"),
        E::NotPending | E::NotApproved | E::Mismatch => (StatusCode::CONFLICT, "approval_invalid"),
    };
    ApiError::new(status, code, e.to_string())
}

/// Requests and approvals: your own, plus every open one if you may approve. Needs `approval.request` or `approval.grant`.
#[utoipa::path(get, path = "/api/v1/approvals", tag = "identity", operation_id = "approvals",
    responses((status = 200, body = ApprovalList), (status = 403, body = ErrorBody)))]
pub(super) async fn approvals(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Res<Json<ApprovalList>> {
    let pool = store(&state)?;
    let rows = if allow_platform(&p, "approval.grant").is_ok() {
        db::list_approvals(pool, None, false).await?
    } else {
        // Your own requests, and open requests for interactive access to applications you may approve for.
        let me = p.actor_ref();
        let mut out = db::list_approvals(pool, Some(&me), false).await?;
        for row in
            db::list_open_approvals_for(pool, &["instance.console", "instance.desktop"]).await?
        {
            if row.requester == me {
                continue;
            }
            let approvable = match super::sessions::approval_resource(&state, &row.target).await? {
                Some(res) => allow(&p, "approval.grant", &res).is_ok(),
                None => false,
            };
            if approvable {
                out.push(row);
            }
        }
        out.sort_by_key(|a| std::cmp::Reverse(a.created_at));
        out
    };
    Ok(Json(ApprovalList {
        items: rows.into_iter().map(approval_dto).collect(),
    }))
}

/// Asks for approval of one risky action on one target. You must be allowed to do the action
/// yourself; someone else with `approval.grant` then decides. Needs `approval.request`.
#[utoipa::path(post, path = "/api/v1/approvals", tag = "identity", operation_id = "request_approval", request_body = NewApproval,
    responses((status = 201, body = ApprovalDto), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody)))]
pub(super) async fn request_approval(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    body: Result<Json<NewApproval>, JsonRejection>,
) -> Res<(StatusCode, Json<ApprovalDto>)> {
    let req = json_body(body)?;
    let pool = store(&state)?;
    let (engine, _) = actor(&p);
    let engine = match &p.caller {
        Some(c) => c.engine.clone(),
        None => engine,
    };
    let mut engine = engine;
    engine.policies = db::load_policies(pool).await?;
    if apelora_auth::is_interactive_access(&req.action) {
        // Access to one module: judged on its application, not on the platform.
        // An application that does not exist and one the caller cannot see answer alike.
        let unknown = || {
            bad(
                "a target looks like instance:<application>/<instance>, of an application you can see",
            )
        };
        let Some(res) = super::sessions::approval_resource(&state, req.target.trim()).await? else {
            return Err(unknown());
        };
        if allow(&p, "application.view", &res).is_err() {
            return Err(unknown());
        }
        // A person's waiting requests are capped, so requests cannot bury other people's.
        if db::count_open_approvals(pool, &p.actor_ref()).await? >= MAX_OPEN_REQUESTS {
            return Err(ApiError::new(
                StatusCode::TOO_MANY_REQUESTS,
                "too_many_requests",
                "you already have many requests waiting; cancel or wait for some",
            ));
        }
        allow(&p, "approval.request", &res)?;
        allow(&p, &req.action, &res)?;
        if !engine.needs_approval_on(&req.action, &res) {
            return Err(bad("that action does not need approval here"));
        }
    } else {
        allow_platform(&p, "approval.request")?;
        if !engine.needs_approval(&req.action) {
            return Err(bad("that action does not need approval"));
        }
        allow_platform(&p, &req.action)?;
    }
    let target = req.target.trim();
    if target.is_empty()
        || target.len() > 200
        || !target
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '.' | '_' | '-' | '/'))
    {
        return Err(bad(
            "a target looks like machine:rack1-07 or evacuate:node-3",
        ));
    }
    if req.reason.chars().count() > 300 {
        return Err(bad("a reason is at most 300 characters"));
    }
    let row =
        db::request_approval(pool, &req.action, target, &p.actor_ref(), req.reason.trim()).await?;
    audit_scoped(
        pool,
        &p,
        "approval.request",
        target,
        "success",
        None,
        &json!({ "id": row.id, "action": req.action, "reason": req.reason.trim() }),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(approval_dto(row))))
}

async fn decide(
    state: &AppState,
    p: &Principal,
    id: Uuid,
    approve: bool,
) -> Res<Json<ApprovalDto>> {
    let pool = store(state)?;
    // Interactive access is approved by someone who may approve on that application; the rest
    // by a platform approver.
    let scoped = match db::approval(pool, id).await? {
        Some(row) if apelora_auth::is_interactive_access(&row.action) => {
            super::sessions::approval_resource(state, &row.target).await?
        }
        _ => None,
    };
    match &scoped {
        Some(res) => allow(p, "approval.grant", res)?,
        None => allow_platform(p, "approval.grant")?,
    }
    let outcome = db::decide_approval(pool, id, &p.actor_ref(), true, approve).await?;
    match outcome {
        Ok(row) => {
            audit_scoped(
                pool,
                p,
                if approve {
                    "approval.grant"
                } else {
                    "approval.reject"
                },
                &row.target,
                "success",
                None,
                &json!({ "id": row.id, "action": row.action, "requester": row.requester }),
            )
            .await?;
            Ok(Json(approval_dto(row)))
        }
        Err(e) => {
            audit_scoped(
                pool,
                p,
                if approve {
                    "approval.grant"
                } else {
                    "approval.reject"
                },
                &id.to_string(),
                "denied",
                None,
                &json!({ "reason": e.to_string() }),
            )
            .await?;
            Err(approval_error(e))
        }
    }
}

/// Approves a request made by someone else. Needs `approval.grant`. The approval is good for 30 minutes, once.
#[utoipa::path(post, path = "/api/v1/approvals/{id}/approve", tag = "identity", operation_id = "approve_request",
    params(("id" = Uuid, Path, description = "Approval id")),
    responses((status = 200, body = ApprovalDto), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody), (status = 410, body = ErrorBody)))]
pub(super) async fn approve_request(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Res<Json<ApprovalDto>> {
    decide(&state, &p, id, true).await
}

/// Rejects a request. Needs `approval.grant`.
#[utoipa::path(post, path = "/api/v1/approvals/{id}/reject", tag = "identity", operation_id = "reject_request",
    params(("id" = Uuid, Path, description = "Approval id")),
    responses((status = 200, body = ApprovalDto), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn reject_request(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Res<Json<ApprovalDto>> {
    decide(&state, &p, id, false).await
}

/// Withdraws your own pending request. Needs `approval.request`.
#[utoipa::path(post, path = "/api/v1/approvals/{id}/cancel", tag = "identity", operation_id = "cancel_request",
    params(("id" = Uuid, Path, description = "Approval id")),
    responses((status = 200, body = ApprovalDto), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn cancel_request(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Res<Json<ApprovalDto>> {
    let pool = store(&state)?;
    match db::cancel_approval(pool, id, &p.actor_ref()).await? {
        Ok(row) => {
            audit_scoped(
                pool,
                &p,
                "approval.cancel",
                &row.target,
                "success",
                None,
                &json!({ "id": row.id }),
            )
            .await?;
            Ok(Json(approval_dto(row)))
        }
        Err(e) => Err(approval_error(e)),
    }
}
