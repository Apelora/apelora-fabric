// SPDX-License-Identifier: AGPL-3.0-only
//! Audit integrity across the whole API (ADR 0011, EVERY-SIZE.md §2.5, gap S4).
//!
//! One story drives every state-changing route of the OpenAPI document through the real router,
//! against PostgreSQL and the mock Incus. For each route it checks that:
//!
//! 1. the route writes its audit row (a `success` row with the expected action), and
//! 2. where the change is Apelora's own state, the change and its audit row are written in one
//!    transaction: with the audit table made unwritable, the request fails and the database is
//!    exactly as before; once the table is writable again, the same request succeeds.
//!
//! A route the story does not exercise must be listed in [`EXEMPT`] with the reason. A new
//! route that is in neither fails `every_state_changing_route_audits_its_change_in_the_same_transaction`.

use std::collections::{BTreeMap, BTreeSet};

use apelora_incus_adapter::MockIncus;
use axum::body::Body;
use axum::http::Request;
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::manage_tests::{mock_lab, settle};
use super::*;

/// State-changing routes this test does not drive, and why. Keep the reason specific: a route
/// belongs here only when it changes nothing in Apelora's database, or when another test
/// exercises it together with its audit row.
const EXEMPT: &[(&str, &str, &str)] = &[
    (
        "POST",
        "/api/v1/cluster/upgrade",
        "exercised with its audit row in clustering_tests::an_upgrade_walks_evacuate_package_and_restore; evacuate and restore change Incus before the row",
    ),
    (
        "POST",
        "/api/v1/provisioners/maas/machines/{system_id}/commission",
        "MAAS is asked first; the audit row is checked in provisioners_tests::commissioning_and_deploy_are_audited_without_the_cloud_init",
    ),
    (
        "POST",
        "/api/v1/provisioners/maas/machines/{system_id}/deploy",
        "MAAS is asked first and the cloud-init is not stored; the audit row is checked in provisioners_tests::commissioning_and_deploy_are_audited_without_the_cloud_init",
    ),
    (
        "POST",
        "/api/v1/provisioners/maas/machines/{system_id}/release",
        "MAAS is asked first; the audit row is checked in provisioners_tests::commissioning_and_deploy_are_audited_without_the_cloud_init",
    ),
    (
        "POST",
        "/api/v1/provisioners/maas/machines/{system_id}/test",
        "MAAS is asked first; the audit row is checked in provisioners_tests::a_burn_in_runs_only_on_a_machine_not_in_use_and_is_audited",
    ),
    // Dry runs and checks: they compute or probe, and change nothing.
    (
        "POST",
        "/api/v1/applications/{name}/plan",
        "dry run: recomputes the deployment plan and changes nothing",
    ),
    (
        "POST",
        "/api/v1/identity/assignments/preview",
        "dry run: previews an assignment with the same checks as saving it, changes nothing",
    ),
    (
        "POST",
        "/api/v1/identity/sign-in/check",
        "reads a provider's discovery document and key set; changes nothing",
    ),
    (
        "POST",
        "/api/v1/identity/sign-in/directories/check",
        "binds once to test a directory; changes nothing",
    ),
    // Terminal sessions live in the control plane's memory, not in the database; each is audited
    // before it takes effect. They need a live websocket rig, so sessions_tests exercises them.
    (
        "POST",
        "/api/v1/applications/{name}/instances/{instance}/sessions",
        "issues a single-use ticket held in memory, discarded when its audit row (session.request) cannot be written; sessions_tests::a_ticket_opens_the_labelled_mock_shell_once_and_the_session_is_audited_without_content",
    ),
    (
        "DELETE",
        "/api/v1/sessions/{id}",
        "ends a session held in memory after its audit row (session.terminate) is written; sessions_tests::sessions_end_when_idle_when_too_long_when_terminated_and_when_access_is_revoked",
    ),
];

/// How a route's change relates to its audit row.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// The change is Apelora's own state: it and its audit row commit or roll back together.
    Atomic,
    /// The change is made in Incus first (or cannot be repeated after a rolled-back attempt);
    /// only checked for its audit row.
    Audited,
}

struct Story {
    l: super::identity_tests::Lab,
    mock: MockIncus,
    root: String,
    covered: BTreeSet<(String, String)>,
}

const PASSWORD: &str = "correct horse battery";
const NEW_PASSWORD: &str = "another horse battery staple";

