// Identity, tenancy and authorisation through the real router, against a real PostgreSQL.
// Tokens are signed with a locally generated key; no identity provider is involved (the
// Keycloak integration is NOT YET VERIFIED, see docs/architecture/IDENTITY-AND-AUTHORIZATION.md).

use std::sync::Arc;

use apelora_auth::{AssignmentKind, Builtin, Conditions, PrincipalRef, RoleRef, Scope};
use apelora_incus_adapter::MockIncus;
use apelora_persistence::identity as db;
use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use jsonwebtoken::{Algorithm, EncodingKey, Header, encode};
use rcgen::{KeyPair, PKCS_ECDSA_P256_SHA256};
use serde_json::json;
use uuid::Uuid;

use super::tests::{module, send};
use super::*;

const ISS: &str = "https://id.example.test/realms/apelora";
const AUD: &str = "apelora-api";

pub(super) struct Lab {
    pub(super) pool: Pool,
    pub(super) state: AppState,
    pem: String,
    #[allow(dead_code)]
    jwks: jsonwebtoken::jwk::JwkSet,
}

pub(super) fn lab_with(pool: Pool, legacy: bool) -> Lab {
    let kp = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("key");
    let raw = kp.public_key_raw();
    let jwks: jsonwebtoken::jwk::JwkSet = serde_json::from_value(json!({ "keys": [{ "kty": "EC", "crv": "P-256", "kid": "k1", "use": "sig", "alg": "ES256", "x": B64.encode(&raw[1..33]), "y": B64.encode(&raw[33..65]) }] })).expect("jwks");
    let verifier = apelora_identity::Verifier::with_keys(
        apelora_identity::OidcConfig::new(ISS, AUD),
        jwks.clone(),
    );
    let mock = MockIncus::default();
    let provider = Provider::Mock(mock.clone());
    let state = AppState::new(provider, Auth::Tokens(pool.clone()))
        .with_admin(Some(Provider::Mock(mock)))
        .with_oidc(Some(verifier))
        .with_legacy_tokens(legacy)
        .with_deploy_projects(vec!["apelora".into()]);
    Lab {
        pool,
        state,
        pem: kp.serialize_pem(),
        jwks,
    }
}

pub(super) fn lab(pool: Pool) -> Lab {
    lab_with(pool, true)
}

fn now() -> i64 {
    time::OffsetDateTime::now_utc().unix_timestamp()
}

impl Lab {
    pub(super) fn sign(&self, claims: &serde_json::Value, kid: Option<&str>) -> String {
        let mut h = Header::new(Algorithm::ES256);
        h.kid = kid.map(str::to_owned);
        encode(
            &h,
            claims,
            &EncodingKey::from_ec_pem(self.pem.as_bytes()).expect("pem"),
        )
        .expect("sign")
    }

    pub(super) fn claims(
        &self,
        sub: &str,
        name: &str,
        groups: &[&str],
        mfa: bool,
    ) -> serde_json::Value {
        json!({ "iss": ISS, "aud": AUD, "sub": sub, "iat": now() - 5, "exp": now() + 600, "preferred_username": name, "email": format!("{name}@example.test"),
                "groups": groups, "amr": if mfa { json!(["pwd", "otp"]) } else { json!(["pwd"]) } })
    }

    pub(super) fn token(&self, sub: &str, name: &str) -> String {
        self.sign(&self.claims(sub, name, &[], false), Some("k1"))
    }

    pub(super) fn token_mfa(&self, sub: &str, name: &str) -> String {
        self.sign(&self.claims(sub, name, &[], true), Some("k1"))
    }

    pub(super) async fn req(
        &self,
        method: &str,
        path: &str,
        token: &str,
        body: Option<serde_json::Value>,
    ) -> (StatusCode, serde_json::Value) {
        send(self.state.clone(), method, path, token, body, None).await
    }

    /// Signs the person in once (which creates their account) and returns their user id.
    pub(super) async fn user(&self, sub: &str, name: &str) -> Uuid {
        let (status, me) = self
            .req("GET", "/api/v1/whoami", &self.token(sub, name), None)
            .await;
        assert_eq!(status, StatusCode::OK, "{me}");
        me["id"].as_str().expect("id").parse().expect("uuid")
    }

    /// A person who is a platform super admin (granted directly in the database, as the bootstrap command does).
    pub(super) async fn super_admin(&self, sub: &str, name: &str) -> (Uuid, String) {
        let id = self.user(sub, name).await;
        db::insert_assignment(
            &self.pool,
            &new_assignment(id, Builtin::PlatformSuperAdmin, Scope::Platform),
        )
        .await
        .expect("assign");
        (id, self.token(sub, name))
    }

    pub(super) async fn grant(
        &self,
        by: &str,
        user: Uuid,
        role: &str,
        scope: serde_json::Value,
    ) -> (StatusCode, serde_json::Value) {
        self.req("POST", "/api/v1/identity/assignments", by, Some(json!({ "principal": { "type": "user", "id": user }, "role": { "type": "builtin", "key": role }, "scope": scope }))).await
    }
}

fn new_assignment(user: Uuid, role: Builtin, scope: Scope) -> db::NewAssignment {
    db::NewAssignment {
        principal: PrincipalRef::User { id: user },
        role: RoleRef::Builtin {
            key: role.key().into(),
        },
        scope,
        conditions: Conditions::default(),
        kind: AssignmentKind::Standard,
        expires_at: None,
        reason: None,
        created_by: "test".into(),
    }
}

pub(super) fn code(v: &serde_json::Value) -> Option<&str> {
    v["error"]["code"].as_str()
}

/// Two organisations with a project and a production environment each, and the people who run them.
pub(super) struct World {
    pub(super) acme: Uuid,
    pub(super) acme_prod: Uuid,
    pub(super) other: Uuid,
    pub(super) other_proj: Uuid,
    pub(super) admin: String,
}

pub(super) async fn world(l: &Lab) -> World {
    let (_, admin) = l.super_admin("sub-root", "root").await;
    let mk = |slug: &'static str| {
        let admin = admin.clone();
        async move {
            l.req(
                "POST",
                "/api/v1/identity/organizations",
                &admin,
                Some(json!({ "slug": slug, "name": slug })),
            )
            .await
        }
    };
    let (_, a) = mk("acme").await;
    let (_, o) = mk("other").await;
    let (acme, other): (Uuid, Uuid) = (
        a["id"].as_str().expect("id").parse().expect("uuid"),
        o["id"].as_str().expect("id").parse().expect("uuid"),
    );
    let project = |org: Uuid, slug: &'static str| {
        let admin = admin.clone();
        async move {
            l.req(
                "POST",
                &format!("/api/v1/identity/organizations/{org}/projects"),
                &admin,
                Some(json!({ "slug": slug, "name": slug })),
            )
            .await
        }
    };
    let (s1, p1) = project(acme, "prod").await;
    let (s2, p2) = project(other, "web").await;
    assert_eq!(
        (s1, s2),
        (StatusCode::CREATED, StatusCode::CREATED),
        "{p1} {p2}"
    );
    let (acme_prod, other_proj): (Uuid, Uuid) = (
        p1["id"].as_str().expect("id").parse().expect("uuid"),
        p2["id"].as_str().expect("id").parse().expect("uuid"),
    );
    for (proj, expect) in [
        (acme_prod, "acme-prod-live"),
        (other_proj, "other-web-live"),
    ] {
        let (s, env) = l
            .req(
                "POST",
                &format!("/api/v1/identity/projects/{proj}/environments"),
                &admin,
                Some(json!({ "slug": "live", "name": "Live", "kind": "production" })),
            )
            .await;
        assert_eq!(
            (s, env["incusProject"].as_str()),
            (StatusCode::CREATED, Some(expect)),
            "{env}"
        );
    }
    World {
        acme,
        acme_prod,
        other,
        other_proj,
        admin,
    }
}

pub(super) fn deploy_body(project: &str) -> serde_json::Value {
    json!({ "project": project, "document": {}, "modules": [module("web-01")] })
}

