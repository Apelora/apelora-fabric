// Application management through the real router, against PostgreSQL and the mock Incus:
// creating applications, drafts, published versions, derived status, permitted actions and the
// start / stop / restart operations. The worker runs in-process, with the coordinator's waits
// skipped (time passing is simulated).

use apelora_incus_adapter::MockIncus;
use serde_json::{Value, json};
use uuid::Uuid;

use super::identity_tests::{Lab, World, code, lab, world};
use super::*;

pub(super) fn mock_lab(pool: Pool) -> (Lab, MockIncus) {
    let mock = MockIncus::default();
    let mut l = lab(pool);
    l.state = l
        .state
        .clone()
        .with_admin(Some(Provider::Mock(mock.clone())))
        .with_network_uplink(Some("incusbr0".into()));
    l.state = AppState {
        provider: std::sync::Arc::new(Provider::Mock(mock.clone())),
        ..l.state.clone()
    };
    (l, mock)
}

/// Runs the worker until nothing is due, skipping waits only when nothing else is runnable.
pub(super) async fn settle(l: &Lab) {
    for _ in 0..600 {
        if apelora_deployment_worker::run_once(&l.pool, &l.state.provider, "w")
            .await
            .expect("run")
        {
            continue;
        }
        let skipped = sqlx::query(
            "UPDATE jobs SET run_after = now() WHERE status = 'pending' AND run_after > now()",
        )
        .execute(&l.pool)
        .await
        .expect("wait")
        .rows_affected();
        if skipped == 0 {
            break;
        }
    }
}

fn doc() -> Value {
    json!({ "apiVersion": "fabric.apelora/v1alpha1", "kind": "ApplicationAssembly", "metadata": { "name": "x", "uid": "u" },
        "spec": { "modules": [
            { "id": "web-01", "ports": [{ "id": "http", "direction": "input" }, { "id": "db", "direction": "output" }] },
            { "id": "db-01", "ports": [{ "id": "sql", "direction": "input" }] } ],
          "connections": [{ "id": "conn-01", "source": { "moduleId": "web-01", "portId": "db" }, "target": { "moduleId": "db-01", "portId": "sql" } }] },
        "editor": { "layoutVersion": 1, "density": "compact", "nodes": [] } })
}

fn module(id: &str, availability: &str) -> Value {
    json!({ "module": id, "availability": availability, "spec": {
        "project": "ignored", "name": "ignored", "kind": "container", "start": true,
        "source": { "protocol": "oci", "server": "https://registry.example", "alias": "nginx:1.29" } } })
}

async fn env_id(l: &Lab, w: &World, project: Uuid) -> Uuid {
    let (s, b) = l
        .req(
            "GET",
            &format!("/api/v1/identity/projects/{project}/environments"),
            &w.admin,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK, "{b}");
    b["items"][0]["id"]
        .as_str()
        .expect("env")
        .parse()
        .expect("uuid")
}

pub(super) struct Shop {
    pub(super) w: World,
    pub(super) owner: String,
    pub(super) env: Uuid,
}

