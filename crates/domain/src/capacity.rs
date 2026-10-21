// SPDX-License-Identifier: AGPL-3.0-only
//! Health and capacity rules (design: `docs/design/health-and-capacity.md`). Pure: the
//! control plane reads Incus and PostgreSQL, these functions decide what the readings mean,
//! so the Console never infers a state on its own.

use serde::{Deserialize, Serialize};

/// A component's or server's state, worst last.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum HealthCondition {
    Healthy,
    /// Deliberately absent (for example no database in development): not a fault.
    NotSetUp,
    /// Could not be observed; never shown as healthy.
    Unknown,
    Degraded,
    Down,
}

impl HealthCondition {
    /// The worst of the states (`NotSetUp` counts as healthy for the overall state).
    pub fn worst(states: impl IntoIterator<Item = HealthCondition>) -> HealthCondition {
        states
            .into_iter()
            .map(|s| {
                if s == HealthCondition::NotSetUp {
                    HealthCondition::Healthy
                } else {
                    s
                }
            })
            .max()
            .unwrap_or(HealthCondition::Unknown)
    }
}

/// Memory use above this share of the total is a warning.
pub const MEMORY_WARN: f64 = 0.90;
/// A storage pool fuller than this is a warning.
pub const DISK_WARN: f64 = 0.85;
/// A worker that has not written a heartbeat for this long is not running.
pub const WORKER_STALE_SECS: i64 = 45;
/// A worker lost within this window (not stopped cleanly) makes the workers degraded.
pub const WORKER_RECENT_LOSS_SECS: i64 = 600;
/// A job that has waited this long for a worker makes the queue degraded.
pub const QUEUE_WAIT_WARN_SECS: i64 = 120;
/// A database round trip slower than this is degraded.
pub const DATABASE_SLOW_MS: f64 = 500.0;

/// Used of total for one resource, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct HealthUsage {
    pub used_bytes: u64,
    pub total_bytes: u64,
}

impl HealthUsage {
    /// Used share of the total; `None` when the total is unknown (zero).
    pub fn share(self) -> Option<f64> {
        (self.total_bytes > 0).then(|| self.used_bytes as f64 / self.total_bytes as f64)
    }

    pub fn free_bytes(self) -> u64 {
        self.total_bytes.saturating_sub(self.used_bytes)
    }
}

/// One storage pool's space on one server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct HealthPoolUsage {
    pub name: String,
    pub used_bytes: u64,
    pub total_bytes: u64,
}

/// What Incus reports about one server's resources. Fields Incus did not report are `None`
/// (or empty), never zero.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct HealthServerResources {
    /// CPU threads (all sockets and cores).
    pub cpu_threads: Option<u64>,
    /// 1, 5 and 15-minute load averages (clustered servers only).
    pub load_averages: Vec<f64>,
    pub memory: Option<HealthUsage>,
    pub pools: Vec<HealthPoolUsage>,
    pub uptime_secs: Option<u64>,
}

fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / 1024f64.powi(3))
}

fn percent(share: f64) -> String {
    format!("{:.0} %", share * 100.0)
}

