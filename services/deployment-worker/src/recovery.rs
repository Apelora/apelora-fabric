// SPDX-License-Identifier: AGPL-3.0-only
//! HA slice B (ADR 0013): restarting a failed member's instances on shared storage on another
//! member, only after the failed member is fenced.
//!
//! [`Recovery::pass`] runs inside every supervision pass of the `replica-supervisor` leader:
//! it opens a `member_failures` row for each member Incus reports Offline (and closes it when
//! the member is back), reads the shared lockspace through the fence probe on the healthy
//! members (learning each Online member's own lease on the way), decides whether each failed
//! member is fenced ([`apelora_domain::fencing::decide`]), records the state, and — once fenced
//! — records the one recovery decision and queues one `instance.recover` job per instance
//! whose disks are all on a shared pool. Every write is fenced by the leader lease; every job
//! has an idempotency key bound to the failure, so a pass repeated by any leader queues nothing
//! twice.
//!
//! [`run`] is the `instance.recover` job: it re-reads Incus at each step, relinks the instance
//! to another member (`POST /1.0/instances/<name>?target=` with `migration: true`, which Incus
//! accepts for an offline member on a shared pool) and starts it there.

use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use apelora_domain::evacuation::Deployed;
use apelora_domain::fencing::{self, FenceInput, FenceObservation, FenceView, KnownHost};
use apelora_domain::plan::DesiredModule;
use apelora_domain::recovery::{RecoveryAction, choose_target, plan_recovery};
use apelora_domain::{ClusterMember, InstanceAction};
use apelora_incus_adapter::{OperationOutcome, Provider};
use apelora_persistence::recovery as store;
use apelora_persistence::{
    ClaimedJob, Enqueued, Leadership, Outcome, Pool, StoreError, begin_fenced, record_operation,
};
use serde::Deserialize;
use serde_json::{Value, json};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::fencing::Probe;

/// Readings older than this do not fence anything.
pub const READING_MAX_AGE_SECS: i64 = 60;
/// An Online member's own lease is read again after this long (and at once while a member is
/// failed, when every reading counts).
const LEARN_EVERY_SECS: i64 = 60;
/// After fencing, how long a start refused by the storage lock ("locked by other host") is
/// waited for: the other members' sanlock see the failed host dead within one renewal period.
const LOCK_WAIT_SECS: i64 = 120;
/// How long one Incus operation of the job may take.
const BUDGET: Duration = Duration::from_secs(300);

fn now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

/// The supervisor's memory for failed members: the latest lease readings.
#[derive(Debug, Default)]
pub struct Recovery {
    probe: Probe,
    lockspace: Option<String>,
    readings: HashMap<String, FenceObservation>,
    /// When an Online member's own reading was last taken.
    read_at: HashMap<String, i64>,
    /// Members whose probe failed on the previous read (logged once per change).
    failing: HashMap<String, String>,
}

/// The deployment records, parsed (unreadable ones are left out, as elsewhere).
type Parsed = (String, String, Vec<DesiredModule>, Uuid);

fn deployments(records: &[apelora_persistence::DeploymentRecord]) -> Vec<Parsed> {
    records
        .iter()
        .filter_map(|r| {
            serde_json::from_value(r.desired.clone()).ok().map(|d| {
                (
                    r.application.clone(),
                    r.project.clone(),
                    d,
                    r.application_id,
                )
            })
        })
        .collect()
}

fn deployed(parsed: &[Parsed]) -> Vec<Deployed<'_>> {
    parsed
        .iter()
        .map(|(application, project, desired, _)| Deployed {
            application,
            project,
            desired,
        })
        .collect()
}

fn encode<T: serde::Serialize>(v: &T) -> Result<Value, StoreError> {
    serde_json::to_value(v).map_err(|e| StoreError::Conflict(e.to_string()))
}

impl Recovery {
    pub fn new(probe: Probe, lockspace: Option<String>) -> Self {
        Recovery {
            probe,
            lockspace,
            ..Default::default()
        }
    }

    /// Forgets readings (a new leadership starts over).
    pub fn forget(&mut self) {
        self.readings.clear();
        self.read_at.clear();
        self.failing.clear();
    }

