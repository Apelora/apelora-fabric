// SPDX-License-Identifier: AGPL-3.0-only
//! What the Console's Servers area says about the cluster: each member's database role, the
//! cluster database's quorum, version skew against the pinned Incus release, and the cluster
//! settings Apelora reads. Pure: the control plane reads Incus, these functions decide what
//! the readings mean, so the Console never infers a state on its own.
//!
//! Role names and member states are Incus's (`database`, `database-standby`,
//! `database-leader`, `database-client`, `event-hub`, `ovn-chassis`; Online, Evacuating,
//! Evacuated, Restoring, Offline, Blocked), checked against the pinned release and read from
//! the test cluster.

use std::collections::BTreeMap;

use serde::Serialize;

use crate::ClusterMember;
use crate::enrolment::HELD;

/// A member's part in the cluster database.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ClusterDatabaseRole {
    /// Votes in the cluster database (Incus role `database`).
    Voter,
    /// Keeps a copy without voting and is promoted when a voter is lost (`database-standby`).
    Standby,
    /// Holds no copy of the cluster database.
    Spare,
}

/// One member with its roles spelled out.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ClusterMemberDetail {
    pub name: String,
    /// Incus status: Online, Evacuating, Evacuated, Restoring, Offline or Blocked.
    pub status: String,
    pub message: String,
    /// The roles exactly as Incus reports them.
    pub roles: Vec<String>,
    pub database_role: ClusterDatabaseRole,
    /// Leads the cluster database now.
    pub leader: bool,
    /// Relays events between members (`event-hub`).
    pub event_hub: bool,
    /// Runs OVN's chassis role (`ovn-chassis`). Incus lists it only when it was assigned.
    pub ovn_chassis: bool,
    /// Kept out of the cluster database on purpose (`database-client`).
    pub database_excluded: bool,
    /// Counts towards the database majority: its daemon answers (not Offline or Blocked).
    pub up_for_database: bool,
    pub failure_domain: String,
    pub groups: Vec<String>,
    pub architecture: String,
    /// Kept out of automatic placement (`scheduler.instance=manual`).
    pub held: bool,
    /// The Incus release the member runs; `null` when it could not be read.
    #[cfg_attr(feature = "utoipa", schema(required = true))]
    pub incus_version: Option<String>,
    /// Why the version could not be read.
    #[cfg_attr(feature = "utoipa", schema(required = true))]
    pub version_error: Option<String>,
}

/// Statuses whose daemon takes part in the cluster database. Blocked members (waiting for an
/// upgrade) and offline ones are not counted, so the figure errs on the cautious side.
fn up_for_database(status: &str) -> bool {
    matches!(status, "Online" | "Evacuating" | "Evacuated" | "Restoring")
}

fn has(roles: &[String], role: &str) -> bool {
    roles.iter().any(|r| r == role)
}

/// A member's database role from its Incus roles.
pub fn database_role(roles: &[String]) -> ClusterDatabaseRole {
    if has(roles, "database") {
        ClusterDatabaseRole::Voter
    } else if has(roles, "database-standby") {
        ClusterDatabaseRole::Standby
    } else {
        ClusterDatabaseRole::Spare
    }
}

