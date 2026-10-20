// SPDX-License-Identifier: AGPL-3.0-only
//! Who may grant a role: the guard against privilege escalation.
//!
//! A person may grant a role only if they could use every permission in it themselves at the
//! target scope, so nobody can create a more powerful account than their own. Roles that
//! carry infrastructure or security permissions can be granted only at the platform (or pool)
//! level, and only by someone who holds them there; an organisation owner can therefore never
//! hand out, or obtain, physical-server administration.

use crate::evaluate::{Engine, PrincipalRef, Subject, confers, withheld_from_services};
use crate::permission::permission;
use crate::role::ResolvedRole;
use crate::scope::{Resource, Scope};

pub struct AssignRequest<'a> {
    pub role: &'a ResolvedRole,
    pub scope: &'a Scope,
    /// The resource the scope stands for, built from the database (an organisation scope is
    /// `Resource::organization(id)`, and so on).
    pub resource: &'a Resource,
    pub principal: PrincipalRef,
    pub emergency: bool,
    pub expires_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssignError {
    /// The role cannot be assigned at that level (for example an infrastructure role to a project).
    RoleNotAssignableAtScope,
    /// The caller may not grant roles in that scope.
    NotPermitted,
    /// The role holds permissions the caller does not hold there.
    WouldEscalate(Vec<String>),
    /// A service identity may not receive these permissions.
    ForbiddenForService(Vec<String>),
    /// Emergency grants need the emergency permission, an expiry and a short life.
    EmergencyRules(&'static str),
}

impl std::fmt::Display for AssignError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AssignError::RoleNotAssignableAtScope => {
                f.write_str("that role cannot be assigned at that scope")
            }
            AssignError::NotPermitted => f.write_str("you may not grant roles in that scope"),
            AssignError::WouldEscalate(p) => write!(
                f,
                "the role contains permissions you do not hold there: {}",
                p.join(", ")
            ),
            AssignError::ForbiddenForService(p) => {
                write!(f, "a service identity may not hold: {}", p.join(", "))
            }
            AssignError::EmergencyRules(why) => f.write_str(why),
        }
    }
}

/// Longest emergency grant, in seconds.
pub const MAX_EMERGENCY_SECS: i64 = 4 * 3600;

