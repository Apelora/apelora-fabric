// SPDX-License-Identifier: AGPL-3.0-only
//! The permission catalogue. Every protected action in the API names exactly one permission
//! from this list; roles are sets of these ids. An id that is not listed here is never granted.

use serde::{Deserialize, Serialize};

use crate::scope::Level;

/// What kind of power a permission is. Only `Tenant` permissions can be granted below the
/// platform level, so an organisation or project role can never reach physical servers,
/// the Incus cluster or the platform's own security settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum Class {
    /// Applications, projects, organisations and their members.
    Tenant,
    /// The cluster, networks, storage and physical hardware.
    Infrastructure,
    /// Platform identity, credentials, policy and emergency access.
    Security,
}

#[derive(Debug, Clone, Copy)]
pub struct PermissionDef {
    pub id: &'static str,
    pub class: Class,
    /// The narrowest level at which an assignment confers this permission.
    pub floor: Level,
    /// Needs a recorded approval from a second person before it may be carried out.
    pub approval: bool,
    pub description: &'static str,
}

const fn t(id: &'static str, floor: Level, description: &'static str) -> PermissionDef {
    PermissionDef { id, class: Class::Tenant, floor, approval: false, description }
}
const fn i(id: &'static str, floor: Level, approval: bool, description: &'static str) -> PermissionDef {
    PermissionDef { id, class: Class::Infrastructure, floor, approval, description }
}
const fn s(id: &'static str, description: &'static str) -> PermissionDef {
    PermissionDef { id, class: Class::Security, floor: Level::Platform, approval: false, description }
}

use Level::{Application as App, Organization as Org, Platform as Plat, Pool, Project as Proj};

pub const PERMISSIONS: &[PermissionDef] = &[
    // --- organisations and projects
    t("organization.view", Org, "See an organisation and its projects"),
    t("organization.update", Org, "Rename an organisation and change its settings"),
    t("project.view", Proj, "See a project and its environments"),
    t("project.create", Org, "Create projects in an organisation"),
    t("project.update", Proj, "Change a project"),
    t("project.delete", Proj, "Delete an empty project"),
    t("environment.manage", Proj, "Create and change environments and their Incus project mapping requests"),
    // --- applications
    t("application.view", App, "See applications and their status"),
    t("application.create", Proj, "Create an application"),
    t("application.design", App, "Edit the application graph and preview a plan"),
    t("application.deploy", App, "Deploy a design to its environment"),
    t("application.operate", App, "Start, stop, restart and recover an application"),
    t("application.delete", App, "Remove an application and its instances"),
    t("application.logs.read", App, "Read application logs"),
    t("application.metrics.read", App, "Read application metrics"),
    t("instance.manage", Proj, "Create, change and delete single instances in a project's environments"),
    // --- people and access inside a scope
    t("user.view", Proj, "See users with access in the scope"),
    t("group.manage", Org, "Create groups and change their members"),
    t("role.assign", Proj, "Grant and revoke roles in the scope (never more than you hold)"),
    t("audit.read", App, "Read audit events of the scope"),
    t("approval.request", App, "Ask for approval of a protected action"),
    t("approval.grant", Proj, "Approve or reject another person's request"),
    // --- infrastructure (platform or hardware pool scope only)
    i("cluster.view", Plat, false, "See the Incus cluster, members, networks and storage pools"),
    i("cluster.manage", Plat, false, "Join tokens, enrolment holds and other cluster changes"),
    i("cluster.evacuate", Plat, false, "Evacuate a member when no workload would be left without service"),
    i("cluster.evacuate_unsafe", Plat, true, "Evacuate a member although some workload would lose service"),
    i("cluster.sensitive_change", Plat, true, "Change cluster authentication, trust or other sensitive settings"),
    i("network.manage", Plat, false, "Manage networks, uplinks and address ranges"),
    i("storage.manage", Plat, false, "Manage storage pools and volume classes"),
    i("hardware.view", Pool, false, "See physical machines"),
    i("hardware.commission", Pool, false, "Commission and test physical machines"),
    i("hardware.deploy", Pool, false, "Install an operating system on a physical machine"),
    i("hardware.power", Pool, true, "Power a physical machine on, off or restart it"),
    i("hardware.delete", Pool, true, "Remove a physical machine from inventory or wipe it"),
    i("machine.enrol", Plat, false, "Register machines and approve them to join the cluster"),
    // --- platform security (platform scope only)
    s("organization.create", "Create organisations"),
    s("role.manage", "Define custom roles"),
    s("service_identity.manage", "Register and disable service identities"),
    s("access_policy.manage", "Change access policies (MFA and approval rules)"),
    s("token.manage", "Create and revoke API tokens"),
    s("platform.security.manage", "Change identity provider settings and other platform security"),
    s("platform.emergency_access", "Grant time-limited emergency access"),
    s("user.manage", "Disable users and see every user"),
];

pub fn permission(id: &str) -> Option<&'static PermissionDef> {
    PERMISSIONS.iter().find(|p| p.id == id)
}

/// Permission ids by class, for building roles.
pub fn ids_where(f: impl Fn(&PermissionDef) -> bool) -> Vec<&'static str> {
    PERMISSIONS.iter().filter(|p| f(p)).map(|p| p.id).collect()
}

/// Is the action carried out only after another person has approved it?
pub fn needs_approval(id: &str) -> bool {
    permission(id).is_some_and(|p| p.approval)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_well_formed() {
        let mut seen = std::collections::HashSet::new();
        for p in PERMISSIONS {
            assert!(seen.insert(p.id), "duplicate {}", p.id);
            assert!(p.id.chars().all(|c| c.is_ascii_lowercase() || c == '.' || c == '_'), "{}", p.id);
            assert!(!p.description.is_empty());
        }
    }

    #[test]
    fn infrastructure_and_security_never_sit_below_platform_except_hardware_pools() {
        for p in PERMISSIONS {
            match p.class {
                Class::Tenant => {}
                Class::Security => assert_eq!(p.floor, Level::Platform, "{}", p.id),
                Class::Infrastructure => assert!(matches!(p.floor, Level::Platform | Level::Pool), "{}", p.id),
            }
        }
    }

    #[test]
    fn the_four_protected_actions_need_approval() {
        for id in ["hardware.delete", "hardware.power", "cluster.evacuate_unsafe", "cluster.sensitive_change"] {
            assert!(needs_approval(id), "{id}");
        }
        assert!(!needs_approval("application.deploy"));
        assert!(!needs_approval("no.such.permission"));
    }
}
