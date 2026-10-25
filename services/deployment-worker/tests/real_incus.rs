// SPDX-License-Identifier: AGPL-3.0-only
//! REAL INCUS TESTED evidence: instance lifecycle through the job queue against a disposable
//! Incus cluster. Ignored by default; run only against the throwaway test cluster:
//!
//! ```sh
//! bash scripts/test-cluster/up.sh && source ~/.config/apelora/test-cluster/env
//! DATABASE_URL=… cargo test -p apelora-deployment-worker --test real_incus -- --ignored --test-threads 1
//! ```
//!
//! Every test creates uniquely named instances in the deploy project and deletes them again.

use apelora_deployment_worker::run_once;
use apelora_domain::ProviderMode;
use apelora_incus_adapter::{InstanceState, Provider, config::provider_from_env};
use apelora_persistence::{JobView, Pool, enqueue_job, get_job};
use serde_json::{Value, json};

fn real_incus() -> Provider {
    let provider = provider_from_env(|k| std::env::var(k).ok()).expect("Incus configuration");
    assert_eq!(
        provider.mode(),
        ProviderMode::Incus,
        "APELORA_INCUS_URL is not set: these tests never run against the mock"
    );
    provider
}

fn project() -> String {
    std::env::var("APELORA_DEPLOY_PROJECTS")
        .ok()
        .and_then(|p| p.split(',').next().map(str::to_owned))
        .unwrap_or_else(|| "apelora".into())
}

fn unique(prefix: &str) -> String {
    format!(
        "{prefix}-{}",
        &uuid::Uuid::new_v4().simple().to_string()[..8]
    )
}

/// Runs the worker until the job is final, including retries after backoff (a VM still
/// booting may ignore its first shutdown request, for example), for at most ten minutes.
async fn finish(pool: &Pool, provider: &Provider, kind: &str, payload: &Value) -> JobView {
    let id = enqueue_job(pool, kind, None, payload, None)
        .await
        .expect("enqueue")
        .id();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
    loop {
        while run_once(pool, provider, "real-incus-test")
            .await
            .expect("run")
        {}
        let job = get_job(pool, id).await.expect("job");
        if matches!(job.status.as_str(), "succeeded" | "failed") {
            return job;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "{kind} still {} after ten minutes: {:?}",
            job.status,
            job.last_error
        );
        if let Some(e) = &job.last_error {
            eprintln!("{kind}: attempt {} will be retried: {e}", job.attempts);
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
}

async fn converge(pool: &Pool, provider: &Provider, kind: &str, payload: &Value) {
    let job = finish(pool, provider, kind, payload).await;
    assert_eq!(job.status, "succeeded", "{kind}: {:?}", job.last_error);
}

async fn status(provider: &Provider, project: &str, name: &str) -> Option<InstanceState> {
    provider.instance(project, name).await.expect("instance")
}

/// create (started) → stop → start → delete, checking Incus's own view after every step.
/// Returns the image fingerprint and architecture Incus used.
async fn lifecycle(pool: Pool, kind: &str, source: Value) -> (String, String) {
    let provider = real_incus();
    let project = project();
    let name = unique("apl-it");
    let target = json!({ "project": project, "name": name });
    let spec =
        json!({ "project": project, "name": name, "kind": kind, "source": source, "start": true });

    converge(&pool, &provider, "instance.create", &spec).await;
    let created = status(&provider, &project, &name).await.expect("created");
    assert_eq!(
        (created.status.as_str(), created.managed),
        ("Running", true)
    );
    let image = created
        .base_image
        .clone()
        .expect("Incus records the base image");
    eprintln!(
        "{project}/{name}: {kind} on {} from image {image}",
        created.architecture
    );

    // Creating again converges without another Incus call, also when pinned to what Incus
    // reports; a different pin is refused, not "fixed".
    converge(&pool, &provider, "instance.create", &spec).await;
    let mut pinned = spec.clone();
    pinned["architectures"] = json!([created.architecture]);
    if spec["source"]["protocol"] == "simplestreams" {
        pinned["source"]["fingerprint"] = json!(image);
    }
    converge(&pool, &provider, "instance.create", &pinned).await;
    let mut other = spec.clone();
    other["architectures"] = json!([if created.architecture == "x86_64" {
        "aarch64"
    } else {
        "x86_64"
    }]);
    let job = finish(&pool, &provider, "instance.create", &other).await;
    assert_eq!(job.status, "failed");
    assert_eq!(job.attempts, 1, "refused without retrying");

    converge(&pool, &provider, "instance.stop", &target).await;
    assert_eq!(
        status(&provider, &project, &name).await.map(|s| s.status),
        Some("Stopped".into())
    );
    converge(&pool, &provider, "instance.start", &target).await;
    assert_eq!(
        status(&provider, &project, &name).await.map(|s| s.status),
        Some("Running".into())
    );

    // Deleting a running instance stops it first; deleting twice is a no-op.
    converge(&pool, &provider, "instance.delete", &target).await;
    assert_eq!(status(&provider, &project, &name).await, None);
    converge(&pool, &provider, "instance.delete", &target).await;
    (image, created.architecture)
}

#[ignore = "needs a disposable Incus: scripts/test-cluster/up.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn system_container_lifecycle(pool: Pool) {
    let source = json!({ "protocol": "simplestreams", "server": "https://images.linuxcontainers.org", "alias": "debian/13" });
    let (image, architecture) = lifecycle(pool.clone(), "container", source.clone()).await;

    // A new instance pinned to that fingerprint is built from exactly that image.
    let provider = real_incus();
    let project = project();
    let name = unique("apl-pin");
    let mut pinned = source;
    pinned["fingerprint"] = json!(image);
    let spec = json!({ "project": project, "name": name, "kind": "container", "source": pinned,
                       "architectures": [architecture], "start": false });
    converge(&pool, &provider, "instance.create", &spec).await;
    let created = status(&provider, &project, &name).await.expect("created");
    assert_eq!(created.base_image.as_deref(), Some(image.as_str()));
    assert_eq!(created.status, "Stopped");
    converge(
        &pool,
        &provider,
        "instance.delete",
        &json!({ "project": project, "name": name }),
    )
    .await;
}

/// A spec no member can run is refused before anything is created.
#[ignore = "needs a disposable Incus: scripts/test-cluster/up.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn unsupported_architecture_is_refused_before_creating(pool: Pool) {
    let provider = real_incus();
    let project = project();
    let members = provider.cluster_members().await.expect("members");
    let other = if members.iter().any(|m| m.architecture == "x86_64") {
        "riscv64"
    } else {
        "x86_64"
    };
    let name = unique("apl-arch");
    let spec = json!({ "project": project, "name": name, "kind": "container", "start": true,
                       "source": { "protocol": "simplestreams", "server": "https://images.linuxcontainers.org", "alias": "debian/13" },
                       "architectures": [other] });
    let job = finish(&pool, &provider, "instance.create", &spec).await;
    assert_eq!((job.status.as_str(), job.attempts), ("failed", 1));
    assert!(
        job.last_error
            .as_deref()
            .is_some_and(|e| e.contains("supported architecture")),
        "{:?}",
        job.last_error
    );
    assert_eq!(status(&provider, &project, &name).await, None);
}

