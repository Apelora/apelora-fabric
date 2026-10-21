// Tenancy against a real Incus (the disposable test cluster, docs/development/TEST-CLUSTER.md).
// Ignored by default. It creates exactly one project, `apelora-test-tenancy-<random>`, prepares
// it the way an environment's project is prepared (the same code as the API), checks that Incus
// enforces the mirrored limits, and deletes it again. It never touches another project.
//
// It prints a dry run of every Incus request first and changes nothing unless
// APELORA_TEST_TENANCY_APPLY=1:
//
// ```sh
// source ~/.config/apelora/test-cluster/env
// APELORA_NETWORK_UPLINK=incusbr0 APELORA_SERVICE_ADDRESSES=198.18.100.0/28 \
//   cargo test -p apelora-control-plane real_tenancy -- --ignored --nocapture            # dry run
// APELORA_TEST_TENANCY_APPLY=1 … cargo test -p apelora-control-plane real_tenancy -- --ignored --nocapture
// ```

use apelora_domain::tenancy::{self as t, ProvisionSettings, QuotaLimits, QuotaScopeKind};
use apelora_domain::{ImageProtocol, ImageSource, InstanceKind, InstanceSpec};
use apelora_incus_adapter::config::{admin_provider_from_env, provider_from_env};
use apelora_incus_adapter::{AdapterError, IncusProfile, Provider};

use super::*;

fn env(k: &str) -> Option<String> {
    std::env::var(k).ok().filter(|v| !v.trim().is_empty())
}

fn spec(project: &str, config: &[(&str, &str)]) -> InstanceSpec {
    InstanceSpec {
        project: project.into(),
        name: "apelora-test-quota".into(),
        kind: InstanceKind::Container,
        source: ImageSource {
            protocol: ImageProtocol::Simplestreams,
            server: "https://images.linuxcontainers.org".into(),
            alias: "debian/13".into(),
            fingerprint: None,
        },
        profiles: vec![],
        config: config
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        start: false,
        architectures: vec![],
        owner: None,
        network: None,
        volumes: vec![],
    }
}

fn principal() -> Principal {
    Principal {
        name: "real-tenancy-test".into(),
        role: Role::parse("fabric-admin"),
        caller: None,
        session: None,
        personal_token: None,
    }
}

async fn set_quota(pool: &Pool, env: uuid::Uuid, q: QuotaLimits) {
    let mut tx = apelora_persistence::begin(pool).await.expect("tx");
    apelora_persistence::tenancy::set_quota_in(
        &mut tx,
        QuotaScopeKind::Environment,
        env,
        &q,
        "test",
    )
    .await
    .expect("quota");
    apelora_persistence::commit(tx).await.expect("commit");
}