async fn call(
    state: &AppState,
    method: &str,
    path: &str,
    token: Option<&str>,
    body: Option<&Value>,
) -> (StatusCode, Value) {
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
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// Every row of every table except the audit trail and the outbox, without the columns that
/// reading or authenticating touches (`last_seen_at`, `last_used_at`, `sign_in_audited_at`).
async fn snapshot(pool: &Pool) -> BTreeMap<String, String> {
    let tables: Vec<String> = sqlx::query_scalar(
        "SELECT tablename::text FROM pg_tables WHERE schemaname = 'public'
         AND tablename NOT IN ('audit_events', 'outbox', '_sqlx_migrations') ORDER BY 1",
    )
    .fetch_all(pool)
    .await
    .expect("tables");
    let mut out = BTreeMap::new();
    for t in tables {
        let rows: String = sqlx::query_scalar(&format!(
            "SELECT coalesce(string_agg(r, E'\\n' ORDER BY r), '') FROM (
                SELECT (to_jsonb(t) - 'last_seen_at' - 'last_used_at' - 'sign_in_audited_at')::text AS r
                FROM \"{t}\" t) x"
        ))
        .fetch_one(pool)
        .await
        .expect("rows");
        out.insert(t, rows);
    }
    out
}

async fn set_audit_writable(pool: &Pool, writable: bool) {
    let sql = if writable {
        "DROP TRIGGER test_audit_unwritable ON audit_events"
    } else {
        "CREATE TRIGGER test_audit_unwritable BEFORE INSERT ON audit_events
         FOR EACH ROW EXECUTE FUNCTION test_audit_unwritable()"
    };
    sqlx::query(sql).execute(pool).await.expect("trigger");
}

impl Story {
    async fn new(pool: Pool) -> Story {
        let (mut l, mock) = mock_lab(pool);
        l.state = l
            .state
            .clone()
            .with_volume_pool(Some("mock-local".into()))
            .with_backup_pool(Some("mock-backup".into()));
        l.state.sign_in_attempts = std::sync::Arc::new(sessions_auth::Attempts::new(
            1000,
            std::time::Duration::from_secs(60),
        ));
        sqlx::query(
            "CREATE FUNCTION test_audit_unwritable() RETURNS trigger LANGUAGE plpgsql AS $$
             BEGIN RAISE EXCEPTION 'the audit table is not writable (test)'; END $$",
        )
        .execute(&l.pool)
        .await
        .expect("function");
        let (_, root) = l.super_admin("sub-root", "root").await;
        Story {
            l,
            mock,
            root,
            covered: BTreeSet::new(),
        }
    }

    /// Sends a request that must succeed, without counting it as the route's check (setup).
    async fn setup(&self, method: &str, path: &str, token: Option<&str>, body: Value) -> Value {
        let body = (!body.is_null()).then_some(body);
        let (status, v) = call(&self.l.state, method, path, token, body.as_ref()).await;
        assert!(status.is_success(), "setup {method} {path}: {status} {v}");
        v
    }

    /// The route's check (see the module documentation). Returns the successful response.
    #[allow(clippy::too_many_arguments)]
    async fn step(
        &mut self,
        mode: Mode,
        method: &str,
        template: &str,
        path: &str,
        token: Option<&str>,
        body: Value,
        action: &str,
    ) -> Value {
        let body = (!body.is_null()).then_some(body);
        let pool = self.l.pool.clone();
        if mode == Mode::Atomic {
            let before = snapshot(&pool).await;
            set_audit_writable(&pool, false).await;
            let (status, v) = call(&self.l.state, method, path, token, body.as_ref()).await;
            set_audit_writable(&pool, true).await;
            assert!(
                status.is_server_error(),
                "{method} {template}: answered {status} although its audit row could not be written: {v}"
            );
            let after = snapshot(&pool).await;
            for (table, rows) in &before {
                assert_eq!(
                    rows, &after[table],
                    "{method} {template} changed {table} although its audit row could not be written"
                );
            }
        }
        let last: i64 = sqlx::query_scalar("SELECT coalesce(max(id), 0) FROM audit_events")
            .fetch_one(&pool)
            .await
            .expect("last");
        let (status, v) = call(&self.l.state, method, path, token, body.as_ref()).await;
        assert!(status.is_success(), "{method} {template}: {status} {v}");
        let audited: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM audit_events WHERE id > $1 AND action = $2 AND outcome = 'success'",
        )
        .bind(last)
        .bind(action)
        .fetch_one(&pool)
        .await
        .expect("audited");
        assert!(
            audited >= 1,
            "{method} {template} succeeded without writing its audit row ({action})"
        );
        self.covered
            .insert((method.to_owned(), template.to_owned()));
        v
    }
}

fn id(v: &Value, key: &str) -> String {
    v[key]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| panic!("no {key} in {v}"))
}

fn document() -> Value {
    json!({ "apiVersion": "fabric.apelora/v1alpha1", "kind": "ApplicationAssembly", "metadata": { "name": "x", "uid": "u" },
        "spec": { "modules": [
            { "id": "web-01", "ports": [{ "id": "http", "direction": "input" }, { "id": "db", "direction": "output" }] },
            { "id": "db-01", "ports": [{ "id": "sql", "direction": "input" }] } ],
          "connections": [{ "id": "conn-01", "source": { "moduleId": "web-01", "portId": "db" }, "target": { "moduleId": "db-01", "portId": "sql" } }] },
        "editor": { "layoutVersion": 1, "density": "compact", "nodes": [] } })
}

fn spec(id: &str, availability: &str, volume: bool) -> Value {
    let mut m = super::tests::module(id);
    m["availability"] = json!(availability);
    if volume {
        m["spec"]["volumes"] = json!([{ "name": "data", "sizeMib": 1024, "path": "/data" }]);
    }
    m
}