/// Two organisations; an organisation owner in the first, who creates application `shop` with a draft.
pub(super) async fn shop(l: &Lab) -> Shop {
    let w = world(l).await;
    let alice = l.user("sub-alice", "alice").await;
    l.grant(
        &w.admin,
        alice,
        "organization-owner",
        json!({ "type": "organization", "id": w.acme }),
    )
    .await;
    let env = env_id(l, &w, w.acme_prod).await;
    let owner = l.token("sub-alice", "alice");
    let (s, b) = l.req("POST", "/api/v1/applications", &owner, Some(json!({ "name": "shop", "environmentId": env, "description": "The web shop", "icon": "nginx", "document": doc() }))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    Shop { w, owner, env }
}

async fn deploy_version(l: &Lab, token: &str, version: Option<i32>) -> (StatusCode, Value) {
    let mut body = json!({ "project": "acme-prod-live", "document": {}, "modules": [module("web-01", "stateless"), module("db-01", "stateful")],
        "connections": [{ "from": "web-01", "to": "db-01", "transport": "tcp", "port": 5432 }] });
    if let Some(v) = version {
        body["version"] = json!(v);
    }
    l.req(
        "POST",
        "/api/v1/applications/shop/deployments",
        token,
        Some(body),
    )
    .await
}

/// Publishes the draft and deploys that version; returns once the instances exist.
pub(super) async fn deployed(l: &Lab, s: &Shop) {
    let (st, v) = l
        .req(
            "POST",
            "/api/v1/applications/shop/versions",
            &s.owner,
            Some(json!({ "label": "first" })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    let (st, d) = deploy_version(
        l,
        &s.owner,
        Some(v["revision"].as_i64().expect("rev") as i32),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{d}");
    settle(l).await;
}

async fn info(l: &Lab, token: &str) -> Value {
    let (s, b) = l.req("GET", "/api/v1/applications/shop", token, None).await;
    assert_eq!(s, StatusCode::OK, "{b}");
    b["application"].clone()
}

async fn operate(l: &Lab, token: &str, action: &str, confirm: bool) -> (StatusCode, Value) {
    l.req(
        "POST",
        "/api/v1/applications/shop/operations",
        token,
        Some(json!({ "action": action, "confirmDisruption": confirm })),
    )
    .await
}

fn actions(i: &Value) -> (bool, bool, bool) {
    let a = &i["actions"];
    (
        a["start"]["allowed"] == true,
        a["stop"]["allowed"] == true,
        a["restart"]["allowed"] == true,
    )
}

// ---- creating and describing ------------------------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn creating_an_application_records_it_in_its_project_with_a_draft_and_validates_the_input(
    pool: Pool,
) {
    let (l, _) = mock_lab(pool);
    let s = shop(&l).await;
    let i = info(&l, &s.owner).await;
    assert_eq!(
        (
            i["name"].as_str(),
            i["description"].as_str(),
            i["icon"].as_str()
        ),
        (Some("shop"), Some("The web shop"), Some("nginx"))
    );
    assert_eq!(
        (
            i["project"]["slug"].as_str(),
            i["environment"]["kind"].as_str(),
            i["environment"]["incusProject"].as_str()
        ),
        (Some("prod"), Some("production"), Some("acme-prod-live"))
    );
    assert_eq!(
        (
            i["design"]["status"].as_str(),
            i["design"]["draftVersion"].as_i64()
        ),
        (Some("validated"), Some(1))
    );
    assert_eq!(i["deployment"]["status"], "not-deployed");
    assert_eq!(
        actions(&i),
        (false, false, false),
        "nothing can be started before a deployment"
    );
    assert_eq!(
        i["actions"]["start"]["reason"],
        "deploy a published version first"
    );

    let make = |body: Value| {
        let (l, owner) = (&l, s.owner.clone());
        async move {
            l.req("POST", "/api/v1/applications", &owner, Some(body))
                .await
        }
    };
    assert_eq!(
        make(json!({ "name": "shop", "environmentId": s.env }))
            .await
            .0,
        StatusCode::CONFLICT,
        "names are unique"
    );
    for bad in [
        json!({ "name": "Bad Name", "environmentId": s.env }),
        json!({ "name": "x".repeat(41), "environmentId": s.env }),
        json!({ "name": "ok-name", "environmentId": s.env, "icon": "Not An Icon" }),
        json!({ "name": "ok-name", "environmentId": s.env, "description": "d".repeat(201) }),
    ] {
        assert_eq!(make(bad).await.0, StatusCode::BAD_REQUEST);
    }
    assert_eq!(
        make(json!({ "name": "ok-name", "environmentId": Uuid::new_v4() }))
            .await
            .0,
        StatusCode::FORBIDDEN,
        "an unknown environment looks like a forbidden one"
    );
    let other_env = env_id(&l, &s.w, s.w.other_proj).await;
    assert_eq!(
        make(json!({ "name": "intruder", "environmentId": other_env }))
            .await
            .0,
        StatusCode::FORBIDDEN,
        "another organisation's environment"
    );
    // A viewer cannot create; a designer can; both can see.
    let (viewer, designer) = (
        l.user("sub-v", "viewer").await,
        l.user("sub-d", "designer").await,
    );
    l.grant(
        &s.w.admin,
        viewer,
        "viewer",
        json!({ "type": "project", "id": s.w.acme_prod }),
    )
    .await;
    l.grant(
        &s.w.admin,
        designer,
        "application-designer",
        json!({ "type": "project", "id": s.w.acme_prod }),
    )
    .await;
    let tv = l.token("sub-v", "viewer");
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications",
            &tv,
            Some(json!({ "name": "by-viewer", "environmentId": s.env }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    let td = l.token("sub-d", "designer");
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications",
            &td,
            Some(json!({ "name": "by-designer", "environmentId": s.env }))
        )
        .await
        .0,
        StatusCode::CREATED
    );
    // The icon and description can change, the name cannot; viewers cannot change anything.
    let (st, b) = l
        .req(
            "PATCH",
            "/api/v1/applications/shop",
            &td,
            Some(json!({ "icon": "postgresql", "description": "Orders and carts" })),
        )
        .await;
    assert_eq!(
        (st, b["icon"].as_str(), b["description"].as_str()),
        (StatusCode::OK, Some("postgresql"), Some("Orders and carts")),
        "{b}"
    );
    assert_eq!(
        l.req(
            "PATCH",
            "/api/v1/applications/shop",
            &tv,
            Some(json!({ "icon": "redis" }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        l.req(
            "PATCH",
            "/api/v1/applications/shop",
            &td,
            Some(json!({ "icon": "Bad Icon" }))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let events = sqlx::query_scalar::<_, String>("SELECT action FROM audit_events WHERE action LIKE 'application.%' AND outcome = 'success' ORDER BY id").fetch_all(&l.pool).await.expect("audit");
    assert!(
        events.contains(&"application.create".to_string())
            && events.contains(&"application.update".to_string()),
        "{events:?}"
    );
}

// ---- drafts, design status and versions -------------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn drafts_are_versioned_never_overwritten_and_drive_the_design_status(pool: Pool) {
    let (l, _) = mock_lab(pool);
    let s = shop(&l).await;
    let put = |base: Option<i32>, document: Value| {
        let (l, owner) = (&l, s.owner.clone());
        async move {
            l.req(
                "PUT",
                "/api/v1/applications/shop/draft",
                &owner,
                Some(json!({ "document": document, "baseVersion": base })),
            )
            .await
        }
    };
    // Version 1 exists from creation; saving from a stale base is refused and changes nothing.
    let (st, b) = put(Some(1), doc()).await;
    assert_eq!(
        (st, b["version"].as_i64()),
        (StatusCode::OK, Some(2)),
        "{b}"
    );
    let mut edited = doc();
    edited["metadata"]["name"] = json!("edited");
    let (st, b) = put(Some(1), edited.clone()).await;
    assert_eq!(
        (st, code(&b), b["currentVersion"].as_i64()),
        (StatusCode::CONFLICT, Some("draft_conflict"), Some(2)),
        "{b}"
    );
    let (_, now) = l
        .req("GET", "/api/v1/applications/shop/draft", &s.owner, None)
        .await;
    assert_eq!(
        (
            now["version"].as_i64(),
            now["document"]["metadata"]["name"].as_str()
        ),
        (Some(2), Some("x")),
        "the refused save wrote nothing"
    );
    assert_eq!(
        put(None, doc()).await.0,
        StatusCode::CONFLICT,
        "the first-save form is refused once a draft exists"
    );

    // Design status: validated → published → validated with unpublished changes → draft when invalid.
    assert_eq!(info(&l, &s.owner).await["design"]["status"], "validated");
    let (st, v) = l
        .req(
            "POST",
            "/api/v1/applications/shop/versions",
            &s.owner,
            Some(json!({ "label": "v1" })),
        )
        .await;
    assert_eq!(
        (
            st,
            v["revision"].as_i64(),
            v["published"].as_bool(),
            v["draftVersion"].as_i64()
        ),
        (StatusCode::CREATED, Some(1), Some(true), Some(2)),
        "{v}"
    );
    let i = info(&l, &s.owner).await;
    assert_eq!(
        (
            i["design"]["status"].as_str(),
            i["design"]["publishedVersion"].as_i64(),
            i["design"]["hasUnpublishedChanges"].as_bool()
        ),
        (Some("published"), Some(1), Some(false))
    );
    let (st, b) = l
        .req(
            "POST",
            "/api/v1/applications/shop/versions",
            &s.owner,
            Some(json!({})),
        )
        .await;
    assert_eq!(
        (st, code(&b)),
        (StatusCode::CONFLICT, Some("already_published")),
        "the same draft is not published twice"
    );
    let (st, _) = put(Some(2), edited).await;
    assert_eq!(st, StatusCode::OK);
    let i = info(&l, &s.owner).await;
    assert_eq!(
        (
            i["design"]["status"].as_str(),
            i["design"]["hasUnpublishedChanges"].as_bool()
        ),
        (Some("validated"), Some(true))
    );
    let mut broken = doc();
    broken["spec"]["connections"][0]["target"]["portId"] = json!("nope");
    let (st, b) = put(Some(3), broken).await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert!(
        b["validationErrors"]
            .as_array()
            .is_some_and(|e| !e.is_empty()),
        "{b}"
    );
    assert_eq!(
        info(&l, &s.owner).await["design"]["status"],
        "draft",
        "an invalid draft is a draft"
    );
    let (st, b) = l
        .req(
            "POST",
            "/api/v1/applications/shop/versions",
            &s.owner,
            Some(json!({})),
        )
        .await;
    assert_eq!(
        (st, code(&b)),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("draft_invalid")),
        "{b}"
    );
    // Versions are immutable copies; the published one still has the original document.
    let (_, v1) = l
        .req(
            "GET",
            "/api/v1/applications/shop/versions/1",
            &s.owner,
            None,
        )
        .await;
    assert_eq!(
        (
            v1["published"].as_bool(),
            v1["document"]["metadata"]["name"].as_str()
        ),
        (Some(true), Some("x"))
    );
    // Bad documents are refused outright.
    assert_eq!(
        put(Some(4), json!([1, 2])).await.1["validationErrors"][0],
        "the document must be a JSON object"
    );
    assert_eq!(
        l.req(
            "PUT",
            "/api/v1/applications/shop/draft",
            &s.owner,
            Some(json!({ "document": "x".repeat(2_100_000), "baseVersion": 4 }))
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn each_application_has_its_own_draft_and_opening_one_never_changes_another(pool: Pool) {
    let (l, _) = mock_lab(pool);
    let s = shop(&l).await;
    let mut other = doc();
    other["metadata"]["name"] = json!("blog-design");
    let (st, _) = l
        .req(
            "POST",
            "/api/v1/applications",
            &s.owner,
            Some(json!({ "name": "blog", "environmentId": s.env, "document": other })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let name = |app: &str| {
        let (l, owner) = (&l, s.owner.clone());
        let app = app.to_owned();
        async move {
            l.req(
                "GET",
                &format!("/api/v1/applications/{app}/draft"),
                &owner,
                None,
            )
            .await
            .1["document"]["metadata"]["name"]
                .clone()
        }
    };
    assert_eq!(
        (name("shop").await, name("blog").await),
        (json!("x"), json!("blog-design"))
    );
    let mut edit = doc();
    edit["metadata"]["name"] = json!("shop-edited");
    let (st, _) = l
        .req(
            "PUT",
            "/api/v1/applications/shop/draft",
            &s.owner,
            Some(json!({ "document": edit, "baseVersion": 1 })),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(
        (name("shop").await, name("blog").await),
        (json!("shop-edited"), json!("blog-design")),
        "saving shop leaves blog alone"
    );
    let (_, listed) = l.req("GET", "/api/v1/applications", &s.owner, None).await;
    assert_eq!(listed["items"].as_array().map(Vec::len), Some(2));
}

// ---- deploying a version ----------------------------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn only_a_published_version_can_be_deployed_and_deploying_does_not_create_revisions(
    pool: Pool,
) {
    let (l, _) = mock_lab(pool);
    let s = shop(&l).await;
    let (st, b) = deploy_version(&l, &s.owner, Some(1)).await;
    assert_eq!(
        (st, code(&b)),
        (StatusCode::NOT_FOUND, Some("not_found")),
        "a draft is not a version: {b}"
    );
    let (st, v) = l
        .req(
            "POST",
            "/api/v1/applications/shop/versions",
            &s.owner,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, b) = deploy_version(&l, &s.owner, Some(9)).await;
    assert_eq!((st, code(&b)), (StatusCode::NOT_FOUND, Some("not_found")));
    // An unpublished revision (forced for the test) is refused.
    sqlx::query("UPDATE application_revisions SET published = false")
        .execute(&l.pool)
        .await
        .expect("unpublish");
    let (st, b) = deploy_version(&l, &s.owner, v["revision"].as_i64().map(|r| r as i32)).await;
    assert_eq!(
        (st, code(&b)),
        (StatusCode::CONFLICT, Some("not_published")),
        "{b}"
    );
    sqlx::query("UPDATE application_revisions SET published = true")
        .execute(&l.pool)
        .await
        .expect("publish");
    let (st, b) = deploy_version(&l, &s.owner, Some(1)).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{b}");
    assert_eq!(b["revision"], 1);
    let revisions: i64 = sqlx::query_scalar("SELECT count(*) FROM application_revisions")
        .fetch_one(&l.pool)
        .await
        .expect("count");
    assert_eq!(revisions, 1, "deploying version 1 made no new revision");
    settle(&l).await;
    let i = info(&l, &s.owner).await;
    assert_eq!(
        (
            i["deployment"]["deployedVersion"].as_i64(),
            i["deployment"]["status"].as_str()
        ),
        (Some(1), Some("running")),
        "{i}"
    );
    assert_eq!(
        (
            i["deployment"]["replicas"]["desired"].as_u64(),
            i["deployment"]["replicas"]["running"].as_u64(),
            i["deployment"]["replicas"]["healthy"].as_u64()
        ),
        (Some(2), Some(2), Some(2))
    );
    assert_eq!(
        i["deployment"]["source"], "mock",
        "mock data is labelled as such"
    );
    assert!(
        i["deployment"]["observedAt"]
            .as_str()
            .is_some_and(|t| t.ends_with('Z'))
    );
    assert_eq!(i["deployment"]["stale"], false);
}

// ---- start, stop, restart ---------------------------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_running_application_offers_stop_only_and_a_stopped_one_offers_start_only(pool: Pool) {
    let (l, mock) = mock_lab(pool);
    let s = shop(&l).await;
    deployed(&l, &s).await;
    let i = info(&l, &s.owner).await;
    assert_eq!(i["deployment"]["status"], "running");
    assert_eq!(
        actions(&i),
        (false, true, false),
        "running: Stop is available; Start and Restart are greyed out"
    );
    assert!(
        i["actions"]["start"]["reason"]
            .as_str()
            .is_some_and(|r| r.contains("already running"))
    );

    // The server enforces the same rules the buttons show.
    let (st, b) = operate(&l, &s.owner, "start", false).await;
    assert_eq!(
        (st, code(&b)),
        (StatusCode::CONFLICT, Some("already_running"))
    );
    let (st, b) = operate(&l, &s.owner, "restart", true).await;
    assert_eq!(
        (st, code(&b)),
        (StatusCode::CONFLICT, Some("action_not_available")),
        "{b}"
    );
    // Stopping a stateful database interrupts clients: it needs an explicit confirmation.
    let (st, b) = operate(&l, &s.owner, "stop", false).await;
    assert_eq!(
        (st, code(&b)),
        (StatusCode::CONFLICT, Some("confirmation_required")),
        "{b}"
    );
    assert!(
        b["error"]["message"]
            .as_str()
            .is_some_and(|m| m.contains("db-01 (stateful)")),
        "{b}"
    );
    assert_eq!(
        info(&l, &s.owner).await["deployment"]["status"],
        "running",
        "nothing changed"
    );

    let (st, b) = operate(&l, &s.owner, "stop", true).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{b}");
    let (op_id, created) = (
        b["operation"]["id"].as_str().expect("id").to_owned(),
        b["created"].clone(),
    );
    assert_eq!(
        (created, b["operation"]["state"].as_str()),
        (json!(true), Some("pending"))
    );
    let traced: bool = sqlx::query_scalar(
        "SELECT request_id IS NOT NULL FROM jobs
         WHERE kind = 'application.operation' AND payload->>'operation' = $1",
    )
    .bind(&op_id)
    .fetch_one(&l.pool)
    .await
    .expect("coordinating job");
    assert!(
        traced,
        "the operation's job names the request that asked for it"
    );
    // While it is in progress the application is "stopping", every action is disabled, and a second request is refused.
    let mid = info(&l, &s.owner).await;
    assert_eq!(mid["deployment"]["status"], "stopping");
    assert_eq!(actions(&mid), (false, false, false));
    let (st, b2) = operate(&l, &s.owner, "stop", true).await;
    assert_eq!(
        (st, code(&b2)),
        (StatusCode::CONFLICT, Some("operation_in_progress"))
    );
    // The browser's picture is pending until the backend confirms: Incus still reports Running.
    assert!(
        mock.instances
            .lock()
            .expect("lock")
            .iter()
            .all(|i| i.instance.status == "Running")
    );

    settle(&l).await;
    let (_, done) = l
        .req(
            "GET",
            &format!("/api/v1/applications/shop/operations/{op_id}"),
            &s.owner,
            None,
        )
        .await;
    assert_eq!(
        (
            done["state"].as_str(),
            done["stepsDone"].as_u64(),
            done["stepsTotal"].as_u64()
        ),
        (Some("succeeded"), Some(2), Some(2)),
        "{done}"
    );
    // Dependency order: the web tier (a consumer) stops before the database.
    assert_eq!(done["phases"][0]["steps"][0]["module"], "web-01");
    assert_eq!(done["phases"][1]["steps"][0]["module"], "db-01");
    let after = info(&l, &s.owner).await;
    assert_eq!(after["deployment"]["status"], "stopped");
    assert_eq!(actions(&after), (true, false, false), "stopped: only Start");
    assert_eq!(after["deployment"]["operation"]["state"], "succeeded");
    // Data and definitions survive: the instances still exist, only stopped.
    assert_eq!(
        mock.instances
            .lock()
            .expect("lock")
            .iter()
            .filter(|i| i.instance.status == "Stopped")
            .count(),
        2
    );
    let (st, b) = operate(&l, &s.owner, "stop", true).await;
    assert_eq!(
        (st, code(&b)),
        (StatusCode::CONFLICT, Some("already_stopped"))
    );

    // Start: providers first (database, then web), and it needs no confirmation.
    let (st, b) = operate(&l, &s.owner, "start", false).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{b}");
    settle(&l).await;
    let (_, ops) = l
        .req(
            "GET",
            "/api/v1/applications/shop/operations",
            &s.owner,
            None,
        )
        .await;
    let start = &ops["items"][0];
    assert_eq!(
        (start["kind"].as_str(), start["state"].as_str()),
        (Some("start"), Some("succeeded"))
    );
    assert_eq!(
        (
            start["phases"][0]["steps"][0]["module"].as_str(),
            start["phases"][1]["steps"][0]["module"].as_str()
        ),
        (Some("db-01"), Some("web-01"))
    );
    let running = info(&l, &s.owner).await;
    assert_eq!(running["deployment"]["status"], "running");
    assert_eq!(actions(&running), (false, true, false));
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_degraded_application_can_be_restarted_and_nothing_is_reported_healthy_by_guesswork(
    pool: Pool,
) {
    let (l, mock) = mock_lab(pool);
    let s = shop(&l).await;
    deployed(&l, &s).await;
    // Something outside Apelora stops the web tier: the supervisor would restart a replica stopped
    // outside Apelora, so freeze the picture first by marking the instance stopped directly.
    mock.instances
        .lock()
        .expect("lock")
        .iter_mut()
        .find(|i| i.instance.name == "shop-web-01")
        .expect("web")
        .instance
        .status = "Stopped".into();
    let i = info(&l, &s.owner).await;
    assert_eq!(i["deployment"]["status"], "degraded");
    assert_eq!(
        (
            i["deployment"]["replicas"]["running"].as_u64(),
            i["deployment"]["replicas"]["desired"].as_u64()
        ),
        (Some(1), Some(2))
    );
    assert_eq!(
        actions(&i),
        (false, true, true),
        "degraded: Restart to recover, or Stop; not Start"
    );
    let (st, b) = operate(&l, &s.owner, "restart", true).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{b}");
    let phases: Vec<&str> = b["operation"]["phases"]
        .as_array()
        .expect("phases")
        .iter()
        .map(|p| p["action"].as_str().unwrap_or(""))
        .collect();
    assert_eq!(
        phases,
        ["stop", "start"],
        "only what was running is cycled: the database stops and starts again"
    );
    settle(&l).await;
    let (_, ops) = l
        .req(
            "GET",
            "/api/v1/applications/shop/operations",
            &s.owner,
            None,
        )
        .await;
    assert_eq!(ops["items"][0]["state"], "succeeded", "{ops}");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn an_unreadable_incus_makes_status_unknown_and_refuses_to_act(pool: Pool) {
    let (l, _) = mock_lab(pool);
    let s = shop(&l).await;
    deployed(&l, &s).await;
    // The same database and sign-in, but the control plane cannot reach Incus.
    let broken = AppState {
        provider: std::sync::Arc::new(Provider::Incus(
            apelora_incus_adapter::IncusClient::plain_for_tests("http://127.0.0.1:1"),
        )),
        ..l.state.clone()
    };
    let get = |path: &'static str| {
        let (state, token) = (broken.clone(), s.owner.clone());
        async move { super::tests::send(state, "GET", path, &token, None, None).await }
    };
    let (st, b) = get("/api/v1/applications/shop").await;
    assert_eq!(st, StatusCode::OK, "{b}");
    assert_eq!(
        (
            b["application"]["deployment"]["status"].as_str(),
            b["application"]["deployment"]["stale"].as_bool()
        ),
        (Some("unknown"), Some(true)),
        "{b}"
    );
    assert_eq!(
        actions(&b["application"]),
        (false, false, false),
        "no actions on a guess"
    );
    let (_, list) = get("/api/v1/applications").await;
    assert_eq!(
        (
            list["stale"].as_bool(),
            list["items"][0]["deployment"]["status"].as_str()
        ),
        (Some(true), Some("unknown"))
    );
    let (st, b) = super::tests::send(
        broken,
        "POST",
        "/api/v1/applications/shop/operations",
        &s.owner,
        Some(json!({ "action": "stop", "confirmDisruption": true })),
        None,
    )
    .await;
    assert_eq!(
        (st, code(&b)),
        (StatusCode::SERVICE_UNAVAILABLE, Some("incus_unreachable")),
        "{b}"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_draft_that_was_never_deployed_cannot_be_started_and_only_its_own_instances_are_touched(
    pool: Pool,
) {
    let (l, mock) = mock_lab(pool);
    let s = shop(&l).await;
    let (st, b) = operate(&l, &s.owner, "start", false).await;
    assert_eq!(
        (st, code(&b)),
        (StatusCode::CONFLICT, Some("not_deployed")),
        "{b}"
    );
    deployed(&l, &s).await;
    // A second application in the same environment, and a foreign instance nobody owns.
    let (st, _) = l
        .req(
            "POST",
            "/api/v1/applications",
            &s.owner,
            Some(json!({ "name": "blog", "environmentId": s.env, "document": doc() })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, v) = l
        .req(
            "POST",
            "/api/v1/applications/blog/versions",
            &s.owner,
            Some(json!({})),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    let body = json!({ "project": "acme-prod-live", "document": {}, "version": 1, "modules": [module("web-01", "stateless")] });
    assert_eq!(
        l.req(
            "POST",
            "/api/v1/applications/blog/deployments",
            &s.owner,
            Some(body)
        )
        .await
        .0,
        StatusCode::ACCEPTED
    );
    settle(&l).await;
    let before: Vec<(String, String)> = mock
        .instances
        .lock()
        .expect("lock")
        .iter()
        .filter(|i| i.instance.name.starts_with("blog-"))
        .map(|i| (i.instance.name.clone(), i.instance.status.clone()))
        .collect();
    assert_eq!(before.len(), 1);
    let (st, b) = operate(&l, &s.owner, "stop", true).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{b}");
    settle(&l).await;
    let after: Vec<(String, String)> = mock
        .instances
        .lock()
        .expect("lock")
        .iter()
        .filter(|i| i.instance.name.starts_with("blog-"))
        .map(|i| (i.instance.name.clone(), i.instance.status.clone()))
        .collect();
    assert_eq!(
        before, after,
        "another application in the same environment is untouched"
    );
    let shop_states: Vec<String> = mock
        .instances
        .lock()
        .expect("lock")
        .iter()
        .filter(|i| i.instance.name.starts_with("shop-"))
        .map(|i| i.instance.status.clone())
        .collect();
    assert_eq!(shop_states, ["Stopped", "Stopped"]);
}

// ---- who may do what --------------------------------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn operators_operate_only_their_scope_and_designers_and_strangers_cannot(pool: Pool) {
    let (l, _) = mock_lab(pool);
    let s = shop(&l).await;
    deployed(&l, &s).await;
    let ctx = apelora_persistence::identity::application_context(&l.pool, "shop")
        .await
        .expect("db")
        .expect("app");
    let (op, des, view, stranger) = (
        l.user("sub-op", "op").await,
        l.user("sub-des", "des").await,
        l.user("sub-view", "view").await,
        l.user("sub-str", "stranger").await,
    );
    l.grant(
        &s.w.admin,
        op,
        "application-operator",
        json!({ "type": "application", "id": ctx.application_id }),
    )
    .await;
    l.grant(
        &s.w.admin,
        des,
        "application-designer",
        json!({ "type": "project", "id": s.w.acme_prod }),
    )
    .await;
    l.grant(
        &s.w.admin,
        view,
        "viewer",
        json!({ "type": "project", "id": s.w.acme_prod }),
    )
    .await;
    let _ = stranger;
    let (top, td, tv, ts) = (
        l.token("sub-op", "op"),
        l.token("sub-des", "des"),
        l.token("sub-view", "view"),
        l.token("sub-str", "stranger"),
    );

    // The buttons reflect the permissions the server will enforce.
    assert_eq!(actions(&info(&l, &top).await), (false, true, false));
    for (who, t) in [("designer", &td), ("viewer", &tv)] {
        let i = info(&l, t).await;
        assert_eq!(actions(&i), (false, false, false), "{who}");
        assert_eq!(
            i["actions"]["stop"]["reason"],
            "you may not start or stop this application"
        );
        let (st, b) = operate(&l, t, "stop", true).await;
        assert_eq!(
            (st, code(&b)),
            (StatusCode::FORBIDDEN, Some("forbidden")),
            "{who}: {b}"
        );
    }
    // A stranger (no assignment) and another organisation see nothing, not even that it exists.
    for path in [
        "/api/v1/applications/shop",
        "/api/v1/applications/shop/draft",
        "/api/v1/applications/shop/versions",
        "/api/v1/applications/shop/operations",
        "/api/v1/applications/shop/events",
    ] {
        assert_eq!(
            l.req("GET", path, &ts, None).await.0,
            StatusCode::FORBIDDEN,
            "{path}: a person with no role at all is stopped at the gate"
        );
    }
    let bob = l.user("sub-bob", "bob").await;
    l.grant(
        &s.w.admin,
        bob,
        "organization-owner",
        json!({ "type": "organization", "id": s.w.other }),
    )
    .await;
    let tb = l.token("sub-bob", "bob");
    for path in [
        "/api/v1/applications/shop",
        "/api/v1/applications/shop/draft",
        "/api/v1/applications/shop/versions/1",
        "/api/v1/applications/shop/operations",
        "/api/v1/applications/shop/events",
    ] {
        assert_eq!(
            l.req("GET", path, &tb, None).await.0,
            StatusCode::NOT_FOUND,
            "{path}"
        );
    }
    assert_eq!(
        operate(&l, &tb, "stop", true).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        l.req(
            "PUT",
            "/api/v1/applications/shop/draft",
            &tb,
            Some(json!({ "document": doc(), "baseVersion": 1 }))
        )
        .await
        .0,
        StatusCode::NOT_FOUND
    );
    // The operator stops it; the designer sees the operation and the events but cannot change it.
    let (st, b) = operate(&l, &top, "stop", true).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{b}");
    settle(&l).await;
    let (_, ops) = l
        .req("GET", "/api/v1/applications/shop/operations", &td, None)
        .await;
    assert_eq!(ops["items"][0]["state"], "succeeded");
    let (_, ev) = l
        .req("GET", "/api/v1/applications/shop/events", &td, None)
        .await;
    let actions_seen: Vec<&str> = ev["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|e| e["action"].as_str().unwrap_or(""))
        .collect();
    assert!(
        actions_seen.contains(&"application.stop.requested")
            && actions_seen.contains(&"application.create"),
        "{actions_seen:?}"
    );
    // The designer edits drafts but is not an administrator of anything else.
    assert_eq!(
        l.req(
            "PUT",
            "/api/v1/applications/shop/draft",
            &td,
            Some(json!({ "document": doc(), "baseVersion": 1 }))
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        l.req("GET", "/api/v1/cluster", &td, None).await.0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        l.req("GET", "/api/v1/applications/shop/draft", &tv, None)
            .await
            .0,
        StatusCode::OK,
        "viewers read"
    );
    assert_eq!(
        l.req(
            "PUT",
            "/api/v1/applications/shop/draft",
            &tv,
            Some(json!({ "document": doc(), "baseVersion": 2 }))
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
    // The list is scoped: Bob's is empty, the operator's has the one application he is assigned.
    assert_eq!(
        l.req("GET", "/api/v1/applications", &tb, None).await.1["items"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    assert_eq!(
        l.req("GET", "/api/v1/applications", &top, None).await.1["items"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn archiving_needs_a_stopped_application_and_hides_it_from_the_default_list(pool: Pool) {
    let (l, _) = mock_lab(pool);
    let s = shop(&l).await;
    deployed(&l, &s).await;
    let (st, b) = l
        .req("POST", "/api/v1/applications/shop/archive", &s.owner, None)
        .await;
    assert_eq!(
        (st, code(&b)),
        (StatusCode::CONFLICT, Some("still_running"))
    );
    assert_eq!(
        operate(&l, &s.owner, "stop", true).await.0,
        StatusCode::ACCEPTED
    );
    settle(&l).await;
    assert_eq!(
        l.req("POST", "/api/v1/applications/shop/archive", &s.owner, None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        l.req("GET", "/api/v1/applications", &s.owner, None).await.1["items"]
            .as_array()
            .map(Vec::len),
        Some(0)
    );
    let (_, all) = l
        .req(
            "GET",
            "/api/v1/applications?includeArchived=true",
            &s.owner,
            None,
        )
        .await;
    assert_eq!(
        (
            all["items"][0]["archived"].as_bool(),
            all["items"][0]["name"].as_str()
        ),
        (Some(true), Some("shop"))
    );
    assert_eq!(
        l.req("POST", "/api/v1/applications/shop/restore", &s.owner, None)
            .await
            .0,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        l.req("GET", "/api/v1/applications", &s.owner, None).await.1["items"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_list_and_the_detail_agree_and_legacy_tokens_still_see_everything(pool: Pool) {
    let (l, _) = mock_lab(pool.clone());
    let s = shop(&l).await;
    deployed(&l, &s).await;
    let (_, list) = l.req("GET", "/api/v1/applications", &s.owner, None).await;
    let one = &list["items"][0];
    let detail = info(&l, &s.owner).await;
    for key in ["name", "description", "icon", "design", "actions"] {
        assert_eq!(one[key], detail[key], "{key}");
    }
    assert_eq!(one["deployment"]["status"], detail["deployment"]["status"]);
    let (legacy, _) = apelora_persistence::create_token(&pool, "ci", "fabric-admin", None)
        .await
        .expect("token");
    let (st, all) = l.req("GET", "/api/v1/applications", &legacy, None).await;
    assert_eq!(
        (
            st,
            all["items"][0]["name"].as_str(),
            all["items"][0]["latestRevision"].as_i64()
        ),
        (StatusCode::OK, Some("shop"), Some(1)),
        "{all}"
    );
    let (_, d) = l
        .req("GET", "/api/v1/applications/shop", &legacy, None)
        .await;
    assert_eq!(d["modules"].as_array().map(Vec::len), Some(2));
    assert_eq!(
        d["modules"][0]["instances"].as_array().map(Vec::len),
        Some(1)
    );
}

#[allow(clippy::unwrap_used)] // test fixture: a failed unwrap is the failure
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_readiness_command_is_checked_before_a_deployment_is_queued_and_kept_with_its_desired_state(
    pool: Pool,
) {
    let (l, _mock) = mock_lab(pool);
    let s = shop(&l).await;
    let plan = |readiness: Value| {
        let mut m = module("db-01", "stateful");
        m["readiness"] = readiness;
        let (l, owner) = (&l, s.owner.clone());
        async move {
            l.req("POST", "/api/v1/applications/shop/plan", &owner, Some(json!({ "project": "acme-prod-live", "document": {}, "modules": [m], "connections": [] }))).await
        }
    };
    for (readiness, why) in [
        (json!({ "command": ["sh", "-c", "pg_isready"] }), "a shell"),
        (
            json!({ "command": ["pg_isready; reboot"] }),
            "metacharacters in the program",
        ),
        (json!({ "command": [] }), "nothing to run"),
        (
            json!({ "command": ["pg_isready"], "timeoutSecs": 99 }),
            "too long",
        ),
    ] {
        let (st, b) = plan(readiness).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{why}: {b}");
        assert!(
            b["error"]["message"]
                .as_str()
                .unwrap()
                .contains("readiness command"),
            "{why}: {b}"
        );
    }
    let (st, b) = plan(json!({ "command": ["pg_isready", "-q"], "timeoutSecs": 3 })).await;
    assert_eq!(st, StatusCode::OK, "{b}");

    // Deployed, the command is part of the recorded desired state.
    let (st, v) = l
        .req(
            "POST",
            "/api/v1/applications/shop/versions",
            &s.owner,
            Some(json!({ "label": "first" })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    let mut m = module("db-01", "stateful");
    m["readiness"] = json!({ "command": ["pg_isready", "-q"], "timeoutSecs": 3 });
    let (st, d) = l.req("POST", "/api/v1/applications/shop/deployments", &s.owner, Some(json!({ "project": "acme-prod-live", "document": {}, "version": v["revision"], "modules": [m], "connections": [] }))).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{d}");
    let desired: Value = sqlx::query_scalar(
        "SELECT desired FROM application_deployments ORDER BY updated_at DESC LIMIT 1",
    )
    .fetch_one(&l.pool)
    .await
    .unwrap();
    assert_eq!(
        desired[0]["readiness"],
        json!({ "command": ["pg_isready", "-q"], "timeoutSecs": 3 }),
        "{desired}"
    );
}
