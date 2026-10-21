// SPDX-License-Identifier: AGPL-3.0-only
//! Personal tokens, sign-ins, effective settings and the support bundle, through the real router
//! against PostgreSQL.

use axum::body::Body;
use axum::http::{HeaderMap, Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use super::identity_tests::{Lab, code, lab};
use super::sessions_auth_tests::{call, create, session, set_up, setup_lab};
use super::*;

const PASSWORD: &str = "correct horse battery";

async fn req(
    l: &Lab,
    method: &str,
    path: &str,
    token: &str,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let (s, v, _) = call(&l.state, method, path, Some(token), body).await;
    (s, v)
}

/// Creates a personal token for the caller; returns the secret and its id.
async fn personal(l: &Lab, by: &str, name: &str, days: i64) -> (String, String) {
    let (s, v, h) = call(
        &l.state,
        "POST",
        "/api/v1/me/tokens",
        Some(by),
        Some(json!({ "name": name, "expiresInDays": days })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    let token = v["token"].as_str().expect("token").to_owned();
    assert!(token.starts_with("apt_") && token.len() == 68);
    (token, v["info"]["id"].as_str().expect("id").to_owned())
}

async fn audited(pool: &Pool, action: &str, outcome: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = $1 AND outcome = $2")
        .bind(action)
        .bind(outcome)
        .fetch_one(pool)
        .await
        .expect("audit")
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_personal_token_acts_as_its_person_with_what_they_hold_now(pool: Pool) {
    let l = lab(pool);
    let (_, root) = l.super_admin("sub-root", "root").await;
    let alice = l.user("sub-alice", "alice").await;
    let alice_jwt = l.token("sub-alice", "alice");
    let (s, grant) = l
        .grant(&root, alice, "viewer", json!({ "type": "platform" }))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{grant}");

    // An expiry is required, 1–365 days.
    for body in [
        json!({ "name": "x" }),
        json!({ "name": "x", "expiresInDays": 366 }),
        json!({ "name": " ", "expiresInDays": 1 }),
    ] {
        let (s, v) = req(&l, "POST", "/api/v1/me/tokens", &alice_jwt, Some(body)).await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    }
    let (token, id) = personal(&l, &alice_jwt, "laptop", 30).await;
    assert_eq!(
        audited(&l.pool, "personal_token.create", "success").await,
        1
    );
    let rows: String = sqlx::query_scalar("SELECT string_agg(detail::text, ' ') FROM audit_events")
        .fetch_one(&l.pool)
        .await
        .expect("audit");
    assert!(!rows.contains(&token), "the token is never audited");

    let (s, me) = req(&l, "GET", "/api/v1/whoami", &token, None).await;
    assert_eq!(s, StatusCode::OK, "{me}");
    assert_eq!(
        (me["name"].as_str(), me["credential"].as_str()),
        (Some("alice"), Some("personal-token"))
    );
    assert_eq!(me["id"].as_str(), Some(alice.to_string().as_str()));
    let (s, _) = req(&l, "GET", "/api/v1/cluster", &token, None).await;
    assert_eq!(s, StatusCode::OK);
    let (s, list) = req(&l, "GET", "/api/v1/me/tokens", &token, None).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(list["items"][0]["id"].as_str(), Some(id.as_str()));
    assert_eq!(list["items"][0]["current"], true);
    assert_eq!(list["items"][0]["state"], "active");
    assert!(list["items"][0]["lastUsedAt"].is_string());

    // The role is revoked: the token loses it on the next request.
    let (s, v) = req(
        &l,
        "DELETE",
        &format!(
            "/api/v1/identity/assignments/{}",
            grant["id"].as_str().expect("id")
        ),
        &root,
        None,
    )
    .await;
    assert!(s.is_success(), "{v}");
    let (s, _) = req(&l, "GET", "/api/v1/cluster", &token, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    // Disabled: refused at once, and still refused after being enabled again (revoked).
    let (s, v) = req(
        &l,
        "POST",
        &format!("/api/v1/identity/users/{alice}/disable"),
        &root,
        None,
    )
    .await;
    assert!(s.is_success(), "{v}");
    let (s, _) = req(&l, "GET", "/api/v1/whoami", &token, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, v) = req(
        &l,
        "POST",
        &format!("/api/v1/identity/users/{alice}/enable"),
        &root,
        None,
    )
    .await;
    assert!(s.is_success(), "{v}");
    let (s, _) = req(&l, "GET", "/api/v1/whoami", &token, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (_, list) = req(&l, "GET", "/api/v1/me/tokens", &alice_jwt, None).await;
    assert_eq!(
        (
            list["items"][0]["state"].as_str(),
            list["items"][0]["revokedBy"].as_str()
        ),
        (Some("revoked"), Some("disabled"))
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_personal_token_never_opens_sessions_or_makes_credentials(pool: Pool) {
    let l = lab(pool);
    let (_, root) = l.super_admin("sub-root", "root").await;
    let (token, id) = personal(&l, &root, "automation", 7).await;
    // Even a super admin's token: interactive sessions and new credentials need the person.
    for (method, path, body) in [
        (
            "POST",
            "/api/v1/applications/shop/instances/shop-web-01/sessions".to_owned(),
            json!({ "kind": "terminal" }),
        ),
        (
            "POST",
            "/api/v1/me/tokens".to_owned(),
            json!({ "name": "more", "expiresInDays": 1 }),
        ),
        ("POST", format!("/api/v1/me/tokens/{id}/rotate"), json!({})),
        (
            "POST",
            "/api/v1/tokens".to_owned(),
            json!({ "name": "ci", "role": "auditor", "expiresInDays": 1 }),
        ),
        (
            "POST",
            "/api/v1/identity/service-identities".to_owned(),
            json!({ "name": "bot", "kind": "automation", "expiresInDays": 1 }),
        ),
        (
            "POST",
            "/api/v1/identity/local-accounts".to_owned(),
            json!({ "username": "eve" }),
        ),
        (
            "POST",
            "/api/v1/cluster/join-tokens".to_owned(),
            json!({ "memberName": "m9" }),
        ),
        (
            "PUT",
            "/api/v1/me/password".to_owned(),
            json!({ "current": "a", "new": "b" }),
        ),
        (
            "POST",
            format!("/api/v1/approvals/{}/approve", Uuid::new_v4()),
            Value::Null,
        ),
        (
            "POST",
            "/api/v1/identity/emergency-access".to_owned(),
            json!({}),
        ),
    ] {
        let body = (!body.is_null()).then_some(body);
        let (s, v) = req(&l, method, &path, &token, body).await;
        assert_eq!(
            (s, code(&v)),
            (StatusCode::FORBIDDEN, Some("personal_token_refused")),
            "{method} {path}: {v}"
        );
    }
    assert_eq!(
        audited(&l.pool, "access.denied", "denied").await,
        10,
        "every refusal is on the record"
    );
    // What the person may do otherwise works.
    let (s, _) = req(&l, "GET", "/api/v1/identity/users", &token, None).await;
    assert_eq!(s, StatusCode::OK);
    // The access check says why, too.
    let (s, v) = req(&l, "GET", "/api/v1/whoami", &token, None).await;
    assert_eq!(
        (s, v["credential"].as_str()),
        (StatusCode::OK, Some("personal-token"))
    );

    // Personal tokens and sign-ins belong to people.
    let legacy = apelora_persistence::create_token(&l.pool, "legacy", "fabric-admin", None)
        .await
        .expect("token")
        .0;
    for (method, path) in [
        ("GET", "/api/v1/me/tokens"),
        ("POST", "/api/v1/me/tokens"),
        ("GET", "/api/v1/me/sign-ins"),
    ] {
        let (s, v) = req(
            &l,
            method,
            path,
            &legacy,
            Some(json!({ "name": "x", "expiresInDays": 1 })),
        )
        .await;
        assert_eq!(
            (s, code(&v)),
            (StatusCode::FORBIDDEN, Some("people_only")),
            "{method} {path}"
        );
    }
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn tokens_rotate_with_an_overlap_and_are_revoked_by_their_person_or_an_administrator(
    pool: Pool,
) {
    let l = lab(pool);
    let (_, root) = l.super_admin("sub-root", "root").await;
    let alice = l.user("sub-alice", "alice").await;
    let alice_jwt = l.token("sub-alice", "alice");
    let bob_jwt = l.token("sub-bob", "bob");
    l.user("sub-bob", "bob").await;
    let (old, id) = personal(&l, &alice_jwt, "backup script", 90).await;

    let (s, v) = req(
        &l,
        "POST",
        &format!("/api/v1/me/tokens/{id}/rotate"),
        &alice_jwt,
        Some(json!({ "overlapMinutes": 1441 })),
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{v}");
    let (s, v) = req(
        &l,
        "POST",
        &format!("/api/v1/me/tokens/{id}/rotate"),
        &bob_jwt,
        Some(json!({})),
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "another person's token: {v}");
    let (s, r, h) = call(
        &l.state,
        "POST",
        &format!("/api/v1/me/tokens/{id}/rotate"),
        Some(&alice_jwt),
        Some(json!({ "overlapMinutes": 30 })),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{r}");
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    let new = r["token"].as_str().expect("token").to_owned();
    assert_eq!(r["info"]["name"], "backup script");
    assert_eq!(r["info"]["rotatedFrom"].as_str(), Some(id.as_str()));
    assert_eq!(r["previous"]["state"], "rotating");
    let left = time::OffsetDateTime::parse(
        r["previous"]["expiresAt"].as_str().expect("expiry"),
        &time::format_description::well_known::Rfc3339,
    )
    .expect("time")
        - time::OffsetDateTime::now_utc();
    assert!(
        left <= time::Duration::minutes(30) && left > time::Duration::minutes(29),
        "{left}"
    );
    // The successor keeps the old token's lifetime.
    let life = time::OffsetDateTime::parse(
        r["info"]["expiresAt"].as_str().expect("expiry"),
        &time::format_description::well_known::Rfc3339,
    )
    .expect("time")
        - time::OffsetDateTime::now_utc();
    assert!(life > time::Duration::days(89), "{life}");
    for t in [&old, &new] {
        let (s, _) = req(&l, "GET", "/api/v1/whoami", t, None).await;
        assert_eq!(s, StatusCode::OK, "both work during the overlap");
    }
    let (s, v) = req(
        &l,
        "POST",
        &format!("/api/v1/me/tokens/{id}/rotate"),
        &alice_jwt,
        Some(json!({})),
    )
    .await;
    assert_eq!(
        (s, code(&v)),
        (StatusCode::CONFLICT, Some("already_rotated"))
    );
    assert_eq!(
        audited(&l.pool, "personal_token.rotate", "success").await,
        1
    );

    // Revoked by the person: only their own.
    let new_id = r["info"]["id"].as_str().expect("id").to_owned();
    let (s, _) = req(
        &l,
        "DELETE",
        &format!("/api/v1/me/tokens/{new_id}"),
        &bob_jwt,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = req(
        &l,
        "DELETE",
        &format!("/api/v1/me/tokens/{id}"),
        &alice_jwt,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = req(&l, "GET", "/api/v1/whoami", &old, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);

    // Administrators see everyone's and revoke any; others may not.
    let (s, _) = req(
        &l,
        "GET",
        "/api/v1/identity/personal-tokens",
        &bob_jwt,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, all) = req(
        &l,
        "GET",
        &format!("/api/v1/identity/personal-tokens?user={alice}"),
        &root,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{all}");
    assert_eq!(all["items"].as_array().map(Vec::len), Some(2));
    assert!(
        all["items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|t| t["owner"] == "alice")
    );
    let (s, _) = req(
        &l,
        "DELETE",
        &format!("/api/v1/identity/personal-tokens/{new_id}"),
        &bob_jwt,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = req(
        &l,
        "DELETE",
        &format!("/api/v1/identity/personal-tokens/{new_id}"),
        &root,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = req(&l, "GET", "/api/v1/whoami", &new, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    assert_eq!(
        audited(&l.pool, "personal_token.revoke", "success").await,
        2
    );
    let by: String =
        sqlx::query_scalar("SELECT revoked_by FROM personal_tokens WHERE id = $1::uuid")
            .bind(&new_id)
            .fetch_one(&l.pool)
            .await
            .expect("revoked by");
    assert!(by.starts_with("user:"), "{by}");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn people_see_and_end_their_sign_ins_and_administrators_anyones(pool: Pool) {
    let (l, admin) = setup_lab(pool).await;
    let (carol, setup) = create(&l.state, &admin, "carol").await;
    assert_eq!(
        set_up(&l.state, &setup, PASSWORD).await.0,
        StatusCode::NO_CONTENT
    );
    let here = session(&l.state, "carol", PASSWORD).await;
    let there = session(&l.state, "carol", PASSWORD).await;

    let (s, list) = req(&l, "GET", "/api/v1/me/sign-ins", &here, None).await;
    assert_eq!(s, StatusCode::OK, "{list}");
    assert_eq!(list["credential"], "session");
    let items = list["items"].as_array().expect("items");
    assert_eq!(items.len(), 2);
    assert_eq!(items.iter().filter(|i| i["current"] == true).count(), 1);
    assert!(
        items
            .iter()
            .all(|i| i["method"] == "password" && i["person"] == "carol")
    );
    let other = items.iter().find(|i| i["current"] == false).expect("other")["id"]
        .as_str()
        .expect("id")
        .to_owned();
    assert!(!list.to_string().contains(&here), "never a session token");

    // A person signed in with a provider has no sessions of their own here, and cannot end carol's.
    let (_, root) = l.super_admin("sub-root", "root").await;
    let dave = l.token("sub-dave", "dave");
    l.user("sub-dave", "dave").await;
    let (s, v) = req(&l, "GET", "/api/v1/me/sign-ins", &dave, None).await;
    assert_eq!(
        (
            s,
            v["credential"].as_str(),
            v["items"].as_array().map(Vec::len)
        ),
        (StatusCode::OK, Some("identity-provider"), Some(0))
    );
    let (s, _) = req(
        &l,
        "DELETE",
        &format!("/api/v1/me/sign-ins/{other}"),
        &dave,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (s, _) = req(&l, "GET", "/api/v1/identity/sign-ins", &dave, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);

    let (s, _) = req(
        &l,
        "DELETE",
        &format!("/api/v1/me/sign-ins/{other}"),
        &here,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = req(&l, "GET", "/api/v1/whoami", &there, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "that browser is signed out");
    assert_eq!(audited(&l.pool, "auth.session.ended", "success").await, 1);

    let (s, all) = req(
        &l,
        "GET",
        &format!("/api/v1/identity/sign-ins?user={carol}"),
        &root,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{all}");
    let id = all["items"][0]["id"].as_str().expect("id").to_owned();
    assert_eq!(all["items"].as_array().map(Vec::len), Some(1));
    let (s, _) = req(
        &l,
        "DELETE",
        &format!("/api/v1/identity/sign-ins/{id}"),
        &root,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    let (s, _) = req(&l, "GET", "/api/v1/whoami", &here, None).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = req(
        &l,
        "DELETE",
        &format!("/api/v1/identity/sign-ins/{id}"),
        &root,
        None,
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND, "already ended");
}

/// Token-shaped and key-shaped values are built at run time, so the repository's secret scan never
/// sees one in the source.
fn planted_token() -> String {
    format!("apl_{}", "0123456789abcdef".repeat(4))
}
fn planted_key() -> String {
    let label = ["PRIVATE", "KEY"].join(" ");
    format!("-----BEGIN {label}-----\nMIIEplantedkeymaterial\n-----END {label}-----")
}

fn planted_environment() -> Vec<(String, String)> {
    let token = planted_token();
    [
        ("APELORA_API_ADDR", "127.0.0.1:8080"),
        (
            "DATABASE_URL",
            "postgres://apelora:db-password-planted@db.internal/apelora",
        ),
        ("APELORA_TLS_KEY", "/srv/planted/api.key"),
        ("APELORA_INCUS_CLIENT_KEY", "/srv/planted/client.key"),
        ("APELORA_OIDC_LABEL", token.as_str()),
        ("APELORA_HOOK_SECRET", "hook-secret-planted"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect()
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn effective_settings_show_values_only_by_allow_list(pool: Pool) {
    let mut l = lab(pool);
    l.state = l.state.clone().with_environment(planted_environment());
    let (_, root) = l.super_admin("sub-root", "root").await;
    let (s, v) = req(&l, "GET", "/api/v1/settings", &root, None).await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let item = |n: &str| {
        v["items"]
            .as_array()
            .expect("items")
            .iter()
            .find(|i| i["name"] == n)
            .cloned()
            .expect(n)
    };
    assert_eq!(item("APELORA_API_ADDR")["value"], "127.0.0.1:8080");
    assert_eq!(item("APELORA_API_ADDR")["isDefault"], true);
    assert_eq!(item("DATABASE_URL")["hidden"], "secret");
    assert_eq!(item("APELORA_HOOK_SECRET")["known"], false);
    assert_eq!(v["saved"][0]["name"], "auth.local_accounts");
    let text = v.to_string();
    let token = planted_token();
    for planted in [
        token.as_str(),
        "db-password-planted",
        "/srv/planted",
        "hook-secret-planted",
    ] {
        assert!(!text.contains(planted), "{planted} leaked");
    }
    // Needs cluster.view on the platform.
    let bob = l.token("sub-bob", "bob");
    l.user("sub-bob", "bob").await;
    let (s, _) = req(&l, "GET", "/api/v1/settings", &bob, None).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

async fn raw(l: &Lab, path: &str, token: &str) -> (StatusCode, HeaderMap, Vec<u8>) {
    let req = Request::builder()
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request");
    let res = router(l.state.clone())
        .oneshot(req)
        .await
        .expect("response");
    let (status, headers) = (res.status(), res.headers().clone());
    let bytes = res
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes()
        .to_vec();
    (status, headers, bytes)
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_support_bundle_holds_the_parts_and_never_a_planted_secret(pool: Pool) {
    let mut l = lab(pool);
    l.state = l.state.clone().with_environment(planted_environment());
    let (_, root) = l.super_admin("sub-root", "root").await;
    // A personal token and an API token exist (only their hashes are stored).
    let (personal_secret, _) = personal(&l, &root, "laptop", 1).await;
    // A failed job whose error and request body hold secrets, with an attempt that does too.
    let job = Uuid::new_v4();
    sqlx::query(
        "INSERT INTO jobs (id, kind, payload, status, attempts, last_error, updated_at)
         VALUES ($1, 'instance.create', $2, 'failed', 1, $3, now())",
    )
    .bind(job)
    .bind(json!({ "config": { "environment.DB_PASSWORD": "payload-password-planted" } }))
    .bind(format!(
        "Incus refused: Authorization: Bearer bearer-planted-value; token {}; {}",
        planted_token(),
        planted_key()
    ))
    .execute(&l.pool)
    .await
    .expect("job");
    sqlx::query(
        "INSERT INTO job_attempts (job_id, attempt, generation, worker, started_at, ended_at, outcome, error)
         VALUES ($1, 1, 1, 'worker-1', now() - interval '1 minute', now(), 'failed', $2)",
    )
    .bind(job)
    .bind(format!(
        "POST https://user:url-password-planted@incus.internal:8443/1.0/instances {}",
        planted_token()
    ))
    .execute(&l.pool)
    .await
    .expect("attempt");

    let (s, h, bytes) = raw(&l, "/api/v1/support-bundle", &root).await;
    assert_eq!(s, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    assert_eq!(h[header::CONTENT_TYPE], "application/x-tar");
    assert_eq!(h[header::CACHE_CONTROL], "no-store");
    assert!(
        h[header::CONTENT_DISPOSITION]
            .to_str()
            .expect("disposition")
            .starts_with("attachment; filename=\"apelora-support-")
    );
    let files = super::support::tests::untar(&bytes);
    let names: Vec<&str> = files
        .iter()
        .map(|(n, _)| n.split_once('/').expect("dir").1)
        .collect();
    assert_eq!(
        names,
        [
            "manifest.json",
            "versions.json",
            "settings.json",
            "health.json",
            "preflight.json",
            "jobs-failed.json",
            "metrics.txt",
            "logs/README.txt",
            "README.txt"
        ]
    );
    let manifest: Value = serde_json::from_slice(&files[0].1).expect("manifest");
    assert_eq!(manifest["format"], "apelora-support-bundle/1");
    assert_eq!(
        manifest["omitted"].as_array().map(Vec::len),
        Some(0),
        "{manifest}"
    );
    let jobs: Value = serde_json::from_slice(&files[5].1).expect("jobs");
    assert_eq!(
        jobs["items"][0]["id"].as_str(),
        Some(job.to_string().as_str())
    );
    assert_eq!(jobs["items"][0]["attemptHistory"][0]["worker"], "worker-1");
    assert!(jobs["items"][0].get("request").is_none() && jobs["items"][0].get("payload").is_none());
    assert!(
        String::from_utf8_lossy(&files[6].1).contains("apelora_"),
        "metrics"
    );

    let all = String::from_utf8_lossy(&bytes);
    let token = planted_token();
    let key_header = format!("BEGIN {}", ["PRIVATE", "KEY"].join(" "));
    for planted in [
        token.as_str(),
        personal_secret.as_str(),
        "db-password-planted",
        "/srv/planted",
        "hook-secret-planted",
        "payload-password-planted",
        "bearer-planted-value",
        "MIIEplantedkeymaterial",
        "url-password-planted",
        key_header.as_str(),
    ] {
        assert!(!all.contains(planted), "{planted} reached the bundle");
    }
    assert_eq!(
        audited(&l.pool, "support.bundle.downloaded", "success").await,
        1
    );

    // Needs support.bundle: a viewer may not.
    let bob = l.user("sub-bob", "bob").await;
    let bob_jwt = l.token("sub-bob", "bob");
    let (s, v) = l
        .grant(&root, bob, "viewer", json!({ "type": "platform" }))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let (s, _, _) = raw(&l, "/api/v1/support-bundle", &bob_jwt).await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // A fabric administrator may.
    let carl = l.user("sub-carl", "carl").await;
    let (s, v) = l
        .grant(
            &root,
            carl,
            "fabric-administrator",
            json!({ "type": "platform" }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{v}");
    let (s, _, _) = raw(&l, "/api/v1/support-bundle", &l.token("sub-carl", "carl")).await;
    assert_eq!(s, StatusCode::OK);
}
