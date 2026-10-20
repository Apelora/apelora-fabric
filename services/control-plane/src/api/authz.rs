// SPDX-License-Identifier: AGPL-3.0-only
//! Authentication results and authorisation checks for the API.
//!
//! Three kinds of caller reach the API:
//! - a **person** with an OIDC access token (`Caller` of kind `User`),
//! - an **automation** service identity with an `aps_…` token (`Caller` of kind `Service`),
//! - a **legacy API token** (`apl_…`), platform-wide and role-based, kept so existing set-ups
//!   keep working until `APELORA_LEGACY_TOKENS=disabled`.
//!
//! Every handler authorises with [`allow`] (or `require` for platform-level routes); a route
//! gate in front of the handlers refuses any route that is not listed in [`GATES`] for a person
//! or service, so a new route is denied until someone decides who may use it.

use apelora_auth::{Engine, Resource, Subject};
use apelora_domain::{Permission, Role};
use axum::extract::{MatchedPath, Request, State};
use axum::http::{HeaderMap, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use uuid::Uuid;

use super::{ApiError, AppState, Auth, Principal};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallerKind {
    User,
    Service,
}

/// An authenticated person or automation identity with its role assignments loaded.
#[derive(Debug, Clone)]
pub struct Caller {
    pub kind: CallerKind,
    pub id: Uuid,
    pub subject: Subject,
    pub engine: Engine,
    /// Holds at least one live emergency-access grant: every request is audited.
    pub emergency: bool,
}

pub fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

impl Caller {
    pub fn check(&self, permission: &str, resource: &Resource) -> apelora_auth::Decision {
        self.engine
            .check(&self.subject, permission, resource, now())
    }
}

impl Principal {
    /// A stable identifier for audit trails, approvals and idempotency keys.
    pub fn actor_ref(&self) -> String {
        match &self.caller {
            Some(c) => match c.kind {
                CallerKind::User => format!("user:{}", c.id),
                CallerKind::Service => format!("service:{}", c.id),
            },
            None if self.role == Some(Role::FabricAdmin) && self.name == "development" => {
                "development".into()
            }
            None => format!("token:{}", self.name),
        }
    }

    pub fn actor_type(&self) -> &'static str {
        match &self.caller {
            Some(c) if c.kind == CallerKind::User => "user",
            Some(_) => "service",
            None if self.name == "development" => "development",
            None => "token",
        }
    }
}

fn forbidden(permission: &str, mfa: bool) -> ApiError {
    if mfa {
        ApiError::new(
            StatusCode::FORBIDDEN,
            "mfa_required",
            format!("{permission} needs a sign-in with multi-factor authentication"),
        )
    } else {
        ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            format!("you may not do this ({permission})"),
        )
    }
}

/// The legacy permission a role-based token needs for a given permission id.
fn legacy_requirement(permission: &str) -> Option<Permission> {
    use Permission::*;
    Some(match permission {
        "cluster.view"
        | "hardware.view"
        | "organization.view"
        | "project.view"
        | "application.view"
        | "application.logs.read"
        | "application.metrics.read"
        | "user.view" => ReadInventory,
        "audit.read" => ReadAudit,
        "application.design" | "application.create" => DesignApplications,
        "application.deploy" | "application.operate" | "application.delete" | "instance.manage" => {
            Deploy
        }
        "token.manage" => ManageTokens,
        "cluster.manage"
        | "cluster.evacuate"
        | "cluster.evacuate_unsafe"
        | "cluster.sensitive_change"
        | "network.manage"
        | "storage.manage"
        | "machine.enrol"
        | "hardware.commission"
        | "hardware.deploy"
        | "hardware.power"
        | "hardware.delete"
        | "approval.request"
        | "approval.grant" => ManageInfrastructure,
        // Identity administration and everything else platform-security: legacy administrators only.
        "organization.create"
        | "organization.update"
        | "project.create"
        | "project.update"
        | "project.delete"
        | "environment.manage"
        | "group.manage"
        | "role.assign"
        | "role.manage"
        | "service_identity.manage"
        | "access_policy.manage"
        | "platform.security.manage"
        | "platform.emergency_access"
        | "user.manage" => ManageTokens,
        _ => return None,
    })
}