    /// Reads the lockspace on the Online members that are due (all of them while a member is
    /// failed) and learns each one's own lease.
    async fn read_leases(
        &mut self,
        pool: &Pool,
        lease: Option<&Leadership>,
        members: &[ClusterMember],
        any_failed: bool,
    ) -> Result<(), StoreError> {
        if !self.probe.configured() {
            return Ok(());
        }
        let t = now();
        let mut set = tokio::task::JoinSet::new();
        for m in members.iter().filter(|m| m.status == "Online") {
            let due = any_failed
                || self
                    .read_at
                    .get(&m.name)
                    .is_none_or(|at| t - at >= LEARN_EVERY_SECS);
            if !due {
                continue;
            }
            let (probe, lockspace, m) = (self.probe.clone(), self.lockspace.clone(), m.clone());
            set.spawn(async move {
                let r = probe.observe(&m, lockspace.as_deref(), now()).await;
                (m.name, r)
            });
        }
        let known: BTreeMap<String, store::LockHostRecord> = store::lock_hosts(pool)
            .await?
            .into_iter()
            .map(|h| (h.member.clone(), h))
            .collect();
        while let Some(done) = set.join_next().await {
            let Ok((member, r)) = done else { continue };
            self.read_at.insert(member.clone(), t);
            match r {
                Ok(o) => {
                    apelora_metrics::FENCE_PROBES.inc(&["ok"]);
                    if self.failing.remove(&member).is_some() {
                        tracing::info!(%member, "recovery: the fence probe answers again");
                    }
                    if let Some(k) = fencing::learn(&member, &o) {
                        let same = known.get(&member).is_some_and(|h| {
                            h.lockspace == k.lockspace
                                && h.host_id == k.host_id as i32
                                && h.generation == k.generation as i64
                        });
                        if !same {
                            let mut tx = begin_fenced(pool, lease).await?;
                            store::learn_lock_host(
                                &mut tx,
                                &member,
                                &k.lockspace,
                                k.host_id as i32,
                                k.generation as i64,
                            )
                            .await?;
                            tx.commit().await?;
                            tracing::info!(%member, lockspace = %k.lockspace, host_id = k.host_id, generation = k.generation, "recovery: learned the member's storage lease");
                        }
                    }
                    self.readings.insert(member, o);
                }
                Err(e) => {
                    apelora_metrics::FENCE_PROBES.inc(&["failed"]);
                    if self.failing.get(&member) != Some(&e) {
                        tracing::warn!(%member, error = %e, "recovery: the fence probe failed");
                        self.failing.insert(member.clone(), e);
                    }
                    self.readings.remove(&member);
                }
            }
        }
        Ok(())
    }