/// The state-changing operations of the OpenAPI document, as `(METHOD, path)`.
fn state_changing_routes() -> BTreeSet<(String, String)> {
    let doc: Value = serde_json::from_str(&openapi_json()).expect("openapi");
    let mut out = BTreeSet::new();
    for (path, item) in doc["paths"].as_object().expect("paths") {
        for method in ["post", "put", "patch", "delete"] {
            if item.get(method).is_some() {
                out.insert((method.to_uppercase(), path.clone()));
            }
        }
    }
    out
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn every_state_changing_route_audits_its_change_in_the_same_transaction(pool: Pool) {
    use Mode::{Atomic, Audited};
    let mut s = Story::new(pool).await;
    let root = s.root.clone();
    let r = Some(root.as_str());

    // ---- tenancy ----
    let org = s
        .step(
            Atomic,
            "POST",
            "/api/v1/identity/organizations",
            "/api/v1/identity/organizations",
            r,
            json!({ "slug": "acme", "name": "Acme" }),
            "organization.create",
        )
        .await;
    let org = id(&org, "id");
    let project = s
        .step(
            Atomic,
            "POST",
            "/api/v1/identity/organizations/{org}/projects",
            &format!("/api/v1/identity/organizations/{org}/projects"),
            r,
            json!({ "slug": "shop", "name": "Shop" }),
            "project.create",
        )
        .await;
    let project = id(&project, "id");
    let env = s
        .step(
            Atomic,
            "POST",
            "/api/v1/identity/projects/{project}/environments",
            &format!("/api/v1/identity/projects/{project}/environments"),
            r,
            json!({ "slug": "prod", "name": "Production", "kind": "production" }),
            "environment.create",
        )
        .await;
    let (env, incus_project) = (id(&env, "id"), id(&env, "incusProject"));
    // Preparing the environment's Incus project again (Incus first, then its state and row).
    s.step(
        Atomic,
        "POST",
        "/api/v1/identity/environments/{id}/incus-project",
        &format!("/api/v1/identity/environments/{env}/incus-project"),
        r,
        Value::Null,
        "environment.incus_project.prepare",
    )
    .await;
    // A generous quota: the deployments further down are checked against it.
    s.step(
        Atomic,
        "PUT",
        "/api/v1/identity/quotas/{scope}/{id}",
        &format!("/api/v1/identity/quotas/project/{project}"),
        r,
        json!({ "instances": 100, "cpu": 400, "memoryGib": 1000, "diskGib": 10000 }),
        "quota.set",
    )
    .await;

    // ---- people, groups, roles, assignments, emergency access ----
    let bob = s.l.user("sub-bob", "bob").await;
    let bob_token = s.l.token("sub-bob", "bob");
    let group = s
        .step(
            Atomic,
            "POST",
            "/api/v1/identity/groups",
            "/api/v1/identity/groups",
            r,
            json!({ "organizationId": org, "name": "Operators" }),
            "group.create",
        )
        .await;
    let group = id(&group, "id");
    let member = format!("/api/v1/identity/groups/{group}/members/{bob}");
    s.step(
        Atomic,
        "PUT",
        "/api/v1/identity/groups/{group}/members/{user}",
        &member,
        r,
        Value::Null,
        "group.member.add",
    )
    .await;
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/identity/groups/{group}/members/{user}",
        &member,
        r,
        Value::Null,
        "group.member.remove",
    )
    .await;
    let platform_group = id(
        &s.setup(
            "POST",
            "/api/v1/identity/groups",
            r,
            json!({ "name": "Platform operators" }),
        )
        .await,
        "id",
    );
    s.step(
        Atomic,
        "PUT",
        "/api/v1/identity/groups/{group}/external",
        &format!("/api/v1/identity/groups/{platform_group}/external"),
        r,
        json!({ "source": "https://id.example.test/realms/apelora", "name": "platform-operators" }),
        "group.external.set",
    )
    .await;
    let role = s
        .step(
            Atomic,
            "POST",
            "/api/v1/identity/roles",
            "/api/v1/identity/roles",
            r,
            json!({ "name": "Audit readers", "permissions": ["audit.read"] }),
            "role.create",
        )
        .await;
    let role = id(&role, "id").trim_start_matches("custom:").to_owned();
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/identity/roles/{role}",
        &format!("/api/v1/identity/roles/{role}"),
        r,
        Value::Null,
        "role.delete",
    )
    .await;
    let assignment = s
        .step(Atomic, "POST", "/api/v1/identity/assignments", "/api/v1/identity/assignments", r,
            json!({ "principal": { "type": "user", "id": bob }, "role": { "type": "builtin", "key": "viewer" }, "scope": { "type": "organization", "id": org } }),
            "role.assign")
        .await;
    let assignment = id(&assignment, "id");
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/identity/assignments/{assignment}",
        &format!("/api/v1/identity/assignments/{assignment}"),
        r,
        Value::Null,
        "role.revoke",
    )
    .await;
    s.step(Atomic, "POST", "/api/v1/identity/emergency-access", "/api/v1/identity/emergency-access", r,
        json!({ "userId": bob, "role": "platform-super-admin", "reason": "incident 42: restore the service", "minutes": 30 }),
        "emergency_access.grant").await;
    s.step(
        Atomic,
        "POST",
        "/api/v1/identity/users/{user}/disable",
        &format!("/api/v1/identity/users/{bob}/disable"),
        r,
        Value::Null,
        "user.disable",
    )
    .await;
    s.step(
        Atomic,
        "POST",
        "/api/v1/identity/users/{user}/enable",
        &format!("/api/v1/identity/users/{bob}/enable"),
        r,
        Value::Null,
        "user.enable",
    )
    .await;

    // ---- service identities and policies ----
    let svc = s
        .step(
            Atomic,
            "POST",
            "/api/v1/identity/service-identities",
            "/api/v1/identity/service-identities",
            r,
            json!({ "name": "ci-bot", "kind": "automation", "expiresInDays": 30 }),
            "service_identity.create",
        )
        .await;
    let svc = id(&svc["identity"], "id");
    s.step(
        Atomic,
        "POST",
        "/api/v1/identity/service-identities/{id}/disable",
        &format!("/api/v1/identity/service-identities/{svc}/disable"),
        r,
        Value::Null,
        "service_identity.disable",
    )
    .await;
    s.step(
        Atomic,
        "POST",
        "/api/v1/identity/service-identities/{id}/enable",
        &format!("/api/v1/identity/service-identities/{svc}/enable"),
        r,
        Value::Null,
        "service_identity.enable",
    )
    .await;
    let policy = s
        .step(Atomic, "PUT", "/api/v1/identity/policies", "/api/v1/identity/policies", r,
            json!({ "name": "mfa-production", "kind": "mfa", "parameters": { "environments": ["production"] }, "enabled": false }),
            "access_policy.save")
        .await;
    let policy = id(&policy, "id");
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/identity/policies/{id}",
        &format!("/api/v1/identity/policies/{policy}"),
        r,
        Value::Null,
        "access_policy.delete",
    )
    .await;

    // ---- approvals (bob has emergency access, so he may decide) ----
    let ask = json!({ "action": "token.create_admin", "target": "token:fabric-admin", "reason": "a token for the pipeline" });
    let a1 = s
        .step(
            Atomic,
            "POST",
            "/api/v1/approvals",
            "/api/v1/approvals",
            r,
            ask.clone(),
            "approval.request",
        )
        .await;
    let a1 = id(&a1, "id");
    s.step(
        Atomic,
        "POST",
        "/api/v1/approvals/{id}/cancel",
        &format!("/api/v1/approvals/{a1}/cancel"),
        r,
        Value::Null,
        "approval.cancel",
    )
    .await;
    let a2 = id(
        &s.setup("POST", "/api/v1/approvals", r, ask.clone()).await,
        "id",
    );
    s.step(
        Atomic,
        "POST",
        "/api/v1/approvals/{id}/approve",
        &format!("/api/v1/approvals/{a2}/approve"),
        Some(&bob_token),
        Value::Null,
        "approval.grant",
    )
    .await;
    let a3 = id(&s.setup("POST", "/api/v1/approvals", r, ask).await, "id");
    s.step(
        Atomic,
        "POST",
        "/api/v1/approvals/{id}/reject",
        &format!("/api/v1/approvals/{a3}/reject"),
        Some(&bob_token),
        Value::Null,
        "approval.reject",
    )
    .await;

    // ---- sign-in settings ----
    s.step(
        Atomic,
        "PUT",
        "/api/v1/identity/sign-in/api-tokens",
        "/api/v1/identity/sign-in/api-tokens",
        r,
        json!({ "enabled": true }),
        "identity.api_tokens.enabled",
    )
    .await;
    let provider = json!({ "name": "Second IdP", "issuer": "https://127.0.0.1:9/realms/second", "audiences": ["apelora-api"], "clientId": "apelora-console" });
    let p = s
        .step(
            Atomic,
            "POST",
            "/api/v1/identity/sign-in/providers",
            "/api/v1/identity/sign-in/providers",
            r,
            provider.clone(),
            "identity.provider.saved",
        )
        .await;
    let p = id(&p, "id");
    let mut renamed = provider;
    renamed["name"] = json!("Second IdP (renamed)");
    s.step(
        Atomic,
        "PUT",
        "/api/v1/identity/sign-in/providers/{id}",
        &format!("/api/v1/identity/sign-in/providers/{p}"),
        r,
        renamed,
        "identity.provider.saved",
    )
    .await;
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/identity/sign-in/providers/{id}",
        &format!("/api/v1/identity/sign-in/providers/{p}"),
        r,
        json!({ "confirm": "Second IdP (renamed)" }),
        "identity.provider.deleted",
    )
    .await;
    let directory = json!({ "name": "Lab directory", "url": "ldaps://127.0.0.1:9", "bindTemplate": "uid={username},ou=people,dc=apelora,dc=test" });
    let d = s
        .step(
            Atomic,
            "POST",
            "/api/v1/identity/sign-in/directories",
            "/api/v1/identity/sign-in/directories",
            r,
            directory.clone(),
            "identity.directory.saved",
        )
        .await;
    let d = id(&d, "id");
    let mut renamed = directory;
    renamed["name"] = json!("Lab directory 2");
    s.step(
        Atomic,
        "PUT",
        "/api/v1/identity/sign-in/directories/{id}",
        &format!("/api/v1/identity/sign-in/directories/{d}"),
        r,
        renamed,
        "identity.directory.saved",
    )
    .await;
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/identity/sign-in/directories/{id}",
        &format!("/api/v1/identity/sign-in/directories/{d}"),
        r,
        json!({ "confirm": "Lab directory 2" }),
        "identity.directory.deleted",
    )
    .await;

    // ---- local accounts, password sign-in, sessions ----
    let carol = s
        .step(
            Atomic,
            "POST",
            "/api/v1/identity/local-accounts",
            "/api/v1/identity/local-accounts",
            r,
            json!({ "username": "carol" }),
            "identity.local_account.created",
        )
        .await;
    let (carol_id, setup) = (id(&carol["user"], "id"), id(&carol, "setupToken"));
    s.step(
        Atomic,
        "POST",
        "/api/v1/sign-in/setup",
        "/api/v1/sign-in/setup",
        None,
        json!({ "token": setup, "password": PASSWORD }),
        "auth.setup.completed",
    )
    .await;
    s.step(
        Atomic,
        "PUT",
        "/api/v1/identity/sign-in/local-accounts",
        "/api/v1/identity/sign-in/local-accounts",
        r,
        json!({ "enabled": true }),
        "identity.local_accounts.enabled",
    )
    .await;
    let session = s
        .step(
            Atomic,
            "POST",
            "/api/v1/sign-in/password",
            "/api/v1/sign-in/password",
            None,
            json!({ "username": "carol", "password": PASSWORD }),
            "auth.password.succeeded",
        )
        .await;
    let carol_token = id(&session, "token");
    s.step(
        Atomic,
        "PUT",
        "/api/v1/me/password",
        "/api/v1/me/password",
        Some(&carol_token),
        json!({ "current": PASSWORD, "new": NEW_PASSWORD }),
        "auth.password.changed",
    )
    .await;
    s.step(
        Atomic,
        "POST",
        "/api/v1/sign-out",
        "/api/v1/sign-out",
        Some(&carol_token),
        Value::Null,
        "auth.signed_out",
    )
    .await;
    s.step(
        Atomic,
        "POST",
        "/api/v1/identity/local-accounts/{id}/reset",
        &format!("/api/v1/identity/local-accounts/{carol_id}/reset"),
        r,
        Value::Null,
        "identity.local_account.reset",
    )
    .await;

    // ---- API tokens ----
    let token = s
        .step(
            Atomic,
            "POST",
            "/api/v1/tokens",
            "/api/v1/tokens",
            r,
            json!({ "name": "pipeline", "role": "operator", "expiresInDays": 30 }),
            "token.create",
        )
        .await;
    let token = id(&token["info"], "id");
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/tokens/{id}",
        &format!("/api/v1/tokens/{token}"),
        r,
        Value::Null,
        "token.revoke",
    )
    .await;

    // ---- personal tokens and sign-ins ----
    let mine = s
        .step(
            Atomic,
            "POST",
            "/api/v1/me/tokens",
            "/api/v1/me/tokens",
            r,
            json!({ "name": "laptop", "expiresInDays": 30 }),
            "personal_token.create",
        )
        .await;
    let mine = id(&mine["info"], "id");
    let rotated = s
        .step(
            Atomic,
            "POST",
            "/api/v1/me/tokens/{id}/rotate",
            &format!("/api/v1/me/tokens/{mine}/rotate"),
            r,
            json!({ "overlapMinutes": 10 }),
            "personal_token.rotate",
        )
        .await;
    let successor = id(&rotated["info"], "id");
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/me/tokens/{id}",
        &format!("/api/v1/me/tokens/{mine}"),
        r,
        Value::Null,
        "personal_token.revoke",
    )
    .await;
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/identity/personal-tokens/{id}",
        &format!("/api/v1/identity/personal-tokens/{successor}"),
        r,
        Value::Null,
        "personal_token.revoke",
    )
    .await;
    let dora = s
        .setup(
            "POST",
            "/api/v1/identity/local-accounts",
            r,
            json!({ "username": "dora" }),
        )
        .await;
    s.setup(
        "POST",
        "/api/v1/sign-in/setup",
        None,
        json!({ "token": id(&dora, "setupToken"), "password": PASSWORD }),
    )
    .await;
    let sign_in = json!({ "username": "dora", "password": PASSWORD });
    let dora_here = id(
        &s.setup("POST", "/api/v1/sign-in/password", None, sign_in.clone())
            .await,
        "token",
    );
    s.setup("POST", "/api/v1/sign-in/password", None, sign_in.clone())
        .await;
    s.setup("POST", "/api/v1/sign-in/password", None, sign_in)
        .await;
    let sessions = s
        .setup("GET", "/api/v1/me/sign-ins", Some(&dora_here), Value::Null)
        .await;
    let others: Vec<String> = sessions["items"]
        .as_array()
        .expect("items")
        .iter()
        .filter(|i| i["current"] == false)
        .map(|i| id(i, "id"))
        .collect();
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/me/sign-ins/{id}",
        &format!("/api/v1/me/sign-ins/{}", others[0]),
        Some(&dora_here),
        Value::Null,
        "auth.session.ended",
    )
    .await;
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/identity/sign-ins/{id}",
        &format!("/api/v1/identity/sign-ins/{}", others[1]),
        r,
        Value::Null,
        "auth.session.ended",
    )
    .await;

    // ---- applications: design, deployment, operations, volumes ----
    s.step(
        Atomic,
        "POST",
        "/api/v1/applications",
        "/api/v1/applications",
        r,
        json!({ "name": "shop", "environmentId": env, "icon": "nginx", "document": document() }),
        "application.create",
    )
    .await;
    s.step(
        Atomic,
        "PATCH",
        "/api/v1/applications/{name}",
        "/api/v1/applications/shop",
        r,
        json!({ "description": "The web shop" }),
        "application.update",
    )
    .await;
    s.step(
        Atomic,
        "PUT",
        "/api/v1/applications/{name}/draft",
        "/api/v1/applications/shop/draft",
        r,
        json!({ "document": document(), "baseVersion": 1 }),
        "application.draft.save",
    )
    .await;
    let v = s
        .step(
            Atomic,
            "POST",
            "/api/v1/applications/{name}/versions",
            "/api/v1/applications/shop/versions",
            r,
            json!({ "label": "first" }),
            "application.version.publish",
        )
        .await;
    let revision = v["revision"].as_i64().expect("revision");
    s.step(
        Atomic,
        "POST",
        "/api/v1/applications/{name}/deployments",
        "/api/v1/applications/shop/deployments",
        r,
        json!({ "project": incus_project, "document": {}, "version": revision,
                "modules": [spec("web-01", "stateless", false), spec("db-01", "stateful", true)] }),
        "application.deploy.requested",
    )
    .await;
    settle(&s.l).await;
    s.step(
        Atomic,
        "POST",
        "/api/v1/applications/{name}/operations",
        "/api/v1/applications/shop/operations",
        r,
        json!({ "action": "stop", "confirmDisruption": true }),
        "application.stop.requested",
    )
    .await;
    settle(&s.l).await;
    s.step(
        Atomic,
        "POST",
        "/api/v1/applications/{name}/archive",
        "/api/v1/applications/shop/archive",
        r,
        Value::Null,
        "application.archive",
    )
    .await;
    s.step(
        Atomic,
        "POST",
        "/api/v1/applications/{name}/restore",
        "/api/v1/applications/shop/restore",
        r,
        Value::Null,
        "application.restore",
    )
    .await;

    let volume = "/api/v1/applications/shop/volumes/shop-db-01-data";
    s.step(
        Atomic,
        "PATCH",
        "/api/v1/applications/{name}/volumes/{volume}",
        volume,
        r,
        json!({ "sizeMib": 2048 }),
        "application.volume.grow",
    )
    .await;
    let snap = s
        .step(
            Audited,
            "POST",
            "/api/v1/applications/{name}/volumes/{volume}/snapshots",
            &format!("{volume}/snapshots"),
            r,
            Value::Null,
            "application.volume.snapshot",
        )
        .await;
    let snap = id(&snap, "name");
    s.step(
        Audited,
        "POST",
        "/api/v1/applications/{name}/volumes/{volume}/snapshots/{snapshot}/restore",
        &format!("{volume}/snapshots/{snap}/restore"),
        r,
        json!({ "confirm": snap }),
        "application.volume.restore",
    )
    .await;
    s.step(
        Audited,
        "DELETE",
        "/api/v1/applications/{name}/volumes/{volume}/snapshots/{snapshot}",
        &format!("{volume}/snapshots/{snap}"),
        r,
        Value::Null,
        "application.volume.snapshot.delete",
    )
    .await;
    // Schedules, a backup on request and a restore from it: each queues or changes Apelora's
    // records together with its audit row.
    s.step(
        Atomic,
        "PUT",
        "/api/v1/applications/{name}/volumes/{volume}/schedules",
        &format!("{volume}/schedules"),
        r,
        json!({ "snapshots": { "everyMinutes": 60, "keep": 3 }, "backups": null }),
        "application.volume.schedules",
    )
    .await;
    s.step(
        Atomic,
        "POST",
        "/api/v1/applications/{name}/volumes/{volume}/backups",
        &format!("{volume}/backups"),
        r,
        Value::Null,
        "application.volume.backup.requested",
    )
    .await;
    settle(&s.l).await;
    let backup: String =
        sqlx::query_scalar("SELECT name FROM volume_backups ORDER BY created_at LIMIT 1")
            .fetch_one(&s.l.pool)
            .await
            .expect("a backup was made");
    s.step(
        Atomic,
        "POST",
        "/api/v1/applications/{name}/volumes/{volume}/backups/{backup}/restore",
        &format!("{volume}/backups/{backup}/restore"),
        r,
        json!({ "confirm": "shop-db-01-data" }),
        "application.volume.restore.requested",
    )
    .await;
    settle(&s.l).await;
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/applications/{name}/volumes/{volume}/backups/{backup}",
        &format!("{volume}/backups/{backup}"),
        r,
        json!({ "confirm": backup }),
        "application.volume.backup.delete",
    )
    .await;
    s.step(
        Atomic,
        "POST",
        "/api/v1/instances/{project}/{name}/move",
        &format!("/api/v1/instances/{incus_project}/shop-db-01/move"),
        r,
        json!({ "target": "mock-member-02", "confirm": "shop-db-01", "allowStop": true }),
        "instance.move.requested",
    )
    .await;
    settle(&s.l).await;
    // Remove the database module (its volume is kept), then delete the volume on purpose.
    s.setup("POST", "/api/v1/applications/shop/deployments", r,
        json!({ "project": incus_project, "document": {}, "removeMissing": true, "modules": [spec("web-01", "stateless", false)] })).await;
    settle(&s.l).await;
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/applications/{name}/volumes/{volume}",
        volume,
        r,
        json!({ "confirm": "shop-db-01-data" }),
        "application.volume.delete",
    )
    .await;

    // ---- instances outside applications ----
    let mut solo = super::tests::module("solo-01")["spec"].clone();
    solo["project"] = json!("apelora");
    solo["name"] = json!("solo-01");
    s.step(
        Atomic,
        "POST",
        "/api/v1/instances",
        "/api/v1/instances",
        r,
        solo,
        "instance.create.requested",
    )
    .await;
    s.step(
        Atomic,
        "POST",
        "/api/v1/instances/{project}/{name}/state",
        "/api/v1/instances/apelora/solo-01/state",
        r,
        json!({ "action": "stop" }),
        "instance.stop.requested",
    )
    .await;
    let deleting = s
        .step(
            Atomic,
            "DELETE",
            "/api/v1/instances/{project}/{name}",
            "/api/v1/instances/apelora/solo-01",
            r,
            Value::Null,
            "instance.delete.requested",
        )
        .await;

    // ---- jobs: retry a failed one, cancel the waiting retry ----
    let failed = id(&deleting, "jobId");
    sqlx::query("UPDATE jobs SET status = 'failed', last_error = 'test' WHERE id = $1::uuid")
        .bind(&failed)
        .execute(&s.l.pool)
        .await
        .expect("fail the job");
    let retried = s
        .step(
            Atomic,
            "POST",
            "/api/v1/jobs/{id}/retry",
            &format!("/api/v1/jobs/{failed}/retry"),
            r,
            Value::Null,
            "job.retry",
        )
        .await;
    s.step(
        Atomic,
        "POST",
        "/api/v1/jobs/{id}/cancel",
        &format!("/api/v1/jobs/{}/cancel", id(&retried, "jobId")),
        r,
        Value::Null,
        "job.cancel",
    )
    .await;

    // ---- cluster membership (Incus's state) and machines ----
    // A legacy fabric-admin token: legacy tokens are exempt from approvals, which people need
    // for these sensitive changes (an approval is spent before the change; see EXEMPT's notes).
    let admin = apelora_persistence::create_token(&s.l.pool, "admin", "fabric-admin", None)
        .await
        .expect("token")
        .0;
    let r = Some(admin.as_str());
    s.step(
        Audited,
        "POST",
        "/api/v1/cluster/join-tokens",
        "/api/v1/cluster/join-tokens",
        r,
        json!({ "memberName": "mock-member-09" }),
        "cluster.join-token.created",
    )
    .await;
    s.step(
        Audited,
        "DELETE",
        "/api/v1/cluster/join-tokens/{member}",
        "/api/v1/cluster/join-tokens/mock-member-09",
        r,
        Value::Null,
        "cluster.join-token.revoked",
    )
    .await;
    s.setup(
        "POST",
        "/api/v1/cluster/join-tokens",
        r,
        json!({ "memberName": "mock-member-09" }),
    )
    .await;
    assert!(s.mock.join("mock-member-09", false), "joined");
    // Cluster settings: the change and its audit row (a legacy admin token needs no approval).
    s.step(
        Audited,
        "PATCH",
        "/api/v1/cluster/settings",
        "/api/v1/cluster/settings",
        r,
        json!({ "changes": { "cluster.max_standby": "1" } }),
        "cluster.settings.change",
    )
    .await;
    let m9 = "/api/v1/cluster/members/mock-member-09";
    let confirm = json!({ "confirm": "mock-member-09" });
    s.step(
        Audited,
        "POST",
        "/api/v1/cluster/members/{member}/evacuate",
        &format!("{m9}/evacuate"),
        r,
        confirm.clone(),
        "cluster.member.evacuate",
    )
    .await;
    s.step(
        Audited,
        "POST",
        "/api/v1/cluster/members/{member}/restore",
        &format!("{m9}/restore"),
        r,
        confirm.clone(),
        "cluster.member.restore",
    )
    .await;
    s.setup("POST", &format!("{m9}/evacuate"), r, confirm.clone())
        .await;
    s.step(
        Audited,
        "POST",
        "/api/v1/cluster/members/{member}/remove",
        &format!("{m9}/remove"),
        r,
        confirm,
        "cluster.member.remove",
    )
    .await;
    // A person confirms a failed server is off (HA slice B): recorded with its audit row.
    s.mock
        .members
        .lock()
        .expect("lock")
        .iter_mut()
        .find(|m| m.name == "mock-member-02")
        .expect("member")
        .status = "Offline".into();
    s.step(
        Atomic,
        "POST",
        "/api/v1/cluster/members/{member}/fencing",
        "/api/v1/cluster/members/mock-member-02/fencing",
        r,
        json!({ "confirm": "mock-member-02", "poweredOff": true }),
        "cluster.member.fence.confirmed",
    )
    .await;
    s.mock
        .members
        .lock()
        .expect("lock")
        .iter_mut()
        .find(|m| m.name == "mock-member-02")
        .expect("member")
        .status = "Online".into();

    s.step(
        Atomic,
        "POST",
        "/api/v1/machines",
        "/api/v1/machines",
        r,
        json!({ "name": "rack1-07" }),
        "machine.register",
    )
    .await;
    s.step(
        Audited,
        "POST",
        "/api/v1/machines/{name}/approve",
        "/api/v1/machines/rack1-07/approve",
        r,
        Value::Null,
        "machine.approve",
    )
    .await;
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/machines/{name}/approval",
        "/api/v1/machines/rack1-07/approval",
        r,
        Value::Null,
        "machine.approval.withdraw",
    )
    .await;
    s.setup("POST", "/api/v1/machines/rack1-07/approve", r, Value::Null)
        .await;
    assert!(s.mock.join("rack1-07", true), "joined, held");
    s.step(
        Audited,
        "POST",
        "/api/v1/machines/{name}/admit",
        "/api/v1/machines/rack1-07/admit",
        r,
        Value::Null,
        "machine.admit",
    )
    .await;
    s.setup("POST", "/api/v1/machines", r, json!({ "name": "rack1-08" }))
        .await;
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/machines/{name}",
        "/api/v1/machines/rack1-08",
        r,
        Value::Null,
        "machine.remove",
    )
    .await;

    // ---- alerts (alerts.rs): every change is Apelora's own state ----
    super::alerts_tests::fire_an_alert(&s.l.pool).await;
    let alert = s.setup("GET", "/api/v1/alerts", r, Value::Null).await["alerts"][0]["id"]
        .as_i64()
        .expect("a firing alert");
    s.step(
        Atomic,
        "POST",
        "/api/v1/alerts/{id}/acknowledge",
        &format!("/api/v1/alerts/{alert}/acknowledge"),
        r,
        json!({ "note": "on it" }),
        "alert.acknowledge",
    )
    .await;
    let silence = s
        .step(
            Atomic,
            "POST",
            "/api/v1/alerts/silences",
            "/api/v1/alerts/silences",
            r,
            json!({ "rule": "job-failures", "minutes": 30, "note": "maintenance" }),
            "alert.silence.create",
        )
        .await;
    let silence = id(&silence, "id");
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/alerts/silences/{id}",
        &format!("/api/v1/alerts/silences/{silence}"),
        r,
        Value::Null,
        "alert.silence.end",
    )
    .await;
    s.step(
        Atomic,
        "PUT",
        "/api/v1/alerts/rules/{rule}",
        "/api/v1/alerts/rules/job-queue-wait",
        r,
        json!({ "enabled": true, "threshold": 600, "forSecs": 120, "severity": "critical" }),
        "alert.rule.update",
    )
    .await;
    let channel = s
        .step(
            Atomic,
            "POST",
            "/api/v1/alerts/channels",
            "/api/v1/alerts/channels",
            r,
            json!({ "name": "ops", "url": "https://hooks.example/apelora", "secretRef": "APELORA_ALERT_SECRET_OPS" }),
            "alert.channel.create",
        )
        .await;
    let channel = id(&channel, "id");
    s.step(
        Atomic,
        "PUT",
        "/api/v1/alerts/channels/{id}",
        &format!("/api/v1/alerts/channels/{channel}"),
        r,
        json!({ "name": "ops", "url": "https://hooks.example/apelora-2", "secretRef": "APELORA_ALERT_SECRET_OPS", "minSeverity": "critical" }),
        "alert.channel.update",
    )
    .await;
    s.step(
        Atomic,
        "POST",
        "/api/v1/alerts/channels/{id}/test",
        &format!("/api/v1/alerts/channels/{channel}/test"),
        r,
        Value::Null,
        "alert.channel.test",
    )
    .await;
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/alerts/channels/{id}",
        &format!("/api/v1/alerts/channels/{channel}"),
        r,
        Value::Null,
        "alert.channel.delete",
    )
    .await;

    // ---- sectors ----
    s.step(
        Atomic,
        "POST",
        "/api/v1/sectors",
        "/api/v1/sectors",
        r,
        json!({ "name": "rack-b1", "site": "site-b", "url": "https://incus.b.example:8443", "credentials": "SITE_B" }),
        "sector.add",
    )
    .await;
    s.step(
        Atomic,
        "DELETE",
        "/api/v1/sectors/{name}",
        "/api/v1/sectors/rack-b1",
        r,
        json!({ "confirm": "rack-b1" }),
        "sector.remove",
    )
    .await;

    // ---- every route is either exercised above or exempt, with a reason ----
    let exempt: BTreeSet<(String, String)> = EXEMPT
        .iter()
        .map(|(m, p, why)| {
            assert!(why.len() > 20, "{m} {p}: say why it is exempt");
            ((*m).to_owned(), (*p).to_owned())
        })
        .collect();
    let routes = state_changing_routes();
    let missing: Vec<String> = routes
        .iter()
        .filter(|r| !s.covered.contains(*r) && !exempt.contains(*r))
        .map(|(m, p)| format!("{m} {p}"))
        .collect();
    assert!(
        missing.is_empty(),
        "state-changing routes with no audit check here and no exemption in EXEMPT: {missing:#?}"
    );
    let stale: Vec<String> = s
        .covered
        .iter()
        .chain(exempt.iter())
        .filter(|r| !routes.contains(*r))
        .map(|(m, p)| format!("{m} {p}"))
        .collect();
    assert!(
        stale.is_empty(),
        "not API operations (any more): {stale:#?}"
    );
    assert!(
        s.covered.is_disjoint(&exempt),
        "a route is both exercised and exempt"
    );
}