/// The platform-level permission id a legacy `Permission` stands for when a person or service
/// identity calls a platform-level route.
fn platform_permission(p: Permission) -> &'static str {
    match p {
        Permission::ReadInventory => "cluster.view",
        Permission::ReadAudit => "audit.read",
        Permission::ManageTokens => "token.manage",
        Permission::ManageInfrastructure => "cluster.manage",
        Permission::DesignApplications => "application.design",
        Permission::Deploy => "application.deploy",
    }
}

/// May the caller use `permission` on `resource`? Deny by default.
pub(super) fn allow(p: &Principal, permission: &str, resource: &Resource) -> Result<(), ApiError> {
    match (&p.caller, p.role) {
        (Some(c), _) => {
            let d = c.check(permission, resource);
            if d.allowed {
                Ok(())
            } else {
                Err(forbidden(
                    permission,
                    d.denial == Some(apelora_auth::Denial::MfaRequired),
                ))
            }
        }
        (None, Some(role)) => match legacy_requirement(permission) {
            Some(need) if role.allows(need) => Ok(()),
            _ => Err(forbidden(permission, false)),
        },
        (None, None) => Err(forbidden(permission, false)),
    }
}

/// [`allow`] on the whole platform.
pub(super) fn allow_platform(p: &Principal, permission: &str) -> Result<(), ApiError> {
    allow(p, permission, &Resource::platform())
}

/// Like [`allow`], satisfied by any one of the permissions.
pub(super) fn allow_any(
    p: &Principal,
    permissions: &[&str],
    resource: &Resource,
) -> Result<(), ApiError> {
    permissions
        .iter()
        .find_map(|perm| allow(p, perm, resource).ok())
        .ok_or_else(|| forbidden(permissions.first().copied().unwrap_or("access"), false))
}

/// Platform-level check by legacy permission (the way the older routes name what they need).
pub(super) fn require(p: &Principal, perm: Permission) -> Result<(), ApiError> {
    match (&p.caller, p.role) {
        (Some(_), _) => allow(p, platform_permission(perm), &Resource::platform()),
        (None, Some(role)) if role.allows(perm) => Ok(()),
        (None, Some(role)) => Err(ApiError::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            format!("role {} may not {perm:?}", role.as_str()),
        )),
        (None, None) => Err(forbidden("access", false)),
    }
}

/// Which resources of a kind may the caller reach with `permission`? Used to scope queries.
pub(super) fn filter_for(p: &Principal, permission: &str) -> apelora_auth::AccessFilter {
    match (&p.caller, p.role) {
        (Some(c), _) => c.engine.filter(&c.subject, permission, now()),
        (None, Some(role))
            if legacy_requirement(permission).is_some_and(|need| role.allows(need)) =>
        {
            apelora_auth::AccessFilter::All
        }
        _ => apelora_auth::AccessFilter::Limited {
            organizations: vec![],
            projects: vec![],
            applications: vec![],
        },
    }
}

/// Does the caller hold the permission at any scope (ignoring conditions)? The route gate's test.
pub(super) fn holds_anywhere(p: &Principal, permission: &str) -> bool {
    match (&p.caller, p.role) {
        (Some(c), _) => c.engine.holds_anywhere(&c.subject, permission, now()),
        (None, Some(role)) => legacy_requirement(permission).is_some_and(|need| role.allows(need)),
        (None, None) => false,
    }
}

