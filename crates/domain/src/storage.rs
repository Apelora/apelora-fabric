// SPDX-License-Identifier: AGPL-3.0-only
//! Persistent volumes of a module (docs/architecture/STORAGE.md).
//!
//! A design asks for a volume by name, size, class and mount path; it never names a pool. The
//! control plane resolves the class to a pool before the spec reaches the worker. Each instance
//! gets its own volumes, named after it, created on the member Incus placed the instance on
//! (Incus does not place an instance next to a member-local volume). Volumes outlive their
//! instance: removing a module never deletes its data.

use serde::{Deserialize, Serialize};

use crate::{InstanceKind, is_valid_name};

/// Volume config key naming the design's volume (`data`); the application and module keys
/// are the instance ones ([`crate::APPLICATION_KEY`], [`crate::MODULE_KEY`]).
pub const VOLUME_KEY: &str = "user.apelora.volume";
/// Label on a backup copy naming the volume it is a copy of.
pub const BACKUP_OF_KEY: &str = "user.apelora.backup-of";
/// Volumes one module may ask for.
pub const MAX_VOLUMES: usize = 4;
/// Smallest and largest volume Apelora creates, in MiB (64 MiB to 4 TiB).
pub const MIN_SIZE_MIB: u32 = 64;
pub const MAX_SIZE_MIB: u32 = 4 * 1024 * 1024;
/// Longest volume name in a design, so `<instance>-<volume>` stays a valid Incus name.
const MAX_NAME: usize = 16;

/// What the volume holds: a mounted filesystem or a raw disk (virtual machines only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum VolumeContent {
    #[default]
    Filesystem,
    Block,
}

impl VolumeContent {
    pub fn as_str(self) -> &'static str {
        match self {
            VolumeContent::Filesystem => "filesystem",
            VolumeContent::Block => "block",
        }
    }
}

/// The policy a volume is created under. Only `data-local` exists so far: persistent, one
/// writer, on the member's own pool; moving it means stopping the instance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum VolumeClass {
    #[default]
    DataLocal,
}

impl VolumeClass {
    pub fn as_str(self) -> &'static str {
        match self {
            VolumeClass::DataLocal => "data-local",
        }
    }
}

/// A volume a module asks for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct VolumeMount {
    /// Name in the design, e.g. `data`: lowercase letters, digits and `-`, at most 16.
    pub name: String,
    pub size_mib: u32,
    #[serde(default)]
    pub content: VolumeContent,
    #[serde(default)]
    pub class: VolumeClass,
    /// Where a filesystem volume is mounted inside the instance; none for a block volume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    /// The pool the class resolves to. Set by the control plane, never by a caller.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pool: Option<String>,
}

impl VolumeMount {
    /// Checks the volume on its own and for an instance of `kind`.
    pub fn validate(&self, kind: InstanceKind) -> Result<(), String> {
        let n = &self.name;
        if n.len() > MAX_NAME || !is_valid_name(n) || n.bytes().any(|c| c.is_ascii_uppercase()) {
            return Err(format!(
                "invalid volume name {n:?} (lowercase letters, digits and hyphens, at most {MAX_NAME})"
            ));
        }
        if !(MIN_SIZE_MIB..=MAX_SIZE_MIB).contains(&self.size_mib) {
            return Err(format!(
                "volume {n}: size must be {MIN_SIZE_MIB} MiB to {} GiB",
                MAX_SIZE_MIB / 1024
            ));
        }
        match (self.content, &self.path) {
            (VolumeContent::Filesystem, None) => {
                return Err(format!(
                    "volume {n}: a filesystem volume needs a mount path"
                ));
            }
            (VolumeContent::Filesystem, Some(p)) => {
                valid_mount_path(p).map_err(|why| format!("volume {n}: mount path {p:?} {why}"))?
            }
            (VolumeContent::Block, Some(_)) => {
                return Err(format!("volume {n}: a block volume has no mount path"));
            }
            (VolumeContent::Block, None) if kind != InstanceKind::VirtualMachine => {
                return Err(format!(
                    "volume {n}: block volumes attach only to virtual machines"
                ));
            }
            (VolumeContent::Block, None) => {}
        }
        if let Some(p) = &self.pool
            && !is_valid_name(p)
        {
            return Err(format!("volume {n}: invalid pool name {p:?}"));
        }
        Ok(())
    }

