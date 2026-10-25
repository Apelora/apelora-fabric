// Local accounts and Apelora sessions through the real router, against a real PostgreSQL
// (ADR 0007). Every rule in docs/design/sign-in-methods.md has a test here.

use std::sync::atomic::Ordering;

use apelora_auth::{Builtin, Scope};
use apelora_incus_adapter::MockIncus;
use axum::body::Body;
use axum::http::{HeaderMap, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::identity_tests::{code, lab, new_assignment};
use super::*;

const PASSWORD: &str = "correct horse battery";

/// A request with its response headers.
pub(super) async fn call(
    state: &AppState,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value, HeaderMap) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .expect("request");
    let res = router(state.clone()).oneshot(req).await.expect("response");
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
        headers,
    )
}

/// A control plane with an identity provider (from the environment) and API tokens on; the
/// address limit is raised so tests can make many attempts (one test checks the real limit).
pub(super) async fn setup_lab(pool: Pool) -> (super::identity_tests::Lab, String) {
    let mut l = lab(pool);
    l.state.sign_in_attempts = std::sync::Arc::new(sessions_auth::Attempts::new(
        1000,
        std::time::Duration::from_secs(60),
    ));
    let admin = apelora_persistence::create_token(&l.pool, "admin", "fabric-admin", None)
        .await
        .expect("token")
        .0;
    let (status, s, _) = call(
        &l.state,
        "PUT",
        "/api/v1/identity/sign-in/local-accounts",
        Some(&admin),
        Some(json!({ "enabled": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{s}");
    assert_eq!(s["enabled"], true);
    (l, admin)
}

pub(super) async fn create(state: &AppState, admin: &str, username: &str) -> (Uuid, String) {
    let (status, c, headers) = call(
        state,
        "POST",
        "/api/v1/identity/local-accounts",
        Some(admin),
        Some(json!({ "username": username, "displayName": format!("{username} (test)") })),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{c}");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(c["user"]["issuer"], "apelora:local");
    let token = c["setupToken"].as_str().expect("token").to_owned();
    assert!(token.starts_with("apw_") && token.len() == 68);
    (
        c["user"]["id"].as_str().expect("id").parse().expect("uuid"),
        token,
    )
}

pub(super) async fn set_up(state: &AppState, token: &str, password: &str) -> (StatusCode, Value) {
    let (s, v, _) = call(
        state,
        "POST",
        "/api/v1/sign-in/setup",
        None,
        Some(json!({ "token": token, "password": password })),
    )
    .await;
    (s, v)
}

async fn sign_in(state: &AppState, username: &str, password: &str) -> (StatusCode, Value) {
    let (s, v, _) = call(
        state,
        "POST",
        "/api/v1/sign-in/password",
        None,
        Some(json!({ "username": username, "password": password })),
    )
    .await;
    (s, v)
}

pub(super) async fn session(state: &AppState, username: &str, password: &str) -> String {
    let (status, s) = sign_in(state, username, password).await;
    assert_eq!(status, StatusCode::OK, "{s}");
    s["token"].as_str().expect("token").to_owned()
}

async fn whoami(state: &AppState, token: &str) -> (StatusCode, Value) {
    let (s, v, _) = call(state, "GET", "/api/v1/whoami", Some(token), None).await;
    (s, v)
}

async fn audit_count(pool: &Pool, action: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = $1")
        .bind(action)
        .fetch_one(pool)
        .await
        .expect("audit")
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn an_account_is_created_set_up_signed_in_and_signed_out(pool: Pool) {
    let (l, admin) = setup_lab(pool).await;
    let (status, opts, _) = call(&l.state, "GET", "/api/v1/sign-in", None, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(
        (&opts["localAccounts"], &opts["directories"]),
        (&json!(true), &json!([]))
    );

    let (id, link) = create(&l.state, &admin, " Alice.B ").await;
    // Before setup, no password can match.
    let (status, e) = sign_in(&l.state, "alice.b", PASSWORD).await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::UNAUTHORIZED, Some("invalid_credentials"))
    );

    // The policy: 12 to 128 characters, not the username.
    let (status, e) = set_up(&l.state, &link, "too short").await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::BAD_REQUEST, Some("weak_password")),
        "{e}"
    );
    assert!(
        e["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("12 to 128")
    );
    let (status, e) = set_up(&l.state, &link, "ALICE.B").await;
    assert_eq!(code(&e), Some("weak_password"), "{status}");
    let (status, _) = set_up(&l.state, &link, PASSWORD).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    // The link works once.
    let (status, e) = set_up(&l.state, &link, "another long passphrase").await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::BAD_REQUEST, Some("setup_invalid"))
    );

    // Usernames are matched in lowercase; the answer names the person.
    let (status, s, headers) = call(
        &l.state,
        "POST",
        "/api/v1/sign-in/password",
        None,
        Some(json!({ "username": "ALICE.B", "password": PASSWORD })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{s}");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(s["name"], "Alice.B  (test)");
    let token = s["token"].as_str().expect("token").to_owned();
    assert!(token.starts_with("apu_") && token.len() == 68);
    let (status, me) = whoami(&l.state, &token).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(
        (&me["kind"], &me["id"], &me["localAccount"], &me["name"]),
        (&json!("user"), &json!(id), &json!(true), &json!("alice.b"))
    );
    // Only the hash is stored; the session lasts eight hours at most.
    let (stored, hours): (Vec<u8>, f64) = sqlx::query_as(
        "SELECT token_hash, (extract(epoch FROM expires_at - created_at) / 3600)::float8 FROM user_sessions",
    )
    .fetch_one(&l.pool)
    .await
    .expect("session");
    assert_ne!(stored, token.as_bytes());
    assert!((hours - 8.0).abs() < 0.01, "{hours}");

    // Signing out ends the session at once; other credentials get 204 and nothing happens.
    let (status, _, _) = call(&l.state, "POST", "/api/v1/sign-out", Some(&token), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = whoami(&l.state, &token).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _, _) = call(&l.state, "POST", "/api/v1/sign-out", Some(&admin), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    let (status, _) = whoami(&l.state, &admin).await;
    assert_eq!(status, StatusCode::OK);

    for action in [
        "identity.local_accounts.enabled",
        "identity.local_account.created",
        "auth.setup.completed",
        "auth.password.succeeded",
        "auth.password.failed",
        "auth.signed_out",
    ] {
        assert!(
            audit_count(&l.pool, action).await >= 1,
            "{action} is audited"
        );
    }
    // No password, setup token or session token in any audit row or target.
    let rows: Vec<String> =
        sqlx::query_scalar("SELECT target || ' ' || detail::text FROM audit_events")
            .fetch_all(&l.pool)
            .await
            .expect("audit");
    for row in rows {
        for secret in [
            PASSWORD,
            link.as_str(),
            token.as_str(),
            &token[4..],
            &link[4..],
        ] {
            assert!(
                !row.contains(secret),
                "a secret reached the audit log: {row}"
            );
        }
    }
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn five_failures_lock_the_account_for_fifteen_minutes_and_success_resets_the_count(
    pool: Pool,
) {
    let (l, admin) = setup_lab(pool).await;
    let (id, link) = create(&l.state, &admin, "bob").await;
    assert_eq!(
        set_up(&l.state, &link, PASSWORD).await.0,
        StatusCode::NO_CONTENT
    );

    // Four failures, then a success: the count starts again.
    for _ in 0..4 {
        assert_eq!(
            sign_in(&l.state, "bob", "wrong password!").await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    session(&l.state, "bob", PASSWORD).await;
    let failed: i32 =
        sqlx::query_scalar("SELECT failed_attempts FROM local_credentials WHERE user_id = $1")
            .bind(id)
            .fetch_one(&l.pool)
            .await
            .expect("count");
    assert_eq!(failed, 0);

    for _ in 0..5 {
        assert_eq!(
            sign_in(&l.state, "bob", "wrong password!").await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(audit_count(&l.pool, "auth.locked").await, 1);
    let minutes: f64 = sqlx::query_scalar(
        "SELECT (extract(epoch FROM locked_until - now()) / 60)::float8 FROM local_credentials WHERE user_id = $1",
    )
    .bind(id)
    .fetch_one(&l.pool)
    .await
    .expect("lock");
    assert!((14.0..=15.0).contains(&minutes), "{minutes}");
    // Locked: even the right password is refused, with the same answer.
    let before = sessions_auth::DUMMY_VERIFICATIONS.load(Ordering::Relaxed);
    let (status, e) = sign_in(&l.state, "bob", PASSWORD).await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::UNAUTHORIZED, Some("invalid_credentials"))
    );
    assert_eq!(
        e["error"]["message"],
        "The username or password is incorrect."
    );
    assert!(
        sessions_auth::DUMMY_VERIFICATIONS.load(Ordering::Relaxed) > before,
        "a locked account still costs one verification"
    );
    let reason: String = sqlx::query_scalar(
        "SELECT detail->>'reason' FROM audit_events WHERE action = 'auth.password.failed' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&l.pool)
    .await
    .expect("audit");
    assert_eq!(reason, "locked");

    // When the lock has run out, the right password works again.
    sqlx::query("UPDATE local_credentials SET locked_until = now() - interval '1 second' WHERE user_id = $1")
        .bind(id)
        .execute(&l.pool)
        .await
        .expect("expire");
    session(&l.state, "bob", PASSWORD).await;
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn unknown_users_get_the_same_answer_after_a_dummy_verification(pool: Pool) {
    let (l, admin) = setup_lab(pool).await;
    let (_, link) = create(&l.state, &admin, "carol").await;
    assert_eq!(
        set_up(&l.state, &link, PASSWORD).await.0,
        StatusCode::NO_CONTENT
    );
    let (_, wrong) = sign_in(&l.state, "carol", "not the password").await;

    let before = sessions_auth::DUMMY_VERIFICATIONS.load(Ordering::Relaxed);
    let (status, unknown) = sign_in(&l.state, "nobody-here", "not the password").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        sessions_auth::DUMMY_VERIFICATIONS.load(Ordering::Relaxed) > before,
        "the unknown-user path verifies the dummy hash"
    );
    assert_eq!(unknown, wrong, "unknown and wrong answer the same");
    let (status, invalid) = sign_in(&l.state, "not a username!", "x").await;
    assert_eq!((status, &invalid), (StatusCode::UNAUTHORIZED, &wrong));
    let reasons: Vec<String> = sqlx::query_scalar(
        "SELECT detail->>'reason' FROM audit_events WHERE action = 'auth.password.failed' ORDER BY id",
    )
    .fetch_all(&l.pool)
    .await
    .expect("audit");
    assert_eq!(reasons, vec!["wrong", "unknown", "unknown"]);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn sessions_end_after_an_idle_hour_and_after_eight_hours(pool: Pool) {
    let (l, admin) = setup_lab(pool).await;
    let (_, link) = create(&l.state, &admin, "dave").await;
    assert_eq!(
        set_up(&l.state, &link, PASSWORD).await.0,
        StatusCode::NO_CONTENT
    );

    let idle = session(&l.state, "dave", PASSWORD).await;
    sqlx::query("UPDATE user_sessions SET last_used_at = now() - interval '59 minutes'")
        .execute(&l.pool)
        .await
        .expect("age");
    assert_eq!(
        whoami(&l.state, &idle).await.0,
        StatusCode::OK,
        "used within the hour"
    );
    sqlx::query("UPDATE user_sessions SET last_used_at = now() - interval '61 minutes'")
        .execute(&l.pool)
        .await
        .expect("age");
    let (status, e) = whoami(&l.state, &idle).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "{e}");

    let old = session(&l.state, "dave", PASSWORD).await;
    sqlx::query("UPDATE user_sessions SET expires_at = now() - interval '1 second' WHERE revoked_at IS NULL AND last_used_at > now() - interval '1 minute'")
        .execute(&l.pool)
        .await
        .expect("expire");
    assert_eq!(whoami(&l.state, &old).await.0, StatusCode::UNAUTHORIZED);
    // A made-up token is refused without a lookup surprise.
    assert_eq!(
        whoami(&l.state, &format!("apu_{}", "0".repeat(64))).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn setup_links_expire_after_a_day_and_a_reset_revokes_password_and_sessions(pool: Pool) {
    let (l, admin) = setup_lab(pool).await;
    let (id, link) = create(&l.state, &admin, "erin").await;
    let hours: f64 = sqlx::query_scalar(
        "SELECT (extract(epoch FROM setup_expires_at - now()) / 3600)::float8 FROM local_credentials WHERE user_id = $1",
    )
    .bind(id)
    .fetch_one(&l.pool)
    .await
    .expect("expiry");
    assert!((23.9..=24.0).contains(&hours), "{hours}");
    sqlx::query("UPDATE local_credentials SET setup_expires_at = now() - interval '1 second'")
        .execute(&l.pool)
        .await
        .expect("expire");
    let (status, e) = set_up(&l.state, &link, PASSWORD).await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::BAD_REQUEST, Some("setup_invalid"))
    );

    // A reset gives a new link (the old one stays dead) and ends every session.
    let path = format!("/api/v1/identity/local-accounts/{id}/reset");
    let (status, r, headers) = call(&l.state, "POST", &path, Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(headers[header::CACHE_CONTROL], "no-store");
    let second = r["setupToken"].as_str().expect("token").to_owned();
    assert_eq!(
        set_up(&l.state, &second, PASSWORD).await.0,
        StatusCode::NO_CONTENT
    );
    let s = session(&l.state, "erin", PASSWORD).await;
    let (status, r, _) = call(&l.state, "POST", &path, Some(&admin), None).await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(
        whoami(&l.state, &s).await.0,
        StatusCode::UNAUTHORIZED,
        "session revoked"
    );
    assert_eq!(
        sign_in(&l.state, "erin", PASSWORD).await.0,
        StatusCode::UNAUTHORIZED,
        "password gone"
    );
    assert_eq!(
        set_up(&l.state, &second, PASSWORD).await.0,
        StatusCode::BAD_REQUEST
    );
    let third = r["setupToken"].as_str().expect("token");
    assert_eq!(
        set_up(&l.state, third, "a brand new passphrase").await.0,
        StatusCode::NO_CONTENT
    );
    session(&l.state, "erin", "a brand new passphrase").await;
    assert_eq!(
        audit_count(&l.pool, "identity.local_account.reset").await,
        2
    );

    // Only local accounts can be reset.
    let person = l.user("someone", "someone").await;
    let (status, _, _) = call(
        &l.state,
        "POST",
        &format!("/api/v1/identity/local-accounts/{person}/reset"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn disabling_a_person_ends_their_sessions_and_refuses_their_password(pool: Pool) {
    let (l, admin) = setup_lab(pool).await;
    let (id, link) = create(&l.state, &admin, "frank").await;
    assert_eq!(
        set_up(&l.state, &link, PASSWORD).await.0,
        StatusCode::NO_CONTENT
    );
    let s = session(&l.state, "frank", PASSWORD).await;
    let (status, _, _) = call(
        &l.state,
        "POST",
        &format!("/api/v1/identity/users/{id}/disable"),
        Some(&admin),
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert_eq!(whoami(&l.state, &s).await.0, StatusCode::UNAUTHORIZED);
    let revoked: Option<String> = sqlx::query_scalar(
        "SELECT detail->>'revokedSessions' FROM audit_events WHERE action = 'user.disable'",
    )
    .fetch_one(&l.pool)
    .await
    .expect("audit");
    assert_eq!(revoked.as_deref(), Some("1"));
    let (status, e) = sign_in(&l.state, "frank", PASSWORD).await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::UNAUTHORIZED, Some("invalid_credentials"))
    );
    let reason: String = sqlx::query_scalar(
        "SELECT detail->>'reason' FROM audit_events WHERE action = 'auth.password.failed'",
    )
    .fetch_one(&l.pool)
    .await
    .expect("audit");
    assert_eq!(reason, "disabled");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn people_change_their_own_password_and_their_other_sessions_end(pool: Pool) {
    let (l, admin) = setup_lab(pool).await;
    let (_, link) = create(&l.state, &admin, "grace").await;
    assert_eq!(
        set_up(&l.state, &link, PASSWORD).await.0,
        StatusCode::NO_CONTENT
    );
    let here = session(&l.state, "grace", PASSWORD).await;
    let elsewhere = session(&l.state, "grace", PASSWORD).await;
    let change = |current: &str, new: &str| json!({ "current": current, "new": new });

    let (status, e, _) = call(
        &l.state,
        "PUT",
        "/api/v1/me/password",
        Some(&here),
        Some(change("nope nope nope", "a fresh passphrase")),
    )
    .await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::BAD_REQUEST, Some("wrong_password"))
    );
    let (status, e, _) = call(
        &l.state,
        "PUT",
        "/api/v1/me/password",
        Some(&here),
        Some(change(PASSWORD, "short")),
    )
    .await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::BAD_REQUEST, Some("weak_password"))
    );
    let (status, e, _) = call(
        &l.state,
        "PUT",
        "/api/v1/me/password",
        Some(&here),
        Some(change(PASSWORD, PASSWORD)),
    )
    .await;
    assert_eq!(
        code(&e),
        Some("weak_password"),
        "{status}: the same password"
    );
    let (status, e, _) = call(
        &l.state,
        "PUT",
        "/api/v1/me/password",
        Some(&here),
        Some(change(PASSWORD, "a fresh passphrase")),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{e}");
    assert_eq!(
        whoami(&l.state, &here).await.0,
        StatusCode::OK,
        "this session stays"
    );
    assert_eq!(
        whoami(&l.state, &elsewhere).await.0,
        StatusCode::UNAUTHORIZED,
        "others end"
    );
    assert_eq!(
        sign_in(&l.state, "grace", PASSWORD).await.0,
        StatusCode::UNAUTHORIZED
    );
    session(&l.state, "grace", "a fresh passphrase").await;
    assert_eq!(audit_count(&l.pool, "auth.password.changed").await, 1);

    // Only local accounts have a password here.
    let person = l.token("someone", "someone");
    let (status, e, _) = call(
        &l.state,
        "PUT",
        "/api/v1/me/password",
        Some(&person),
        Some(change("x", "a fresh passphrase")),
    )
    .await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::CONFLICT, Some("not_a_local_account"))
    );
    let (status, e, _) = call(
        &l.state,
        "PUT",
        "/api/v1/me/password",
        Some(&admin),
        Some(change("x", "a fresh passphrase")),
    )
    .await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::CONFLICT, Some("not_a_local_account"))
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn ten_attempts_a_minute_per_address_then_429_with_retry_after(pool: Pool) {
    let (mut l, admin) = setup_lab(pool).await;
    l.state.sign_in_attempts = std::sync::Arc::new(sessions_auth::Attempts::default());
    let _ = admin;
    for _ in 0..10 {
        assert_eq!(
            sign_in(&l.state, "nobody", "whatever it is").await.0,
            StatusCode::UNAUTHORIZED
        );
    }
    let (status, e, headers) = call(
        &l.state,
        "POST",
        "/api/v1/sign-in/password",
        None,
        Some(json!({ "username": "nobody", "password": "whatever it is" })),
    )
    .await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::TOO_MANY_REQUESTS, Some("rate_limited"))
    );
    let wait: u64 = headers[header::RETRY_AFTER]
        .to_str()
        .expect("header")
        .parse()
        .expect("seconds");
    assert!((1..=60).contains(&wait), "{wait}");
    // Setup links count against the same allowance.
    let (status, _) = set_up(&l.state, "apw_x", PASSWORD).await;
    assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
}

/// A password sign-in from `peer`, optionally through a proxy that forwarded it for `forwarded`.
async fn sign_in_from(state: &AppState, peer: &str, forwarded: Option<&str>) -> StatusCode {
    let mut req = Request::builder()
        .method("POST")
        .uri("/api/v1/sign-in/password")
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(f) = forwarded {
        req = req.header("x-forwarded-for", f);
    }
    let mut req = req
        .body(Body::from(
            json!({ "username": "nobody", "password": "whatever it is" }).to_string(),
        ))
        .expect("request");
    req.extensions_mut()
        .insert(axum::extract::ConnectInfo(std::net::SocketAddr::new(
            peer.parse().expect("address"),
            40_000,
        )));
    router(state.clone())
        .oneshot(req)
        .await
        .expect("response")
        .status()
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn behind_a_trusted_proxy_each_client_has_its_own_sign_in_allowance(pool: Pool) {
    let (mut l, _) = setup_lab(pool.clone()).await;
    l.state = l
        .state
        .clone()
        .with_rate_limits(rate_limit::RateLimitConfig {
            trusted_proxies: vec![rate_limit::Cidr::parse("10.0.0.0/24").expect("cidr")],
            ..rate_limit::RateLimitConfig::default()
        });
    l.state.sign_in_attempts = std::sync::Arc::new(sessions_auth::Attempts::default());
    let proxy = "10.0.0.2";
    for _ in 0..10 {
        assert_eq!(
            sign_in_from(&l.state, proxy, Some("198.51.100.7")).await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        sign_in_from(&l.state, proxy, Some("198.51.100.7")).await,
        StatusCode::TOO_MANY_REQUESTS,
        "the client that used its allowance is limited"
    );
    assert_eq!(
        sign_in_from(&l.state, proxy, Some("198.51.100.8")).await,
        StatusCode::UNAUTHORIZED,
        "another client behind the same proxy is not"
    );
    // A forwarded address from an untrusted peer is not believed: that peer is the client.
    assert_eq!(
        sign_in_from(&l.state, "198.51.100.7", Some("203.0.113.1")).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    // IPv6 clients share an allowance per /64.
    for _ in 0..10 {
        assert_eq!(
            sign_in_from(&l.state, proxy, Some("2001:db8:1:2::1")).await,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        sign_in_from(&l.state, proxy, Some("2001:db8:1:2:ffff::9")).await,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(
        sign_in_from(&l.state, proxy, Some("2001:db8:1:3::1")).await,
        StatusCode::UNAUTHORIZED
    );
    // The audit trail names the client, never the proxy.
    let addresses: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT detail->>'address' FROM audit_events WHERE action = 'auth.password.failed' ORDER BY 1",
    )
    .fetch_all(&pool)
    .await
    .expect("audit");
    assert_eq!(
        addresses,
        [
            "198.51.100.7",
            "198.51.100.8",
            "2001:db8:1:2::1",
            "2001:db8:1:3::1"
        ]
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn creating_accounts_is_checked_validated_and_unique(pool: Pool) {
    let (l, admin) = setup_lab(pool).await;
    create(&l.state, &admin, "henry").await;
    let post = |token: String, body: Value| {
        let state = l.state.clone();
        async move {
            call(
                &state,
                "POST",
                "/api/v1/identity/local-accounts",
                Some(&token),
                Some(body),
            )
            .await
        }
    };
    let (status, e, _) = post(admin.clone(), json!({ "username": "HENRY" })).await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::CONFLICT, Some("username_taken"))
    );
    // Nobody else may already go by that name either (an identity-provider user here).
    l.user("sub-ivy", "ivy").await;
    let (status, e, _) = post(admin.clone(), json!({ "username": "ivy" })).await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::CONFLICT, Some("username_taken"))
    );
    for bad in ["a", "two words", "x@example.org", &"y".repeat(65)] {
        let (status, _, _) = post(admin.clone(), json!({ "username": bad })).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    let (status, _, _) = post(
        admin.clone(),
        json!({ "username": "jack", "email": "not an address" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // People without user.manage are refused at the gate and by the handler.
    let auditor = apelora_persistence::create_token(&l.pool, "aud", "auditor", None)
        .await
        .expect("token")
        .0;
    let (status, _, _) = post(auditor.clone(), json!({ "username": "kate" })).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let person = l.token("sub-lee", "lee");
    let (status, e, _) = post(person.clone(), json!({ "username": "kate" })).await;
    assert_eq!(status, StatusCode::FORBIDDEN, "{e}");
    let (status, _, _) = call(
        &l.state,
        "PUT",
        "/api/v1/identity/sign-in/local-accounts",
        Some(&person),
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let (status, _, _) = call(
        &l.state,
        "POST",
        "/api/v1/identity/local-accounts",
        None,
        Some(json!({ "username": "kate" })),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn turning_local_accounts_off_ends_password_sessions_and_refuses_sign_in(pool: Pool) {
    let (l, admin) = setup_lab(pool).await;
    let (id, link) = create(&l.state, &admin, "mia").await;
    assert_eq!(
        set_up(&l.state, &link, PASSWORD).await.0,
        StatusCode::NO_CONTENT
    );
    let s = session(&l.state, "mia", PASSWORD).await;
    // A directory that is not offered for sign-in is refused before any bind.
    let (status, e, _) = call(
        &l.state,
        "POST",
        "/api/v1/sign-in/password",
        None,
        Some(json!({ "username": "mia", "password": PASSWORD, "directory": Uuid::new_v4() })),
    )
    .await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::BAD_REQUEST, Some("directory_unavailable"))
    );

    // A super admin signed in with a local account cannot turn local accounts off themselves.
    apelora_persistence::identity::insert_assignment(
        &l.pool,
        &new_assignment(id, Builtin::PlatformSuperAdmin, Scope::Platform),
    )
    .await
    .expect("assign");
    let (status, e, _) = call(
        &l.state,
        "PUT",
        "/api/v1/identity/sign-in/local-accounts",
        Some(&s),
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::CONFLICT, Some("would_lock_out"))
    );
    let (status, settings, _) =
        call(&l.state, "GET", "/api/v1/identity/sign-in", Some(&s), None).await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(
        settings["localAccounts"],
        json!({ "enabled": true, "ready": 1, "pending": 0 })
    );

    let (status, r, _) = call(
        &l.state,
        "PUT",
        "/api/v1/identity/sign-in/local-accounts",
        Some(&admin),
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(r["enabled"], false);
    assert_eq!(whoami(&l.state, &s).await.0, StatusCode::UNAUTHORIZED);
    let (status, e) = sign_in(&l.state, "mia", PASSWORD).await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::BAD_REQUEST, Some("local_accounts_disabled"))
    );
    let (_, opts, _) = call(&l.state, "GET", "/api/v1/sign-in", None, None).await;
    assert_eq!(opts["localAccounts"], false);
    assert_eq!(
        audit_count(&l.pool, "identity.local_accounts.disabled").await,
        1
    );
}

/// A lab installation with no identity provider: local accounts can be the only way in, and
/// the guards count them.
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn local_accounts_count_as_a_way_in_for_the_lockout_guards(pool: Pool) {
    let state = AppState::new(
        apelora_incus_adapter::Provider::Mock(MockIncus::default()),
        Auth::Tokens(pool.clone()),
    );
    let admin = apelora_persistence::create_token(&pool, "admin", "fabric-admin", None)
        .await
        .expect("token")
        .0;
    assert!(
        !sessions_auth::local_way_in(&state).await.expect("guard"),
        "off"
    );
    let (status, _, _) = call(
        &state,
        "PUT",
        "/api/v1/identity/sign-in/local-accounts",
        Some(&admin),
        Some(json!({ "enabled": true })),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !sessions_auth::local_way_in(&state).await.expect("guard"),
        "on, but nobody is set up"
    );
    let (id, link) = create(&state, &admin, "root-admin").await;
    assert_eq!(
        set_up(&state, &link, PASSWORD).await.0,
        StatusCode::NO_CONTENT
    );
    assert!(sessions_auth::local_way_in(&state).await.expect("guard"));
    apelora_persistence::identity::insert_assignment(
        &pool,
        &new_assignment(id, Builtin::PlatformSuperAdmin, Scope::Platform),
    )
    .await
    .expect("assign");

    // With a local account set up, API tokens can be turned off (by the local admin).
    let s = session(&state, "root-admin", PASSWORD).await;
    let (status, r, _) = call(
        &state,
        "PUT",
        "/api/v1/identity/sign-in/api-tokens",
        Some(&s),
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{r}");
    assert_eq!(
        whoami(&state, &admin).await.0,
        StatusCode::UNAUTHORIZED,
        "API tokens are off"
    );
    assert_eq!(
        whoami(&state, &s).await.0,
        StatusCode::OK,
        "the local account is the way in"
    );
    // Now local accounts are the only way in: they cannot be turned off.
    assert!(super::auth_config::guard_leaving_local_accounts(&state).is_err());
    let (status, e, _) = call(
        &state,
        "PUT",
        "/api/v1/identity/sign-in/local-accounts",
        Some(&s),
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(
        (status, code(&e)),
        (StatusCode::CONFLICT, Some("would_lock_out"))
    );
    // A disabled person does not count.
    apelora_persistence::identity::set_user_disabled(&pool, id, true)
        .await
        .expect("disable");
    assert!(!sessions_auth::local_way_in(&state).await.expect("guard"));
    // With API tokens on, turning local accounts off is fine.
    let on = AppState::new(
        apelora_incus_adapter::Provider::Mock(MockIncus::default()),
        Auth::Tokens(pool.clone()),
    );
    assert!(super::auth_config::guard_leaving_local_accounts(&on).is_ok());
}
