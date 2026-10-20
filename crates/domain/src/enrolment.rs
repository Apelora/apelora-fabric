// SPDX-License-Identifier: AGPL-3.0-only
//! Node enrolment (Phase 09, ADR 0002, `docs/design/node-enrolment.md`): how a machine becomes
//! an Incus member Apelora places appliances on — the same way whichever provisioner installed
//! it. Pure: the caller observes Incus and the machine record; this decides.
//!
//! A new member is *held* (`scheduler.instance=manual`: Incus places only instances targeted at
//! it) until an administrator admits it after the admission checks pass.

use serde::{Deserialize, Serialize};

use crate::preflight::{Check, CheckStatus, MIN_INCUS_MAJOR, MIN_KERNEL, check, major_minor};
use crate::{ClusterMember, Network, ServerInfo, StoragePool};

/// `scheduler.instance` value that keeps a member out of automatic placement.
pub const HELD: &str = "manual";

/// What installs and powers a machine. Every provisioner enrols the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ProvisionerKind {
    /// The operating system is installed by hand; the administrator runs the enrolment script.
    Manual,
    /// Machines discovered, installed and powered through MAAS (not available yet).
    Maas,
}

impl ProvisionerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ProvisionerKind::Manual => "manual",
            ProvisionerKind::Maas => "maas",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "manual" => Some(ProvisionerKind::Manual),
            "maas" => Some(ProvisionerKind::Maas),
            _ => None,
        }
    }

    pub fn capabilities(self) -> Capabilities {
        match self {
            ProvisionerKind::Manual => Capabilities {
                discovers: false,
                installs: false,
                powers: false,
            },
            ProvisionerKind::Maas => Capabilities {
                discovers: true,
                installs: true,
                powers: true,
            },
        }
    }
}

/// What a provisioner can do to a machine without someone at the server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Capabilities {
    pub discovers: bool,
    pub installs: bool,
    pub powers: bool,
}

/// Where a machine is in enrolment. Derived from the record and from Incus, never stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum MachineState {
    /// Known to Apelora, not approved.
    Registered,
    /// A join token is waiting for the host.
    Approved,
    /// The approval's token was used up, revoked or expired without the host joining.
    ApprovalExpired,
    /// A cluster member Incus does not place instances on automatically.
    Held,
    /// A cluster member Incus places instances on.
    Eligible,
    /// Was admitted, is no longer a cluster member.
    Left,
}

/// What is known about one machine.
pub struct Observation<'a> {
    pub approved: bool,
    pub admitted: bool,
    /// The cluster member with the machine's name, if there is one.
    pub member: Option<&'a ClusterMember>,
    /// Incus lists an unused join token for the machine's name.
    pub token_pending: bool,
}

pub fn machine_state(o: &Observation<'_>) -> MachineState {
    match o.member {
        Some(m) if m.scheduler == HELD => MachineState::Held,
        Some(_) => MachineState::Eligible,
        None if o.token_pending => MachineState::Approved,
        None if o.admitted => MachineState::Left,
        None if o.approved => MachineState::ApprovalExpired,
        None => MachineState::Registered,
    }
}

/// What the administrator does next, in a sentence.
pub fn next_step(state: MachineState, provider: ProvisionerKind) -> &'static str {
    match (state, provider) {
        (MachineState::Registered | MachineState::ApprovalExpired, ProvisionerKind::Manual) => {
            "Install Incus on the server, then approve it."
        }
        (MachineState::Registered | MachineState::ApprovalExpired, ProvisionerKind::Maas) => {
            "Approve it to install and enrol it."
        }
        (MachineState::Approved, ProvisionerKind::Manual) => {
            "Run the enrolment script on the server and paste the join token."
        }
        (MachineState::Approved, ProvisionerKind::Maas) => "Waiting for the server to join.",
        (MachineState::Held, _) => "Check and admit it.",
        (MachineState::Eligible, _) => "None.",
        (MachineState::Left, _) => "Approve it again, or remove it.",
    }
}

/// Things the administrator should know about a machine.
pub fn warnings(o: &Observation<'_>) -> Vec<String> {
    let mut out = Vec::new();
    if machine_state(o) == MachineState::Eligible && !o.admitted {
        out.push(
            "it joined without being held from placement: Incus may already place instances on \
             it although it was not admitted; run the admission checks"
                .into(),
        );
    }
    if let Some(m) = o.member
        && m.status != "Online"
    {
        out.push(format!("the member is {}: {}", m.status, m.message));
    }
    out
}

/// What admission looks at.
pub struct AdmissionInput<'a> {
    pub member: &'a ClusterMember,
    /// `GET /1.0?target=<member>`: the member's own server facts.
    pub server: &'a ServerInfo,
    /// Version of the server the control plane talks to.
    pub cluster_version: Option<&'a str>,
    /// The other members.
    pub others: &'a [ClusterMember],
    pub pools: &'a [StoragePool],
    pub networks: &'a [Network],
}

