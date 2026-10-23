// The Servers area's read-only routes through the real router: mock provider (MOCK VERIFIED)
// and a real PostgreSQL for permissions.

#![allow(clippy::unwrap_used)] // test fixtures: a failed unwrap is the failure

use apelora_incus_adapter::MockIncus;
use serde_json::json;

use super::identity_tests::{code, lab, world};
use super::tests::send;
use super::*;

const PATHS: [&str; 3] = [
    "/api/v1/cluster/members",
    "/api/v1/cluster/groups",
    "/api/v1/cluster/settings",
];

fn dev(mock: &MockIncus, admin: bool) -> AppState {
    AppState::new(Provider::Mock(mock.clone()), Auth::DisabledForDevelopment)
        .with_admin(admin.then(|| Provider::Mock(mock.clone())))
}

async fn get(state: &AppState, path: &str) -> (StatusCode, Value) {
    send(
        state.clone(),
        "GET",
        path,
        "unused-in-development",
        None,
        None,
    )
    .await
}

#[tokio::test]
async fn members_show_roles_quorum_and_versions() {
    let mock = MockIncus::default();
    let state = dev(&mock, false);
    let (status, r) = get(&state, "/api/v1/cluster/members").await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["source"]["provider"], "mock");
    assert_eq!(r["sites"]["planned"], false);
    assert_eq!(r["sites"]["connectedSites"], 1);
    assert_eq!(r["sites"]["members"], 3);
    let place = apelora_metrics::Place::from_env(|key| std::env::var(key).ok()).expect("place");
    assert_eq!(r["sites"]["region"], place.region);
    assert_eq!(r["sites"]["site"], place.site);
    assert_eq!(r["sites"]["sector"], place.sector);
    assert!(
        r["sites"]["survival"]
            .as_str()
            .is_some_and(|s| s.contains("No other site is connected")),
        "{r}"
    );
    assert_eq!(r["clustered"], true);
    assert_eq!(r["members"][0]["name"], "mock-member-01");
    assert_eq!(r["members"][0]["databaseRole"], "voter");
    assert_eq!(r["members"][0]["leader"], true);
    assert_eq!(r["members"][1]["leader"], false);
    assert_eq!(r["quorum"]["state"], "healthy");
    assert_eq!(r["quorum"]["leader"], "mock-member-01");
    assert_eq!(
        (
            &r["quorum"]["voters"],
            &r["quorum"]["needed"],
            &r["quorum"]["canLose"]
        ),
        (&json!(3), &json!(2), &json!(1))
    );
    // The mock does not know its release: never shown as matching the pin.
    assert_eq!(r["members"][0]["incusVersion"], Value::Null);
    assert_eq!(r["versions"]["state"], "unknown");
    assert_eq!(r["versions"]["ready"], false);

    for m in ["mock-member-01", "mock-member-02", "mock-member-03"] {
        mock.member_versions
            .lock()
            .expect("lock")
            .insert(m.into(), "7.0.1".into());
    }
    let (_, r) = get(&state, "/api/v1/cluster/members").await;
    assert_eq!(r["members"][2]["incusVersion"], "7.0.1");
    assert_eq!(
        r["versions"]["pinned"],
        admin::pinned_incus().trim_start_matches('v')
    );
    assert_eq!(r["versions"]["state"], "behind-pin");
    assert_eq!(r["versions"]["ready"], true);

    // One voter offline: one more loss stops changes, and the upgrade waits.
    mock.members.lock().expect("lock")[2].status = "Offline".into();
    let (_, r) = get(&state, "/api/v1/cluster/members").await;
    assert_eq!(r["quorum"]["state"], "at-risk");
    assert_eq!(r["quorum"]["votersUp"], 2);
    assert_eq!(r["versions"]["ready"], false);
    assert!(
        r["versions"]["blockers"][0]
            .as_str()
            .is_some_and(|b| b.starts_with("offline: mock-member-03")),
        "{r}"
    );
}

#[tokio::test]
async fn groups_list_their_members() {
    let (status, g) = get(&dev(&MockIncus::default(), false), "/api/v1/cluster/groups").await;
    assert_eq!(status, StatusCode::OK, "{g}");
    assert_eq!(g["items"][0]["name"], "default");
    assert_eq!(g["items"][0]["members"].as_array().map(Vec::len), Some(3));
}

