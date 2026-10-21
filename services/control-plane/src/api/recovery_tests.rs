// Failed members (HA slice B): the recovery report and a person's confirmation, through the
// real router with the mock provider and a real PostgreSQL (MOCK VERIFIED).

use serde_json::json;

use super::identity_tests::{code, lab, world};
use super::*;

fn set_member(state: &AppState, name: &str, status: &str) {
    let Provider::Mock(m) = &*state.provider else {
        unreachable!("the lab uses the mock")
    };
    let mut members = m.members.lock().expect("lock");
    members
        .iter_mut()
        .find(|x| x.name == name)
        .expect("member")
        .status = status.into();
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn failed_members_are_reported_and_confirmation_is_guarded(pool: Pool) {
    let l = lab(pool.clone());
    let (admin_token, _) = apelora_persistence::create_token(&pool, "ops", "fabric-admin", None)
        .await
        .expect("token");
    let (status, r) = l
        .req("GET", "/api/v1/cluster/recovery", &admin_token, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["items"], json!([]), "no failed member");

    set_member(&l.state, "mock-member-02", "Offline");
    let (_, r) = l
        .req("GET", "/api/v1/cluster/recovery", &admin_token, None)
        .await;
    let m = &r["items"][0];
    assert_eq!(m["member"], "mock-member-02");
    assert_eq!(m["fencing"]["condition"], "waiting");
    assert!(
        m["fencing"]["reason"]
            .as_str()
            .expect("reason")
            .contains("not looked")
    );
    // The mock's seed instances there were not deployed by Apelora: they wait.
    let items = m["items"].as_array().expect("items");
    assert!(!items.is_empty());
    assert!(items.iter().all(|i| i["action"] == "wait"), "{items:?}");

    let confirm = |member: &'static str, typed: &'static str, off: bool| {
        let t = admin_token.clone();
        let l = &l;
        async move {
            l.req(
                "POST",
                &format!("/api/v1/cluster/members/{member}/fencing"),
                &t,
                Some(json!({ "confirm": typed, "poweredOff": off })),
            )
            .await
        }
    };
    let (s, b) = confirm("mock-member-02", "mock-member-03", true).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::BAD_REQUEST, Some("confirmation_mismatch"))
    );
    let (s, _) = confirm("mock-member-02", "mock-member-02", false).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, b) = confirm("mock-member-03", "mock-member-03", true).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("member_not_offline")),
        "an online server is never fenced"
    );
    let (s, b) = confirm("mock-member-02", "mock-member-02", true).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    assert_eq!(b["confirmedBy"], "token:ops");
    let open = apelora_persistence::recovery::open_failures(&pool)
        .await
        .expect("open");
    assert_eq!(open[0].confirmed_by.as_deref(), Some("token:ops"));
    let audit = apelora_persistence::recent_audit(&pool, 10)
        .await
        .expect("audit");
    assert!(
        audit
            .iter()
            .any(|e| e.action == "cluster.member.fence.confirmed" && e.target == "mock-member-02")
    );

    // A person: a viewer reads but cannot confirm; a fabric administrator needs an approval.
    let w = world(&l).await;
    let viewer = l.user("sub-view", "viewer").await;
    l.grant(&w.admin, viewer, "viewer", json!({ "type": "platform" }))
        .await;
    let tv = l.token("sub-view", "viewer");
    assert_eq!(
        l.req("GET", "/api/v1/cluster/recovery", &tv, None).await.0,
        StatusCode::OK
    );
    let body = Some(json!({ "confirm": "mock-member-02", "poweredOff": true }));
    let (s, b) = l
        .req(
            "POST",
            "/api/v1/cluster/members/mock-member-02/fencing",
            &tv,
            body.clone(),
        )
        .await;
    assert_eq!((s, code(&b)), (StatusCode::FORBIDDEN, Some("forbidden")));
    let fa = l.user("sub-fa", "fa").await;
    l.grant(
        &w.admin,
        fa,
        "fabric-administrator",
        json!({ "type": "platform" }),
    )
    .await;
    let tf = l.token_mfa("sub-fa", "fa");
    let (s, b) = l
        .req(
            "POST",
            "/api/v1/cluster/members/mock-member-02/fencing",
            &tf,
            body,
        )
        .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("approval_required")),
        "{b}"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn data_replicated_volumes_and_the_root_disk_go_to_the_shared_pool(pool: Pool) {
    use super::tests::{drain, module, send};
    let mut mock = apelora_incus_adapter::MockIncus::default();
    for (name, driver) in [("shared", "lvmcluster"), ("plain", "lvm")] {
        mock.pools.push(apelora_domain::StoragePool {
            name: name.into(),
            driver: driver.into(),
            status: "Created".into(),
            description: "mock data".into(),
            locations: Vec::new(),
        });
    }
    let provider = Provider::Mock(mock);
    let operator = apelora_persistence::create_token(&pool, "op", "operator", None)
        .await
        .expect("token")
        .0;
    let design = |classes: &[&str]| {
        let mut db = module("db-01");
        db["availability"] = json!("stateful");
        db["spec"]["volumes"] = json!(classes
            .iter()
            .enumerate()
            .map(|(i, c)| json!({ "name": format!("v{i}"), "sizeMib": 256, "path": format!("/v{i}"), "class": c }))
            .collect::<Vec<_>>());
        json!({ "project": "apelora", "document": {}, "modules": [db] })
    };
    let state = |shared: Option<&str>| {
        AppState::new(provider.clone(), Auth::Tokens(pool.clone()))
            .with_volume_pool(Some("mock-local".into()))
            .with_shared_pool(shared.map(str::to_owned))
    };
    let plan = |st: AppState, body: Value| {
        let operator = operator.clone();
        async move {
            send(
                st,
                "POST",
                "/api/v1/applications/shop/plan",
                &operator,
                Some(body),
                None,
            )
            .await
        }
    };
    let message = |b: &Value| {
        b["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .to_owned()
    };

    let (s, b) = plan(state(None), design(&["data-replicated"])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(message(&b).contains("APELORA_SHARED_POOL"), "{b}");
    let (s, b) = plan(state(Some("plain")), design(&["data-replicated"])).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(message(&b).contains("not shared"), "{b}");
    let (s, b) = plan(
        state(Some("shared")),
        design(&["data-replicated", "data-local"]),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(message(&b).contains("cannot be mixed"), "{b}");
    let mut caller_root = design(&["data-replicated"]);
    caller_root["modules"][0]["spec"]["rootPool"] = json!("shared");
    let (s, b) = plan(state(Some("shared")), caller_root).await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "a caller never chooses the root pool: {b}"
    );

    let (s, b) = send(
        state(Some("shared")),
        "POST",
        "/api/v1/applications/shop/deployments",
        &operator,
        Some(design(&["data-replicated"])),
        Some("shared-1"),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{b}");
    drain(&pool, &provider).await;
    let record = apelora_persistence::list_deployments(&pool)
        .await
        .expect("records")
        .remove(0);
    assert_eq!(
        record.desired[0]["spec"]["rootPool"], "shared",
        "{}",
        record.desired
    );
    assert_eq!(record.desired[0]["spec"]["volumes"][0]["pool"], "shared");
    let Provider::Mock(m) = &provider else {
        unreachable!()
    };
    let (root, status) = {
        let all = m.instances.lock().expect("lock");
        let db = all
            .iter()
            .find(|i| i.instance.name == "shop-db-01")
            .expect("created");
        (db.root_pool.clone(), db.instance.status.clone())
    };
    assert_eq!(root.as_deref(), Some("shared"));
    assert_eq!(status, "Running");
    let v = apelora_persistence::application_volumes(&pool, "shop")
        .await
        .expect("volumes");
    assert_eq!(
        (
            v[0].storage_pool.as_str(),
            v[0].member.as_str(),
            v[0].class.as_str()
        ),
        ("shared", "", "data-replicated"),
        "a volume on the shared pool belongs to no member"
    );
}
