// SPDX-License-Identifier: AGPL-3.0-only
//! The replica supervisor (Phase 08, M1/M2): restores lost replicas of stateless appliances.
//!
//! It compares each application's last deployment (`application_deployments`) with Incus and
//! queues an ordinary `instance.create` job for a missing stateless replica, unless a job for
//! that instance is already in flight or failed within the back-off window. Instance creation
//! is idempotent, so two supervisors racing still produce one instance. Singleton and stateful
//! appliances are never recreated automatically: they are reported and left to an operator.
//! Every recreation is audited; afterwards the application's network rules are re-synced so
//! the new replica's address joins its address set.

use std::collections::BTreeMap;
use std::time::Duration;

use apelora_domain::network::Connection;
use apelora_domain::plan::{Availability, DesiredAppliance, PlanAction, deploy_spec, plan};
use apelora_incus_adapter::Provider;
use apelora_persistence::{Pool, StoreError};
use serde_json::json;

/// After a failed recreation, wait this long before trying again.
const BACKOFF_SECS: i64 = 300;

/// One supervision pass. Returns the instances it queued for recreation.
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
        for item in p.items.iter().filter(|i| i.action == PlanAction::Create) {
            let Some(da) = desired.iter().find(|x| x.appliance == item.appliance) else {
                continue;
            };
            if da.availability != Availability::Stateless {
                tracing::warn!(application = %d.application, instance = %item.instance, availability = ?da.availability,
                    "supervisor: instance is missing; not recreated automatically (only stateless appliances are)");
                continue;
            }
            if apelora_persistence::job_in_flight(
                pool,
                "instance.create",
                &d.project,
                &item.instance,
            )
            .await?
                || apelora_persistence::job_failed_recently(
                    pool,
                    "instance.create",
                    &d.project,
                    &item.instance,
                    BACKOFF_SECS,
                )
                .await?
            {
                continue;
            }
            let uplink = d.uplink.as_deref().filter(|_| connections.is_some());
            let spec = deploy_spec(&d.application, &d.project, da, item.replica, uplink);
            let payload =
                serde_json::to_value(&spec).map_err(|e| StoreError::Conflict(e.to_string()))?;
            let job = apelora_persistence::enqueue_job(
                pool,
                "instance.create",
                Some(d.application_id),
                &payload,
                None,
            )
            .await?
            .id();
            apelora_persistence::audit(
                pool,
                "supervisor",
                "application.replica.recreate",
                &format!("{}/{}", d.project, item.instance),
                &json!({ "application": d.application, "appliance": item.appliance, "replica": item.replica, "revision": d.revision, "jobId": job }),
            )
            .await?;
            tracing::info!(application = %d.application, instance = %item.instance, %job, "supervisor: recreating a lost stateless replica");
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
            let payload = json!({
                "project": d.project,
                "application": d.application,
                "uplink": uplink,
                "appliances": desired.iter().map(|x| x.appliance.clone()).collect::<Vec<_>>(),
                "connections": connections,
                "replicas": desired.iter().map(|x| (x.appliance.clone(), x.replicas)).collect::<BTreeMap<_, _>>(),
            });
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
        apelora_persistence::record_deployment(
            pool,
            app.id,
            PROJECT,
            1,
            &serde_json::to_value(&d).expect("json"),
            None,
            None,
        )
        .await
        .expect("record");
        drain(pool, provider).await;
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
        sqlx::query("UPDATE jobs SET status = 'failed' WHERE kind = 'instance.create' AND status = 'queued'")
            .execute(&pool)
            .await
            .expect("fail it");
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
        apelora_persistence::record_deployment(&pool, app.id, PROJECT, 2, &json!([]), None, None)
            .await
            .expect("clear");
        delete(&pool, &provider, "shop-web-01").await;
        assert_eq!(
            supervise_once(&pool, &provider).await.expect("pass"),
            Vec::<String>::new()
        );
    }
}
