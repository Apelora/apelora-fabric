// SPDX-License-Identifier: AGPL-3.0-only
//! The replica supervisor (Phase 08, M1/M2): restores lost replicas of stateless modules.
//!
//! It compares each application's last deployment (`application_deployments`) with Incus and
//! queues an ordinary `instance.create` job for a missing stateless replica — or
//! `instance.start` for one that stopped although it should run, unless it was stopped on
//! purpose through Apelora (its latest lifecycle job is a stop) — unless a job for
//! that instance is already in flight or failed within the back-off window. Instance creation
//! is idempotent, so two supervisors racing still produce one instance. Singleton and stateful
//! modules are never recreated automatically: they are reported and left to an operator.
//! Every recreation is audited; afterwards the application's network rules are re-synced so
//! the new replica's address joins its address set.
//!
//! A member that fails (Incus reports it Offline for longer than the recovery delay) keeps its
//! instances: Incus cannot move them, and its healing stays off. A stateless replica whose
//! every instance is on a failed member gets a *substitute* on another member
//! (`<replica>-s<n>`, same labels). When the member is back, the slot holds two instances and
//! the one that keeps replicas apart stays; the other is retired (deleted). Both are audited.
//!
//! Each pass also observes the health of running service replicas ([`crate::health`]) and
//! restarts a stateless replica that keeps failing its port check.
//!
//! Only one worker supervises at a time: the holder of the `replica-supervisor` leader lease in
//! PostgreSQL ([`LeaderLease`], ADR 0011). The others skip their passes and take over within
//! [`LEADER_LEASE_SECS`] once the holder stops renewing. Every write of a pass (a queued job
//! with its audit row, a health record) runs in a transaction that first checks the lease is
//! still held at its generation, so a leader that stalled past its lease writes nothing.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use apelora_domain::Instance;
use apelora_domain::health::HealthPolicy;
use apelora_domain::network::{Connection, SyncRequest, discovery_env, with_discovery};
use apelora_domain::plan::{
    Availability, DesiredModule, PlanAction, deploy_spec, plan, slot_instances, substitute_name,
};
use apelora_incus_adapter::Provider;
use apelora_persistence::{
    Leadership, Pool, StoreError, acquire_leadership, begin_fenced, renew_leadership,
};
use serde_json::{Value, json};
use tokio::sync::watch;
use uuid::Uuid;

use crate::health::HealthWatch;

/// After a failed recreation, wait this long before trying again.
const BACKOFF_SECS: i64 = 300;

/// A failed network sync is queued again after this many seconds.
const SYNC_RETRY_SECS: f64 = 60.0;

/// A condition that persists across passes (a missing stateful instance) is logged again at
/// most this often, and at once when it changes.
const REPEAT_LOG_AFTER: Duration = Duration::from_secs(600);

/// Remembers since when each member has been offline, so a short outage (a reboot, a network
/// blip) does not trigger recovery: hysteresis across passes.
#[derive(Debug)]
pub struct Supervisor {
    recover_after: Duration,
    offline_since: HashMap<String, Instant>,
    health: HealthWatch,
    /// What was last reported per instance (`project/name`) and when, so a lasting condition
    /// is not logged on every pass.
    reported: HashMap<String, (String, Instant)>,
    /// The member list could not be read on the previous pass.
    members_unreadable: bool,
    /// The leader lease the current pass writes under; `None` for one-shot passes (tests).
    leadership: Option<Leadership>,
    /// Failed members: fencing and restarts on shared storage (HA slice B, ADR 0013).
    recovery: crate::recovery::Recovery,
}

/// The leader lease the replica supervisor runs under.
pub const LEADER_LEASE: &str = "replica-supervisor";
/// How long the lease lasts unless renewed; a new leader takes over this long after the old one
/// stopped renewing.
pub const LEADER_LEASE_SECS: f64 = 15.0;
/// How often the holder renews the lease and the others try to take it.
pub const LEADER_RENEW_EVERY: Duration = Duration::from_secs(5);

/// One process's view of a leader lease: renews it while held, tries to take it otherwise, and
/// logs every change of leadership at info.
#[derive(Debug)]
pub struct LeaderLease {
    name: String,
    holder: String,
    ttl_secs: f64,
    held: Option<Leadership>,
}

impl LeaderLease {
    pub fn new(name: &str, holder: &str, ttl_secs: f64) -> Self {
        LeaderLease {
            name: name.to_owned(),
            holder: holder.to_owned(),
            ttl_secs,
            held: None,
        }
    }

    /// Renews the lease if held, otherwise tries to take it. Returns the leadership now held.
    /// When the database cannot be reached the lease is kept as it was: writes are fenced, so a
    /// lease that expired meanwhile cannot be used to write.
    pub async fn tick(&mut self, pool: &Pool) -> Option<Leadership> {
        match self.held.take() {
            Some(l) => match renew_leadership(pool, &l, self.ttl_secs).await {
                Ok(()) => self.held = Some(l),
                Err(StoreError::Conflict(_)) => {
                    tracing::info!(lease = %l.name, holder = %l.holder, generation = l.generation, "leader: lease lost (it expired or another worker took it); no longer acting as the leader");
                }
                Err(e) => {
                    tracing::warn!(lease = %l.name, error = %e, "leader: cannot renew the lease; writes stay fenced until it is renewed");
                    self.held = Some(l);
                }
            },
            None => match acquire_leadership(pool, &self.name, &self.holder, self.ttl_secs).await {
                Ok(Some(l)) => {
                    tracing::info!(lease = %l.name, holder = %l.holder, generation = l.generation, "leader: lease acquired; acting as the leader");
                    self.held = Some(l);
                }
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(lease = %self.name, error = %e, "leader: cannot read the lease; not acting as the leader")
                }
            },
        }
        self.held.clone()
    }
}

/// Queues a job and its audit row in one transaction, fenced by `lease` when one is held.
/// `detail` gets the new job's id.
async fn queue_audited(
    pool: &Pool,
    lease: Option<&Leadership>,
    (kind, application_id, payload): (&str, Option<Uuid>, &Value),
    action: &str,
    target: &str,
    detail: impl FnOnce(Uuid) -> Value,
) -> Result<Uuid, StoreError> {
    let mut tx = begin_fenced(pool, lease).await?;
    let job =
        apelora_persistence::enqueue_job_in(&mut tx, kind, application_id, payload, None, None)
            .await?
            .id();
    apelora_persistence::audit(&mut *tx, "supervisor", action, target, &detail(job)).await?;
    tx.commit().await?;
    Ok(job)
}