    /// One pass; returns the instances it queued for recovery.
    pub async fn pass(
        &mut self,
        pool: &Pool,
        provider: &Provider,
        lease: Option<&Leadership>,
    ) -> Result<Vec<String>, StoreError> {
        let members = match provider.cluster_members().await {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(error = %e, "recovery: cluster members not readable; skipping");
                return Ok(Vec::new());
            }
        };
        let open = store::open_failures(pool).await?;
        // Back (or no longer listed): the failure is over.
        for f in &open {
            let status = members
                .iter()
                .find(|m| m.name == f.member)
                .map(|m| m.status.as_str());
            if status != Some("Offline") {
                let mut tx = begin_fenced(pool, lease).await?;
                store::resolve_failure(&mut tx, f.id).await?;
                apelora_persistence::audit(&mut *tx, "supervisor", "cluster.member.failure.resolved", &f.member,
                    &json!({ "failure": f.id, "status": status, "fenced": f.fenced_at.is_some(), "decided": f.decided_at.is_some() })).await?;
                tx.commit().await?;
                tracing::info!(member = %f.member, failure = %f.id, status = ?status, "recovery: the member is no longer offline; failure closed");
            }
        }
        let offline: Vec<&ClusterMember> =
            members.iter().filter(|m| m.status == "Offline").collect();
        self.read_leases(pool, lease, &members, !offline.is_empty())
            .await?;
        if offline.is_empty() {
            apelora_metrics::MEMBER_FAILURES.replace([]);
            return Ok(Vec::new());
        }
        let instances = match provider.instances().await {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!(error = %e, "recovery: cannot read instances; nothing is decided this pass");
                return Ok(Vec::new());
            }
        };
        let pools = match provider.storage_pools().await {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "recovery: cannot read storage pools; nothing is decided this pass");
                return Ok(Vec::new());
            }
        };
        let parsed = deployments(&apelora_persistence::list_deployments(pool).await?);
        let deployed = deployed(&parsed);
        let known: BTreeMap<String, KnownHost> = store::lock_hosts(pool)
            .await?
            .into_iter()
            .map(|h| {
                (
                    h.member.clone(),
                    KnownHost {
                        member: h.member,
                        lockspace: h.lockspace,
                        host_id: h.host_id.max(0) as u32,
                        generation: h.generation.max(0) as u64,
                    },
                )
            })
            .collect();
        let readings: Vec<FenceObservation> = self.readings.values().cloned().collect();
        let (mut waiting_members, mut fenced_members) = (0.0, 0.0);
        let mut queued = Vec::new();
        for m in offline {
            let heartbeat = fencing::last_heartbeat(&m.message)
                .and_then(|t| OffsetDateTime::from_unix_timestamp(t).ok());
            let mut tx = begin_fenced(pool, lease).await?;
            let (row, created) = store::open_failure(&mut tx, &m.name, heartbeat).await?;
            if created {
                apelora_persistence::audit(&mut *tx, "supervisor", "cluster.member.failed", &m.name,
                    &json!({ "failure": row.id, "message": m.message, "lastHeartbeat": heartbeat.map(|h| h.unix_timestamp()) })).await?;
                tracing::warn!(member = %m.name, failure = %row.id, message = %m.message, "recovery: member failed; nothing on it restarts elsewhere before it is fenced");
            }
            tx.commit().await?;
            let confirmed = row
                .confirmed_by
                .as_deref()
                .zip(row.confirmed_at.map(|t| t.unix_timestamp()));
            let view: FenceView = fencing::decide(&FenceInput {
                member: &m.name,
                offline: true,
                known: known.get(&m.name),
                observations: &readings,
                confirmed,
                probe: self.probe.configured(),
                now: now(),
                max_age_secs: READING_MAX_AGE_SECS,
            });
            let fenced = row.fenced_at.is_some() || view.fenced();
            let plan = plan_recovery(m, fenced, &members, &instances, &deployed, &pools);
            let waiting = plan
                .iter()
                .filter(|i| i.action == RecoveryAction::AfterFencing)
                .count();
            let mut tx = begin_fenced(pool, lease).await?;
            // Once fenced, the recorded state stays fenced (the evidence was taken while it held).
            let shown = if row.fenced_at.is_some() && !view.fenced() {
                row.fencing.clone().unwrap_or(encode(&view)?)
            } else {
                encode(&view)?
            };
            store::set_fencing(
                &mut tx,
                row.id,
                &shown,
                i32::try_from(waiting).unwrap_or(i32::MAX),
            )
            .await?;
            if row.fenced_at.is_none()
                && view.fenced()
                && let Some(e) = &view.evidence
                && store::mark_fenced(&mut tx, row.id, &encode(e)?).await?
            {
                apelora_persistence::audit(&mut *tx, "supervisor", "cluster.member.fenced", &m.name,
                    &json!({ "failure": row.id, "evidence": e, "secondsSinceDetected": now() - row.detected_at.unix_timestamp() })).await?;
                tracing::warn!(member = %m.name, failure = %row.id, kind = ?e.kind, evidence = %e.summary, "recovery: member fenced");
            }
            if fenced {
                if row.decided_at.is_none()
                    && store::record_decision(&mut tx, row.id, &encode(&plan)?).await?
                {
                    apelora_persistence::audit(
                        &mut *tx,
                        "supervisor",
                        "cluster.member.recovery.decided",
                        &m.name,
                        &json!({ "failure": row.id, "items": plan }),
                    )
                    .await?;
                    tracing::info!(member = %m.name, failure = %row.id, restart = plan.iter().filter(|i| i.action == RecoveryAction::Restart).count(), wait = plan.iter().filter(|i| i.action == RecoveryAction::Wait).count(), "recovery: decision recorded");
                }
                for item in plan.iter().filter(|i| i.action == RecoveryAction::Restart) {
                    let (Some(application), Some(module)) = (&item.application, &item.module)
                    else {
                        continue;
                    };
                    let application_id = parsed
                        .iter()
                        .find(|(a, p, _, _)| a == application && *p == item.project)
                        .map(|(_, _, _, id)| *id);
                    let payload = json!({ "project": item.project, "name": item.instance, "application": application,
                        "module": module, "failedMember": m.name, "failure": row.id });
                    let key = format!("recover:{}:{}/{}", row.id, item.project, item.instance);
                    let enq = apelora_persistence::enqueue_job_in(
                        &mut tx,
                        "instance.recover",
                        application_id,
                        &payload,
                        Some(&key),
                        None,
                    )
                    .await?;
                    if let Enqueued::Created(job) = enq {
                        apelora_persistence::audit(&mut *tx, "supervisor", "application.instance.recover",
                            &format!("{}/{}", item.project, item.instance),
                            &json!({ "application": application, "module": module, "failure": row.id, "from": m.name, "plannedTarget": item.target, "jobId": job })).await?;
                        apelora_metrics::RECOVERIES.inc(&["queued"]);
                        tracing::info!(member = %m.name, instance = %format!("{}/{}", item.project, item.instance), %job, "recovery: restart on another member queued");
                        queued.push(item.instance.clone());
                    }
                }
                fenced_members += 1.0;
            } else {
                waiting_members += 1.0;
                tracing::debug!(member = %m.name, reason = %view.reason, waiting, "recovery: waiting for fencing");
            }
            tx.commit().await?;
        }
        apelora_metrics::MEMBER_FAILURES.replace([
            (vec!["waiting"], waiting_members),
            (vec!["fenced"], fenced_members),
        ]);
        Ok(queued)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecoverRequest {
    pub project: String,
    pub name: String,
    pub application: String,
    pub module: String,
    pub failed_member: String,
    pub failure: Uuid,
}

