use super::*;
use apelora_incus_adapter::{IncusClient, MockIncus};
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use tower::ServiceExt;

async fn call(
    state: AppState,
    path: &str,
    token: Option<&str>,
) -> (StatusCode, Value, Option<String>) {
    let mut req = Request::get(path);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let res = router(state)
        .oneshot(req.body(Body::empty()).expect("request"))
        .await
        .expect("response");
    let status = res.status();
    let www = res
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .map(|v| v.to_str().unwrap_or_default().to_owned());
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (status, serde_json::from_slice(&bytes).expect("json"), www)
}

fn dev(provider: Provider) -> AppState {
    AppState::new(provider, Auth::DisabledForDevelopment)
}

#[tokio::test]
async fn health_reports_mock_provider_and_disabled_auth_in_development() {
    let (status, body, _) = call(
        dev(Provider::Mock(MockIncus::default())),
        "/api/v1/health",
        None,
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(body["provider"], "mock");
    assert_eq!(body["auth"], "disabled-development");
    assert_eq!(body["incus"]["connected"], false);
}

#[tokio::test]
async fn inventory_endpoints_label_their_source() {
    for path in [
        "/api/v1/cluster",
        "/api/v1/instances",
        "/api/v1/projects",
        "/api/v1/networks",
        "/api/v1/storage-pools",
    ] {
        let (status, body, _) = call(dev(Provider::Mock(MockIncus::default())), path, None).await;
        assert_eq!(status, 200, "{path}");
        assert_eq!(body["source"]["provider"], "mock", "{path}");
        assert!(
            body["source"]["observedAt"]
                .as_str()
                .is_some_and(|t| t.ends_with('Z')),
            "{path}: {body}"
        );
    }
    let (_, cluster, _) = call(
        dev(Provider::Mock(MockIncus::default())),
        "/api/v1/cluster",
        None,
    )
    .await;
    assert_eq!(cluster["members"].as_array().map(Vec::len), Some(3));
    let (_, inst, _) = call(
        dev(Provider::Mock(MockIncus::default())),
        "/api/v1/instances",
        None,
    )
    .await;
    assert_eq!(inst["items"][3]["kind"], "virtual-machine");
}

#[tokio::test]
async fn unreachable_incus_is_a_502_with_a_stable_code() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    drop(listener);
    let provider = Provider::Incus(IncusClient::plain_for_tests(&format!("http://{addr}")));
    let (status, body, _) = call(dev(provider.clone()), "/api/v1/instances", None).await;
    assert_eq!(status, 502);
    assert_eq!(body["error"]["code"], "incus_unreachable");
    let (status, body, _) = call(dev(provider), "/api/v1/health", None).await;
    assert_eq!(status, 200);
    assert_eq!(body["incus"]["connected"], false);
}

#[tokio::test]
async fn audit_needs_a_database_in_development_mode() {
    let (status, body, _) = call(
        dev(Provider::Mock(MockIncus::default())),
        "/api/v1/audit",
        None,
    )
    .await;
    assert_eq!(status, 503);
    assert_eq!(body["error"]["code"], "store_unavailable");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn tokens_are_required_and_roles_are_enforced(pool: Pool) {
    let state = AppState::new(
        Provider::Mock(MockIncus::default()),
        Auth::Tokens(pool.clone()),
    );
    let (status, body, www) = call(state.clone(), "/api/v1/instances", None).await;
    assert_eq!(
        (status, body["error"]["code"].as_str(), www.as_deref()),
        (
            StatusCode::UNAUTHORIZED,
            Some("unauthenticated"),
            Some("Bearer")
        )
    );
    let (status, _, _) = call(state.clone(), "/api/v1/instances", Some("apl_0000")).await;
    assert_eq!(status, 401);
    let (status, body, _) = call(state.clone(), "/api/v1/health", None).await;
    assert_eq!(
        (status, body["auth"].as_str()),
        (StatusCode::OK, Some("tokens")),
        "health stays public"
    );

    let (auditor, _) = apelora_persistence::create_token(&pool, "ci-auditor", "auditor", None)
        .await
        .expect("token");
    let (designer, designer_rec) =
        apelora_persistence::create_token(&pool, "alice", "studio-designer", None)
            .await
            .expect("token");
    apelora_persistence::audit(&pool, "cli", "token.create", "alice", &json!({}))
        .await
        .expect("audit");

    let (status, body, _) = call(state.clone(), "/api/v1/whoami", Some(&auditor)).await;
    assert_eq!(
        (status, body["role"].as_str()),
        (StatusCode::OK, Some("auditor"))
    );
    assert_eq!(
        call(state.clone(), "/api/v1/cluster", Some(&auditor))
            .await
            .0,
        200
    );
    let (status, body, _) = call(state.clone(), "/api/v1/audit", Some(&auditor)).await;
    assert_eq!(
        (status, body["items"][0]["action"].as_str()),
        (StatusCode::OK, Some("token.create"))
    );

    let (status, body, _) = call(state.clone(), "/api/v1/audit", Some(&designer)).await;
    assert_eq!(
        (status, body["error"]["code"].as_str()),
        (StatusCode::FORBIDDEN, Some("forbidden"))
    );
    assert_eq!(
        call(state.clone(), "/api/v1/instances", Some(&designer))
            .await
            .0,
        200
    );

    apelora_persistence::revoke_token(&pool, designer_rec.id)
        .await
        .expect("revoke");
    assert_eq!(
        call(state, "/api/v1/instances", Some(&designer)).await.0,
        401
    );
}