pub fn check_assignment(
    engine: &Engine,
    actor: &Subject,
    req: &AssignRequest<'_>,
    now: i64,
) -> Result<(), AssignError> {
    if !req.role.may_be_assigned_at(req.scope) {
        return Err(AssignError::RoleNotAssignableAtScope);
    }
    // For pool or platform grants the "scope resource" is the platform or the pool itself.
    if !engine
        .check(actor, "role.assign", req.resource, now)
        .allowed
    {
        return Err(AssignError::NotPermitted);
    }
    // Only permissions that would take effect at the target scope count: the rest are inert there.
    let missing: Vec<String> = req
        .role
        .permissions
        .iter()
        .filter(|p| permission(p).is_some_and(|d| confers(req.scope, d)))
        .filter(|p| !engine.check(actor, p, req.resource, now).allowed)
        .cloned()
        .collect();
    if !missing.is_empty() {
        return Err(AssignError::WouldEscalate(missing));
    }
    if matches!(req.principal, PrincipalRef::Service { .. }) {
        let bad: Vec<String> = req
            .role
            .permissions
            .iter()
            .filter(|p| permission(p).is_some_and(withheld_from_services))
            .cloned()
            .collect();
        if !bad.is_empty() {
            return Err(AssignError::ForbiddenForService(bad));
        }
    }
    if req.emergency {
        if !engine
            .check(
                actor,
                "platform.emergency_access",
                &Resource::platform(),
                now,
            )
            .allowed
        {
            return Err(AssignError::EmergencyRules(
                "emergency access needs the platform.emergency_access permission",
            ));
        }
        match req.expires_at {
            None => return Err(AssignError::EmergencyRules("emergency access must expire")),
            Some(t) if t <= now => {
                return Err(AssignError::EmergencyRules("the expiry is in the past"));
            }
            Some(t) if t - now > MAX_EMERGENCY_SECS => {
                return Err(AssignError::EmergencyRules(
                    "emergency access lasts at most 4 hours",
                ));
            }
            Some(_) => {}
        }
        if !matches!(req.scope, Scope::Platform) {
            return Err(AssignError::EmergencyRules(
                "emergency access is granted at the platform level",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::evaluate::{Assignment, fixtures::*};
    use crate::role::{Builtin, CustomRole, RoleRef};

    fn req<'a>(role: &'a ResolvedRole, scope: &'a Scope, res: &'a Resource) -> AssignRequest<'a> {
        AssignRequest {
            role,
            scope,
            resource: res,
            principal: PrincipalRef::User { id: id(9) },
            emergency: false,
            expires_at: None,
        }
    }
    fn user_ref(n: u128) -> PrincipalRef {
        PrincipalRef::User { id: id(n) }
    }

    #[test]
    fn an_organisation_owner_can_grant_tenant_roles_inside_the_organisation() {
        let w = world();
        let e = engine(vec![assign(
            1,
            user_ref(1),
            Builtin::OrganizationOwner,
            Scope::Organization { id: w.acme },
        )]);
        let scope = Scope::Project { id: w.acme_prod };
        let res = Resource::project(w.acme, w.acme_prod);
        for b in [
            Builtin::ProjectAdministrator,
            Builtin::ApplicationOperator,
            Builtin::Viewer,
        ] {
            let role = ResolvedRole::builtin(b);
            assert_eq!(
                check_assignment(&e, &user(1), &req(&role, &scope, &res), NOW),
                Ok(()),
                "{b:?}"
            );
        }
    }

    #[test]
    fn an_organisation_owner_cannot_grant_across_organisations_or_infrastructure_roles() {
        let w = world();
        let e = engine(vec![assign(
            1,
            user_ref(1),
            Builtin::OrganizationOwner,
            Scope::Organization { id: w.acme },
        )]);
        let role = ResolvedRole::builtin(Builtin::Viewer);
        let other_scope = Scope::Organization { id: w.other };
        let other_res = Resource::organization(w.other);
        assert_eq!(
            check_assignment(&e, &user(1), &req(&role, &other_scope, &other_res), NOW),
            Err(AssignError::NotPermitted)
        );
        for b in [
            Builtin::FabricAdministrator,
            Builtin::HardwareOperator,
            Builtin::PlatformSuperAdmin,
        ] {
            let role = ResolvedRole::builtin(b);
            let platform = check_assignment(
                &e,
                &user(1),
                &req(&role, &Scope::Platform, &Resource::platform()),
                NOW,
            );
            assert_eq!(
                platform,
                Err(AssignError::NotPermitted),
                "{b:?} at platform"
            );
            let org = Scope::Organization { id: w.acme };
            let org_res = Resource::organization(w.acme);
            assert_eq!(
                check_assignment(&e, &user(1), &req(&role, &org, &org_res), NOW),
                Err(AssignError::RoleNotAssignableAtScope),
                "{b:?} at organisation"
            );
        }
    }

    #[test]
    fn nobody_can_grant_more_than_they_hold() {
        let w = world();
        // The actor may assign roles in the project and see applications, but may not deploy.
        let delegator = CustomRole {
            id: id(600),
            name: "Delegator".into(),
            description: String::new(),
            permissions: ["role.assign", "application.view"].map(String::from).into(),
        };
        let mut e = engine(vec![Assignment {
            role: RoleRef::Custom { id: delegator.id },
            ..assign(
                1,
                user_ref(1),
                Builtin::Viewer,
                Scope::Project { id: w.acme_prod },
            )
        }]);
        e.custom_roles.insert(delegator.id, delegator);
        let scope = Scope::Project { id: w.acme_prod };
        let res = Resource::project(w.acme, w.acme_prod);
        let operator = ResolvedRole::builtin(Builtin::ApplicationOperator);
        match check_assignment(&e, &user(1), &req(&operator, &scope, &res), NOW) {
            Err(AssignError::WouldEscalate(missing)) => assert!(
                missing.contains(&"application.deploy".to_string()),
                "{missing:?}"
            ),
            other => panic!("expected escalation refusal, got {other:?}"),
        }
        // Roles whose permissions are inert at the scope (organisation-level views) are not counted.
        let viewer = ResolvedRole::builtin(Builtin::Viewer);
        assert!(
            matches!(check_assignment(&e, &user(1), &req(&viewer, &scope, &res), NOW), Err(AssignError::WouldEscalate(m)) if !m.contains(&"organization.view".to_string()))
        );
    }

    #[test]
    fn a_project_administrator_can_grant_what_they_hold_in_their_project_only() {
        let w = world();
        let e = engine(vec![assign(
            1,
            user_ref(1),
            Builtin::ProjectAdministrator,
            Scope::Project { id: w.acme_prod },
        )]);
        let scope = Scope::Project { id: w.acme_prod };
        let res = Resource::project(w.acme, w.acme_prod);
        for b in [
            Builtin::ApplicationOperator,
            Builtin::ApplicationDesigner,
            Builtin::Viewer,
            Builtin::ProjectAdministrator,
        ] {
            let role = ResolvedRole::builtin(b);
            assert_eq!(
                check_assignment(&e, &user(1), &req(&role, &scope, &res), NOW),
                Ok(()),
                "{b:?}"
            );
        }
        let dev = Scope::Project { id: w.acme_dev };
        let dev_res = Resource::project(w.acme, w.acme_dev);
        let role = ResolvedRole::builtin(Builtin::Viewer);
        assert_eq!(
            check_assignment(&e, &user(1), &req(&role, &dev, &dev_res), NOW),
            Err(AssignError::NotPermitted),
            "sibling project"
        );
        let org = ResolvedRole::builtin(Builtin::OrganizationOwner);
        let org_scope = Scope::Organization { id: w.acme };
        assert_eq!(
            check_assignment(
                &e,
                &user(1),
                &req(&org, &org_scope, &Resource::organization(w.acme)),
                NOW
            ),
            Err(AssignError::NotPermitted),
            "cannot climb to the organisation"
        );
    }

    #[test]
    fn a_designer_cannot_grant_anything() {
        let w = world();
        let e = engine(vec![assign(
            1,
            user_ref(1),
            Builtin::ApplicationDesigner,
            Scope::Project { id: w.acme_prod },
        )]);
        let role = ResolvedRole::builtin(Builtin::Viewer);
        let scope = Scope::Project { id: w.acme_prod };
        let res = Resource::project(w.acme, w.acme_prod);
        assert_eq!(
            check_assignment(&e, &user(1), &req(&role, &scope, &res), NOW),
            Err(AssignError::NotPermitted)
        );
    }

    #[test]
    fn only_a_super_admin_can_grant_platform_roles() {
        let e = engine(vec![
            assign(1, user_ref(1), Builtin::PlatformSuperAdmin, Scope::Platform),
            assign(
                2,
                user_ref(2),
                Builtin::FabricAdministrator,
                Scope::Platform,
            ),
        ]);
        let role = ResolvedRole::builtin(Builtin::HardwareOperator);
        let scope = Scope::Pool {
            name: "rack-1".into(),
        };
        let res = Resource::pool("rack-1");
        assert_eq!(
            check_assignment(&e, &user(1), &req(&role, &scope, &res), NOW),
            Ok(())
        );
        assert_eq!(
            check_assignment(&e, &user(2), &req(&role, &scope, &res), NOW),
            Err(AssignError::NotPermitted),
            "fabric administrators do not manage roles"
        );
    }

    #[test]
    fn service_identities_cannot_be_given_security_or_people_permissions() {
        let e = engine(vec![assign(
            1,
            user_ref(1),
            Builtin::PlatformSuperAdmin,
            Scope::Platform,
        )]);
        let role = ResolvedRole::builtin(Builtin::PlatformSuperAdmin);
        let plat = Resource::platform();
        let mut r = req(&role, &Scope::Platform, &plat);
        r.principal = PrincipalRef::Service { id: id(80) };
        assert!(matches!(
            check_assignment(&e, &user(1), &r, NOW),
            Err(AssignError::ForbiddenForService(_))
        ));
        let ok = ResolvedRole::builtin(Builtin::FabricAdministrator);
        let mut r2 = req(&ok, &Scope::Platform, &plat);
        r2.principal = PrincipalRef::Service { id: id(80) };
        assert!(
            matches!(check_assignment(&e, &user(1), &r2, NOW), Err(AssignError::ForbiddenForService(p)) if p == vec!["approval.grant".to_string()])
        );
    }

    #[test]
    fn emergency_grants_need_the_permission_a_short_expiry_and_the_platform_scope() {
        let e = engine(vec![
            assign(1, user_ref(1), Builtin::PlatformSuperAdmin, Scope::Platform),
            assign(
                2,
                user_ref(2),
                Builtin::FabricAdministrator,
                Scope::Platform,
            ),
        ]);
        let role = ResolvedRole::builtin(Builtin::FabricAdministrator);
        let plat = Resource::platform();
        let mut r = req(&role, &Scope::Platform, &plat);
        r.emergency = true;
        assert!(
            matches!(
                check_assignment(&e, &user(1), &r, NOW),
                Err(AssignError::EmergencyRules(_))
            ),
            "no expiry"
        );
        r.expires_at = Some(NOW + MAX_EMERGENCY_SECS + 1);
        assert!(
            matches!(
                check_assignment(&e, &user(1), &r, NOW),
                Err(AssignError::EmergencyRules(_))
            ),
            "too long"
        );
        r.expires_at = Some(NOW - 1);
        assert!(
            matches!(
                check_assignment(&e, &user(1), &r, NOW),
                Err(AssignError::EmergencyRules(_))
            ),
            "past"
        );
        r.expires_at = Some(NOW + 3600);
        assert_eq!(check_assignment(&e, &user(1), &r, NOW), Ok(()));
        assert!(
            check_assignment(&e, &user(2), &r, NOW).is_err(),
            "a fabric administrator cannot grant it"
        );
    }
}
