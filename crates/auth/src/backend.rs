// SPDX-License-Identifier: AGPL-3.0-only
//! How Apelora's scopes line up with Incus projects and MAAS resource pools.
//!
//! Nothing here talks to a server. These are the *intended* mappings, written down as code so
//! they can be tested and reviewed. **They have not been validated against an installed Incus
//! or MAAS** (see the architecture document for what remains to be verified).

use std::collections::BTreeMap;

use crate::scope::EnvKind;

/// Name of the Incus project that holds one environment: `<organisation>-<project>-<environment>`
/// in lowercase, restricted to what Incus accepts in a project name (letters, digits, `-`), at
/// most 63 characters. Returns `None` when a part has no usable characters.
pub fn incus_project_name(org: &str, project: &str, environment: &str) -> Option<String> {
    let clean = |s: &str| -> String {
        let mut out = String::new();
        for c in s.to_lowercase().chars() {
            if c.is_ascii_alphanumeric() {
                out.push(c);
            } else if !out.ends_with('-') && !out.is_empty() {
                out.push('-');
            }
        }
        out.trim_matches('-').to_owned()
    };
    let parts = [clean(org), clean(project), clean(environment)];
    if parts.iter().any(String::is_empty) {
        return None;
    }
    let name = parts.join("-");
    (name.len() <= 63 && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic())).then_some(name)
}

/// Quotas for one environment's Incus project.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Limits {
    pub instances: Option<u32>,
    pub cpu: Option<u32>,
    pub memory_gib: Option<u32>,
    pub disk_gib: Option<u32>,
}

/// Configuration keys for a restricted Incus project: tenants may not nest, run privileged
/// containers, choose cluster members, or attach host devices and paths; their networks and
/// storage devices must be the managed ones. Production gets the same restrictions as every
/// other environment (isolation does not depend on the environment's kind).
pub fn incus_project_config(_kind: EnvKind, limits: &Limits) -> BTreeMap<String, String> {
    let mut c: BTreeMap<String, String> = [
        ("restricted", "true"),
        ("restricted.containers.nesting", "block"),
        ("restricted.containers.privilege", "unprivileged"),
        ("restricted.containers.lowlevel", "block"),
        ("restricted.virtual-machines.lowlevel", "block"),
        ("restricted.cluster.target", "block"),
        ("restricted.devices.disk", "managed"),
        ("restricted.devices.nic", "managed"),
        ("restricted.devices.gpu", "block"),
        ("restricted.devices.pci", "block"),
        ("restricted.devices.usb", "block"),
        ("restricted.devices.unix-char", "block"),
        ("restricted.devices.unix-block", "block"),
        ("restricted.devices.unix-hotplug", "block"),
        ("restricted.devices.infiniband", "block"),
        ("restricted.devices.proxy", "block"),
        ("features.images", "true"),
        ("features.profiles", "true"),
        ("features.storage.volumes", "true"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    if let Some(n) = limits.instances {
        c.insert("limits.instances".into(), n.to_string());
    }
    if let Some(n) = limits.cpu {
        c.insert("limits.cpu".into(), n.to_string());
    }
    if let Some(n) = limits.memory_gib {
        c.insert("limits.memory".into(), format!("{n}GiB"));
    }
    if let Some(n) = limits.disk_gib {
        c.insert("limits.disk".into(), format!("{n}GiB"));
    }
    c
}

/// Evidence level of the mappings in this module.
pub const MAPPING_EVIDENCE: &str = "NOT YET VERIFIED against Incus 7.0.1 or MAAS";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MaasError {
    /// The version is older than the entitlement model the mapping targets, or unknown.
    UnsupportedVersion(String),
    /// Apelora checks this permission itself; MAAS has no entitlement that would authorise it.
    NotRepresentable(String),
}

impl std::fmt::Display for MaasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MaasError::UnsupportedVersion(v) => write!(f, "MAAS {v}: group-based resource-pool entitlements are only mapped for 3.8 and later; earlier releases use a different RBAC model"),
            MaasError::NotRepresentable(p) => write!(f, "{p} has no MAAS entitlement; Apelora authorises it and the provisioning identity performs it"),
        }
    }
}

fn at_least_3_8(v: &str) -> bool {
    let mut it = v.trim_start_matches('v').split(|c: char| !c.is_ascii_digit()).filter(|s| !s.is_empty()).map(|s| s.parse::<u32>().unwrap_or(0));
    let (major, minor) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
    major > 3 || (major == 3 && minor >= 8)
}

/// The MAAS resource-pool entitlement that corresponds to an Apelora hardware permission, for
/// MAAS 3.8 or later. Only entitlements named in MAAS's documentation for that release are
/// mapped; everything else stays an Apelora decision.
pub fn maas_entitlement(maas_version: &str, permission: &str) -> Result<&'static str, MaasError> {
    if !at_least_3_8(maas_version) {
        return Err(MaasError::UnsupportedVersion(maas_version.to_owned()));
    }
    match permission {
        "hardware.view" => Ok("can_view_machines"),
        "hardware.deploy" => Ok("can_deploy_machines"),
        other => Err(MaasError::NotRepresentable(other.to_owned())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incus_project_names_are_derived_and_validated() {
        assert_eq!(incus_project_name("Acme", "Production", "prod").as_deref(), Some("acme-production-prod"));
        assert_eq!(incus_project_name("acme corp", "web_shop", "dev").as_deref(), Some("acme-corp-web-shop-dev"));
        assert_eq!(incus_project_name("", "p", "e"), None);
        assert_eq!(incus_project_name("1acme", "p", "e"), None, "must start with a letter");
        assert_eq!(incus_project_name(&"a".repeat(41), &"b".repeat(20), "e"), None, "63 characters at most");
    }

    #[test]
    fn restricted_project_blocks_privilege_host_devices_and_member_choice() {
        let c = incus_project_config(EnvKind::Production, &Limits { instances: Some(20), cpu: Some(16), memory_gib: Some(64), disk_gib: Some(500) });
        assert_eq!(c["restricted"], "true");
        assert_eq!(c["restricted.containers.privilege"], "unprivileged");
        assert_eq!(c["restricted.containers.nesting"], "block");
        assert_eq!(c["restricted.cluster.target"], "block");
        assert_eq!(c["restricted.devices.disk"], "managed");
        assert_eq!(c["limits.memory"], "64GiB");
        assert_eq!(c["limits.instances"], "20");
        for (k, v) in &c {
            if k.starts_with("restricted.devices.") && k != "restricted.devices.disk" && k != "restricted.devices.nic" {
                assert_eq!(v, "block", "{k}");
            }
        }
        assert!(!incus_project_config(EnvKind::Development, &Limits::default()).contains_key("limits.cpu"));
    }

    #[test]
    fn maas_entitlements_are_version_gated_and_never_invented() {
        assert_eq!(maas_entitlement("3.8.0", "hardware.view"), Ok("can_view_machines"));
        assert_eq!(maas_entitlement("v3.8", "hardware.deploy"), Ok("can_deploy_machines"));
        assert_eq!(maas_entitlement("4.0.1", "hardware.view"), Ok("can_view_machines"));
        assert!(matches!(maas_entitlement("3.5.2", "hardware.view"), Err(MaasError::UnsupportedVersion(_))));
        assert!(matches!(maas_entitlement("", "hardware.view"), Err(MaasError::UnsupportedVersion(_))));
        assert!(matches!(maas_entitlement("3.8.0", "hardware.delete"), Err(MaasError::NotRepresentable(_))));
        assert!(matches!(maas_entitlement("3.8.0", "hardware.power"), Err(MaasError::NotRepresentable(_))));
    }
}