/// Checks a joined member before it is admitted. A `fail` blocks admission.
pub fn admission_checks(i: &AdmissionInput<'_>) -> Vec<Check> {
    use CheckStatus::*;
    let name = i.member.name.as_str();
    let mut out = Vec::new();

    out.push(if i.member.status == "Online" {
        check("member-online", "Member online", Pass, "online")
    } else {
        check(
            "member-online",
            "Member online",
            Fail,
            format!("{}: {}", i.member.status, i.member.message),
        )
    });

    let version = i.server.server_version.as_deref();
    out.push(match version.map(|v| (v, major_minor(v))) {
        None | Some((_, None)) => check(
            "incus-version",
            "Incus version",
            NotChecked,
            "the member did not report its version",
        ),
        Some((v, Some((major, _)))) if major < MIN_INCUS_MAJOR => check(
            "incus-version",
            "Incus version",
            Fail,
            format!("{v}; Apelora needs {MIN_INCUS_MAJOR}.0 or later"),
        ),
        Some((v, _)) if i.cluster_version.is_some_and(|c| c != v) => check(
            "incus-version",
            "Incus version",
            Warn,
            format!(
                "{v}, while the cluster runs {}; upgrade members to the same release",
                i.cluster_version.unwrap_or_default()
            ),
        ),
        Some((v, _)) => check("incus-version", "Incus version", Pass, v),
    });

    out.push(
        match i
            .server
            .kernel_version
            .as_deref()
            .map(|v| (v, major_minor(v)))
        {
            None | Some((_, None)) => check(
                "kernel-version",
                "Kernel version",
                NotChecked,
                "the member did not report its kernel",
            ),
            Some((v, Some(k))) if k < MIN_KERNEL => check(
                "kernel-version",
                "Kernel version",
                Fail,
                format!(
                    "{v}; Apelora needs {}.{} or later",
                    MIN_KERNEL.0, MIN_KERNEL.1
                ),
            ),
            Some((v, _)) => check("kernel-version", "Kernel version", Pass, v),
        },
    );

    let known: Vec<&str> = i
        .others
        .iter()
        .filter(|m| m.name != name)
        .map(|m| m.architecture.as_str())
        .collect();
    let arch = i.member.architecture.as_str();
    out.push(if known.is_empty() || known.contains(&arch) {
        check("architecture", "Architecture", Pass, arch)
    } else {
        check(
            "architecture",
            "Architecture",
            Warn,
            format!(
                "{arch} is new to this cluster: only images built for it can run there, and \
                 instances cannot move between it and the other members"
            ),
        )
    });

    let missing_pools: Vec<&str> = i
        .pools
        .iter()
        .filter(|p| !p.locations.is_empty() && !p.locations.iter().any(|l| l == name))
        .map(|p| p.name.as_str())
        .collect();
    out.push(if missing_pools.is_empty() {
        check(
            "storage-pools",
            "Storage pools",
            Pass,
            format!("{} created on the member", i.pools.len()),
        )
    } else {
        check(
            "storage-pools",
            "Storage pools",
            Fail,
            format!("not created on the member: {}", missing_pools.join(", ")),
        )
    });

    // Bridges and physical networks exist per member; OVN networks are cluster-wide and list
    // only the members that existed when they were created, so they are not checked here.
    let per_member = |n: &&Network| n.managed && n.kind != "ovn" && !n.locations.is_empty();
    let missing_networks: Vec<String> = i
        .networks
        .iter()
        .filter(per_member)
        .filter(|n| !n.locations.iter().any(|l| l == name))
        .map(|n| format!("{} ({})", n.name, n.project))
        .collect();
    out.push(if missing_networks.is_empty() {
        check(
            "networks",
            "Networks",
            Pass,
            format!(
                "{} created on the member",
                i.networks.iter().filter(per_member).count()
            ),
        )
    } else {
        check(
            "networks",
            "Networks",
            Fail,
            format!("not created on the member: {}", missing_networks.join(", ")),
        )
    });

    if i.networks.iter().any(|n| n.kind == "ovn") {
        out.push(check(
            "ovn-chassis",
            "OVN on the member",
            NotChecked,
            "Incus does not report OVN health; the enrolment script sets OVN up",
        ));
    }
    out.push(check(
        "clock",
        "Clock",
        NotChecked,
        "Incus cannot measure it; keep the member's clock synchronised",
    ));
    out
}

