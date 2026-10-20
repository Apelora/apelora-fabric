// SPDX-License-Identifier: AGPL-3.0-only
//! Predefined roles (fixed in code, so a database edit cannot widen them) and custom roles
//! (explicit permission lists stored in the database).

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::permission::{Class, PERMISSIONS, ids_where, permission};
use crate::scope::{Level, Scope};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Builtin {
    PlatformSuperAdmin,
    FabricAdministrator,
    HardwareOperator,
    OrganizationOwner,
    ProjectAdministrator,
    ApplicationDesigner,
    ApplicationOperator,
    InstanceAccess,
    Viewer,
    Auditor,
}

impl Builtin {
    pub const ALL: [Builtin; 10] = [
        Builtin::PlatformSuperAdmin,
        Builtin::FabricAdministrator,
        Builtin::HardwareOperator,
        Builtin::OrganizationOwner,
        Builtin::ProjectAdministrator,
        Builtin::ApplicationDesigner,
        Builtin::ApplicationOperator,
        Builtin::InstanceAccess,
        Builtin::Viewer,
        Builtin::Auditor,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Builtin::PlatformSuperAdmin => "platform-super-admin",
            Builtin::FabricAdministrator => "fabric-administrator",
            Builtin::HardwareOperator => "hardware-operator",
            Builtin::OrganizationOwner => "organization-owner",
            Builtin::ProjectAdministrator => "project-administrator",
            Builtin::ApplicationDesigner => "application-designer",
            Builtin::ApplicationOperator => "application-operator",
            Builtin::InstanceAccess => "instance-access",
            Builtin::Viewer => "viewer",
            Builtin::Auditor => "auditor",
        }
    }

    pub fn parse(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|b| b.key() == key)
    }

    pub fn name(self) -> &'static str {
        match self {
            Builtin::PlatformSuperAdmin => "Platform Super Admin",
            Builtin::FabricAdministrator => "Fabric Administrator",
            Builtin::HardwareOperator => "Hardware Operator",
            Builtin::OrganizationOwner => "Organization Owner",
            Builtin::ProjectAdministrator => "Project Administrator",
            Builtin::ApplicationDesigner => "Application Designer",
            Builtin::ApplicationOperator => "Application Operator",
            Builtin::InstanceAccess => "Instance Access",
            Builtin::Viewer => "Viewer",
            Builtin::Auditor => "Auditor",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Builtin::PlatformSuperAdmin => {
                "Everything, including identity, security settings and emergency access. Keep to a few named people."
            }
            Builtin::FabricAdministrator => {
                "Runs the cluster: members, networks, storage and enrolment. Does not manage people or hardware power."
            }
            Builtin::HardwareOperator => {
                "Commissions, installs and powers physical machines in assigned hardware pools. No Incus or application access."
            }
            Builtin::OrganizationOwner => {
                "Runs one organisation: its projects, people and applications, within the limits the platform allocates. No infrastructure access."
            }
            Builtin::ProjectAdministrator => {
                "Runs one project: its members, environments and applications."
            }
            Builtin::ApplicationDesigner => {
                "Creates and edits application graphs and previews plans. Cannot deploy."
            }
            Builtin::ApplicationOperator => {
                "Deploys approved designs and starts, stops and recovers applications."
            }
            Builtin::InstanceAccess => {
                "Opens a terminal, PowerShell or remote desktop session on running modules, and reads the scope. Cannot change or operate anything else; production sessions need another person's approval."
            }
            Builtin::Viewer => "Reads what the scope contains.",
            Builtin::Auditor => {
                "Reads what the scope contains, plus its audit trail, changing nothing."
            }
        }
    }

    /// Scope levels at which this role may be assigned.
    pub fn scopes(self) -> &'static [Level] {
        use Level::*;
        match self {
            Builtin::PlatformSuperAdmin | Builtin::FabricAdministrator => &[Platform],
            Builtin::HardwareOperator => &[Platform, Pool],
            Builtin::OrganizationOwner => &[Organization],
            Builtin::ProjectAdministrator => &[Project],
            Builtin::ApplicationDesigner => &[Project, Application],
            Builtin::ApplicationOperator => &[Project, Application],
            Builtin::InstanceAccess => &[Project, Application],
            Builtin::Viewer => &[Platform, Organization, Project, Application],
            Builtin::Auditor => &[Platform, Organization, Project, Application],
        }
    }

    pub fn permissions(self) -> BTreeSet<&'static str> {
        let tenant_all = || ids_where(|p| p.class == Class::Tenant);
        let list: Vec<&'static str> = match self {
            Builtin::PlatformSuperAdmin => PERMISSIONS.iter().map(|p| p.id).collect(),
            Builtin::FabricAdministrator => vec![
                "cluster.view",
                "cluster.manage",
                "cluster.evacuate",
                "cluster.evacuate_unsafe",
                "cluster.sensitive_change",
                "network.manage",
                "storage.manage",
                "machine.enrol",
                "hardware.view",
                "organization.view",
                "project.view",
                "application.view",
                "application.metrics.read",
                "audit.read",
                "approval.request",
                "approval.grant",
            ],
            Builtin::HardwareOperator => vec![
                "hardware.view",
                "hardware.commission",
                "hardware.deploy",
                "hardware.power",
                "hardware.delete",
                "approval.request",
            ],
            Builtin::OrganizationOwner => tenant_all(),
            Builtin::ProjectAdministrator => tenant_all()
                .into_iter()
                .filter(|id| {
                    permission(id)
                        .is_some_and(|d| matches!(d.floor, Level::Project | Level::Application))
                })
                .collect(),
            Builtin::ApplicationDesigner => vec![
                "project.view",
                "application.view",
                "application.create",
                "application.design",
                "application.logs.read",
                "application.metrics.read",
            ],
            Builtin::ApplicationOperator => vec![
                "project.view",
                "application.view",
                "application.deploy",
                "application.operate",
                "application.logs.read",
                "application.metrics.read",
                "approval.request",
            ],
            Builtin::InstanceAccess => vec![
                "project.view",
                "application.view",
                "application.logs.read",
                "application.metrics.read",
                "instance.console",
                "instance.desktop",
                "approval.request",
            ],
            Builtin::Viewer => vec![
                "organization.view",
                "project.view",
                "application.view",
                "application.metrics.read",
                "cluster.view",
                "hardware.view",
            ],
            Builtin::Auditor => vec![
                "organization.view",
                "project.view",
                "application.view",
                "application.metrics.read",
                "application.logs.read",
                "audit.read",
                "user.view",
                "cluster.view",
                "hardware.view",
            ],
        };
        list.into_iter().collect()
    }
}

