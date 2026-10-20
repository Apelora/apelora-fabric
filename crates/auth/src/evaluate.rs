// SPDX-License-Identifier: AGPL-3.0-only
//! The authorisation decision. Pure and deterministic: it takes the assignments already loaded
//! for a caller and answers "may this subject do this to that resource, and why".
//!
//! Rules, in order of importance:
//! 1. **Deny by default.** No applicable assignment, an unknown permission, an unknown role,
//!    a revoked or expired assignment: all deny.
//! 2. **Class limits.** Infrastructure and security permissions count only from platform (or,
//!    for hardware, pool) assignments, whatever the role says.
//! 3. **Scope.** An assignment reaches its scope and everything beneath it; nothing else.
//! 4. **Conditions and policies** (MFA, environment) narrow a grant; they never widen one.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::permission::{Class, PERMISSIONS, PermissionDef, permission};
use crate::role::{Builtin, CustomRole, ResolvedRole, RoleRef};
use crate::scope::{EnvKind, Level, Resource, Scope, covers};

/// Who an assignment is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum PrincipalRef {
    User { id: Uuid },
    Group { id: Uuid },
    Service { id: Uuid },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Conditions {
    /// The grant counts only when the caller signed in with multi-factor authentication.
    #[serde(default)]
    pub require_mfa: bool,
    /// The grant counts only for resources in environments of these kinds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environments: Option<Vec<EnvKind>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum AssignmentKind {
    Standard,
    /// Time-limited elevated access with a recorded reason (break-glass).
    Emergency,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Assignment {
    pub id: Uuid,
    pub principal: PrincipalRef,
    pub role: RoleRef,
    pub scope: Scope,
    pub conditions: Conditions,
    /// Seconds since the Unix epoch; `None` = does not expire (emergency grants always expire).
    pub expires_at: Option<i64>,
    pub revoked: bool,
    pub kind: AssignmentKind,
}

/// The caller as far as authorisation cares.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Subject {
    pub user: Option<Uuid>,
    pub service: Option<Uuid>,
    pub groups: BTreeSet<Uuid>,
    /// Signed in with multi-factor authentication (from the token's authentication claims).
    pub mfa: bool,
}

impl Subject {
    pub fn matches(&self, p: &PrincipalRef) -> Option<Option<Uuid>> {
        match p {
            PrincipalRef::User { id } if self.user == Some(*id) => Some(None),
            PrincipalRef::Group { id } if self.groups.contains(id) => Some(Some(*id)),
            PrincipalRef::Service { id } if self.service == Some(*id) => Some(None),
            _ => None,
        }
    }
}