#[ignore = "needs a disposable Incus: scripts/test-cluster/up.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn oci_container_lifecycle(pool: Pool) {
    lifecycle(
        pool,
        "container",
        json!({ "protocol": "oci", "server": "https://docker.io", "alias": "library/nginx:1.29" }),
    )
    .await;
}

/// Pinned by the registry's multi-architecture index digest (nginx 1.29 as of 2026-10-20);
/// Incus resolves the platform's manifest from it.
#[ignore = "needs a disposable Incus: scripts/test-cluster/up.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn oci_container_pinned_by_digest_lifecycle(pool: Pool) {
    lifecycle(
        pool,
        "container",
        json!({ "protocol": "oci", "server": "https://docker.io",
                "alias": "library/nginx@sha256:1881968aff6f7cdcc4b888c00a11f4ce241ad7ec957e0cb4a9e19e93a3ff87ea" }),
    )
    .await;
}

#[ignore = "needs a disposable Incus with nested virtualisation: scripts/test-cluster/up.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn virtual_machine_lifecycle(pool: Pool) {
    lifecycle(
        pool,
        "virtual-machine",
        json!({ "protocol": "simplestreams", "server": "https://images.linuxcontainers.org", "alias": "debian/13" }),
    )
    .await;
}

/// The worker refuses to stop or delete an instance it did not create.
#[ignore = "needs a disposable Incus: scripts/test-cluster/up.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn unowned_instances_are_refused(pool: Pool) {
    let provider = real_incus();
    let Ok(name) = std::env::var("APELORA_TEST_UNOWNED_INSTANCE") else {
        panic!(
            "set APELORA_TEST_UNOWNED_INSTANCE to an instance in the deploy project that Apelora did not create"
        );
    };
    let project = project();
    let before = status(&provider, &project, &name)
        .await
        .expect("the unowned instance exists");
    assert!(!before.managed);
    let target = json!({ "project": project, "name": name });
    for kind in ["instance.stop", "instance.delete"] {
        let job = finish(&pool, &provider, kind, &target).await;
        assert_eq!(job.status, "failed", "{kind}");
        assert_eq!(job.attempts, 1, "{kind}: refused without retrying");
    }
    assert_eq!(status(&provider, &project, &name).await, Some(before));
}

/// Every catalog runtime runs: the requests come from the catalog itself
/// (`npm run -s runtime-requests -w @apelora/module-catalog > requests.json`).
#[ignore = "needs a disposable Incus and APELORA_TEST_CATALOG_REQUESTS"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn catalog_runtimes_run(pool: Pool) {
    let path = std::env::var("APELORA_TEST_CATALOG_REQUESTS")
        .expect("APELORA_TEST_CATALOG_REQUESTS: a file written by the runtime-requests script");
    let entries: Vec<Value> =
        serde_json::from_str(&std::fs::read_to_string(&path).expect("read requests"))
            .expect("json");
    assert!(!entries.is_empty(), "the catalog has no runtimes");
    let provider = real_incus();
    let project = project();
    let volume_pool = std::env::var("APELORA_TEST_VOLUME_POOL").unwrap_or_else(|_| "local".into());
    let app = "apl-catalog-test";
    for entry in entries {
        let name = unique("apl-cat");
        let mut spec = entry["request"].clone();
        spec["project"] = json!(project);
        spec["name"] = json!(name);
        spec["owner"] = json!({ "application": app, "module": entry["class"], "replica": 1 });
        // The control plane picks the pool from the volume class; here it is the test pool.
        for v in spec
            .get_mut("volumes")
            .and_then(Value::as_array_mut)
            .into_iter()
            .flatten()
        {
            v["pool"] = json!(volume_pool);
        }
        converge(&pool, &provider, "instance.create", &spec).await;
        let s = status(&provider, &project, &name).await.expect("created");
        assert_eq!((s.status.as_str(), s.managed), ("Running", true), "{entry}");
        if let Some(check) = entry["check"].as_str() {
            // The software takes a while to start; its settings must be the ones asked for.
            let command = ["sh".to_owned(), "-c".to_owned(), check.to_owned()];
            let mut passed = false;
            for _ in 0..30 {
                let run = provider
                    .run_probe(&project, &name, &command, 0, 10)
                    .await
                    .expect("run the check");
                if run.exit == Some(0) {
                    passed = true;
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            }
            assert!(
                passed,
                "{}: the check never passed: {check}",
                entry["class"]
            );
        }
        eprintln!(
            "{} {}: running on {} from image {}{}",
            entry["class"],
            entry["config"],
            s.architecture,
            s.base_image.as_deref().unwrap_or("?"),
            if entry["check"].is_string() {
                ", its check passed inside"
            } else {
                ""
            }
        );
        let member = Some(s.location.clone()).filter(|m| !m.is_empty() && m != "none");
        converge(
            &pool,
            &provider,
            "instance.delete",
            &json!({ "project": project, "name": name }),
        )
        .await;
        for v in spec["volumes"].as_array().into_iter().flatten() {
            let volume = format!("{name}-{}", v["name"].as_str().expect("volume name"));
            provider
                .delete_volume(&project, &volume_pool, member.as_deref(), &volume, app)
                .await
                .expect("delete the test volume");
        }
    }
}

/// A join token issued through the admin identity is listed (without its secret) and revoked.
/// The deploy identity cannot issue one. Nothing joins: the name is never used by a server.
#[ignore = "needs a disposable Incus with the admin identity: scripts/test-cluster/up.sh"]
#[tokio::test]
async fn join_tokens_need_the_admin_identity_and_can_be_revoked() {
    use apelora_incus_adapter::config::admin_provider_from_env;
    let deploy = real_incus();
    let admin = admin_provider_from_env(|k| std::env::var(k).ok(), &deploy)
        .expect("admin configuration")
        .expect("APELORA_INCUS_ADMIN_CLIENT_CERT/KEY are set");
    let member = unique("apl-join");
    assert!(
        deploy.create_join_token(&member).await.is_err(),
        "the project-restricted identity must not issue join tokens"
    );
    let token = admin.create_join_token(&member).await.expect("token");
    assert_eq!(token.member_name, member);
    assert!(token.token.len() > 40);
    let pending = admin.join_tokens().await.expect("list");
    assert!(pending.iter().any(|p| p.member_name == member));
    assert!(!format!("{pending:?}").contains(&token.token));
    assert_eq!(admin.revoke_join_token(&member).await, Ok(true));
    assert!(
        !admin
            .join_tokens()
            .await
            .expect("list")
            .iter()
            .any(|p| p.member_name == member)
    );
    assert_eq!(admin.revoke_join_token(&member).await, Ok(false));
}