/// Routes and the permissions that open them. A person or service identity needs at least one
/// of the listed permissions at some scope to get past the gate; the handler then checks the
/// exact resource. An empty list means any signed-in caller. **A route that is not here is
/// refused.** Legacy tokens are checked by their handlers as before.
pub(super) const GATES: &[(&str, &str, &[&str])] = &[
    ("GET", "/api/v1/whoami", &[]),
    (
        "GET",
        "/api/v1/applications/{name}/access",
        &["application.view"],
    ),
    (
        "POST",
        "/api/v1/applications/{name}/instances/{instance}/sessions",
        &["instance.console", "instance.desktop"],
    ),
    // The handlers show a person their own sessions, and those of applications they operate.
    (
        "GET",
        "/api/v1/sessions",
        &[
            "instance.console",
            "instance.desktop",
            "application.operate",
        ],
    ),
    (
        "DELETE",
        "/api/v1/sessions/{id}",
        &[
            "instance.console",
            "instance.desktop",
            "application.operate",
        ],
    ),
    ("GET", "/api/v1/cluster", &["cluster.view"]),
    ("GET", "/api/v1/instances", &["cluster.view"]),
    ("POST", "/api/v1/instances", &["instance.manage"]),
    (
        "POST",
        "/api/v1/instances/{project}/{name}/state",
        &["instance.manage"],
    ),
    (
        "DELETE",
        "/api/v1/instances/{project}/{name}",
        &["instance.manage"],
    ),
    ("GET", "/api/v1/applications", &["application.view"]),
    ("POST", "/api/v1/applications", &["application.create"]),
    ("GET", "/api/v1/applications/{name}", &["application.view"]),
    (
        "PATCH",
        "/api/v1/applications/{name}",
        &["application.design"],
    ),
    (
        "GET",
        "/api/v1/applications/{name}/draft",
        &["application.view"],
    ),
    (
        "PUT",
        "/api/v1/applications/{name}/draft",
        &["application.design"],
    ),
    (
        "GET",
        "/api/v1/applications/{name}/versions",
        &["application.view"],
    ),
    (
        "POST",
        "/api/v1/applications/{name}/versions",
        &["application.design"],
    ),
    (
        "GET",
        "/api/v1/applications/{name}/versions/{revision}",
        &["application.view"],
    ),
    (
        "POST",
        "/api/v1/applications/{name}/archive",
        &["application.delete"],
    ),
    (
        "POST",
        "/api/v1/applications/{name}/restore",
        &["application.delete"],
    ),
    (
        "GET",
        "/api/v1/applications/{name}/operations",
        &["application.view"],
    ),
    (
        "POST",
        "/api/v1/applications/{name}/operations",
        &["application.operate"],
    ),
    (
        "GET",
        "/api/v1/applications/{name}/operations/{id}",
        &["application.view"],
    ),
    (
        "GET",
        "/api/v1/applications/{name}/events",
        &["application.view"],
    ),
    (
        "GET",
        "/api/v1/applications/{name}/volumes",
        &["application.view"],
    ),
    (
        "DELETE",
        "/api/v1/applications/{name}/volumes/{volume}",
        &["application.delete"],
    ),
    (
        "PATCH",
        "/api/v1/applications/{name}/volumes/{volume}",
        &["application.deploy"],
    ),
    (
        "POST",
        "/api/v1/applications/{name}/plan",
        &["application.design", "application.deploy"],
    ),
    (
        "POST",
        "/api/v1/applications/{name}/deployments",
        &["application.deploy"],
    ),
    (
        "GET",
        "/api/v1/applications/{name}/status",
        &["application.view"],
    ),
    (
        "GET",
        "/api/v1/jobs",
        &["cluster.view", "application.view", "instance.manage"],
    ),
    (
        "GET",
        "/api/v1/jobs/{id}",
        &["cluster.view", "application.view", "instance.manage"],
    ),
    ("GET", "/api/v1/tokens", &["token.manage"]),
    ("POST", "/api/v1/tokens", &["token.manage"]),
    ("DELETE", "/api/v1/tokens/{id}", &["token.manage"]),
    ("GET", "/api/v1/system", &["cluster.view"]),
    (
        "GET",
        "/api/v1/cluster/join-tokens",
        &["cluster.manage", "cluster.sensitive_change"],
    ),
    (
        "POST",
        "/api/v1/cluster/join-tokens",
        &["cluster.sensitive_change"],
    ),
    (
        "DELETE",
        "/api/v1/cluster/join-tokens/{member}",
        &["cluster.manage", "cluster.sensitive_change"],
    ),
    (
        "GET",
        "/api/v1/cluster/members/{member}/evacuation",
        &["cluster.view"],
    ),
    (
        "POST",
        "/api/v1/cluster/members/{member}/evacuate",
        &["cluster.evacuate", "cluster.evacuate_unsafe"],
    ),
    (
        "POST",
        "/api/v1/cluster/members/{member}/restore",
        &["cluster.evacuate", "cluster.evacuate_unsafe"],
    ),
    ("GET", "/api/v1/machines", &["cluster.view"]),
    ("POST", "/api/v1/machines", &["machine.enrol"]),
    ("DELETE", "/api/v1/machines/{name}", &["hardware.delete"]),
    (
        "POST",
        "/api/v1/machines/{name}/approve",
        &["cluster.sensitive_change"],
    ),
    (
        "DELETE",
        "/api/v1/machines/{name}/approval",
        &["machine.enrol"],
    ),
    (
        "GET",
        "/api/v1/machines/{name}/admission",
        &["machine.enrol"],
    ),
    ("POST", "/api/v1/machines/{name}/admit", &["machine.enrol"]),
    ("GET", "/api/v1/preflight", &["cluster.view"]),
    ("GET", "/api/v1/projects", &["cluster.view"]),
    ("GET", "/api/v1/networks", &["cluster.view"]),
    ("GET", "/api/v1/storage-pools", &["cluster.view"]),
    ("GET", "/api/v1/audit", &["audit.read"]),
    // Identity and access (see identity.rs)
    (
        "GET",
        "/api/v1/identity/organizations",
        &["organization.view"],
    ),
    (
        "POST",
        "/api/v1/identity/organizations",
        &["organization.create"],
    ),
    (
        "GET",
        "/api/v1/identity/projects",
        &["project.view", "organization.view"],
    ),
    (
        "POST",
        "/api/v1/identity/organizations/{org}/projects",
        &["project.create"],
    ),
    (
        "GET",
        "/api/v1/identity/projects/{project}/environments",
        &["project.view"],
    ),
    (
        "POST",
        "/api/v1/identity/projects/{project}/environments",
        &["environment.manage"],
    ),
    (
        "GET",
        "/api/v1/identity/users",
        &["user.view", "user.manage"],
    ),
    (
        "GET",
        "/api/v1/identity/users/lookup",
        &["role.assign", "group.manage"],
    ),
    (
        "POST",
        "/api/v1/identity/users/{user}/disable",
        &["user.manage"],
    ),
    (
        "POST",
        "/api/v1/identity/users/{user}/enable",
        &["user.manage"],
    ),
    (
        "GET",
        "/api/v1/identity/groups",
        &["user.view", "group.manage"],
    ),
    ("POST", "/api/v1/identity/groups", &["group.manage"]),
    (
        "GET",
        "/api/v1/identity/groups/{group}/members",
        &["user.view", "group.manage"],
    ),
    (
        "PUT",
        "/api/v1/identity/groups/{group}/members/{user}",
        &["group.manage"],
    ),
    (
        "DELETE",
        "/api/v1/identity/groups/{group}/members/{user}",
        &["group.manage"],
    ),
    (
        "GET",
        "/api/v1/identity/roles",
        &["role.assign", "role.manage", "user.view"],
    ),
    ("POST", "/api/v1/identity/roles", &["role.manage"]),
    ("DELETE", "/api/v1/identity/roles/{role}", &["role.manage"]),
    ("GET", "/api/v1/identity/permissions", &[]),
    (
        "GET",
        "/api/v1/identity/assignments",
        &["user.view", "role.assign"],
    ),
    ("POST", "/api/v1/identity/assignments", &["role.assign"]),
    (
        "POST",
        "/api/v1/identity/assignments/preview",
        &["role.assign"],
    ),
    (
        "DELETE",
        "/api/v1/identity/assignments/{assignment}",
        &["role.assign", "platform.emergency_access"],
    ),
    (
        "POST",
        "/api/v1/identity/emergency-access",
        &["platform.emergency_access"],
    ),
    (
        "GET",
        "/api/v1/identity/service-identities",
        &["service_identity.manage"],
    ),
    (
        "POST",
        "/api/v1/identity/service-identities",
        &["service_identity.manage"],
    ),
    (
        "POST",
        "/api/v1/identity/service-identities/{id}/disable",
        &["service_identity.manage"],
    ),
    (
        "POST",
        "/api/v1/identity/service-identities/{id}/enable",
        &["service_identity.manage"],
    ),
    (
        "GET",
        "/api/v1/identity/policies",
        &["access_policy.manage", "audit.read"],
    ),
    (
        "PUT",
        "/api/v1/identity/policies",
        &["access_policy.manage"],
    ),
    (
        "DELETE",
        "/api/v1/identity/policies/{id}",
        &["access_policy.manage"],
    ),
    ("GET", "/api/v1/identity/audit", &["audit.read"]),
    (
        "GET",
        "/api/v1/approvals",
        &["approval.request", "approval.grant"],
    ),
    ("POST", "/api/v1/approvals", &["approval.request"]),
    (
        "POST",
        "/api/v1/approvals/{id}/approve",
        &["approval.grant"],
    ),
    ("POST", "/api/v1/approvals/{id}/reject", &["approval.grant"]),
    (
        "POST",
        "/api/v1/approvals/{id}/cancel",
        &["approval.request"],
    ),
];