/// The member as the Servers area shows it; `version` is what `GET /1.0?target=` said.
pub fn member_detail(
    m: &ClusterMember,
    version: Result<Option<String>, String>,
) -> ClusterMemberDetail {
    let (incus_version, version_error) = match version {
        Ok(v) => (v, None),
        Err(e) => (None, Some(e)),
    };
    ClusterMemberDetail {
        name: m.name.clone(),
        status: m.status.clone(),
        message: m.message.clone(),
        roles: m.roles.clone(),
        database_role: database_role(&m.roles),
        leader: has(&m.roles, "database-leader"),
        event_hub: has(&m.roles, "event-hub"),
        ovn_chassis: has(&m.roles, "ovn-chassis"),
        database_excluded: has(&m.roles, "database-client"),
        up_for_database: up_for_database(&m.status),
        failure_domain: m.failure_domain.clone(),
        groups: m.groups.clone(),
        architecture: m.architecture.clone(),
        held: m.scheduler == HELD,
        incus_version,
        version_error,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ClusterQuorumState {
    /// At least one more voter can be lost without stopping changes.
    Healthy,
    /// Exactly a majority of voters answers: losing one more stops every change.
    AtRisk,
    /// Fewer than a majority answers: the cluster cannot change anything.
    Lost,
    /// A single server, no cluster database to keep a majority for.
    NotClustered,
    /// Incus reported no voters, so the majority cannot be judged.
    Unknown,
}

/// The cluster database's majority, now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ClusterQuorum {
    pub state: ClusterQuorumState,
    pub voters: u32,
    pub voters_up: u32,
    /// Voters that must answer for the cluster to accept changes (a majority).
    pub needed: u32,
    pub standbys: u32,
    pub standbys_up: u32,
    /// Members holding no copy of the cluster database.
    pub spares: u32,
    #[cfg_attr(feature = "utoipa", schema(required = true))]
    pub leader: Option<String>,
    /// Voters that can be lost at once, now, before changes stop. Incus promotes a stand-by
    /// when a voter goes offline, which raises this again afterwards.
    pub can_lose: u32,
    /// One sentence for the page.
    pub summary: String,
    #[cfg_attr(feature = "utoipa", schema(required = true))]
    pub next_step: Option<String>,
    pub warnings: Vec<String>,
}

fn plural(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// Judges the cluster database's majority from the members and their roles.
pub fn quorum(clustered: bool, members: &[ClusterMemberDetail]) -> ClusterQuorum {
    let count = |role: ClusterDatabaseRole, up: bool| {
        members
            .iter()
            .filter(|m| m.database_role == role && (!up || m.up_for_database))
            .count() as u32
    };
    let voters = count(ClusterDatabaseRole::Voter, false);
    let voters_up = count(ClusterDatabaseRole::Voter, true);
    let standbys = count(ClusterDatabaseRole::Standby, false);
    let standbys_up = count(ClusterDatabaseRole::Standby, true);
    let spares = count(ClusterDatabaseRole::Spare, false);
    let needed = if voters == 0 { 0 } else { voters / 2 + 1 };
    let leader = members.iter().find(|m| m.leader).map(|m| m.name.clone());
    let can_lose = voters_up.saturating_sub(needed);
    let mut warnings = Vec::new();
    let (state, summary, next_step) = if !clustered {
        (
            ClusterQuorumState::NotClustered,
            "A single Incus server: there is no cluster database majority to keep.".to_owned(),
            None,
        )
    } else if voters == 0 {
        (
            ClusterQuorumState::Unknown,
            "Incus reports no database voters, so the majority cannot be judged.".to_owned(),
            Some("check the members with incus cluster list on a member".to_owned()),
        )
    } else {
        let online = format!("{voters_up} of {voters} voters answer; {needed} needed",);
        if voters_up < needed {
            (
                ClusterQuorumState::Lost,
                format!(
                    "{online}. The cluster cannot change anything until a majority is back; instances keep running."
                ),
                Some(
                    "bring the offline voters back before anything else (runbook: quorum-loss)"
                        .to_owned(),
                ),
            )
        } else if can_lose == 0 {
            (
                ClusterQuorumState::AtRisk,
                format!("{online}. Losing one more voter stops every change to the cluster."),
                Some(if standbys_up > 0 {
                    "Incus promotes a stand-by when a voter is lost; keep maintenance off the voters until all answer".to_owned()
                } else {
                    "bring the missing voters back, or add members so Incus can elect more, before any maintenance".to_owned()
                }),
            )
        } else {
            (
                ClusterQuorumState::Healthy,
                format!(
                    "{online}. {} can be lost at once without stopping changes.",
                    plural(can_lose, "voter", "voters")
                ),
                None,
            )
        }
    };
    if clustered && voters > 0 && voters < 3 && members.len() >= 3 {
        warnings.push(format!(
            "only {} with {} members: Incus normally keeps three",
            plural(voters, "voter", "voters"),
            members.len()
        ));
    }
    if clustered && voters > 0 && leader.is_none() {
        warnings.push("no member reports leading the cluster database".to_owned());
    }
    let domains: std::collections::BTreeSet<&str> =
        members.iter().map(|m| m.failure_domain.as_str()).collect();
    let voter_domains: std::collections::BTreeSet<&str> = members
        .iter()
        .filter(|m| m.database_role == ClusterDatabaseRole::Voter)
        .map(|m| m.failure_domain.as_str())
        .collect();
    if domains.len() > 1 && voters > 1 && voter_domains.len() == 1 {
        warnings.push(format!(
            "every voter is in failure domain {}: one fault there can take the majority",
            voter_domains.into_iter().next().unwrap_or_default()
        ));
    }
    ClusterQuorum {
        state,
        voters,
        voters_up,
        needed,
        standbys,
        standbys_up,
        spares,
        leader,
        can_lose,
        summary,
        next_step,
        warnings,
    }
}

/// How a running release compares with the pinned one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ClusterVersionComparison {
    Same,
    Older,
    Newer,
    Unknown,
}

/// The members running one release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ClusterVersionGroup {
    /// `null`: the members whose version could not be read.
    #[cfg_attr(feature = "utoipa", schema(required = true))]
    pub version: Option<String>,
    pub members: Vec<String>,
    pub against_pin: ClusterVersionComparison,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum ClusterVersionState {
    /// Every member runs the pinned release.
    MatchesPin,
    /// Every member runs one release, older than the pinned one: an upgrade is due.
    BehindPin,
    /// Every member runs one release, newer than the one Apelora is pinned to and tested with.
    AheadOfPin,
    /// Members run different releases: an upgrade is under way or was left half done.
    Mixed,
    /// Some versions could not be read.
    Unknown,
}

/// Version skew across members and whether a rolling upgrade could start now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ClusterVersions {
    /// The Incus release Apelora is pinned to (`upstream.lock.yaml`), without the `v`.
    pub pinned: String,
    pub groups: Vec<ClusterVersionGroup>,
    pub state: ClusterVersionState,
    pub summary: String,
    /// Nothing stands in the way of upgrading the next member.
    pub ready: bool,
    /// What must be fixed before a member is upgraded.
    pub blockers: Vec<String>,
    /// Facts worth knowing during an upgrade (members in maintenance, blocked members).
    pub notes: Vec<String>,
}