/// Admission is allowed when no check failed.
pub fn admissible(checks: &[Check]) -> bool {
    checks.iter().all(|c| c.status != CheckStatus::Fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, scheduler: &str, arch: &str) -> ClusterMember {
        ClusterMember {
            name: name.into(),
            url: format!("https://{name}:8443"),
            roles: vec![],
            status: "Online".into(),
            message: "Fully operational".into(),
            architecture: arch.into(),
            failure_domain: "default".into(),
            database: false,
            groups: vec!["default".into()],
            scheduler: scheduler.into(),
        }
    }

    fn obs<'a>(
        approved: bool,
        admitted: bool,
        member: Option<&'a ClusterMember>,
        token_pending: bool,
    ) -> Observation<'a> {
        Observation {
            approved,
            admitted,
            member,
            token_pending,
        }
    }

    #[test]
    fn the_state_follows_incus() {
        let held = member("m4", HELD, "aarch64");
        let all = member("m4", "all", "aarch64");
        assert_eq!(
            machine_state(&obs(false, false, None, false)),
            MachineState::Registered
        );
        assert_eq!(
            machine_state(&obs(true, false, None, true)),
            MachineState::Approved
        );
        assert_eq!(
            machine_state(&obs(true, false, None, false)),
            MachineState::ApprovalExpired
        );
        assert_eq!(
            machine_state(&obs(true, false, Some(&held), false)),
            MachineState::Held
        );
        assert_eq!(
            machine_state(&obs(true, true, Some(&all), false)),
            MachineState::Eligible
        );
        assert_eq!(
            machine_state(&obs(true, true, None, false)),
            MachineState::Left
        );
        // Approved again after leaving: the pending token wins.
        assert_eq!(
            machine_state(&obs(true, true, None, true)),
            MachineState::Approved
        );
    }

    #[test]
    fn a_member_that_skipped_the_hold_is_reported() {
        let all = member("m4", "all", "aarch64");
        let w = warnings(&obs(true, false, Some(&all), false));
        assert!(w[0].contains("without being held"), "{w:?}");
        assert!(warnings(&obs(true, true, Some(&all), false)).is_empty());
    }

    #[test]
    fn the_manual_provider_cannot_act_on_the_server() {
        let c = ProvisionerKind::Manual.capabilities();
        assert!(!c.discovers && !c.installs && !c.powers);
        assert_eq!(
            ProvisionerKind::parse("manual"),
            Some(ProvisionerKind::Manual)
        );
        assert_eq!(ProvisionerKind::parse("pxe"), None);
    }

    fn server(version: &str, kernel: &str) -> ServerInfo {
        ServerInfo {
            api_version: "1.0".into(),
            server_version: Some(version.into()),
            kernel_version: Some(kernel.into()),
            api_extensions: vec![],
            architectures: vec!["aarch64".into()],
            clustered: true,
            auth: "trusted".into(),
        }
    }

    fn pool(locations: &[&str]) -> StoragePool {
        StoragePool {
            name: "local".into(),
            driver: "btrfs".into(),
            status: "Created".into(),
            description: String::new(),
            locations: locations.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn net(name: &str, kind: &str, locations: &[&str]) -> Network {
        Network {
            name: name.into(),
            project: "default".into(),
            kind: kind.into(),
            managed: true,
            status: "Created".into(),
            description: String::new(),
            locations: locations.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn status_of(checks: &[Check], id: &str) -> CheckStatus {
        checks.iter().find(|c| c.id == id).expect(id).status
    }

    #[test]
    fn a_well_joined_member_is_admissible() {
        let m = member("m4", HELD, "aarch64");
        let others = [member("m1", "all", "aarch64"), m.clone()];
        let checks = admission_checks(&AdmissionInput {
            member: &m,
            server: &server("7.0.1", "6.12.111+deb13-cloud-arm64"),
            cluster_version: Some("7.0.1"),
            others: &others,
            pools: &[pool(&["m1", "m4"])],
            // An OVN network created before m4 joined does not list it, and must not block.
            networks: &[
                net("incusbr0", "bridge", &["m1", "m4"]),
                net("app", "ovn", &["m1"]),
            ],
        });
        assert!(admissible(&checks), "{checks:#?}");
        assert_eq!(status_of(&checks, "ovn-chassis"), CheckStatus::NotChecked);
        assert_eq!(status_of(&checks, "architecture"), CheckStatus::Pass);
    }

    #[test]
    fn missing_storage_an_old_kernel_or_an_offline_member_block_admission() {
        let mut m = member("m4", HELD, "x86_64");
        m.status = "Offline".into();
        let others = [member("m1", "all", "aarch64")];
        let checks = admission_checks(&AdmissionInput {
            member: &m,
            server: &server("7.0.2", "6.1.0-28-amd64"),
            cluster_version: Some("7.0.1"),
            others: &others,
            pools: &[pool(&["m1"])],
            networks: &[net("incusbr0", "bridge", &["m1"])],
        });
        assert!(!admissible(&checks));
        for (id, want) in [
            ("member-online", CheckStatus::Fail),
            ("kernel-version", CheckStatus::Fail),
            ("storage-pools", CheckStatus::Fail),
            ("networks", CheckStatus::Fail),
            ("incus-version", CheckStatus::Warn),
            ("architecture", CheckStatus::Warn),
        ] {
            assert_eq!(status_of(&checks, id), want, "{id}");
        }
    }
}