#[tokio::test]
async fn settings_need_the_admin_identity_and_return_only_known_keys() {
    let mock = MockIncus::default();
    let (status, body) = get(&dev(&mock, false), "/api/v1/cluster/settings").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE, "{body}");
    assert_eq!(code(&body), Some("cluster_admin_not_configured"));

    mock.config
        .lock()
        .expect("lock")
        .insert("core.trust_password".into(), "never-shown".into());
    let (status, s) = get(&dev(&mock, true), "/api/v1/cluster/settings").await;
    assert_eq!(status, StatusCode::OK, "{s}");
    assert_eq!(s["identity"], "admin");
    let items = s["items"].as_array().expect("items");
    assert_eq!(
        items.len(),
        apelora_domain::clustering::CLUSTER_SETTINGS.len()
    );
    let find = |k: &str| items.iter().find(|i| i["key"] == k).expect(k).clone();
    let expiry = find("cluster.join_token_expiry");
    assert_eq!(
        (&expiry["value"], &expiry["usesDefault"]),
        (&json!("1H"), &json!(false))
    );
    let offline = find("cluster.offline_threshold");
    assert_eq!(
        (
            &offline["value"],
            &offline["default"],
            &offline["usesDefault"]
        ),
        (&Value::Null, &json!("20"), &json!(true))
    );
    assert!(!s.to_string().contains("never-shown"), "{s}");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn servers_routes_need_cluster_view(pool: Pool) {
    let l = lab(pool.clone());
    let w = world(&l).await;

    // Without a token: refused before anything is read.
    for path in PATHS {
        let (status, _) = l.req("GET", path, "", None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{path}");
    }

    // A designer in one project does not see the cluster.
    let des = l.user("sub-des", "des").await;
    l.grant(
        &w.admin,
        des,
        "application-designer",
        json!({ "type": "project", "id": w.acme_prod }),
    )
    .await;
    let td = l.token("sub-des", "des");
    for path in PATHS {
        let (status, body) = l.req("GET", path, &td, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}: {body}");
        assert_eq!(code(&body), Some("forbidden"));
    }

    // An organisation owner (tenant permissions only) does not either.
    let owner = l.user("sub-own", "owner").await;
    l.grant(
        &w.admin,
        owner,
        "organization-owner",
        json!({ "type": "organization", "id": w.other }),
    )
    .await;
    let to = l.token("sub-own", "owner");
    for path in PATHS {
        assert_eq!(
            l.req("GET", path, &to, None).await.0,
            StatusCode::FORBIDDEN,
            "{path}"
        );
    }

    // A platform viewer holds cluster.view and reads all three (the lab has an admin identity).
    let viewer = l.user("sub-view", "viewer").await;
    l.grant(&w.admin, viewer, "viewer", json!({ "type": "platform" }))
        .await;
    let tv = l.token("sub-view", "viewer");
    for path in PATHS {
        let (status, body) = l.req("GET", path, &tv, None).await;
        assert_eq!(status, StatusCode::OK, "{path}: {body}");
    }

    // A legacy auditor token reads inventory, so it reads these too.
    let (auditor, _) = apelora_persistence::create_token(&pool, "audit", "auditor", None)
        .await
        .expect("token");
    for path in PATHS {
        assert_eq!(
            l.req("GET", path, &auditor, None).await.0,
            StatusCode::OK,
            "{path}"
        );
    }
    let _ = w.other_proj;
}

// ---- changing settings: permission, dry run, approval, audit ----------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn cluster_settings_change_only_with_the_permission_and_another_persons_approval(pool: Pool) {
    use super::sessions_tests::call;
    let l = lab(pool.clone());
    let w = world(&l).await;
    let two = l.super_admin("sub-two", "second").await.1;
    let patch = |token: &str, body: Value, headers: &[(&str, &str)]| {
        let (state, token) = (l.state.clone(), token.to_owned());
        let headers: Vec<(String, String)> = headers
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect();
        async move {
            let h: Vec<(&str, &str)> = headers
                .iter()
                .map(|(k, v)| (k.as_str(), v.as_str()))
                .collect();
            call(
                state,
                "PATCH",
                "/api/v1/cluster/settings",
                &token,
                Some(body),
                &h,
            )
            .await
        }
    };

    // People without cluster.sensitive_change are refused, whatever they hold elsewhere.
    let viewer = l.user("sub-view", "viewer").await;
    l.grant(&w.admin, viewer, "viewer", json!({ "type": "platform" }))
        .await;
    let designer = l.user("sub-des", "des").await;
    l.grant(
        &w.admin,
        designer,
        "application-designer",
        json!({ "type": "project", "id": w.acme_prod }),
    )
    .await;
    for token in [l.token("sub-view", "viewer"), l.token("sub-des", "des")] {
        let (s, b) = patch(
            &token,
            json!({ "changes": { "cluster.max_standby": "1" } }),
            &[],
        )
        .await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{b}");
    }

    // A dry run reports what would change and what to watch for, and changes nothing.
    let (s, r) = patch(&w.admin, json!({ "changes": { "cluster.healing_threshold": "300", "cluster.max_standby": "1" }, "dryRun": true }), &[]).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["applied"], json!(false));
    assert_eq!(
        r["approvalTarget"],
        json!("cluster-settings:cluster.healing_threshold/cluster.max_standby")
    );
    let healing = r["changes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["key"] == "cluster.healing_threshold")
        .unwrap();
    assert_eq!(
        (&healing["from"], &healing["to"]),
        (&Value::Null, &json!("300"))
    );
    assert!(
        healing["warnings"][0]
            .as_str()
            .unwrap()
            .contains("two recovery authorities"),
        "{healing}"
    );
    let (_, now) = l
        .req("GET", "/api/v1/cluster/settings", &w.admin, None)
        .await;
    let standby = now["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["key"] == "cluster.max_standby")
        .unwrap();
    assert!(
        standby["usesDefault"].as_bool().unwrap(),
        "a dry run changes nothing: {standby}"
    );

    // Applying needs an approval of exactly these settings.
    let body = json!({ "changes": { "cluster.max_standby": "1" } });
    let (s, b) = patch(&w.admin, body.clone(), &[]).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("approval_required")),
        "{b}"
    );
    let target = "cluster-settings:cluster.max_standby";
    let (s, a) = l.req("POST", "/api/v1/approvals", &w.admin, Some(json!({ "action": "cluster.sensitive_change", "target": target, "reason": "more database copies" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    let id = a["id"].as_str().unwrap().to_owned();
    let (s, _) = l
        .req(
            "POST",
            &format!("/api/v1/approvals/{id}/approve"),
            &w.admin,
            None,
        )
        .await;
    assert_ne!(s, StatusCode::OK, "nobody approves their own request");
    let (s, _) = l
        .req(
            "POST",
            &format!("/api/v1/approvals/{id}/approve"),
            &two,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);
    // Another setting is not covered by it.
    let (s, b) = patch(
        &w.admin,
        json!({ "changes": { "cluster.max_voters": "5" } }),
        &[("apelora-approval", &id)],
    )
    .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("approval_invalid")),
        "{b}"
    );
    // The right one, once.
    let (s, r) = patch(&w.admin, body.clone(), &[("apelora-approval", &id)]).await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(r["applied"], json!(true));
    assert_eq!(r["changes"][0]["to"], json!("1"));
    let (s, b) = patch(
        &w.admin,
        json!({ "changes": { "cluster.max_standby": "2" } }),
        &[("apelora-approval", &id)],
    )
    .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("approval_invalid")),
        "spent: {b}"
    );

    // The change is visible, and audited with the old and new values.
    let (_, now) = l
        .req("GET", "/api/v1/cluster/settings", &w.admin, None)
        .await;
    let standby = now["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["key"] == "cluster.max_standby")
        .unwrap();
    assert_eq!(standby["value"], json!("1"));
    let detail: Value = sqlx::query_scalar(
        "SELECT detail FROM audit_events WHERE action = 'cluster.settings.change'",
    )
    .fetch_one(&pool)
    .await
    .expect("audit");
    assert_eq!(
        detail["changes"],
        json!([{ "key": "cluster.max_standby", "from": null, "to": "1" }])
    );
    assert_eq!(detail["approval"], json!(id));

    // Removing a setting makes Incus's default apply again (needs its own approval).
    let (_, a2) = l.req("POST", "/api/v1/approvals", &w.admin, Some(json!({ "action": "cluster.sensitive_change", "target": target, "reason": "back to default" }))).await;
    let id2 = a2["id"].as_str().unwrap().to_owned();
    l.req(
        "POST",
        &format!("/api/v1/approvals/{id2}/approve"),
        &two,
        None,
    )
    .await;
    let (s, r) = patch(
        &w.admin,
        json!({ "changes": { "cluster.max_standby": null } }),
        &[("apelora-approval", &id2)],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{r}");
    assert_eq!(
        (&r["changes"][0]["from"], &r["changes"][0]["to"]),
        (&json!("1"), &Value::Null)
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn cluster_setting_changes_refuse_unknown_keys_bad_values_and_no_ops(pool: Pool) {
    use super::sessions_tests::call;
    let l = lab(pool);
    let w = world(&l).await;
    let patch = |body: Value| {
        let (state, token) = (l.state.clone(), w.admin.clone());
        async move {
            call(
                state,
                "PATCH",
                "/api/v1/cluster/settings",
                &token,
                Some(body),
                &[],
            )
            .await
        }
    };
    for (body, why) in [
        (
            json!({ "changes": { "core.https_address": ":9999" } }),
            "a setting Apelora does not change",
        ),
        (
            json!({ "changes": { "cluster.max_voters": "4" } }),
            "an even number of voters",
        ),
        (
            json!({ "changes": { "cluster.offline_threshold": "soon" } }),
            "not a number",
        ),
        (json!({ "changes": {} }), "nothing named"),
        (json!({}), "no body fields"),
    ] {
        let (s, b) = patch(body).await;
        assert!(
            s == StatusCode::BAD_REQUEST || s == StatusCode::UNPROCESSABLE_ENTITY,
            "{why}: {s} {b}"
        );
    }
    let many: std::collections::BTreeMap<String, String> = (0..10)
        .map(|i| (format!("cluster.k{i}"), "1".into()))
        .collect();
    assert_eq!(
        patch(json!({ "changes": many })).await.0,
        StatusCode::BAD_REQUEST
    );
    // The mock already holds join_token_expiry = 1H: asking for it again changes nothing.
    let (s, b) = patch(json!({ "changes": { "cluster.join_token_expiry": "1H" } })).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("nothing_to_change")),
        "{b}"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn an_upgrade_walks_evacuate_package_and_restore(pool: Pool) {
    use apelora_incus_adapter::Provider;
    let l = lab(pool.clone());
    let (token, _) = apelora_persistence::create_token(&pool, "ops", "fabric-admin", None)
        .await
        .expect("token");
    let Provider::Mock(mock) = &*l.state.provider else {
        unreachable!("lab")
    };
    for m in ["mock-member-01", "mock-member-02", "mock-member-03"] {
        mock.member_versions
            .lock()
            .expect("lock")
            .insert(m.into(), "7.0.1".into());
    }
    let (st, r) = l
        .req(
            "POST",
            "/api/v1/cluster/upgrade",
            &token,
            Some(json!({ "action": "start", "confirm": "upgrade" })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(r["next"], "evacuate");
    let member = r["member"].as_str().expect("member").to_owned();
    assert_ne!(member, "mock-member-01", "the leader is last");
    let (st, r) = l
        .req(
            "POST",
            "/api/v1/cluster/upgrade",
            &token,
            Some(json!({ "action": "evacuate", "member": member, "confirm": member })),
        )
        .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{r}");
    assert_eq!(r["next"], "upgraded");
    let (st, r) = l
        .req(
            "POST",
            "/api/v1/cluster/upgrade",
            &token,
            Some(json!({ "action": "upgraded", "member": member, "confirm": member })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{r}");
    assert_eq!(r["next"], "restore");
    let (st, r) = l
        .req(
            "POST",
            "/api/v1/cluster/upgrade",
            &token,
            Some(json!({ "action": "restore", "member": member, "confirm": member })),
        )
        .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{r}");
    let rows = apelora_persistence::recent_audit(&pool, 20)
        .await
        .expect("audit");
    for action in [
        "cluster.upgrade.start",
        "cluster.upgrade.evacuate",
        "cluster.upgrade.upgraded",
        "cluster.upgrade.restore",
    ] {
        assert!(rows.iter().any(|e| e.action == action), "missing {action}");
    }
}