/// Rules that apply to everyone (managed under "Access policies").
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policies {
    pub mfa_for_environments: Vec<EnvKind>,
    pub mfa_for_infrastructure: bool,
    /// Further permission ids that need approval, on top of the catalogue's.
    pub extra_approval: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Grant {
    pub assignment: Uuid,
    pub role: RoleRef,
    pub role_name: String,
    pub scope: Scope,
    /// Set when the grant reaches the caller through a group.
    pub via_group: Option<Uuid>,
    pub emergency: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Denial {
    UnknownPermission,
    NoApplicableAssignment,
    MfaRequired,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub allowed: bool,
    pub grants: Vec<Grant>,
    pub denial: Option<Denial>,
}

impl Decision {
    fn deny(d: Denial) -> Self {
        Self {
            allowed: false,
            grants: Vec::new(),
            denial: Some(d),
        }
    }
}

/// Which resources of a kind a caller may reach with a permission, for scoping queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessFilter {
    /// Everything (a platform-wide grant).
    All,
    /// Only what lies under these scopes. Empty lists mean no access.
    Limited {
        organizations: Vec<Uuid>,
        projects: Vec<Uuid>,
        applications: Vec<Uuid>,
    },
}

impl AccessFilter {
    pub fn is_empty(&self) -> bool {
        matches!(self, AccessFilter::Limited { organizations, projects, applications } if organizations.is_empty() && projects.is_empty() && applications.is_empty())
    }
}

#[derive(Debug, Clone, Default)]
pub struct Engine {
    pub assignments: Vec<Assignment>,
    pub custom_roles: HashMap<Uuid, CustomRole>,
    pub policies: Policies,
}

/// Does a grant at `scope` confer a permission, ignoring where the resource is? Platform
/// scope confers everything; pool scope only hardware permissions; the tenant scopes confer
/// tenant permissions whose floor is at least as wide as the scope.
pub fn confers(scope: &Scope, def: &PermissionDef) -> bool {
    match scope {
        Scope::Platform => true,
        Scope::Pool { .. } => def.floor == Level::Pool,
        _ => def.class == Class::Tenant && scope.level() <= def.floor && def.floor != Level::Pool,
    }
}

/// Permissions a service identity can never hold: acting on people and approvals needs a person.
pub fn withheld_from_services(def: &PermissionDef) -> bool {
    def.class == Class::Security
        || matches!(def.id, "role.assign" | "approval.grant" | "group.manage")
}

impl Engine {
    pub fn resolve(&self, r: &RoleRef) -> Option<ResolvedRole> {
        match r {
            RoleRef::Builtin { key } => Builtin::parse(key).map(ResolvedRole::builtin),
            RoleRef::Custom { id } => self.custom_roles.get(id).map(ResolvedRole::custom),
        }
    }

    fn live(a: &Assignment, now: i64) -> bool {
        !a.revoked && a.expires_at.is_none_or(|t| t > now)
    }

    fn needs_mfa(&self, def: &PermissionDef, res: &Resource) -> bool {
        (def.class != Class::Tenant && self.policies.mfa_for_infrastructure)
            || res
                .environment
                .is_some_and(|e| self.policies.mfa_for_environments.contains(&e))
    }

    /// May `subject` use `permission` on `resource`?
    pub fn check(
        &self,
        subject: &Subject,
        permission_id: &str,
        res: &Resource,
        now: i64,
    ) -> Decision {
        let Some(def) = permission(permission_id) else {
            return Decision::deny(Denial::UnknownPermission);
        };
        if subject.service.is_some() && subject.user.is_none() && withheld_from_services(def) {
            return Decision::deny(Denial::NoApplicableAssignment);
        }
        let mut grants = Vec::new();
        let mut mfa_blocked = false;
        for a in self.assignments.iter().filter(|a| Self::live(a, now)) {
            let Some(via_group) = subject.matches(&a.principal) else {
                continue;
            };
            let Some(role) = self.resolve(&a.role) else {
                continue;
            };
            if !role.permissions.contains(def.id)
                || !confers(&a.scope, def)
                || !covers(&a.scope, res)
            {
                continue;
            }
            if let Some(envs) = &a.conditions.environments
                && !res.environment.is_some_and(|e| envs.contains(&e))
            {
                continue;
            }
            if (a.conditions.require_mfa || self.needs_mfa(def, res)) && !subject.mfa {
                mfa_blocked = true;
                continue;
            }
            grants.push(Grant {
                assignment: a.id,
                role: a.role.clone(),
                role_name: role.name,
                scope: a.scope.clone(),
                via_group,
                emergency: a.kind == AssignmentKind::Emergency,
            });
        }
        if grants.is_empty() {
            return Decision::deny(if mfa_blocked {
                Denial::MfaRequired
            } else {
                Denial::NoApplicableAssignment
            });
        }
        Decision {
            allowed: true,
            grants,
            denial: None,
        }
    }

    /// Every permission the subject holds on the resource, with where each comes from.
    pub fn effective(
        &self,
        subject: &Subject,
        res: &Resource,
        now: i64,
    ) -> BTreeMap<&'static str, Vec<Grant>> {
        PERMISSIONS
            .iter()
            .filter_map(|p| {
                let d = self.check(subject, p.id, res, now);
                d.allowed.then_some((p.id, d.grants))
            })
            .collect()
    }

    /// The scopes under which the subject holds `permission`, for restricting database queries.
    /// Grants that depend on conditions are left out (fail closed): the per-item check still
    /// lets those resources through when the conditions hold.
    pub fn filter(&self, subject: &Subject, permission_id: &str, now: i64) -> AccessFilter {
        let none = AccessFilter::Limited {
            organizations: vec![],
            projects: vec![],
            applications: vec![],
        };
        let Some(def) = permission(permission_id) else {
            return none;
        };
        if subject.service.is_some() && subject.user.is_none() && withheld_from_services(def) {
            return none;
        }
        let (mut orgs, mut projects, mut apps) = (Vec::new(), Vec::new(), Vec::new());
        for a in self.assignments.iter().filter(|a| Self::live(a, now)) {
            if subject.matches(&a.principal).is_none() {
                continue;
            }
            let Some(role) = self.resolve(&a.role) else {
                continue;
            };
            if !role.permissions.contains(def.id) || !confers(&a.scope, def) {
                continue;
            }
            if a.conditions.environments.is_some()
                || ((a.conditions.require_mfa || self.needs_mfa(def, &Resource::platform()))
                    && !subject.mfa)
            {
                continue;
            }
            match &a.scope {
                Scope::Platform => return AccessFilter::All,
                Scope::Organization { id } => orgs.push(*id),
                Scope::Project { id } => projects.push(*id),
                Scope::Application { id } => apps.push(*id),
                Scope::Pool { .. } => {}
            }
        }
        AccessFilter::Limited {
            organizations: orgs,
            projects,
            applications: apps,
        }
    }

    /// Does the subject hold the permission through any live assignment, ignoring conditions
    /// (MFA, environment) and the resource? A coarse test for gating routes; the exact decision
    /// is [`Engine::check`] on the resource.
    pub fn holds_anywhere(&self, subject: &Subject, permission_id: &str, now: i64) -> bool {
        let Some(def) = permission(permission_id) else {
            return false;
        };
        if subject.service.is_some() && subject.user.is_none() && withheld_from_services(def) {
            return false;
        }
        self.assignments
            .iter()
            .filter(|a| Self::live(a, now) && subject.matches(&a.principal).is_some())
            .any(|a| {
                confers(&a.scope, def)
                    && self
                        .resolve(&a.role)
                        .is_some_and(|r| r.permissions.contains(def.id))
            })
    }

    /// Does the action need a second person's approval?
    pub fn needs_approval(&self, permission_id: &str) -> bool {
        crate::permission::needs_approval(permission_id)
            || self.policies.extra_approval.contains(permission_id)
    }
}

