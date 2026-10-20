// SPDX-License-Identifier: AGPL-3.0-only
//! The replica supervisor (Phase 08, M1/M2): restores lost replicas of stateless appliances.
//!
//! It compares each application's last deployment (`application_deployments`) with Incus and
//! queues an ordinary `instance.create` job for a missing stateless replica — or
//! `instance.start` for one that stopped although it should run, unless it was stopped on
//! purpose through Apelora (its latest lifecycle job is a stop) — unless a job for
//! that instance is already in flight or failed within the back-off window. Instance creation
//! is idempotent, so two supervisors racing still produce one instance. Singleton and stateful
//! appliances are never recreated automatically: they are reported and left to an operator.
//! Every recreation is audited; afterwards the application's network rules are re-synced so
//! the new replica's address joins its address set.
//!
//! A member that fails (Incus reports it Offline for longer than the recovery delay) keeps its
//! instances: Incus cannot move them, and its healing stays off. A stateless replica whose
//! every instance is on a failed member gets a *substitute* on another member
//! (`<replica>-s<n>`, same labels). When the member is back, the slot holds two instances and
//! the one that keeps replicas apart stays; the other is retired (deleted). Both are audited.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use apelora_domain::Instance;
use apelora_domain::network::{Connection, SyncRequest};
use apelora_domain::plan::{
    Availability, DesiredAppliance, PlanAction, deploy_spec, plan, slot_instances, substitute_name,
};
use apelora_incus_adapter::Provider;
use apelora_persistence::{Pool, StoreError};
use serde_json::json;

/// After a failed recreation, wait this long before trying again.
const BACKOFF_SECS: i64 = 300;

/// A failed network sync is queued again after this many seconds.
const SYNC_RETRY_SECS: f64 = 60.0;

/// Remembers since when each member has been offline, so a short outage (a reboot, a network
/// blip) does not trigger recovery: hysteresis across passes.
#[derive(Debug, Default)]
pub struct Supervisor {
    recover_after: Duration,
    offline_since: HashMap<String, Instant>,
}

/// One supervision pass with no memory: offline members count as failed at once.
pub async fn supervise_once(pool: &Pool, provider: &Provider) -> Result<Vec<String>, StoreError> {
    Supervisor::new(Duration::ZERO).pass(pool, provider).await
}

impl Supervisor {
    pub fn new(recover_after: Duration) -> Self {
        Supervisor {
            recover_after,
            offline_since: HashMap::new(),
        }
    }