/// Two replicas of a stateless module land on different members (the anti-affinity
/// scriptlet, scripts/test-cluster/placement.sh), and the supervisor brings back a lost one,
/// again on the member without a sibling. Needs two members and the scriptlet installed.
#[ignore = "needs a disposable two-member Incus with the placement scriptlet: scripts/test-cluster/placement.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn replicas_spread_across_members_and_lost_ones_return(pool: Pool) {
    use apelora_deployment_worker::supervisor::supervise_once;
    use apelora_domain::plan::{DesiredModule, deploy_spec};

    let provider = real_incus();
    let project = project();
    let application = unique("apl-ha");
    let app = apelora_persistence::ensure_application(&pool, &application)
        .await
        .expect("application");
    let web: DesiredModule = serde_json::from_value(json!({
        "module": "web-01",
        "spec": { "project": "", "name": "", "kind": "container", "start": true,
                  "source": { "protocol": "simplestreams", "server": "https://images.linuxcontainers.org", "alias": "debian/13" } },
        "replicas": 2,
        "availability": "stateless",
    }))
    .expect("desired");
    let names = [
        format!("{application}-web-01"),
        format!("{application}-web-01-r2"),
    ];
    for replica in 1..=2 {
        let spec = deploy_spec(&application, &project, &web, replica, None);
        converge(
            &pool,
            &provider,
            "instance.create",
            &serde_json::to_value(&spec).expect("json"),
        )
        .await;
    }
    let desired = json!([web]);
    apelora_persistence::record_deployment(
        &pool,
        &apelora_persistence::NewDeployment {
            application_id: app.id,
            project: &project,
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

    let locations = async || {
        let all = provider.instances().await.expect("instances");
        names
            .iter()
            .map(|n| {
                all.iter()
                    .find(|i| &i.name == n)
                    .map(|i| i.location.clone())
            })
            .collect::<Vec<_>>()
    };
    let before = locations().await;
    eprintln!("replicas placed on {before:?}");
    assert!(before.iter().all(Option::is_some), "both replicas exist");
    assert_ne!(before[0], before[1], "replicas share a member");

    // Lose replica 2; the supervisor queues its recreation and the worker carries it out.
    let lost = json!({ "project": project, "name": names[1] });
    converge(&pool, &provider, "instance.delete", &lost).await;
    assert_eq!(
        supervise_once(&pool, &provider).await.expect("supervise"),
        vec![names[1].clone()]
    );
    while run_once(&pool, &provider, "real-incus-test")
        .await
        .expect("run")
    {}
    let after = locations().await;
    eprintln!("after recreation: {after:?}");
    assert_eq!(after[0], before[0]);
    assert!(after[1].is_some(), "the lost replica is back");
    assert_ne!(
        after[0], after[1],
        "the recreated replica avoided its sibling's member"
    );
    assert_eq!(
        status(&provider, &project, &names[1])
            .await
            .map(|s| (s.status, s.managed)),
        Some(("Running".into(), true))
    );

    for n in &names {
        converge(
            &pool,
            &provider,
            "instance.delete",
            &json!({ "project": project, "name": n }),
        )
        .await;
    }
}

/// Two replicas of a stateless module on one member: one supervisor pass with rebalancing on
/// queues one audited move of the highest-named replica to another member, the worker carries it
/// out, and the next pass has nothing to do. Needs two members and member targeting allowed.
#[ignore = "needs a disposable Incus with two members and member targeting allowed: scripts/test-cluster/up.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn replicas_sharing_a_member_are_spread_by_the_rebalancer(pool: Pool) {
    use apelora_deployment_worker::supervisor::Supervisor;
    use apelora_domain::plan::{DesiredModule, deploy_spec};

    let provider = real_incus();
    let project = project();
    let online: Vec<String> = provider
        .cluster_members()
        .await
        .expect("members")
        .into_iter()
        .filter(|m| m.status == "Online")
        .map(|m| m.name)
        .collect();
    assert!(online.len() >= 2, "needs two online members");
    let application = unique("apl-rb");
    let app = apelora_persistence::ensure_application(&pool, &application)
        .await
        .expect("application");
    let web: DesiredModule = serde_json::from_value(json!({
        "module": "web-01",
        "spec": { "project": "", "name": "", "kind": "container", "start": true,
                  "source": { "protocol": "simplestreams", "server": "https://images.linuxcontainers.org", "alias": "debian/13" } },
        "replicas": 2,
        "availability": "stateless",
    }))
    .expect("desired");
    let names = [
        format!("{application}-web-01"),
        format!("{application}-web-01-r2"),
    ];
    for replica in 1..=2 {
        let spec = deploy_spec(&application, &project, &web, replica, None);
        converge(
            &pool,
            &provider,
            "instance.create",
            &serde_json::to_value(&spec).expect("json"),
        )
        .await;
    }
    let location = async |n: &str| {
        status(&provider, &project, n)
            .await
            .expect("exists")
            .location
    };
    // Put both on one member, as a migration of the whole application would.
    let shared = location(&names[0]).await;
    let second = location(&names[1]).await;
    if second != shared {
        converge(
            &pool,
            &provider,
            "instance.move",
            &json!({ "project": project, "name": names[1],
            "application": application, "from": second, "target": shared, "live": false,
            "startAfter": true, "volumes": [] }),
        )
        .await;
    }
    assert_eq!(
        location(&names[1]).await,
        shared,
        "both replicas share {shared}"
    );
    apelora_persistence::record_deployment(
        &pool,
        &apelora_persistence::NewDeployment {
            application_id: app.id,
            project: &project,
            revision: 1,
            desired: &json!([web]),
            connections: None,
            uplink: None,
            service_range: None,
            published: None,
        },
    )
    .await
    .expect("record");

    let started = std::time::Instant::now();
    let mut supervisor = Supervisor::new(std::time::Duration::from_secs(3600)).with_rebalance(true);
    let queued = supervisor.pass(&pool, &provider).await.expect("pass");
    assert_eq!(
        queued,
        vec![names[1].clone()],
        "one move, of the highest name"
    );
    while run_once(&pool, &provider, "real-incus-test")
        .await
        .expect("run")
    {}
    let moved = status(&provider, &project, &names[1])
        .await
        .expect("exists");
    eprintln!(
        "{} moved {shared} → {} in {:.1} s, {}",
        names[1],
        moved.location,
        started.elapsed().as_secs_f64(),
        moved.status
    );
    assert_ne!(moved.location, shared, "the replica left the shared member");
    assert_eq!((moved.status.as_str(), moved.managed), ("Running", true));
    assert_eq!(
        location(&names[0]).await,
        shared,
        "the other replica stayed"
    );
    let audit = apelora_persistence::recent_audit(&pool, 50)
        .await
        .expect("audit");
    assert!(
        audit
            .iter()
            .any(|e| e.action == "application.replica.rebalance" && e.target.ends_with(&names[1])),
        "audited"
    );
    assert!(
        supervisor
            .pass(&pool, &provider)
            .await
            .expect("pass")
            .is_empty(),
        "spread: nothing more to move"
    );

    for n in &names {
        converge(
            &pool,
            &provider,
            "instance.delete",
            &json!({ "project": project, "name": n }),
        )
        .await;
    }
}

