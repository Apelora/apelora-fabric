// SPDX-License-Identifier: AGPL-3.0-only
//! Moving one instance to another member (docs/design/mobility-and-ha.md, slice A).
//!
//! A running VM with live migration on moves while running, unless volumes on its member's
//! own pool would have to follow (they cannot, live). Anything else moves stopped: its volumes
//! are copied to the new member first, and it is started there if it was running. A stop is
//! only planned when the person allowed it.

use serde::{Deserialize, Serialize};

use crate::{ClusterMember, InstanceKind};

/// What the move does, decided before anything changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct MovePlan {
    pub instance: String,
    pub from: String,
    pub to: String,
    /// Moves while running.
    pub live: bool,
    /// Stops for the move (and starts again on the new member if it was running).
    pub stops: bool,
    /// Volumes copied to the new member, then deleted on the old one once it runs there.
    pub volumes: Vec<String>,
    /// How much data is copied while it is stopped, in MiB.
    pub copy_mib: u64,
    /// Why the move is refused; empty when it may proceed.
    pub blocked: Vec<String>,
    /// What to expect, in sentences.
    pub notes: Vec<String>,
}

/// The instance being moved, as Incus reports it.
pub struct MoveSubject<'a> {
    pub name: &'a str,
    pub kind: InstanceKind,
    pub status: &'a str,
    pub location: &'a str,
    pub architecture: &'a str,
    /// `migration.stateful=true`.
    pub stateful: bool,
    pub managed: bool,
    /// Its volumes on member-local pools: name and size in MiB.
    pub volumes: &'a [(String, u32)],
}

pub fn plan_move(
    s: &MoveSubject<'_>,
    to: &str,
    members: &[ClusterMember],
    allow_stop: bool,
) -> MovePlan {
    let running = s.status == "Running";
    let mut blocked = Vec::new();
    let mut notes = Vec::new();
    if !s.managed {
        blocked.push(format!("{} was not created by Apelora", s.name));
    }
    match members.iter().find(|m| m.name == to) {
        None => blocked.push(format!("{to} is not a cluster member")),
        Some(m) if m.name == s.location => blocked.push(format!("{} already runs on {to}", s.name)),
        Some(m) if m.status != "Online" => {
            blocked.push(format!("{to} is {}, not Online", m.status))
        }
        Some(m) if m.architecture != s.architecture => blocked.push(format!(
            "{to} runs {}, the instance needs {}",
            m.architecture, s.architecture
        )),
        Some(_) => {}
    }
    let live =
        running && s.kind == InstanceKind::VirtualMachine && s.stateful && s.volumes.is_empty();
    let stops = running && !live;
    if stops {
        let why = if s.kind != InstanceKind::VirtualMachine {
            "a container cannot move while running"
        } else if !s.stateful {
            "live migration is off for this module"
        } else {
            "volumes on its member's own pool cannot follow a running VM"
        };
        if allow_stop {
            notes.push(format!(
                "it stops for the move ({why}) and starts again on {to}"
            ));
        } else {
            blocked.push(format!(
                "it would have to stop: {why}; allow a short stop to move it"
            ));
        }
    }
    if live {
        notes.push("it keeps running: memory and disk are copied while it runs, then it pauses for under a second".into());
    }
    let copy_mib: u64 = s.volumes.iter().map(|(_, mib)| u64::from(*mib)).sum();
    if !s.volumes.is_empty() {
        notes.push(format!(
            "{} volume{} ({} MiB) copied to {to}; the old copies are deleted once it runs there",
            s.volumes.len(),
            if s.volumes.len() == 1 { "" } else { "s" },
            copy_mib
        ));
    }
    MovePlan {
        instance: s.name.to_owned(),
        from: s.location.to_owned(),
        to: to.to_owned(),
        live,
        stops: stops && allow_stop,
        volumes: s.volumes.iter().map(|(n, _)| n.clone()).collect(),
        copy_mib,
        blocked,
        notes,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, status: &str, arch: &str) -> ClusterMember {
        ClusterMember {
            name: name.into(),
            url: String::new(),
            roles: vec![],
            status: status.into(),
            message: String::new(),
            architecture: arch.into(),
            failure_domain: "default".into(),
            database: false,
            groups: vec![],
            scheduler: "all".into(),
        }
    }

    fn members() -> Vec<ClusterMember> {
        vec![
            member("m1", "Online", "x86_64"),
            member("m2", "Online", "x86_64"),
            member("m3", "Offline", "x86_64"),
            member("m4", "Online", "aarch64"),
        ]
    }

    fn vm<'a>(stateful: bool, volumes: &'a [(String, u32)]) -> MoveSubject<'a> {
        MoveSubject {
            name: "shop-vm-01",
            kind: InstanceKind::VirtualMachine,
            status: "Running",
            location: "m1",
            architecture: "x86_64",
            stateful,
            managed: true,
            volumes,
        }
    }

    #[test]
    fn a_running_vm_with_live_migration_moves_running() {
        let p = plan_move(&vm(true, &[]), "m2", &members(), false);
        assert!(p.blocked.is_empty(), "{:?}", p.blocked);
        assert!(p.live && !p.stops);
    }

    #[test]
    fn otherwise_it_moves_only_with_a_stop_the_person_allowed() {
        let vols = [("shop-vm-01-data".to_owned(), 2048)];
        for subject in [vm(false, &[]), vm(true, &vols)] {
            let refused = plan_move(&subject, "m2", &members(), false);
            assert!(
                refused
                    .blocked
                    .iter()
                    .any(|b| b.contains("allow a short stop")),
                "{refused:?}"
            );
            let allowed = plan_move(&subject, "m2", &members(), true);
            assert!(allowed.blocked.is_empty() && allowed.stops && !allowed.live);
        }
        let with_volume = plan_move(&vm(true, &vols), "m2", &members(), true);
        assert_eq!((with_volume.volumes.len(), with_volume.copy_mib), (1, 2048));
        let container = MoveSubject {
            kind: InstanceKind::Container,
            stateful: false,
            ..vm(false, &[])
        };
        assert!(
            plan_move(&container, "m2", &members(), false)
                .blocked
                .iter()
                .any(|b| b.contains("container"))
        );
        let stopped = MoveSubject {
            status: "Stopped",
            ..vm(false, &[])
        };
        let p = plan_move(&stopped, "m2", &members(), false);
        assert!(p.blocked.is_empty() && !p.stops && !p.live);
    }

    #[test]
    fn the_target_must_be_another_online_member_of_the_same_architecture() {
        let m = members();
        for (to, why) in [
            ("m1", "already runs"),
            ("m3", "not Online"),
            ("m4", "aarch64"),
            ("m9", "not a cluster member"),
        ] {
            assert!(
                plan_move(&vm(true, &[]), to, &m, true)
                    .blocked
                    .iter()
                    .any(|b| b.contains(why)),
                "{to}"
            );
        }
        let foreign = MoveSubject {
            managed: false,
            ..vm(true, &[])
        };
        assert!(!plan_move(&foreign, "m2", &m, true).blocked.is_empty());
    }
}