    /// One supervision pass. Returns the instances it queued for creation, restart or retirement.
    pub async fn pass(
        &mut self,
        pool: &Pool,
        provider: &Provider,
    ) -> Result<Vec<String>, StoreError> {
        let deployments = apelora_persistence::list_deployments(pool).await?;
        if deployments.is_empty() {
            return Ok(Vec::new());
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
        let members = provider.cluster_members().await.unwrap_or_default();
        let unavailable: Vec<String> = members
            .iter()
            .filter(|m| m.status != "Online")
            .map(|m| m.name.clone())
            .collect();
        let now = Instant::now();
        self.offline_since.retain(|name, _| {
            members
                .iter()
                .any(|m| &m.name == name && m.status == "Offline")
        });
        for m in members.iter().filter(|m| m.status == "Offline") {
            self.offline_since.entry(m.name.clone()).or_insert(now);
        }
        let failed: Vec<String> = self
            .offline_since
            .iter()
            .filter(|(_, since)| now.duration_since(**since) >= self.recover_after)
            .map(|(name, _)| name.clone())
            .collect();
        let mut recreated = Vec::new();
        for d in deployments {
            let Ok(desired) = serde_json::from_value::<Vec<DesiredAppliance>>(d.desired.clone())
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
            let mut queued_here = 0;
            // Replica slots first: substitutes for replicas stranded on a failed member, and
            // retirement of the extra instance once that member is back.
            let mut handled: Vec<String> = Vec::new();
            for da in desired
                .iter()
                .filter(|x| x.availability == Availability::Stateless)
            {
                for replica in 1..=da.replicas.max(1) {
                    let slot = slot_instances(
                        &d.application,
                        &d.project,
                        &da.appliance,
                        replica,
                        &observed,
                    );
                    if slot.is_empty() {
                        continue;
                    }
                    if slot.iter().all(|i| failed.contains(&i.location)) {
                        handled.extend(slot.iter().map(|i| i.name.clone()));
                        let Some(name) = (1..=9)
                            .map(|n| substitute_name(&d.application, &da.appliance, replica, n))
                            .find(|n| {
                                !observed
                                    .iter()
                                    .any(|i| i.project == d.project && &i.name == n)
                            })
                        else {
                            tracing::warn!(application = %d.application, appliance = %da.appliance, replica, "supervisor: no substitute name left");
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
                            continue;
                        }
                        let uplink = d.uplink.as_deref().filter(|_| connections.is_some());
                        let mut spec = deploy_spec(&d.application, &d.project, da, replica, uplink);
                        spec.name = name.clone();
                        let payload = serde_json::to_value(&spec)
                            .map_err(|e| StoreError::Conflict(e.to_string()))?;
                        let job = apelora_persistence::enqueue_job(
                            pool,
                            "instance.create",
                            Some(d.application_id),
                            &payload,
                            None,
                        )
                        .await?
                        .id();
                        let stranded: Vec<String> = slot
                            .iter()
                            .map(|i| format!("{} on {}", i.name, i.location))
                            .collect();
                        apelora_persistence::audit(
                        pool,
                        "supervisor",
                        "application.replica.substitute",
                        &format!("{}/{name}", d.project),
                        &json!({ "application": d.application, "appliance": da.appliance, "replica": replica, "stranded": stranded, "jobId": job }),
                    )
                    .await?;
                        tracing::info!(application = %d.application, instance = %name, ?stranded, "supervisor: substituting a replica stranded on a failed member");
                        recreated.push(name);
                        queued_here += 1;
                    } else if slot.len() > 1
                        && slot.iter().all(|i| !unavailable.contains(&i.location))
                    {
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
                            let job = apelora_persistence::enqueue_job(
                                pool,
                                "instance.delete",
                                Some(d.application_id),
                                &json!({ "project": d.project, "name": other.name }),
                                None,
                            )
                            .await?
                            .id();
                            apelora_persistence::audit(
                            pool,
                            "supervisor",
                            "application.replica.retire",
                            &format!("{}/{}", d.project, other.name),
                            &json!({ "application": d.application, "appliance": da.appliance, "replica": replica, "kept": keep.name, "jobId": job }),
                        )
                        .await?;
                            tracing::info!(application = %d.application, instance = %other.name, kept = %keep.name, "supervisor: retiring a duplicate replica");
                            recreated.push(other.name.clone());
                            queued_here += 1;
                        }
                    }
                }
            }
            for item in p.items.iter().filter(|i| !handled.contains(&i.instance)) {
                let Some(da) = desired.iter().find(|x| x.appliance == item.appliance) else {
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
                    if !apelora_persistence::job_in_flight(pool, kind, &d.project, &item.instance)
                        .await?
                    {
                        tracing::warn!(application = %d.application, instance = %item.instance, availability = ?da.availability,
                        "supervisor: instance is missing or stopped; only stateless replicas are restored automatically");
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
                    continue;
                }
                let payload = if stopped {
                    json!({ "project": d.project, "name": item.instance })
                } else {
                    let uplink = d.uplink.as_deref().filter(|_| connections.is_some());
                    let spec = deploy_spec(&d.application, &d.project, da, item.replica, uplink);
                    serde_json::to_value(&spec).map_err(|e| StoreError::Conflict(e.to_string()))?
                };
                let job = apelora_persistence::enqueue_job(
                    pool,
                    kind,
                    Some(d.application_id),
                    &payload,
                    None,
                )
                .await?
                .id();
                let action = if stopped {
                    "application.replica.restart"
                } else {
                    "application.replica.recreate"
                };
                apelora_persistence::audit(
                pool,
                "supervisor",
                action,
                &format!("{}/{}", d.project, item.instance),
                &json!({ "application": d.application, "appliance": item.appliance, "replica": item.replica, "revision": d.revision, "jobId": job }),
            )
            .await?;
                tracing::info!(application = %d.application, instance = %item.instance, %job, kind, "supervisor: restoring a stateless replica");
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
                apelora_persistence::enqueue_job(
                    pool,
                    "network.sync",
                    Some(d.application_id),
                    &payload,
                    None,
                )
                .await?;
            }
        }
        Ok(recreated)
    }
}

/// Of a slot's instances (all reachable), the one to keep: running, on the member with the
/// fewest other instances of the same appliance (replicas apart), then the original name.
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
                            a.application == b.application && a.appliance == b.appliance
                        })
            })
            .count()
    };
    slot.iter()
        .filter(|i| i.status == "Running")
        .min_by_key(|i| (siblings(i), i.name.len(), i.name.clone()))
        .copied()
}

/// Runs supervision passes every `every` until the task is dropped; a member counts as failed
/// once it has been offline for `recover_after`.
pub async fn supervise(pool: Pool, provider: Provider, every: Duration, recover_after: Duration) {
    let mut supervisor = Supervisor::new(recover_after);
    loop {
        if let Err(e) = supervisor.pass(&pool, &provider).await {
            tracing::error!(error = %e, "supervisor pass failed");
        }
        tokio::time::sleep(every).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apelora_domain::{ImageProtocol, ImageSource, InstanceKind, InstanceSpec};
    use apelora_incus_adapter::MockIncus;
    use apelora_persistence::{enqueue_job, recent_audit, recent_jobs};

    const PROJECT: &str = "apelora";

    fn desired(appliance: &str, replicas: u8, availability: Availability) -> DesiredAppliance {
        DesiredAppliance {
            appliance: appliance.into(),
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
            },
            replicas,
            availability,
            internet: Default::default(),
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
            owner.map(|o| (o.application, o.appliance, o.replica)),
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
    async fn stateful_appliances_are_not_recreated_and_failures_back_off(pool: Pool) {
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
}