/// A volume is created on the member Incus placed its instance on (Incus does not follow a
/// member-local volume), attached before the instance starts, and kept when the instance is
/// deleted. Needs a pool named by `APELORA_TEST_VOLUME_POOL` (default `local`).
#[ignore = "needs a disposable Incus: scripts/test-cluster/up.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn volumes_follow_the_instance_and_outlive_it(pool: Pool) {
    let provider = real_incus();
    let project = project();
    let volume_pool = std::env::var("APELORA_TEST_VOLUME_POOL").unwrap_or_else(|_| "local".into());
    let name = unique("apl-vol");
    let spec = json!({ "project": project, "name": name, "kind": "container", "start": true,
        "source": { "protocol": "simplestreams", "server": "https://images.linuxcontainers.org", "alias": "debian/13" },
        "owner": { "application": "apl-volume-test", "module": "db", "replica": 1 },
        "volumes": [{ "name": "data", "sizeMib": 256, "path": "/srv/data", "pool": volume_pool }] });
    converge(&pool, &provider, "instance.create", &spec).await;
    let created = status(&provider, &project, &name).await.expect("created");
    assert_eq!(created.status, "Running");
    assert_eq!(created.volume_devices, vec!["vol-data".to_owned()]);
    let volume_name = format!("{name}-data");
    let member = Some(created.location.as_str()).filter(|m| !m.is_empty() && *m != "none");
    let volume = provider
        .volume(&project, &volume_pool, member, &volume_name)
        .await
        .expect("read volume")
        .expect("volume on the instance's member");
    assert_eq!(volume.size_mib, Some(256));
    assert_eq!(volume.application.as_deref(), Some("apl-volume-test"));
    assert!(volume.managed);
    assert_eq!(volume.used_by.len(), 1, "{:?}", volume.used_by);

    // It grows while the instance runs; a shrink is refused before it reaches Incus.
    provider
        .grow_volume(
            &project,
            &volume_pool,
            member,
            &volume_name,
            "apl-volume-test",
            512,
        )
        .await
        .expect("grow");
    let grown = provider
        .volume(&project, &volume_pool, member, &volume_name)
        .await
        .expect("read volume")
        .expect("volume");
    assert_eq!(grown.size_mib, Some(512));
    assert!(
        provider
            .grow_volume(
                &project,
                &volume_pool,
                member,
                &volume_name,
                "apl-volume-test",
                256
            )
            .await
            .is_err(),
        "a shrink must be refused"
    );

    // Snapshots: taken while running (crash-consistent), restored only once the instance stops.
    let (app, vp) = ("apl-volume-test", volume_pool.as_str());
    provider
        .snapshot_volume(&project, vp, member, &volume_name, app, "snap0")
        .await
        .expect("snapshot");
    let snaps = provider
        .volume_snapshots(&project, vp, member, &volume_name)
        .await
        .expect("list snapshots");
    assert_eq!(
        snaps.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["snap0"]
    );
    assert!(
        provider
            .restore_volume(&project, vp, member, &volume_name, app, "snap0")
            .await
            .is_err(),
        "Incus refuses a restore while the instance runs"
    );
    converge(
        &pool,
        &provider,
        "instance.stop",
        &json!({ "project": project, "name": name }),
    )
    .await;
    provider
        .restore_volume(&project, vp, member, &volume_name, app, "snap0")
        .await
        .expect("restore after stop");
    provider
        .delete_snapshot(&project, vp, member, &volume_name, app, "snap0")
        .await
        .expect("delete snapshot");
    assert!(
        provider
            .volume_snapshots(&project, vp, member, &volume_name)
            .await
            .expect("list snapshots")
            .is_empty()
    );

    // Creating again converges without a second volume or device.
    converge(&pool, &provider, "instance.create", &spec).await;
    converge(
        &pool,
        &provider,
        "instance.delete",
        &json!({ "project": project, "name": name }),
    )
    .await;
    let kept = provider
        .volume(&project, &volume_pool, member, &volume_name)
        .await
        .expect("read volume")
        .expect("volume kept after the instance is deleted");
    assert!(kept.used_by.is_empty());
    assert!(
        provider
            .delete_volume(&project, &volume_pool, member, &volume_name, "someone-else")
            .await
            .is_err(),
        "another application's volume must not be deleted"
    );
    assert_eq!(
        provider
            .delete_volume(
                &project,
                &volume_pool,
                member,
                &volume_name,
                "apl-volume-test"
            )
            .await,
        Ok(true)
    );
}