/// Records `op` with the job, waits on it, and says how it ended.
async fn wait(
    pool: &Pool,
    provider: &Provider,
    job: &ClaimedJob,
    worker: &str,
    op: &str,
    what: &str,
) -> Result<Result<(), String>, StoreError> {
    record_operation(pool, job, worker, Some(op)).await?;
    tracing::info!(operation = %op, step = %what, "recovery: waiting on the Incus operation");
    let outcome = provider.wait_operation(op, BUDGET).await;
    record_operation(pool, job, worker, None).await?;
    Ok(match outcome {
        Ok(OperationOutcome::Succeeded) => Ok(()),
        Ok(OperationOutcome::Failed(e)) => Err(format!("{what}: {e}")),
        Ok(OperationOutcome::StillRunning) => Err(format!("{what} is still running")),
        Err(e) => Err(format!("{what}: {e}")),
    })
}

/// The `instance.recover` job.
pub async fn run(
    pool: &Pool,
    provider: &Provider,
    job: &ClaimedJob,
    worker: &str,
) -> Result<Outcome, StoreError> {
    let r: RecoverRequest = match serde_json::from_value(job.payload.clone()) {
        Ok(r) => r,
        Err(e) => return Ok(Outcome::Rejected(format!("invalid payload: {e}"))),
    };
    let id = format!("{}/{}", r.project, r.name);
    // An operation an earlier attempt started: let it finish before deciding again.
    if let Some(op) = &job.external_operation {
        let _ = provider.wait_operation(op, BUDGET).await;
    }
    for _ in 0..6 {
        let instances = match provider.instances().await {
            Ok(i) => i,
            Err(e) => return Ok(Outcome::Failed(e.to_string())),
        };
        let members = match provider.cluster_members().await {
            Ok(m) => m,
            Err(e) => return Ok(Outcome::Failed(e.to_string())),
        };
        let Some(inst) = instances
            .iter()
            .find(|i| i.project == r.project && i.name == r.name)
        else {
            apelora_metrics::RECOVERIES.inc(&["failed"]);
            return Ok(Outcome::Rejected(format!("instance {id} does not exist")));
        };
        if inst.location == r.failed_member {
            let failure = store::failure(pool, r.failure).await?;
            let failed = members.iter().find(|m| m.name == r.failed_member);
            let ok = failure
                .as_ref()
                .is_some_and(|f| f.resolved_at.is_none() && f.fenced_at.is_some())
                && failed.is_some_and(|m| m.status == "Offline");
            if !ok {
                apelora_metrics::RECOVERIES.inc(&["failed"]);
                return Ok(Outcome::Rejected(format!(
                    "{} is no longer a fenced, offline member (it came back, or the failure was closed): {} stays there; nothing was changed",
                    r.failed_member, r.name
                )));
            }
            // Safe point: nothing has changed yet.
            if let Some(by) = apelora_persistence::jobs::cancel_requested(pool, job.id).await? {
                return Ok(Outcome::Cancelled(format!(
                    "cancelled by {by}; {} stays on {}",
                    r.name, r.failed_member
                )));
            }
            let Some(failed) = failed else {
                unreachable!("checked above")
            };
            let Some(target) =
                choose_target(failed, &members, &instances, &r.application, &r.module)
            else {
                return Ok(Outcome::Failed(format!(
                    "no online member with architecture {} can take {}",
                    failed.architecture, r.name
                )));
            };
            tracing::info!(instance = %id, from = %r.failed_member, to = %target, failure = %r.failure, "recovery: relinking to another member");
            let op = match provider
                .move_instance(&r.project, &r.name, &target, false)
                .await
            {
                Ok(op) => op,
                Err(e) => {
                    return Ok(Outcome::Failed(format!(
                        "moving {} to {target}: {e}",
                        r.name
                    )));
                }
            };
            if let Err(e) = wait(
                pool,
                provider,
                job,
                worker,
                &op,
                &format!("moving {} to {target}", r.name),
            )
            .await?
            {
                return Ok(Outcome::Failed(e));
            }
            continue;
        }
        if !members
            .iter()
            .any(|m| m.name == inst.location && m.status == "Online")
        {
            return Ok(Outcome::Failed(format!(
                "{} is on {}, which is not Online",
                r.name, inst.location
            )));
        }
        let Some(now) = (match provider.instance(&r.project, &r.name).await {
            Ok(s) => s,
            Err(e) => return Ok(Outcome::Failed(e.to_string())),
        }) else {
            continue;
        };
        if !now.managed {
            apelora_metrics::RECOVERIES.inc(&["failed"]);
            return Ok(Outcome::Rejected(format!(
                "{} was not created by Apelora",
                r.name
            )));
        }
        if now.status != "Running" {
            let what = format!("starting {} on {}", r.name, now.location);
            let started = match provider
                .change_state(&r.project, &r.name, InstanceAction::Start, false)
                .await
            {
                Ok(op) => wait(pool, provider, job, worker, &op, &what).await?,
                Err(e) => Err(format!("{what}: {e}")),
            };
            if let Err(e) = started {
                // Each member's sanlock judges the failed host dead from its own last reading
                // of that host's lease, so the member that starts the instance may still see it
                // alive a few seconds after the member that fenced it (REAL INCUS TESTED: 7 s).
                // The lock layer refuses until then: wait without using up an attempt.
                if e.contains("locked by other host") {
                    let fenced_for = store::failure(pool, r.failure)
                        .await?
                        .and_then(|f| f.fenced_at)
                        .map(|t| (OffsetDateTime::now_utc() - t).whole_seconds());
                    if fenced_for.is_some_and(|s| s <= LOCK_WAIT_SECS) {
                        tracing::info!(instance = %id, member = %now.location, ?fenced_for, "recovery: the shared storage lock is still held for the failed member as this member sees it; waiting");
                        return Ok(Outcome::Waiting(5));
                    }
                }
                return Ok(Outcome::Failed(e));
            }
            continue;
        }
        let failure = store::failure(pool, r.failure).await?;
        let since = failure
            .as_ref()
            .map(|f| (OffsetDateTime::now_utc() - f.detected_at).whole_seconds());
        let evidence = failure.and_then(|f| f.fence_evidence);
        apelora_persistence::audit(pool, "worker", "application.instance.recovered", &id,
            &json!({ "application": r.application, "module": r.module, "failure": r.failure, "from": r.failed_member,
                     "to": now.location, "secondsSinceDetected": since, "evidence": evidence, "jobId": job.id })).await?;
        apelora_metrics::RECOVERIES.inc(&["succeeded"]);
        tracing::info!(instance = %id, from = %r.failed_member, to = %now.location, seconds_since_detected = ?since, "recovery: running on another member");
        return Ok(Outcome::Succeeded);
    }
    Ok(Outcome::Failed(format!(
        "recovering {} did not finish in 6 steps",
        r.name
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::supervisor::Supervisor;
    use apelora_domain::StoragePool;
    use apelora_incus_adapter::MockIncus;
    use apelora_persistence::{enqueue_job, get_job, recent_audit, recent_jobs};
    use std::sync::{Arc, Mutex};

    const PROJECT: &str = "apelora";

    type Readings = Arc<Mutex<BTreeMap<String, Result<String, String>>>>;

    /// What each member's sanlock prints: its own host id (member n = host n) and every host's
    /// state, generation 1.
    fn set_readings(r: &Readings, states: [&str; 3]) {
        let hosts: String = states
            .iter()
            .enumerate()
            .map(|(i, s)| format!("h {} gen 1 timestamp 100 {s}\n", i + 1))
            .collect();
        let mut map = r.lock().expect("lock");
        for n in 1..=3 {
            map.insert(
                format!("mock-member-0{n}"),
                Ok(format!("s lvm_apelora-shared:{n}:/dev/mapper/x:0\n{hosts}")),
            );
        }
    }

    fn set_member(mock: &MockIncus, name: &str, status: &str) {
        let mut members = mock.members.lock().unwrap_or_else(|e| e.into_inner());
        let m = members.iter_mut().find(|m| m.name == name).expect("member");
        m.status = status.into();
        m.message = if status == "Offline" {
            "No heartbeat for 30s (2026-10-21 14:02:03 +0000)".into()
        } else {
            "Fully operational".into()
        };
    }

    async fn drain(pool: &Pool, provider: &Provider) {
        while crate::run_once(pool, provider, "test").await.expect("run") {}
    }

    /// `shop`: `db` (stateful, everything on the shared pool) and `files` (stateful, data on
    /// the member-local pool), both deployed and recorded; they land on mock-member-01.
    async fn deploy(pool: &Pool, provider: &Provider) {
        let app = apelora_persistence::ensure_application(pool, "shop")
            .await
            .expect("app");
        let desired = json!([
            { "module": "db", "availability": "stateful", "spec": { "project": "", "name": "", "kind": "container", "start": true,
              "source": { "protocol": "simplestreams", "server": "https://images.example", "alias": "debian/13" }, "rootPool": "shared",
              "volumes": [{ "name": "data", "sizeMib": 256, "path": "/data", "class": "data-replicated", "pool": "shared" }] } },
            { "module": "files", "availability": "stateful", "spec": { "project": "", "name": "", "kind": "container", "start": true,
              "source": { "protocol": "simplestreams", "server": "https://images.example", "alias": "debian/13" },
              "volumes": [{ "name": "data", "sizeMib": 256, "path": "/data", "pool": "mock-local" }] } }
        ]);
        let modules: Vec<DesiredModule> = serde_json::from_value(desired.clone()).expect("desired");
        for m in &modules {
            let spec = apelora_domain::plan::deploy_spec("shop", PROJECT, m, 1, None);
            enqueue_job(
                pool,
                "instance.create",
                Some(app.id),
                &serde_json::to_value(&spec).expect("json"),
                None,
            )
            .await
            .expect("enqueue");
        }
        apelora_persistence::record_deployment(
            pool,
            &apelora_persistence::NewDeployment {
                application_id: app.id,
                project: PROJECT,
                revision: 1,
                desired: &desired,
                connections: None,
                uplink: None,
                service_range: None,
                published: None,
            },
        )
        .await
        .expect("record");
        drain(pool, provider).await;
        for j in recent_jobs(pool, 20).await.expect("jobs") {
            let j = get_job(pool, j.id).await.expect("job");
            assert_eq!(j.status, "succeeded", "{} {:?}", j.kind, j.last_error);
        }
    }

    fn mock_with_shared_pool() -> MockIncus {
        let mut mock = MockIncus::default();
        mock.pools.push(StoragePool {
            name: "shared".into(),
            driver: "lvmcluster".into(),
            status: "Created".into(),
            description: "mock data".into(),
            locations: Vec::new(),
        });
        mock
    }

    fn located(provider: &Provider, name: &str) -> (String, String) {
        let Provider::Mock(m) = provider else {
            unreachable!()
        };
        let all = m.instances.lock().expect("lock");
        let i = all.iter().find(|i| i.instance.name == name).expect(name);
        (i.instance.location.clone(), i.instance.status.clone())
    }

    fn reason(f: &store::FailureRecord) -> String {
        f.fencing.as_ref().expect("state")["reason"]
            .as_str()
            .expect("reason")
            .to_owned()
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn shared_storage_restarts_elsewhere_only_after_the_lease_is_dead(pool: Pool) {
        let mock = mock_with_shared_pool();
        let provider = Provider::Mock(mock.clone());
        deploy(&pool, &provider).await;
        assert_eq!(located(&provider, "shop-db").0, "mock-member-01");
        let readings: Readings = Arc::default();
        set_readings(&readings, ["LIVE", "LIVE", "LIVE"]);
        let mut s = Supervisor::new(Duration::from_secs(3600))
            .with_recovery(Recovery::new(Probe::Fixed(readings.clone()), None));
        s.pass(&pool, &provider).await.expect("pass");
        assert_eq!(
            store::lock_hosts(&pool).await.expect("hosts").len(),
            3,
            "each member's own lease learned"
        );

        // The member fails; its lease is still renewed, then failing: nothing moves.
        set_member(&mock, "mock-member-01", "Offline");
        for states in [["LIVE", "LIVE", "LIVE"], ["FAIL", "LIVE", "LIVE"]] {
            set_readings(&readings, states);
            assert_eq!(
                s.pass(&pool, &provider).await.expect("pass"),
                Vec::<String>::new()
            );
        }
        let open = store::open_failures(&pool).await.expect("open");
        assert_eq!(open.len(), 1);
        let f = &open[0];
        assert!(f.fenced_at.is_none());
        assert_eq!(
            f.waiting_instances, 1,
            "db waits for fencing; files waits for its member"
        );
        assert_eq!(f.fencing.as_ref().expect("state")["condition"], "waiting");
        assert!(reason(f).contains("FAIL"), "{}", reason(f));
        assert!(f.last_heartbeat.is_some(), "Incus's last heartbeat is kept");
        assert_eq!(
            located(&provider, "shop-db"),
            ("mock-member-01".into(), "Running".into())
        );

        // The lease is dead: fenced, decided once, one job. The member it restarts on still sees
        // the lock held twice (its own sanlock has not judged the host dead yet): it waits.
        mock.lock_refusals
            .lock()
            .expect("lock")
            .insert("shop-db".into(), 2);
        set_readings(&readings, ["DEAD", "LIVE", "LIVE"]);
        assert_eq!(
            s.pass(&pool, &provider).await.expect("pass"),
            vec!["shop-db".to_owned()]
        );
        assert_eq!(
            s.pass(&pool, &provider).await.expect("pass"),
            Vec::<String>::new(),
            "not twice"
        );
        let f = store::open_failures(&pool).await.expect("open").remove(0);
        assert!(f.fenced_at.is_some() && f.decided_at.is_some());
        assert_eq!(
            f.fence_evidence.as_ref().expect("evidence")["kind"],
            "storage-lease"
        );
        let decision = f.decision.expect("decision");
        let action = |n: &str| {
            decision
                .as_array()
                .expect("items")
                .iter()
                .find(|i| i["instance"] == n)
                .expect(n)["action"]
                .clone()
        };
        assert_eq!(action("shop-db"), "restart");
        assert_eq!(action("shop-files"), "wait");
        // Waiting runs again after 5 s: let it run now.
        for _ in 0..3 {
            drain(&pool, &provider).await;
            sqlx::query("UPDATE jobs SET run_after = now() WHERE status = 'pending'")
                .execute(&pool)
                .await
                .expect("due now");
        }
        drain(&pool, &provider).await;
        let jobs: Vec<_> = recent_jobs(&pool, 20)
            .await
            .expect("jobs")
            .into_iter()
            .filter(|j| j.kind == "instance.recover")
            .collect();
        assert_eq!(jobs.len(), 1);
        let j = get_job(&pool, jobs[0].id).await.expect("job");
        assert_eq!(j.status, "succeeded", "{:?}", j.last_error);
        assert_eq!(
            (j.incus_operations.len(), j.attempts),
            (2, 1),
            "relink and start are recorded; waiting for the lock used up no attempt: {:?}",
            j.incus_operations
        );
        // mock-member-02 holds two seed instances, mock-member-03 one.
        assert_eq!(
            located(&provider, "shop-db"),
            ("mock-member-03".into(), "Running".into())
        );
        assert_eq!(
            located(&provider, "shop-files").0,
            "mock-member-01",
            "member-local data waits"
        );
        let audit = recent_audit(&pool, 30).await.expect("audit");
        for action in [
            "cluster.member.failed",
            "cluster.member.fenced",
            "cluster.member.recovery.decided",
            "application.instance.recover",
            "application.instance.recovered",
        ] {
            assert_eq!(
                audit.iter().filter(|e| e.action == action).count(),
                1,
                "{action}"
            );
        }

        // The member returns: the failure is closed.
        set_member(&mock, "mock-member-01", "Online");
        set_readings(&readings, ["LIVE", "LIVE", "LIVE"]);
        s.pass(&pool, &provider).await.expect("pass");
        assert!(store::open_failures(&pool).await.expect("open").is_empty());
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn without_a_dead_lease_only_a_confirmation_fences_and_a_returned_member_keeps_its_instance(
        pool: Pool,
    ) {
        let mock = mock_with_shared_pool();
        let provider = Provider::Mock(mock.clone());
        deploy(&pool, &provider).await;
        // No probe: the failure waits for a person.
        let mut s = Supervisor::new(Duration::from_secs(3600));
        set_member(&mock, "mock-member-01", "Offline");
        assert_eq!(
            s.pass(&pool, &provider).await.expect("pass"),
            Vec::<String>::new()
        );
        let f = store::open_failures(&pool).await.expect("open").remove(0);
        assert!(reason(&f).contains("APELORA_FENCE_PROBE"), "{}", reason(&f));

        let mut c = pool.acquire().await.expect("conn");
        store::confirm_powered_off(&mut c, "mock-member-01", "user:ana")
            .await
            .expect("confirm");
        assert_eq!(
            s.pass(&pool, &provider).await.expect("pass"),
            vec!["shop-db".to_owned()]
        );
        let f = store::open_failures(&pool).await.expect("open").remove(0);
        assert_eq!(
            f.fence_evidence.expect("evidence")["confirmedBy"],
            "user:ana"
        );

        // The member came back before the job ran: nothing changes, it runs where it was.
        set_member(&mock, "mock-member-01", "Online");
        s.pass(&pool, &provider).await.expect("pass");
        drain(&pool, &provider).await;
        let j = recent_jobs(&pool, 20)
            .await
            .expect("jobs")
            .into_iter()
            .find(|j| j.kind == "instance.recover")
            .expect("job");
        let j = get_job(&pool, j.id).await.expect("job");
        assert_eq!(j.status, "failed");
        assert!(
            j.last_error
                .unwrap_or_default()
                .contains("nothing was changed")
        );
        assert_eq!(
            located(&provider, "shop-db"),
            ("mock-member-01".into(), "Running".into())
        );
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_lease_of_another_generation_does_not_fence(pool: Pool) {
        let mock = mock_with_shared_pool();
        let provider = Provider::Mock(mock.clone());
        deploy(&pool, &provider).await;
        let readings: Readings = Arc::default();
        set_readings(&readings, ["LIVE", "LIVE", "LIVE"]);
        let mut s = Supervisor::new(Duration::from_secs(3600))
            .with_recovery(Recovery::new(Probe::Fixed(readings.clone()), None));
        s.pass(&pool, &provider).await.expect("learn");
        set_member(&mock, "mock-member-01", "Offline");
        let text = "s lvm_apelora-shared:2:/dev/x:0\nh 1 gen 2 timestamp 1 DEAD\nh 2 gen 1 timestamp 1 LIVE\n";
        readings
            .lock()
            .expect("lock")
            .insert("mock-member-02".into(), Ok(text.into()));
        readings
            .lock()
            .expect("lock")
            .insert("mock-member-03".into(), Err("connection refused".into()));
        assert_eq!(
            s.pass(&pool, &provider).await.expect("pass"),
            Vec::<String>::new()
        );
        let f = store::open_failures(&pool).await.expect("open").remove(0);
        assert!(f.fenced_at.is_none());
        assert!(reason(&f).contains("generation"), "{}", reason(&f));
        assert_eq!(located(&provider, "shop-db").0, "mock-member-01");
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn the_mock_refuses_to_relink_member_local_storage_like_incus(pool: Pool) {
        let mock = mock_with_shared_pool();
        let provider = Provider::Mock(mock.clone());
        deploy(&pool, &provider).await;
        set_member(&mock, "mock-member-01", "Offline");
        let refused = provider
            .move_instance(PROJECT, "shop-files", "mock-member-02", false)
            .await;
        assert!(
            matches!(
                refused,
                Err(apelora_incus_adapter::AdapterError::Api { status: 500, .. })
            ),
            "{refused:?}"
        );
        provider
            .move_instance(PROJECT, "shop-db", "mock-member-02", false)
            .await
            .expect("relinked");
        assert_eq!(
            located(&provider, "shop-db"),
            ("mock-member-02".into(), "Stopped".into())
        );
    }
}
