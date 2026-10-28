// SPDX-License-Identifier: AGPL-3.0-only
//! Rolling updates (docs/design/rolling-updates.md): the phases of an `update` operation that
//! queue no instance job of their own, and what a failed phase leaves to undo. A singleton or
//! stateful module is not replaced but rebuilt in place (`rebuild`, an `instance.rebuild`). Each replica of a stateless module whose image or
//! settings changed goes through four phases: `create` (an `instance.create` of its update,
//! `<replica>-u<n>`), `healthy` (here: the update passes its service's health check for a
//! while), `drain` (here: the old replica leaves its service address) and `delete` (an
//! `instance.delete` of the old replica). The coordinator in [`crate::operations`] runs them in
//! order, so the next replica starts only once the previous one is done, and a failure stops
//! the update with the earlier replicas still serving.
//!
//! Progress (when a phase started, since when the update passes) is kept in the step's payload,
//! which the operation row stores, so a worker restart resumes where it was.

use apelora_domain::network::network_name;
use apelora_incus_adapter::Provider;
use apelora_persistence::drains::{Drain, drain as drain_instances, undrain};
use apelora_persistence::lifecycle::OperationRow;
use apelora_persistence::{
    ClaimedJob, DeploymentRecord, JobOrigin, Pool, StoreError, enqueue_job_from, job_status,
};
use serde_json::{Value, json};
use time::OffsetDateTime;

use crate::operations::Step;

/// How long a new replica must pass, how long it may take, and how long an old replica keeps
/// running after it left its service address (`APELORA_UPDATE_*_SECS`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UpdatePolicy {
    pub healthy_secs: i64,
    pub timeout_secs: i64,
    pub drain_secs: i64,
}

impl Default for UpdatePolicy {
    fn default() -> Self {
        UpdatePolicy {
            healthy_secs: 20,
            timeout_secs: 300,
            drain_secs: 5,
        }
    }
}

impl UpdatePolicy {
    /// From the environment; a value that is not a whole number of seconds keeps its default.
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Self {
        let d = UpdatePolicy::default();
        let secs = |key: &str, default: i64| {
            get(key)
                .and_then(|v| v.trim().parse::<i64>().ok())
                .filter(|v| (0..=86_400).contains(v))
                .unwrap_or(default)
        };
        UpdatePolicy {
            healthy_secs: secs("APELORA_UPDATE_HEALTHY_SECS", d.healthy_secs),
            timeout_secs: secs("APELORA_UPDATE_TIMEOUT_SECS", d.timeout_secs),
            drain_secs: secs("APELORA_UPDATE_DRAIN_SECS", d.drain_secs),
        }
    }
}

/// What a look at a `healthy` or `drain` step found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    Waiting,
    Passed,
    Failed(String),
}

fn now() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp()
}

fn field<'a>(step: &'a Step, key: &str) -> Option<&'a Value> {
    step.payload.as_ref()?.get(key)
}

fn int(step: &Step, key: &str) -> Option<i64> {
    field(step, key)?.as_i64()
}

fn set(step: &mut Step, key: &str, value: Value) {
    let p = step.payload.get_or_insert_with(|| json!({}));
    if !p.is_object() {
        *p = json!({});
    }
    p[key] = value;
}

async fn deployment(
    pool: &Pool,
    op: &OperationRow,
) -> Result<Option<DeploymentRecord>, StoreError> {
    Ok(apelora_persistence::list_deployments(pool)
        .await?
        .into_iter()
        .find(|d| d.application_id == op.application_id && d.project == op.project))
}

/// Queues a network sync of the application from its deployment record and remembers the job
/// on the step. `false` when its connections are not enforced: there is no service address.
async fn sync(
    pool: &Pool,
    job: &ClaimedJob,
    op: &OperationRow,
    step: &mut Step,
    key: &str,
) -> Result<bool, StoreError> {
    let Some(request) = deployment(pool, op)
        .await?
        .as_ref()
        .and_then(crate::supervisor::sync_request_for)
    else {
        return Ok(false);
    };
    let payload = serde_json::to_value(request).map_err(|e| StoreError::Conflict(e.to_string()))?;
    let n = int(step, "syncs").unwrap_or(0) + 1;
    let mut conn = pool.acquire().await?;
    let id = enqueue_job_from(
        &mut conn,
        "network.sync",
        Some(op.application_id),
        &payload,
        Some(&format!("{key}:sync:{n}")),
        &JobOrigin {
            request_id: job.request_id.as_deref(),
            queued_by: Some(&op.requested_by),
            retry_of: None,
        },
    )
    .await?
    .id();
    step.job = Some(id);
    set(step, "syncs", json!(n));
    Ok(true)
}