/// A backup is a copy, through a short-lived snapshot, into the backup pool on another member;
/// it carries the size and Apelora's labels, and the snapshot is removed again. Needs a second
/// pool named by `APELORA_TEST_BACKUP_POOL` (default `backup`) on every member.
#[ignore = "needs a disposable Incus with a backup pool: scripts/test-cluster/backup-pool.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_backup_is_copied_to_another_member(pool: Pool) {
    let provider = real_incus();
    let project = project();
    let backup_pool = std::env::var("APELORA_TEST_BACKUP_POOL").unwrap_or_else(|_| "backup".into());
    let name = unique("apl-bk");
    let spec = json!({ "project": project, "name": name, "kind": "container", "start": true,
        "source": { "protocol": "simplestreams", "server": "https://images.linuxcontainers.org", "alias": "debian/13" },
        "owner": { "application": "apl-backup-test", "module": "db", "replica": 1 },
        "volumes": [{ "name": "data", "sizeMib": 128, "path": "/srv/data", "pool": "local" }] });
    converge(&pool, &provider, "instance.create", &spec).await;
    let created = status(&provider, &project, &name).await.expect("created");
    let volume = format!("{name}-data");
    let req = apelora_deployment_worker::storage::BackupRequest {
        application: "apl-backup-test".into(),
        project: project.clone(),
        module: "db".into(),
        volume: volume.clone(),
        pool: "local".into(),
        member: created.location.clone(),
        size_mib: 128,
        backup_pool: backup_pool.clone(),
    };
    let (backup, to, snapshot) = apelora_deployment_worker::storage::back_up(
        &provider,
        &req,
        time::OffsetDateTime::now_utc(),
    )
    .await
    .expect("backup");
    assert!(snapshot.starts_with("bk-"));
    assert_ne!(to, created.location, "the backup is on another member");
    let copy = provider
        .volume(&project, &backup_pool, Some(&to), &backup)
        .await
        .expect("read backup")
        .expect("backup exists");
    assert_eq!(
        (copy.size_mib, copy.application.as_deref(), copy.managed),
        (Some(128), Some("apl-backup-test"), true)
    );
    let member = Some(created.location.as_str());
    assert!(
        provider
            .volume_snapshots(&project, "local", member, &volume)
            .await
            .expect("snapshots")
            .is_empty(),
        "the temporary snapshot is gone"
    );
    converge(
        &pool,
        &provider,
        "instance.delete",
        &json!({ "project": project, "name": name }),
    )
    .await;
    for (p, m, v) in [
        (backup_pool.as_str(), Some(to.as_str()), backup.as_str()),
        ("local", member, volume.as_str()),
    ] {
        assert_eq!(
            provider
                .delete_volume(&project, p, m, v, "apl-backup-test")
                .await,
            Ok(true),
            "{v}"
        );
    }
}

/// Moves through the `instance.move` job: a container with a volume stops, its volume is copied
/// to the new member, it moves and starts there and the old copy is deleted; a VM with live
/// migration moves while running. Needs `restricted.cluster.target=allow` on the project and a
/// second online member.
#[ignore = "needs a disposable Incus with two members and member targeting allowed: scripts/test-cluster/up.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn instances_move_with_their_volumes_and_vms_move_live(pool: Pool) {
    let provider = real_incus();
    let project = project();
    apelora_persistence::ensure_application(&pool, "apl-move-test")
        .await
        .expect("application");
    let members: Vec<String> = provider
        .cluster_members()
        .await
        .expect("members")
        .into_iter()
        .filter(|m| m.status == "Online")
        .map(|m| m.name)
        .collect();
    assert!(members.len() >= 2, "needs two online members");
    let other = |from: &str| {
        members
            .iter()
            .find(|m| *m != from)
            .expect("another member")
            .clone()
    };

    // A container with a volume.
    let name = unique("apl-mv");
    converge(&pool, &provider, "instance.create", &json!({ "project": project, "name": name, "kind": "container", "start": true,
        "source": { "protocol": "simplestreams", "server": "https://images.linuxcontainers.org", "alias": "debian/13" },
        "owner": { "application": "apl-move-test", "module": "db", "replica": 1 },
        "volumes": [{ "name": "data", "sizeMib": 128, "path": "/srv/data", "pool": "local" }] })).await;
    let from = status(&provider, &project, &name)
        .await
        .expect("created")
        .location;
    let to = other(&from);
    let volume = format!("{name}-data");
    converge(&pool, &provider, "instance.move", &json!({ "project": project, "name": name, "application": "apl-move-test",
        "from": from, "target": to, "live": false, "startAfter": true, "volumes": [{ "pool": "local", "name": volume }] })).await;
    let moved = status(&provider, &project, &name).await.expect("exists");
    assert_eq!(
        (moved.location.as_str(), moved.status.as_str()),
        (to.as_str(), "Running")
    );
    assert!(
        provider
            .volume(&project, "local", Some(&to), &volume)
            .await
            .expect("read")
            .is_some()
    );
    assert!(
        provider
            .volume(&project, "local", Some(&from), &volume)
            .await
            .expect("read")
            .is_none(),
        "old copy deleted"
    );
    converge(
        &pool,
        &provider,
        "instance.delete",
        &json!({ "project": project, "name": name }),
    )
    .await;
    assert_eq!(
        provider
            .delete_volume(&project, "local", Some(&to), &volume, "apl-move-test")
            .await,
        Ok(true)
    );

    // A VM with live migration.
    let vm = unique("apl-mvm");
    converge(&pool, &provider, "instance.create", &json!({ "project": project, "name": vm, "kind": "virtual-machine", "start": true,
        "source": { "protocol": "simplestreams", "server": "https://images.linuxcontainers.org", "alias": "debian/13" },
        "config": { "migration.stateful": "true", "limits.memory": "1GiB" },
        "owner": { "application": "apl-move-test", "module": "vm", "replica": 1 } })).await;
    let from = status(&provider, &project, &vm)
        .await
        .expect("created")
        .location;
    let to = other(&from);
    converge(
        &pool,
        &provider,
        "instance.move",
        &json!({ "project": project, "name": vm, "application": "apl-move-test",
        "from": from, "target": to, "live": true, "startAfter": true }),
    )
    .await;
    let moved = status(&provider, &project, &vm).await.expect("exists");
    assert_eq!(
        (moved.location.as_str(), moved.status.as_str()),
        (to.as_str(), "Running")
    );
    converge(
        &pool,
        &provider,
        "instance.delete",
        &json!({ "project": project, "name": vm }),
    )
    .await;
}