/// A server's state and warnings from its Incus status and resources. `resources` is `Err`
/// with the reason when they could not be read.
pub fn server_state(
    status: &str,
    resources: Result<&HealthServerResources, &str>,
) -> (HealthCondition, Vec<String>) {
    let mut warnings = Vec::new();
    let mut state = HealthCondition::Healthy;
    match status {
        "Online" => {}
        "Evacuated" => {
            warnings.push("Evacuated: it runs no workloads until it is restored".into());
            state = HealthCondition::Degraded;
        }
        "Offline" => {
            warnings
                .push("Offline: the cluster cannot reach it; its instances are not running".into());
            state = HealthCondition::Down;
        }
        other => {
            warnings.push(format!("Incus reports it as {other}"));
            state = HealthCondition::Degraded;
        }
    }
    match resources {
        Err(reason) => {
            warnings.push(format!("Resources unavailable: {reason}"));
            state = state.max(HealthCondition::Unknown);
        }
        Ok(r) => {
            if let Some(m) = r.memory
                && let Some(s) = m.share()
                && s > MEMORY_WARN
            {
                warnings.push(format!(
                    "Memory {} used ({} of {})",
                    percent(s),
                    gib(m.used_bytes),
                    gib(m.total_bytes)
                ));
                state = state.max(HealthCondition::Degraded);
            }
            for p in &r.pools {
                let u = HealthUsage {
                    used_bytes: p.used_bytes,
                    total_bytes: p.total_bytes,
                };
                if let Some(s) = u.share()
                    && s > DISK_WARN
                {
                    warnings.push(format!(
                        "Storage pool {} {} full ({} free)",
                        p.name,
                        percent(s),
                        gib(u.free_bytes())
                    ));
                    state = state.max(HealthCondition::Degraded);
                }
            }
            if let (Some(threads), Some(load)) = (r.cpu_threads, r.load_averages.first())
                && threads > 0
                && *load > threads as f64
            {
                warnings.push(format!("CPU busy: load {load:.1} on {threads} threads"));
                state = state.max(HealthCondition::Degraded);
            }
        }
    }
    (state, warnings)
}

/// A worker's state from the age of its last heartbeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "kebab-case")]
pub enum HealthWorkerState {
    Running,
    /// Stopped cleanly (it said so before exiting).
    Stopped,
    /// Its heartbeats stopped without a clean shutdown.
    Lost,
}

pub fn worker_state(seconds_since_seen: i64, stopped: bool) -> HealthWorkerState {
    if stopped {
        HealthWorkerState::Stopped
    } else if seconds_since_seen <= WORKER_STALE_SECS {
        HealthWorkerState::Running
    } else {
        HealthWorkerState::Lost
    }
}

/// The workers' combined state and, when not healthy, the reason. Input: (state, seconds
/// since last seen) per worker seen recently.
pub fn workers_state(workers: &[(HealthWorkerState, i64)]) -> (HealthCondition, Option<String>) {
    let running = workers
        .iter()
        .filter(|(s, _)| *s == HealthWorkerState::Running)
        .count();
    let recently_lost = workers
        .iter()
        .filter(|(s, age)| *s == HealthWorkerState::Lost && *age <= WORKER_RECENT_LOSS_SECS)
        .count();
    if running == 0 {
        return (
            HealthCondition::Down,
            Some("No deployment worker is running: deployments, moves and recovery wait until one starts".into()),
        );
    }
    if recently_lost > 0 {
        return (
            HealthCondition::Degraded,
            Some(format!(
                "{recently_lost} worker{} stopped sending heartbeats in the last 10 minutes without shutting down",
                if recently_lost == 1 { "" } else { "s" }
            )),
        );
    }
    (HealthCondition::Healthy, None)
}

/// Counts of the job queue.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "utoipa", derive(utoipa::ToSchema))]
#[serde(rename_all = "camelCase")]
pub struct HealthQueueCounts {
    /// Ready to run, waiting for a worker.
    pub waiting: i64,
    /// Waiting for a retry or a scheduled time.
    pub retrying: i64,
    pub running: i64,
    /// Running, but the worker's lease expired (the worker died or hangs); claimed again soon.
    pub stalled: i64,
    pub failed_last_hour: i64,
    /// Age of the oldest job ready to run.
    pub oldest_waiting_secs: Option<i64>,
}

pub fn queue_state(
    q: &HealthQueueCounts,
    workers_running: bool,
) -> (HealthCondition, Option<String>) {
    if q.waiting > 0 && !workers_running {
        return (
            HealthCondition::Down,
            Some(format!(
                "{} job{} wait and no worker is running",
                q.waiting,
                if q.waiting == 1 { "" } else { "s" }
            )),
        );
    }
    let mut reasons = Vec::new();
    if let Some(age) = q.oldest_waiting_secs
        && age > QUEUE_WAIT_WARN_SECS
    {
        reasons.push(format!(
            "the oldest job has waited {} for a worker",
            duration(age)
        ));
    }
    if q.stalled > 0 {
        reasons.push(format!(
            "{} running job{} lost {} worker",
            q.stalled,
            if q.stalled == 1 { "" } else { "s" },
            if q.stalled == 1 { "its" } else { "their" }
        ));
    }
    if q.failed_last_hour > 0 {
        reasons.push(format!(
            "{} job{} failed in the last hour",
            q.failed_last_hour,
            if q.failed_last_hour == 1 { "" } else { "s" }
        ));
    }
    if reasons.is_empty() {
        (HealthCondition::Healthy, None)
    } else {
        (HealthCondition::Degraded, Some(reasons.join("; ")))
    }
}