/// Default-deny gate for people and service identities.
pub(super) async fn route_gate(matched: Option<MatchedPath>, req: Request, next: Next) -> Response {
    let Some(p) = req.extensions().get::<Principal>().cloned() else {
        return forbidden("access", false).into_response();
    };
    if p.caller.is_some() {
        let pattern = matched.as_ref().map(MatchedPath::as_str).unwrap_or("");
        let method = req.method().as_str();
        match GATES
            .iter()
            .find(|(m, pat, _)| *m == method && *pat == pattern)
        {
            Some((_, _, any_of))
                if any_of.is_empty() || any_of.iter().any(|perm| holds_anywhere(&p, perm)) => {}
            Some((_, _, any_of)) => {
                return forbidden(any_of.first().copied().unwrap_or("access"), false)
                    .into_response();
            }
            None => {
                return ApiError::new(
                    StatusCode::FORBIDDEN,
                    "route_not_authorizable",
                    "this route has no access rule for signed-in users",
                )
                .into_response();
            }
        }
    }
    next.run(req).await
}

/// Records every refused request in the audit trail (who, what, and that it was denied).
pub(super) async fn audit_denials(
    State(state): State<AppState>,
    req: Request,
    next: Next,
) -> Response {
    let who = req.extensions().get::<Principal>().cloned();
    let method = req.method().to_string();
    let path = req.uri().path().to_owned();
    let res = next.run(req).await;
    if res.status() == StatusCode::FORBIDDEN
        && let (Some(p), Auth::Tokens(pool)) = (who, &state.auth)
    {
        let _ = apelora_persistence::identity::audit_event(
            pool,
            &apelora_persistence::identity::AuditEntry {
                actor: &p.actor_ref(),
                actor_type: p.actor_type(),
                action: "access.denied",
                target: &format!("{method} {path}"),
                outcome: "denied",
                scope: None,
                detail: &json!({ "name": p.name }),
            },
        )
        .await;
    }
    res
}