/// Compares dotted release numbers (`7.0.1` < `7.0.10`); `None` when either is not numeric.
pub fn compare_versions(a: &str, b: &str) -> Option<std::cmp::Ordering> {
    let parse = |s: &str| -> Option<Vec<u64>> {
        s.trim()
            .trim_start_matches('v')
            .split('.')
            .map(|p| p.parse().ok())
            .collect()
    };
    let (mut x, mut y) = (parse(a)?, parse(b)?);
    let n = x.len().max(y.len());
    x.resize(n, 0);
    y.resize(n, 0);
    Some(x.cmp(&y))
}

/// Groups members by the release they run and judges the skew against `pinned`.
pub fn versions(
    pinned: &str,
    members: &[ClusterMemberDetail],
    quorum: &ClusterQuorum,
) -> ClusterVersions {
    use std::cmp::Ordering;
    let pinned = pinned.trim().trim_start_matches('v').to_owned();
    let mut by: BTreeMap<Option<String>, Vec<String>> = BTreeMap::new();
    for m in members {
        by.entry(m.incus_version.clone())
            .or_default()
            .push(m.name.clone());
    }
    let against = |v: &Option<String>| match v.as_deref().and_then(|v| compare_versions(v, &pinned))
    {
        Some(Ordering::Equal) => ClusterVersionComparison::Same,
        Some(Ordering::Less) => ClusterVersionComparison::Older,
        Some(Ordering::Greater) => ClusterVersionComparison::Newer,
        None => ClusterVersionComparison::Unknown,
    };
    let groups: Vec<ClusterVersionGroup> = by
        .into_iter()
        .map(|(version, members)| ClusterVersionGroup {
            against_pin: against(&version),
            version,
            members,
        })
        .collect();
    let unknown = groups.iter().any(|g| g.version.is_none());
    let known: Vec<&ClusterVersionGroup> = groups.iter().filter(|g| g.version.is_some()).collect();
    let (state, summary) = if members.is_empty() || unknown {
        (
            ClusterVersionState::Unknown,
            "The Incus release of some members could not be read.".to_owned(),
        )
    } else if known.len() > 1 {
        (
            ClusterVersionState::Mixed,
            format!(
                "Members run {} different Incus releases; Apelora is pinned to {pinned}.",
                known.len()
            ),
        )
    } else {
        let v = known[0].version.clone().unwrap_or_default();
        match known[0].against_pin {
            ClusterVersionComparison::Same => (
                ClusterVersionState::MatchesPin,
                format!("Every member runs Incus {v}, the release Apelora is pinned to."),
            ),
            ClusterVersionComparison::Older => (
                ClusterVersionState::BehindPin,
                format!(
                    "Every member runs Incus {v}; Apelora is pinned to {pinned}, so an upgrade is due."
                ),
            ),
            ClusterVersionComparison::Newer => (
                ClusterVersionState::AheadOfPin,
                format!(
                    "Every member runs Incus {v}, newer than {pinned}, the release Apelora is pinned to and tested with."
                ),
            ),
            ClusterVersionComparison::Unknown => (
                ClusterVersionState::Unknown,
                format!("Every member runs Incus {v}, which cannot be compared with {pinned}."),
            ),
        }
    };
    let mut blockers = Vec::new();
    let offline: Vec<&str> = members
        .iter()
        .filter(|m| m.status == "Offline")
        .map(|m| m.name.as_str())
        .collect();
    if !offline.is_empty() {
        blockers.push(format!(
            "offline: {}. Incus cannot upgrade a cluster with offline members, and the cluster would end up blocked",
            offline.join(", ")
        ));
    }
    match quorum.state {
        ClusterQuorumState::AtRisk | ClusterQuorumState::Lost => blockers.push(
            "the cluster database has no voter to spare: restarting a voter for its upgrade could stop every change".to_owned(),
        ),
        _ => {}
    }
    if unknown {
        blockers.push("read every member's release first".to_owned());
    }
    let mut notes = Vec::new();
    let blocked: Vec<&str> = members
        .iter()
        .filter(|m| m.status == "Blocked")
        .map(|m| m.name.as_str())
        .collect();
    if !blocked.is_empty() {
        notes.push(format!(
            "blocked: {}. The release being installed changes the database or API, and these members wait until every member runs it; upgrade the remaining members",
            blocked.join(", ")
        ));
    }
    let maintenance: Vec<&str> = members
        .iter()
        .filter(|m| matches!(m.status.as_str(), "Evacuating" | "Evacuated" | "Restoring"))
        .map(|m| m.name.as_str())
        .collect();
    if !maintenance.is_empty() {
        notes.push(format!(
            "in maintenance: {} (evacuated or moving instances). Restore {} once its upgrade is done",
            maintenance.join(", "),
            if maintenance.len() == 1 { "it" } else { "each" }
        ));
    }
    let due = matches!(
        state,
        ClusterVersionState::BehindPin | ClusterVersionState::Mixed
    );
    ClusterVersions {
        pinned,
        groups,
        state,
        summary,
        ready: due && blockers.is_empty(),
        blockers,
        notes,
    }
}