/// Counts a replica action for metrics (`queued`, or `deferred` by a job in flight or the
/// back-off after a failure).
pub(crate) fn count_action(action: &str, outcome: &str) {
    apelora_metrics::SUPERVISOR_ACTIONS.inc(&[action, outcome]);
}

/// One supervision pass with no memory: offline members count as failed at once, and one
/// failing health observation is enough to act.
pub async fn supervise_once(pool: &Pool, provider: &Provider) -> Result<Vec<String>, StoreError> {
    Supervisor::new(Duration::ZERO)
        .with_health(HealthPolicy::immediate())
        .pass(pool, provider)
        .await
}

impl Supervisor {
    pub fn new(recover_after: Duration) -> Self {
        Supervisor {
            recover_after,
            offline_since: HashMap::new(),
            health: HealthWatch::new(HealthPolicy::default()),
            reported: HashMap::new(),
            members_unreadable: false,
            leadership: None,
            recovery: crate::recovery::Recovery::default(),
        }
    }

    /// Restarts on shared storage after fencing, with this fence probe (ADR 0013).
    pub fn with_recovery(mut self, recovery: crate::recovery::Recovery) -> Self {
        self.recovery = recovery;
        self
    }

    /// One pass as the holder of `lease`, or `None` without a pass when this process does not
    /// lead. Leadership that is new (a takeover, or regained after losing it) starts from a clean
    /// memory: what was observed under an earlier leadership may be stale.
    pub async fn pass_if_leader(
        &mut self,
        pool: &Pool,
        provider: &Provider,
        lease: Option<Leadership>,
    ) -> Result<Option<Vec<String>>, StoreError> {
        let Some(lease) = lease else {
            self.leadership = None;
            return Ok(None);
        };
        if self.leadership.as_ref().map(|l| l.generation) != Some(lease.generation) {
            self.offline_since.clear();
            self.reported.clear();
            self.health.forget();
            self.recovery.forget();
            self.members_unreadable = false;
        }
        self.leadership = Some(lease);
        self.pass(pool, provider).await.map(Some)
    }

    /// Whether `condition` of `instance` should be logged now: it is new, it changed, or it was
    /// last logged longer than [`REPEAT_LOG_AFTER`] ago.
    fn should_report(&mut self, instance: &str, condition: &str) -> bool {
        let now = Instant::now();
        match self.reported.get(instance) {
            Some((c, at)) if c == condition && now.duration_since(*at) < REPEAT_LOG_AFTER => false,
            _ => {
                self.reported
                    .insert(instance.to_owned(), (condition.to_owned(), now));
                true
            }
        }
    }

    pub fn with_health(mut self, policy: HealthPolicy) -> Self {
        self.health = HealthWatch::new(policy);
        self
    }