/// Writes an audit event for the caller: who (stable id and kind), what, on what.
pub(super) async fn audit_as(
    pool: &apelora_persistence::Pool,
    p: &Principal,
    action: &str,
    target: &str,
    detail: &Value,
) -> apelora_persistence::Result<()> {
    audit_scoped(pool, p, action, target, "success", None, detail).await
}

pub(super) async fn audit_scoped(
    pool: &apelora_persistence::Pool,
    p: &Principal,
    action: &str,
    target: &str,
    outcome: &str,
    scope: Option<&Resource>,
    detail: &Value,
) -> apelora_persistence::Result<()> {
    let mut detail = detail.clone();
    if let Some(o) = detail.as_object_mut() {
        o.entry("name").or_insert_with(|| json!(p.name));
        if let Some(r) = p.role {
            o.entry("role").or_insert_with(|| json!(r.as_str()));
        }
        if p.caller.as_ref().is_some_and(|c| c.emergency) {
            o.insert("emergencyAccess".into(), json!(true));
        }
    }
    apelora_persistence::identity::audit_event(
        pool,
        &apelora_persistence::identity::AuditEntry {
            actor: &p.actor_ref(),
            actor_type: p.actor_type(),
            action,
            target,
            outcome,
            scope,
            detail: &detail,
        },
    )
    .await
}