async fn count(pool: &Pool, action: &str) -> i64 {
    sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action = $1")
        .bind(action)
        .fetch_one(pool)
        .await
        .expect("count")
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn provider_sign_ins_are_audited_once_per_person_per_hour_whatever_the_request_rate(
    pool: Pool,
) {
    let l = super::identity_tests::lab(pool.clone());
    let ada = l.token("sub-ada", "ada");
    // Many requests at once, then more: one row.
    let calls: Vec<_> = (0..8)
        .map(|_| l.req("GET", "/api/v1/whoami", &ada, None))
        .collect();
    for (status, _) in futures_util::future::join_all(calls).await {
        assert_eq!(status, StatusCode::OK);
    }
    l.req("GET", "/api/v1/whoami", &ada, None).await;
    assert_eq!(count(&pool, "auth.oidc.signed_in").await, 1);
    let (actor, name, detail): (String, Option<String>, Value) = sqlx::query_as(
        "SELECT actor, actor_name, detail FROM audit_events WHERE action = 'auth.oidc.signed_in'",
    )
    .fetch_one(&pool)
    .await
    .expect("row");
    assert!(actor.starts_with("user:"), "{actor}");
    assert_eq!(name.as_deref(), Some("ada"));
    assert_eq!(detail["method"], "oidc");
    assert!(
        detail["issuer"]
            .as_str()
            .is_some_and(|i| i.starts_with("https://"))
    );
    assert!(detail["requestId"].as_str().is_some(), "{detail}");
    assert!(!detail.to_string().contains(&ada), "never the token");

    // Another person is audited on their own; an hour later Ada is audited again.
    l.req("GET", "/api/v1/whoami", &l.token("sub-bea", "bea"), None)
        .await;
    assert_eq!(count(&pool, "auth.oidc.signed_in").await, 2);
    sqlx::query(
        "UPDATE users SET sign_in_audited_at = now() - interval '61 minutes' WHERE username = 'ada'",
    )
    .execute(&pool)
    .await
    .expect("age");
    l.req("GET", "/api/v1/whoami", &ada, None).await;
    assert_eq!(count(&pool, "auth.oidc.signed_in").await, 3);

    // A refused token is never audited (anyone could send them).
    let (status, _) = l
        .req("GET", "/api/v1/whoami", "eyJhbGciOiJub25lIn0.e30.x", None)
        .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert_eq!(count(&pool, "auth.oidc.signed_in").await, 3);
}

async fn export(
    state: &AppState,
    token: &str,
    query: &str,
) -> (StatusCode, axum::http::HeaderMap, String) {
    let req = Request::builder()
        .uri(format!("/api/v1/audit/export?{query}"))
        .header(header::AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::empty())
        .expect("request");
    let res = router(state.clone()).oneshot(req).await.expect("response");
    let status = res.status();
    let headers = res.headers().clone();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        headers,
        String::from_utf8(bytes.to_vec()).expect("utf-8"),
    )
}