    /// One supervision pass. Returns the instances it queued for creation, restart or retirement.
    pub async fn pass(
        &mut self,
        pool: &Pool,
        provider: &Provider,
    ) -> Result<Vec<String>, StoreError> {
        // Failed members first: fencing and restarts on shared storage are decided before
        // anything else, and recorded even when no application is deployed.
        let mut recovered = self
            .recovery
            .pass(pool, provider, self.leadership.as_ref())
            .await?;
        let deployments = apelora_persistence::list_deployments(pool).await?;
        if deployments.is_empty() {
            return Ok(recovered);
        }
        let observed = match provider.instances().await {
            Ok(o) => o,
            Err(e) => {
                // Without an inventory nothing can be decided safely.
                tracing::warn!(error = %e, "supervisor: cannot read instances; skipping this pass");
                return Ok(Vec::new());
            }
        };
        // Instances on a member that is not Online (evacuated, being evacuated, offline) are
        // Incus's to handle: restarting them would fight a maintenance evacuation.
        let members = match provider.cluster_members().await {
            Ok(m) => {
                if std::mem::take(&mut self.members_unreadable) {
                    tracing::info!("supervisor: the cluster member list is readable again");
                }
                Some(m)
            }
            Err(e) => {
                if !std::mem::replace(&mut self.members_unreadable, true) {
                    tracing::warn!(error = %e, "supervisor: cannot read the cluster members; offline detection and substitutes are skipped until they can be read");
                } else {
                    tracing::debug!(error = %e, "supervisor: cluster members still unreadable");
                }
                None
            }
        };
        let members_known = members.is_some();
        let members = members.unwrap_or_default();
        let unavailable: Vec<String> = members
            .iter()
            .filter(|m| m.status != "Online")
            .map(|m| m.name.clone())
            .collect();
        let now = Instant::now();
        // Without the member list nothing is known about members: keep what was known.
        self.offline_since.retain(|name, since| {
            if !members_known {
                return true;
            }
            let still = members
                .iter()
                .any(|m| &m.name == name && m.status == "Offline");
            if !still {
                tracing::info!(member = %name, offline_secs = since.elapsed().as_secs(), "supervisor: member is no longer offline");
            }
            still
        });
        for m in members.iter().filter(|m| m.status == "Offline") {
            self.offline_since.entry(m.name.clone()).or_insert_with(|| {
                tracing::warn!(member = %m.name, message = %m.message, "supervisor: member is offline");
                now
            });
        }
        let failed: Vec<String> = self
            .offline_since
            .iter()
            .filter(|_| members_known)
            .filter(|(_, since)| now.duration_since(**since) >= self.recover_after)
            .map(|(name, _)| name.clone())
            .collect();
        let mut recreated = std::mem::take(&mut recovered);
        for d in deployments {
            let Ok(desired) = serde_json::from_value::<Vec<DesiredModule>>(d.desired.clone())
            else {
                tracing::warn!(application = %d.application, "supervisor: unreadable deployment record");
                continue;
            };
            let published: Vec<apelora_domain::network::Published> = d
                .published
                .clone()
                .and_then(|p| serde_json::from_value(p).ok())
                .unwrap_or_default();
            let connections: Option<Vec<Connection>> = d
                .connections
                .clone()
                .and_then(|c| serde_json::from_value(c).ok());
            let p = plan(&d.application, &d.project, &desired, &observed);
            // Recreated and substituted instances get the same discovery environment (ADR 0004).
            let addresses: std::collections::BTreeMap<String, String> =
                apelora_persistence::service_addresses(pool, d.application_id, &d.project)
                    .await?
                    .into_iter()
                    .collect();
            let mut queued_here = 0;
            // Replica slots first: substitutes for replicas stranded on a failed member, and
            // retirement of the extra instance once that member is back.
            let mut handled: Vec<String> = Vec::new();
            for da in desired
                .iter()
                .filter(|x| x.availability == Availability::Stateless)
            {
                for replica in 1..=da.replicas.max(1) {
                    let slot =
                        slot_instances(&d.application, &d.project, &da.module, replica, &observed);
                    if slot.is_empty() {
                        continue;
                    }
                    if slot.iter().all(|i| failed.contains(&i.location)) {
                        handled.extend(slot.iter().map(|i| i.name.clone()));
                        let Some(name) = (1..=9)
                            .map(|n| substitute_name(&d.application, &da.module, replica, n))
                            .find(|n| {
                                !observed
                                    .iter()
                                    .any(|i| i.project == d.project && &i.name == n)
                            })
                        else {
                            tracing::warn!(application = %d.application, module = %da.module, replica, "supervisor: no substitute name left");
                            continue;
                        };
                        if apelora_persistence::job_in_flight(
                            pool,
                            "instance.create",
                            &d.project,
                            &name,
                        )
                        .await?
                            || apelora_persistence::job_failed_recently(
                                pool,
                                "instance.create",
                                &d.project,
                                &name,
                                BACKOFF_SECS,
                            )
                            .await?
                        {
                            tracing::debug!(application = %d.application, instance = %format!("{}/{name}", d.project), "supervisor: substitute already queued or failed recently; not queued again");
                            count_action("substitute", "deferred");
                            continue;
                        }
                        let uplink = d.uplink.as_deref().filter(|_| connections.is_some());
                        let mut spec = with_discovery(
                            deploy_spec(&d.application, &d.project, da, replica, uplink),
                            &discovery_env(
                                &d.application,
                                &da.module,
                                connections.as_deref().unwrap_or_default(),
                                &addresses,
                            ),
                        );
                        spec.name = name.clone();
                        let payload = serde_json::to_value(&spec)
                            .map_err(|e| StoreError::Conflict(e.to_string()))?;
                        let stranded: Vec<String> = slot
                            .iter()
                            .map(|i| format!("{} on {}", i.name, i.location))
                            .collect();
                        queue_audited(
                            pool,
                            self.leadership.as_ref(),
                            ("instance.create", Some(d.application_id), &payload),
                            "application.replica.substitute",
                            &format!("{}/{name}", d.project),
                            |job| json!({ "application": d.application, "module": da.module, "replica": replica, "stranded": stranded, "jobId": job }),
                        )
                        .await?;
                        tracing::info!(application = %d.application, instance = %name, ?stranded, "supervisor: substituting a replica stranded on a failed member");
                        count_action("substitute", "queued");
                        recreated.push(name);
                        queued_here += 1;
                    } else if slot.len() > 1
                        && slot.iter().all(|i| !unavailable.contains(&i.location))
                    {
                        // Decide once: a retirement already under way is never reversed.
                        let mut retiring = false;
                        for i in &slot {
                            retiring |= apelora_persistence::job_in_flight(
                                pool,
                                "instance.delete",
                                &d.project,
                                &i.name,
                            )
                            .await?;
                        }
                        if retiring {
                            tracing::debug!(application = %d.application, module = %da.module, replica, "supervisor: a duplicate is already being retired");
                            handled.extend(slot.iter().map(|i| i.name.clone()));
                            continue;
                        }
                        // Decide only when every instance runs (a returning member's instances
                        // may still be starting; the restart below may start them).
                        if slot.iter().any(|i| i.status != "Running") {
                            tracing::debug!(application = %d.application, module = %da.module, replica, "supervisor: duplicate replicas, not all running yet; deciding later");
                            continue;
                        }
                        handled.extend(slot.iter().map(|i| i.name.clone()));
                        let Some(keep) = keeper(&slot, &observed) else {
                            continue;
                        };
                        for other in slot.iter().filter(|i| i.name != keep.name) {
                            if apelora_persistence::job_in_flight(
                                pool,
                                "instance.delete",
                                &d.project,
                                &other.name,
                            )
                            .await?
                            {
                                continue;
                            }
                            queue_audited(
                                pool,
                                self.leadership.as_ref(),
                                ("instance.delete", Some(d.application_id), &json!({ "project": d.project, "name": other.name })),
                                "application.replica.retire",
                                &format!("{}/{}", d.project, other.name),
                                |job| json!({ "application": d.application, "module": da.module, "replica": replica, "kept": keep.name, "jobId": job }),
                            )
                            .await?;
                            tracing::info!(application = %d.application, instance = %other.name, kept = %keep.name, "supervisor: retiring a duplicate replica");
                            count_action("retire", "queued");
                            recreated.push(other.name.clone());
                            queued_here += 1;
                        }
                    }
                }
            }
            for item in p.items.iter().filter(|i| !handled.contains(&i.instance)) {
                let Some(da) = desired.iter().find(|x| x.module == item.module) else {
                    continue;
                };
                // Missing → recreate; kept but stopped although it should run → restart.
                let stopped = item.action == PlanAction::Keep
                    && da.spec.start
                    && observed.iter().any(|i| {
                        i.project == d.project
                            && i.name == item.instance
                            && i.status == "Stopped"
                            && !unavailable.contains(&i.location)
                    });
                if item.action != PlanAction::Create && !stopped {
                    continue;
                }
                let kind = if stopped {
                    "instance.start"
                } else {
                    "instance.create"
                };
                if da.availability != Availability::Stateless {
                    // Still being created or started by a job: nothing is lost yet.
                    let instance = format!("{}/{}", d.project, item.instance);
                    if !apelora_persistence::job_in_flight(pool, kind, &d.project, &item.instance)
                        .await?
                        && self
                            .should_report(&instance, if stopped { "stopped" } else { "missing" })
                    {
                        tracing::warn!(application = %d.application, %instance, availability = ?da.availability, state = if stopped { "stopped" } else { "missing" },
                        "supervisor: instance is missing or stopped; only stateless replicas are restored automatically (repeated at most every 10 minutes)");
                    }
                    continue;
                }
                if stopped
                    && apelora_persistence::last_lifecycle_job(pool, &d.project, &item.instance)
                        .await?
                        .as_deref()
                        == Some("instance.stop")
                {
                    // Stopped on purpose (through Apelora): respected until someone starts it.
                    tracing::debug!(application = %d.application, instance = %format!("{}/{}", d.project, item.instance), "supervisor: stopped on purpose through Apelora; left stopped");
                    continue;
                }
                if apelora_persistence::job_in_flight(pool, kind, &d.project, &item.instance)
                    .await?
                    || apelora_persistence::job_failed_recently(
                        pool,
                        kind,
                        &d.project,
                        &item.instance,
                        BACKOFF_SECS,
                    )
                    .await?
                {
                    tracing::debug!(application = %d.application, instance = %format!("{}/{}", d.project, item.instance), kind, backoff_secs = BACKOFF_SECS, "supervisor: a job for the instance is in flight or failed recently; not queued again");
                    count_action(if stopped { "restart" } else { "recreate" }, "deferred");
                    continue;
                }
                let payload = if stopped {
                    json!({ "project": d.project, "name": item.instance })
                } else {
                    let uplink = d.uplink.as_deref().filter(|_| connections.is_some());
                    let spec = with_discovery(
                        deploy_spec(&d.application, &d.project, da, item.replica, uplink),
                        &discovery_env(
                            &d.application,
                            &da.module,
                            connections.as_deref().unwrap_or_default(),
                            &addresses,
                        ),
                    );
                    serde_json::to_value(&spec).map_err(|e| StoreError::Conflict(e.to_string()))?
                };
                let action = if stopped {
                    "application.replica.restart"
                } else {
                    "application.replica.recreate"
                };
                let job = queue_audited(
                    pool,
                    self.leadership.as_ref(),
                    (kind, Some(d.application_id), &payload),
                    action,
                    &format!("{}/{}", d.project, item.instance),
                    |job| json!({ "application": d.application, "module": item.module, "replica": item.replica, "revision": d.revision, "jobId": job }),
                )
                .await?;
                tracing::info!(application = %d.application, instance = %item.instance, %job, kind, "supervisor: restoring a stateless replica");
                count_action(if stopped { "restart" } else { "recreate" }, "queued");
                recreated.push(item.instance.clone());
                queued_here += 1;
            }
            // A network sync that failed (e.g. Incus refused a load balancer update while a
            // member was going down) is retried after a minute, so address sets and service
            // backends catch up without a new deployment.
            let sync_failed = match apelora_persistence::latest_application_job(
                pool,
                "network.sync",
                d.application_id,
            )
            .await?
            {
                Some((status, age)) => status == "failed" && age >= SYNC_RETRY_SECS,
                None => false,
            };
            if (queued_here > 0 || sync_failed)
                && let (Some(uplink), Some(connections)) = (&d.uplink, &connections)
                && !apelora_persistence::application_job_in_flight(
                    pool,
                    "network.sync",
                    d.application_id,
                )
                .await?
            {
                let payload = serde_json::to_value(
                    SyncRequest::for_deployment(
                        &d.application,
                        &d.project,
                        uplink,
                        &desired,
                        connections,
                        d.service_range.as_deref(),
                    )
                    .with_published(&published),
                )
                .map_err(|e| StoreError::Conflict(e.to_string()))?;
                let mut tx = begin_fenced(pool, self.leadership.as_ref()).await?;
                apelora_persistence::enqueue_job_in(
                    &mut tx,
                    "network.sync",
                    Some(d.application_id),
                    &payload,
                    None,
                    None,
                )
                .await?;
                tx.commit().await?;
            }
            recreated.extend(
                self.health
                    .check(
                        pool,
                        provider,
                        &d,
                        &desired,
                        &observed,
                        &unavailable,
                        self.leadership.as_ref(),
                    )
                    .await?,
            );
        }
        Ok(recreated)
    }
}

