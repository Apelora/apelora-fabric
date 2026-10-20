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

use std::time::Duration;

use apelora_domain::network::{Connection, SyncRequest};
use apelora_domain::plan::{Availability, DesiredAppliance, PlanAction, deploy_spec, plan};
use apelora_incus_adapter::Provider;
use apelora_persistence::{Pool, StoreError};
use serde_json::json;

/// After a failed recreation, wait this long before trying again.
const BACKOFF_SECS: i64 = 300;

/// One supervision pass. Returns the instances it queued for recreation or restart.
pub async fn supervise_once(pool: &Pool, provider: &Provider) -> Result<Vec<String>, StoreError> {
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
    let mut recreated = Vec::new();
    for d in deployments {
        let Ok(desired) = serde_json::from_value::<Vec<DesiredAppliance>>(d.desired.clone()) else {
            tracing::warn!(application = %d.application, "supervisor: unreadable deployment record");
            continue;
        };
        let connections: Option<Vec<Connection>> = d
            .connections
            .clone()
            .and_then(|c| serde_json::from_value(c).ok());
        let p = plan(&d.application, &d.project, &desired, &observed);
        let mut queued_here = 0;
        for item in &p.items {
            let Some(da) = desired.iter().find(|x| x.appliance == item.appliance) else {
                continue;
            };
            // Missing → recreate; kept but stopped although it should run → restart.
            let stopped = item.action == PlanAction::Keep
                && da.spec.start
                && observed.iter().any(|i| {
                    i.project == d.project && i.name == item.instance && i.status == "Stopped"
                });
            if item.action != PlanAction::Create && !stopped {
                continue;
            }
            if da.availability != Availability::Stateless {
                tracing::warn!(application = %d.application, instance = %item.instance, availability = ?da.availability,
                    "supervisor: instance is missing or stopped; only stateless replicas are restored automatically");
                continue;
            }
            let kind = if stopped {
                "instance.start"
            } else {
                "instance.create"
            };
            if stopped
                && apelora_persistence::last_lifecycle_job(pool, &d.project, &item.instance)
                    .await?
                    .as_deref()
                    == Some("instance.stop")
            {
                // Stopped on purpose (through Apelora): respected until someone starts it.
                continue;
            }
            if apelora_persistence::job_in_flight(pool, kind, &d.project, &item.instance).await?
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
        if queued_here > 0
            && let (Some(uplink), Some(connections)) = (&d.uplink, &connections)
            && !apelora_persistence::application_job_in_flight(
                pool,
                "network.sync",
                d.application_id,
            )
            .await?
        {
            let payload = serde_json::to_value(SyncRequest::for_deployment(
                &d.application,
                &d.project,
                uplink,
                &desired,
                connections,
                d.service_range.as_deref(),
            ))
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

/// Runs supervision passes every `every` until the task is dropped.
pub async fn supervise(pool: Pool, provider: Provider, every: Duration) {
    loop {
        if let Err(e) = supervise_once(&pool, &provider).await {
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
}
