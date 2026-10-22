// SPDX-License-Identifier: AGPL-3.0-only
//! Authentication results and authorisation checks for the API.
//!
//! Three kinds of caller reach the API:
//! - a **person** with an OIDC access token or an Apelora session after a password sign-in
//!   (`apu_…`, see `sessions_auth`) (`Caller` of kind `User`),
//! - an **automation** service identity with an `aps_…` token (`Caller` of kind `Service`),
//! - a **legacy API token** (`apl_…`), platform-wide and role-based, kept so existing set-ups
//!   keep working until `APELORA_LEGACY_TOKENS=disabled`.
//!
//! Every handler authorises with [`allow`] (or `require` for platform-level routes); a route
//! gate in front of the handlers refuses any route that is not listed in [`GATES`] for every
//! caller, so a new route is denied until someone decides who may use it.

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

/// Counts a refusal by the permission that was missing: a catalogue id, or `other` (a free
/// text never becomes a label).
fn count_denial(permission: &str) {
    let label = if apelora_auth::permission(permission).is_some() {
        permission
    } else {
        "other"
    };
    apelora_metrics::AUTHZ_DENIALS.inc(&[label]);
}

fn forbidden(permission: &str, mfa: bool) -> ApiError {
    count_denial(permission);
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
        | "capacity.view"
        | "fabric.metrics.read"
        | "user.view" => ReadInventory,
        "audit.read" => ReadAudit,
        "application.design" | "application.create" => DesignApplications,
        "application.deploy" | "application.operate" | "application.delete" | "instance.manage" => {
            Deploy
        }
        "token.manage" | "token.create_admin" => ManageTokens,
        "cluster.manage"
        | "cluster.evacuate"
        | "cluster.evacuate_unsafe"
        | "cluster.fence_confirm"
        | "cluster.sensitive_change"
        | "network.manage"
        | "storage.manage"
        | "machine.enrol"
        | "support.bundle"
        | "alert.respond"
        | "alert.configure"
        | "quota.manage"
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

/// What an API token of `role` would be able to do and the caller may not (at platform level),
/// in catalogue order. Empty when the caller may create such a token: a token never carries more
/// than its creator holds.
pub(super) fn beyond_caller(p: &Principal, role: Role) -> Vec<&'static str> {
    apelora_auth::PERMISSIONS
        .iter()
        .map(|d| d.id)
        .filter(|id| legacy_requirement(id).is_some_and(|need| role.allows(need)))
        .filter(|id| allow_platform(p, id).is_err())
        .collect()
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
        (None, Some(role)) => Err({
            count_denial(platform_permission(perm));
            ApiError::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                format!("role {} may not {perm:?}", role.as_str()),
            )
        }),
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
/// refused.** Legacy tokens pass the same gate through their role's legacy mapping.
pub(super) const GATES: &[(&str, &str, &[&str])] = &[
    ("GET", "/api/v1/whoami", &[]),
    // Ends the caller's own session; changes the caller's own password (local accounts only,
    // checked by the handler).
    ("POST", "/api/v1/sign-out", &[]),
    ("PUT", "/api/v1/me/password", &[]),
    // A person's own personal tokens and sign-ins (the handlers refuse anyone but a person).
    ("GET", "/api/v1/me/tokens", &[]),
    ("POST", "/api/v1/me/tokens", &[]),
    ("POST", "/api/v1/me/tokens/{id}/rotate", &[]),
    ("DELETE", "/api/v1/me/tokens/{id}", &[]),
    ("GET", "/api/v1/me/sign-ins", &[]),
    ("DELETE", "/api/v1/me/sign-ins/{id}", &[]),
    // Everyone's: token.manage for tokens, user.manage for sign-ins (platform level, checked
    // again by the handlers).
    ("GET", "/api/v1/identity/personal-tokens", &["token.manage"]),
    (
        "DELETE",
        "/api/v1/identity/personal-tokens/{id}",
        &["token.manage"],
    ),
    ("GET", "/api/v1/identity/sign-ins", &["user.manage"]),
    ("DELETE", "/api/v1/identity/sign-ins/{id}", &["user.manage"]),
    // The settings this control plane runs with (secrets never shown) and the support bundle.
    ("GET", "/api/v1/settings", &["cluster.view"]),
    ("GET", "/api/v1/support-bundle", &["support.bundle"]),
    ("POST", "/api/v1/identity/local-accounts", &["user.manage"]),
    (
        "POST",
        "/api/v1/identity/local-accounts/{id}/reset",
        &["user.manage"],
    ),
    (
        "PUT",
        "/api/v1/identity/sign-in/local-accounts",
        &["platform.security.manage"],
    ),
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
    // Servers area (clustering.rs), read-only; settings are read with the admin identity.
    ("GET", "/api/v1/cluster/members", &["cluster.view"]),
    ("GET", "/api/v1/cluster/groups", &["cluster.view"]),
    ("GET", "/api/v1/cluster/settings", &["cluster.view"]),
    (
        "PATCH",
        "/api/v1/cluster/settings",
        &["cluster.sensitive_change"],
    ),
    ("GET", "/api/v1/cluster/recovery", &["cluster.view"]),
    ("POST", "/api/v1/cluster/upgrade", &["cluster.evacuate"]),
    ("GET", "/api/v1/health/components", &["cluster.view"]),
    ("GET", "/api/v1/health/servers", &["cluster.view"]),
    // Server names only with cluster.view; capacity.view sees numbered servers (the handler decides).
    (
        "GET",
        "/api/v1/capacity",
        &["cluster.view", "capacity.view"],
    ),
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
    (
        "GET",
        "/api/v1/instances/{project}/{name}/move",
        &["instance.manage"],
    ),
    // The handlers decide per instance (its environment and its application) and answer 404
    // for instances the caller may not see.
    (
        "GET",
        "/api/v1/instances/{project}/{name}",
        &["cluster.view", "instance.manage", "application.view"],
    ),
    (
        "GET",
        "/api/v1/instances/{project}/{name}/logs",
        &["application.logs.read", "instance.manage"],
    ),
    (
        "GET",
        "/api/v1/instances/{project}/{name}/logs/{file}",
        &["application.logs.read", "instance.manage"],
    ),
    (
        "GET",
        "/api/v1/instances/{project}/{name}/console-log",
        &["application.logs.read", "instance.manage"],
    ),
    (
        "POST",
        "/api/v1/instances/{project}/{name}/move",
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
        "/api/v1/applications/{name}/migration",
        &["application.operate"],
    ),
    (
        "GET",
        "/api/v1/applications/{name}/events",
        &["application.view"],
    ),
    (
        "GET",
        "/api/v1/identity/sign-in",
        &["platform.security.manage"],
    ),
    (
        "POST",
        "/api/v1/identity/sign-in/check",
        &["platform.security.manage"],
    ),
    (
        "POST",
        "/api/v1/identity/sign-in/providers",
        &["platform.security.manage"],
    ),
    (
        "PUT",
        "/api/v1/identity/sign-in/providers/{id}",
        &["platform.security.manage"],
    ),
    (
        "DELETE",
        "/api/v1/identity/sign-in/providers/{id}",
        &["platform.security.manage"],
    ),
    (
        "PUT",
        "/api/v1/identity/sign-in/api-tokens",
        &["platform.security.manage"],
    ),
    (
        "GET",
        "/api/v1/identity/sign-in/directories",
        &["platform.security.manage"],
    ),
    (
        "POST",
        "/api/v1/identity/sign-in/directories",
        &["platform.security.manage"],
    ),
    (
        "POST",
        "/api/v1/identity/sign-in/directories/check",
        &["platform.security.manage"],
    ),
    (
        "PUT",
        "/api/v1/identity/sign-in/directories/{id}",
        &["platform.security.manage"],
    ),
    (
        "DELETE",
        "/api/v1/identity/sign-in/directories/{id}",
        &["platform.security.manage"],
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
        "GET",
        "/api/v1/applications/{name}/volumes/{volume}/snapshots",
        &["application.view"],
    ),
    (
        "POST",
        "/api/v1/applications/{name}/volumes/{volume}/snapshots",
        &["application.operate"],
    ),
    (
        "DELETE",
        "/api/v1/applications/{name}/volumes/{volume}/snapshots/{snapshot}",
        &["application.delete"],
    ),
    (
        "POST",
        "/api/v1/applications/{name}/volumes/{volume}/snapshots/{snapshot}/restore",
        &["application.operate"],
    ),
    (
        "GET",
        "/api/v1/applications/{name}/volumes/{volume}/backups",
        &["application.view"],
    ),
    (
        "POST",
        "/api/v1/applications/{name}/volumes/{volume}/backups",
        &["application.operate"],
    ),
    (
        "DELETE",
        "/api/v1/applications/{name}/volumes/{volume}/backups/{backup}",
        &["application.delete"],
    ),
    (
        "PUT",
        "/api/v1/applications/{name}/volumes/{volume}/schedules",
        &["application.deploy"],
    ),
    // The dry run is offered to whoever may restore; both check the application.
    (
        "GET",
        "/api/v1/applications/{name}/volumes/{volume}/backups/{backup}/restore",
        &["application.operate"],
    ),
    (
        "POST",
        "/api/v1/applications/{name}/volumes/{volume}/backups/{backup}/restore",
        &["application.operate"],
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
    (
        "GET",
        "/api/v1/jobs/{id}/detail",
        &["cluster.view", "application.view", "instance.manage"],
    ),
    // The permission that queued the job's kind, checked by the handler on the job's target.
    (
        "POST",
        "/api/v1/jobs/{id}/retry",
        &[
            "instance.manage",
            "application.deploy",
            "application.operate",
            "cluster.manage",
        ],
    ),
    (
        "POST",
        "/api/v1/jobs/{id}/cancel",
        &[
            "instance.manage",
            "application.deploy",
            "application.operate",
            "cluster.manage",
        ],
    ),
    ("GET", "/api/v1/tokens", &["token.manage"]),
    ("POST", "/api/v1/tokens", &["token.manage"]),
    ("DELETE", "/api/v1/tokens/{id}", &["token.manage"]),
    ("GET", "/api/v1/system", &["cluster.view"]),
    ("GET", "/api/v1/database-backups", &["cluster.view"]),
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
    (
        "POST",
        "/api/v1/cluster/members/{member}/fencing",
        &["cluster.fence_confirm"],
    ),
    (
        "GET",
        "/api/v1/cluster/members/{member}/removal",
        &["cluster.view"],
    ),
    (
        "POST",
        "/api/v1/cluster/members/{member}/remove",
        &["cluster.manage"],
    ),
    ("GET", "/api/v1/machines", &["cluster.view"]),
    (
        "GET",
        "/api/v1/provisioners/maas/machines",
        &["cluster.view"],
    ),
    (
        "POST",
        "/api/v1/provisioners/maas/machines/{system_id}/commission",
        &["hardware.commission"],
    ),
    (
        "POST",
        "/api/v1/provisioners/maas/machines/{system_id}/deploy",
        &["hardware.deploy"],
    ),
    (
        "POST",
        "/api/v1/provisioners/maas/machines/{system_id}/release",
        &["hardware.deploy"],
    ),
    (
        "POST",
        "/api/v1/provisioners/maas/machines/{system_id}/test",
        &["hardware.commission"],
    ),
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
    ("GET", "/api/v1/image-mirrors", &["cluster.view"]),
    ("GET", "/api/v1/projects", &["cluster.view"]),
    ("GET", "/api/v1/networks", &["cluster.view"]),
    // Infrastructure settings in effect: uplink, address ranges, project limits.
    ("GET", "/api/v1/networks/settings", &["cluster.view"]),
    // Per application; the handler lists only applications the caller may view.
    (
        "GET",
        "/api/v1/networks/applications",
        &["application.view"],
    ),
    ("GET", "/api/v1/storage-pools", &["cluster.view"]),
    ("GET", "/api/v1/storage/pools", &["cluster.view"]),
    // Per application; the handler lists only applications the caller may view.
    ("GET", "/api/v1/storage/volumes", &["application.view"]),
    // Custom volumes no application references: infrastructure, platform-wide.
    ("GET", "/api/v1/storage/orphans", &["cluster.view"]),
    ("GET", "/api/v1/audit", &["audit.read"]),
    ("GET", "/api/v1/audit/export", &["audit.read"]),
    // Monitoring (metrics.rs): scraping needs the metrics permission; the set-up page also
    // opens to people who can see the cluster.
    ("GET", "/metrics", &["fabric.metrics.read"]),
    (
        "GET",
        "/api/v1/monitoring",
        &["fabric.metrics.read", "cluster.view"],
    ),
    // Alerts (alerts.rs): seen with the cluster; acknowledged and silenced with alert.respond;
    // rules and channels changed with alert.configure. All platform-level.
    ("GET", "/api/v1/alerts", &["cluster.view"]),
    ("GET", "/api/v1/alerts/history", &["cluster.view"]),
    (
        "POST",
        "/api/v1/alerts/{id}/acknowledge",
        &["alert.respond"],
    ),
    ("POST", "/api/v1/alerts/silences", &["alert.respond"]),
    ("DELETE", "/api/v1/alerts/silences/{id}", &["alert.respond"]),
    ("GET", "/api/v1/alerts/rules", &["cluster.view"]),
    ("PUT", "/api/v1/alerts/rules/{rule}", &["alert.configure"]),
    ("GET", "/api/v1/alerts/channels", &["cluster.view"]),
    ("POST", "/api/v1/alerts/channels", &["alert.configure"]),
    ("PUT", "/api/v1/alerts/channels/{id}", &["alert.configure"]),
    (
        "DELETE",
        "/api/v1/alerts/channels/{id}",
        &["alert.configure"],
    ),
    (
        "POST",
        "/api/v1/alerts/channels/{id}/test",
        &["alert.configure"],
    ),
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
    // Tenancy and quotas (ADR 0012): the handlers check the exact scope; setting a quota is
    // `quota.manage`, or a tenant dividing its own (`organization.update`, `project.update`).
    (
        "GET",
        "/api/v1/identity/tenancy",
        &["organization.view", "project.view", "quota.manage"],
    ),
    (
        "GET",
        "/api/v1/identity/quotas/{scope}/{id}",
        &["organization.view", "project.view", "quota.manage"],
    ),
    (
        "PUT",
        "/api/v1/identity/quotas/{scope}/{id}",
        &["quota.manage", "organization.update", "project.update"],
    ),
    (
        "POST",
        "/api/v1/identity/environments/{id}/incus-project",
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

/// Default-deny gate for every caller: people, service identities and legacy API tokens (and the
/// development principal), the latter through their role's legacy mapping.
pub(super) async fn route_gate(matched: Option<MatchedPath>, req: Request, next: Next) -> Response {
    let Some(p) = req.extensions().get::<Principal>().cloned() else {
        return forbidden("access", false).into_response();
    };
    let pattern = matched.as_ref().map(MatchedPath::as_str).unwrap_or("");
    let method = req.method().as_str();
    if p.personal_token.is_some()
        && super::account::REFUSED_TO_PERSONAL_TOKENS
            .iter()
            .any(|(m, pat)| *m == method && *pat == pattern)
    {
        apelora_metrics::AUTHZ_DENIALS.inc(&["other"]);
        return ApiError::new(
            StatusCode::FORBIDDEN,
            "personal_token_refused",
            "a personal token cannot do this: interactive sessions, new credentials, approvals, emergency access and password changes need you to sign in",
        )
        .into_response();
    }
    match GATES
        .iter()
        .find(|(m, pat, _)| *m == method && *pat == pattern)
    {
        Some((_, _, any_of))
            if any_of.is_empty() || any_of.iter().any(|perm| holds_anywhere(&p, perm)) => {}
        Some((_, _, any_of)) => {
            return forbidden(any_of.first().copied().unwrap_or("access"), false).into_response();
        }
        None => {
            apelora_metrics::AUTHZ_DENIALS.inc(&["no_route_rule"]);
            return ApiError::new(
                StatusCode::FORBIDDEN,
                "route_not_authorizable",
                "this route has no access rule for signed-in users",
            )
            .into_response();
        }
    }
    next.run(req).await
}

/// Records every refused request (403) in the audit trail: who, what, that it was denied, and
/// why (`code`: `forbidden`, `mfa_required`, `route_not_authorizable`, `account_disabled`, ...).
/// Unauthenticated requests (401) are logged, not audited: anyone could fill the trail with them.
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
        let refusal = res.extensions().get::<super::requests::ErrorCode>();
        let mut detail = json!({
            "name": p.name,
            "code": refusal.map_or("forbidden", |r| r.code),
            "reason": refusal.map(|r| r.message.as_str()),
        });
        super::requests::tag(&mut detail);
        audit_failed(
            "access.denied",
            apelora_persistence::identity::audit_event(
                pool,
                &apelora_persistence::identity::AuditEntry {
                    actor: &p.actor_ref(),
                    actor_type: p.actor_type(),
                    actor_name: Some(&p.name),
                    action: "access.denied",
                    target: &format!("{method} {path}"),
                    outcome: "denied",
                    scope: None,
                    detail: &detail,
                },
            )
            .await,
        );
    }
    res
}

/// For audit writes that must not fail the caller's request: a failure is still never silent
/// (a broken audit table has to be noticed).
pub(super) fn audit_failed(action: &str, result: apelora_persistence::Result<()>) {
    if let Err(e) = result {
        tracing::error!(action, error = %e, "audit: could not record {action}");
    }
}

/// How often a person signing in with identity-provider tokens is audited: on the first request
/// of each period (an hour, the longest token lifetime Apelora accepts). Password and directory
/// sign-ins are audited when their session is issued; tokens have no such moment.
pub(super) const PROVIDER_SIGN_IN_PERIOD_SECS: f64 = 3600.0;

/// Records `auth.oidc.signed_in` for the first request a person makes with identity-provider
/// tokens in each [`PROVIDER_SIGN_IN_PERIOD_SECS`]: at most one row per person per period, so
/// valid tokens cannot flood the trail (refused tokens are logged, never audited). A failure is
/// logged at error and does not refuse the request: every change still needs its own audit row.
pub(super) async fn audit_provider_sign_in(
    pool: &apelora_persistence::Pool,
    user: Uuid,
    issuer: &str,
    mfa: bool,
) {
    let mut detail = json!({ "method": "oidc", "issuer": issuer, "mfa": mfa });
    super::requests::tag(&mut detail);
    let recorded = apelora_persistence::identity::audit_provider_sign_in(
        pool,
        user,
        PROVIDER_SIGN_IN_PERIOD_SECS,
        &detail,
    )
    .await;
    audit_failed("auth.oidc.signed_in", recorded.map(|_| ()));
}

/// Begins the transaction a change and its audit row are written in (EVERY-SIZE.md §2.5): pass
/// `&mut tx` to the persistence functions and to [`audit_as`]/[`audit_scoped`], then
/// [`commit`]. If the audit row cannot be written, the change is rolled back with it.
pub(super) async fn begin(
    pool: &apelora_persistence::Pool,
) -> apelora_persistence::Result<apelora_persistence::Tx> {
    apelora_persistence::begin(pool).await
}

/// Commits a change and its audit row together.
pub(super) async fn commit(tx: apelora_persistence::Tx) -> apelora_persistence::Result<()> {
    apelora_persistence::commit(tx).await
}

/// Writes the caller's audit row for a change, inside the change's transaction: who (stable id,
/// kind and name), what, on what.
pub(super) async fn audit_as(
    tx: &mut apelora_persistence::PgConnection,
    p: &Principal,
    action: &str,
    target: &str,
    detail: &Value,
) -> apelora_persistence::Result<()> {
    audit_scoped(tx, p, action, target, "success", None, detail).await
}

/// [`audit_as`] with an outcome and the organisation, project or application concerned.
pub(super) async fn audit_scoped(
    tx: &mut apelora_persistence::PgConnection,
    p: &Principal,
    action: &str,
    target: &str,
    outcome: &str,
    scope: Option<&Resource>,
    detail: &Value,
) -> apelora_persistence::Result<()> {
    write_audit(&mut *tx, p, action, target, outcome, scope, detail).await
}

/// Writes the caller's audit row on its own, for an event that changes nothing in Apelora's
/// database: a refusal, a failed attempt, an export, or a change carried out in Incus or in
/// this process's memory (the audit-coverage test lists those routes and why).
pub(super) async fn audit_alone(
    pool: &apelora_persistence::Pool,
    p: &Principal,
    action: &str,
    target: &str,
    outcome: &str,
    scope: Option<&Resource>,
    detail: &Value,
) -> apelora_persistence::Result<()> {
    write_audit(pool, p, action, target, outcome, scope, detail).await
}

async fn write_audit(
    db: impl apelora_persistence::PgExecutor<'_>,
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
    super::requests::tag(&mut detail);
    apelora_persistence::identity::audit_event(
        db,
        &apelora_persistence::identity::AuditEntry {
            actor: &p.actor_ref(),
            actor_type: p.actor_type(),
            actor_name: Some(&p.name),
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
/// Legacy tokens are not subject to approvals or multi-factor rules (they are platform-wide
/// credentials that cannot present either; disable them with `APELORA_LEGACY_TOKENS=disabled` to
/// enforce the rules for everyone). Where that exemption would let a token mint a more powerful
/// credential, the handler refuses legacy tokens instead (a `fabric-admin` token, `admin.rs`).
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
    let reason = match apelora_persistence::identity::consume_approval(
        pool,
        id,
        permission,
        target,
        &p.actor_ref(),
    )
    .await
    {
        Ok(Ok(_)) => {
            tracing::info!(approval = %id, permission, target, "approval spent");
            return Ok(Some(id));
        }
        Ok(Err(e)) => e.to_string(),
        Err(apelora_persistence::StoreError::NotFound(_)) => "no such approval".to_owned(),
        Err(e) => return Err(e.into()),
    };
    // A spent, someone else's or mismatched approval is a refusal: on the record.
    audit_failed(
        "access.denied",
        audit_alone(
            pool,
            p,
            "access.denied",
            target,
            "denied",
            None,
            &json!({ "code": "approval_invalid", "approval": id, "permission": permission, "reason": reason }),
        )
        .await,
    );
    Err(ApiError::new(
        StatusCode::CONFLICT,
        "approval_invalid",
        reason,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn legacy(name: &str, role: Role) -> Principal {
        Principal {
            name: name.into(),
            role: Some(role),
            caller: None,
            session: None,
            personal_token: None,
        }
    }

    #[test]
    fn audit_trails_name_who_acted_without_confusing_tokens_with_the_development_account() {
        assert_eq!(
            legacy("development", Role::FabricAdmin).actor_ref(),
            "development"
        );
        // Only the real development account has that name; a token cannot borrow it.
        for p in [
            legacy("development", Role::Operator),
            legacy("ci", Role::FabricAdmin),
        ] {
            assert_eq!(p.actor_ref(), format!("token:{}", p.name));
        }
        assert_eq!(legacy("ci", Role::FabricAdmin).actor_ref(), "token:ci");
    }

    #[test]
    fn legacy_roles_keep_exactly_the_reach_they_had() {
        let may = |role: Role, permission: &str| {
            legacy_requirement(permission).is_some_and(|need| role.allows(need))
        };
        for p in ["application.design", "application.create"] {
            assert!(may(Role::StudioDesigner, p), "{p}");
            assert!(may(Role::Operator, p), "{p}");
            assert!(!may(Role::Auditor, p), "{p}");
        }
        for p in [
            "application.deploy",
            "application.operate",
            "instance.manage",
        ] {
            assert!(may(Role::Operator, p), "{p}");
            assert!(!may(Role::StudioDesigner, p), "{p}");
        }
        assert!(may(Role::Auditor, "audit.read"));
        assert!(!may(Role::Operator, "audit.read"));
        assert!(!may(Role::Operator, "token.manage"));
        // Identity administration and platform security stay with legacy administrators.
        for p in [
            "organization.create",
            "organization.update",
            "project.create",
            "project.update",
            "project.delete",
            "environment.manage",
            "group.manage",
            "role.assign",
            "role.manage",
            "service_identity.manage",
            "access_policy.manage",
            "platform.security.manage",
            "platform.emergency_access",
            "user.manage",
        ] {
            assert!(may(Role::FabricAdmin, p), "{p}");
            for lesser in [Role::Operator, Role::StudioDesigner, Role::Auditor] {
                assert!(!may(lesser, p), "{lesser:?} {p}");
            }
        }
        // Reading and infrastructure map as before.
        assert!(may(Role::Auditor, "application.view") && may(Role::Auditor, "cluster.view"));
        assert!(!may(Role::Operator, "cluster.manage") && may(Role::FabricAdmin, "cluster.manage"));
        assert!(!may(Role::StudioDesigner, "approval.grant"));
        // Interactive access is never reachable through a legacy token, whatever its role.
        for p in ["instance.console", "instance.desktop"] {
            assert!(legacy_requirement(p).is_none(), "{p}");
            assert!(!may(Role::FabricAdmin, p), "{p}");
        }
    }
}
