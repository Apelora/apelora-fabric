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
/// (`npm run -s runtime-requests -w @apelora/appliance-catalog > requests.json`).
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
    for entry in entries {
        let name = unique("apl-cat");
        let mut spec = entry["request"].clone();
        spec["project"] = json!(project);
        spec["name"] = json!(name);
        converge(&pool, &provider, "instance.create", &spec).await;
        let s = status(&provider, &project, &name).await.expect("created");
        assert_eq!((s.status.as_str(), s.managed), ("Running", true), "{entry}");
        eprintln!(
            "{} {}: running on {} from image {}",
            entry["class"],
            entry["config"],
            s.architecture,
            s.base_image.as_deref().unwrap_or("?")
        );
        converge(
            &pool,
            &provider,
            "instance.delete",
            &json!({ "project": project, "name": name }),
        )
        .await;
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

/// Two replicas of a stateless appliance land on different members (the anti-affinity
/// scriptlet, scripts/test-cluster/placement.sh), and the supervisor brings back a lost one,
/// again on the member without a sibling. Needs two members and the scriptlet installed.
#[ignore = "needs a disposable two-member Incus with the placement scriptlet: scripts/test-cluster/placement.sh"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn replicas_spread_across_members_and_lost_ones_return(pool: Pool) {
    use apelora_deployment_worker::supervisor::supervise_once;
    use apelora_domain::plan::{DesiredAppliance, deploy_spec};

    let provider = real_incus();
    let project = project();
    let application = unique("apl-ha");
    let app = apelora_persistence::ensure_application(&pool, &application)
        .await
        .expect("application");
    let web: DesiredAppliance = serde_json::from_value(json!({
        "appliance": "web-01",
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