/// A short human duration: `45 s`, `3 min`, `2 h 5 min`, `3 d 4 h`.
pub fn duration(secs: i64) -> String {
    let s = secs.max(0);
    match s {
        0..60 => format!("{s} s"),
        60..3600 => format!("{} min", s / 60),
        3600..86_400 => match (s % 3600) / 60 {
            0 => format!("{} h", s / 3600),
            m => format!("{} h {m} min", s / 3600),
        },
        _ => match (s % 86_400) / 3600 {
            0 => format!("{} d", s / 86_400),
            h => format!("{} d {h} h", s / 86_400),
        },
    }
}

/// Network and storage-pool state: created everywhere it should be.
pub fn inventory_state(problems: &[String]) -> HealthCondition {
    if problems.is_empty() {
        HealthCondition::Healthy
    } else {
        HealthCondition::Degraded
    }
}

/// Storage drivers whose pool is one pool that every member uses (its space must be counted
/// once, not per member). Local drivers (dir, btrfs, zfs, lvm) are per member.
pub fn pool_is_shared(driver: &str) -> bool {
    matches!(driver, "ceph" | "cephfs" | "cephobject" | "lvmcluster")
}

/// Members a cluster-wide object should be on but is not. OVN networks list only the members
/// present when they were created, so they are never reported here.
pub fn missing_on(locations: &[String], members: &[String], kind: &str) -> Vec<String> {
    if locations.is_empty() || kind == "ovn" {
        return Vec::new();
    }
    members
        .iter()
        .filter(|m| !locations.contains(m))
        .cloned()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(mem: (u64, u64), pool: (u64, u64), load: f64) -> HealthServerResources {
        HealthServerResources {
            cpu_threads: Some(4),
            load_averages: vec![load, 0.0, 0.0],
            memory: Some(HealthUsage {
                used_bytes: mem.0,
                total_bytes: mem.1,
            }),
            pools: vec![HealthPoolUsage {
                name: "local".into(),
                used_bytes: pool.0,
                total_bytes: pool.1,
            }],
            uptime_secs: Some(10),
        }
    }

    #[test]
    fn a_quiet_online_server_is_healthy() {
        let (s, w) = server_state("Online", Ok(&res((50, 100), (10, 100), 0.5)));
        assert_eq!((s, w.len()), (HealthCondition::Healthy, 0));
    }

    #[test]
    fn thresholds_are_strictly_above() {
        // Exactly 90 % memory and 85 % disk are still fine.
        let (s, _) = server_state("Online", Ok(&res((90, 100), (85, 100), 4.0)));
        assert_eq!(s, HealthCondition::Healthy);
        let (s, w) = server_state("Online", Ok(&res((91, 100), (86, 100), 4.1)));
        assert_eq!(s, HealthCondition::Degraded);
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(w[0].starts_with("Memory 91 % used"));
        assert!(w[1].starts_with("Storage pool local 86 % full"));
        assert!(w[2].starts_with("CPU busy: load 4.1 on 4 threads"));
    }

    #[test]
    fn unknown_totals_never_warn() {
        let (s, w) = server_state("Online", Ok(&res((5, 0), (5, 0), 0.0)));
        assert_eq!((s, w.len()), (HealthCondition::Healthy, 0));
    }

    #[test]
    fn status_and_unreadable_resources() {
        assert_eq!(
            server_state("Offline", Err("timeout")).0,
            HealthCondition::Down
        );
        let (s, w) = server_state("Online", Err("timeout"));
        assert_eq!(s, HealthCondition::Unknown);
        assert_eq!(w, ["Resources unavailable: timeout"]);
        assert_eq!(
            server_state("Evacuated", Ok(&HealthServerResources::default())).0,
            HealthCondition::Degraded
        );
        assert_eq!(
            server_state("Blocked", Ok(&HealthServerResources::default())).0,
            HealthCondition::Degraded
        );
    }

    #[test]
    fn workers() {
        assert_eq!(worker_state(45, false), HealthWorkerState::Running);
        assert_eq!(worker_state(46, false), HealthWorkerState::Lost);
        assert_eq!(worker_state(1, true), HealthWorkerState::Stopped);
        assert_eq!(workers_state(&[]).0, HealthCondition::Down);
        assert_eq!(
            workers_state(&[(HealthWorkerState::Stopped, 5)]).0,
            HealthCondition::Down
        );
        assert_eq!(
            workers_state(&[(HealthWorkerState::Running, 5)]),
            (HealthCondition::Healthy, None)
        );
        assert_eq!(
            workers_state(&[
                (HealthWorkerState::Running, 5),
                (HealthWorkerState::Lost, 300)
            ])
            .0,
            HealthCondition::Degraded
        );
        // A worker lost long ago is history, not a current problem.
        assert_eq!(
            workers_state(&[
                (HealthWorkerState::Running, 5),
                (HealthWorkerState::Lost, 601)
            ])
            .0,
            HealthCondition::Healthy
        );
    }

    #[test]
    fn queue() {
        let q = HealthQueueCounts {
            waiting: 2,
            ..Default::default()
        };
        assert_eq!(queue_state(&q, false).0, HealthCondition::Down);
        assert_eq!(queue_state(&q, true).0, HealthCondition::Healthy);
        let old = HealthQueueCounts {
            waiting: 1,
            oldest_waiting_secs: Some(121),
            stalled: 1,
            failed_last_hour: 2,
            ..Default::default()
        };
        let (s, r) = queue_state(&old, true);
        assert_eq!(s, HealthCondition::Degraded);
        assert_eq!(
            r.as_deref(),
            Some(
                "the oldest job has waited 2 min for a worker; 1 running job lost its worker; 2 jobs failed in the last hour"
            )
        );
        // Nothing waiting and no worker: nothing is blocked.
        assert_eq!(
            queue_state(&HealthQueueCounts::default(), false).0,
            HealthCondition::Healthy
        );
    }

    #[test]
    fn durations_and_overall() {
        assert_eq!(duration(59), "59 s");
        assert_eq!(duration(60), "1 min");
        assert_eq!(duration(3600), "1 h");
        assert_eq!(duration(3_900), "1 h 5 min");
        assert_eq!(duration(90_000), "1 d 1 h");
        assert_eq!(duration(-3), "0 s");
        assert_eq!(
            HealthCondition::worst([HealthCondition::Healthy, HealthCondition::NotSetUp]),
            HealthCondition::Healthy
        );
        assert_eq!(
            HealthCondition::worst([HealthCondition::Unknown, HealthCondition::Degraded]),
            HealthCondition::Degraded
        );
        assert_eq!(HealthCondition::worst([]), HealthCondition::Unknown);
    }

    #[test]
    fn shared_pools_are_counted_once() {
        assert!(pool_is_shared("lvmcluster") && pool_is_shared("ceph"));
        assert!(!pool_is_shared("btrfs") && !pool_is_shared("dir") && !pool_is_shared("zfs"));
    }

    #[test]
    fn missing_members() {
        let members = vec!["a".to_owned(), "b".to_owned()];
        assert_eq!(missing_on(&["a".into()], &members, "bridge"), ["b"]);
        assert!(missing_on(&["a".into()], &members, "ovn").is_empty());
        assert!(missing_on(&[], &members, "bridge").is_empty());
        assert_eq!(inventory_state(&[]), HealthCondition::Healthy);
        assert_eq!(inventory_state(&["x".into()]), HealthCondition::Degraded);
    }
}