    /// The Incus disk device the volume is attached as.
    pub fn device(&self) -> String {
        format!("vol-{}", self.name)
    }
}

/// The Incus volume of `instance` for the design's volume `volume`.
pub fn volume_name(instance: &str, volume: &str) -> String {
    format!("{instance}-{volume}")
}

/// An absolute path without `.` or `..` segments, not `/` itself, of printable characters.
fn valid_mount_path(p: &str) -> Result<(), &'static str> {
    if !p.starts_with('/') || p == "/" {
        return Err("must be an absolute path below /");
    }
    if p.len() > 255 || p.chars().any(|c| c.is_control()) {
        return Err("is too long or has control characters");
    }
    if p.split('/').any(|s| s == "." || s == "..") {
        return Err("must not contain . or ..");
    }
    Ok(())
}

/// Checks a set of volumes for one instance: each on its own, names and paths unique, and the
/// Incus volume names valid for `instance`.
pub fn validate_volumes(
    instance: &str,
    kind: InstanceKind,
    volumes: &[VolumeMount],
) -> Result<(), String> {
    if volumes.len() > MAX_VOLUMES {
        return Err(format!("at most {MAX_VOLUMES} volumes per module"));
    }
    let mut names = std::collections::BTreeSet::new();
    let mut paths = std::collections::BTreeSet::new();
    for v in volumes {
        v.validate(kind)?;
        if !names.insert(v.name.as_str()) {
            return Err(format!("volume {} is listed twice", v.name));
        }
        if let Some(p) = &v.path
            && !paths.insert(p.trim_end_matches('/'))
        {
            return Err(format!("two volumes are mounted at {p}"));
        }
        let full = volume_name(instance, &v.name);
        if !is_valid_name(&full) {
            return Err(format!(
                "volume {}: {full:?} is too long for Incus; shorten the module or volume name",
                v.name
            ));
        }
    }
    Ok(())
}

/// Resize is grow-only: Incus accepts a shrink, which can destroy data.
pub fn check_resize(current_mib: u32, wanted_mib: u32) -> Result<(), String> {
    if wanted_mib < current_mib {
        return Err(format!(
            "volumes only grow: {wanted_mib} MiB is smaller than the current {current_mib} MiB"
        ));
    }
    if wanted_mib > MAX_SIZE_MIB {
        return Err(format!("at most {} GiB", MAX_SIZE_MIB / 1024));
    }
    Ok(())
}

/// How often something is done for a volume and how many results are kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct Schedule {
    /// Minutes between two runs: 15 minutes to one week.
    pub every_minutes: u32,
    /// How many of the scheduled results are kept: 1 to 50.
    pub keep: u32,
}

impl Schedule {
    pub fn validate(&self, what: &str) -> Result<(), String> {
        if !(15..=7 * 24 * 60).contains(&self.every_minutes) {
            return Err(format!("{what}: every 15 minutes to once a week"));
        }
        if !(1..=50).contains(&self.keep) {
            return Err(format!("{what}: keep 1 to 50"));
        }
        Ok(())
    }
}

/// Prefix of the snapshots the scheduler takes; only these are ever pruned by it.
pub const AUTO_SNAPSHOT_PREFIX: &str = "auto-";

/// `<prefix>YYYYMMDD-HHMMSS` for a moment in UTC.
pub fn stamped(prefix: &str, at: time::OffsetDateTime) -> String {
    let at = at.to_offset(time::UtcOffset::UTC);
    format!(
        "{prefix}{:04}{:02}{:02}-{:02}{:02}{:02}",
        at.year(),
        u8::from(at.month()),
        at.day(),
        at.hour(),
        at.minute(),
        at.second()
    )
}

/// Which scheduled snapshots to delete so that the newest `keep` remain. Names sort by time;
/// snapshots someone took by hand are never pruned.
pub fn snapshots_to_prune<'a>(names: &[&'a str], keep: u32) -> Vec<&'a str> {
    let mut auto: Vec<&str> = names
        .iter()
        .copied()
        .filter(|n| n.starts_with(AUTO_SNAPSHOT_PREFIX))
        .collect();
    auto.sort_unstable();
    let excess = auto.len().saturating_sub(keep as usize);
    auto.truncate(excess);
    auto
}