#[ignore = "needs the disposable Incus test cluster; prints a dry run unless APELORA_TEST_TENANCY_APPLY=1"]
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn real_tenancy_project_is_created_restricted_granted_limited_and_removed(pool: Pool) {
    let get = |k: &str| std::env::var(k).ok();
    let deploy = provider_from_env(get).expect("Incus settings");
    assert!(
        matches!(deploy, Provider::Incus(_)),
        "refusing to run against the mock: source ~/.config/apelora/test-cluster/env"
    );
    let admin = admin_provider_from_env(get, &deploy)
        .expect("admin settings")
        .expect("the admin identity (APELORA_INCUS_ADMIN_CLIENT_CERT) is needed");
    let pem = std::fs::read(env("APELORA_INCUS_CLIENT_CERT").expect("client certificate"))
        .expect("read certificate");
    let fp = super::tenancy::certificate_fingerprint(&pem).expect("fingerprint");
    let suffix: String = uuid::Uuid::new_v4().simple().to_string()[..6].to_owned();
    let env_slug = format!("tenancy-{suffix}");
    let name =
        apelora_auth::backend::incus_project_name("apelora", "test", &env_slug).expect("name");
    assert!(name.starts_with("apelora-test-tenancy-"), "{name}");
    let settings = ProvisionSettings {
        uplink: env("APELORA_NETWORK_UPLINK"),
        service_range: env("APELORA_SERVICE_ADDRESSES"),
    };
    let quota_a = QuotaLimits {
        instances: Some(0),
        cpu: Some(2),
        memory_gib: Some(2),
        disk_gib: Some(30),
    };
    let quota_b = QuotaLimits {
        instances: Some(5),
        ..quota_a
    };
    let short = &fp[..12];

    println!("== DRY RUN: every Incus request this test makes (project {name} only) ==");
    let baseline = t::baseline_config("<environment id>", &settings);
    for line in [
        format!("GET    /1.0/projects/{name}                      admin   (must answer 404: the project is new)"),
        format!("POST   /1.0/projects  name={name} config={baseline:?}   admin"),
        format!("GET    /1.0/profiles/default?project={name}      admin"),
        "GET    /1.0/profiles/default?project=default     admin   (read only: root disk pool to copy)".to_owned(),
        format!("PUT    /1.0/profiles/default?project={name}      admin   root disk (pool from default, size {}GiB), limits.cpu=1, limits.memory=1GiB{}", t::DEFAULT_ROOT_DISK_GIB, if settings.uplink.is_some() { "" } else { ", eth0 copied" }),
        format!("GET    /1.0/certificates/{short}…             admin   (the deploy identity's trust entry)"),
        format!("PATCH  /1.0/certificates/{short}…             admin   projects = <current list> + {name}"),
        format!("GET    /1.0/certificates/{short}…             admin   (read back)"),
        format!("GET    /1.0/projects/{name}                      deploy  (the deploy identity can now use it)"),
        format!("GET    /1.0/projects/{name}                      admin   then PUT /1.0/projects/{name} with {:?}", t::limit_config(&quota_a)),
        format!("GET    /1.0/projects/{name}/state                deploy"),
        format!("POST   /1.0/instances?project={name}             deploy  (expected refusal: limits.instances=0; refused before any image is fetched)"),
        format!("PUT    /1.0/projects/{name}                      admin   limits {:?}", t::limit_config(&quota_b)),
        format!("POST   /1.0/instances?project={name}             deploy  limits.cpu=4 (expected refusal: limits.cpu=2)"),
        format!("POST   /1.0/projects  name={name}-x              deploy  (expected refusal: a restricted certificate cannot create projects)"),
        format!("PUT    /1.0/profiles/default?project={name}      admin   empty profile (before deleting)"),
        format!("DELETE /1.0/projects/{name}                      admin"),
        format!("GET    /1.0/certificates/{short}…             admin   (the project list is as before)"),
    ] {
        println!("  {line}");
    }
    if env("APELORA_TEST_TENANCY_APPLY").as_deref() != Some("1") {
        println!("== dry run only: set APELORA_TEST_TENANCY_APPLY=1 to apply ==");
        return;
    }

    let before = admin
        .trust_entry(&fp)
        .await
        .expect("trust")
        .expect("the deploy identity is trusted");
    println!(
        "deploy identity {short}…: restricted {}, projects {:?}",
        before.restricted, before.projects
    );
    assert_eq!(
        admin.tenancy_project(&name).await.expect("read"),
        None,
        "refusing: {name} already exists"
    );

    // The environment, as the API would have created it, with a quota.
    let mut tx = apelora_persistence::begin(&pool).await.expect("tx");
    let o = apelora_persistence::identity::create_organization(&mut *tx, "apelora", "Apelora lab")
        .await
        .expect("org");
    let p = apelora_persistence::identity::create_project(&mut *tx, o.id, "test", "Test")
        .await
        .expect("project");
    let e = apelora_persistence::identity::create_environment(
        &mut *tx,
        p.id,
        &env_slug,
        "Tenancy test",
        apelora_auth::EnvKind::Development,
        Some(&name),
    )
    .await
    .expect("env");
    apelora_persistence::commit(tx).await.expect("commit");
    set_quota(&pool, e.id, quota_a).await;

    let state = AppState::new(deploy.clone(), Auth::Tokens(pool.clone()))
        .with_admin(Some(admin.clone()))
        .with_deploy_fingerprint(Some(fp.clone()))
        .with_network_uplink(settings.uplink.clone())
        .with_service_range(settings.service_range.clone())
        .with_volume_pool(env("APELORA_VOLUME_POOL"));
    let chain = apelora_persistence::tenancy::environment_chain(&pool, e.id)
        .await
        .expect("db")
        .expect("env");

    // Everything after this point runs, then the project is removed whatever happened.
    let outcome = async {
        let prepared = super::tenancy::prepare(&state, &principal(), &chain)
            .await
            .map_err(|e| format!("{} {}", e.code, e.message))?;
        println!(
            "prepared: {:?} — {}; findings {:?}; limits {:?}",
            prepared.state,
            prepared.detail,
            prepared.findings,
            prepared
                .limits
                .as_ref()
                .map(|l| (l.result, &l.limits, &l.detail))
        );
        if prepared.state != t::TenancyIncusState::Created {
            return Err(format!("not created: {}", prepared.detail));
        }
        let seen = deploy
            .tenancy_project(&name)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("the deploy identity cannot see the project")?;
        for (k, v) in [
            ("restricted", "true"),
            ("restricted.containers.nesting", "block"),
            ("restricted.devices.disk", "managed"),
            ("restricted.cluster.target", "allow"),
            ("limits.instances", "0"),
            ("limits.cpu", "2"),
            ("limits.memory", "2GiB"),
            ("limits.disk", "30GiB"),
        ] {
            if seen.config.get(k).map(String::as_str) != Some(v) {
                return Err(format!("{k} is {:?}, expected {v}", seen.config.get(k)));
            }
        }
        println!(
            "Incus config as the deploy identity reads it: {:?}",
            seen.config
        );
        let profile = admin
            .tenancy_profile(&name, "default")
            .await
            .map_err(|e| e.to_string())?
            .ok_or("no profile")?;
        println!(
            "default profile: config {:?}, devices {:?}",
            profile.config, profile.devices
        );
        let used = deploy
            .tenancy_project_use(&name)
            .await
            .map_err(|e| e.to_string())?;
        println!("project state (deploy identity): {used:?}");
        if used.instances.limit != Some(0) || used.cpu.limit != Some(2) {
            return Err(format!("state does not report the limits: {used:?}"));
        }
        let refused = deploy.create_instance(&spec(&name, &[])).await;
        println!("instance with limits.instances=0: {refused:?}");
        match &refused {
            Err(e) if e.to_string().contains("maximum number of instances") => {}
            other => {
                return Err(format!(
                    "expected Incus to refuse the instance, got {other:?}"
                ));
            }
        }
        set_quota(&pool, e.id, quota_b).await;
        let chain = apelora_persistence::tenancy::environment_chain(&pool, e.id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("env")?;
        let mirrored = super::tenancy::mirror_limits(&state, &principal(), &chain)
            .await
            .map_err(|e| format!("{} {}", e.code, e.message))?;
        println!(
            "mirrored: {:?} {:?} {}",
            mirrored.result, mirrored.limits, mirrored.detail
        );
        let refused = deploy
            .create_instance(&spec(
                &name,
                &[("limits.cpu", "4"), ("limits.memory", "1GiB")],
            ))
            .await;
        println!("instance asking for 4 CPUs with limits.cpu=2: {refused:?}");
        match &refused {
            Err(e)
                if e.to_string().contains("limits.cpu")
                    || e.to_string().contains("maximum aggregate") => {}
            other => return Err(format!("expected Incus to refuse 4 CPUs, got {other:?}")),
        }
        let refused = deploy
            .create_tenancy_project(&format!("{name}-x"), "", &Default::default())
            .await;
        println!("project creation with the deploy identity: {refused:?}");
        if !matches!(refused, Err(AdapterError::Authentication(_))) {
            return Err(format!(
                "expected the restricted certificate to be refused, got {refused:?}"
            ));
        }
        Ok::<(), String>(())
    }
    .await;

    // Clean up: the instance (if Incus created one after all), the profile's devices, the project.
    if let Ok(Some(_)) = deploy.instance(&name, "apelora-test-quota").await {
        let op = deploy.delete_instance(&name, "apelora-test-quota").await;
        println!("unexpected instance deleted: {op:?}");
        if let Ok(op) = op {
            let _ = deploy
                .wait_operation(&op, std::time::Duration::from_secs(60))
                .await;
        }
    }
    let emptied = admin
        .update_tenancy_profile(&name, "default", &IncusProfile::default())
        .await;
    let deleted = admin.delete_tenancy_project(&name).await;
    println!("cleanup: profile emptied {emptied:?}, project deleted {deleted:?}");
    let after = admin
        .trust_entry(&fp)
        .await
        .expect("trust")
        .expect("trusted");
    println!("deploy identity projects after: {:?}", after.projects);
    assert_eq!(
        admin.tenancy_project(&name).await.expect("read"),
        None,
        "{name} is gone"
    );
    let mut was = before.projects.clone();
    let mut now = after.projects.clone();
    was.sort();
    now.sort();
    assert_eq!(
        now, was,
        "Incus removed the deleted project from the deploy identity's list"
    );
    outcome.expect("tenancy against real Incus");
    deleted.expect("project deleted");
}
