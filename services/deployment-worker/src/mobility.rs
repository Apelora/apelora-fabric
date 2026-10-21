// SPDX-License-Identifier: AGPL-3.0-only
//! The `instance.move` job (docs/design/mobility-and-ha.md, slice A): moves one Apelora
//! instance to another member. Level-triggered like the lifecycle jobs: every step re-reads
//! where the instance and its volumes are, so a reclaimed job continues where the last one
//! stopped. A live move is one Incus operation. Otherwise the instance is stopped, its volumes
//! are copied to the new member, it is moved and started again if it was running; the old
//! copies of the volumes are deleted only once it runs on the new member. When copying or
//! moving fails, it is started again where it was.

use std::time::Duration;

use apelora_domain::InstanceAction;
use apelora_incus_adapter::{AdapterError, OperationOutcome, Provider};
use apelora_persistence::{Outcome, Pool, StoreError, audit, set_volume_member};
use serde::Deserialize;
use serde_json::json;

const BUDGET: Duration = Duration::from_secs(900);
const MAX_STEPS: usize = 8;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VolumeRef {
    pub pool: String,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveRequest {
    pub project: String,
    pub name: String,
    pub application: String,
    pub from: String,
    pub target: String,
    pub live: bool,
    /// It was running when the move was asked for: start it on the new member.
    pub start_after: bool,
    #[serde(default)]
    pub volumes: Vec<VolumeRef>,
}

async fn wait(provider: &Provider, op: &str, what: &str) -> Result<(), AdapterError> {
    match provider.wait_operation(op, BUDGET).await? {
        OperationOutcome::Succeeded => Ok(()),
        OperationOutcome::Failed(e) => Err(AdapterError::Api {
            status: 500,
            message: format!("{what}: {e}"),
        }),
        OperationOutcome::StillRunning => Err(AdapterError::Api {
            status: 504,
            message: format!("{what} is still running"),
        }),
    }
}

async fn change(
    provider: &Provider,
    r: &MoveRequest,
    action: InstanceAction,
) -> Result<(), AdapterError> {
    let op = provider
        .change_state(&r.project, &r.name, action, false)
        .await?;
    wait(provider, &op, &format!("{} {}", action.as_str(), r.name)).await
}

/// Copies every volume to the target member (those already there are left), then moves the
/// stopped instance.
async fn relocate(provider: &Provider, r: &MoveRequest) -> Result<(), AdapterError> {
    for v in &r.volumes {
        if provider
            .volume(&r.project, &v.pool, Some(&r.target), &v.name)
            .await?
            .is_none()
        {
            provider
                .copy_volume(
                    &r.project,
                    &v.pool,
                    Some(&r.from),
                    &v.name,
                    &r.application,
                    None,
                    &v.pool,
                    Some(&r.target),
                    &v.name,
                    None,
                )
                .await?;
        }
    }
    let op = provider
        .move_instance(&r.project, &r.name, &r.target, false)
        .await?;
    wait(provider, &op, &format!("moving {}", r.name)).await
}

pub async fn run(
    pool: &Pool,
    provider: &Provider,
    payload: &serde_json::Value,
) -> Result<Outcome, StoreError> {
    let r: MoveRequest = match serde_json::from_value(payload.clone()) {
        Ok(r) => r,
        Err(e) => return Ok(Outcome::Rejected(format!("invalid payload: {e}"))),
    };
    for _ in 0..MAX_STEPS {
        let Some(now) = (match provider.instance(&r.project, &r.name).await {
            Ok(s) => s,
            Err(e) => return Ok(Outcome::Failed(e.to_string())),
        }) else {
            return Ok(Outcome::Rejected(format!(
                "instance {}/{} does not exist",
                r.project, r.name
            )));
        };
        if !now.managed {
            return Ok(Outcome::Rejected(format!(
                "{} was not created by Apelora",
                r.name
            )));
        }
        tracing::debug!(instance = %format!("{}/{}", r.project, r.name), member = %now.location, status = %now.status, from = %r.from, to = %r.target, live = r.live, "move: deciding the next step");
        if now.location == r.target {
            if r.start_after && now.status != "Running" {
                if let Err(e) = change(provider, &r, InstanceAction::Start).await {
                    return Ok(Outcome::Failed(format!(
                        "{} moved to {} but did not start: {e}; its old volumes are kept on {}",
                        r.name, r.target, r.from
                    )));
                }
                continue;
            }
            // It runs (or was not running) on the new member: the old copies can go.
            for v in &r.volumes {
                match provider
                    .delete_volume(&r.project, &v.pool, Some(&r.from), &v.name, &r.application)
                    .await
                {
                    Ok(_) => {
                        set_volume_member(pool, &r.project, &v.pool, &v.name, &r.target).await?
                    }
                    Err(e) => {
                        tracing::warn!(volume = %v.name, member = %r.from, error = %e, "move: the old copy of a volume was not deleted")
                    }
                }
            }
            tracing::info!(instance = %format!("{}/{}", r.project, r.name), from = %r.from, to = %r.target, live = r.live, volumes = r.volumes.len(), "move: finished");
            audit(pool, "worker", "instance.moved", &format!("{}/{}", r.project, r.name),
                &json!({ "application": r.application, "from": r.from, "to": r.target, "live": r.live, "volumes": r.volumes.iter().map(|v| v.name.clone()).collect::<Vec<_>>() }))
                .await?;
            return Ok(Outcome::Succeeded);
        }
        if now.location != r.from {
            return Ok(Outcome::Rejected(format!(
                "{} is on {}, neither {} nor {}",
                r.name, now.location, r.from, r.target
            )));
        }
        if r.live && now.status == "Running" {
            let moved = async {
                let op = provider
                    .move_instance(&r.project, &r.name, &r.target, true)
                    .await?;
                wait(provider, &op, &format!("moving {} live", r.name)).await
            }
            .await;
            if let Err(e) = moved {
                return Ok(Outcome::Failed(e.to_string()));
            }
            continue;
        }
        if now.status == "Running" {
            if let Err(e) = change(provider, &r, InstanceAction::Stop).await {
                return Ok(Outcome::Failed(e.to_string()));
            }
            continue;
        }
        if let Err(e) = relocate(provider, &r).await {
            // Back to where it was, running if it was.
            if r.start_after
                && let Err(start) = change(provider, &r, InstanceAction::Start).await
            {
                tracing::warn!(instance = %r.name, error = %start, "move failed and the instance did not start again where it was");
            }
            tracing::warn!(instance = %format!("{}/{}", r.project, r.name), from = %r.from, to = %r.target, error = %e, "move: relocation failed; the instance stays where it was");
            return Ok(Outcome::Failed(format!(
                "moving {} to {} failed: {e}; it stays on {}",
                r.name, r.target, r.from
            )));
        }
    }
    Ok(Outcome::Failed(format!(
        "moving {} did not finish in {MAX_STEPS} steps",
        r.name
    )))
}

#[cfg(test)]
mod tests {
    use super::*;
    use apelora_incus_adapter::MockIncus;
    use apelora_persistence::{
        application_volumes, enqueue_job, ensure_application, get_job, recent_audit,
    };

    async fn drain(pool: &Pool, provider: &Provider) {
        while crate::run_once(pool, provider, "test").await.expect("run") {}
    }

    async fn job(pool: &Pool, kind: &str, payload: serde_json::Value) -> uuid::Uuid {
        enqueue_job(pool, kind, None, &payload, None)
            .await
            .expect("enqueue")
            .id()
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_container_with_a_volume_stops_its_volume_is_copied_and_it_starts_on_the_new_member(
        pool: Pool,
    ) {
        let m = MockIncus::default();
        let provider = Provider::Mock(m.clone());
        ensure_application(&pool, "shop")
            .await
            .expect("application");
        job(&pool, "instance.create", json!({ "project": "apelora", "name": "shop-db-01", "kind": "container",
            "source": { "protocol": "oci", "server": "https://registry.example", "alias": "postgres:18" }, "start": true,
            "owner": { "application": "shop", "module": "db-01", "replica": 1 },
            "volumes": [{ "name": "data", "sizeMib": 1024, "path": "/data", "pool": "local" }] })).await;
        drain(&pool, &provider).await;
        let id = job(
            &pool,
            "instance.move",
            json!({ "project": "apelora", "name": "shop-db-01", "application": "shop",
            "from": "mock-member-01", "target": "mock-member-02", "live": false, "startAfter": true,
            "volumes": [{ "pool": "local", "name": "shop-db-01-data" }] }),
        )
        .await;
        drain(&pool, &provider).await;
        let j = get_job(&pool, id).await.expect("job");
        assert_eq!(j.status, "succeeded", "{:?}", j.last_error);
        let now = provider
            .instance("apelora", "shop-db-01")
            .await
            .expect("read")
            .expect("exists");
        assert_eq!(
            (now.location.as_str(), now.status.as_str()),
            ("mock-member-02", "Running")
        );
        let at = |member: &'static str| {
            provider.volume("apelora", "local", Some(member), "shop-db-01-data")
        };
        assert!(
            at("mock-member-02").await.expect("read").is_some(),
            "copied to the new member"
        );
        assert!(
            at("mock-member-01").await.expect("read").is_none(),
            "the old copy is gone once it runs there"
        );
        assert_eq!(
            application_volumes(&pool, "shop").await.expect("volumes")[0].member,
            "mock-member-02"
        );
        assert!(
            recent_audit(&pool, 10)
                .await
                .expect("audit")
                .iter()
                .any(|e| e.action == "instance.moved")
        );
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_vm_with_live_migration_moves_running_and_a_failed_copy_leaves_it_running_where_it_was(
        pool: Pool,
    ) {
        let m = MockIncus::default();
        let provider = Provider::Mock(m.clone());
        ensure_application(&pool, "shop")
            .await
            .expect("application");
        job(&pool, "instance.create", json!({ "project": "apelora", "name": "shop-vm-01", "kind": "virtual-machine",
            "source": { "protocol": "simplestreams", "server": "https://images.example", "alias": "debian/13" }, "start": true,
            "config": { "migration.stateful": "true" },
            "owner": { "application": "shop", "module": "vm-01", "replica": 1 } })).await;
        drain(&pool, &provider).await;
        let id = job(&pool, "instance.move", json!({ "project": "apelora", "name": "shop-vm-01", "application": "shop",
            "from": "mock-member-01", "target": "mock-member-03", "live": true, "startAfter": true })).await;
        drain(&pool, &provider).await;
        assert_eq!(get_job(&pool, id).await.expect("job").status, "succeeded");
        let now = provider
            .instance("apelora", "shop-vm-01")
            .await
            .expect("read")
            .expect("exists");
        assert_eq!(
            (now.location.as_str(), now.status.as_str()),
            ("mock-member-03", "Running")
        );

        // A volume that cannot be copied: the move fails and the instance runs where it was.
        job(&pool, "instance.create", json!({ "project": "apelora", "name": "shop-db-01", "kind": "container",
            "source": { "protocol": "oci", "server": "https://registry.example", "alias": "postgres:18" }, "start": true,
            "owner": { "application": "shop", "module": "db-01", "replica": 1 },
            "volumes": [{ "name": "data", "sizeMib": 1024, "path": "/data", "pool": "local" }] })).await;
        drain(&pool, &provider).await;
        m.storage().fail_create.insert("shop-db-01-data".into());
        let id = job(
            &pool,
            "instance.move",
            json!({ "project": "apelora", "name": "shop-db-01", "application": "shop",
            "from": "mock-member-01", "target": "mock-member-02", "live": false, "startAfter": true,
            "volumes": [{ "pool": "local", "name": "shop-db-01-data" }] }),
        )
        .await;
        sqlx::query("UPDATE jobs SET max_attempts = 1 WHERE id = $1")
            .bind(id)
            .execute(&pool)
            .await
            .expect("cap");
        drain(&pool, &provider).await;
        let j = get_job(&pool, id).await.expect("job");
        assert_eq!(j.status, "failed");
        assert!(
            j.last_error
                .unwrap_or_default()
                .contains("stays on mock-member-01")
        );
        let now = provider
            .instance("apelora", "shop-db-01")
            .await
            .expect("read")
            .expect("exists");
        assert_eq!(
            (now.location.as_str(), now.status.as_str()),
            ("mock-member-01", "Running")
        );
    }
}