async fn listed(l: &super::identity_tests::Lab, token: &str, query: &str) -> Vec<(String, String)> {
    let (status, body) = l
        .req("GET", &format!("/api/v1/audit?{query}"), token, None)
        .await;
    assert_eq!(status, StatusCode::OK, "{query}: {body}");
    body["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|i| {
            (
                i["action"].as_str().unwrap_or_default().to_owned(),
                i["target"].as_str().unwrap_or_default().to_owned(),
            )
        })
        .collect()
}

/// The read side: filters on the list, the export in both formats with its own audit row, and
/// no access without `audit.read`.
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_audit_trail_is_filtered_and_exported_and_exports_are_audited(pool: Pool) {
    let l = super::identity_tests::lab(pool.clone());
    let (_, root) = l.super_admin("sub-root", "root").await;
    for slug in ["acme", "umbrella"] {
        let (status, _) = l
            .req(
                "POST",
                "/api/v1/identity/organizations",
                &root,
                Some(json!({ "slug": slug, "name": slug })),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED);
    }
    let org = |t: &str| ("organization.create".to_owned(), t.to_owned());
    assert_eq!(
        listed(&l, &root, "action=organization.").await,
        [org("umbrella"), org("acme")],
        "newest first"
    );
    assert_eq!(
        listed(&l, &root, "action=organization.&target=acme").await,
        [org("acme")]
    );
    assert_eq!(
        listed(&l, &root, "actor=ROOT&action=organization.")
            .await
            .len(),
        2,
        "by name, ignoring case"
    );
    assert!(
        listed(&l, &root, "actor=root&outcome=denied")
            .await
            .is_empty()
    );
    assert!(
        listed(&l, &root, "action=organization.&to=2000-01-01T00:00:00Z")
            .await
            .is_empty()
    );
    assert_eq!(
        listed(
            &l,
            &root,
            "action=organization.&from=2000-01-01T00:00:00Z&limit=1"
        )
        .await
        .len(),
        1
    );
    let (_, first) = l
        .req("GET", "/api/v1/audit?action=organization.", &root, None)
        .await;
    assert_eq!(first["items"][0]["actorName"], "root");
    let newest = first["items"][0]["id"].as_i64().expect("id");
    assert_eq!(
        listed(&l, &root, &format!("action=organization.&before={newest}")).await,
        [org("acme")],
        "paging"
    );
    for bad in [
        "outcome=maybe",
        "from=yesterday",
        "limit=0",
        "from=2026-01-02T00:00:00Z&to=2026-01-01T00:00:00Z",
    ] {
        let (status, _) = l
            .req("GET", &format!("/api/v1/audit?{bad}"), &root, None)
            .await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{bad}");
    }
    // The Identity & Access history takes the same filters.
    let (status, body) = l
        .req(
            "GET",
            "/api/v1/identity/audit?action=organization.&target=acme",
            &root,
            None,
        )
        .await;
    assert_eq!(
        (status, body["items"].as_array().map(Vec::len)),
        (StatusCode::OK, Some(1))
    );

    // Export: a header row (CSV) or one object per line, and on the record.
    let (status, headers, csv) = export(&l.state, &root, "format=csv&action=organization.").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(headers[header::CONTENT_TYPE], "text/csv; charset=utf-8");
    let disposition = headers[header::CONTENT_DISPOSITION]
        .to_str()
        .expect("ascii");
    assert!(
        disposition.starts_with("attachment; filename=\"apelora-audit-")
            && disposition.ends_with(".csv\""),
        "{disposition}"
    );
    let lines: Vec<&str> = csv.lines().collect();
    assert_eq!(
        lines[0],
        "id,at,actor,actorName,actorType,action,target,outcome,requestId,scope,detail"
    );
    assert_eq!(lines.len(), 3, "{csv}");
    assert!(
        lines[1].contains(",root,user,organization.create,umbrella,success,"),
        "{}",
        lines[1]
    );
    let (_, headers, jsonl) = export(&l.state, &root, "action=organization.&target=acme").await;
    assert_eq!(headers[header::CONTENT_TYPE], "application/x-ndjson");
    let rows: Vec<Value> = jsonl
        .lines()
        .map(|l| serde_json::from_str(l).expect("json line"))
        .collect();
    assert_eq!(rows.len(), 1);
    assert_eq!(
        (rows[0]["action"].as_str(), rows[0]["target"].as_str()),
        (Some("organization.create"), Some("acme"))
    );
    assert_eq!(count(&pool, "audit.exported").await, 2);
    let (target, detail): (String, Value) = sqlx::query_as(
        "SELECT target, detail FROM audit_events WHERE action = 'audit.exported' ORDER BY id DESC LIMIT 1",
    )
    .fetch_one(&pool)
    .await
    .expect("row");
    assert_eq!(target, "audit");
    assert_eq!(detail["format"], "jsonl");
    assert_eq!(
        detail["filters"],
        json!({ "action": "organization.", "target": "acme" })
    );
    let (status, _, _) = export(&l.state, &root, "format=xml").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // More events than one database page come out whole and in order.
    for n in 0..520 {
        apelora_persistence::audit(&pool, "test", "test.bulk", &format!("t{n}"), &json!({}))
            .await
            .expect("audit");
    }
    let (_, _, bulk) = export(&l.state, &root, "action=test.bulk").await;
    let ids: Vec<i64> = bulk
        .lines()
        .map(|l| {
            serde_json::from_str::<Value>(l).expect("json")["id"]
                .as_i64()
                .expect("id")
        })
        .collect();
    assert_eq!(ids.len(), 520);
    assert!(
        ids.windows(2).all(|w| w[0] > w[1]),
        "newest first, no repeats"
    );

    // Someone without audit.read gets neither the list nor an export.
    let nobody = l.token("sub-nobody", "nobody");
    for path in ["/api/v1/audit", "/api/v1/audit/export"] {
        let (status, _) = l.req("GET", path, &nobody, None).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{path}");
    }
}