/// A `healthy` step: the new replica runs and its service's load balancer has reported it
/// online for `healthy_secs` in a row. A module without a service address is checked only for
/// running. Fails once `timeout_secs` have passed since the phase began.
pub(crate) async fn healthy(
    pool: &Pool,
    provider: &Provider,
    job: &ClaimedJob,
    op: &OperationRow,
    step: &mut Step,
    key: &str,
    policy: UpdatePolicy,
) -> Result<Check, StoreError> {
    let t = now();
    let Some(since) = int(step, "since") else {
        // First look: the new replica becomes a backend of its service address.
        set(step, "since", json!(t));
        let enforced = sync(pool, job, op, step, key).await?;
        set(step, "enforced", json!(enforced));
        return Ok(Check::Waiting);
    };
    let waited = t - since;
    let name = step.name.clone();
    let late = |last: &str| {
        if waited >= policy.timeout_secs {
            Check::Failed(format!(
                "{name} was not healthy within {} s ({last})",
                policy.timeout_secs
            ))
        } else {
            Check::Waiting
        }
    };
    let status = match provider.instance(&step.project, &step.name).await {
        Ok(Some(i)) => i.status,
        Ok(None) => return Ok(Check::Failed(format!("{} no longer exists", step.name))),
        Err(e) => {
            tracing::warn!(instance = %format!("{}/{}", step.project, step.name), error = %e, "update: cannot read the new replica; looking again");
            return Ok(late("Incus could not be read"));
        }
    };
    if status != "Running" {
        let last = format!("it is {status}");
        return Ok(late(&last));
    }
    if field(step, "enforced") != Some(&json!(true)) {
        return Ok(Check::Passed);
    }
    let Some(d) = deployment(pool, op).await? else {
        return Ok(Check::Failed(
            "the application is no longer deployed".into(),
        ));
    };
    let address = apelora_persistence::service_addresses(pool, op.application_id, &op.project)
        .await?
        .into_iter()
        .find(|(m, _)| *m == step.module)
        .map(|(_, a)| a);
    let Some(address) = address else {
        // Nothing connects to it and it is not published: running is all that can be checked.
        return Ok(Check::Passed);
    };
    // A sync Incus refused (e.g. while a member is unreachable) is asked for again.
    if let Some(id) = step.job
        && job_status(pool, id).await?.0 == "failed"
    {
        sync(pool, job, op, step, key).await?;
    }
    let backend = match provider
        .load_balancer_health(&op.project, &network_name(&d.application), &address)
        .await
    {
        Ok(backends) => backends
            .into_iter()
            .find(|(b, _)| {
                crate::health::backend_instance(&d.application, &step.module, b).as_deref()
                    == Some(step.name.as_str())
            })
            .map(|(_, s)| s),
        Err(e) => {
            tracing::debug!(%address, error = %e, "update: cannot read the load balancer's state");
            None
        }
    };
    if backend.as_deref() == Some("online") {
        let from = match int(step, "onlineSince") {
            Some(f) => f,
            None => {
                set(step, "onlineSince", json!(t));
                t
            }
        };
        if t - from >= policy.healthy_secs {
            return Ok(Check::Passed);
        }
        return Ok(Check::Waiting);
    }
    if let Some(p) = step.payload.as_mut().and_then(Value::as_object_mut) {
        p.remove("onlineSince");
    }
    let last = match backend {
        Some(s) => format!("its health check reports {s}"),
        None => "not yet a backend of its service address".into(),
    };
    Ok(late(&last))
}