/// A role defined by an administrator from an explicit list of permission ids.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
pub struct CustomRole {
    pub id: Uuid,
    pub name: String,
    pub description: String,
    pub permissions: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum RoleRef {
    Builtin { key: String },
    Custom { id: Uuid },
}

/// A role with its permissions resolved, whatever its origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedRole {
    pub reference: RoleRef,
    pub name: String,
    pub permissions: BTreeSet<String>,
    pub scopes: Vec<Level>,
}

impl ResolvedRole {
    pub fn builtin(b: Builtin) -> Self {
        Self {
            reference: RoleRef::Builtin {
                key: b.key().to_owned(),
            },
            name: b.name().to_owned(),
            permissions: b.permissions().into_iter().map(str::to_owned).collect(),
            scopes: b.scopes().to_vec(),
        }
    }

    pub fn custom(c: &CustomRole) -> Self {
        Self {
            reference: RoleRef::Custom { id: c.id },
            name: c.name.clone(),
            permissions: c.permissions.clone(),
            scopes: custom_scopes(&c.permissions),
        }
    }

    pub fn may_be_assigned_at(&self, scope: &Scope) -> bool {
        self.scopes.contains(&scope.level())
    }
}

/// The scopes a custom role may be assigned at follow from what it contains: a role with any
/// security permission is platform-only; one with infrastructure permissions only is
/// platform or pool; a purely tenant role may go anywhere in the tenant tree.
pub fn custom_scopes(perms: &BTreeSet<String>) -> Vec<Level> {
    let classes: Vec<Class> = perms
        .iter()
        .filter_map(|p| permission(p))
        .map(|d| d.class)
        .collect();
    if classes.contains(&Class::Security)
        || (classes.contains(&Class::Infrastructure) && classes.contains(&Class::Tenant))
    {
        return vec![Level::Platform];
    }
    if classes.contains(&Class::Infrastructure) {
        let all_pool = perms
            .iter()
            .filter_map(|p| permission(p))
            .all(|d| d.floor == Level::Pool);
        return if all_pool {
            vec![Level::Platform, Level::Pool]
        } else {
            vec![Level::Platform]
        };
    }
    // A tenant role may sit anywhere down to the narrowest floor among its permissions.
    let narrowest = perms
        .iter()
        .filter_map(|p| permission(p))
        .map(|d| d.floor)
        .max()
        .unwrap_or(Level::Platform);
    [
        Level::Platform,
        Level::Organization,
        Level::Project,
        Level::Application,
    ]
    .into_iter()
    .filter(|l| *l <= narrowest)
    .collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RoleError {
    NameInvalid,
    NameTaken,
    NoPermissions,
    TooManyPermissions,
    UnknownPermission(String),
}

impl std::fmt::Display for RoleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RoleError::NameInvalid => {
                f.write_str("a role name is 1 to 60 characters: letters, digits, spaces, - and _")
            }
            RoleError::NameTaken => f.write_str("a role with that name already exists"),
            RoleError::NoPermissions => f.write_str("a role needs at least one permission"),
            RoleError::TooManyPermissions => f.write_str("a role has at most 100 permissions"),
            RoleError::UnknownPermission(p) => write!(f, "unknown permission {p:?}"),
        }
    }
}