/// Of a slot's instances (all reachable), the one to keep: running, on the member with the
/// fewest other instances of the same module (replicas apart), then the original name.
fn keeper<'a>(slot: &[&'a Instance], observed: &[Instance]) -> Option<&'a Instance> {
    let siblings = |i: &Instance| {
        observed
            .iter()
            .filter(|x| {
                x.location == i.location
                    && x.project == i.project
                    && !slot.iter().any(|s| s.name == x.name)
                    && x.owner
                        .as_ref()
                        .zip(i.owner.as_ref())
                        .is_some_and(|(a, b)| {
                            a.application == b.application && a.module == b.module
                        })
            })
            .count()
    };
    slot.iter()
        .filter(|i| i.status == "Running")
        .min_by_key(|i| (siblings(i), i.name.len(), i.name.clone()))
        .copied()
}

/// Runs supervision passes every `every` while this worker (`holder`) holds the leader lease,
/// until the task is dropped; a member counts as failed once it has been offline for
/// `recover_after`. The lease is renewed on its own task, so a long pass does not let it lapse.
pub async fn supervise(
    pool: Pool,
    provider: Provider,
    every: Duration,
    recover_after: Duration,
    health: HealthPolicy,
    holder: String,
    recovery: crate::recovery::Recovery,
) {
    let mut lease = LeaderLease::new(LEADER_LEASE, &holder, LEADER_LEASE_SECS);
    let (tx, rx) = watch::channel(lease.tick(&pool).await);
    let renewals = tokio::spawn({
        let pool = pool.clone();
        async move {
            loop {
                tokio::time::sleep(LEADER_RENEW_EVERY).await;
                tx.send_replace(lease.tick(&pool).await);
            }
        }
    });
    // Stop renewing when supervision stops (the task is aborted at shutdown).
    let _renewals = AbortOnDrop(renewals);
    let mut supervisor = Supervisor::new(recover_after)
        .with_health(health)
        .with_recovery(recovery);
    loop {
        let current = rx.borrow().clone();
        let started = std::time::Instant::now();
        let result = supervisor.pass_if_leader(&pool, &provider, current).await;
        if !matches!(result, Ok(None)) {
            apelora_metrics::SUPERVISOR_PASS.observe(
                &[if result.is_ok() { "ok" } else { "error" }],
                started.elapsed().as_secs_f64(),
            );
            apelora_metrics::SUPERVISOR_LAST_PASS.set(&[], apelora_metrics::now_secs());
        }
        match result {
            Ok(Some(_)) => {}
            Ok(None) => tracing::debug!(
                "supervisor: another worker holds the leader lease; skipping this pass"
            ),
            Err(StoreError::Conflict(msg)) => {
                tracing::warn!(%msg, "supervisor: pass stopped by a refused write (the leader lease is no longer held, or a record could not be encoded)")
            }
            Err(e) => tracing::error!(error = %e, "supervisor pass failed"),
        }
        tokio::time::sleep(every).await;
    }
}