/// A `drain` step: the old replica leaves its service address (the same drain an evacuation
/// uses), then keeps running `drain_secs` for requests in flight.
pub(crate) async fn drain(
    pool: &Pool,
    provider: &Provider,
    job: &ClaimedJob,
    op: &OperationRow,
    step: &mut Step,
    key: &str,
    policy: UpdatePolicy,
) -> Result<Check, StoreError> {
    let t = now();
    if int(step, "since").is_none() {
        let location = match provider.instance(&step.project, &step.name).await {
            Ok(Some(i)) => i.location,
            // Already gone: nothing left to drain.
            Ok(None) => return Ok(Check::Passed),
            Err(e) => {
                tracing::warn!(instance = %format!("{}/{}", step.project, step.name), error = %e, "update: cannot read the old replica; looking again");
                return Ok(Check::Waiting);
            }
        };
        drain_instances(
            pool,
            &[Drain {
                project: step.project.clone(),
                instance: step.name.clone(),
                from_member: location.clone(),
            }],
            &op.requested_by,
        )
        .await?;
        set(step, "since", json!(t));
        set(step, "from", json!(location));
        let enforced = sync(pool, job, op, step, key).await?;
        set(step, "enforced", json!(enforced));
        return Ok(Check::Waiting);
    }
    if field(step, "enforced") != Some(&json!(true)) {
        return Ok(Check::Passed);
    }
    let Some(id) = step.job else {
        return Ok(Check::Passed);
    };
    let (status, _, error) = job_status(pool, id).await?;
    match status.as_str() {
        "succeeded" => {
            let left = match int(step, "leftAt") {
                Some(l) => l,
                None => {
                    set(step, "leftAt", json!(t));
                    t
                }
            };
            Ok(if t - left >= policy.drain_secs {
                Check::Passed
            } else {
                Check::Waiting
            })
        }
        "failed" => Ok(Check::Failed(format!(
            "{} could not leave its service address: {}",
            step.name,
            error.unwrap_or_else(|| "the network sync failed".into())
        ))),
        _ => Ok(Check::Waiting),
    }
}