/// The Incus name of a backup copy of `volume` taken at `at`: `<volume>-bk-<YYYYMMDDHHMMSS>`,
/// shortened from the front of the volume's name when needed so it stays a valid name.
pub fn backup_name(volume: &str, at: time::OffsetDateTime) -> String {
    let stamp = stamped("", at).replace('-', "");
    let suffix = format!("-bk-{stamp}");
    let room = 63 - suffix.len();
    let base: String = volume.chars().take(room).collect();
    format!("{}{suffix}", base.trim_end_matches('-'))
}

/// What restoring a volume from one of its backups would do (the dry run of a restore). The
/// restore copies the backup over the volume (an Incus `refresh` copy): everything written since
/// the backup is replaced and the volume's own snapshots are deleted, so a safety backup of the
/// current data is taken first unless the person says otherwise.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct RestorePlan {
    /// The volume to overwrite (Incus name): what to type to confirm.
    pub volume: String,
    pub pool: String,
    /// Member of a member-local pool; empty on a standalone server.
    pub member: String,
    pub size_mib: u32,
    pub backup: String,
    pub backup_pool: String,
    pub backup_member: String,
    /// When the backup was taken (RFC 3339): everything written after it is replaced.
    pub backup_taken_at: String,
    /// The volume's own snapshots, which the restore deletes.
    pub snapshots_deleted: Vec<String>,
    /// Instances using the volume.
    pub used_by: Vec<String>,
    /// Of those, the ones running now: they must be stopped first.
    pub running: Vec<String>,
    /// The pool a safety backup of the current data goes to; `None` when none is taken.
    pub safety_backup_pool: Option<String>,
    /// Why the restore cannot run now; empty when it can.
    pub blocked: Vec<String>,
    /// What the restore job will do, in order.
    pub steps: Vec<String>,
}

/// What [`plan_restore`] looks at.
#[derive(Debug, Clone)]
pub struct RestoreSubject<'a> {
    pub volume: &'a str,
    pub pool: &'a str,
    pub member: &'a str,
    pub size_mib: u32,
    /// The volume exists in Incus.
    pub volume_present: bool,
    pub backup: &'a str,
    pub backup_pool: &'a str,
    pub backup_member: &'a str,
    pub backup_taken_at: &'a str,
    /// The backup exists in Incus.
    pub backup_present: bool,
    pub snapshots: &'a [String],
    /// Instances using the volume, with whether each runs now.
    pub users: &'a [(String, bool)],
    /// Where a safety backup would go (the configured backup pool), if anywhere.
    pub safety_pool: Option<&'a str>,
    /// The person asked for a safety backup.
    pub safety_backup: bool,
}