pub(crate) struct AbortOnDrop(pub(crate) tokio::task::JoinHandle<()>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apelora_domain::{ImageProtocol, ImageSource, InstanceKind, InstanceSpec};
    use apelora_incus_adapter::MockIncus;
    use apelora_persistence::{enqueue_job, recent_audit, recent_jobs};

    const PROJECT: &str = "apelora";

    #[test]
    fn a_lasting_condition_is_reported_once_until_it_changes() {
        let mut s = Supervisor::new(Duration::ZERO);
        assert!(s.should_report("apelora/db-01", "missing"));
        assert!(
            !s.should_report("apelora/db-01", "missing"),
            "not every pass"
        );
        assert!(s.should_report("apelora/db-01", "stopped"), "a change is");
        assert!(
            s.should_report("apelora/cache-01", "missing"),
            "per instance"
        );
    }

    fn desired(module: &str, replicas: u8, availability: Availability) -> DesiredModule {
        DesiredModule {
            module: module.into(),
            spec: InstanceSpec {
                project: String::new(),
                name: String::new(),
                kind: InstanceKind::Container,
                source: ImageSource {
                    protocol: ImageProtocol::Simplestreams,
                    server: "https://images.example".into(),
                    alias: "debian/13".into(),
                    fingerprint: None,
                },
                profiles: vec![],
                config: Default::default(),
                start: true,
                architectures: vec![],
                owner: None,
                network: None,
                volumes: Vec::new(),
                root_pool: None,
            },
            replicas,
            availability,
            internet: Default::default(),
            guest_os: Default::default(),
        }
    }

    async fn drain(pool: &Pool, provider: &Provider) {
        while crate::run_once(pool, provider, "test-worker")
            .await
            .expect("run")
        {}
    }

    /// Deploys `shop` (two stateless web replicas, one stateful database) and records it.
    async fn deploy(pool: &Pool, provider: &Provider) {
        let app = apelora_persistence::ensure_application(pool, "shop")
            .await
            .expect("app");
        let d = vec![
            desired("web-01", 2, Availability::Stateless),
            desired("db-01", 1, Availability::Stateful),
        ];
        for x in &d {
            for r in 1..=x.replicas {
                let spec = deploy_spec("shop", PROJECT, x, r, None);
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
        }
        record(pool, app.id, 1, &serde_json::to_value(&d).expect("json")).await;
        drain(pool, provider).await;
    }

    async fn record(
        pool: &Pool,
        application_id: uuid::Uuid,
        revision: i32,
        desired: &serde_json::Value,
    ) {
        let d = apelora_persistence::NewDeployment {
            application_id,
            project: PROJECT,
            revision,
            desired,
            connections: None,
            uplink: None,
            service_range: None,
            published: None,
        };
        apelora_persistence::record_deployment(pool, &d)
            .await
            .expect("record");
    }

    async fn delete(pool: &Pool, provider: &Provider, name: &str) {
        enqueue_job(
            pool,
            "instance.delete",
            None,
            &json!({ "project": PROJECT, "name": name }),
            None,
        )
        .await
        .expect("enqueue");
        drain(pool, provider).await;
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_lost_stateless_replica_is_recreated_once_and_audited(pool: Pool) {
        let provider = Provider::Mock(MockIncus::default());
        deploy(&pool, &provider).await;
        assert_eq!(
            supervise_once(&pool, &provider).await.expect("pass"),
            Vec::<String>::new(),
            "nothing is missing"
        );

        delete(&pool, &provider, "shop-web-01-r2").await;
        assert_eq!(
            supervise_once(&pool, &provider).await.expect("pass"),
            vec!["shop-web-01-r2".to_owned()]
        );
        assert_eq!(
            supervise_once(&pool, &provider).await.expect("pass"),
            Vec::<String>::new(),
            "a recreation already queued is not queued twice"
        );
        drain(&pool, &provider).await;
        let restored = provider
            .instance(PROJECT, "shop-web-01-r2")
            .await
            .expect("read")
            .expect("recreated");
        assert_eq!(
            (restored.status.as_str(), restored.managed),
            ("Running", true)
        );
        let owner = provider
            .instances()
            .await
            .expect("list")
            .into_iter()
            .find(|i| i.name == "shop-web-01-r2")
            .and_then(|i| i.owner);
        assert_eq!(
            owner.map(|o| (o.application, o.module, o.replica)),
            Some(("shop".into(), "web-01".into(), 2))
        );
        let audit = recent_audit(&pool, 10).await.expect("audit");
        assert_eq!(
            audit
                .iter()
                .filter(|e| e.action == "application.replica.recreate" && e.actor == "supervisor")
                .count(),
            1
        );
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn stateful_modules_are_not_recreated_and_failures_back_off(pool: Pool) {
        let provider = Provider::Mock(MockIncus::default());
        deploy(&pool, &provider).await;
        delete(&pool, &provider, "shop-db-01").await;
        assert_eq!(
            supervise_once(&pool, &provider).await.expect("pass"),
            Vec::<String>::new(),
            "stateful: left to an operator"
        );
        assert_eq!(provider.instance(PROJECT, "shop-db-01").await, Ok(None));

        // A recreation that failed is not retried within the back-off window.
        delete(&pool, &provider, "shop-web-01").await;
        assert_eq!(
            supervise_once(&pool, &provider).await.expect("pass"),
            vec!["shop-web-01".to_owned()]
        );
        let failed = sqlx::query(
            "UPDATE jobs SET status = 'failed', updated_at = now()
             WHERE kind = 'instance.create' AND status = 'pending'",
        )
        .execute(&pool)
        .await
        .expect("fail it");
        assert_eq!(
            failed.rows_affected(),
            1,
            "the queued recreation is the one that fails"
        );
        assert_eq!(
            supervise_once(&pool, &provider).await.expect("pass"),
            Vec::<String>::new(),
            "backing off"
        );
        let creates = recent_jobs(&pool, 50)
            .await
            .expect("jobs")
            .into_iter()
            .filter(|j| j.kind == "instance.create")
            .count();
        assert_eq!(creates, 4, "three from the deployment, one recreation");
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn removed_applications_are_not_supervised(pool: Pool) {
        let provider = Provider::Mock(MockIncus::default());
        deploy(&pool, &provider).await;
        let app = apelora_persistence::ensure_application(&pool, "shop")
            .await
            .expect("app");
        record(&pool, app.id, 2, &json!([])).await;
        delete(&pool, &provider, "shop-web-01").await;
        assert_eq!(
            supervise_once(&pool, &provider).await.expect("pass"),
            Vec::<String>::new()
        );
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_crashed_replica_is_restarted_but_a_deliberate_stop_is_respected(pool: Pool) {
        let provider = Provider::Mock(MockIncus::default());
        deploy(&pool, &provider).await;

        // Stopped behind Apelora's back (a crash): started again, once.
        provider
            .change_state(
                PROJECT,
                "shop-web-01",
                apelora_domain::InstanceAction::Stop,
                true,
            )
            .await
            .expect("crash");
        assert_eq!(
            supervise_once(&pool, &provider).await.expect("pass"),
            vec!["shop-web-01".to_owned()]
        );
        assert_eq!(
            supervise_once(&pool, &provider).await.expect("pass"),
            Vec::<String>::new()
        );
        drain(&pool, &provider).await;
        assert_eq!(
            provider
                .instance(PROJECT, "shop-web-01")
                .await
                .map(|s| s.map(|s| s.status)),
            Ok(Some("Running".into()))
        );
        assert_eq!(
            recent_audit(&pool, 10)
                .await
                .expect("audit")
                .iter()
                .filter(|e| e.action == "application.replica.restart")
                .count(),
            1
        );

        // Stopped through Apelora: left alone. A stopped stateful instance is never started.
        for name in ["shop-web-01-r2", "shop-db-01"] {
            enqueue_job(
                &pool,
                "instance.stop",
                None,
                &json!({ "project": PROJECT, "name": name }),
                None,
            )
            .await
            .expect("enqueue");
        }
        drain(&pool, &provider).await;
        assert_eq!(
            supervise_once(&pool, &provider).await.expect("pass"),
            Vec::<String>::new()
        );
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn replicas_stopped_by_an_evacuation_are_left_to_incus(pool: Pool) {
        let provider = Provider::Mock(MockIncus::default());
        deploy(&pool, &provider).await;
        provider
            .change_state(
                PROJECT,
                "shop-web-01",
                apelora_domain::InstanceAction::Stop,
                true,
            )
            .await
            .expect("stopped by the evacuation");
        // The mock places every instance on mock-member-01.
        provider
            .member_state("mock-member-01", false)
            .await
            .expect("evacuate");
        assert_eq!(
            supervise_once(&pool, &provider).await.expect("pass"),
            Vec::<String>::new()
        );
        provider
            .member_state("mock-member-01", true)
            .await
            .expect("restore");
        assert_eq!(
            supervise_once(&pool, &provider).await.expect("pass"),
            vec!["shop-web-01".to_owned()]
        );
    }

    fn set_member(mock: &MockIncus, name: &str, status: &str) {
        let mut members = mock.members.lock().unwrap_or_else(|e| e.into_inner());
        members
            .iter_mut()
            .find(|m| m.name == name)
            .expect("member")
            .status = status.into();
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn replicas_on_a_failed_member_get_substitutes_and_extras_retire_when_it_returns(
        pool: Pool,
    ) {
        let mock = MockIncus::default();
        let provider = Provider::Mock(mock.clone());
        deploy(&pool, &provider).await; // everything lands on mock-member-01

        // A short outage is tolerated: no recovery before the delay.
        set_member(&mock, "mock-member-01", "Offline");
        let mut patient = Supervisor::new(Duration::from_secs(3600));
        assert_eq!(
            patient.pass(&pool, &provider).await.expect("pass"),
            Vec::<String>::new()
        );

        // Past the delay: both stateless replicas get substitutes elsewhere; the stateful
        // database does not.
        let mut supervisor = Supervisor::new(Duration::ZERO);
        let mut queued = supervisor.pass(&pool, &provider).await.expect("pass");
        queued.sort();
        assert_eq!(queued, ["shop-web-01-r2-s1", "shop-web-01-s1"]);
        assert_eq!(
            supervisor.pass(&pool, &provider).await.expect("pass"),
            Vec::<String>::new(),
            "not twice"
        );
        drain(&pool, &provider).await;
        let located = |name: &str| {
            let provider = provider.clone();
            let name = name.to_owned();
            async move {
                provider
                    .instances()
                    .await
                    .expect("list")
                    .into_iter()
                    .find(|i| i.name == name)
                    .map(|i| i.location)
            }
        };
        assert_eq!(
            located("shop-web-01-s1").await.as_deref(),
            Some("mock-member-02")
        );
        assert_eq!(
            supervisor.pass(&pool, &provider).await.expect("pass"),
            Vec::<String>::new(),
            "slots are filled"
        );

        // The member returns: each slot holds two instances, one is retired.
        set_member(&mock, "mock-member-01", "Online");
        let mut retired = supervisor.pass(&pool, &provider).await.expect("pass");
        retired.sort();
        assert_eq!(retired.len(), 2, "{retired:?}");
        drain(&pool, &provider).await;
        let names: Vec<String> = provider
            .instances()
            .await
            .expect("list")
            .into_iter()
            .filter(|i| i.name.starts_with("shop-web-01"))
            .map(|i| i.name)
            .collect();
        assert_eq!(names.len(), 2, "one instance per replica again: {names:?}");
        let audit = recent_audit(&pool, 20).await.expect("audit");
        for action in [
            "application.replica.substitute",
            "application.replica.retire",
        ] {
            assert_eq!(
                audit.iter().filter(|e| e.action == action).count(),
                2,
                "{action}"
            );
        }
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_failed_network_sync_is_retried_after_a_minute(pool: Pool) {
        let provider = Provider::Mock(MockIncus::default());
        deploy(&pool, &provider).await;
        let app = apelora_persistence::ensure_application(&pool, "shop")
            .await
            .expect("app");
        let d =
            serde_json::to_value([desired("web-01", 2, Availability::Stateless)]).expect("json");
        let conns = json!([]);
        apelora_persistence::record_deployment(
            &pool,
            &apelora_persistence::NewDeployment {
                application_id: app.id,
                project: PROJECT,
                revision: 2,
                desired: &d,
                connections: Some(&conns),
                uplink: Some("incusbr0"),
                service_range: None,
                published: None,
            },
        )
        .await
        .expect("record");
        let failed = enqueue_job(&pool, "network.sync", Some(app.id), &json!({}), None)
            .await
            .expect("enqueue")
            .id();
        sqlx::query("UPDATE jobs SET status = 'failed', updated_at = now() - interval '2 minutes' WHERE id = $1")
            .bind(failed)
            .execute(&pool)
            .await
            .expect("fail it");
        supervise_once(&pool, &provider).await.expect("pass");
        let syncs = recent_jobs(&pool, 20)
            .await
            .expect("jobs")
            .into_iter()
            .filter(|j| j.kind == "network.sync")
            .count();
        assert_eq!(syncs, 2, "queued again");
        supervise_once(&pool, &provider).await.expect("pass");
        let syncs = recent_jobs(&pool, 20)
            .await
            .expect("jobs")
            .into_iter()
            .filter(|j| j.kind == "network.sync")
            .count();
        assert_eq!(syncs, 2, "not while the retry is waiting");
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn retirement_waits_for_a_running_slot_and_never_decides_twice(pool: Pool) {
        let mock = MockIncus::default();
        let provider = Provider::Mock(mock.clone());
        deploy(&pool, &provider).await;
        set_member(&mock, "mock-member-01", "Offline");
        let mut supervisor = Supervisor::new(Duration::ZERO);
        supervisor.pass(&pool, &provider).await.expect("pass");
        drain(&pool, &provider).await;

        // The member returns, but its instances are still starting: nothing is retired yet.
        set_member(&mock, "mock-member-01", "Online");
        for name in ["shop-web-01", "shop-web-01-r2"] {
            provider
                .change_state(PROJECT, name, apelora_domain::InstanceAction::Stop, true)
                .await
                .expect("booting");
        }
        let queued = supervisor.pass(&pool, &provider).await.expect("pass");
        assert!(
            !queued.iter().any(|n| n.ends_with("-s1")),
            "no retirement while a slot is not all running: {queued:?}"
        );
        drain(&pool, &provider).await; // the originals are started again

        // Now one decision per slot, and the next pass does not reverse it.
        let first = supervisor.pass(&pool, &provider).await.expect("pass");
        assert_eq!(first.len(), 2, "{first:?}");
        let second = supervisor.pass(&pool, &provider).await.expect("pass");
        assert!(
            second.is_empty(),
            "a slot with a retirement in flight is left alone: {second:?}"
        );
        drain(&pool, &provider).await;
        let names: Vec<String> = provider
            .instances()
            .await
            .expect("list")
            .into_iter()
            .filter(|i| i.name.starts_with("shop-web-01"))
            .map(|i| i.name)
            .collect();
        assert_eq!(
            names.len(),
            2,
            "one instance per replica, none lost: {names:?}"
        );
    }

    /// `shop`'s web service with a load balancer over both replicas (as `network.sync` makes it).
    async fn web_service(pool: &Pool, provider: &Provider) {
        use apelora_domain::network::{Backend, LoadBalancer, network_name};
        use apelora_domain::plan::replica_name;
        let app = apelora_persistence::ensure_application(pool, "shop")
            .await
            .expect("app");
        let address = apelora_persistence::service_address(
            pool,
            app.id,
            PROJECT,
            "web-01",
            &["198.18.100.1".to_owned()],
        )
        .await
        .expect("allocate")
        .expect("address");
        let backend = |r: u8, last: u8| Backend {
            name: format!("r{r}"),
            address: std::net::Ipv4Addr::new(10, 0, 0, last),
            description: format!("{} uuid-{r}", replica_name("shop", "web-01", r)),
        };
        let lb = LoadBalancer {
            listen_address: address.parse().expect("ipv4"),
            module: "web-01".into(),
            backends: vec![backend(1, 2), backend(2, 3)],
            ports: vec![("tcp".into(), 80)],
        };
        provider
            .ensure_load_balancer(PROJECT, &network_name("shop"), &lb, "shop")
            .await
            .expect("load balancer");
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_replica_failing_its_check_is_restarted_once_per_cooldown_and_recorded(pool: Pool) {
        use apelora_domain::plan::replica_name;
        let mock = MockIncus::default();
        let provider = Provider::Mock(mock.clone());
        deploy(&pool, &provider).await;
        web_service(&pool, &provider).await;
        let (r1, r2) = (
            replica_name("shop", "web-01", 1),
            replica_name("shop", "web-01", 2),
        );
        let policy = HealthPolicy {
            restart_cooldown: 3600.0,
            ..HealthPolicy::immediate()
        };
        let mut supervisor = Supervisor::new(Duration::ZERO).with_health(policy);

        // Both pass: healthy, nothing queued.
        assert!(
            supervisor
                .pass(&pool, &provider)
                .await
                .expect("pass")
                .is_empty()
        );
        let health = |pool: Pool| async move {
            apelora_persistence::application_health(&pool, "shop")
                .await
                .expect("health")
                .into_iter()
                .map(|h| (h.instance, h.health))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            health(pool.clone()).await,
            [
                (r1.clone(), "healthy".to_owned()),
                (r2.clone(), "healthy".to_owned())
            ]
        );

        // r2 fails its port check: unhealthy, restarted once and audited.
        mock.net().failing.insert(r2.clone());
        assert_eq!(
            supervisor.pass(&pool, &provider).await.expect("pass"),
            std::slice::from_ref(&r2)
        );
        drain(&pool, &provider).await;
        assert_eq!(mock.net().restarts, std::slice::from_ref(&r2));
        assert_eq!(
            health(pool.clone()).await,
            [
                (r1.clone(), "healthy".to_owned()),
                (r2.clone(), "unhealthy".to_owned())
            ]
        );
        // Still failing: not restarted again within the cooldown.
        assert!(
            supervisor
                .pass(&pool, &provider)
                .await
                .expect("pass")
                .is_empty()
        );
        drain(&pool, &provider).await;
        assert_eq!(mock.net().restarts.len(), 1);
        let audit = recent_audit(&pool, 50).await.expect("audit");
        let restarts: Vec<&str> = audit
            .iter()
            .filter(|e| e.action == "application.replica.restart-unhealthy")
            .map(|e| e.target.as_str())
            .collect();
        assert_eq!(restarts, [format!("{PROJECT}/{r2}")]);

        // Passing again: healthy.
        mock.net().failing.clear();
        supervisor.pass(&pool, &provider).await.expect("pass");
        assert_eq!(
            health(pool.clone()).await,
            [(r1, "healthy".to_owned()), (r2, "healthy".to_owned())]
        );
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn when_every_replica_fails_one_is_restarted_per_pass(pool: Pool) {
        use apelora_domain::plan::replica_name;
        let mock = MockIncus::default();
        let provider = Provider::Mock(mock.clone());
        deploy(&pool, &provider).await;
        web_service(&pool, &provider).await;
        for r in 1..=2 {
            mock.net().failing.insert(replica_name("shop", "web-01", r));
        }
        let mut supervisor = Supervisor::new(Duration::ZERO).with_health(HealthPolicy::immediate());
        assert_eq!(
            supervisor.pass(&pool, &provider).await.expect("pass").len(),
            1
        );
        drain(&pool, &provider).await;
        assert_eq!(
            supervisor.pass(&pool, &provider).await.expect("pass").len(),
            1
        );
        drain(&pool, &provider).await;
        let mut restarted = mock.net().restarts.clone();
        restarted.sort();
        assert_eq!(
            restarted,
            [
                replica_name("shop", "web-01", 1),
                replica_name("shop", "web-01", 2)
            ]
        );
    }

    /// Jobs and audit rows the supervisor wrote for the lost replica `shop-web-01-r2`.
    async fn actions(pool: &Pool) -> (usize, usize) {
        let jobs = recent_jobs(pool, 50)
            .await
            .expect("jobs")
            .into_iter()
            .filter(|j| j.kind == "instance.create" && j.payload["name"] == "shop-web-01-r2")
            .count();
        let audit = recent_audit(pool, 50)
            .await
            .expect("audit")
            .into_iter()
            .filter(|e| e.action == "application.replica.recreate")
            .count();
        (jobs, audit)
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn two_supervisors_on_one_database_only_the_leader_acts(pool: Pool) {
        let provider = Provider::Mock(MockIncus::default());
        deploy(&pool, &provider).await;
        delete(&pool, &provider, "shop-web-01-r2").await;
        let before = actions(&pool).await;

        let (mut lease_a, mut lease_b) = (
            LeaderLease::new(LEADER_LEASE, "worker-a", LEADER_LEASE_SECS),
            LeaderLease::new(LEADER_LEASE, "worker-b", LEADER_LEASE_SECS),
        );
        let (mut a, mut b) = (
            Supervisor::new(Duration::ZERO).with_health(HealthPolicy::immediate()),
            Supervisor::new(Duration::ZERO).with_health(HealthPolicy::immediate()),
        );
        let (la, lb) = (lease_a.tick(&pool).await, lease_b.tick(&pool).await);
        assert!(la.is_some() && lb.is_none(), "one leader: {la:?} {lb:?}");
        // Both run their passes at the same time.
        let (ra, rb) = tokio::join!(
            a.pass_if_leader(&pool, &provider, la),
            b.pass_if_leader(&pool, &provider, lb)
        );
        assert_eq!(
            ra.expect("pass"),
            Some(vec!["shop-web-01-r2".to_owned()]),
            "the leader acts"
        );
        assert_eq!(rb.expect("pass"), None, "the other skips its pass");
        assert_eq!(
            actions(&pool).await,
            (before.0 + 1, before.1 + 1),
            "one job and one audit row"
        );
        // Renewing keeps the roles.
        assert!(lease_a.tick(&pool).await.is_some());
        assert!(lease_b.tick(&pool).await.is_none());
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_leader_that_stops_renewing_is_replaced_and_its_late_writes_are_refused(pool: Pool) {
        let provider = Provider::Mock(MockIncus::default());
        deploy(&pool, &provider).await;
        let mut lease_a = LeaderLease::new(LEADER_LEASE, "worker-a", 1.0);
        let mut lease_b = LeaderLease::new(LEADER_LEASE, "worker-b", 1.0);
        let stale = lease_a.tick(&pool).await.expect("worker-a leads");
        assert!(lease_b.tick(&pool).await.is_none());

        // worker-a stalls (stops renewing); worker-b takes over once the lease expired.
        let started = std::time::Instant::now();
        let current = loop {
            if let Some(l) = lease_b.tick(&pool).await {
                break l;
            }
            assert!(
                started.elapsed() < Duration::from_secs(5),
                "never taken over"
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        };
        assert!(started.elapsed() >= Duration::from_millis(800));
        assert_eq!(current.generation, stale.generation + 1);

        // worker-a wakes up with its old lease and finds a lost replica: its writes are refused.
        delete(&pool, &provider, "shop-web-01-r2").await;
        let before = actions(&pool).await;
        let mut a = Supervisor::new(Duration::ZERO).with_health(HealthPolicy::immediate());
        assert!(matches!(
            a.pass_if_leader(&pool, &provider, Some(stale)).await,
            Err(StoreError::Conflict(_))
        ));
        assert_eq!(
            actions(&pool).await,
            before,
            "nothing written by the stale leader"
        );
        assert!(
            lease_a.tick(&pool).await.is_none(),
            "worker-a learns it lost the lease"
        );

        // The new leader acts.
        let mut b = Supervisor::new(Duration::ZERO).with_health(HealthPolicy::immediate());
        assert_eq!(
            b.pass_if_leader(&pool, &provider, Some(current))
                .await
                .expect("pass"),
            Some(vec!["shop-web-01-r2".to_owned()])
        );
        assert_eq!(actions(&pool).await, (before.0 + 1, before.1 + 1));
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn two_running_supervisors_elect_one_leader_and_restore_a_replica_once(pool: Pool) {
        let provider = Provider::Mock(MockIncus::default());
        deploy(&pool, &provider).await;
        delete(&pool, &provider, "shop-web-01-r2").await;
        let before = actions(&pool).await;
        let run = |holder: &str| {
            tokio::spawn(supervise(
                pool.clone(),
                provider.clone(),
                Duration::from_millis(50),
                Duration::ZERO,
                HealthPolicy::immediate(),
                holder.to_owned(),
                crate::recovery::Recovery::default(),
            ))
        };
        let (a, b) = (run("worker-a"), run("worker-b"));
        let started = std::time::Instant::now();
        while actions(&pool).await == before {
            assert!(started.elapsed() < Duration::from_secs(5), "nobody acted");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        // Several more passes of both.
        tokio::time::sleep(Duration::from_millis(500)).await;
        a.abort();
        b.abort();
        assert_eq!(actions(&pool).await, (before.0 + 1, before.1 + 1));
        let (holder, generation, valid) = apelora_persistence::leader_of(&pool, LEADER_LEASE)
            .await
            .expect("read")
            .expect("a leader");
        assert!(
            ["worker-a", "worker-b"].contains(&holder.as_str()) && generation == 1 && valid,
            "{holder} {generation} {valid}"
        );
    }
}