/// Undoes what a failed phase of an update left half done, and says so: a new replica that
/// never became healthy is deleted; an old replica that was leaving its service address
/// returns to it. A failed `delete` leaves both serving.
pub(crate) async fn stopped(
    pool: &Pool,
    job: &ClaimedJob,
    op: &OperationRow,
    action: &str,
    steps: &mut [Step],
    key: &str,
) -> Result<String, StoreError> {
    let mut notes = Vec::new();
    for s in steps.iter_mut() {
        match action {
            "create" | "healthy" => {
                let mut conn = pool.acquire().await?;
                enqueue_job_from(
                    &mut conn,
                    "instance.delete",
                    Some(op.application_id),
                    &json!({ "project": s.project, "name": s.name }),
                    Some(&format!("{key}:undo:{}", s.name)),
                    &JobOrigin {
                        request_id: job.request_id.as_deref(),
                        queued_by: Some(&op.requested_by),
                        retry_of: None,
                    },
                )
                .await?;
                notes.push(format!("{} is removed", s.name));
            }
            // A rebuild that failed (after its attempts) leaves the instance stopped: it is
            // started again as it is, so the module serves rather than stays down.
            "rebuild" => {
                let mut conn = pool.acquire().await?;
                enqueue_job_from(
                    &mut conn,
                    "instance.start",
                    Some(op.application_id),
                    &json!({ "project": s.project, "name": s.name }),
                    Some(&format!("{key}:undo:{}", s.name)),
                    &JobOrigin {
                        request_id: job.request_id.as_deref(),
                        queued_by: Some(&op.requested_by),
                        retry_of: None,
                    },
                )
                .await?;
                notes.push(format!("{} is started again as it is", s.name));
            }
            "drain" => {
                if let Some(from) = field(s, "from").and_then(Value::as_str).map(str::to_owned) {
                    undrain(
                        pool,
                        &[Drain {
                            project: s.project.clone(),
                            instance: s.name.clone(),
                            from_member: from,
                        }],
                    )
                    .await?;
                    sync(pool, job, op, s, key).await?;
                }
                notes.push(format!("{} returns to its service address", s.name));
            }
            _ => {}
        }
    }
    notes.push("the replicas not yet replaced keep serving".into());
    Ok(notes.join("; "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use apelora_domain::network::Connection;
    use apelora_domain::plan::{
        Availability, DesiredModule, deploy_spec, replica_name, update_name,
    };
    use apelora_domain::{ImageProtocol, ImageSource, InstanceKind, InstanceSpec};
    use apelora_incus_adapter::MockIncus;
    use apelora_persistence::lifecycle::{NewOperation, create_operation, get_operation};

    const PROJECT: &str = "apelora";

    fn module(name: &str, replicas: u8, availability: Availability, mode: &str) -> DesiredModule {
        DesiredModule {
            module: name.into(),
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
                config: [("environment.MODE".to_owned(), mode.to_owned())].into(),
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
            readiness: None,
            dns_resolver: false,
        }
    }

    async fn run_jobs(pool: &Pool, provider: &Provider) {
        while crate::run_once(pool, provider, "test-worker")
            .await
            .expect("run")
        {}
    }

    /// `shop`: two web replicas (stateless, behind a service address the database connects to)
    /// and a database, deployed as `blue`; the deployment record then asks for `green` web
    /// replicas, as a deployment of a changed version does. Returns the application's id.
    async fn deployed(pool: &Pool, provider: &Provider) -> uuid::Uuid {
        let app = apelora_persistence::ensure_application(pool, "shop")
            .await
            .expect("app");
        let blue = [
            module("web-01", 2, Availability::Stateless, "blue"),
            module("db-01", 1, Availability::Stateful, "blue"),
        ];
        for d in &blue {
            for r in 1..=d.replicas {
                let spec = deploy_spec("shop", PROJECT, d, r, Some("incusbr0"));
                apelora_persistence::enqueue_job(
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
        run_jobs(pool, provider).await;
        apelora_persistence::service_address(
            pool,
            app.id,
            PROJECT,
            "web-01",
            &["198.18.100.1".to_owned()],
        )
        .await
        .expect("allocate")
        .expect("address");
        let green = [
            module("web-01", 2, Availability::Stateless, "green"),
            blue[1].clone(),
        ];
        let connections = vec![Connection {
            from: "db-01".into(),
            to: "web-01".into(),
            transport: "tcp".into(),
            port: Some(80),
        }];
        apelora_persistence::record_deployment(
            pool,
            &apelora_persistence::NewDeployment {
                application_id: app.id,
                project: PROJECT,
                revision: 2,
                desired: &serde_json::to_value(&green).expect("json"),
                connections: Some(&serde_json::to_value(&connections).expect("json")),
                uplink: Some("incusbr0"),
                service_range: Some("198.18.100.0/28"),
                published: None,
            },
        )
        .await
        .expect("record");
        app.id
    }

    /// The phases the control plane queues for replacing both web replicas.
    fn update_phases() -> Value {
        let green = module("web-01", 2, Availability::Stateless, "green");
        let mut phases = Vec::new();
        for r in 1..=2 {
            let (old, new) = (
                replica_name("shop", "web-01", r),
                update_name("shop", "web-01", r, 1),
            );
            let mut spec = deploy_spec("shop", PROJECT, &green, r, Some("incusbr0"));
            spec.name = new.clone();
            let step = |n: &str, payload: Option<Value>| json!({ "project": PROJECT, "name": n, "module": "web-01", "state": "pending", "payload": payload });
            for (action, n, payload) in [
                (
                    "create",
                    &new,
                    Some(serde_json::to_value(&spec).expect("json")),
                ),
                ("healthy", &new, None),
                ("drain", &old, None),
                ("delete", &old, None),
            ] {
                phases.push(
                    json!({ "action": action, "state": "pending", "steps": [step(n, payload)] }),
                );
            }
        }
        json!(phases)
    }

    /// Moves every recorded moment of the operation `secs` into the past, as time passing would.
    async fn age(pool: &Pool, op: uuid::Uuid, secs: i64) {
        let row = get_operation(pool, op).await.expect("op").expect("row");
        let mut phases = row.phases.clone();
        for phase in phases.as_array_mut().expect("phases") {
            for step in phase["steps"].as_array_mut().expect("steps") {
                if let Some(p) = step["payload"].as_object_mut() {
                    for k in ["since", "onlineSince", "leftAt"] {
                        if let Some(t) = p.get(k).and_then(Value::as_i64) {
                            p.insert(k.into(), json!(t - secs));
                        }
                    }
                }
            }
        }
        apelora_persistence::lifecycle::update_operation(
            pool,
            op,
            &row.state,
            &phases,
            row.error.as_deref(),
        )
        .await
        .expect("age");
    }

    /// Runs jobs until the operation ends, letting `secs` pass whenever nothing is runnable.
    async fn finish(pool: &Pool, provider: &Provider, op: uuid::Uuid, secs: i64) -> OperationRow {
        for _ in 0..200 {
            run_jobs(pool, provider).await;
            let row = get_operation(pool, op).await.expect("op").expect("row");
            if matches!(row.state.as_str(), "succeeded" | "failed" | "cancelled") {
                run_jobs(pool, provider).await;
                return row;
            }
            age(pool, op, secs).await;
            sqlx::query("UPDATE jobs SET run_after = now() WHERE status = 'pending'")
                .execute(pool)
                .await
                .expect("wake");
        }
        panic!("the update did not end");
    }

    async fn names(provider: &Provider) -> Vec<String> {
        let mut n: Vec<String> = provider
            .instances()
            .await
            .expect("instances")
            .into_iter()
            .map(|i| i.name)
            .filter(|n| n.starts_with("shop-"))
            .collect();
        n.sort();
        n
    }

    async fn update(pool: &Pool, app: uuid::Uuid) -> uuid::Uuid {
        create_operation(
            pool,
            &NewOperation {
                application_id: app,
                project: PROJECT,
                kind: "update",
                phases: &update_phases(),
                requested_by: "tester",
                confirmed: false,
                key: None,
                request_id: None,
            },
        )
        .await
        .expect("operation")
        .0
        .id
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn an_update_replaces_replicas_one_at_a_time_behind_the_service_address(pool: Pool) {
        let mock = MockIncus::default();
        let provider = Provider::Mock(mock.clone());
        let app = deployed(&pool, &provider).await;
        let op = update(&pool, app).await;

        // Replica 1's update is created first; replica 2 is not touched yet.
        run_jobs(&pool, &provider).await;
        assert_eq!(
            names(&provider).await,
            [
                "shop-db-01",
                "shop-web-01",
                "shop-web-01-r2",
                "shop-web-01-u1"
            ]
        );
        // The supervisor leaves the slot holding two instances to the update.
        assert!(
            crate::supervisor::supervise_once(&pool, &provider)
                .await
                .expect("pass")
                .is_empty()
        );

        let done = finish(&pool, &provider, op, 15).await;
        assert_eq!((done.state.as_str(), done.error), ("succeeded", None));
        assert_eq!(
            names(&provider).await,
            ["shop-db-01", "shop-web-01-r2-u1", "shop-web-01-u1"]
        );
        // The service address now points at the two updates only.
        let lb = mock
            .net()
            .load_balancers
            .values()
            .map(|(_, lb)| {
                let mut b: Vec<String> = lb.backends.iter().map(|b| b.name.clone()).collect();
                b.sort();
                b
            })
            .collect::<Vec<_>>();
        assert_eq!(lb, [["r1-u1", "r2-u1"]]);
        // Each new replica passed its health check before its predecessor was drained.
        let phases: Vec<Value> = serde_json::from_value(done.phases).expect("phases");
        let healthy = &phases[1]["steps"][0]["payload"];
        assert!(healthy["onlineSince"].is_i64(), "{healthy}");
        assert_eq!(phases[2]["steps"][0]["payload"]["enforced"], json!(true));
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn an_update_whose_new_replica_never_passes_stops_and_removes_it(pool: Pool) {
        let mock = MockIncus::default();
        let provider = Provider::Mock(mock.clone());
        let app = deployed(&pool, &provider).await;
        mock.net().failing.insert("shop-web-01-u1".into());
        let op = update(&pool, app).await;

        let done = finish(&pool, &provider, op, 60).await;
        assert_eq!(done.state, "failed");
        let error = done.error.unwrap_or_default();
        assert!(
            error.starts_with("update stopped: shop-web-01-u1: shop-web-01-u1 was not healthy within 300 s (its health check reports offline)"),
            "{error}"
        );
        assert!(error.contains("shop-web-01-u1 is removed"), "{error}");
        // The old replicas keep serving; replica 2 was never touched.
        assert_eq!(
            names(&provider).await,
            ["shop-db-01", "shop-web-01", "shop-web-01-r2"]
        );
        let phases: Vec<Value> = serde_json::from_value(done.phases).expect("phases");
        assert_eq!(
            phases
                .iter()
                .map(|p| p["state"].as_str().unwrap_or_default())
                .collect::<Vec<_>>(),
            [
                "succeeded",
                "failed",
                "skipped",
                "skipped",
                "skipped",
                "skipped",
                "skipped",
                "skipped"
            ]
        );
    }

    /// An update with one `rebuild` phase for the database, from `mode` and image `alias`.
    async fn rebuild(pool: &Pool, app: uuid::Uuid, mode: &str, alias: &str) -> uuid::Uuid {
        let mut db = module("db-01", 1, Availability::Stateful, mode);
        db.spec.source.alias = alias.into();
        let spec = deploy_spec("shop", PROJECT, &db, 1, Some("incusbr0"));
        let phases = json!([{ "action": "rebuild", "state": "pending", "steps": [
            { "project": PROJECT, "name": "shop-db-01", "module": "db-01", "state": "pending", "payload": spec }
        ] }]);
        create_operation(
            pool,
            &NewOperation {
                application_id: app,
                project: PROJECT,
                kind: "update",
                phases: &phases,
                requested_by: "tester",
                confirmed: true,
                key: None,
                request_id: None,
            },
        )
        .await
        .expect("operation")
        .0
        .id
    }

    #[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
    async fn a_stateful_module_is_rebuilt_in_place_and_a_settings_change_alone_is_not_rebuilt(
        pool: Pool,
    ) {
        let mock = MockIncus::default();
        let provider = Provider::Mock(mock.clone());
        let app = deployed(&pool, &provider).await;
        let before = provider
            .instance(PROJECT, "shop-db-01")
            .await
            .expect("read")
            .expect("exists");

        // A new image and a new setting: stopped, rebuilt, given the setting, started again.
        let op = rebuild(&pool, app, "green", "debian/14").await;
        let done = finish(&pool, &provider, op, 15).await;
        assert_eq!((done.state.as_str(), done.error), ("succeeded", None));
        assert_eq!(mock.net().rebuilds, ["shop-db-01"]);
        let mut green = module("db-01", 1, Availability::Stateful, "green");
        green.spec.source.alias = "debian/14".into();
        let after = provider
            .instance(PROJECT, "shop-db-01")
            .await
            .expect("read")
            .expect("exists");
        assert_eq!(
            after.built_from.as_deref(),
            Some(apelora_domain::plan::spec_fingerprint(&green.spec).as_str())
        );
        assert_eq!(
            after.settings.get("environment.MODE").map(String::as_str),
            Some("green")
        );
        assert_eq!(
            (after.status.as_str(), after.location.as_str()),
            ("Running", before.location.as_str()),
            "the same instance, on the same member, running again"
        );

        // The setting alone changes again: no rebuild, the settings only.
        let op = rebuild(&pool, app, "blue", "debian/14").await;
        let done = finish(&pool, &provider, op, 15).await;
        assert_eq!(done.state, "succeeded");
        assert_eq!(mock.net().rebuilds, ["shop-db-01"], "not rebuilt again");
        let after = provider
            .instance(PROJECT, "shop-db-01")
            .await
            .expect("read")
            .expect("exists");
        assert_eq!(
            after.settings.get("environment.MODE").map(String::as_str),
            Some("blue")
        );
        assert_eq!(after.status, "Running");
    }

    #[test]
    fn the_policy_reads_whole_seconds_and_keeps_defaults_otherwise() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            move |k: &str| {
                pairs
                    .iter()
                    .find(|(key, _)| *key == k)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert_eq!(UpdatePolicy::from_env(env(&[])), UpdatePolicy::default());
        let p = UpdatePolicy::from_env(env(&[
            ("APELORA_UPDATE_HEALTHY_SECS", "5"),
            ("APELORA_UPDATE_TIMEOUT_SECS", "-1"),
            ("APELORA_UPDATE_DRAIN_SECS", "soon"),
        ]));
        assert_eq!(
            p,
            UpdatePolicy {
                healthy_secs: 5,
                ..UpdatePolicy::default()
            }
        );
    }
}