#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;

    pub const NOW: i64 = 1_000_000;
    pub fn id(n: u128) -> Uuid {
        Uuid::from_u128(n)
    }
    pub fn builtin(b: Builtin) -> RoleRef {
        RoleRef::Builtin {
            key: b.key().to_owned(),
        }
    }
    pub fn assign(n: u128, principal: PrincipalRef, role: Builtin, scope: Scope) -> Assignment {
        Assignment {
            id: id(1000 + n),
            principal,
            role: builtin(role),
            scope,
            conditions: Conditions::default(),
            expires_at: None,
            revoked: false,
            kind: AssignmentKind::Standard,
        }
    }
    pub fn user(n: u128) -> Subject {
        Subject {
            user: Some(id(n)),
            ..Subject::default()
        }
    }
    pub fn engine(a: Vec<Assignment>) -> Engine {
        Engine {
            assignments: a,
            ..Engine::default()
        }
    }
    /// Two organisations, each with one project and one application.
    pub struct World {
        pub acme: Uuid,
        pub acme_prod: Uuid,
        pub acme_dev: Uuid,
        pub portal: Uuid,
        pub other: Uuid,
        pub other_proj: Uuid,
        pub other_app: Uuid,
    }
    pub fn world() -> World {
        World {
            acme: id(10),
            acme_prod: id(11),
            acme_dev: id(12),
            portal: id(13),
            other: id(20),
            other_proj: id(21),
            other_app: id(22),
        }
    }
    impl World {
        pub fn portal_res(&self) -> Resource {
            Resource::application(
                self.acme,
                self.acme_prod,
                self.portal,
                Some(EnvKind::Production),
            )
        }
        pub fn other_res(&self) -> Resource {
            Resource::application(
                self.other,
                self.other_proj,
                self.other_app,
                Some(EnvKind::Production),
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;

    fn u(n: u128) -> PrincipalRef {
        PrincipalRef::User { id: id(n) }
    }

    #[test]
    fn no_assignment_means_no_access() {
        let w = world();
        let e = engine(vec![]);
        let d = e.check(&user(1), "application.view", &w.portal_res(), NOW);
        assert!(!d.allowed);
        assert_eq!(d.denial, Some(Denial::NoApplicableAssignment));
        assert!(
            !e.check(&user(1), "no.such", &Resource::platform(), NOW)
                .allowed
        );
        assert_eq!(
            e.check(&user(1), "no.such", &Resource::platform(), NOW)
                .denial,
            Some(Denial::UnknownPermission)
        );
    }

    #[test]
    fn an_organisation_role_reaches_its_own_projects_and_never_another_organisation() {
        let w = world();
        let e = engine(vec![assign(
            1,
            u(1),
            Builtin::OrganizationOwner,
            Scope::Organization { id: w.acme },
        )]);
        assert!(
            e.check(&user(1), "application.deploy", &w.portal_res(), NOW)
                .allowed,
            "inherited downwards"
        );
        assert!(
            e.check(
                &user(1),
                "project.create",
                &Resource::organization(w.acme),
                NOW
            )
            .allowed
        );
        assert!(
            !e.check(&user(1), "application.view", &w.other_res(), NOW)
                .allowed,
            "cross-organisation denied"
        );
        assert!(
            !e.check(
                &user(1),
                "project.create",
                &Resource::organization(w.other),
                NOW
            )
            .allowed
        );
        assert!(
            !e.check(&user(1), "application.view", &Resource::platform(), NOW)
                .allowed,
            "platform-level resources are out of reach"
        );
    }

    #[test]
    fn the_same_person_can_hold_different_roles_in_different_projects() {
        let w = world();
        let e = engine(vec![
            assign(
                1,
                u(1),
                Builtin::ProjectAdministrator,
                Scope::Project { id: w.acme_prod },
            ),
            assign(2, u(1), Builtin::Viewer, Scope::Project { id: w.acme_dev }),
        ]);
        let dev = Resource::application(w.acme, w.acme_dev, id(14), Some(EnvKind::Development));
        assert!(
            e.check(&user(1), "application.deploy", &w.portal_res(), NOW)
                .allowed
        );
        assert!(e.check(&user(1), "application.view", &dev, NOW).allowed);
        assert!(
            !e.check(&user(1), "application.deploy", &dev, NOW).allowed,
            "viewer in the other project"
        );
        assert!(
            !e.check(&user(1), "application.view", &w.other_res(), NOW)
                .allowed
        );
    }

    #[test]
    fn an_application_level_role_does_not_leak_to_sibling_applications_or_the_project() {
        let w = world();
        let e = engine(vec![assign(
            1,
            u(1),
            Builtin::ApplicationOperator,
            Scope::Application { id: w.portal },
        )]);
        assert!(
            e.check(&user(1), "application.deploy", &w.portal_res(), NOW)
                .allowed
        );
        let sibling = Resource::application(w.acme, w.acme_prod, id(15), None);
        assert!(
            !e.check(&user(1), "application.deploy", &sibling, NOW)
                .allowed
        );
        assert!(
            !e.check(
                &user(1),
                "project.view",
                &Resource::project(w.acme, w.acme_prod),
                NOW
            )
            .allowed
        );
    }

    #[test]
    fn an_organisation_owner_never_reaches_infrastructure_even_with_a_custom_infrastructure_role() {
        let w = world();
        for p in PERMISSIONS.iter().filter(|p| p.class != Class::Tenant) {
            let e = engine(vec![assign(
                1,
                u(1),
                Builtin::OrganizationOwner,
                Scope::Organization { id: w.acme },
            )]);
            assert!(
                !e.check(&user(1), p.id, &w.portal_res(), NOW).allowed,
                "{}",
                p.id
            );
            assert!(
                !e.check(&user(1), p.id, &Resource::platform(), NOW).allowed,
                "{}",
                p.id
            );
            assert!(
                !e.check(&user(1), p.id, &Resource::pool("rack-1"), NOW)
                    .allowed,
                "{}",
                p.id
            );
        }
        // Even a role that lists infrastructure permissions grants none of them below the platform.
        let custom = CustomRole {
            id: id(500),
            name: "Sneaky".into(),
            description: String::new(),
            permissions: [
                "cluster.manage",
                "hardware.power",
                "token.manage",
                "application.view",
            ]
            .map(String::from)
            .into(),
        };
        let mut e = engine(vec![Assignment {
            role: RoleRef::Custom { id: custom.id },
            ..assign(1, u(1), Builtin::Viewer, Scope::Organization { id: w.acme })
        }]);
        e.custom_roles.insert(custom.id, custom);
        assert!(
            e.check(&user(1), "application.view", &w.portal_res(), NOW)
                .allowed
        );
        for p in ["cluster.manage", "hardware.power", "token.manage"] {
            assert!(
                !e.check(&user(1), p, &Resource::platform(), NOW).allowed,
                "{p}"
            );
            assert!(!e.check(&user(1), p, &w.portal_res(), NOW).allowed, "{p}");
        }
    }

    #[test]
    fn hardware_operators_work_in_their_pool_only_and_have_no_cluster_or_application_access() {
        let e = engine(vec![assign(
            1,
            u(1),
            Builtin::HardwareOperator,
            Scope::Pool {
                name: "rack-1".into(),
            },
        )]);
        assert!(
            e.check(&user(1), "hardware.deploy", &Resource::pool("rack-1"), NOW)
                .allowed
        );
        assert!(
            !e.check(&user(1), "hardware.deploy", &Resource::pool("rack-2"), NOW)
                .allowed
        );
        assert!(
            !e.check(&user(1), "hardware.view", &Resource::platform(), NOW)
                .allowed
        );
        assert!(
            !e.check(&user(1), "cluster.view", &Resource::platform(), NOW)
                .allowed
        );
        assert!(
            !e.check(&user(1), "application.view", &world().portal_res(), NOW)
                .allowed
        );
    }

    #[test]
    fn group_membership_grants_and_is_reported_as_the_origin() {
        let w = world();
        let g = id(70);
        let e = engine(vec![assign(
            1,
            PrincipalRef::Group { id: g },
            Builtin::ApplicationDesigner,
            Scope::Project { id: w.acme_prod },
        )]);
        let mut s = user(1);
        assert!(
            !e.check(&s, "application.design", &w.portal_res(), NOW)
                .allowed
        );
        s.groups.insert(g);
        let d = e.check(&s, "application.design", &w.portal_res(), NOW);
        assert!(d.allowed);
        assert_eq!(d.grants[0].via_group, Some(g));
        assert_eq!(d.grants[0].role_name, "Application Designer");
    }

    #[test]
    fn revoked_and_expired_assignments_stop_working() {
        let w = world();
        let base = assign(
            1,
            u(1),
            Builtin::ApplicationOperator,
            Scope::Project { id: w.acme_prod },
        );
        let live = engine(vec![base.clone()]);
        assert!(
            live.check(&user(1), "application.deploy", &w.portal_res(), NOW)
                .allowed
        );
        let revoked = engine(vec![Assignment {
            revoked: true,
            ..base.clone()
        }]);
        assert!(
            !revoked
                .check(&user(1), "application.deploy", &w.portal_res(), NOW)
                .allowed
        );
        let expiring = engine(vec![Assignment {
            expires_at: Some(NOW + 10),
            ..base.clone()
        }]);
        assert!(
            expiring
                .check(&user(1), "application.deploy", &w.portal_res(), NOW)
                .allowed
        );
        assert!(
            !expiring
                .check(&user(1), "application.deploy", &w.portal_res(), NOW + 10)
                .allowed,
            "expires at the instant"
        );
        assert!(
            !expiring
                .check(&user(1), "application.deploy", &w.portal_res(), NOW + 11)
                .allowed
        );
    }

    #[test]
    fn an_unknown_role_grants_nothing() {
        let w = world();
        let a = Assignment {
            role: RoleRef::Custom { id: id(404) },
            ..assign(1, u(1), Builtin::Viewer, Scope::Platform)
        };
        assert!(
            !engine(vec![a.clone()])
                .check(&user(1), "application.view", &w.portal_res(), NOW)
                .allowed
        );
        let b = Assignment {
            role: RoleRef::Builtin { key: "root".into() },
            ..a
        };
        assert!(
            !engine(vec![b])
                .check(&user(1), "application.view", &w.portal_res(), NOW)
                .allowed
        );
    }

    #[test]
    fn mfa_conditions_and_policies_narrow_a_grant() {
        let w = world();
        let mut a = assign(
            1,
            u(1),
            Builtin::ApplicationOperator,
            Scope::Project { id: w.acme_prod },
        );
        a.conditions.require_mfa = true;
        let e = engine(vec![a]);
        let d = e.check(&user(1), "application.deploy", &w.portal_res(), NOW);
        assert_eq!((d.allowed, d.denial), (false, Some(Denial::MfaRequired)));
        let with_mfa = Subject {
            mfa: true,
            ..user(1)
        };
        assert!(
            e.check(&with_mfa, "application.deploy", &w.portal_res(), NOW)
                .allowed
        );

        let mut e2 = engine(vec![assign(
            1,
            u(1),
            Builtin::ProjectAdministrator,
            Scope::Project { id: w.acme_prod },
        )]);
        e2.policies.mfa_for_environments = vec![EnvKind::Production];
        assert_eq!(
            e2.check(&user(1), "application.deploy", &w.portal_res(), NOW)
                .denial,
            Some(Denial::MfaRequired)
        );
        let dev = Resource::application(w.acme, w.acme_prod, id(14), Some(EnvKind::Development));
        assert!(
            e2.check(&user(1), "application.deploy", &dev, NOW).allowed,
            "the policy names production only"
        );
        let mut e3 = engine(vec![assign(
            1,
            u(1),
            Builtin::FabricAdministrator,
            Scope::Platform,
        )]);
        e3.policies.mfa_for_infrastructure = true;
        assert!(
            !e3.check(&user(1), "cluster.manage", &Resource::platform(), NOW)
                .allowed
        );
        assert!(
            e3.check(
                &Subject {
                    mfa: true,
                    ..user(1)
                },
                "cluster.manage",
                &Resource::platform(),
                NOW
            )
            .allowed
        );
    }

    #[test]
    fn environment_conditions_limit_a_grant_to_those_kinds() {
        let w = world();
        let mut a = assign(
            1,
            u(1),
            Builtin::ApplicationOperator,
            Scope::Project { id: w.acme_prod },
        );
        a.conditions.environments = Some(vec![EnvKind::Development]);
        let e = engine(vec![a]);
        assert!(
            !e.check(&user(1), "application.deploy", &w.portal_res(), NOW)
                .allowed,
            "production is not development"
        );
        let dev = Resource::application(w.acme, w.acme_prod, id(14), Some(EnvKind::Development));
        assert!(e.check(&user(1), "application.deploy", &dev, NOW).allowed);
    }

    #[test]
    fn service_identities_never_hold_people_or_security_permissions() {
        let svc = Subject {
            service: Some(id(90)),
            ..Subject::default()
        };
        let e = engine(vec![assign(
            1,
            PrincipalRef::Service { id: id(90) },
            Builtin::PlatformSuperAdmin,
            Scope::Platform,
        )]);
        assert!(
            e.check(&svc, "cluster.view", &Resource::platform(), NOW)
                .allowed,
            "ordinary permissions still work"
        );
        for p in [
            "token.manage",
            "role.manage",
            "role.assign",
            "approval.grant",
            "platform.emergency_access",
            "group.manage",
            "user.manage",
        ] {
            assert!(!e.check(&svc, p, &Resource::platform(), NOW).allowed, "{p}");
        }
        assert_eq!(
            e.filter(&svc, "role.assign", NOW),
            AccessFilter::Limited {
                organizations: vec![],
                projects: vec![],
                applications: vec![]
            }
        );
    }

    #[test]
    fn the_filter_lists_exactly_the_scopes_that_confer_the_permission() {
        let w = world();
        let e = engine(vec![
            assign(
                1,
                u(1),
                Builtin::ProjectAdministrator,
                Scope::Project { id: w.acme_prod },
            ),
            assign(
                2,
                u(1),
                Builtin::ApplicationOperator,
                Scope::Application { id: id(77) },
            ),
            assign(
                3,
                u(1),
                Builtin::Viewer,
                Scope::Organization { id: w.other },
            ),
            assign(4, u(2), Builtin::Viewer, Scope::Platform),
        ]);
        assert_eq!(
            e.filter(&user(1), "application.deploy", NOW),
            AccessFilter::Limited {
                organizations: vec![],
                projects: vec![w.acme_prod],
                applications: vec![id(77)]
            }
        );
        assert_eq!(
            e.filter(&user(1), "application.view", NOW),
            AccessFilter::Limited {
                organizations: vec![w.other],
                projects: vec![w.acme_prod],
                applications: vec![id(77)]
            }
        );
        assert_eq!(
            e.filter(&user(2), "application.view", NOW),
            AccessFilter::All
        );
        assert!(e.filter(&user(3), "application.view", NOW).is_empty());
        assert!(
            e.filter(&user(2), "application.deploy", NOW).is_empty(),
            "a viewer cannot deploy anywhere"
        );
    }

    #[test]
    fn effective_permissions_name_their_origin() {
        let w = world();
        let e = engine(vec![
            assign(
                1,
                u(1),
                Builtin::ApplicationDesigner,
                Scope::Project { id: w.acme_prod },
            ),
            assign(2, u(1), Builtin::Viewer, Scope::Organization { id: w.acme }),
        ]);
        let eff = e.effective(&user(1), &w.portal_res(), NOW);
        assert_eq!(eff["application.design"].len(), 1);
        assert_eq!(
            eff["application.view"].len(),
            2,
            "granted twice: both origins are shown"
        );
        assert!(!eff.contains_key("application.deploy"));
    }

    #[test]
    fn emergency_grants_are_flagged() {
        let w = world();
        let mut a = assign(1, u(1), Builtin::PlatformSuperAdmin, Scope::Platform);
        a.kind = AssignmentKind::Emergency;
        a.expires_at = Some(NOW + 3600);
        let d = engine(vec![a]).check(&user(1), "cluster.manage", &w.portal_res(), NOW);
        assert!(d.allowed && d.grants[0].emergency);
    }

    #[test]
    fn holds_anywhere_ignores_conditions_and_resource_but_not_class_or_expiry() {
        let w = world();
        let mut a = assign(
            1,
            u(1),
            Builtin::ApplicationOperator,
            Scope::Project { id: w.acme_prod },
        );
        a.conditions.require_mfa = true;
        let e = engine(vec![
            a,
            assign(
                2,
                u(1),
                Builtin::OrganizationOwner,
                Scope::Organization { id: w.other },
            ),
        ]);
        assert!(e.holds_anywhere(&user(1), "application.deploy", NOW));
        assert!(
            !e.holds_anywhere(&user(1), "cluster.manage", NOW),
            "organisation scope never confers infrastructure"
        );
        assert!(!e.holds_anywhere(&user(2), "application.deploy", NOW));
        assert!(!e.holds_anywhere(&user(1), "no.such", NOW));
    }

    #[test]
    fn approvals_are_required_for_the_protected_actions_and_policy_can_add_more() {
        let mut e = Engine::default();
        assert!(e.needs_approval("hardware.delete"));
        assert!(!e.needs_approval("cluster.manage"));
        e.policies.extra_approval.insert("cluster.manage".into());
        assert!(e.needs_approval("cluster.manage"));
    }
}