// ---- authentication ---------------------------------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn oidc_tokens_are_validated_and_people_are_created_on_first_sign_in(pool: Pool) {
    let l = lab(pool);
    let (status, me) = l
        .req("GET", "/api/v1/whoami", &l.token("sub-1", "alice"), None)
        .await;
    assert_eq!(status, StatusCode::OK, "{me}");
    assert_eq!(
        (
            me["kind"].as_str(),
            me["name"].as_str(),
            me["role"].is_null()
        ),
        (Some("user"), Some("alice"), true),
        "a person has no legacy role"
    );
    assert_eq!(
        me["platformPermissions"].as_array().map(Vec::len),
        Some(0),
        "a new person holds nothing: deny by default"
    );
    assert_eq!(
        db::list_users(&l.pool, &apelora_auth::AccessFilter::All)
            .await
            .expect("users")
            .len(),
        1
    );
    l.req("GET", "/api/v1/whoami", &l.token("sub-1", "alice"), None)
        .await;
    assert_eq!(
        db::list_users(&l.pool, &apelora_auth::AccessFilter::All)
            .await
            .expect("users")
            .len(),
        1,
        "the same subject is the same person"
    );

    let bad = |claims: serde_json::Value| l.sign(&claims, Some("k1"));
    let mut expired = l.claims("sub-1", "alice", &[], false);
    expired["exp"] = json!(now() - 3600);
    expired["iat"] = json!(now() - 4000);
    let mut wrong_aud = l.claims("sub-1", "alice", &[], false);
    wrong_aud["aud"] = json!("portal.example.com");
    let mut wrong_iss = l.claims("sub-1", "alice", &[], false);
    wrong_iss["iss"] = json!("https://evil.example.test/realms/apelora");
    let mut no_sub = l.claims("sub-1", "alice", &[], false);
    no_sub.as_object_mut().expect("object").remove("sub");
    let good = l.token("sub-1", "alice");
    let parts: Vec<&str> = good.split('.').collect();
    let mut admin_claims = l.claims("sub-1", "alice", &[], false);
    admin_claims["sub"] = json!("someone-else");
    let forged = format!(
        "{}.{}.{}",
        parts[0],
        B64.encode(serde_json::to_vec(&admin_claims).expect("json")),
        parts[2]
    );
    let none_alg = format!(
        "{}.{}.",
        B64.encode(br#"{"alg":"none"}"#),
        B64.encode(serde_json::to_vec(&l.claims("sub-1", "alice", &[], false)).expect("json"))
    );
    let unknown_key = l.sign(
        &l.claims("sub-1", "alice", &[], false),
        Some("not-published"),
    );
    for (label, token) in [
        ("expired", bad(expired)),
        ("audience", bad(wrong_aud)),
        ("issuer", bad(wrong_iss)),
        ("no subject", bad(no_sub)),
        ("forged payload", forged),
        ("alg none", none_alg),
        ("unknown key", unknown_key),
        ("garbage", "a.b.c".to_owned()),
    ] {
        let (status, body) = l.req("GET", "/api/v1/whoami", &token, None).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED, "{label}: {body}");
    }
    assert_eq!(
        l.req("GET", "/api/v1/whoami", "", None).await.0,
        StatusCode::UNAUTHORIZED
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn without_oidc_configured_tokens_from_a_provider_are_refused(pool: Pool) {
    let l = lab(pool);
    let token = l.token("sub-1", "alice");
    let state = AppState::new(
        Provider::Mock(MockIncus::default()),
        Auth::Tokens(l.pool.clone()),
    );
    let (status, body) = send(state, "GET", "/api/v1/whoami", &token, None, None).await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::UNAUTHORIZED, Some("unauthenticated"))
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn disabled_people_are_locked_out_at_once_and_it_is_audited(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let alice = l.user("sub-a", "alice").await;
    assert_eq!(
        l.req(
            "POST",
            &format!("/api/v1/identity/users/{alice}/disable"),
            &w.admin,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    let (status, body) = l
        .req("GET", "/api/v1/whoami", &l.token("sub-a", "alice"), None)
        .await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::FORBIDDEN, Some("account_disabled"))
    );
    assert_eq!(
        l.req(
            "POST",
            &format!("/api/v1/identity/users/{alice}/enable"),
            &w.admin,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        l.req("GET", "/api/v1/whoami", &l.token("sub-a", "alice"), None)
            .await
            .0,
        StatusCode::OK
    );
    let (root, _) = (l.user("sub-root", "root").await, ());
    let (status, body) = l
        .req(
            "POST",
            &format!("/api/v1/identity/users/{root}/disable"),
            &w.admin,
            None,
        )
        .await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::CONFLICT, Some("self_lockout"))
    );
    let events: Vec<String> = sqlx::query_scalar("SELECT action FROM audit_events WHERE action IN ('user.disable', 'user.enable', 'auth.refused') ORDER BY id").fetch_all(&l.pool).await.expect("audit");
    assert_eq!(events, ["user.disable", "auth.refused", "user.enable"]);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn legacy_tokens_keep_working_and_can_be_switched_off(pool: Pool) {
    let l = lab(pool.clone());
    let (legacy, _) = apelora_persistence::create_token(&pool, "ci", "fabric-admin", None)
        .await
        .expect("token");
    let (status, me) = l.req("GET", "/api/v1/whoami", &legacy, None).await;
    assert_eq!(
        (status, me["kind"].as_str(), me["role"].as_str()),
        (StatusCode::OK, Some("legacy-token"), Some("fabric-admin"))
    );
    let off = lab_with(pool, false);
    let (status, body) = off.req("GET", "/api/v1/whoami", &legacy, None).await;
    assert_eq!(
        (status, code(&body)),
        (StatusCode::UNAUTHORIZED, Some("unauthenticated"))
    );
}

// ---- default deny and the route gate ----------------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_person_with_no_role_is_denied_everywhere(pool: Pool) {
    let l = lab(pool);
    let _ = world(&l).await;
    let t = l.token("sub-nobody", "nobody");
    for (method, path) in [
        ("GET", "/api/v1/cluster"),
        ("GET", "/api/v1/instances"),
        ("GET", "/api/v1/applications"),
        ("GET", "/api/v1/jobs"),
        ("GET", "/api/v1/tokens"),
        ("GET", "/api/v1/machines"),
        ("GET", "/api/v1/audit"),
        ("GET", "/api/v1/system"),
        ("GET", "/api/v1/preflight"),
        ("GET", "/api/v1/identity/organizations"),
        ("GET", "/api/v1/identity/users"),
        ("GET", "/api/v1/identity/assignments"),
        ("GET", "/api/v1/identity/service-identities"),
        ("GET", "/api/v1/identity/policies"),
        ("GET", "/api/v1/approvals"),
        ("POST", "/api/v1/instances"),
        ("POST", "/api/v1/machines"),
        ("POST", "/api/v1/tokens"),
        ("POST", "/api/v1/identity/organizations"),
        ("POST", "/api/v1/identity/emergency-access"),
        ("POST", "/api/v1/cluster/join-tokens"),
    ] {
        let (status, body) = l.req(method, path, &t, Some(json!({}))).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{method} {path}: {body}");
    }
    assert_eq!(
        l.req("GET", "/api/v1/identity/permissions", &t, None)
            .await
            .0,
        StatusCode::OK,
        "the permission catalogue is public to signed-in people"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn every_route_has_an_access_rule_so_a_new_route_is_denied_until_someone_decides(pool: Pool) {
    let l = lab(pool);
    let (status, doc) = send(
        l.state.clone(),
        "GET",
        "/api/v1/openapi.json",
        "x",
        None,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let mut seen = 0;
    for (path, item) in doc["paths"].as_object().expect("paths") {
        // Public: what any visitor needs before signing in.
        if path == "/api/v1/health" || path == "/api/v1/openapi.json" || path == "/api/v1/sign-in" {
            continue;
        }
        for method in item.as_object().expect("item").keys() {
            let m = method.to_uppercase();
            assert!(
                authz::GATES
                    .iter()
                    .any(|(gm, gp, _)| *gm == m && gp == path),
                "{m} {path} has no entry in GATES"
            );
            seen += 1;
        }
    }
    assert!(seen > 60, "{seen}");
    for (m, p, _) in authz::GATES {
        assert!(
            doc["paths"][p][m.to_lowercase()].is_object(),
            "GATES lists {m} {p}, which is not an API operation"
        );
    }
}

// ---- tenancy: organisations, projects, scoping ------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn organisation_owners_run_their_own_tree_and_nothing_in_another_organisation(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let alice = l.user("sub-alice", "alice").await;
    let bob = l.user("sub-bob", "bob").await;
    assert_eq!(
        l.grant(
            &w.admin,
            alice,
            "organization-owner",
            json!({ "type": "organization", "id": w.acme })
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        l.grant(
            &w.admin,
            bob,
            "organization-owner",
            json!({ "type": "organization", "id": w.other })
        )
        .await
        .0,
        StatusCode::CREATED
    );
    let (ta, tb) = (l.token("sub-alice", "alice"), l.token("sub-bob", "bob"));

    // Each sees only their own organisation and projects.
    let (_, orgs) = l
        .req("GET", "/api/v1/identity/organizations", &ta, None)
        .await;
    assert_eq!(
        orgs["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|o| o["slug"].as_str().unwrap_or(""))
            .collect::<Vec<_>>(),
        ["acme"]
    );
    let (_, projects) = l.req("GET", "/api/v1/identity/projects", &tb, None).await;
    assert_eq!(
        projects["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|o| o["slug"].as_str().unwrap_or(""))
            .collect::<Vec<_>>(),
        ["web"]
    );

    // Alice can create a project in her organisation, not in Bob's, and cannot create organisations.
    let (s, _) = l
        .req(
            "POST",
            &format!("/api/v1/identity/organizations/{}/projects", w.acme),
            &ta,
            Some(json!({ "slug": "dev", "name": "Dev" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, _) = l
        .req(
            "POST",
            &format!("/api/v1/identity/organizations/{}/projects", w.other),
            &ta,
            Some(json!({ "slug": "dev", "name": "Dev" })),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "cross-organisation");
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/identity/organizations",
            &ta,
            Some(json!({ "slug": "mine", "name": "Mine" }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // Environments of Bob's project are invisible to Alice.
    assert_eq!(
        l.req(
            "GET",
            &format!("/api/v1/identity/projects/{}/environments", w.other_proj),
            &ta,
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        l.req(
            "GET",
            &format!("/api/v1/identity/projects/{}/environments", w.acme_prod),
            &ta,
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    // Users: Alice sees the people in her organisation only.
    let (_, users) = l.req("GET", "/api/v1/identity/users", &ta, None).await;
    assert_eq!(
        users["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|u| u["username"].as_str().unwrap_or(""))
            .collect::<Vec<_>>(),
        ["alice"]
    );
    // Assignments are scoped the same way.
    let (_, asg) = l
        .req("GET", "/api/v1/identity/assignments", &tb, None)
        .await;
    assert!(
        asg["items"]
            .as_array()
            .expect("items")
            .iter()
            .all(|a| a["scope"]["id"] == json!(w.other)),
        "{asg}"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn applications_are_scoped_by_project_and_other_tenants_cannot_see_or_touch_them(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let (alice, bob) = (
        l.user("sub-alice", "alice").await,
        l.user("sub-bob", "bob").await,
    );
    l.grant(
        &w.admin,
        alice,
        "organization-owner",
        json!({ "type": "organization", "id": w.acme }),
    )
    .await;
    l.grant(
        &w.admin,
        bob,
        "organization-owner",
        json!({ "type": "organization", "id": w.other }),
    )
    .await;
    let (ta, tb) = (l.token("sub-alice", "alice"), l.token("sub-bob", "bob"));

    let (s, d) = l
        .req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &ta,
            Some(deploy_body("acme-prod-live")),
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{d}");
    // The application lives in Alice's project.
    let ctx = db::application_context(&l.pool, "shop")
        .await
        .expect("db")
        .expect("app");
    assert_eq!((ctx.project_id, ctx.organization_id), (w.acme_prod, w.acme));

    // Bob cannot see, plan, deploy to or read the status of it, and his list is empty.
    let (_, listed) = l.req("GET", "/api/v1/applications", &tb, None).await;
    assert_eq!(
        listed["items"].as_array().map(Vec::len),
        Some(0),
        "{listed}"
    );
    let (_, listed) = l.req("GET", "/api/v1/applications", &ta, None).await;
    assert_eq!(listed["items"][0]["name"], "shop");
    assert_eq!(
        l.req("GET", "/api/v1/applications/shop/status", &tb, None)
            .await
            .0,
        StatusCode::NOT_FOUND,
        "no existence leak"
    );
    assert_eq!(
        l.req("GET", "/api/v1/applications/shop/status", &ta, None)
            .await
            .0,
        StatusCode::OK
    );
    let (s, _) = l
        .req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &tb,
            Some(deploy_body("other-web-live")),
        )
        .await;
    assert_eq!(
        s,
        StatusCode::CONFLICT,
        "the name belongs to another project"
    );
    let (s, _) = l
        .req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &tb,
            Some(deploy_body("acme-prod-live")),
        )
        .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "Bob may not deploy into Alice's environment"
    );
    let (s, _) = l
        .req(
            "POST",
            "/api/v1/applications/shop/plan",
            &tb,
            Some(deploy_body("acme-prod-live")),
        )
        .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    // Alice cannot deploy into Bob's environment or into an Incus project no environment owns.
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/mine/deployments",
            &ta,
            Some(deploy_body("other-web-live"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/mine/deployments",
            &ta,
            Some(deploy_body("apelora"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "unmapped Incus projects are infrastructure"
    );
    // Her application cannot be moved to another environment's Incus project.
    let (s, b) = l
        .req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &ta,
            Some(deploy_body("other-web-live")),
        )
        .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("project_mismatch")),
        "{b}"
    );
    // Raw instances follow the same environment rule.
    let spec = |p: &str| json!({ "project": p, "name": "web-01", "kind": "container", "source": { "protocol": "oci", "server": "https://registry.example", "alias": "nginx:1.29" }, "start": true });
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/instances",
            &ta,
            Some(spec("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/instances",
            &ta,
            Some(spec("other-web-live"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        l.req("POST", "/api/v1/instances", &ta, Some(spec("apelora")))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // Jobs: Bob cannot read Alice's jobs by id; Alice can.
    let job = d["jobs"][0]["jobId"]
        .as_str()
        .or(d["networkJob"].as_str())
        .map(str::to_owned);
    if let Some(id) = job {
        assert_eq!(
            l.req("GET", &format!("/api/v1/jobs/{id}"), &tb, None)
                .await
                .0,
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            l.req("GET", &format!("/api/v1/jobs/{id}"), &ta, None)
                .await
                .0,
            StatusCode::OK
        );
    }
    let (_, jobs) = l.req("GET", "/api/v1/jobs", &tb, None).await;
    assert!(
        jobs["items"].as_array().is_none_or(|j| j
            .iter()
            .all(|x| x["request"]["project"] != "acme-prod-live")),
        "{jobs}"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_same_person_holds_different_roles_in_different_projects_and_roles_inherit_downwards(
    pool: Pool,
) {
    let l = lab(pool);
    let w = world(&l).await;
    let (s, dev) = l
        .req(
            "POST",
            &format!("/api/v1/identity/organizations/{}/projects", w.acme),
            &w.admin,
            Some(json!({ "slug": "dev", "name": "Dev" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let acme_dev: Uuid = dev["id"].as_str().expect("id").parse().expect("uuid");
    let (s, _) = l
        .req(
            "POST",
            &format!("/api/v1/identity/projects/{acme_dev}/environments"),
            &w.admin,
            Some(json!({ "slug": "live", "name": "Live", "kind": "development" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let carol = l.user("sub-carol", "carol").await;
    l.grant(
        &w.admin,
        carol,
        "project-administrator",
        json!({ "type": "project", "id": w.acme_prod }),
    )
    .await;
    l.grant(
        &w.admin,
        carol,
        "viewer",
        json!({ "type": "project", "id": acme_dev }),
    )
    .await;
    let t = l.token("sub-carol", "carol");

    // Administrator in production: may deploy there (inherited by every application).
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &t,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    // Viewer in development: nothing exists to deploy; she cannot create or deploy there.
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/tool/deployments",
            &t,
            Some(deploy_body("acme-dev-live"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // Another administrator creates the development application; Carol can view its status but not change it.
    let dan = l.user("sub-dan", "dan").await;
    l.grant(
        &w.admin,
        dan,
        "project-administrator",
        json!({ "type": "project", "id": acme_dev }),
    )
    .await;
    let td = l.token("sub-dan", "dan");
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/tool/deployments",
            &td,
            Some(deploy_body("acme-dev-live"))
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        l.req("GET", "/api/v1/applications/tool/status", &t, None)
            .await
            .0,
        StatusCode::OK,
        "viewer"
    );
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/tool/deployments",
            &t,
            Some(deploy_body("acme-dev-live"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "but not deploy"
    );
    // Dan sees only the development application; Carol sees both.
    let names = |v: serde_json::Value| {
        v["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|a| a["name"].as_str().unwrap_or("").to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(
        names(l.req("GET", "/api/v1/applications", &td, None).await.1),
        ["tool"]
    );
    assert_eq!(
        names(l.req("GET", "/api/v1/applications", &t, None).await.1),
        ["shop", "tool"]
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn designers_design_and_operators_deploy_only_where_assigned(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let owner = l.user("sub-own", "owner").await;
    l.grant(
        &w.admin,
        owner,
        "organization-owner",
        json!({ "type": "organization", "id": w.acme }),
    )
    .await;
    let to = l.token("sub-own", "owner");
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &to,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/blog/deployments",
            &to,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    let shop = db::application_context(&l.pool, "shop")
        .await
        .expect("db")
        .expect("app");

    let (des, op) = (l.user("sub-des", "des").await, l.user("sub-op", "op").await);
    l.grant(
        &w.admin,
        des,
        "application-designer",
        json!({ "type": "project", "id": w.acme_prod }),
    )
    .await;
    l.grant(
        &w.admin,
        op,
        "application-operator",
        json!({ "type": "application", "id": shop.application_id }),
    )
    .await;
    let (td, top) = (l.token("sub-des", "des"), l.token("sub-op", "op"));

    // The designer previews plans but cannot deploy, create, or touch infrastructure.
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/shop/plan",
            &td,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &td,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    for (m, p) in [
        ("GET", "/api/v1/cluster"),
        ("GET", "/api/v1/machines"),
        ("POST", "/api/v1/machines"),
        ("GET", "/api/v1/tokens"),
        ("POST", "/api/v1/cluster/join-tokens"),
        ("GET", "/api/v1/storage-pools"),
    ] {
        assert_eq!(
            l.req(m, p, &td, Some(json!({}))).await.0,
            StatusCode::FORBIDDEN,
            "{m} {p}"
        );
    }
    // The operator deploys the one application she is assigned to; not its sibling; cannot create new ones.
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &top,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/blog/deployments",
            &top,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/newone/deployments",
            &top,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        l.req("GET", "/api/v1/applications/blog/status", &top, None)
            .await
            .0,
        StatusCode::NOT_FOUND
    );
    let (_, listed) = l.req("GET", "/api/v1/applications", &top, None).await;
    assert_eq!(listed["items"].as_array().map(Vec::len), Some(1));
    // She sees the jobs of her application.
    assert_eq!(
        l.req("GET", "/api/v1/jobs", &top, None).await.0,
        StatusCode::OK
    );
}

// ---- privilege escalation ---------------------------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn nobody_can_grant_more_than_they_hold_and_an_organisation_owner_never_reaches_infrastructure(
    pool: Pool,
) {
    let l = lab(pool);
    let w = world(&l).await;
    let alice = l.user("sub-alice", "alice").await;
    let mallory = l.user("sub-mallory", "mallory").await;
    l.grant(
        &w.admin,
        alice,
        "organization-owner",
        json!({ "type": "organization", "id": w.acme }),
    )
    .await;
    let ta = l.token("sub-alice", "alice");
    let org = json!({ "type": "organization", "id": w.acme });

    // Roles she may grant inside her organisation.
    for role in ["project-administrator", "viewer", "auditor"] {
        let scope = if role == "project-administrator" {
            json!({ "type": "project", "id": w.acme_prod })
        } else {
            org.clone()
        };
        assert_eq!(
            l.grant(&ta, mallory, role, scope).await.0,
            StatusCode::CREATED,
            "{role}"
        );
    }
    // Infrastructure and platform roles: refused at any scope, with the reason recorded.
    for role in [
        "platform-super-admin",
        "fabric-administrator",
        "hardware-operator",
    ] {
        for scope in [
            json!({ "type": "platform" }),
            org.clone(),
            json!({ "type": "pool", "name": "rack-1" }),
        ] {
            let (s, b) = l.grant(&ta, mallory, role, scope.clone()).await;
            assert_eq!(s, StatusCode::FORBIDDEN, "{role} at {scope}: {b}");
        }
    }
    // Another organisation, and roles on herself beyond her own.
    assert_eq!(
        l.grant(
            &ta,
            mallory,
            "viewer",
            json!({ "type": "organization", "id": w.other })
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        l.grant(
            &ta,
            alice,
            "platform-super-admin",
            json!({ "type": "platform" })
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // She cannot define roles, mint emergency access, register service identities or machines, or issue tokens.
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/identity/roles",
            &ta,
            Some(json!({ "name": "Mine", "permissions": ["application.view"] }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(l.req("POST", "/api/v1/identity/emergency-access", &ta, Some(json!({ "userId": mallory, "role": "fabric-administrator", "reason": "just because", "minutes": 30 }))).await.0, StatusCode::FORBIDDEN);
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/identity/service-identities",
            &ta,
            Some(json!({ "name": "x", "kind": "automation" }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        l.req(
            "PUT",
            "/api/v1/identity/policies",
            &ta,
            Some(json!({ "name": "x", "kind": "mfa", "parameters": {} }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/tokens",
            &ta,
            Some(json!({ "name": "x", "role": "fabric-admin" }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // An organisation owner cannot map an environment to an arbitrary Incus project.
    let (s, b) = l.req("POST", &format!("/api/v1/identity/projects/{}/environments", w.acme_prod), &ta, Some(json!({ "slug": "sneaky", "name": "S", "kind": "other", "incusProject": "default" }))).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{b}");
    // …but she can create an environment, whose Incus project is derived from the names.
    let (s, env) = l
        .req(
            "POST",
            &format!("/api/v1/identity/projects/{}/environments", w.acme_prod),
            &ta,
            Some(json!({ "slug": "staging", "name": "Staging", "kind": "staging" })),
        )
        .await;
    assert_eq!(
        (
            s,
            env["incusProject"].as_str(),
            env["incusProvisioned"].as_bool()
        ),
        (StatusCode::CREATED, Some("acme-prod-staging"), Some(false)),
        "{env}"
    );
    assert_eq!(env["incusProjectConfig"]["restricted"], "true");
    // The evidence: the refusals are in the audit trail.
    let refused: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'role.assign' AND outcome = 'denied'",
    )
    .fetch_one(&l.pool)
    .await
    .expect("count");
    assert!(refused >= 9, "{refused}");
    // Every infrastructure route is closed to her.
    for (m, p) in [
        ("GET", "/api/v1/cluster"),
        ("GET", "/api/v1/machines"),
        ("POST", "/api/v1/machines"),
        ("DELETE", "/api/v1/machines/rack1-07"),
        ("POST", "/api/v1/cluster/join-tokens"),
        ("POST", "/api/v1/cluster/members/mock-member-01/evacuate"),
        ("GET", "/api/v1/storage-pools"),
        ("GET", "/api/v1/networks"),
    ] {
        assert_eq!(
            l.req(m, p, &ta, Some(json!({}))).await.0,
            StatusCode::FORBIDDEN,
            "{m} {p}"
        );
    }
    // Impersonation is not offered: a header naming someone else changes nothing.
    let (status, _) = send(l.state.clone(), "GET", "/api/v1/cluster", &ta, None, None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let req = axum::http::Request::builder()
        .method("GET")
        .uri("/api/v1/cluster")
        .header(header::AUTHORIZATION, format!("Bearer {ta}"))
        .header("x-apelora-impersonate", "root")
        .header("x-forwarded-user", "root")
        .body(axum::body::Body::empty())
        .expect("request");
    let res = tower::ServiceExt::oneshot(router(l.state.clone()), req)
        .await
        .expect("response");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn custom_roles_use_explicit_permissions_and_cannot_exceed_their_creator(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let (s, role) = l.req("POST", "/api/v1/identity/roles", &w.admin, Some(json!({ "name": "Release manager", "description": "Deploys and reads", "permissions": ["application.view", "application.deploy"] }))).await;
    assert_eq!(s, StatusCode::CREATED, "{role}");
    assert_eq!(
        role["scopes"],
        json!(["platform", "organization", "project", "application"])
    );
    for bad in [
        json!({ "name": "", "permissions": ["application.view"] }),
        json!({ "name": "Empty", "permissions": [] }),
        json!({ "name": "Typo", "permissions": ["application.fly"] }),
        json!({ "name": "Viewer", "permissions": ["application.view"] }),
    ] {
        assert_eq!(
            l.req("POST", "/api/v1/identity/roles", &w.admin, Some(bad))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    // Assign it to a person in a project; it grants exactly deploy and view there.
    let rm = l.user("sub-rm", "rm").await;
    let role_id = role["reference"]["id"].as_str().expect("id");
    let (s, _) = l.req("POST", "/api/v1/identity/assignments", &w.admin, Some(json!({ "principal": { "type": "user", "id": rm }, "role": { "type": "custom", "id": role_id }, "scope": { "type": "project", "id": w.acme_prod } }))).await;
    assert_eq!(s, StatusCode::CREATED);
    // Deploying needs application.create for a new application: the custom role has none.
    let t = l.token("sub-rm", "rm");
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &t,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let owner = l.user("sub-own", "owner").await;
    l.grant(
        &w.admin,
        owner,
        "organization-owner",
        json!({ "type": "organization", "id": w.acme }),
    )
    .await;
    l.req(
        "POST",
        "/api/v1/applications/shop/deployments",
        &l.token("sub-own", "owner"),
        Some(deploy_body("acme-prod-live")),
    )
    .await;
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &t,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::ACCEPTED,
        "existing application: deploy is enough"
    );
    // A role in use cannot be deleted; unused ones can.
    let (s, b) = l
        .req(
            "DELETE",
            &format!("/api/v1/identity/roles/{role_id}"),
            &w.admin,
            None,
        )
        .await;
    assert_eq!((s, code(&b)), (StatusCode::CONFLICT, Some("conflict")));
    let (_, spare) = l
        .req(
            "POST",
            "/api/v1/identity/roles",
            &w.admin,
            Some(json!({ "name": "Spare", "permissions": ["application.view"] })),
        )
        .await;
    assert_eq!(
        l.req(
            "DELETE",
            &format!(
                "/api/v1/identity/roles/{}",
                spare["reference"]["id"].as_str().expect("id")
            ),
            &w.admin,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    // A holder of role.manage who lacks a permission cannot put it in a role.
    let fa = l.user("sub-fa", "fa").await;
    l.grant(
        &w.admin,
        fa,
        "fabric-administrator",
        json!({ "type": "platform" }),
    )
    .await;
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/identity/roles",
            &l.token("sub-fa", "fa"),
            Some(json!({ "name": "Nope", "permissions": ["application.view"] }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "fabric administrators do not define roles"
    );
}

// ---- revocation, expiry, groups ---------------------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn revoking_an_assignment_or_leaving_a_group_ends_access_on_the_next_request(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let (carol, dave) = (
        l.user("sub-carol", "carol").await,
        l.user("sub-dave", "dave").await,
    );
    let (s, asg) = l
        .grant(
            &w.admin,
            carol,
            "project-administrator",
            json!({ "type": "project", "id": w.acme_prod }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let t = l.token("sub-carol", "carol");
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &t,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        l.req(
            "DELETE",
            &format!(
                "/api/v1/identity/assignments/{}",
                asg["id"].as_str().expect("id")
            ),
            &w.admin,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &t,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "same token, access gone"
    );
    assert_eq!(
        l.req("GET", "/api/v1/applications", &t, None).await.0,
        StatusCode::FORBIDDEN,
        "no scope reaches any application"
    );

    // Through a group: membership grants, removal revokes.
    let (s, g) = l
        .req(
            "POST",
            "/api/v1/identity/groups",
            &w.admin,
            Some(json!({ "organizationId": w.acme, "name": "Operators" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{g}");
    let gid: Uuid = g["id"].as_str().expect("id").parse().expect("uuid");
    let (s, _) = l.req("POST", "/api/v1/identity/assignments", &w.admin, Some(json!({ "principal": { "type": "group", "id": gid }, "role": { "type": "builtin", "key": "project-administrator" }, "scope": { "type": "project", "id": w.acme_prod } }))).await;
    assert_eq!(s, StatusCode::CREATED);
    let td = l.token("sub-dave", "dave");
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &td,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "not yet a member"
    );
    assert_eq!(
        l.req(
            "PUT",
            &format!("/api/v1/identity/groups/{gid}/members/{dave}"),
            &w.admin,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &td,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        l.req(
            "DELETE",
            &format!("/api/v1/identity/groups/{gid}/members/{dave}"),
            &w.admin,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &td,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // An organisation's group cannot be used in another organisation's tree.
    let (s, b) = l.req("POST", "/api/v1/identity/assignments", &w.admin, Some(json!({ "principal": { "type": "group", "id": gid }, "role": { "type": "builtin", "key": "viewer" }, "scope": { "type": "organization", "id": w.other } }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST, "{b}");
    // Groups named in the token map to groups by their provider name.
    let (s, _) = l
        .req(
            "POST",
            "/api/v1/identity/groups",
            &w.admin,
            Some(json!({ "name": "Platform team", "externalName": "platform-team" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let (_, groups) = l
        .req("GET", "/api/v1/identity/groups", &w.admin, None)
        .await;
    let platform_group: Uuid = groups["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|g| g["externalName"] == "platform-team")
        .and_then(|g| g["id"].as_str())
        .expect("group")
        .parse()
        .expect("uuid");
    l.req("POST", "/api/v1/identity/assignments", &w.admin, Some(json!({ "principal": { "type": "group", "id": platform_group }, "role": { "type": "builtin", "key": "auditor" }, "scope": { "type": "platform" } }))).await;
    let via_idp = l.sign(
        &l.claims("sub-eve", "eve", &["/platform-team"], false),
        Some("k1"),
    );
    assert_eq!(
        l.req("GET", "/api/v1/audit", &via_idp, None).await.0,
        StatusCode::OK,
        "the token's group name maps to the group"
    );
    assert_eq!(
        l.req("GET", "/api/v1/audit", &l.token("sub-eve", "eve"), None)
            .await
            .0,
        StatusCode::FORBIDDEN,
        "without the group claim, nothing"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn expired_assignments_and_the_last_super_admin(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let carol = l.user("sub-carol", "carol").await;
    let mut a = new_assignment(
        carol,
        Builtin::ProjectAdministrator,
        Scope::Project { id: w.acme_prod },
    );
    a.expires_at = Some(time::OffsetDateTime::now_utc() - time::Duration::minutes(1));
    db::insert_assignment(&l.pool, &a).await.expect("assign");
    assert_eq!(
        l.req(
            "GET",
            "/api/v1/applications",
            &l.token("sub-carol", "carol"),
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "an expired assignment grants nothing"
    );
    // Past expiries cannot be created through the API.
    let (s, _) = l.req("POST", "/api/v1/identity/assignments", &w.admin, Some(json!({ "principal": { "type": "user", "id": carol }, "role": { "type": "builtin", "key": "viewer" }, "scope": { "type": "platform" }, "expiresAt": "2020-01-01T00:00:00Z" }))).await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // The only platform super admin cannot be revoked.
    let (_, mine) = l
        .req("GET", "/api/v1/identity/assignments", &w.admin, None)
        .await;
    let sa = mine["items"]
        .as_array()
        .expect("items")
        .iter()
        .find(|x| x["roleName"] == "Platform Super Admin")
        .and_then(|x| x["id"].as_str())
        .expect("assignment")
        .to_owned();
    let (s, b) = l
        .req(
            "DELETE",
            &format!("/api/v1/identity/assignments/{sa}"),
            &w.admin,
            None,
        )
        .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("last_super_admin"))
    );
}

// ---- service identities -----------------------------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn service_credentials_are_separated_and_cannot_be_misused(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    // Outbound identities record a credential name, never a secret, and never get an API token.
    let (s, svc) = l.req("POST", "/api/v1/identity/service-identities", &w.admin, Some(json!({ "name": "incus-deploy", "kind": "workload", "credentialRef": "incus-deploy-cert", "restrictions": { "projects": ["acme-prod-live"] } }))).await;
    assert_eq!(s, StatusCode::CREATED, "{svc}");
    assert!(svc["token"].is_null());
    assert_eq!(svc["identity"]["apiAccess"], false);
    assert!(
        svc["identity"]["operations"]
            .as_array()
            .expect("ops")
            .iter()
            .any(|o| o == "instance.create")
    );
    assert!(
        svc["identity"]["operations"]
            .as_array()
            .expect("ops")
            .iter()
            .all(|o| o != "machine.power"),
        "workload credentials cannot power machines"
    );
    // Built at run time so that no key-shaped text sits in the source (the secret scanner would rightly object).
    let pasted_key = format!(
        "{} {} KEY{}abc",
        "-".repeat(5) + "BEGIN",
        "PRIVATE",
        "-".repeat(5)
    );
    for (name, kind, cred) in [
        ("bad-secret", "network", pasted_key.as_str()),
        ("spaces", "storage", "my secret value"),
    ] {
        let (s, _) = l
            .req(
                "POST",
                "/api/v1/identity/service-identities",
                &w.admin,
                Some(json!({ "name": name, "kind": kind, "credentialRef": cred })),
            )
            .await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "{name}");
    }
    let (s, _) = l
        .req(
            "POST",
            "/api/v1/identity/service-identities",
            &w.admin,
            Some(json!({ "name": "ci-with-cert", "kind": "automation", "credentialRef": "x" })),
        )
        .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    // A workload identity cannot hold role assignments or authenticate to the API.
    let wid: Uuid = svc["identity"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let (s, _) = l.req("POST", "/api/v1/identity/assignments", &w.admin, Some(json!({ "principal": { "type": "service", "id": wid }, "role": { "type": "builtin", "key": "viewer" }, "scope": { "type": "platform" } }))).await;
    assert_eq!(
        s,
        StatusCode::BAD_REQUEST,
        "outbound credentials hold no role assignments"
    );
    assert_eq!(
        l.req(
            "GET",
            "/api/v1/whoami",
            "aps_0000000000000000000000000000000000000000000000000000000000000000",
            None
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );

    // An automation identity gets a token (once) and only the permissions it is assigned.
    let (s, ci) = l
        .req(
            "POST",
            "/api/v1/identity/service-identities",
            &w.admin,
            Some(json!({ "name": "ci", "kind": "automation" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED);
    let token = ci["token"].as_str().expect("token").to_owned();
    let ci_id: Uuid = ci["identity"]["id"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    let (s, me) = l.req("GET", "/api/v1/whoami", &token, None).await;
    assert_eq!((s, me["kind"].as_str()), (StatusCode::OK, Some("service")));
    assert_eq!(
        l.req("GET", "/api/v1/cluster", &token, None).await.0,
        StatusCode::FORBIDDEN,
        "no assignment, no access"
    );
    // It cannot be made a super admin, nor given the power to manage people or approve.
    let grant = |role: &'static str, scope: serde_json::Value| {
        let admin = w.admin.clone();
        let l = &l;
        async move {
            l.req("POST", "/api/v1/identity/assignments", &admin, Some(json!({ "principal": { "type": "service", "id": ci_id }, "role": { "type": "builtin", "key": role }, "scope": scope }))).await
        }
    };
    assert_eq!(
        grant("platform-super-admin", json!({ "type": "platform" }))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        grant("fabric-administrator", json!({ "type": "platform" }))
            .await
            .0,
        StatusCode::FORBIDDEN,
        "it holds approval.grant, which a service may not"
    );
    assert_eq!(
        grant(
            "application-operator",
            json!({ "type": "project", "id": w.acme_prod })
        )
        .await
        .0,
        StatusCode::CREATED
    );
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &token,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "an operator cannot create applications"
    );
    // Identity administration is out of reach for a service even with broad roles.
    for (m, p) in [
        ("POST", "/api/v1/tokens"),
        ("POST", "/api/v1/identity/roles"),
        ("POST", "/api/v1/identity/assignments"),
        ("POST", "/api/v1/identity/emergency-access"),
        ("GET", "/api/v1/identity/service-identities"),
        (
            "POST",
            "/api/v1/approvals/00000000-0000-0000-0000-000000000000/approve",
        ),
    ] {
        assert_eq!(
            l.req(m, p, &token, Some(json!({}))).await.0,
            StatusCode::FORBIDDEN,
            "{m} {p}"
        );
    }
    // Disabling stops it at once; expired tokens stop too.
    assert_eq!(
        l.req(
            "POST",
            &format!("/api/v1/identity/service-identities/{ci_id}/disable"),
            &w.admin,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        l.req("GET", "/api/v1/whoami", &token, None).await.0,
        StatusCode::UNAUTHORIZED
    );
    l.req(
        "POST",
        &format!("/api/v1/identity/service-identities/{ci_id}/enable"),
        &w.admin,
        None,
    )
    .await;
    sqlx::query(
        "UPDATE service_identities SET expires_at = now() - interval '1 minute' WHERE id = $1",
    )
    .bind(ci_id)
    .execute(&l.pool)
    .await
    .expect("expire");
    assert_eq!(
        l.req("GET", "/api/v1/whoami", &token, None).await.0,
        StatusCode::UNAUTHORIZED,
        "expired service credential"
    );
}

// ---- approvals, emergency access, audit, policies, preview -----------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn risky_actions_need_another_persons_approval_that_is_exact_and_single_use(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let (hw, fa) = (l.user("sub-hw", "hw").await, l.user("sub-fa", "fa").await);
    l.grant(
        &w.admin,
        hw,
        "hardware-operator",
        json!({ "type": "platform" }),
    )
    .await;
    l.grant(
        &w.admin,
        fa,
        "fabric-administrator",
        json!({ "type": "platform" }),
    )
    .await;
    let (thw, tfa) = (l.token("sub-hw", "hw"), l.token("sub-fa", "fa"));
    apelora_persistence::register_machine(&l.pool, "rack1-07", "manual", "", "test", false)
        .await
        .expect("machine");
    apelora_persistence::register_machine(&l.pool, "rack1-08", "manual", "", "test", false)
        .await
        .expect("machine");

    // Without an approval the deletion is refused, with instructions.
    let (s, b) = l
        .req("DELETE", "/api/v1/machines/rack1-07", &thw, None)
        .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("approval_required")),
        "{b}"
    );
    // Who may ask: someone who may do the thing. A fabric administrator may not delete hardware.
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/approvals",
            &tfa,
            Some(json!({ "action": "hardware.delete", "target": "machine:rack1-07" }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/approvals",
            &thw,
            Some(json!({ "action": "application.view", "target": "x" }))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST,
        "only risky actions need approvals"
    );
    let (s, ap) = l.req("POST", "/api/v1/approvals", &thw, Some(json!({ "action": "hardware.delete", "target": "machine:rack1-07", "reason": "decommissioned" }))).await;
    assert_eq!(s, StatusCode::CREATED, "{ap}");
    let id = ap["id"].as_str().expect("id").to_owned();
    // The requester cannot approve their own request, and a pending request is not an approval.
    assert_eq!(
        l.req(
            "POST",
            &format!("/api/v1/approvals/{id}/approve"),
            &thw,
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "hardware operators hold no approval.grant"
    );
    let hdr = |token: &str, target: &str, id: &str| {
        let req = axum::http::Request::builder()
            .method("DELETE")
            .uri(format!("/api/v1/machines/{target}"))
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .header("apelora-approval", id)
            .body(axum::body::Body::empty())
            .expect("request");
        let router = router(l.state.clone());
        async move {
            let res = tower::ServiceExt::oneshot(router, req)
                .await
                .expect("response");
            let status = res.status();
            let bytes = http_body_util::BodyExt::collect(res.into_body())
                .await
                .expect("body")
                .to_bytes();
            (
                status,
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default(),
            )
        }
    };
    let (s, b) = hdr(&thw, "rack1-07", &id).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("approval_invalid")),
        "pending"
    );
    // A different person approves.
    let (s, done) = l
        .req(
            "POST",
            &format!("/api/v1/approvals/{id}/approve"),
            &tfa,
            None,
        )
        .await;
    assert_eq!(
        (s, done["state"].as_str()),
        (StatusCode::OK, Some("approved")),
        "{done}"
    );
    assert_eq!(
        l.req(
            "POST",
            &format!("/api/v1/approvals/{id}/approve"),
            &tfa,
            None
        )
        .await
        .0,
        StatusCode::CONFLICT,
        "decided once"
    );
    // It is for exactly this machine and this requester.
    let (s, b) = hdr(&thw, "rack1-08", &id).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("approval_invalid")),
        "another machine: {b}"
    );
    // Presenting an approval that does not fit is a refusal on the record.
    let refused: Vec<(String, Option<String>, bool)> = sqlx::query_as(
        "SELECT target, detail->>'approval', detail ? 'requestId' FROM audit_events
         WHERE action = 'access.denied' AND outcome = 'denied' AND detail->>'code' = 'approval_invalid'",
    )
    .fetch_all(&l.pool)
    .await
    .expect("audit");
    assert!(
        refused
            .iter()
            .any(|(t, a, rid)| t.contains("rack1-08") && a.as_deref() == Some(id.as_str()) && *rid),
        "{refused:?}"
    );
    let (s, _) = hdr(&tfa, "rack1-07", &id).await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "someone else cannot use it (they may not delete machines at all)"
    );
    // It works once.
    let (s, b) = hdr(&thw, "rack1-07", &id).await;
    assert_eq!(s, StatusCode::NO_CONTENT, "{b}");
    let (s, b) = hdr(&thw, "rack1-07", &id).await;
    assert!(
        s == StatusCode::NOT_FOUND || s == StatusCode::CONFLICT,
        "{s} {b}"
    );
    assert!(
        apelora_persistence::machine(&l.pool, "rack1-07")
            .await
            .is_err(),
        "the machine is gone"
    );
    assert!(
        apelora_persistence::machine(&l.pool, "rack1-08")
            .await
            .is_ok(),
        "the other machine is untouched"
    );
    let removed: serde_json::Value =
        sqlx::query_scalar("SELECT detail FROM audit_events WHERE action = 'machine.remove'")
            .fetch_one(&l.pool)
            .await
            .expect("audit");
    assert_eq!(
        removed["approval"].as_str(),
        Some(id.as_str()),
        "the audit event names the approval"
    );
    // A rejected request cannot be used, and a requester can withdraw their own.
    let (_, ap2) = l
        .req(
            "POST",
            "/api/v1/approvals",
            &thw,
            Some(json!({ "action": "hardware.delete", "target": "machine:rack1-08" })),
        )
        .await;
    let id2 = ap2["id"].as_str().expect("id").to_owned();
    assert_eq!(
        l.req(
            "POST",
            &format!("/api/v1/approvals/{id2}/reject"),
            &tfa,
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(hdr(&thw, "rack1-08", &id2).await.0, StatusCode::CONFLICT);
    let (_, ap3) = l
        .req(
            "POST",
            "/api/v1/approvals",
            &thw,
            Some(json!({ "action": "hardware.delete", "target": "machine:rack1-08" })),
        )
        .await;
    let id3 = ap3["id"].as_str().expect("id").to_owned();
    assert_eq!(
        l.req(
            "POST",
            &format!("/api/v1/approvals/{id3}/cancel"),
            &tfa,
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "only the requester withdraws"
    );
    assert_eq!(
        l.req(
            "POST",
            &format!("/api/v1/approvals/{id3}/cancel"),
            &thw,
            None
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn sensitive_cluster_changes_and_unsafe_evacuation_need_approval(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let (fa, fb) = (l.user("sub-fa", "fa").await, l.user("sub-fb", "fb").await);
    l.grant(
        &w.admin,
        fa,
        "fabric-administrator",
        json!({ "type": "platform" }),
    )
    .await;
    l.grant(
        &w.admin,
        fb,
        "fabric-administrator",
        json!({ "type": "platform" }),
    )
    .await;
    let (ta, tb) = (l.token("sub-fa", "fa"), l.token("sub-fb", "fb"));
    let with_approval = |token: String,
                         method: &'static str,
                         path: String,
                         approval: Option<String>,
                         body: serde_json::Value| {
        let state = l.state.clone();
        async move {
            let mut req = axum::http::Request::builder()
                .method(method)
                .uri(path)
                .header(header::AUTHORIZATION, format!("Bearer {token}"))
                .header(header::CONTENT_TYPE, "application/json");
            if let Some(a) = approval {
                req = req.header("apelora-approval", a);
            }
            let res = tower::ServiceExt::oneshot(
                router(state),
                req.body(axum::body::Body::from(body.to_string()))
                    .expect("request"),
            )
            .await
            .expect("response");
            let status = res.status();
            let bytes = http_body_util::BodyExt::collect(res.into_body())
                .await
                .expect("body")
                .to_bytes();
            (
                status,
                serde_json::from_slice::<serde_json::Value>(&bytes).unwrap_or_default(),
            )
        }
    };
    // Issuing a join token admits a server to the cluster: approval first.
    let body = json!({ "memberName": "mock-member-04" });
    let (s, b) = with_approval(
        ta.clone(),
        "POST",
        "/api/v1/cluster/join-tokens".into(),
        None,
        body.clone(),
    )
    .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("approval_required"))
    );
    let (_, ap) = l.req("POST", "/api/v1/approvals", &ta, Some(json!({ "action": "cluster.sensitive_change", "target": "join-token:mock-member-04" }))).await;
    let id = ap["id"].as_str().expect("id").to_owned();
    assert_eq!(
        l.req(
            "POST",
            &format!("/api/v1/approvals/{id}/approve"),
            &ta,
            None
        )
        .await
        .0,
        StatusCode::FORBIDDEN,
        "not your own request"
    );
    assert_eq!(
        l.req(
            "POST",
            &format!("/api/v1/approvals/{id}/approve"),
            &tb,
            None
        )
        .await
        .0,
        StatusCode::OK
    );
    let (s, b) = with_approval(
        ta.clone(),
        "POST",
        "/api/v1/cluster/join-tokens".into(),
        Some(id.clone()),
        body.clone(),
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    assert_eq!(
        with_approval(
            ta.clone(),
            "POST",
            "/api/v1/cluster/join-tokens".into(),
            Some(id),
            body
        )
        .await
        .0,
        StatusCode::CONFLICT,
        "single use"
    );

    // Evacuating a member is a normal operation; going ahead when it is blocked is unsafe and needs approval.
    let base = "/api/v1/cluster/members/mock-member-02";
    let (s, b) = with_approval(
        ta.clone(),
        "POST",
        format!("{base}/evacuate"),
        None,
        json!({ "confirm": "mock-member-02" }),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{b}");
    let (s, b) = with_approval(
        ta.clone(),
        "POST",
        format!("{base}/evacuate"),
        None,
        json!({ "confirm": "mock-member-02" }),
    )
    .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("evacuation_blocked")),
        "blocked without force"
    );
    let (s, b) = with_approval(
        ta.clone(),
        "POST",
        format!("{base}/evacuate"),
        None,
        json!({ "confirm": "mock-member-02", "force": true }),
    )
    .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("approval_required")),
        "forcing needs approval"
    );
    let (_, ap) = l.req("POST", "/api/v1/approvals", &ta, Some(json!({ "action": "cluster.evacuate_unsafe", "target": "evacuate:mock-member-02", "reason": "disk failing" }))).await;
    let id = ap["id"].as_str().expect("id").to_owned();
    l.req(
        "POST",
        &format!("/api/v1/approvals/{id}/approve"),
        &tb,
        None,
    )
    .await;
    let (s, b) = with_approval(
        ta,
        "POST",
        format!("{base}/evacuate"),
        Some(id),
        json!({ "confirm": "mock-member-02", "force": true }),
    )
    .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{b}");
    let events: Vec<String> = sqlx::query_scalar(
        "SELECT action FROM audit_events WHERE action LIKE 'cluster.member.evacuate%' ORDER BY id",
    )
    .fetch_all(&l.pool)
    .await
    .expect("audit");
    assert_eq!(
        events,
        ["cluster.member.evacuate", "cluster.member.evacuate.unsafe"]
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn emergency_access_is_time_limited_reasoned_audited_and_never_self_granted(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let eve = l.user("sub-eve", "eve").await;
    let te = l.token("sub-eve", "eve");
    assert_eq!(
        l.req("GET", "/api/v1/cluster", &te, None).await.0,
        StatusCode::FORBIDDEN
    );
    let grant = |body: serde_json::Value| {
        let admin = w.admin.clone();
        let l = &l;
        async move {
            l.req(
                "POST",
                "/api/v1/identity/emergency-access",
                &admin,
                Some(body),
            )
            .await
        }
    };
    for bad in [
        json!({ "userId": eve, "role": "fabric-administrator", "reason": "short", "minutes": 30 }),
        json!({ "userId": eve, "role": "fabric-administrator", "reason": "quorum lost on the cluster", "minutes": 600 }),
        json!({ "userId": eve, "role": "nonsense", "reason": "quorum lost on the cluster", "minutes": 30 }),
    ] {
        assert_eq!(grant(bad).await.0, StatusCode::BAD_REQUEST);
    }
    let root = l.user("sub-root", "root").await;
    let (s, b) = grant(json!({ "userId": root, "role": "fabric-administrator", "reason": "testing self grant", "minutes": 30 })).await;
    assert_eq!((s, code(&b)), (StatusCode::CONFLICT, Some("self_grant")));
    let (s, g) = grant(json!({ "userId": eve, "role": "fabric-administrator", "reason": "quorum lost on the cluster", "minutes": 30 })).await;
    assert_eq!(s, StatusCode::CREATED, "{g}");
    assert_eq!(
        (g["kind"].as_str(), g["reason"].as_str()),
        (Some("emergency"), Some("quorum lost on the cluster"))
    );
    assert_eq!(
        l.req("GET", "/api/v1/cluster", &te, None).await.0,
        StatusCode::OK,
        "the grant works at once"
    );
    let (_, me) = l.req("GET", "/api/v1/whoami", &te, None).await;
    assert_eq!(me["roles"][0]["emergency"], true);
    // Everything done under it is on the record; ending it ends the access.
    let used: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM audit_events WHERE action = 'emergency_access.request'",
    )
    .fetch_one(&l.pool)
    .await
    .expect("count");
    assert!(used >= 2, "{used}");
    assert_eq!(
        l.req(
            "DELETE",
            &format!(
                "/api/v1/identity/assignments/{}",
                g["id"].as_str().expect("id")
            ),
            &w.admin,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        l.req("GET", "/api/v1/cluster", &te, None).await.0,
        StatusCode::FORBIDDEN
    );
    let kinds: Vec<String> = sqlx::query_scalar("SELECT action FROM audit_events WHERE action IN ('emergency_access.grant', 'role.revoke') ORDER BY id").fetch_all(&l.pool).await.expect("audit");
    assert_eq!(kinds, ["emergency_access.grant", "role.revoke"]);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_audit_trail_records_refusals_and_is_scoped_to_what_the_reader_may_see(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let (alice, bob, aud) = (
        l.user("sub-alice", "alice").await,
        l.user("sub-bob", "bob").await,
        l.user("sub-aud", "aud").await,
    );
    l.grant(
        &w.admin,
        alice,
        "organization-owner",
        json!({ "type": "organization", "id": w.acme }),
    )
    .await;
    l.grant(
        &w.admin,
        bob,
        "organization-owner",
        json!({ "type": "organization", "id": w.other }),
    )
    .await;
    l.grant(&w.admin, aud, "auditor", json!({ "type": "platform" }))
        .await;
    let (ta, tb, tu) = (
        l.token("sub-alice", "alice"),
        l.token("sub-bob", "bob"),
        l.token("sub-aud", "aud"),
    );
    l.req(
        "POST",
        "/api/v1/applications/shop/deployments",
        &ta,
        Some(deploy_body("acme-prod-live")),
    )
    .await;
    l.req(
        "POST",
        "/api/v1/applications/mine/deployments",
        &tb,
        Some(deploy_body("other-web-live")),
    )
    .await;
    // Bob is refused (Alice's environment) and that refusal is recorded against Bob.
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/x/deployments",
            &tb,
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let denied: Vec<(String, String, Option<String>, bool)> = sqlx::query_as(
        "SELECT actor, outcome, detail->>'code', detail ? 'requestId' FROM audit_events WHERE action = 'access.denied'",
    )
    .fetch_all(&l.pool)
    .await
    .expect("audit");
    assert!(
        denied
            .iter()
            .any(|(a, o, code, rid)| a == &format!("user:{bob}")
                && o == "denied"
                && code.as_deref() == Some("forbidden")
                && *rid),
        "the refusal says why and which request: {denied:?}"
    );
    // Alice reads only her organisation's events; Bob only his; the auditor everything; a designer nothing.
    let events = |v: serde_json::Value| v["items"].as_array().expect("items").clone();
    let alice_events = events(l.req("GET", "/api/v1/audit", &ta, None).await.1);
    assert!(
        alice_events
            .iter()
            .any(|e| e["action"] == "application.deploy.requested"),
        "{alice_events:?}"
    );
    assert!(
        alice_events
            .iter()
            .all(|e| e["scope"]["organization"] == json!(w.acme)),
        "{alice_events:?}"
    );
    let bob_events = events(
        l.req(
            "GET",
            "/api/v1/identity/audit?action=application.",
            &tb,
            None,
        )
        .await
        .1,
    );
    assert_eq!(bob_events.len(), 1);
    assert_eq!(bob_events[0]["actor"], json!(format!("user:{bob}")));
    assert_eq!(bob_events[0]["actorType"], "user");
    let all = events(l.req("GET", "/api/v1/identity/audit", &tu, None).await.1);
    assert!(
        all.len() >= 6
            && all.iter().any(|e| e["outcome"] == "denied")
            && all.iter().any(|e| e["action"] == "organization.create")
    );
    let only_denied = events(
        l.req("GET", "/api/v1/identity/audit?outcome=denied", &tu, None)
            .await
            .1,
    );
    assert!(!only_denied.is_empty() && only_denied.iter().all(|e| e["outcome"] == "denied"));
    assert_eq!(
        l.req("GET", "/api/v1/identity/audit?outcome=weird", &tu, None)
            .await
            .0,
        StatusCode::BAD_REQUEST
    );
    let des = l.user("sub-des", "des").await;
    l.grant(
        &w.admin,
        des,
        "application-designer",
        json!({ "type": "project", "id": w.acme_prod }),
    )
    .await;
    assert_eq!(
        l.req("GET", "/api/v1/audit", &l.token("sub-des", "des"), None)
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    // The trail cannot be rewritten, even by SQL through the application's own connection.
    assert!(
        sqlx::query("DELETE FROM audit_events")
            .execute(&l.pool)
            .await
            .is_err()
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn mfa_policy_for_production_applies_to_signed_in_people(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let carol = l.user("sub-carol", "carol").await;
    l.grant(
        &w.admin,
        carol,
        "project-administrator",
        json!({ "type": "project", "id": w.acme_prod }),
    )
    .await;
    let (s, _) = l.req("PUT", "/api/v1/identity/policies", &w.admin, Some(json!({ "name": "Production needs MFA", "kind": "mfa", "parameters": { "environments": ["production"] } }))).await;
    assert_eq!(s, StatusCode::OK);
    for bad in [
        json!({ "name": "x", "kind": "mfa", "parameters": { "environments": ["moon"] } }),
        json!({ "name": "x", "kind": "approval", "parameters": { "actions": ["no.such"] } }),
        json!({ "name": "x", "kind": "weird", "parameters": {} }),
    ] {
        assert_eq!(
            l.req("PUT", "/api/v1/identity/policies", &w.admin, Some(bad))
                .await
                .0,
            StatusCode::BAD_REQUEST
        );
    }
    let (s, b) = l
        .req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &l.token("sub-carol", "carol"),
            Some(deploy_body("acme-prod-live")),
        )
        .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::FORBIDDEN, Some("mfa_required")),
        "{b}"
    );
    let (s, b) = l
        .req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &l.token_mfa("sub-carol", "carol"),
            Some(deploy_body("acme-prod-live")),
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{b}");
    let (_, policies) = l
        .req("GET", "/api/v1/identity/policies", &w.admin, None)
        .await;
    let id = policies["items"][0]["id"].as_str().expect("id").to_owned();
    assert_eq!(
        l.req(
            "DELETE",
            &format!("/api/v1/identity/policies/{id}"),
            &w.admin,
            None
        )
        .await
        .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/shop/deployments",
            &l.token("sub-carol", "carol"),
            Some(deploy_body("acme-prod-live"))
        )
        .await
        .0,
        StatusCode::ACCEPTED,
        "policy removed"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_preview_shows_effective_permissions_with_their_origins_before_saving(pool: Pool) {
    let l = lab(pool);
    let w = world(&l).await;
    let (carol, owner) = (
        l.user("sub-carol", "carol").await,
        l.user("sub-own", "owner").await,
    );
    l.grant(
        &w.admin,
        owner,
        "organization-owner",
        json!({ "type": "organization", "id": w.acme }),
    )
    .await;
    l.grant(
        &w.admin,
        carol,
        "viewer",
        json!({ "type": "organization", "id": w.acme }),
    )
    .await;
    let preview = |by: &str, role: &str, scope: serde_json::Value| {
        let by = by.to_owned();
        let body = json!({ "principal": { "type": "user", "id": carol }, "role": { "type": "builtin", "key": role }, "scope": scope, "conditions": { "requireMfa": true } });
        let l = &l;
        async move {
            l.req(
                "POST",
                "/api/v1/identity/assignments/preview",
                &by,
                Some(body),
            )
            .await
        }
    };
    let (s, p) = preview(
        &l.token("sub-own", "owner"),
        "application-operator",
        json!({ "type": "project", "id": w.acme_prod }),
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{p}");
    assert_eq!(p["allowed"], true);
    let before: Vec<&str> = p["before"]
        .as_array()
        .expect("before")
        .iter()
        .map(|e| e["permission"].as_str().unwrap_or(""))
        .collect();
    assert!(
        before.contains(&"application.view") && !before.contains(&"application.deploy"),
        "{before:?}"
    );
    assert!(
        p["added"]
            .as_array()
            .expect("added")
            .iter()
            .any(|a| a == "application.deploy")
    );
    let after = p["after"].as_array().expect("after");
    let view = after
        .iter()
        .find(|e| e["permission"] == "application.view")
        .expect("view");
    assert_eq!(
        view["grants"].as_array().map(Vec::len),
        Some(2),
        "granted by the viewer role and by the operator role: both origins are shown"
    );
    assert!(
        view["grants"]
            .as_array()
            .expect("grants")
            .iter()
            .any(|g| g["roleName"] == "Viewer" && g["scope"]["type"] == "organization")
    );
    assert!(
        p["warnings"]
            .as_array()
            .expect("warnings")
            .iter()
            .any(|x| x.as_str().is_some_and(|t| t.contains("multi-factor")))
    );
    // The same preview for an infrastructure role says no, and why.
    let (s, p) = preview(
        &l.token("sub-own", "owner"),
        "fabric-administrator",
        json!({ "type": "platform" }),
    )
    .await;
    assert_eq!(
        (s, p["allowed"].clone()),
        (StatusCode::FORBIDDEN, serde_json::Value::Null),
        "someone who cannot grant at a scope may not even preview it: {p}"
    );
    let (s, p) = preview(
        &w.admin,
        "viewer",
        json!({ "type": "application", "id": Uuid::new_v4() }),
    )
    .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "an unknown scope looks like a forbidden one: {p}"
    );
    let (s, p) = preview(
        &w.admin,
        "application-operator",
        json!({ "type": "platform" }),
    )
    .await;
    assert_eq!(
        (s, p["allowed"].clone()),
        (StatusCode::OK, json!(false)),
        "operators cannot be assigned platform-wide: {p}"
    );
    assert!(
        p["error"]
            .as_str()
            .is_some_and(|e| e.contains("cannot be assigned"))
    );
    // Nothing was saved by previewing.
    let (_, mine) = l
        .req(
            "GET",
            &format!("/api/v1/identity/assignments?principalType=user&principalId={carol}"),
            &w.admin,
            None,
        )
        .await;
    assert_eq!(mine["items"].as_array().map(Vec::len), Some(1));
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_application_of_end_users_is_separate_from_apelora_access(pool: Pool) {
    // A token issued for an application's own users (another audience) never opens the Apelora API,
    // even from the same identity provider and for the same person.
    let l = lab(pool);
    let _ = world(&l).await;
    let mut portal = l.claims("sub-customer", "customer", &[], true);
    portal["aud"] = json!("portal.example.com");
    let t = l.sign(&portal, Some("k1"));
    let (s, b) = l.req("GET", "/api/v1/whoami", &t, None).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::UNAUTHORIZED, Some("invalid_token")),
        "{b}"
    );
    assert_eq!(
        db::list_users(&l.pool, &apelora_auth::AccessFilter::All)
            .await
            .expect("users")
            .iter()
            .filter(|u| u.subject == "sub-customer")
            .count(),
        0,
        "no Apelora account is created for application users"
    );
    let _ = Arc::new(l.jwks.clone());
}

/// A real identity provider over HTTP on this computer: its discovery document and key set,
/// as the control plane reads them when a provider is saved in the Console.
async fn local_provider(jwks: jsonwebtoken::jwk::JwkSet) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let issuer = format!("http://{}", listener.local_addr().expect("addr"));
    let doc = json!({ "issuer": issuer, "authorization_endpoint": format!("{issuer}/auth"), "token_endpoint": format!("{issuer}/token"), "jwks_uri": format!("{issuer}/keys") });
    let app = axum::Router::new()
        .route(
            "/.well-known/openid-configuration",
            axum::routing::get(move || {
                let d = doc.clone();
                async move { axum::Json(d) }
            }),
        )
        .route(
            "/keys",
            axum::routing::get(move || {
                let k = jwks.clone();
                async move { axum::Json(k) }
            }),
        );
    tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
    issuer
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn identity_providers_are_configured_in_the_console_applied_at_once_and_nobody_is_locked_out(
    pool: Pool,
) {
    let l = lab(pool.clone());
    // A control plane with no provider in its environment: everything comes from the Console.
    let state = AppState::new(
        Provider::Mock(MockIncus::default()),
        Auth::Tokens(pool.clone()),
    );
    let req = |token: String,
               method: &'static str,
               path: &'static str,
               body: Option<serde_json::Value>| {
        let state = state.clone();
        async move { send(state, method, path, &token, body, None).await }
    };
    let admin = apelora_persistence::create_token(&pool, "admin", "fabric-admin", None)
        .await
        .expect("token")
        .0;
    let issuer = local_provider(l.jwks.clone()).await;
    let sign = |sub: &str, iss: &str| {
        l.sign(&json!({ "iss": iss, "aud": "apelora-api", "sub": sub, "iat": now() - 5, "exp": now() + 600, "preferred_username": sub }), Some("k1"))
    };

    let (status, s) = req(admin.clone(), "GET", "/api/v1/identity/sign-in", None).await;
    assert_eq!(status, StatusCode::OK, "{s}");
    assert_eq!(
        (
            s["providers"].as_array().map(Vec::len),
            s["apiTokens"]["enabled"].as_bool()
        ),
        (Some(0), Some(true))
    );
    let (status, _) = req(sign("alex", &issuer), "GET", "/api/v1/whoami", None).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED, "no provider yet");

    // Check first: what the provider publishes.
    let (status, c) = req(
        admin.clone(),
        "POST",
        "/api/v1/identity/sign-in/check",
        Some(json!({ "issuer": issuer })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{c}");
    assert_eq!(c["keys"], 1);
    let (status, _) = req(
        admin.clone(),
        "POST",
        "/api/v1/identity/sign-in/check",
        Some(json!({ "issuer": "http://id.example.test" })),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    // Save it: tokens it issues are accepted at once, others are not.
    let (status, p) = req(admin.clone(), "POST", "/api/v1/identity/sign-in/providers",
        Some(json!({ "name": "Lab IdP", "issuer": issuer, "audiences": ["apelora-api"], "clientId": "apelora-console" }))).await;
    assert_eq!(status, StatusCode::CREATED, "{p}");
    assert_eq!(
        (p["reachable"].as_bool(), p["source"].as_str()),
        (Some(true), Some("console"))
    );
    let id = p["id"].as_str().expect("id").to_owned();
    let (status, me) = req(sign("alex", &issuer), "GET", "/api/v1/whoami", None).await;
    assert_eq!(status, StatusCode::OK, "{me}");
    let (status, e) = req(
        sign("alex", "https://other.example.test"),
        "GET",
        "/api/v1/whoami",
        None,
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(
        e["error"]["message"]
            .as_str()
            .unwrap_or_default()
            .contains("no identity provider set up here"),
        "{e}"
    );
    let (_, opts) = send(state.clone(), "GET", "/api/v1/sign-in", "", None, None).await;
    assert_eq!(opts["providers"][0]["label"], "Lab IdP", "{opts}");

    // Turning API tokens off: not by someone signed in with one.
    let (status, e) = req(
        admin.clone(),
        "PUT",
        "/api/v1/identity/sign-in/api-tokens",
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "{e}");
    // A super admin signed in through the provider may.
    let me_id: Uuid = me["id"].as_str().expect("id").parse().expect("uuid");
    db::insert_assignment(
        &pool,
        &new_assignment(me_id, Builtin::PlatformSuperAdmin, Scope::Platform),
    )
    .await
    .expect("assign");
    let person = sign("alex", &issuer);
    let (status, t) = req(
        person.clone(),
        "PUT",
        "/api/v1/identity/sign-in/api-tokens",
        Some(json!({ "enabled": false })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{t}");
    assert_eq!(t["enabled"], false);
    let (status, _) = req(admin.clone(), "GET", "/api/v1/cluster", None).await;
    assert_eq!(
        status,
        StatusCode::UNAUTHORIZED,
        "API tokens are refused now"
    );

    // The last way in cannot be disabled or removed.
    let path = format!("/api/v1/identity/sign-in/providers/{id}");
    let put = send(state.clone(), "PUT", &path, &person, Some(json!({ "name": "Lab IdP", "issuer": issuer, "audiences": ["apelora-api"], "clientId": "apelora-console", "enabled": false })), None).await;
    assert_eq!(put.0, StatusCode::CONFLICT, "{}", put.1);
    let del = send(
        state.clone(),
        "DELETE",
        &path,
        &person,
        Some(json!({ "confirm": "Lab IdP" })),
        None,
    )
    .await;
    assert_eq!(del.0, StatusCode::CONFLICT, "{}", del.1);
    // Renaming works and is applied.
    let put = send(state.clone(), "PUT", &path, &person, Some(json!({ "name": "Lab sign-in", "issuer": issuer, "audiences": ["apelora-api"], "clientId": "apelora-console" })), None).await;
    assert_eq!(put.0, StatusCode::OK, "{}", put.1);
    let audited: i64 = sqlx::query_scalar("SELECT count(*) FROM audit_events WHERE action IN ('identity.provider.saved', 'identity.api_tokens.disabled')")
        .fetch_one(&pool)
        .await
        .expect("audit");
    assert_eq!(audited, 3);
}
