// Containment of credentials and the guards around the last platform super admin, through the
// real router against a real PostgreSQL (CONSOLE-PLAN security gaps 1, 2, 3 and 6).

use apelora_persistence::identity as db;
use serde_json::json;
use uuid::Uuid;

use super::identity_tests::{Lab, code, lab, world};
use super::*;

/// What a token of each legacy role can do, through the legacy mapping in `authz`.
const AUDITOR_TOKEN_CAN: &[&str] = &[
    "cluster.view",
    "hardware.view",
    "organization.view",
    "project.view",
    "application.view",
    "application.logs.read",
    "application.metrics.read",
    "capacity.view",
    "fabric.metrics.read",
    "user.view",
    "audit.read",
];

/// Creates a custom role with exactly these permissions and grants it at platform level.
async fn custom_at_platform(l: &Lab, admin: &str, user: Uuid, name: &str, perms: &[&str]) {
    let (s, role) = l
        .req(
            "POST",
            "/api/v1/identity/roles",
            admin,
            Some(json!({ "name": name, "permissions": perms })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{role}");
    let (s, b) = l.req("POST", "/api/v1/identity/assignments", admin, Some(json!({ "principal": { "type": "user", "id": user }, "role": { "type": "custom", "id": role["reference"]["id"] }, "scope": { "type": "platform" } }))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
}

async fn with_approval(
    l: &Lab,
    token: &str,
    path: &str,
    approval: Option<&str>,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    use http_body_util::BodyExt;
    use tower::ServiceExt;
    let mut req = axum::http::Request::post(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .header(header::CONTENT_TYPE, "application/json");
    if let Some(a) = approval {
        req = req.header("apelora-approval", a);
    }
    let res = router(l.state.clone())
        .oneshot(
            req.body(axum::body::Body::from(body.to_string()))
                .expect("request"),
        )
        .await
        .expect("response");
    let status = res.status();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

async fn audit_rows(pool: &Pool, action: &str) -> Vec<(String, String)> {
    sqlx::query_as("SELECT outcome, detail::text FROM audit_events WHERE action = $1 ORDER BY id")
        .bind(action)
        .fetch_all(pool)
        .await
        .expect("audit")
}

// ---- S1: tokens --------------------------------------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_token_never_carries_more_than_its_creator_holds(pool: Pool) {
    let l = lab(pool.clone());
    let w = world(&l).await;
    let clerk = l.user("sub-clerk", "clerk").await;
    let mut perms = vec!["token.manage"];
    perms.extend_from_slice(AUDITOR_TOKEN_CAN);
    custom_at_platform(&l, &w.admin, clerk, "Token clerk", &perms).await;
    let t = l.token("sub-clerk", "clerk");

    // Everything an auditor token can do, the clerk can do: allowed.
    let (s, b) = l
        .req(
            "POST",
            "/api/v1/tokens",
            &t,
            Some(json!({ "name": "audit-bot", "role": "auditor" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    // An operator token could deploy; the clerk cannot: refused, with what is missing.
    let (s, b) = l
        .req(
            "POST",
            "/api/v1/tokens",
            &t,
            Some(json!({ "name": "ci", "role": "operator" })),
        )
        .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::FORBIDDEN, Some("token_exceeds_caller")),
        "{b}"
    );
    let msg = b["error"]["message"].as_str().unwrap_or_default();
    assert!(
        msg.contains("application.deploy") && msg.contains("operator"),
        "{msg}"
    );
    assert!(!msg.contains("audit.read"), "only what is missing: {msg}");
    // A fabric-admin token: refused too (it could manage people and security).
    let (s, b) = l
        .req(
            "POST",
            "/api/v1/tokens",
            &t,
            Some(json!({ "name": "root", "role": "fabric-admin" })),
        )
        .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::FORBIDDEN, Some("token_exceeds_caller")),
        "{b}"
    );
    // Losing one permission of the auditor set is enough to be refused.
    let narrow = l.user("sub-narrow", "narrow").await;
    custom_at_platform(
        &l,
        &w.admin,
        narrow,
        "Narrow clerk",
        &["token.manage", "cluster.view"],
    )
    .await;
    let (s, b) = l
        .req(
            "POST",
            "/api/v1/tokens",
            &l.token("sub-narrow", "narrow"),
            Some(json!({ "name": "x", "role": "auditor" })),
        )
        .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::FORBIDDEN, Some("token_exceeds_caller")),
        "{b}"
    );

    // Refusals are on the record: as a denied token.create and as a denied request.
    let denied = audit_rows(&pool, "token.create").await;
    assert_eq!(
        denied.iter().map(|(o, _)| o.as_str()).collect::<Vec<_>>(),
        ["success", "denied", "denied", "denied"]
    );
    assert!(denied[1].1.contains("operator") && denied[1].1.contains("application.deploy"));
    let gate: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = 'access.denied' AND detail->>'code' = 'token_exceeds_caller'").fetch_one(&pool).await.expect("count");
    assert_eq!(gate, 3);
    // Nothing was created by the refused calls.
    let roles: Vec<String> = sqlx::query_scalar("SELECT role FROM api_tokens ORDER BY created_at")
        .fetch_all(&pool)
        .await
        .expect("tokens");
    assert_eq!(roles, ["auditor"]);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_fabric_admin_token_needs_another_persons_approval(pool: Pool) {
    let l = lab(pool.clone());
    let w = world(&l).await;
    let fa = l.user("sub-fa", "fa").await;
    let (s, _) = l
        .grant(
            &w.admin,
            fa,
            "fabric-administrator",
            json!({ "type": "platform" }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let approver = l.token("sub-fa", "fa");
    let body = json!({ "name": "break-glass", "role": "fabric-admin", "expiresInDays": 7 });

    let (s, b) = with_approval(&l, &w.admin, "/api/v1/tokens", None, body.clone()).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("approval_required")),
        "{b}"
    );
    let (s, ap) = l.req("POST", "/api/v1/approvals", &w.admin, Some(json!({ "action": "token.create_admin", "target": "token:fabric-admin", "reason": "break-glass token for the runbook" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{ap}");
    let id = ap["id"].as_str().expect("id").to_owned();
    assert_eq!(
        l.req(
            "POST",
            &format!("/api/v1/approvals/{id}/approve"),
            &w.admin,
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "not your own request"
    );
    let (s, b) = with_approval(&l, &w.admin, "/api/v1/tokens", Some(&id), body.clone()).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("approval_invalid")),
        "pending: {b}"
    );
    assert_eq!(
        l.req(
            "POST",
            &format!("/api/v1/approvals/{id}/approve"),
            &approver,
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    let (s, b) = with_approval(&l, &w.admin, "/api/v1/tokens", Some(&id), body.clone()).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert_eq!(b["info"]["role"], "fabric-admin");
    let (s, b) = with_approval(&l, &w.admin, "/api/v1/tokens", Some(&id), body.clone()).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("approval_invalid")),
        "single use: {b}"
    );
    // The approval is recorded with the token.
    let created = audit_rows(&pool, "token.create").await;
    assert!(
        created
            .iter()
            .any(|(o, d)| o == "success" && d.contains(&id)),
        "{created:?}"
    );
    // Other roles need no approval.
    let (s, _) = l
        .req(
            "POST",
            "/api/v1/tokens",
            &w.admin,
            Some(json!({ "name": "ci", "role": "operator" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);

    // A legacy token cannot present an approval, so it cannot mint fabric-admin tokens through
    // the API (the command line on the server still can); smaller roles still work.
    let (legacy, _) = apelora_persistence::create_token(&pool, "old-admin", "fabric-admin", None)
        .await
        .expect("token");
    let (s, b) = l.req("POST", "/api/v1/tokens", &legacy, Some(body)).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::FORBIDDEN, Some("approval_unavailable")),
        "{b}"
    );
    let (s, _) = l
        .req(
            "POST",
            "/api/v1/tokens",
            &legacy,
            Some(json!({ "name": "reader", "role": "auditor" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn legacy_tokens_pass_the_same_route_gate(pool: Pool) {
    let l = lab(pool.clone());
    let (auditor, _) = apelora_persistence::create_token(&pool, "reader", "auditor", None)
        .await
        .expect("token");
    // Approvals need approval.request or approval.grant; a legacy auditor holds neither.
    let (s, b) = l.req("GET", "/api/v1/approvals", &auditor, None).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::FORBIDDEN, Some("forbidden")),
        "{b}"
    );
    // What the role allows still works.
    for path in [
        "/api/v1/cluster",
        "/api/v1/audit",
        "/api/v1/whoami",
        "/api/v1/jobs",
    ] {
        assert_eq!(
            l.req("GET", path, &auditor, None).await.0,
            StatusCode::OK,
            "{path}"
        );
    }
    let denied: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = 'access.denied' AND target = 'GET /api/v1/approvals' AND actor = 'token:reader'").fetch_one(&pool).await.expect("count");
    assert_eq!(denied, 1);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_service_token_lifetime_over_the_maximum_is_refused_not_shortened(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    for days in [0, 366, 10_000] {
        let (s, b) = l
            .req(
                "POST",
                "/api/v1/identity/service-identities",
                &w.admin,
                Some(json!({ "name": format!("ci-{days}"), "kind": "automation", "expiresInDays": days })),
            )
            .await;
        assert_eq!(
            (s, code(&b)),
            (StatusCode::BAD_REQUEST, Some("invalid_request")),
            "{days}: {b}"
        );
    }
    let (s, b) = l
        .req(
            "POST",
            "/api/v1/identity/service-identities",
            &w.admin,
            Some(json!({ "name": "ci-year", "kind": "automation", "expiresInDays": 365 })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
}

// ---- S2: revoking roles and the last super admin ---------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn revoking_a_role_needs_the_power_to_grant_it(pool: Pool) {
    let l = lab(pool.clone());
    let w = world(&l).await;
    let (pa, carol, vic) = (
        l.user("sub-pa", "pa").await,
        l.user("sub-carol", "carol").await,
        l.user("sub-vic", "vic").await,
    );
    let project = json!({ "type": "project", "id": w.acme_prod });
    // pa may assign roles in the project, but holds only what a viewer holds there.
    let (s, role) = l.req("POST", "/api/v1/identity/roles", &w.admin, Some(json!({ "name": "Membership clerk", "permissions": ["role.assign", "project.view", "application.view", "application.metrics.read", "capacity.view"] }))).await;
    assert_eq!(s, StatusCode::CREATED, "{role}");
    let (s, b) = l.req("POST", "/api/v1/identity/assignments", &w.admin, Some(json!({ "principal": { "type": "user", "id": pa }, "role": { "type": "custom", "id": role["reference"]["id"] }, "scope": project }))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let (_, admin_grant) = l
        .grant(&w.admin, carol, "project-administrator", project.clone())
        .await;
    let (_, viewer_grant) = l.grant(&w.admin, vic, "viewer", project.clone()).await;
    let t = l.token("sub-pa", "pa");

    let path = |g: &serde_json::Value| {
        format!(
            "/api/v1/identity/assignments/{}",
            g["id"].as_str().expect("id")
        )
    };
    let (s, b) = l.req("DELETE", &path(&admin_grant), &t, None).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::FORBIDDEN, Some("revocation_refused")),
        "{b}"
    );
    assert!(
        b["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("application.deploy")),
        "{b}"
    );
    // Carol keeps her role.
    assert_eq!(
        l.req(
            "GET",
            "/api/v1/applications",
            &l.token("sub-carol", "carol"),
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    // A role pa could grant, pa may revoke.
    assert_eq!(
        l.req("DELETE", &path(&viewer_grant), &t, None).await.0,
        StatusCode::NO_CONTENT
    );
    let rows = audit_rows(&pool, "role.revoke").await;
    assert_eq!(
        rows.iter().map(|(o, _)| o.as_str()).collect::<Vec<_>>(),
        ["denied", "success"]
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_last_enabled_super_admin_cannot_be_disabled(pool: Pool) {
    let l = lab(pool.clone());
    let w = world(&l).await;
    let root = l.user("sub-root", "root").await;
    let pm = l.user("sub-pm", "pm").await;
    custom_at_platform(&l, &w.admin, pm, "People admin", &["user.manage"]).await;
    let tp = l.token("sub-pm", "pm");

    let disable = |who: Uuid| format!("/api/v1/identity/users/{who}/disable");
    let (s, b) = l.req("POST", &disable(root), &tp, None).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("last_super_admin")),
        "{b}"
    );
    assert_eq!(
        l.req("GET", "/api/v1/whoami", &w.admin, None).await.0,
        StatusCode::OK,
        "root still signs in"
    );
    let rows = audit_rows(&pool, "user.disable").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "denied");
    assert!(rows[0].1.contains("last_super_admin"), "{rows:?}");

    // With a second super admin, either one may go, but not both.
    let (sa2, _) = l.super_admin("sub-sa2", "sa2").await;
    assert_eq!(
        l.req("POST", &disable(root), &tp, None).await.0,
        StatusCode::NO_CONTENT
    );
    let (s, b) = l.req("POST", &disable(sa2), &tp, None).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("last_super_admin")),
        "{b}"
    );
    // A disabled super admin does not count: re-enabling root restores the spare.
    assert_eq!(
        l.req(
            "POST",
            &format!("/api/v1/identity/users/{root}/enable"),
            &tp,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    // Revoking an assignment counts people who are enabled, not rows.
    let (_, list) = l
        .req(
            "GET",
            &format!("/api/v1/identity/assignments?principalType=user&principalId={sa2}"),
            &w.admin,
            None,
        )
        .await;
    let sa2_grant = list["items"][0]["id"].as_str().expect("id").to_owned();
    assert_eq!(
        l.req("POST", &disable(root), &tp, None).await.0,
        StatusCode::NO_CONTENT
    );
    let (s, b) = l
        .req(
            "DELETE",
            &format!("/api/v1/identity/assignments/{sa2_grant}"),
            &l.token("sub-sa2", "sa2"),
            None,
        )
        .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("last_super_admin")),
        "root's grant is still there, but root is disabled: {b}"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn two_people_disabling_the_last_two_super_admins_at_once_leave_one(pool: Pool) {
    let l = lab(pool.clone());
    let w = world(&l).await;
    let root = l.user("sub-root", "root").await;
    let (sa2, _) = l.super_admin("sub-sa2", "sa2").await;
    let (p1, p2) = (l.user("sub-p1", "p1").await, l.user("sub-p2", "p2").await);
    let (s, role) = l
        .req(
            "POST",
            "/api/v1/identity/roles",
            &w.admin,
            Some(json!({ "name": "People admin", "permissions": ["user.manage"] })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    for who in [p1, p2] {
        let (s, b) = l.req("POST", "/api/v1/identity/assignments", &w.admin, Some(json!({ "principal": { "type": "user", "id": who }, "role": { "type": "custom", "id": role["reference"]["id"] }, "scope": { "type": "platform" } }))).await;
        assert_eq!(s, StatusCode::CREATED, "{b}");
    }
    let (t1, t2) = (l.token("sub-p1", "p1"), l.token("sub-p2", "p2"));
    let (d1, d2) = (
        format!("/api/v1/identity/users/{root}/disable"),
        format!("/api/v1/identity/users/{sa2}/disable"),
    );
    for round in 0..5 {
        let (a, b) = tokio::join!(l.req("POST", &d1, &t1, None), l.req("POST", &d2, &t2, None));
        let mut statuses = [a.0, b.0];
        statuses.sort();
        assert_eq!(
            statuses,
            [StatusCode::NO_CONTENT, StatusCode::CONFLICT],
            "round {round}: {a:?} {b:?}"
        );
        let enabled: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM users WHERE id = ANY($1) AND disabled_at IS NULL",
        )
        .bind(vec![root, sa2])
        .fetch_one(&pool)
        .await
        .expect("count");
        assert_eq!(enabled, 1, "round {round}");
        for who in [root, sa2] {
            db::set_user_disabled(&pool, who, false)
                .await
                .expect("enable");
        }
    }
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_last_member_of_the_super_admin_group_cannot_leave_it(pool: Pool) {
    let l = lab(pool.clone());
    let w = world(&l).await;
    let dave = l.user("sub-dave", "dave").await;
    let (s, g) = l
        .req(
            "POST",
            "/api/v1/identity/groups",
            &w.admin,
            Some(json!({ "name": "Platform owners" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{g}");
    let gid = g["id"].as_str().expect("id").to_owned();
    let (s, b) = l.req("POST", "/api/v1/identity/assignments", &w.admin, Some(json!({ "principal": { "type": "group", "id": gid }, "role": { "type": "builtin", "key": "platform-super-admin" }, "scope": { "type": "platform" } }))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let group_grant = b["id"].as_str().expect("id").to_owned();
    let member = format!("/api/v1/identity/groups/{gid}/members/{dave}");
    assert_eq!(
        l.req("PUT", &member, &w.admin, None).await.0,
        StatusCode::NO_CONTENT
    );
    // dave is now a super admin through the group, so root's own grant may go.
    let td = l.token("sub-dave", "dave");
    let root = l.user("sub-root", "root").await;
    let (_, list) = l
        .req(
            "GET",
            &format!("/api/v1/identity/assignments?principalType=user&principalId={root}"),
            &td,
            None,
        )
        .await;
    let root_grant = list["items"][0]["id"].as_str().expect("id").to_owned();
    assert_eq!(
        l.req(
            "DELETE",
            &format!("/api/v1/identity/assignments/{root_grant}"),
            &td,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    // Now dave, through the group, is the only one: he cannot leave it, nor revoke it.
    let (s, b) = l.req("DELETE", &member, &td, None).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("last_super_admin")),
        "{b}"
    );
    let (s, b) = l
        .req(
            "DELETE",
            &format!("/api/v1/identity/assignments/{group_grant}"),
            &td,
            None,
        )
        .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("last_super_admin")),
        "{b}"
    );
    assert_eq!(
        l.req("GET", "/api/v1/identity/users", &td, None).await.0,
        StatusCode::OK,
        "dave keeps the role"
    );
    let rows = audit_rows(&pool, "group.member.remove").await;
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, "denied");
}

/// Joining a group is receiving its roles: the escalation guard applies to every role the group holds.
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn joining_a_group_never_gives_more_than_the_caller_could_grant(pool: Pool) {
    let l = lab(pool.clone());
    let w = world(&l).await;
    let clerk = l.user("sub-clerk", "clerk").await;
    let eve = l.user("sub-eve", "eve").await;
    custom_at_platform(
        &l,
        &w.admin,
        clerk,
        "Group clerk",
        &["group.manage", "user.view"],
    )
    .await;
    let t = l.token("sub-clerk", "clerk");
    let group = |name: &'static str| {
        let admin = w.admin.clone();
        let l = &l;
        async move {
            let (s, g) = l
                .req(
                    "POST",
                    "/api/v1/identity/groups",
                    &admin,
                    Some(json!({ "name": name })),
                )
                .await;
            assert_eq!(s, StatusCode::CREATED, "{g}");
            g["id"].as_str().expect("id").to_owned()
        }
    };
    let owners = group("Platform owners").await;
    let (s, b) = l.req("POST", "/api/v1/identity/assignments", &w.admin, Some(json!({ "principal": { "type": "group", "id": owners }, "role": { "type": "builtin", "key": "platform-super-admin" }, "scope": { "type": "platform" } }))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let acme_owners = group("Acme owners").await;
    let (s, b) = l.req("POST", "/api/v1/identity/assignments", &w.admin, Some(json!({ "principal": { "type": "group", "id": acme_owners }, "role": { "type": "builtin", "key": "organization-owner" }, "scope": { "type": "organization", "id": w.acme } }))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let plain = group("Book club").await;

    let member = |g: &str, u: Uuid| format!("/api/v1/identity/groups/{g}/members/{u}");
    // The clerk may not put himself, or anyone else, into a group that holds roles he could not grant.
    for (g, who) in [
        (&owners, clerk),
        (&owners, eve),
        (&acme_owners, clerk),
        (&acme_owners, eve),
    ] {
        let (s, b) = l.req("PUT", &member(g, who), &t, None).await;
        assert_eq!(
            (s, code(&b)),
            (StatusCode::FORBIDDEN, Some("membership_refused")),
            "{g} {who}: {b}"
        );
    }
    let joined: i64 = sqlx::query_scalar("SELECT count(*) FROM group_members")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(joined, 0, "nobody joined a privileged group");
    assert_eq!(
        l.req("GET", "/api/v1/identity/roles", &t, None).await.0,
        StatusCode::OK
    );
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/identity/roles",
            &t,
            Some(json!({ "name": "x", "permissions": ["user.view"] }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "the clerk is still not a super admin"
    );
    // A group without roles is the clerk's to run.
    assert_eq!(
        l.req("PUT", &member(&plain, eve), &t, None).await.0,
        StatusCode::NO_CONTENT
    );
    // Someone who could grant the group's roles may add members.
    assert_eq!(
        l.req("PUT", &member(&owners, eve), &w.admin, None).await.0,
        StatusCode::NO_CONTENT
    );
    let rows = audit_rows(&pool, "group.member.add").await;
    assert_eq!(
        rows.iter().map(|(o, _)| o.as_str()).collect::<Vec<_>>(),
        ["denied", "denied", "denied", "denied", "success", "success"]
    );
    assert!(
        rows[0].1.contains("platform-super-admin") || rows[0].1.contains("Platform Super Admin"),
        "{rows:?}"
    );
}

/// An approval is another person's judgement: a legacy token is not a person (it is recorded as a
/// different principal from whoever holds it), so it may neither approve nor reject.
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn only_a_person_decides_an_approval(pool: Pool) {
    let l = lab(pool.clone());
    let w = world(&l).await;
    let (legacy, _) = apelora_persistence::create_token(&pool, "ops", "fabric-admin", None)
        .await
        .expect("token");
    let ask = json!({ "action": "token.create_admin", "target": "token:fabric-admin", "reason": "break-glass token for the runbook" });
    for verb in ["approve", "reject"] {
        let (s, ap) = l
            .req("POST", "/api/v1/approvals", &w.admin, Some(ask.clone()))
            .await;
        assert_eq!(s, StatusCode::CREATED, "{ap}");
        let id = ap["id"].as_str().expect("id").to_owned();
        let (s, b) = l
            .req(
                "POST",
                &format!("/api/v1/approvals/{id}/{verb}"),
                &legacy,
                None,
            )
            .await;
        assert_eq!(
            (s, code(&b)),
            (StatusCode::FORBIDDEN, Some("approval_needs_a_person")),
            "{verb}: {b}"
        );
    }
    let pending: i64 = sqlx::query_scalar("SELECT count(*) FROM approvals WHERE state = 'pending'")
        .fetch_one(&pool)
        .await
        .expect("count");
    assert_eq!(pending, 2, "both requests are still waiting for a person");
    let denied: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_events WHERE outcome = 'denied' AND actor = 'token:ops' AND action IN ('approval.grant', 'approval.reject') ORDER BY id",
    )
    .fetch_all(&pool)
    .await
    .expect("audit");
    assert_eq!(denied, ["approval.grant", "approval.reject"]);
}