/// A cluster group (Incus `/1.0/cluster/groups`): a named set of members that instances and
/// projects can be placed on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ClusterGroupInfo {
    pub name: String,
    pub description: String,
    pub members: Vec<String>,
}

/// Cluster-wide settings the Servers area shows: key, label and what it does, in our words.
/// Only these keys ever leave the control plane (server configuration also holds secrets).
pub const CLUSTER_SETTINGS: &[(&str, &str, &str)] = &[
    (
        "cluster.offline_threshold",
        "Offline after (seconds)",
        "How long a member may miss heartbeats before it counts as offline.",
    ),
    (
        "cluster.healing_threshold",
        "Evacuate offline members after (seconds)",
        "0 leaves offline members alone; otherwise Incus evacuates them after this long.",
    ),
    (
        "cluster.max_voters",
        "Database voters",
        "How many members vote in the cluster database (an odd number, 3 or more).",
    ),
    (
        "cluster.max_standby",
        "Database stand-bys",
        "How many members keep a copy of the cluster database without voting (0 to 5).",
    ),
    (
        "cluster.join_token_expiry",
        "Join tokens expire after",
        "Apelora recommends one hour or less.",
    ),
    (
        "cluster.images_minimal_replica",
        "Copies of each image",
        "How many members keep each image (-1: every member).",
    ),
    (
        "cluster.rebalance.interval",
        "Rebalance every (minutes)",
        "0: Incus does not move instances to even out load.",
    ),
    (
        "images.auto_update_interval",
        "Check for image updates every (hours)",
        "0 turns the checks off.",
    ),
    (
        "images.auto_update_cached",
        "Update cached images",
        "Whether images cached from image servers are refreshed automatically.",
    ),
];