pub(super) const APPROVAL_HEADER: &str = "apelora-approval";

/// [`guard`] for a permission whose approval rule depends on the resource (interactive access
/// in a production environment).
pub(super) async fn guard_on(
    state: &AppState,
    p: &Principal,
    headers: &HeaderMap,
    permission: &str,
    target: &str,
    res: &Resource,
) -> Result<Option<Uuid>, ApiError> {
    let Some(caller) = &p.caller else {
        return Ok(None);
    };
    if !caller.engine.needs_approval_on(permission, res) {
        return Ok(None);
    }
    guard_required(state, p, headers, permission, target).await
}

/// The approval gate for risky actions. When the action needs approval, the caller must present
/// an approval of exactly this action and target that someone else granted; it is spent here.
/// Legacy tokens are not subject to approvals (they are platform-wide credentials; disable them
/// with `APELORA_LEGACY_TOKENS=disabled` to enforce the rule for everyone).
pub(super) async fn guard(
    state: &AppState,
    p: &Principal,
    headers: &HeaderMap,
    permission: &str,
    target: &str,
) -> Result<Option<Uuid>, ApiError> {
    let Some(caller) = &p.caller else {
        return Ok(None);
    };
    if !caller.engine.needs_approval(permission) {
        return Ok(None);
    }
    guard_required(state, p, headers, permission, target).await
}

/// Spends the approval the caller presented for exactly this action and target.
async fn guard_required(
    state: &AppState,
    p: &Principal,
    headers: &HeaderMap,
    permission: &str,
    target: &str,
) -> Result<Option<Uuid>, ApiError> {
    let pool = super::deploy::store(state)?;
    let Some(raw) = headers.get(APPROVAL_HEADER).and_then(|v| v.to_str().ok()) else {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "approval_required",
            format!(
                "{permission} on {target} needs another person's approval: create a request (POST /api/v1/approvals), have an approver grant it, then repeat this call with the header {APPROVAL_HEADER}: <approval id>"
            ),
        ));
    };
    let id: Uuid = raw.trim().parse().map_err(|_| {
        ApiError::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "the approval header must be an approval id",
        )
    })?;
    match apelora_persistence::identity::consume_approval(
        pool,
        id,
        permission,
        target,
        &p.actor_ref(),
    )
    .await
    {
        Ok(Ok(_)) => Ok(Some(id)),
        Ok(Err(e)) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "approval_invalid",
            e.to_string(),
        )),
        Err(apelora_persistence::StoreError::NotFound(_)) => Err(ApiError::new(
            StatusCode::CONFLICT,
            "approval_invalid",
            "no such approval",
        )),
        Err(e) => Err(e.into()),
    }
}
