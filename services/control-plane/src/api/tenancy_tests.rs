// Tenancy isolation and quotas through the real router, against PostgreSQL and the mock Incus
// (ADR 0012, docs/design/tenancy-isolation.md). MOCK VERIFIED: the mock behaves like Incus for
// the calls used here (restricted projects, profiles, the trust store, limits refused below use).

use apelora_domain::tenancy::ENVIRONMENT_KEY;
use apelora_incus_adapter::{MOCK_DEPLOY_FINGERPRINT, MockIncus};
use serde_json::{Value, json};
use uuid::Uuid;

use super::identity_tests::{Lab, World, deploy_body, lab, world};
use super::tests::module;
use super::*;

fn mock(l: &Lab) -> MockIncus {
    match &*l.state.provider {
        Provider::Mock(m) => m.clone(),
        Provider::Incus(_) => unreachable!("tests use the mock"),
    }
}

async fn env_id(l: &Lab, project: Uuid, token: &str) -> (Uuid, Value) {
    let (s, envs) = l
        .req(
            "GET",
            &format!("/api/v1/identity/projects/{project}/environments"),
            token,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{envs}");
    let env = envs["items"][0].clone();
    (env["id"].as_str().expect("id").parse().expect("uuid"), env)
}

async fn actions(l: &Lab, prefix: &str) -> Vec<(String, String)> {
    sqlx::query_as::<_, (String, String)>(
        "SELECT action, outcome FROM audit_events WHERE action LIKE $1 ORDER BY id",
    )
    .bind(format!("{prefix}%"))
    .fetch_all(&l.pool)
    .await
    .expect("audit")
}

/// A person with a role at a scope; returns their token.
async fn person(l: &Lab, w: &World, sub: &str, role: &str, scope: Value) -> String {
    let id = l.user(sub, sub).await;
    let (s, v) = l.grant(&w.admin, id, role, scope).await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    l.token(sub, sub)
}

fn two_modules(project: &str) -> Value {
    json!({ "project": project, "document": {}, "modules": [module("web-01"), module("db-01")] })
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn an_environment_gets_a_restricted_incus_project_its_deploy_identity_may_use(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let m = mock(&l);
    let (env, dto) = env_id(&l, w.acme_prod, &w.admin).await;
    assert_eq!(
        (
            dto["incusState"].as_str(),
            dto["incusProvisioned"].as_bool()
        ),
        (Some("created"), Some(true)),
        "{dto}"
    );
    assert_eq!(
        dto["incusProjectConfig"]["restricted.cluster.target"],
        "allow"
    );
    {
        let t = m.tenancy();
        let p = &t.projects["acme-prod-live"];
        for (k, v) in [
            ("restricted", "true"),
            ("restricted.containers.nesting", "block"),
            ("restricted.containers.privilege", "unprivileged"),
            ("restricted.devices.disk", "managed"),
            ("restricted.devices.gpu", "block"),
            ("features.profiles", "true"),
            ("features.networks", "false"),
        ] {
            assert_eq!(p.config.get(k).map(String::as_str), Some(v), "{k}");
        }
        assert_eq!(p.config[ENVIRONMENT_KEY], env.to_string());
        assert!(
            !p.config.keys().any(|k| k.starts_with("limits.")),
            "no quota yet"
        );
        let profile = &t.profiles[&("acme-prod-live".to_owned(), "default".to_owned())];
        assert_eq!(profile.devices["root"]["pool"], "mock-local");
        assert_eq!(profile.devices["root"]["size"], "10GiB");
        assert_eq!(
            profile.devices["eth0"]["network"], "mock-br0",
            "without an uplink the default project's network is used"
        );
        assert_eq!(profile.config["limits.cpu"], "1");
        let grants = &t.trust[MOCK_DEPLOY_FINGERPRINT].projects;
        assert!(
            grants.iter().any(|p| p == "acme-prod-live") && grants.iter().any(|p| p == "apelora"),
            "the deploy identity keeps its projects and gains the new one: {grants:?}"
        );
    }
    assert!(
        actions(&l, "environment.incus_project.prepare")
            .await
            .iter()
            .filter(|(_, o)| o == "success")
            .count()
            >= 2,
        "one per environment of the world"
    );
    // Preparing again changes nothing in Incus.
    let (s, again) = l
        .req(
            "POST",
            &format!("/api/v1/identity/environments/{env}/incus-project"),
            &w.admin,
            None,
        )
        .await;
    assert_eq!(
        (s, again["state"].as_str()),
        (StatusCode::OK, Some("created")),
        "{again}"
    );
    let creates = m
        .tenancy()
        .calls
        .iter()
        .filter(|c| *c == "POST /1.0/projects acme-prod-live")
        .count();
    assert_eq!(creates, 1);
    // Someone without environment.manage on the project cannot.
    let bob = person(
        &l,
        &w,
        "bob",
        "organization-owner",
        json!({ "type": "organization", "id": w.other }),
    )
    .await;
    let (s, _) = l
        .req(
            "POST",
            &format!("/api/v1/identity/environments/{env}/incus-project"),
            &bob,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn without_an_admin_identity_environments_stay_pending_and_deployments_still_work(
    pool: Pool,
) {
    let mut l = lab(pool);
    l.state = l.state.clone().with_admin(None);
    let w = world(&l).await;
    let (env, dto) = env_id(&l, w.acme_prod, &w.admin).await;
    assert_eq!(dto["incusState"], "pending");
    assert!(
        dto["incusDetail"]
            .as_str()
            .is_some_and(|d| d.contains("APELORA_INCUS_ADMIN_CLIENT_CERT")),
        "{dto}"
    );
    let (s, v) = l
        .req(
            "POST",
            &format!("/api/v1/identity/environments/{env}/incus-project"),
            &w.admin,
            None,
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (
            StatusCode::SERVICE_UNAVAILABLE,
            Some("cluster_admin_not_configured")
        )
    );
    // A quota is still stored and checked by Apelora; its mirror is skipped with the reason.
    let (s, v) = l
        .req(
            "PUT",
            &format!("/api/v1/identity/quotas/environment/{env}"),
            &w.admin,
            Some(json!({ "instances": 5 })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["environments"][0]["result"], "skipped");
    let (s, d) = l
        .req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &w.admin,
            Some(deploy_body("acme-prod-live")),
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{d}");
    assert_eq!(d["plan"]["quota"]["allowed"], true, "{d}");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn existing_projects_are_adopted_only_when_restricted(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let m = mock(&l);
    {
        let mut t = m.tenancy();
        for (name, config) in [
            ("legacy-open", json!({})),
            (
                "legacy-closed",
                json!({ "restricted": "true", "restricted.devices.gpu": "allow", "restricted.snapshots": "allow", "restricted.cluster.target": "allow" }),
            ),
        ] {
            t.projects.insert(
                name.into(),
                apelora_domain::tenancy::TenancyIncusProject {
                    name: name.into(),
                    description: "made by hand".into(),
                    config: serde_json::from_value(config).expect("config"),
                },
            );
        }
    }
    let create = |slug: &'static str, existing: &'static str| {
        let (l, admin, project) = (&l, w.admin.clone(), w.acme_prod);
        async move {
            l.req(
                "POST",
                &format!("/api/v1/identity/projects/{project}/environments"),
                &admin,
                Some(json!({ "slug": slug, "name": slug, "kind": "development", "incusProject": existing })),
            )
            .await
        }
    };
    let (s, open) = create("open", "legacy-open").await;
    assert_eq!(s, StatusCode::CREATED, "{open}");
    assert_eq!(open["incusState"], "pending");
    assert!(
        open["incusDetail"]
            .as_str()
            .is_some_and(|d| d.contains("not restricted")),
        "{open}"
    );
    let (s, closed) = create("closed", "legacy-closed").await;
    assert_eq!(s, StatusCode::CREATED, "{closed}");
    assert_eq!(closed["incusState"], "adopted", "{closed}");
    // Apelora keeps the adopted project's restrictions, and says what it allows.
    assert_eq!(
        m.tenancy().projects["legacy-closed"].config["restricted.devices.gpu"],
        "allow"
    );
    let (_, overview) = l
        .req("GET", "/api/v1/identity/tenancy", &w.admin, None)
        .await;
    let envs: Vec<Value> = overview["organizations"]
        .as_array()
        .expect("orgs")
        .iter()
        .flat_map(|o| o["projects"].as_array().cloned().unwrap_or_default())
        .flat_map(|p| p["environments"].as_array().cloned().unwrap_or_default())
        .collect();
    let closed = envs.iter().find(|e| e["slug"] == "closed").expect("closed");
    assert_eq!(
        closed["findings"][0]["key"], "restricted.devices.gpu",
        "{closed}"
    );
    assert_eq!(closed["findings"][0]["severity"], "warning");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn quotas_are_set_by_who_may_and_mirrored_as_the_tightest_limit(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let m = mock(&l);
    let (env, _) = env_id(&l, w.acme_prod, &w.admin).await;
    let limits = || m.tenancy().projects["acme-prod-live"].config.clone();

    // A platform administrator sets the organisation's quota; Incus gets it.
    let (s, v) = l
        .req(
            "PUT",
            &format!("/api/v1/identity/quotas/organization/{}", w.acme),
            &w.admin,
            Some(json!({ "instances": 2, "cpu": 4, "memoryGib": 8, "diskGib": 100 })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["environments"][0]["result"], "applied", "{v}");
    let c = limits();
    assert_eq!(
        (
            c["limits.instances"].as_str(),
            c["limits.cpu"].as_str(),
            c["limits.memory"].as_str(),
            c["limits.disk"].as_str()
        ),
        ("2", "4", "8GiB", "100GiB")
    );

    let alice = person(
        &l,
        &w,
        "alice",
        "organization-owner",
        json!({ "type": "organization", "id": w.acme }),
    )
    .await;
    let carol = person(
        &l,
        &w,
        "carol",
        "project-administrator",
        json!({ "type": "project", "id": w.acme_prod }),
    )
    .await;
    let dave = person(
        &l,
        &w,
        "dave",
        "viewer",
        json!({ "type": "organization", "id": w.acme }),
    )
    .await;
    let bob = person(
        &l,
        &w,
        "bob",
        "organization-owner",
        json!({ "type": "organization", "id": w.other }),
    )
    .await;
    let project = format!("/api/v1/identity/quotas/project/{}", w.acme_prod);
    let org = format!("/api/v1/identity/quotas/organization/{}", w.acme);
    let environment = format!("/api/v1/identity/quotas/environment/{env}");

    // The organisation owner divides it, but cannot give more than she has, nor raise her own.
    let (s, v) = l
        .req("PUT", &project, &alice, Some(json!({ "instances": 3 })))
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("quota_above_parent")),
        "{v}"
    );
    let (s, v) = l
        .req(
            "PUT",
            &project,
            &alice,
            Some(json!({ "instances": 1, "cpu": 4, "memoryGib": 8, "diskGib": 50 })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(limits()["limits.instances"], "1", "the tightest quota wins");
    assert_eq!(limits()["limits.disk"], "50GiB");
    assert_eq!(
        l.req("PUT", &org, &alice, Some(json!({ "instances": 9 })))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // The project administrator sets the environment's, not the project's.
    let (s, v) = l
        .req(
            "PUT",
            &environment,
            &carol,
            Some(json!({ "instances": 1, "cpu": 2, "memoryGib": 8, "diskGib": 50 })),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(limits()["limits.cpu"], "2");
    assert_eq!(
        l.req("PUT", &project, &carol, Some(json!({ "instances": 1 })))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // A viewer reads, cannot change; another organisation's owner neither reads nor changes.
    let (s, v) = l.req("GET", &project, &dave, None).await;
    assert_eq!(
        (s, v["canSet"].as_bool()),
        (StatusCode::OK, Some(false)),
        "{v}"
    );
    assert_eq!(v["quota"]["instances"], 1);
    assert_eq!(
        v["bound"]["instances"], 2,
        "the organisation's quota bounds it"
    );
    assert_eq!(v["usage"]["instances"], 0);
    assert_eq!(
        l.req("PUT", &project, &dave, Some(json!({ "instances": 1 })))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        l.req("GET", &project, &bob, None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        l.req("PUT", &environment, &bob, Some(json!({ "instances": 1 })))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // Invalid values and scopes.
    assert_eq!(
        l.req("PUT", &org, &w.admin, Some(json!({ "cpu": -1 })))
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        l.req(
            "GET",
            &format!("/api/v1/identity/quotas/region/{env}"),
            &w.admin,
            None
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        l.req(
            "GET",
            &format!("/api/v1/identity/quotas/project/{}", Uuid::new_v4()),
            &w.admin,
            None
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        l.req(
            "GET",
            &format!("/api/v1/identity/quotas/project/{}", Uuid::new_v4()),
            &bob,
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "no existence leak"
    );

    // Removing quotas removes the limits Apelora wrote, and only those.
    m.tenancy()
        .projects
        .get_mut("acme-prod-live")
        .expect("project")
        .config
        .insert("limits.processes".into(), "500".into());
    for path in [&environment, &project, &org] {
        let (s, v) = l.req("PUT", path, &w.admin, Some(json!({}))).await;
        assert_eq!(s, StatusCode::OK, "{v}");
        assert!(v["quota"].is_null());
    }
    let c = limits();
    assert!(
        !c.contains_key("limits.cpu") && !c.contains_key("limits.instances"),
        "{c:?}"
    );
    assert_eq!(c["limits.processes"], "500", "not Apelora's to remove");
    let audited = actions(&l, "quota.set").await;
    assert_eq!(audited.len(), 6, "{audited:?}");
    assert!(
        actions(&l, "environment.incus_limits.apply")
            .await
            .iter()
            .all(|(_, o)| o == "success")
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_deployment_over_quota_is_refused_before_anything_is_saved_or_queued(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let (s, _) = l
        .req(
            "PUT",
            &format!("/api/v1/identity/quotas/organization/{}", w.acme),
            &w.admin,
            Some(json!({ "instances": 1 })),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    // The plan shows the quota use and says applying would be refused.
    let (s, plan) = l
        .req(
            "POST",
            "/api/v1/applications/shop/plan",
            &w.admin,
            Some(two_modules("acme-prod-live")),
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{plan}");
    assert_eq!(plan["quota"]["allowed"], false, "{plan}");
    let dim = &plan["quota"]["scopes"][0]["dimensions"][0];
    assert_eq!(
        (
            dim["dimension"].as_str(),
            dim["used"].as_i64(),
            dim["adding"].as_i64(),
            dim["limit"].as_i64()
        ),
        (Some("instances"), Some(0), Some(2), Some(1))
    );
    assert!(
        plan["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|x| x.as_str().is_some_and(|x| x.starts_with("over quota"))),
        "{plan}"
    );
    // Applying is refused, and nothing is saved or queued.
    let (s, v) = l
        .req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &w.admin,
            Some(two_modules("acme-prod-live")),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("quota_exceeded")),
        "{v}"
    );
    assert!(
        v["error"]["message"].as_str().is_some_and(
            |m| m.contains("organization acme: 0 instances in use, this adds 2, quota 1")
        ),
        "{v}"
    );
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
        .fetch_one(&l.pool)
        .await
        .expect("jobs");
    let apps: i64 = sqlx::query_scalar("SELECT count(*) FROM applications WHERE name = 'shop'")
        .fetch_one(&l.pool)
        .await
        .expect("apps");
    assert_eq!((jobs, apps), (0, 0));
    // Within the quota it goes ahead and the response carries the check.
    let (s, d) = l
        .req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &w.admin,
            Some(deploy_body("acme-prod-live")),
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{d}");
    assert_eq!(d["plan"]["quota"]["allowed"], true);
    super::tests::drain(&l.pool, &l.state.provider).await;
    // Now the organisation is full: a second application and a raw instance are refused.
    let (s, v) = l
        .req(
            "POST",
            "/api/v1/applications/blog/deployments",
            &w.admin,
            Some(deploy_body("acme-prod-live")),
        )
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("quota_exceeded")),
        "{v}"
    );
    let mut spec = module("x")["spec"].clone();
    spec["project"] = json!("acme-prod-live");
    spec["name"] = json!("loose-01");
    let (s, v) = l
        .req("POST", "/api/v1/instances", &w.admin, Some(spec))
        .await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("quota_exceeded")),
        "{v}"
    );
    // Redeploying the same design adds nothing and is allowed.
    let (s, d) = l
        .req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &w.admin,
            Some(deploy_body("acme-prod-live")),
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{d}");
    // Another organisation is not affected.
    let (s, d) = l
        .req(
            "POST",
            "/api/v1/applications/web/deployments",
            &w.admin,
            Some(two_modules("other-web-live")),
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{d}");
    assert!(d["plan"]["quota"].is_null(), "no quota applies there: {d}");
}

/// Two modules with the given CPU limits.
fn sized(project: &str, web: &str, db: &str) -> Value {
    let mut web_m = module("web-01");
    web_m["spec"]["config"] = json!({ "limits.cpu": web, "limits.memory": "1GiB" });
    let mut db_m = module("db-01");
    db_m["spec"]["config"] = json!({ "limits.cpu": db, "limits.memory": "1GiB" });
    json!({ "project": project, "document": {}, "modules": [web_m, db_m] })
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn resizing_counts_only_the_increase_against_the_quota_and_queues_resize_jobs(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let deploy = |body: Value| {
        l.req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &w.admin,
            Some(body),
        )
    };
    let (s, d) = deploy(sized("acme-prod-live", "1", "1")).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{d}");
    super::tests::drain(&l.pool, &l.state.provider).await;
    let (s, _) = l
        .req(
            "PUT",
            &format!("/api/v1/identity/quotas/organization/{}", w.acme),
            &w.admin,
            Some(json!({ "cpu": 3 })),
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    // Both modules to 2 CPUs adds 2 to the 2 in use: over the quota of 3, nothing queued.
    let jobs_before: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
        .fetch_one(&l.pool)
        .await
        .expect("jobs");
    let (s, v) = deploy(sized("acme-prod-live", "2", "2")).await;
    assert_eq!(
        (s, v["error"]["code"].as_str()),
        (StatusCode::CONFLICT, Some("quota_exceeded")),
        "{v}"
    );
    let jobs: i64 = sqlx::query_scalar("SELECT count(*) FROM jobs")
        .fetch_one(&l.pool)
        .await
        .expect("jobs");
    assert_eq!(jobs, jobs_before);
    // One of them adds 1: within the quota, and one resize job is queued.
    let (s, d) = deploy(sized("acme-prod-live", "2", "1")).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{d}");
    let resize: Vec<&Value> = d["plan"]["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter(|i| i["action"] == "resize")
        .collect();
    assert_eq!(resize.len(), 1, "{d}");
    assert_eq!(resize[0]["module"], "web-01");
    assert_eq!(resize[0]["changes"][0]["to"], "2", "{d}");
    assert_eq!(d["jobs"].as_array().expect("jobs").len(), 1, "{d}");
    super::tests::drain(&l.pool, &l.state.provider).await;
    let now = l
        .state
        .provider
        .instance(
            "acme-prod-live",
            resize[0]["instance"].as_str().expect("instance"),
        )
        .await
        .expect("read")
        .expect("exists");
    assert_eq!(now.limits.get("limits.cpu").map(String::as_str), Some("2"));
    // A decrease is never refused for quota and adds nothing.
    let (s, d) = deploy(sized("acme-prod-live", "1", "1")).await;
    assert_eq!(s, StatusCode::ACCEPTED, "{d}");
    assert_eq!(
        actions(&l, "application.deploy.requested").await.len(),
        3,
        "every accepted deployment is audited, a refused one is not"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_limit_incus_refuses_leaves_the_quota_in_force_and_says_why(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let (env, _) = env_id(&l, w.acme_prod, &w.admin).await;
    let (s, d) = l
        .req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &w.admin,
            Some(two_modules("acme-prod-live")),
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{d}");
    super::tests::drain(&l.pool, &l.state.provider).await;
    let path = format!("/api/v1/identity/quotas/environment/{env}");
    let (s, v) = l
        .req("PUT", &path, &w.admin, Some(json!({ "instances": 1 })))
        .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["environments"][0]["result"], "failed", "{v}");
    assert!(
        v["environments"][0]["detail"]
            .as_str()
            .is_some_and(|d| d.contains("too low")),
        "{v}"
    );
    let (_, q) = l.req("GET", &path, &w.admin, None).await;
    assert_eq!(
        (
            q["quota"]["instances"].as_i64(),
            q["usage"]["instances"].as_i64()
        ),
        (Some(1), Some(2)),
        "{q}"
    );
    assert_eq!(
        actions(&l, "environment.incus_limits.apply").await,
        vec![(
            "environment.incus_limits.apply".to_owned(),
            "failed".to_owned()
        )]
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_tenancy_page_shows_what_each_caller_may_see(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let (s, all) = l
        .req("GET", "/api/v1/identity/tenancy", &w.admin, None)
        .await;
    assert_eq!(s, StatusCode::OK, "{all}");
    assert_eq!(all["adminConfigured"], true);
    assert_eq!(all["source"]["provider"], "mock");
    assert_eq!(all["baseline"]["restricted"], "true");
    let orgs = all["organizations"].as_array().expect("orgs");
    assert_eq!(
        orgs.iter()
            .map(|o| o["slug"].as_str().unwrap_or(""))
            .collect::<Vec<_>>(),
        ["acme", "default", "other"],
        "the default organisation holds the platform's deploy project"
    );
    let default_env = &orgs[1]["projects"][0]["environments"][0];
    assert_eq!(default_env["incusProject"], "apelora");
    assert_eq!(default_env["incusState"], "pending");
    assert!(
        default_env["incusDetail"]
            .as_str()
            .is_some_and(|d| d.contains("APELORA_DEPLOY_PROJECTS")),
        "{default_env}"
    );
    let acme = &orgs[0];
    let env = &acme["projects"][0]["environments"][0];
    assert_eq!(
        (env["incusProject"].as_str(), env["incusState"].as_str()),
        (Some("acme-prod-live"), Some("created")),
        "{env}"
    );
    assert_eq!(
        env["incus"]["restrictions"]["restricted.containers.nesting"],
        "block"
    );
    assert_eq!(env["incus"]["usage"]["instances"]["used"], 0);
    assert_eq!(
        (env["canSetQuota"].as_bool(), env["canPrepare"].as_bool()),
        (Some(true), Some(true))
    );
    assert_eq!(acme["usage"]["instances"], 0);
    // A project administrator sees her project only, and not the organisation's use.
    let carol = person(
        &l,
        &w,
        "carol",
        "project-administrator",
        json!({ "type": "project", "id": w.acme_prod }),
    )
    .await;
    let (s, mine) = l.req("GET", "/api/v1/identity/tenancy", &carol, None).await;
    assert_eq!(s, StatusCode::OK, "{mine}");
    assert_eq!(
        mine["organizations"].as_array().map(Vec::len),
        Some(1),
        "{mine}"
    );
    let org = &mine["organizations"][0];
    assert_eq!(org["slug"], "acme");
    assert!(org["usage"].is_null(), "{org}");
    assert_eq!(org["canSetQuota"], false);
    assert_eq!(org["projects"][0]["canSetQuota"], false);
    assert_eq!(org["projects"][0]["environments"][0]["canSetQuota"], true);
    // Someone with no tenancy permission is refused.
    let eve = l.user("sub-eve", "eve").await;
    let _ = eve;
    assert_eq!(
        l.req(
            "GET",
            "/api/v1/identity/tenancy",
            &l.token("sub-eve", "eve"),
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn legacy_tokens_follow_their_roles(pool: Pool) {
    let l = lab(pool.clone());
    let w = world(&l).await;
    let (admin, _) = apelora_persistence::create_token(&pool, "infra", "fabric-admin", None)
        .await
        .expect("token");
    let (operator, _) = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token");
    let path = format!("/api/v1/identity/quotas/organization/{}", w.acme);
    assert_eq!(
        l.req("PUT", &path, &admin, Some(json!({ "cpu": 8 })))
            .await
            .0,
        StatusCode::OK
    );
    assert_eq!(
        l.req("PUT", &path, &operator, Some(json!({ "cpu": 8 })))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(l.req("GET", &path, &operator, None).await.0, StatusCode::OK);
}