/// One cluster setting: its value when set, and Incus's default.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct ClusterSetting {
    pub key: String,
    pub label: String,
    pub meaning: String,
    /// The value set in the cluster's configuration; `null` when it is not set.
    #[cfg_attr(feature = "utoipa", schema(required = true))]
    pub value: Option<String>,
    /// Incus's default as the server describes it; `null` when it did not say.
    #[cfg_attr(feature = "utoipa", schema(required = true))]
    pub default: Option<String>,
    /// Not set, so Incus's default applies.
    pub uses_default: bool,
}

/// The settings in [`CLUSTER_SETTINGS`] order, from the configured values and the defaults.
pub fn settings(
    values: &BTreeMap<String, String>,
    defaults: &BTreeMap<String, String>,
) -> Vec<ClusterSetting> {
    CLUSTER_SETTINGS
        .iter()
        .map(|(key, label, meaning)| {
            let value = values.get(*key).filter(|v| !v.is_empty()).cloned();
            ClusterSetting {
                key: (*key).to_owned(),
                label: (*label).to_owned(),
                meaning: (*meaning).to_owned(),
                uses_default: value.is_none(),
                value,
                default: defaults.get(*key).cloned(),
            }
        })
        .collect()
}

/// Why a value is not acceptable for a cluster setting, or `Ok` when it is. An empty value is
/// always acceptable: it removes the setting so Incus's default applies again.
pub fn check_setting(key: &str, value: &str) -> Result<(), String> {
    if !CLUSTER_SETTINGS.iter().any(|(k, _, _)| *k == key) {
        return Err(format!("{key} is not a setting Apelora changes"));
    }
    if value.is_empty() {
        return Ok(());
    }
    let int = || {
        value
            .parse::<i64>()
            .map_err(|_| format!("{key} is a whole number"))
    };
    let range = |lo: i64, hi: i64, unit: &str| {
        let n = int()?;
        if (lo..=hi).contains(&n) {
            Ok(())
        } else {
            Err(format!("{key} is {lo} to {hi}{unit}"))
        }
    };
    match key {
        "cluster.offline_threshold" => range(10, 3600, " seconds"),
        "cluster.healing_threshold" => {
            let n = int()?;
            if n == 0 || (10..=86_400).contains(&n) {
                Ok(())
            } else {
                Err(format!("{key} is 0 (off) or 10 to 86400 seconds"))
            }
        }
        "cluster.max_voters" => {
            let n = int()?;
            if (3..=11).contains(&n) && n % 2 == 1 {
                Ok(())
            } else {
                Err(format!("{key} is an odd number from 3 to 11"))
            }
        }
        "cluster.max_standby" => range(0, 5, ""),
        "cluster.images_minimal_replica" => {
            let n = int()?;
            if n == -1 || (1..=100).contains(&n) {
                Ok(())
            } else {
                Err(format!("{key} is -1 (every member) or 1 to 100"))
            }
        }
        "cluster.rebalance.interval" => range(0, 10_080, " minutes"),
        "images.auto_update_interval" => range(0, 8_760, " hours"),
        "images.auto_update_cached" => match value {
            "true" | "false" => Ok(()),
            _ => Err(format!("{key} is true or false")),
        },
        "cluster.join_token_expiry" => {
            let (digits, unit) = value.split_at(value.len().saturating_sub(1));
            let ok = !digits.is_empty()
                && digits.len() <= 4
                && !digits.starts_with('0')
                && digits.bytes().all(|b| b.is_ascii_digit())
                && matches!(unit, "S" | "M" | "H" | "d" | "w" | "m" | "y");
            if ok {
                Ok(())
            } else {
                Err(format!(
                    "{key} is a number and a unit (S, M, H, d, w, m, y), like 1H"
                ))
            }
        }
        _ => Err(format!("{key} is not a setting Apelora changes")),
    }
}