/// HA slice B, the instance part (ADR 0013): a module whose root disk and volume are on the
/// shared pool is created with both there, its volume belongs to no member, and it moves to
/// another member without anything being copied. Killing a member is not done here: that is
/// `scripts/test-cluster/ha-drill.sh` (plan first). Needs the clustered LVM pool
/// (`scripts/test-cluster/shared-storage.sh`, `APELORA_TEST_SHARED_POOL`, default `shared`) and
/// member targeting allowed.
#[ignore = "needs a disposable Incus with the shared pool: scripts/test-cluster/shared-storage.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn shared_pool_instances_keep_their_disks_on_it_and_move_without_copying(pool: Pool) {
    let provider = real_incus();
    let project = project();
    let shared = std::env::var("APELORA_TEST_SHARED_POOL").unwrap_or_else(|_| "shared".into());
    let driver = provider
        .storage_pools()
        .await
        .expect("pools")
        .into_iter()
        .find(|p| p.name == shared)
        .map(|p| p.driver)
        .expect("the shared pool exists");
    assert!(
        apelora_domain::storage::is_shared_driver(&driver),
        "{shared} uses {driver}"
    );
    let app = "apelora-ha-test";
    apelora_persistence::ensure_application(&pool, app)
        .await
        .expect("application");
    let name = unique("apelora-ha-test");
    let volume = format!("{name}-data");
    converge(&pool, &provider, "instance.create", &json!({ "project": project, "name": name, "kind": "container", "start": true,
        "source": { "protocol": "simplestreams", "server": "https://images.linuxcontainers.org", "alias": "debian/13" },
        "owner": { "application": app, "module": "db", "replica": 1 }, "rootPool": shared,
        "volumes": [{ "name": "data", "sizeMib": 256, "path": "/srv/data", "class": "data-replicated", "pool": shared }] })).await;
    let created = status(&provider, &project, &name).await.expect("created");
    assert_eq!(created.status, "Running");
    assert_eq!(created.volume_devices, vec!["vol-data".to_owned()]);
    let view = provider
        .instance_view(&project, &name)
        .await
        .expect("view")
        .expect("exists");
    let root = view
        .disks
        .iter()
        .find(|d| d.path.as_deref() == Some("/"))
        .expect("root disk");
    assert_eq!(
        root.pool.as_deref(),
        Some(shared.as_str()),
        "{:?}",
        view.disks
    );
    let v = provider
        .volume(&project, &shared, None, &volume)
        .await
        .expect("read volume")
        .expect("the volume is on the shared pool, without a member");
    assert!(v.managed && v.used_by.len() == 1, "{v:?}");
    let recorded = apelora_persistence::application_volumes(&pool, app)
        .await
        .expect("volumes");
    assert_eq!(
        (
            recorded[0].storage_pool.as_str(),
            recorded[0].member.as_str()
        ),
        (shared.as_str(), "")
    );

    // A cold move: nothing to copy, it runs on the other member with the same disks.
    let to = provider
        .cluster_members()
        .await
        .expect("members")
        .into_iter()
        .find(|m| m.status == "Online" && m.name != created.location)
        .map(|m| m.name)
        .expect("another online member");
    let started = std::time::Instant::now();
    converge(
        &pool,
        &provider,
        "instance.move",
        &json!({ "project": project, "name": name, "application": app,
        "from": created.location, "target": to, "live": false, "startAfter": true, "volumes": [] }),
    )
    .await;
    let moved = status(&provider, &project, &name).await.expect("exists");
    assert_eq!(
        (moved.location.as_str(), moved.status.as_str()),
        (to.as_str(), "Running")
    );
    eprintln!(
        "{project}/{name}: moved from {} to {to} on {shared} in {:.1} s (stop, relink, start)",
        created.location,
        started.elapsed().as_secs_f64()
    );
    assert!(
        provider
            .volume(&project, &shared, None, &volume)
            .await
            .expect("read volume")
            .is_some_and(|v| v.used_by.len() == 1),
        "the same volume, still attached"
    );

    converge(
        &pool,
        &provider,
        "instance.delete",
        &json!({ "project": project, "name": name }),
    )
    .await;
    assert_eq!(
        provider
            .delete_volume(&project, &shared, None, &volume, app)
            .await,
        Ok(true)
    );
}

#[ignore = "needs a disposable Incus: scripts/test-cluster/up.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_running_container_is_resized_in_place_and_answers_readiness_commands(pool: Pool) {
    let provider = real_incus();
    let project = project();
    let name = unique("apl-resize");
    let target = json!({ "project": project, "name": name });
    let spec = json!({ "project": project, "name": name, "kind": "container", "start": true,
        "source": { "protocol": "simplestreams", "server": "https://images.linuxcontainers.org", "alias": "debian/13" },
        "config": { "limits.cpu": "1", "limits.memory": "512MiB" } });
    converge(&pool, &provider, "instance.create", &spec).await;
    let cpus = |provider: Provider, project: String, name: String, want: &'static str| async move {
        let script = format!("[ \"$(nproc)\" = \"{want}\" ]");
        provider
            .run_probe(&project, &name, &["sh".into(), "-c".into(), script], 0, 10)
            .await
            .expect("probe")
            .exit
    };
    let ready = |args: &[&str], uid: u32, secs: u8| {
        let provider = provider.clone();
        let (p, n) = (project.clone(), name.clone());
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        async move { provider.run_probe(&p, &n, &args, uid, secs).await }
    };

    // A readiness command reports only its exit status; a hanging one is stopped at its limit;
    // one that cannot be started says so; neither of the last two is an exit status.
    assert_eq!(ready(&["true"], 0, 10).await.expect("run").exit, Some(0));
    assert_eq!(ready(&["false"], 0, 10).await.expect("run").exit, Some(1));
    let hang = ready(&["sleep", "30"], 0, 2).await.expect("run");
    assert!(hang.timed_out && hang.exit.is_none(), "{hang:?}");
    let missing = ready(&["/nonexistent/binary"], 0, 5).await;
    assert!(
        missing.as_ref().map(|r| r.exit != Some(0)).unwrap_or(true),
        "{missing:?}"
    );
    // As an unprivileged user (uid 1000) it cannot read root's files.
    assert_eq!(
        ready(&["cat", "/etc/shadow"], 1000, 5)
            .await
            .expect("run")
            .exit,
        Some(1),
        "an unprivileged user cannot read /etc/shadow"
    );
    assert_eq!(
        cpus(provider.clone(), project.clone(), name.clone(), "1").await,
        Some(0),
        "one CPU before"
    );

    // Resize while it runs: it stays Running, Incus has the new limits, the guest sees two CPUs.
    let resize = json!({ "project": project, "name": name,
        "limits": { "limits.cpu": "2", "limits.memory": "1024MiB" } });
    converge(&pool, &provider, "instance.resize", &resize).await;
    let after = status(&provider, &project, &name).await.expect("exists");
    assert_eq!(after.status, "Running", "resizing does not stop it");
    assert_eq!(
        after.limits.get("limits.cpu").map(String::as_str),
        Some("2")
    );
    assert_eq!(
        after.limits.get("limits.memory").map(String::as_str),
        Some("1024MiB")
    );
    assert_eq!(
        cpus(provider.clone(), project.clone(), name.clone(), "2").await,
        Some(0),
        "two CPUs after"
    );
    // The same job again changes nothing; an instance that is not there fails without retry.
    converge(&pool, &provider, "instance.resize", &resize).await;
    let ghost = json!({ "project": project, "name": unique("apl-ghost"),
        "limits": { "limits.cpu": "2" } });
    assert_eq!(
        finish(&pool, &provider, "instance.resize", &ghost)
            .await
            .status,
        "failed"
    );

    converge(&pool, &provider, "instance.delete", &target).await;
    assert_eq!(status(&provider, &project, &name).await, None);
}

