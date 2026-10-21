// The Servers area's read-only routes through the real router: mock provider (MOCK VERIFIED)
// and a real PostgreSQL for permissions.

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
