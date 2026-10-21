// SPDX-License-Identifier: AGPL-3.0-only
//! How Apelora's scopes line up with Incus projects and MAAS resource pools.
//!
//! Nothing here talks to a server. These are the *intended* mappings, written down as code so
//! they can be tested and reviewed. **They have not been validated against an installed Incus
//! or MAAS** (see the architecture document for what remains to be verified).
//!
//! What an environment's Incus project contains (restrictions, limits, its default profile)
//! is decided in `apelora_domain::tenancy` (ADR 0012); only its name is derived here.

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
    (name.len() <= 63 && name.chars().next().is_some_and(|c| c.is_ascii_alphabetic()))
        .then_some(name)
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
            MaasError::UnsupportedVersion(v) => write!(
                f,
                "MAAS {v}: group-based resource-pool entitlements are only mapped for 3.8 and later; earlier releases use a different RBAC model"
            ),
            MaasError::NotRepresentable(p) => write!(
                f,
                "{p} has no MAAS entitlement; Apelora authorises it and the provisioning identity performs it"
            ),
        }
    }
}

fn at_least_3_8(v: &str) -> bool {
    let mut it = v
        .trim_start_matches('v')
        .split(|c: char| !c.is_ascii_digit())
        .filter(|s| !s.is_empty())
        .map(|s| s.parse::<u32>().unwrap_or(0));
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
        assert_eq!(
            incus_project_name("Acme", "Production", "prod").as_deref(),
            Some("acme-production-prod")
        );
        assert_eq!(
            incus_project_name("acme corp", "web_shop", "dev").as_deref(),
            Some("acme-corp-web-shop-dev")
        );
        assert_eq!(incus_project_name("", "p", "e"), None);
        assert_eq!(
            incus_project_name("1acme", "p", "e"),
            None,
            "must start with a letter"
        );
        assert_eq!(
            incus_project_name(&"a".repeat(41), &"b".repeat(20), "e"),
            None,
            "63 characters at most"
        );
    }

    #[test]
    fn maas_entitlements_are_version_gated_and_never_invented() {
        assert_eq!(
            maas_entitlement("3.8.0", "hardware.view"),
            Ok("can_view_machines")
        );
        assert_eq!(
            maas_entitlement("v3.8", "hardware.deploy"),
            Ok("can_deploy_machines")
        );
        assert_eq!(
            maas_entitlement("4.0.1", "hardware.view"),
            Ok("can_view_machines")
        );
        assert!(matches!(
            maas_entitlement("3.5.2", "hardware.view"),
            Err(MaasError::UnsupportedVersion(_))
        ));
        assert!(matches!(
            maas_entitlement("", "hardware.view"),
            Err(MaasError::UnsupportedVersion(_))
        ));
        assert!(matches!(
            maas_entitlement("3.8.0", "hardware.delete"),
            Err(MaasError::NotRepresentable(_))
        ));
        assert!(matches!(
            maas_entitlement("3.8.0", "hardware.power"),
            Err(MaasError::NotRepresentable(_))
        ));
    }
}