/// The dry run of a restore: what is replaced, what blocks it and the steps. Pure.
pub fn plan_restore(s: &RestoreSubject<'_>) -> RestorePlan {
    let running: Vec<String> = s
        .users
        .iter()
        .filter(|(_, r)| *r)
        .map(|(n, _)| n.clone())
        .collect();
    let mut blocked = Vec::new();
    if !s.volume_present {
        blocked.push(format!(
            "volume {} is not in Incus (its member may be unreachable); nothing can be restored into it",
            s.volume
        ));
    }
    if !s.backup_present {
        blocked.push(format!("backup {} is not in Incus", s.backup));
    }
    if !running.is_empty() {
        blocked.push(format!(
            "stop {} first: a volume is never overwritten under a running instance",
            running.join(", ")
        ));
    }
    let safety = if s.safety_backup {
        match s.safety_pool {
            Some(p) => Some(p.to_owned()),
            None => {
                blocked.push(
                    "no backup pool is configured (APELORA_BACKUP_POOL) for the safety backup"
                        .into(),
                );
                None
            }
        }
    } else {
        None
    };
    let at = if s.member.is_empty() {
        String::new()
    } else {
        format!(" on {}", s.member)
    };
    let mut steps = Vec::new();
    if let Some(p) = &safety {
        steps.push(format!(
            "Copy the volume as it is now into pool {p} (a safety backup, listed with the others)"
        ));
    }
    steps.push(format!(
        "Copy backup {} over volume {}{at}",
        s.backup, s.volume
    ));
    if !s.snapshots.is_empty() {
        steps.push(format!(
            "Incus deletes the volume's {} snapshot{} while copying",
            s.snapshots.len(),
            if s.snapshots.len() == 1 { "" } else { "s" }
        ));
    }
    if !s.users.is_empty() {
        steps.push(format!(
            "Leave {} stopped: start {} again once the data is checked",
            s.users
                .iter()
                .map(|(n, _)| n.as_str())
                .collect::<Vec<_>>()
                .join(", "),
            if s.users.len() == 1 { "it" } else { "them" }
        ));
    }
    RestorePlan {
        volume: s.volume.to_owned(),
        pool: s.pool.to_owned(),
        member: s.member.to_owned(),
        size_mib: s.size_mib,
        backup: s.backup.to_owned(),
        backup_pool: s.backup_pool.to_owned(),
        backup_member: s.backup_member.to_owned(),
        backup_taken_at: s.backup_taken_at.to_owned(),
        snapshots_deleted: s.snapshots.to_vec(),
        used_by: s.users.iter().map(|(n, _)| n.clone()).collect(),
        running,
        safety_backup_pool: safety,
        blocked,
        steps,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn subject<'a>(users: &'a [(String, bool)], snapshots: &'a [String]) -> RestoreSubject<'a> {
        RestoreSubject {
            volume: "shop-db-01-data",
            pool: "local",
            member: "m1",
            size_mib: 1024,
            volume_present: true,
            backup: "shop-db-01-data-bk-20261021120000",
            backup_pool: "backup",
            backup_member: "m2",
            backup_taken_at: "2026-10-21T12:00:00Z",
            backup_present: true,
            snapshots,
            users,
            safety_pool: Some("backup"),
            safety_backup: true,
        }
    }

    #[test]
    fn a_restore_says_what_it_replaces_and_refuses_under_a_running_instance() {
        let snaps = vec!["snap-1".to_owned(), "auto-2".to_owned()];
        let stopped = vec![("shop-db-01".to_owned(), false)];
        let p = plan_restore(&subject(&stopped, &snaps));
        assert!(p.blocked.is_empty(), "{:?}", p.blocked);
        assert_eq!(p.safety_backup_pool.as_deref(), Some("backup"));
        assert_eq!(p.snapshots_deleted, snaps);
        assert_eq!(p.steps.len(), 4, "{:?}", p.steps);
        assert!(p.steps[0].contains("safety backup"));
        assert!(p.steps[2].contains("2 snapshots"));

        let running = vec![("shop-db-01".to_owned(), true)];
        let p = plan_restore(&subject(&running, &[]));
        assert_eq!(p.running, vec!["shop-db-01".to_owned()]);
        assert!(p.blocked[0].starts_with("stop shop-db-01 first"));

        let mut s = subject(&stopped, &[]);
        s.safety_pool = None;
        assert!(plan_restore(&s).blocked[0].contains("APELORA_BACKUP_POOL"));
        s.safety_backup = false;
        let p = plan_restore(&s);
        assert!(p.blocked.is_empty() && p.safety_backup_pool.is_none());
        assert!(p.steps[0].starts_with("Copy backup"));

        let mut s = subject(&stopped, &[]);
        s.volume_present = false;
        s.backup_present = false;
        assert_eq!(plan_restore(&s).blocked.len(), 2);
    }

    fn fs(name: &str, path: &str) -> VolumeMount {
        VolumeMount {
            name: name.into(),
            size_mib: 1024,
            content: VolumeContent::Filesystem,
            class: VolumeClass::DataLocal,
            path: Some(path.into()),
            pool: None,
        }
    }

    #[test]
    fn a_filesystem_volume_needs_a_safe_absolute_path() {
        let ok = fs("data", "/var/lib/postgresql");
        assert_eq!(ok.validate(InstanceKind::Container), Ok(()));
        for bad in ["relative", "/", "/srv/../etc", "/srv/./x", "/a\nb"] {
            assert!(
                fs("data", bad).validate(InstanceKind::Container).is_err(),
                "{bad:?} accepted"
            );
        }
        let none = VolumeMount {
            path: None,
            ..ok.clone()
        };
        assert!(none.validate(InstanceKind::Container).is_err());
    }

    #[test]
    fn names_are_short_lowercase_and_sizes_bounded() {
        for bad in ["Data", "-x", "x-", "a_b", "abcdefghijklmnopq", ""] {
            assert!(
                fs(bad, "/srv").validate(InstanceKind::Container).is_err(),
                "{bad:?} accepted"
            );
        }
        assert!(
            fs("abcdefghijklmnop", "/srv")
                .validate(InstanceKind::Container)
                .is_ok()
        );
        for (size, ok) in [
            (63, false),
            (64, true),
            (MAX_SIZE_MIB, true),
            (MAX_SIZE_MIB + 1, false),
        ] {
            let v = VolumeMount {
                size_mib: size,
                ..fs("data", "/srv")
            };
            assert_eq!(v.validate(InstanceKind::Container).is_ok(), ok, "{size}");
        }
    }

    #[test]
    fn block_volumes_are_for_virtual_machines_and_have_no_path() {
        let block = VolumeMount {
            content: VolumeContent::Block,
            path: None,
            ..fs("disk", "/x")
        };
        assert!(block.validate(InstanceKind::VirtualMachine).is_ok());
        assert!(block.validate(InstanceKind::Container).is_err());
        let with_path = VolumeMount {
            path: Some("/srv".into()),
            ..block
        };
        assert!(with_path.validate(InstanceKind::VirtualMachine).is_err());
    }

    #[test]
    fn a_set_has_unique_names_and_paths_and_fits_incus_names() {
        let k = InstanceKind::Container;
        assert!(validate_volumes("shop-db-01", k, &[fs("data", "/a"), fs("logs", "/b")]).is_ok());
        assert!(validate_volumes("shop-db-01", k, &[fs("data", "/a"), fs("data", "/b")]).is_err());
        assert!(validate_volumes("shop-db-01", k, &[fs("data", "/a"), fs("logs", "/a/")]).is_err());
        let five: Vec<_> = (0..5)
            .map(|i| fs(&format!("v{i}"), &format!("/v{i}")))
            .collect();
        assert!(validate_volumes("shop-db-01", k, &five).is_err());
        let long = "a".repeat(59);
        assert!(validate_volumes(&long, k, &[fs("data", "/a")]).is_err());
    }

    #[test]
    fn resize_only_grows() {
        assert!(check_resize(1024, 1024).is_ok());
        assert!(check_resize(1024, 2048).is_ok());
        assert!(check_resize(1024, 512).is_err());
        assert!(check_resize(1024, MAX_SIZE_MIB + 1).is_err());
    }

    #[test]
    fn names_of_the_incus_volume_and_device() {
        assert_eq!(volume_name("shop-db-01", "data"), "shop-db-01-data");
        assert_eq!(fs("data", "/srv").device(), "vol-data");
    }

    #[test]
    fn schedules_are_bounded() {
        let ok = Schedule {
            every_minutes: 60,
            keep: 7,
        };
        assert!(ok.validate("snapshots").is_ok());
        for (every, keep) in [(14, 7), (7 * 24 * 60 + 1, 7), (60, 0), (60, 51)] {
            assert!(
                Schedule {
                    every_minutes: every,
                    keep
                }
                .validate("snapshots")
                .is_err(),
                "{every}/{keep}"
            );
        }
    }

    #[test]
    fn only_the_oldest_scheduled_snapshots_are_pruned() {
        let names = [
            "auto-20261021-010000",
            "manual-before-upgrade",
            "auto-20261021-030000",
            "auto-20261021-020000",
            "snap-20261020-120000",
        ];
        assert_eq!(snapshots_to_prune(&names, 2), vec!["auto-20261021-010000"]);
        assert_eq!(snapshots_to_prune(&names, 3), Vec::<&str>::new());
        assert_eq!(
            snapshots_to_prune(&names, 1),
            vec!["auto-20261021-010000", "auto-20261021-020000"]
        );
    }

    #[test]
    fn names_carry_the_time_and_stay_valid() {
        let at = time::macros::datetime!(2026-10-21 09:05:03 UTC);
        assert_eq!(stamped(AUTO_SNAPSHOT_PREFIX, at), "auto-20261021-090503");
        assert_eq!(
            backup_name("shop-db-01-data", at),
            "shop-db-01-data-bk-20261021090503"
        );
        let long = format!("{}-data", "a".repeat(55));
        let b = backup_name(&long, at);
        assert!(crate::is_valid_name(&b) && b.len() <= 63, "{b}");
        assert!(b.ends_with("-bk-20261021090503"));
    }
}