/// The restore a person starts in the Console, through the same jobs: back up, change the data,
/// stop the instance, `volume.restore` (a safety backup first), start and read the data back.
#[ignore = "needs a disposable Incus with a backup pool: scripts/test-cluster/backup-pool.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_volume_is_restored_from_its_backup_with_a_safety_backup_first(pool: Pool) {
    let provider = real_incus();
    let project = project();
    let backup_pool = std::env::var("APELORA_TEST_BACKUP_POOL").unwrap_or_else(|_| "backup".into());
    let app = unique("apl-rs");
    apelora_persistence::ensure_application(&pool, &app)
        .await
        .expect("application");
    let name = format!("{app}-db-01");
    let target = json!({ "project": project, "name": name });
    let spec = json!({ "project": project, "name": name, "kind": "container", "start": true,
        "source": { "protocol": "simplestreams", "server": "https://images.linuxcontainers.org", "alias": "debian/13" },
        "owner": { "application": app, "module": "db-01", "replica": 1 },
        "volumes": [{ "name": "data", "sizeMib": 128, "path": "/srv/data", "pool": "local" }] });
    converge(&pool, &provider, "instance.create", &spec).await;
    let created = status(&provider, &project, &name).await.expect("created");
    let volume = format!("{name}-data");
    let sh = |script: &'static str| {
        let provider = provider.clone();
        let (p, n) = (project.clone(), name.clone());
        async move {
            provider
                .run_probe(&p, &n, &["sh".into(), "-c".into(), script.into()], 0, 10)
                .await
                .expect("probe")
                .exit
        }
    };
    assert_eq!(sh("echo first > /srv/data/marker && sync").await, Some(0));

    // Back up (the job the Console's "Back up now" queues).
    let backup_payload = json!({ "application": app, "project": project, "module": "db-01",
        "volume": volume, "pool": "local", "member": created.location, "sizeMib": 128, "backupPool": backup_pool });
    converge(&pool, &provider, "volume.backup", &backup_payload).await;
    let backup = apelora_persistence::volume_backups(&pool, &app, &volume)
        .await
        .expect("backups")
        .pop()
        .expect("a backup is recorded");

    // The data changes after the backup; then the instance stops, as the restore requires.
    assert_eq!(sh("echo second > /srv/data/marker && sync").await, Some(0));
    let running = json!({ "application": app, "project": project, "module": "db-01", "volume": volume,
        "pool": "local", "member": created.location, "sizeMib": 128, "backup": backup.name,
        "backupPool": backup.storage_pool, "backupMember": backup.member, "safetyPool": backup_pool });
    let refused = finish(&pool, &provider, "volume.restore", &running).await;
    assert_eq!(refused.status, "failed", "refused while the instance runs");
    assert!(
        refused.last_error.unwrap_or_default().contains("stop"),
        "the reason says to stop it"
    );
    converge(&pool, &provider, "instance.stop", &target).await;
    converge(&pool, &provider, "volume.restore", &running).await;
    converge(&pool, &provider, "instance.start", &target).await;
    assert_eq!(
        sh("grep -qx first /srv/data/marker").await,
        Some(0),
        "the data is the backup's"
    );
    let backups = apelora_persistence::volume_backups(&pool, &app, &volume)
        .await
        .expect("backups");
    assert!(
        backups.iter().any(|b| b.before_restore.is_some()),
        "the safety backup is recorded with the restore: {backups:?}"
    );

    converge(&pool, &provider, "instance.delete", &target).await;
    let member = Some(created.location.as_str());
    for b in &backups {
        let m = Some(b.member.as_str());
        let _ = provider
            .delete_volume(&project, &b.storage_pool, m, &b.name, &app)
            .await;
    }
    let _ = provider
        .delete_volume(&project, "local", member, &volume, &app)
        .await;
}

/// The instance detail and logs the Console shows, read from a real running container: facts,
/// addresses and disks, the log files Incus keeps, a bounded tail of each and the console log.
#[ignore = "needs a disposable Incus: scripts/test-cluster/up.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_real_instance_has_a_detail_log_files_and_a_bounded_console_log(pool: Pool) {
    let provider = real_incus();
    let project = project();
    let name = unique("apl-logs");
    let target = json!({ "project": project, "name": name });
    let spec = json!({ "project": project, "name": name, "kind": "container", "start": true,
        "source": { "protocol": "simplestreams", "server": "https://images.linuxcontainers.org", "alias": "debian/13" },
        "config": { "limits.cpu": "1", "limits.memory": "512MiB" } });
    converge(&pool, &provider, "instance.create", &spec).await;

    let view = provider
        .instance_view(&project, &name)
        .await
        .expect("detail")
        .expect("exists");
    assert_eq!(
        (view.name.as_str(), view.status.as_str()),
        (name.as_str(), "Running")
    );
    assert_eq!(view.limits.cpu.as_deref(), Some("1"));
    assert_eq!(view.limits.memory.as_deref(), Some("512MiB"));
    assert!(
        view.usage.is_some(),
        "a running container reports its usage"
    );
    assert!(!view.disks.is_empty(), "the root disk is listed");

    let files = provider
        .instance_logs(&project, &name)
        .await
        .expect("log files");
    eprintln!("log files: {files:?}");
    assert!(!files.is_empty(), "Incus keeps log files for a container");
    for f in &files {
        let tail = provider
            .instance_log(&project, &name, f, 1024)
            .await
            .expect("a log file");
        assert!(tail.content.len() <= 1024, "{f}: a tail is bounded");
        assert!(
            !tail.truncated || tail.size > 1024,
            "{f}: truncated only when larger"
        );
    }
    // A name that is not one of the listed files is refused before it reaches Incus.
    assert!(
        provider
            .instance_log(&project, &name, "../../etc/passwd", 1024)
            .await
            .is_err()
    );
    let console = provider.console_log(&project, &name, 1024).await;
    eprintln!(
        "console: {:?}",
        console.as_ref().map(|c| (c.size, c.truncated))
    );
    assert!(console.is_ok(), "{console:?}");

    converge(&pool, &provider, "instance.delete", &target).await;
}

/// A cluster setting is changed through the administrator identity and put back exactly as it
/// was (set or unset), as the Console's change does after its review and approval. The
/// project-restricted deploy identity must not be able to change it.
#[ignore = "needs a disposable Incus with the admin identity: scripts/test-cluster/up.sh"]
#[tokio::test]
async fn a_cluster_setting_is_changed_by_the_admin_identity_and_restored() {
    use apelora_incus_adapter::config::admin_provider_from_env;
    use std::collections::BTreeMap;
    let deploy = real_incus();
    let admin = admin_provider_from_env(|k| std::env::var(k).ok(), &deploy)
        .expect("admin configuration")
        .expect("APELORA_INCUS_ADMIN_CLIENT_CERT/KEY are set");
    let key = "cluster.max_standby";
    let before = admin
        .server_config(&[key])
        .await
        .expect("read")
        .get(key)
        .cloned();
    let wanted = if before.as_deref() == Some("1") {
        "2"
    } else {
        "1"
    };
    let change = |v: Option<&str>| BTreeMap::from([(key.to_owned(), v.map(str::to_owned))]);
    assert!(
        deploy
            .set_server_config(&change(Some(wanted)))
            .await
            .is_err(),
        "the project-restricted identity must not change cluster settings"
    );
    admin
        .set_server_config(&change(Some(wanted)))
        .await
        .expect("changed");
    let now = admin.server_config(&[key]).await.expect("read");
    assert_eq!(now.get(key).map(String::as_str), Some(wanted));
    admin
        .set_server_config(&change(before.as_deref()))
        .await
        .expect("restored");
    let after = admin.server_config(&[key]).await.expect("read");
    assert_eq!(after.get(key), before.as_ref(), "put back as it was");
    // A key outside the allowed list never reaches Incus.
    assert!(
        admin
            .set_server_config(&BTreeMap::from([(
                "core.https_address".to_owned(),
                Some(":8443".to_owned())
            )]))
            .await
            .is_err()
    );
}

