// SPDX-License-Identifier: AGPL-3.0-only
//! The rolling-upgrade assistant (phase 09). Apelora does not install Incus. It orders the
//! members (spares and stand-bys first, the database leader last) and remembers which step
//! each one is on: evacuate, the person upgrades the package, restore.
//!
//! Pure. The control plane stores the run and calls the same evacuate and restore Incus uses
//! from Servers → Members.

use serde::{Deserialize, Serialize};

use crate::clustering::{ClusterDatabaseRole, ClusterMemberDetail};

/// One member in an upgrade run, and how far it has gone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct UpgradeStep {
    pub member: String,
    /// `pending` (evacuate next), `evacuated` (the package upgrade is next), `upgraded`
    /// (restore next) or `restored`.
    pub phase: String,
}

/// What the Upgrades tab shows, whether or not a run has started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct UpgradeView {
    pub running: bool,
    /// Spares, then stand-bys, then voters, the database leader last.
    pub order: Vec<String>,
    /// Whose turn it is, while a run is going.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub member: Option<String>,
    /// `start`, `evacuate`, `upgraded`, `restore` or `none`.
    pub next: String,
    pub summary: String,
}

/// The order a rolling upgrade walks: nothing that holds the database first, the leader last.
pub fn order(members: &[ClusterMemberDetail]) -> Vec<String> {
    let mut ranked: Vec<(u8, &str)> = members
        .iter()
        .map(|m| {
            let rank = if m.leader {
                3
            } else {
                match m.database_role {
                    ClusterDatabaseRole::Spare => 0,
                    ClusterDatabaseRole::Standby => 1,
                    ClusterDatabaseRole::Voter => 2,
                }
            };
            (rank, m.name.as_str())
        })
        .collect();
    ranked.sort_by_key(|(rank, name)| (*rank, *name));
    ranked
        .into_iter()
        .map(|(_, name)| name.to_owned())
        .collect()
}

/// The view of a run, or of the order alone when none is running.
pub fn view(order: Vec<String>, steps: Option<&[UpgradeStep]>) -> UpgradeView {
    let Some(steps) = steps else {
        return UpgradeView {
            running: false,
            order,
            member: None,
            next: "start".into(),
            summary: "No upgrade is in progress. Starting one walks the members below, spares and stand-bys first and the database leader last. Apelora evacuates and restores; the package upgrade happens on the server.".into(),
        };
    };
    if let Some(step) = steps.iter().find(|s| s.phase != "restored") {
        let (next, summary) = match step.phase.as_str() {
            "evacuated" => (
                "upgraded",
                format!(
                    "{} is evacuated. Upgrade Incus on that server and restart it, then record that the package is upgraded.",
                    step.member
                ),
            ),
            "upgraded" => (
                "restore",
                format!(
                    "{} has been upgraded. Restore it so Incus moves its instances back.",
                    step.member
                ),
            ),
            _ => (
                "evacuate",
                format!(
                    "Next: evacuate {}. Read the dry run on Members; this does the same evacuation.",
                    step.member
                ),
            ),
        };
        return UpgradeView {
            running: true,
            order,
            member: Some(step.member.clone()),
            next: next.into(),
            summary,
        };
    }
    UpgradeView {
        running: false,
        order,
        member: None,
        next: "none".into(),
        summary: "The upgrade run is finished. Every member was restored.".into(),
    }
}

/// The phase a step moves to, or `None` when `action` is not the one this phase accepts.
pub fn advance(phase: &str, action: &str) -> Option<&'static str> {
    match (phase, action) {
        ("pending", "evacuate") => Some("evacuated"),
        ("evacuated", "upgraded") => Some("upgraded"),
        ("upgraded", "restore") => Some("restored"),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ClusterMember;
    use crate::clustering::{ClusterMemberDetail, member_detail};

    fn detail(name: &str, roles: &[&str], leader: bool) -> ClusterMemberDetail {
        let mut m = member_detail(
            &ClusterMember {
                name: name.into(),
                url: String::new(),
                roles: roles.iter().map(|r| (*r).to_owned()).collect(),
                status: "Online".into(),
                message: String::new(),
                architecture: "aarch64".into(),
                failure_domain: String::new(),
                database: roles.contains(&"database"),
                groups: vec![],
                scheduler: "all".into(),
            },
            Ok(Some("7.0.1".into())),
        );
        m.leader = leader;
        m
    }

    #[test]
    fn spares_and_standbys_come_before_voters_and_the_leader_is_last() {
        let members = vec![
            detail("n1", &["database", "database-leader"], true),
            detail("n2", &["database"], false),
            detail("n3", &["database-standby"], false),
            detail("n4", &[], false),
        ];
        assert_eq!(order(&members), vec!["n4", "n3", "n2", "n1"]);
    }

    #[test]
    fn a_run_walks_evacuate_then_the_package_then_restore() {
        let order = vec!["n4".into(), "n1".into()];
        let idle = view(order.clone(), None);
        assert_eq!(idle.next, "start");
        assert!(!idle.running);
        let steps = vec![
            UpgradeStep {
                member: "n4".into(),
                phase: "pending".into(),
            },
            UpgradeStep {
                member: "n1".into(),
                phase: "pending".into(),
            },
        ];
        let started = view(order, Some(&steps));
        assert_eq!(started.member.as_deref(), Some("n4"));
        assert_eq!(started.next, "evacuate");
        assert_eq!(advance("pending", "evacuate"), Some("evacuated"));
        assert_eq!(advance("evacuated", "upgraded"), Some("upgraded"));
        assert_eq!(advance("upgraded", "restore"), Some("restored"));
        assert_eq!(advance("pending", "restore"), None);
    }
}