/// What a person should know before this value takes effect (never a refusal).
pub fn setting_warnings(key: &str, value: &str) -> Vec<String> {
    let mut out = Vec::new();
    let n = value.parse::<i64>().ok();
    match key {
        "cluster.healing_threshold" if n.is_some_and(|n| n > 0) => out.push(
            "Incus will evacuate offline members by itself. Apelora's supervisor already recovers failed servers, and two recovery authorities can act on the same workload; keep this at 0 unless you mean it."
                .into(),
        ),
        "cluster.rebalance.interval" if n.is_some_and(|n| n > 0) => out.push(
            "Incus will move instances to even out load without Apelora's dry run, and a module with a data-local volume stops while it moves."
                .into(),
        ),
        "cluster.offline_threshold" if n.is_some_and(|n| n < 20) => out.push(
            "Below Incus's default of 20 seconds a busy member can be reported offline by mistake, which starts recovery."
                .into(),
        ),
        "cluster.max_voters" | "cluster.max_standby" => out.push(
            "Changing database roles moves the cluster database between members. Do it with every member online."
                .into(),
        ),
        "cluster.join_token_expiry" => {
            let hours = value
                .split_at(value.len().saturating_sub(1))
                .0
                .parse::<i64>()
                .ok()
                .map(|d| match value.chars().last() {
                    Some('S') => d / 3600,
                    Some('M') => d / 60,
                    Some('H') => d,
                    Some('d') => d * 24,
                    Some('w') => d * 24 * 7,
                    Some('m') => d * 24 * 30,
                    Some('y') => d * 24 * 365,
                    _ => 0,
                });
            if hours.is_some_and(|h| h > 1) {
                out.push("Apelora recommends join tokens that last an hour or less.".into());
            }
        }
        "images.auto_update_interval" if n == Some(0) => out.push(
            "Image updates stop being checked; pinned images are unaffected, others go stale.".into(),
        ),
        _ => {}
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str, roles: &[&str], status: &str, domain: &str) -> ClusterMember {
        ClusterMember {
            name: name.into(),
            url: String::new(),
            roles: roles.iter().map(|r| (*r).to_owned()).collect(),
            status: status.into(),
            message: String::new(),
            architecture: "aarch64".into(),
            failure_domain: domain.into(),
            database: !roles.is_empty(),
            groups: vec!["default".into()],
            scheduler: "all".into(),
        }
    }

    fn details(members: &[ClusterMember], version: &str) -> Vec<ClusterMemberDetail> {
        members
            .iter()
            .map(|m| member_detail(m, Ok(Some(version.into()))))
            .collect()
    }

    /// The test cluster's shape (REAL INCUS TESTED read, 2026-10-21): three voters, one of them
    /// the leader, and a stand-by.
    fn lab(status3: &str) -> Vec<ClusterMember> {
        vec![
            member("t1", &["database-leader", "database"], "Online", "default"),
            member("t2", &["database"], "Online", "default"),
            member("t3", &["database"], status3, "default"),
            member("t4", &["database-standby"], "Online", "default"),
        ]
    }

    #[test]
    fn roles_are_read_from_incus_names() {
        let d = details(&lab("Online"), "7.0.1");
        assert_eq!(d[0].database_role, ClusterDatabaseRole::Voter);
        assert!(d[0].leader);
        assert_eq!(d[3].database_role, ClusterDatabaseRole::Standby);
        let spare = member_detail(
            &member(
                "t5",
                &["event-hub", "ovn-chassis", "database-client"],
                "Online",
                "a",
            ),
            Err("no answer".into()),
        );
        assert_eq!(spare.database_role, ClusterDatabaseRole::Spare);
        assert!(spare.event_hub && spare.ovn_chassis && spare.database_excluded);
        assert_eq!(spare.version_error.as_deref(), Some("no answer"));
    }

    #[test]
    fn three_voters_survive_one_loss_and_two_do_not() {
        let q = quorum(true, &details(&lab("Online"), "7.0.1"));
        assert_eq!(
            (q.state, q.voters, q.voters_up, q.needed, q.can_lose),
            (ClusterQuorumState::Healthy, 3, 3, 2, 1)
        );
        assert_eq!(q.leader.as_deref(), Some("t1"));
        assert_eq!((q.standbys, q.standbys_up, q.spares), (1, 1, 0));
        assert!(q.summary.contains("1 voter can be lost"), "{}", q.summary);

        let q = quorum(true, &details(&lab("Offline"), "7.0.1"));
        assert_eq!(
            (q.state, q.voters_up, q.can_lose),
            (ClusterQuorumState::AtRisk, 2, 0)
        );
        assert!(
            q.next_step
                .as_deref()
                .is_some_and(|s| s.contains("stand-by"))
        );

        let mut two_down = lab("Offline");
        two_down[1].status = "Offline".into();
        let q = quorum(true, &details(&two_down, "7.0.1"));
        assert_eq!(q.state, ClusterQuorumState::Lost);

        // Evacuated members keep voting; blocked ones are not counted.
        let q = quorum(true, &details(&lab("Evacuated"), "7.0.1"));
        assert_eq!(q.state, ClusterQuorumState::Healthy);
        let q = quorum(true, &details(&lab("Blocked"), "7.0.1"));
        assert_eq!(q.state, ClusterQuorumState::AtRisk);
    }

    #[test]
    fn quorum_warnings_and_edge_cases() {
        assert_eq!(quorum(false, &[]).state, ClusterQuorumState::NotClustered);
        let none = details(&[member("a", &[], "Online", "x")], "7.0.1");
        assert_eq!(quorum(true, &none).state, ClusterQuorumState::Unknown);
        let mut m = lab("Online");
        m[3].failure_domain = "rack-2".into();
        let q = quorum(true, &details(&m, "7.0.1"));
        assert!(
            q.warnings[0].contains("failure domain default"),
            "{:?}",
            q.warnings
        );
        // A single voter: losing it stops changes.
        let one = details(
            &[member("a", &["database-leader", "database"], "Online", "x")],
            "7.0.1",
        );
        let q = quorum(true, &one);
        assert_eq!((q.state, q.needed), (ClusterQuorumState::AtRisk, 1));
    }

    #[test]
    fn versions_compare_numerically() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("7.0.1", "v7.0.2"), Some(Less));
        assert_eq!(compare_versions("7.0.10", "7.0.2"), Some(Greater));
        assert_eq!(compare_versions("7.0", "7.0.0"), Some(Equal));
        assert_eq!(compare_versions("7.0.1-rc", "7.0.1"), None);
    }

    #[test]
    fn version_skew_and_upgrade_readiness() {
        let m = details(&lab("Online"), "7.0.1");
        let q = quorum(true, &m);
        let v = versions("v7.0.2", &m, &q);
        assert_eq!((v.state, v.ready), (ClusterVersionState::BehindPin, true));
        assert_eq!(v.pinned, "7.0.2");
        assert_eq!(v.groups[0].against_pin, ClusterVersionComparison::Older);

        let mut m = details(&lab("Online"), "7.0.1");
        m[0].incus_version = Some("7.0.2".into());
        m[0].status = "Evacuated".into();
        let v = versions("v7.0.2", &m, &quorum(true, &m));
        assert_eq!((v.state, v.ready), (ClusterVersionState::Mixed, true));
        assert_eq!(v.groups.len(), 2);
        assert!(
            v.notes[0].starts_with("in maintenance: t1"),
            "{:?}",
            v.notes
        );

        let m = details(&lab("Offline"), "7.0.2");
        let v = versions("v7.0.2", &m, &quorum(true, &m));
        assert_eq!(v.state, ClusterVersionState::MatchesPin);
        assert!(!v.ready, "nothing due");
        assert!(v.blockers[0].starts_with("offline: t3"), "{:?}", v.blockers);
        assert!(v.blockers[1].contains("no voter to spare"));

        let mut m = details(&lab("Online"), "7.0.3");
        m[1].incus_version = None;
        let v = versions("v7.0.2", &m, &quorum(true, &m));
        assert_eq!((v.state, v.ready), (ClusterVersionState::Unknown, false));
    }

    #[test]
    fn settings_say_which_values_are_defaults_and_never_leak_other_keys() {
        let values = BTreeMap::from([
            ("cluster.join_token_expiry".to_owned(), "30M".to_owned()),
            ("core.trust_password".to_owned(), "secret".to_owned()),
            ("cluster.max_voters".to_owned(), String::new()),
        ]);
        let defaults = BTreeMap::from([("cluster.max_voters".to_owned(), "3".to_owned())]);
        let s = settings(&values, &defaults);
        assert_eq!(s.len(), CLUSTER_SETTINGS.len());
        let token = s
            .iter()
            .find(|x| x.key == "cluster.join_token_expiry")
            .expect("key");
        assert_eq!(
            (token.value.as_deref(), token.uses_default),
            (Some("30M"), false)
        );
        let voters = s
            .iter()
            .find(|x| x.key == "cluster.max_voters")
            .expect("key");
        assert_eq!(
            (
                voters.value.as_deref(),
                voters.default.as_deref(),
                voters.uses_default
            ),
            (None, Some("3"), true)
        );
        assert!(!serde_json::to_string(&s).expect("json").contains("secret"));
    }

    #[test]
    fn cluster_setting_values_are_checked_at_their_limits() {
        let ok = |k: &str, v: &str| check_setting(k, v).is_ok();
        assert!(ok("cluster.offline_threshold", "10") && ok("cluster.offline_threshold", "3600"));
        assert!(!ok("cluster.offline_threshold", "9") && !ok("cluster.offline_threshold", "3601"));
        assert!(
            ok("cluster.healing_threshold", "0")
                && ok("cluster.healing_threshold", "10")
                && ok("cluster.healing_threshold", "86400")
        );
        assert!(!ok("cluster.healing_threshold", "5") && !ok("cluster.healing_threshold", "86401"));
        for v in ["3", "5", "11"] {
            assert!(ok("cluster.max_voters", v), "{v}");
        }
        for v in ["1", "2", "4", "13", "x"] {
            assert!(!ok("cluster.max_voters", v), "{v}");
        }
        assert!(
            ok("cluster.max_standby", "0")
                && ok("cluster.max_standby", "5")
                && !ok("cluster.max_standby", "6")
                && !ok("cluster.max_standby", "-1")
        );
        assert!(
            ok("cluster.images_minimal_replica", "-1")
                && ok("cluster.images_minimal_replica", "1")
                && ok("cluster.images_minimal_replica", "100")
        );
        assert!(
            !ok("cluster.images_minimal_replica", "0")
                && !ok("cluster.images_minimal_replica", "101")
                && !ok("cluster.images_minimal_replica", "-2")
        );
        assert!(
            ok("cluster.rebalance.interval", "0")
                && ok("cluster.rebalance.interval", "10080")
                && !ok("cluster.rebalance.interval", "10081")
        );
        assert!(
            ok("images.auto_update_interval", "0")
                && ok("images.auto_update_interval", "8760")
                && !ok("images.auto_update_interval", "8761")
        );
        assert!(
            ok("images.auto_update_cached", "true")
                && ok("images.auto_update_cached", "false")
                && !ok("images.auto_update_cached", "yes")
        );
        for v in ["1H", "30M", "2d", "9999y"] {
            assert!(ok("cluster.join_token_expiry", v), "{v}");
        }
        for v in ["H", "0H", "1h", "10000H", "1", "1Hx", "-1H"] {
            assert!(!ok("cluster.join_token_expiry", v), "{v}");
        }
    }

    #[test]
    fn an_empty_value_removes_a_setting_and_unknown_keys_are_refused() {
        for (k, _, _) in CLUSTER_SETTINGS {
            assert!(check_setting(k, "").is_ok(), "{k}");
        }
        for k in [
            "core.https_address",
            "cluster.https_address",
            "oidc.client.secret",
            "",
        ] {
            assert!(check_setting(k, "1").is_err(), "{k}");
            assert!(
                check_setting(k, "").is_err(),
                "{k} stays refused even when empty"
            );
        }
        assert!(
            check_setting("cluster.offline_threshold", "abc")
                .expect_err("refused")
                .contains("whole number")
        );
    }

    #[test]
    fn risky_settings_come_with_a_warning_and_safe_ones_without() {
        assert!(!setting_warnings("cluster.healing_threshold", "300").is_empty());
        assert!(setting_warnings("cluster.healing_threshold", "0").is_empty());
        assert!(!setting_warnings("cluster.rebalance.interval", "5").is_empty());
        assert!(setting_warnings("cluster.rebalance.interval", "0").is_empty());
        assert!(!setting_warnings("cluster.offline_threshold", "15").is_empty());
        assert!(setting_warnings("cluster.offline_threshold", "20").is_empty());
        assert!(!setting_warnings("cluster.max_voters", "5").is_empty());
        assert!(!setting_warnings("cluster.max_standby", "1").is_empty());
        assert!(!setting_warnings("cluster.join_token_expiry", "2H").is_empty());
        assert!(!setting_warnings("cluster.join_token_expiry", "1d").is_empty());
        assert!(setting_warnings("cluster.join_token_expiry", "1H").is_empty());
        assert!(setting_warnings("cluster.join_token_expiry", "30M").is_empty());
        assert!(!setting_warnings("images.auto_update_interval", "0").is_empty());
        assert!(setting_warnings("images.auto_update_interval", "6").is_empty());
        assert!(setting_warnings("images.auto_update_cached", "true").is_empty());
    }
}