/// Application operations on the real cluster (ADR 0015): a graceful shutdown, an immediate stop
/// (its instance jobs carry `force`) and a migration of every instance to another member, each
/// confirmed against what Incus reports. Needs a second online member and member targeting
/// allowed on the project, like the move test above.
#[ignore = "needs a disposable Incus with two members and member targeting allowed: scripts/test-cluster/up.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn an_application_is_shut_down_stopped_at_once_and_migrated(pool: Pool) {
    use apelora_persistence::lifecycle::{NewOperation, create_operation, get_operation};
    let provider = real_incus();
    let project = project();
    let app = apelora_persistence::ensure_application(&pool, "apl-op-test")
        .await
        .expect("application");
    let names = [unique("apl-op-web"), unique("apl-op-db")];
    for (module, name) in ["web", "db"].iter().zip(&names) {
        converge(&pool, &provider, "instance.create", &json!({ "project": project, "name": name, "kind": "container", "start": true,
            "source": { "protocol": "simplestreams", "server": "https://images.linuxcontainers.org", "alias": "debian/13" },
            "owner": { "application": "apl-op-test", "module": module, "replica": 1 } })).await;
    }
    let step = |module: &str, name: &str, payload: Option<Value>| {
        let mut s =
            json!({ "project": project, "name": name, "module": module, "state": "pending" });
        if let Some(p) = payload {
            s["payload"] = p;
        }
        s
    };
    let run = |kind: &'static str, phases: Value| {
        let (pool, provider, project, app_id) =
            (pool.clone(), provider.clone(), project.clone(), app.id);
        async move {
            let (op, _) = create_operation(
                &pool,
                &NewOperation {
                    application_id: app_id,
                    project: &project,
                    kind,
                    phases: &phases,
                    requested_by: "real-incus-test",
                    confirmed: true,
                    key: None,
                    request_id: None,
                },
            )
            .await
            .expect("operation");
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
            loop {
                while run_once(&pool, &provider, "real-incus-test")
                    .await
                    .expect("run")
                {}
                let now = get_operation(&pool, op.id)
                    .await
                    .expect("read")
                    .expect("row");
                if matches!(now.state.as_str(), "succeeded" | "failed" | "cancelled") {
                    return now;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "{kind} still {} after ten minutes",
                    now.state
                );
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
        }
    };
    let both = |action: &str| {
        json!([
            { "action": action, "state": "pending", "steps": [step("web", &names[0], None)] },
            { "action": action, "state": "pending", "steps": [step("db", &names[1], None)] },
        ])
    };
    let statuses = || async {
        let mut v = Vec::new();
        for n in &names {
            v.push(status(&provider, &project, n).await.expect("exists").status);
        }
        v
    };

    // Shut down: graceful, in order.
    let done = run("shutdown", both("stop")).await;
    assert_eq!(done.state, "succeeded", "{:?}", done.error);
    assert_eq!(statuses().await, ["Stopped", "Stopped"]);

    // Start and Restart through their operations, providers first: both end up running.
    let done = run("start", both("start")).await;
    assert_eq!(done.state, "succeeded", "{:?}", done.error);
    assert_eq!(statuses().await, ["Running", "Running"]);
    let restart = json!([
        { "action": "stop", "state": "pending", "steps": [step("web", &names[0], None)] },
        { "action": "start", "state": "pending", "steps": [step("web", &names[0], None)] },
    ]);
    let done = run("restart", restart).await;
    assert_eq!(done.state, "succeeded", "{:?}", done.error);
    assert_eq!(statuses().await, ["Running", "Running"]);

    // Stop at once: the instance jobs say `force`, and Incus agrees they are stopped.
    let done = run("stop", both("poweroff")).await;
    assert_eq!(done.state, "succeeded", "{:?}", done.error);
    assert_eq!(statuses().await, ["Stopped", "Stopped"]);
    let forced: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM jobs WHERE kind = 'instance.stop' AND payload->>'force' = 'true'",
    )
    .fetch_one(&pool)
    .await
    .expect("count");
    assert_eq!(forced, 2);
    for n in &names {
        converge(
            &pool,
            &provider,
            "instance.start",
            &json!({ "project": project, "name": n }),
        )
        .await;
    }

    // Migrate both to a member that runs neither, confirmed on the new member and running again.
    let members: Vec<String> = provider
        .cluster_members()
        .await
        .expect("members")
        .into_iter()
        .filter(|m| m.status == "Online")
        .map(|m| m.name)
        .collect();
    let mut at = Vec::new();
    for n in &names {
        at.push(
            status(&provider, &project, n)
                .await
                .expect("exists")
                .location,
        );
    }
    let target = members
        .iter()
        .find(|m| !at.contains(m))
        .or_else(|| members.iter().find(|m| **m != at[0]))
        .expect("a target member")
        .clone();
    let steps: Vec<Value> = names
        .iter()
        .zip(["web", "db"])
        .zip(&at)
        .filter(|((_, _), from)| **from != target)
        .map(|((n, module), from)| {
            step(module, n, Some(json!({ "project": project, "name": n, "application": "apl-op-test", "from": from, "target": target, "live": false, "startAfter": true, "volumes": [] })))
        })
        .collect();
    assert!(!steps.is_empty(), "something has to move");
    let done = run(
        "migrate",
        json!([{ "action": "migrate", "state": "pending", "steps": steps }]),
    )
    .await;
    assert_eq!(done.state, "succeeded", "{:?}", done.error);
    for n in &names {
        let now = status(&provider, &project, n).await.expect("exists");
        assert_eq!(
            (now.location.as_str(), now.status.as_str()),
            (target.as_str(), "Running"),
            "{n}"
        );
    }
    for n in &names {
        converge(
            &pool,
            &provider,
            "instance.delete",
            &json!({ "project": project, "name": n }),
        )
        .await;
    }
}