/// Checks a custom role definition. Names may not shadow a predefined role.
pub fn validate_custom(name: &str, perms: &BTreeSet<String>) -> Result<(), RoleError> {
    let n = name.trim();
    if n.is_empty()
        || n.chars().count() > 60
        || !n
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, ' ' | '-' | '_'))
    {
        return Err(RoleError::NameInvalid);
    }
    let lower = n.to_lowercase();
    if Builtin::ALL
        .iter()
        .any(|b| b.name().to_lowercase() == lower || b.key() == lower)
    {
        return Err(RoleError::NameTaken);
    }
    if perms.is_empty() {
        return Err(RoleError::NoPermissions);
    }
    if perms.len() > 100 {
        return Err(RoleError::TooManyPermissions);
    }
    if let Some(bad) = perms.iter().find(|p| permission(p).is_none()) {
        return Err(RoleError::UnknownPermission(bad.clone()));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_predefined_role_uses_only_catalogue_permissions_and_valid_scopes() {
        for b in Builtin::ALL {
            let perms = b.permissions();
            assert!(!perms.is_empty(), "{b:?}");
            for p in &perms {
                assert!(permission(p).is_some(), "{b:?} has unknown {p}");
            }
            assert!(!b.scopes().is_empty());
            assert_eq!(Builtin::parse(b.key()), Some(b));
        }
    }

    #[test]
    fn roles_assignable_below_the_platform_hold_no_infrastructure_or_security_permission() {
        for b in Builtin::ALL {
            let below = b
                .scopes()
                .iter()
                .any(|l| matches!(l, Level::Organization | Level::Project | Level::Application));
            let only_view = |id: &str| matches!(id, "cluster.view" | "hardware.view");
            for p in b.permissions() {
                let d = permission(p).expect("listed");
                if below && d.class != Class::Tenant {
                    // Read-only infrastructure views are inert below the platform (the evaluator
                    // ignores them); anything else would be a design error.
                    assert!(
                        only_view(p),
                        "{b:?} may be assigned below the platform but holds {p}"
                    );
                }
            }
        }
        for b in [
            Builtin::OrganizationOwner,
            Builtin::ProjectAdministrator,
            Builtin::ApplicationDesigner,
            Builtin::ApplicationOperator,
        ] {
            assert!(
                b.permissions()
                    .iter()
                    .all(|p| permission(p).is_some_and(|d| d.class == Class::Tenant)),
                "{b:?}"
            );
        }
    }

    #[test]
    fn super_admin_holds_everything_and_hardware_operator_has_no_cluster_or_application_access() {
        assert_eq!(
            Builtin::PlatformSuperAdmin.permissions().len(),
            PERMISSIONS.len()
        );
        let hw = Builtin::HardwareOperator.permissions();
        assert!(
            hw.iter()
                .all(|p| p.starts_with("hardware.") || *p == "approval.request")
        );
    }

    #[test]
    fn the_designer_cannot_deploy_and_the_operator_cannot_design() {
        assert!(
            !Builtin::ApplicationDesigner
                .permissions()
                .contains("application.deploy")
        );
        assert!(
            !Builtin::ApplicationOperator
                .permissions()
                .contains("application.design")
        );
        assert!(
            !Builtin::Auditor
                .permissions()
                .iter()
                .any(|p| p.ends_with(".manage") || p.contains("deploy") || p.contains("design"))
        );
    }

    #[test]
    fn custom_role_scopes_follow_their_content() {
        let set = |ids: &[&str]| ids.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        assert_eq!(
            custom_scopes(&set(&["application.view"])),
            vec![
                Level::Platform,
                Level::Organization,
                Level::Project,
                Level::Application
            ]
        );
        assert_eq!(
            custom_scopes(&set(&["project.update"])),
            vec![Level::Platform, Level::Organization, Level::Project]
        );
        assert_eq!(
            custom_scopes(&set(&["hardware.view", "hardware.power"])),
            vec![Level::Platform, Level::Pool]
        );
        assert_eq!(
            custom_scopes(&set(&["cluster.manage"])),
            vec![Level::Platform]
        );
        assert_eq!(
            custom_scopes(&set(&["token.manage", "application.view"])),
            vec![Level::Platform]
        );
        assert_eq!(
            custom_scopes(&set(&["cluster.manage", "application.view"])),
            vec![Level::Platform]
        );
    }

    #[test]
    fn custom_role_validation() {
        let one = |s: &str| [s.to_string()].into_iter().collect::<BTreeSet<_>>();
        assert!(validate_custom("Release manager", &one("application.deploy")).is_ok());
        assert_eq!(
            validate_custom("", &one("application.deploy")),
            Err(RoleError::NameInvalid)
        );
        assert_eq!(
            validate_custom("Viewer", &one("application.view")),
            Err(RoleError::NameTaken)
        );
        assert_eq!(
            validate_custom("auditor", &one("application.view")),
            Err(RoleError::NameTaken)
        );
        assert_eq!(
            validate_custom("x", &BTreeSet::new()),
            Err(RoleError::NoPermissions)
        );
        assert_eq!(
            validate_custom("x", &one("root.everything")),
            Err(RoleError::UnknownPermission("root.everything".into()))
        );
    }
}
