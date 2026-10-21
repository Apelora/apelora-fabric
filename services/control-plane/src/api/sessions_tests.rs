// Interactive sessions through the real router and a real socket, against PostgreSQL and the
// mock Incus (whose "shell" is a labelled fake that executes nothing). Evidence: MOCK VERIFIED
// for the control plane's behaviour; the Incus exec protocol is covered by the adapter's
// contract tests (tests/exec.rs), and neither ran against a real Incus.

// Test fixtures: a failed `unwrap` is the failure.
#![allow(clippy::unwrap_used)]

use std::net::SocketAddr;
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, header};
use futures_util::{SinkExt, StreamExt};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tower::ServiceExt;
use uuid::Uuid;

use super::identity_tests::{Lab, code};
use super::manage_tests::{Shop, deployed, mock_lab, settle, shop};
use super::sessions::{CONNECT_PATH, ConsoleSettings, PROTOCOL};
use super::*;

const WEB: &str = "shop-web-01";
const DB: &str = "shop-db-01";

/// Like `Lab::req`, with extra request headers.
async fn call(
    state: AppState,
    method: &str,
    path: &str,
    token: &str,
    body: Option<Value>,
    headers: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut req = Request::builder()
        .method(method)
        .uri(path)
        .header(header::AUTHORIZATION, format!("Bearer {token}"));
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let req = match body {
        Some(b) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(b.to_string())),
        None => req.body(Body::empty()),
    }
    .expect("request");
    let res = router(state).oneshot(req).await.expect("response");
    let status = res.status();
    let bytes = res.into_body().collect().await.expect("body").to_bytes();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

struct Rig {
    l: Lab,
    s: Shop,
    /// Has `instance-access` on the project; needs approval in production.
    bob: String,
    bob_id: Uuid,
    /// Organisation owner who may approve (and is not the requester).
    carol: String,
    /// Viewer, application operator, and an owner of another organisation.
    dave: String,
    erin: String,
    mallory: String,
    mock: apelora_incus_adapter::MockIncus,
}

async fn rig(pool: Pool) -> Rig {
    let (l, mock) = mock_lab(pool);
    let s = shop(&l).await;
    deployed(&l, &s).await;
    let acme_prod = s.w.acme_prod;
    let project = json!({ "type": "project", "id": acme_prod });
    let person = |sub: &'static str, name: &'static str| {
        let l = &l;
        async move { (l.user(sub, name).await, l.token(sub, name)) }
    };
    let (bob_id, bob) = person("sub-bob", "bob").await;
    let (carol_id, carol) = person("sub-carol", "carol").await;
    let (dave_id, dave) = person("sub-dave", "dave").await;
    let (erin_id, erin) = person("sub-erin", "erin").await;
    let (mallory_id, mallory) = person("sub-mallory", "mallory").await;
    for (user, role, scope) in [
        (bob_id, "instance-access", project.clone()),
        (
            carol_id,
            "organization-owner",
            json!({ "type": "organization", "id": s.w.acme }),
        ),
        (dave_id, "viewer", project.clone()),
        (erin_id, "application-operator", project.clone()),
        (
            mallory_id,
            "organization-owner",
            json!({ "type": "organization", "id": s.w.other }),
        ),
    ] {
        let (st, b) = l.grant(&s.w.admin, user, role, scope).await;
        assert_eq!(st, StatusCode::CREATED, "{role}: {b}");
    }
    Rig {
        l,
        s,
        bob,
        bob_id,
        carol,
        dave,
        erin,
        mallory,
        mock,
    }
}

impl Rig {
    async fn open_with(
        &self,
        token: &str,
        instance: &str,
        headers: &[(&str, &str)],
    ) -> (StatusCode, Value) {
        call(
            self.l.state.clone(),
            "POST",
            &format!("/api/v1/applications/shop/instances/{instance}/sessions"),
            token,
            Some(json!({ "kind": "terminal", "cols": 100, "rows": 30 })),
            headers,
        )
        .await
    }

    async fn open(&self, token: &str, instance: &str) -> (StatusCode, Value) {
        self.open_with(token, instance, &[]).await
    }

    /// Requests approval as `who`, has carol grant it, returns the approval id.
    async fn approval(&self, who: &str, instance: &str) -> String {
        let (s, a) = self
            .l
            .req(
                "POST",
                "/api/v1/approvals",
                who,
                Some(json!({ "action": "instance.console", "target": format!("instance:shop/{instance}"), "reason": "investigating a slow query" })),
            )
            .await;
        assert_eq!(s, StatusCode::CREATED, "{a}");
        let id = a["id"].as_str().expect("id").to_owned();
        let (s, b) = self
            .l
            .req(
                "POST",
                &format!("/api/v1/approvals/{id}/approve"),
                &self.carol,
                None,
            )
            .await;
        assert_eq!(s, StatusCode::OK, "{b}");
        id
    }

    /// A ticket for bob on `instance`, after the approval production requires.
    async fn ticket(&self, instance: &str) -> Value {
        let approval = self.approval(&self.bob, instance).await;
        let (s, t) = self
            .open_with(&self.bob, instance, &[("apelora-approval", &approval)])
            .await;
        assert_eq!(s, StatusCode::CREATED, "{t}");
        t
    }

    async fn audit(&self, action: &str) -> Vec<Value> {
        sqlx::query_scalar::<_, Value>(
            "SELECT detail FROM audit_events WHERE action = $1 ORDER BY id",
        )
        .bind(action)
        .fetch_all(&self.l.pool)
        .await
        .expect("audit")
    }
}

async fn serve(state: AppState) -> SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let _ = axum::serve(
            listener,
            router(state).into_make_service_with_connect_info::<SocketAddr>(),
        )
        .await;
    });
    addr
}

type Client =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect_ws(
    addr: SocketAddr,
    protocols: &str,
    origin: Option<&str>,
) -> Result<Client, tokio_tungstenite::tungstenite::Error> {
    let mut req = format!("ws://{addr}{CONNECT_PATH}")
        .into_client_request()
        .expect("request");
    req.headers_mut()
        .insert("sec-websocket-protocol", protocols.parse().expect("header"));
    if let Some(o) = origin {
        req.headers_mut()
            .insert("origin", o.parse().expect("header"));
    }
    tokio_tungstenite::connect_async(req)
        .await
        .map(|(ws, _)| ws)
}

fn protocols(ticket: &Value) -> String {
    format!(
        "{PROTOCOL}, ticket.{}",
        ticket["ticket"].as_str().expect("ticket")
    )
}

fn same_origin(addr: SocketAddr) -> String {
    format!("http://{addr}")
}

fn status_of(e: &tokio_tungstenite::tungstenite::Error) -> Option<u16> {
    match e {
        tokio_tungstenite::tungstenite::Error::Http(r) => Some(r.status().as_u16()),
        _ => None,
    }
}

/// Reads until `needle` shows up in the terminal output.
async fn read_until(ws: &mut Client, needle: &str) -> String {
    let mut seen = String::new();
    loop {
        let m = tokio::time::timeout(Duration::from_secs(5), ws.next())
            .await
            .unwrap_or_else(|_| panic!("timed out waiting for {needle:?}; saw {seen:?}"));
        match m {
            Some(Ok(Message::Binary(b))) => {
                seen.push_str(&String::from_utf8_lossy(&b));
                if seen.contains(needle) {
                    return seen;
                }
            }
            Some(Ok(Message::Text(t))) => seen.push_str(&t),
            other => panic!("ended before {needle:?}: {other:?}; saw {seen:?}"),
        }
    }
}

/// Everything until the socket ends; returns the reason the server gave.
async fn closed_reason(ws: &mut Client) -> String {
    loop {
        let m = tokio::time::timeout(Duration::from_secs(8), ws.next())
            .await
            .expect("the session ends in time");
        match m {
            Some(Ok(Message::Text(t))) => {
                let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                if v["type"] == "closed" {
                    return v["reason"].as_str().unwrap_or_default().to_owned();
                }
            }
            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return "socket-closed".into(),
            Some(Ok(_)) => {}
        }
    }
}

// ---- who may open what ----------------------------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_access_list_says_what_each_person_may_open_and_why_not(pool: Pool) {
    let r = rig(pool).await;
    let get = |token: &str| {
        let (state, token) = (r.l.state.clone(), token.to_owned());
        async move {
            call(
                state,
                "GET",
                "/api/v1/applications/shop/access",
                &token,
                None,
                &[],
            )
            .await
        }
    };

    let (s, a) = get(&r.bob).await;
    assert_eq!(s, StatusCode::OK, "{a}");
    assert_eq!(a["observed"], json!(true));
    let names: Vec<&str> = a["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["instance"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec![DB, WEB]);
    for item in a["items"].as_array().unwrap() {
        assert_eq!(item["guestOs"], json!("linux"));
        let kinds = item["kinds"].as_array().unwrap();
        assert_eq!(kinds.len(), 1, "a Linux module offers a terminal only");
        assert_eq!(kinds[0]["kind"], json!("terminal"));
        assert_eq!(kinds[0]["allowed"], json!(true), "{item}");
        assert_eq!(kinds[0]["approvalRequired"], json!(true), "production");
    }

    for (who, token) in [("viewer", &r.dave), ("operator", &r.erin)] {
        let (_, a) = get(token).await;
        let k = &a["items"][0]["kinds"][0];
        assert_eq!(k["allowed"], json!(false), "{who}");
        assert!(
            k["reason"].as_str().unwrap().contains("instance.console"),
            "{who}: {k}"
        );
    }
    // Another organisation cannot even see the application.
    assert_eq!(get(&r.mallory).await.0, StatusCode::NOT_FOUND);
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn only_signed_in_people_with_instance_console_on_this_application_can_open_one(pool: Pool) {
    let r = rig(pool).await;
    // No permission: 403, and the refusal is in the audit trail.
    for token in [&r.dave, &r.erin] {
        let (s, b) = r.open(token, WEB).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{b}");
    }
    // Another organisation's person: the application does not exist for them.
    assert_eq!(r.open(&r.mallory, WEB).await.0, StatusCode::NOT_FOUND);
    // An instance that is not this application's: 404 as well.
    let (s, b) = r.open(&r.s.owner, "shop-nope").await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::NOT_FOUND, Some("not_found")),
        "{b}"
    );
    assert_eq!(
        r.open(&r.s.owner, "bad_name").await.0,
        StatusCode::BAD_REQUEST
    );

    // Legacy platform-wide tokens and service identities never get a session.
    let (legacy, _) = apelora_persistence::create_token(&r.l.pool, "ci", "fabric-admin", None)
        .await
        .expect("token");
    assert_eq!(
        r.open(&legacy, WEB).await.0,
        StatusCode::FORBIDDEN,
        "a legacy token may view applications but never open a session"
    );
    let (st, ci) =
        r.l.req(
            "POST",
            "/api/v1/identity/service-identities",
            &r.s.w.admin,
            Some(json!({ "name": "ci", "kind": "automation" })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{ci}");
    let svc = ci["token"].as_str().unwrap().to_owned();
    let svc_id: Uuid = ci["identity"]["id"].as_str().unwrap().parse().unwrap();
    let (st, b) = r
        .l
        .req("POST", "/api/v1/identity/assignments", &r.s.w.admin, Some(json!({ "principal": { "type": "service", "id": svc_id }, "role": { "type": "builtin", "key": "instance-access" }, "scope": { "type": "project", "id": r.s.w.acme_prod } })))
        .await;
    assert_eq!(
        st,
        StatusCode::FORBIDDEN,
        "a service may not even be given the role: {b}"
    );
    let (st, b) = r
        .l
        .req("POST", "/api/v1/identity/assignments", &r.s.w.admin, Some(json!({ "principal": { "type": "service", "id": svc_id }, "role": { "type": "builtin", "key": "application-operator" }, "scope": { "type": "project", "id": r.s.w.acme_prod } })))
        .await;
    assert_eq!(st, StatusCode::CREATED, "{b}");
    let (s, b) = r.open(&svc, WEB).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "{b}");

    // People without the permission anywhere, and legacy tokens (which can never hold it), are
    // stopped by the route gate (`access.denied`), and that is on record. Every refusal above
    // happened there, so the handler had nothing to refuse (`session.denied`).
    let gate = r.audit("access.denied").await;
    let handler = r.audit("session.denied").await;
    assert!(gate.len() >= 4, "{gate:?}");
    assert!(
        gate.iter().any(|d| d["name"] == "ci"),
        "the legacy token's refusal: {gate:?}"
    );
    assert!(handler.is_empty(), "{handler:?}");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_stopped_application_gives_no_session_and_says_why(pool: Pool) {
    let r = rig(pool).await;
    let (s, b) =
        r.l.req(
            "POST",
            "/api/v1/applications/shop/operations",
            &r.s.owner,
            Some(json!({ "action": "stop", "confirmDisruption": true })),
        )
        .await;
    assert_eq!(s, StatusCode::ACCEPTED, "{b}");
    settle(&r.l).await;
    let (s, b) = r.open(&r.s.owner, WEB).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("session_unavailable")),
        "{b}"
    );
    assert!(
        b["error"]["message"].as_str().unwrap().contains("stopped"),
        "{b}"
    );
    let (_, a) = call(
        r.l.state.clone(),
        "GET",
        "/api/v1/applications/shop/access",
        &r.s.owner,
        None,
        &[],
    )
    .await;
    let k = &a["items"][0]["kinds"][0];
    assert_eq!(k["allowed"], json!(false));
    assert!(
        k["reason"]
            .as_str()
            .unwrap()
            .contains("start the application"),
        "{k}"
    );
}

// ---- approval in production -----------------------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn production_sessions_need_another_persons_approval_which_works_once_for_that_instance(
    pool: Pool,
) {
    let r = rig(pool).await;
    let (s, b) = r.open(&r.bob, WEB).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("approval_required")),
        "{b}"
    );

    // bob cannot approve his own request, nor can erin (an operator) approve for the application.
    let (s, a) = r
        .l
        .req("POST", "/api/v1/approvals", &r.bob, Some(json!({ "action": "instance.console", "target": format!("instance:shop/{WEB}"), "reason": "look at the logs" })))
        .await;
    assert_eq!(s, StatusCode::CREATED, "{a}");
    let id = a["id"].as_str().unwrap().to_owned();
    for token in [&r.bob, &r.erin, &r.mallory] {
        let (s, _) =
            r.l.req(
                "POST",
                &format!("/api/v1/approvals/{id}/approve"),
                token,
                None,
            )
            .await;
        assert_eq!(s, StatusCode::FORBIDDEN);
    }
    // carol, who may approve on the application, sees it in her list and grants it.
    let (_, list) = r.l.req("GET", "/api/v1/approvals", &r.carol, None).await;
    assert!(
        list["items"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["id"] == json!(id)),
        "{list}"
    );
    let (s, _) =
        r.l.req(
            "POST",
            &format!("/api/v1/approvals/{id}/approve"),
            &r.carol,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::OK);

    // It is for web-01 only.
    let (s, b) = r.open_with(&r.bob, DB, &[("apelora-approval", &id)]).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("approval_invalid")),
        "{b}"
    );
    // Whoever else presents it gets nothing.
    let (s, _) = r
        .open_with(&r.s.owner, WEB, &[("apelora-approval", &id)])
        .await;
    assert_eq!(s, StatusCode::CONFLICT);
    // The right person, once.
    let (s, t) = r.open_with(&r.bob, WEB, &[("apelora-approval", &id)]).await;
    assert_eq!(s, StatusCode::CREATED, "{t}");
    assert_eq!(t["protocol"], json!(PROTOCOL));
    assert_eq!(t["provider"], json!("mock"));
    let (s, b) = r.open_with(&r.bob, WEB, &[("apelora-approval", &id)]).await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::CONFLICT, Some("approval_invalid")),
        "{b}"
    );
    let granted = r.audit("session.request").await;
    assert_eq!(granted.len(), 1, "{granted:?}");
    assert_eq!(granted[0]["approval"], json!(id));
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn an_approval_for_a_session_is_judged_on_the_application_not_the_platform(pool: Pool) {
    let r = rig(pool).await;
    // Asking for something the person may not do themselves is refused, as is a malformed or foreign target.
    for (who, action, target) in [
        (&r.dave, "instance.console", format!("instance:shop/{WEB}")),
        (&r.bob, "instance.console", "instance:nosuch/x".to_owned()),
        (&r.bob, "instance.console", "machine:rack1".to_owned()),
        (
            &r.mallory,
            "instance.console",
            format!("instance:shop/{WEB}"),
        ),
    ] {
        let (s, b) =
            r.l.req(
                "POST",
                "/api/v1/approvals",
                who,
                Some(json!({ "action": action, "target": target, "reason": "x" })),
            )
            .await;
        assert!(
            s == StatusCode::FORBIDDEN || s == StatusCode::BAD_REQUEST,
            "{target}: {s} {b}"
        );
    }
}

// ---- the websocket ---------------------------------------------------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_ticket_opens_the_labelled_mock_shell_once_and_the_session_is_audited_without_content(
    pool: Pool,
) {
    let r = rig(pool).await;
    let addr = serve(r.l.state.clone()).await;
    let t = r.ticket(WEB).await;
    let ws = connect_ws(addr, &protocols(&t), Some(&same_origin(addr))).await;
    // The ticket was issued without an Origin (an API client), so a browser page may not use it.
    let err = ws.as_ref().err().and_then(status_of);
    assert_eq!(err, Some(403), "origin does not match the ticket: {ws:?}");

    let t = r.ticket(DB).await;
    let mut ws = connect_ws(addr, &protocols(&t), None)
        .await
        .expect("connect");
    let banner = read_until(&mut ws, "$ ").await;
    assert!(banner.contains("MOCK session for shop-db-01"), "{banner}");
    ws.send(Message::Binary(b"hostname\r".to_vec().into()))
        .await
        .unwrap();
    read_until(&mut ws, "shop-db-01\r\n").await;
    ws.send(Message::Text(
        json!({ "type": "resize", "cols": 120, "rows": 40 })
            .to_string()
            .into(),
    ))
    .await
    .unwrap();
    ws.send(Message::Binary(b"exit\r".to_vec().into()))
        .await
        .unwrap();
    assert_eq!(closed_reason(&mut ws).await, "exited");

    let open = r.audit("session.open").await;
    // The close is recorded just after the browser is told: give it a moment under load.
    let mut close = r.audit("session.close").await;
    for _ in 0..50 {
        if !close.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        close = r.audit("session.close").await;
    }
    assert_eq!((open.len(), close.len()), (1, 1), "{open:?} {close:?}");
    assert_eq!(open[0]["kind"], json!("terminal"));
    assert_eq!(open[0]["runsAs"], json!(1000), "not root");
    assert_eq!(open[0]["provider"], json!("mock"));
    assert_eq!(close[0]["reason"], json!("exited"));
    assert!(close[0]["bytesIn"].as_u64().unwrap() >= 14);
    // What was typed and shown never reaches the audit trail.
    let everything: Vec<String> = sqlx::query_scalar("SELECT detail::text FROM audit_events")
        .fetch_all(&r.l.pool)
        .await
        .unwrap();
    for text in &everything {
        assert!(
            !text.contains("hostname") && !text.contains("MOCK session"),
            "{text}"
        );
        assert!(
            !text.contains(t["ticket"].as_str().unwrap()),
            "the ticket is never stored: {text}"
        );
    }
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn tickets_are_single_use_bound_to_their_page_and_never_taken_from_the_url(pool: Pool) {
    let r = rig(pool).await;
    let addr = serve(r.l.state.clone()).await;

    let t = r.ticket(WEB).await;
    let mut ws = connect_ws(addr, &protocols(&t), None)
        .await
        .expect("first use");
    read_until(&mut ws, "$ ").await;
    // Used once, spent for good (even while the first session is still open).
    let again = connect_ws(addr, &protocols(&t), None).await;
    assert_eq!(
        again.as_ref().err().and_then(status_of),
        Some(401),
        "{again:?}"
    );
    drop(ws);

    // Wrong origin: refused before the ticket is looked at, so it can still be used from the right place.
    let t = r.ticket(WEB).await;
    let evil = connect_ws(addr, &protocols(&t), Some("https://evil.example.test")).await;
    assert_eq!(
        evil.as_ref().err().and_then(status_of),
        Some(403),
        "{evil:?}"
    );
    assert!(
        connect_ws(addr, &protocols(&t), None).await.is_ok(),
        "an unspent ticket still works"
    );

    // No ticket, a made-up ticket, and a ticket in the query string.
    for p in [
        PROTOCOL.to_owned(),
        format!("{PROTOCOL}, ticket.{}", "0".repeat(64)),
    ] {
        let res = connect_ws(addr, &p, None).await;
        assert_eq!(
            res.as_ref().err().and_then(status_of),
            Some(401),
            "{p}: {res:?}"
        );
    }
    let t = r.ticket(WEB).await;
    let mut req = format!(
        "ws://{addr}{CONNECT_PATH}?ticket={}",
        t["ticket"].as_str().unwrap()
    )
    .into_client_request()
    .unwrap();
    req.headers_mut()
        .insert("sec-websocket-protocol", PROTOCOL.parse().unwrap());
    let res = tokio_tungstenite::connect_async(req).await;
    assert_eq!(
        res.as_ref().err().and_then(status_of),
        Some(401),
        "a ticket in the URL is ignored"
    );

    // A ticket expires.
    let short = ConsoleSettings {
        ticket_ttl: Duration::from_millis(50),
        ..ConsoleSettings::default()
    };
    let state = r.l.state.clone().with_console(short);
    let addr2 = serve(state.clone()).await;
    let approval = r.approval(&r.bob, DB).await;
    let (s, t) = call(
        state,
        "POST",
        &format!("/api/v1/applications/shop/instances/{DB}/sessions"),
        &r.bob,
        Some(json!({ "kind": "terminal" })),
        &[("apelora-approval", &approval)],
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{t}");
    tokio::time::sleep(Duration::from_millis(150)).await;
    let late = connect_ws(addr2, &protocols(&t), None).await;
    assert_eq!(
        late.as_ref().err().and_then(status_of),
        Some(401),
        "{late:?}"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_browser_ticket_only_works_from_the_page_it_was_issued_to(pool: Pool) {
    let r = rig(pool).await;
    let addr = serve(r.l.state.clone()).await;
    let origin = same_origin(addr);
    let approval = r.approval(&r.bob, WEB).await;
    let (s, t) = call(
        r.l.state.clone(),
        "POST",
        &format!("/api/v1/applications/shop/instances/{WEB}/sessions"),
        &r.bob,
        Some(json!({ "kind": "terminal" })),
        &[("apelora-approval", &approval), ("origin", &origin)],
    )
    .await;
    assert_eq!(s, StatusCode::CREATED, "{t}");
    let mut ws = connect_ws(addr, &protocols(&t), Some(&origin))
        .await
        .expect("same page");
    read_until(&mut ws, "$ ").await;
}

/// The Fabric Console is a second door into the same sessions. In development it runs on another
/// origin than the Studio (5174 and 5173); both are configured by name, a ticket stays bound to
/// the page it was issued to, and a third origin is refused.
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_console_origin_is_configured_explicitly_and_a_third_origin_is_refused(pool: Pool) {
    const STUDIO: &str = "http://127.0.0.1:5173";
    const CONSOLE: &str = "http://127.0.0.1:5174";
    const THIRD: &str = "http://127.0.0.1:5175";
    let r = rig(pool).await;
    let settings = ConsoleSettings::from_env(|k| {
        (k == "APELORA_ALLOWED_ORIGINS").then(|| format!("{STUDIO},{CONSOLE}"))
    })
    .unwrap();
    let state = r.l.state.clone().with_console(settings);
    let addr = serve(state.clone()).await;
    let ticket_from = |origin: &'static str| {
        let (state, bob) = (state.clone(), r.bob.clone());
        let r = &r;
        async move {
            let approval = r.approval(&bob, WEB).await;
            let (s, t) = call(
                state,
                "POST",
                &format!("/api/v1/applications/shop/instances/{WEB}/sessions"),
                &bob,
                Some(json!({ "kind": "terminal" })),
                &[("apelora-approval", &approval), ("origin", origin)],
            )
            .await;
            assert_eq!(s, StatusCode::CREATED, "{t}");
            t
        }
    };

    // A ticket the Console asked for opens from the Console.
    let t = ticket_from(CONSOLE).await;
    let mut ws = connect_ws(addr, &protocols(&t), Some(CONSOLE))
        .await
        .expect("the Console opens its own ticket");
    read_until(&mut ws, "$ ").await;
    drop(ws);

    // The Studio is allowed too, but not with the Console's ticket.
    let t = ticket_from(CONSOLE).await;
    let other = connect_ws(addr, &protocols(&t), Some(STUDIO)).await;
    assert_eq!(
        other.as_ref().err().and_then(status_of),
        Some(403),
        "a Console ticket used from the Studio: {other:?}"
    );

    // A third origin is refused at the door, even with a ticket issued to it.
    let t = ticket_from(THIRD).await;
    let third = connect_ws(addr, &protocols(&t), Some(THIRD)).await;
    assert_eq!(
        third.as_ref().err().and_then(status_of),
        Some(403),
        "{THIRD} is not configured: {third:?}"
    );
    // Without the configuration the Console's origin is just another origin.
    let plain = serve(r.l.state.clone()).await;
    let t = ticket_from(CONSOLE).await;
    let refused = connect_ws(plain, &protocols(&t), Some(CONSOLE)).await;
    assert_eq!(
        refused.as_ref().err().and_then(status_of),
        Some(403),
        "{refused:?}"
    );
}

/// The sessions list says when the person last typed and when the session ends if they stop.
#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_sessions_list_shows_the_last_input_and_whose_session_it_is(pool: Pool) {
    let r = rig(pool).await;
    let addr = serve(r.l.state.clone()).await;
    let mut ws = connect_ws(addr, &protocols(&r.ticket(WEB).await), None)
        .await
        .unwrap();
    read_until(&mut ws, "$ ").await;
    let list = |token: String| {
        let state = r.l.state.clone();
        async move {
            let (s, v) = call(state, "GET", "/api/v1/sessions", &token, None, &[]).await;
            assert_eq!(s, StatusCode::OK, "{v}");
            v["items"][0].clone()
        }
    };
    let first = list(r.bob.clone()).await;
    assert_eq!(first["mine"], json!(true));
    assert_eq!(first["lastActivityAt"], first["startedAt"], "{first}");
    let at = |v: &Value, k: &str| {
        time::OffsetDateTime::parse(
            v[k].as_str().unwrap(),
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap()
    };
    assert_eq!(
        at(&first, "idleEndsAt") - at(&first, "lastActivityAt"),
        time::Duration::minutes(15)
    );
    assert_eq!(
        at(&first, "endsBy") - at(&first, "startedAt"),
        time::Duration::minutes(60)
    );
    // Typing moves the last activity (noted at most once a second).
    tokio::time::sleep(Duration::from_millis(1100)).await;
    ws.send(Message::Binary(b"x".to_vec().into()))
        .await
        .unwrap();
    let mut later = first.clone();
    for _ in 0..40 {
        later = list(r.bob.clone()).await;
        if later["lastActivityAt"] != first["lastActivityAt"] {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(
        at(&later, "lastActivityAt") > at(&first, "lastActivityAt"),
        "{later}"
    );
    // An operator of the application sees it, as someone else's.
    let theirs = list(r.erin.clone()).await;
    assert_eq!(theirs["mine"], json!(false));
    assert_eq!(theirs["person"], json!("bob"));
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn sessions_end_when_idle_when_too_long_when_terminated_and_when_access_is_revoked(
    pool: Pool,
) {
    let r = rig(pool).await;
    let tuned = |idle_ms: u64, max_ms: u64| ConsoleSettings {
        idle: Duration::from_millis(idle_ms),
        max: Duration::from_millis(max_ms),
        recheck: Duration::from_millis(150),
        max_per_user: 10,
        ..ConsoleSettings::default()
    };
    // Idle: nothing typed.
    let addr = serve(r.l.state.clone().with_console(tuned(400, 20_000))).await;
    let mut ws = connect_ws(addr, &protocols(&r.ticket(WEB).await), None)
        .await
        .unwrap();
    assert_eq!(closed_reason(&mut ws).await, "idle");

    // Typing keeps it alive past the idle limit, the absolute limit does not.
    let addr = serve(r.l.state.clone().with_console(tuned(500, 1_200))).await;
    let mut ws = connect_ws(addr, &protocols(&r.ticket(WEB).await), None)
        .await
        .unwrap();
    read_until(&mut ws, "$ ").await;
    for _ in 0..4 {
        tokio::time::sleep(Duration::from_millis(300)).await;
        ws.send(Message::Binary(b"x".to_vec().into()))
            .await
            .unwrap();
    }
    assert_eq!(closed_reason(&mut ws).await, "max-duration");

    // Terminated: by the person, or by whoever may operate the application; nobody else sees it.
    let addr = serve(r.l.state.clone().with_console(tuned(20_000, 20_000))).await;
    let mut ws = connect_ws(addr, &protocols(&r.ticket(WEB).await), None)
        .await
        .unwrap();
    read_until(&mut ws, "$ ").await;
    let state = r.l.state.clone();
    let (_, mine) = call(state.clone(), "GET", "/api/v1/sessions", &r.bob, None, &[]).await;
    let id = mine["items"][0]["id"]
        .as_str()
        .expect("my session")
        .to_owned();
    assert_eq!(mine["items"][0]["person"], json!("bob"));
    let (s, _) = call(state.clone(), "GET", "/api/v1/sessions", &r.dave, None, &[]).await;
    assert_eq!(s, StatusCode::FORBIDDEN, "a viewer has no use for the list");
    let (_, ops) = call(state.clone(), "GET", "/api/v1/sessions", &r.erin, None, &[]).await;
    assert_eq!(
        ops["items"].as_array().unwrap().len(),
        1,
        "an operator of the application sees its sessions"
    );
    let (s, _) = call(
        state.clone(),
        "DELETE",
        &format!("/api/v1/sessions/{id}"),
        &r.dave,
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        state.clone(),
        "DELETE",
        &format!("/api/v1/sessions/{id}"),
        &r.erin,
        None,
        &[],
    )
    .await;
    assert_eq!(
        s,
        StatusCode::NO_CONTENT,
        "an operator of the application may end it"
    );
    assert_eq!(closed_reason(&mut ws).await, "terminated");
    assert_eq!(r.audit("session.terminate").await.len(), 1);

    // Revoked: the role is taken away while the session runs.
    let addr = serve(r.l.state.clone().with_console(tuned(20_000, 20_000))).await;
    let mut ws = connect_ws(addr, &protocols(&r.ticket(DB).await), None)
        .await
        .unwrap();
    read_until(&mut ws, "$ ").await;
    let (_, assignments) =
        r.l.req("GET", "/api/v1/identity/assignments", &r.s.w.admin, None)
            .await;
    let mine = assignments["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["principal"]["id"] == json!(r.bob_id) && a["revokedAt"].is_null())
        .expect("bob's assignment")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (s, _) =
        r.l.req(
            "DELETE",
            &format!("/api/v1/identity/assignments/{mine}"),
            &r.s.w.admin,
            None,
        )
        .await;
    assert!(s.is_success(), "{s}");
    assert_eq!(closed_reason(&mut ws).await, "access-revoked");
    // The close is audited just after the socket ends.
    let mut reasons = Vec::new();
    for _ in 0..40 {
        reasons = r
            .audit("session.close")
            .await
            .iter()
            .map(|d| d["reason"].as_str().unwrap().to_owned())
            .collect();
        if reasons.len() == 4 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        reasons,
        vec!["idle", "max-duration", "terminated", "access-revoked"]
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_person_cannot_hold_more_sessions_than_allowed_and_the_feature_can_be_switched_off(
    pool: Pool,
) {
    let r = rig(pool).await;
    let capped = r.l.state.clone().with_console(ConsoleSettings {
        max_per_user: 1,
        ..ConsoleSettings::default()
    });
    let a1 = r.approval(&r.bob, WEB).await;
    let a2 = r.approval(&r.bob, DB).await;
    let path = |i: &str| format!("/api/v1/applications/shop/instances/{i}/sessions");
    let (s, _) = call(
        capped.clone(),
        "POST",
        &path(WEB),
        &r.bob,
        Some(json!({ "kind": "terminal" })),
        &[("apelora-approval", &a1)],
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, b) = call(
        capped,
        "POST",
        &path(DB),
        &r.bob,
        Some(json!({ "kind": "terminal" })),
        &[("apelora-approval", &a2)],
    )
    .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::TOO_MANY_REQUESTS, Some("too_many_sessions")),
        "{b}"
    );

    let off = r.l.state.clone().with_console(ConsoleSettings {
        enabled: false,
        ..ConsoleSettings::default()
    });
    let (s, b) = call(
        off.clone(),
        "POST",
        &path(WEB),
        &r.s.owner,
        Some(json!({ "kind": "terminal" })),
        &[],
    )
    .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::SERVICE_UNAVAILABLE, Some("console_disabled")),
        "{b}"
    );
    let addr = serve(off.clone()).await;
    let res = connect_ws(addr, PROTOCOL, None).await;
    assert_eq!(res.as_ref().err().and_then(status_of), Some(503));
    let (_, a) = call(
        off,
        "GET",
        "/api/v1/applications/shop/access",
        &r.s.owner,
        None,
        &[],
    )
    .await;
    assert!(
        a["items"][0]["kinds"][0]["reason"]
            .as_str()
            .unwrap()
            .contains("turned off"),
        "{a}"
    );
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn the_kind_must_suit_the_module_and_the_size_must_be_sane(pool: Pool) {
    let r = rig(pool).await;
    let path = format!("/api/v1/applications/shop/instances/{WEB}/sessions");
    for body in [
        json!({ "kind": "powershell" }),
        json!({ "kind": "rdp" }),
        json!({ "kind": "terminal", "cols": 0 }),
        json!({ "kind": "terminal", "rows": 5000 }),
        json!({ "kind": "ssh" }),
        json!({}),
    ] {
        let (s, b) = call(
            r.l.state.clone(),
            "POST",
            &path,
            &r.s.owner,
            Some(body.clone()),
            &[],
        )
        .await;
        assert!(
            s == StatusCode::BAD_REQUEST || s == StatusCode::UNPROCESSABLE_ENTITY,
            "{body}: {s} {b}"
        );
    }
}

// ---- limits that must not cost an approval, groups, Windows -----------------------------------

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_refusal_by_a_limit_does_not_burn_the_approval(pool: Pool) {
    let r = rig(pool).await;
    let capped = r.l.state.clone().with_console(ConsoleSettings {
        max_per_user: 1,
        ..ConsoleSettings::default()
    });
    let path = |i: &str| format!("/api/v1/applications/shop/instances/{i}/sessions");
    let first = r.approval(&r.bob, WEB).await;
    let second = r.approval(&r.bob, DB).await;
    let (s, _) = call(
        capped.clone(),
        "POST",
        &path(WEB),
        &r.bob,
        Some(json!({ "kind": "terminal" })),
        &[("apelora-approval", &first)],
    )
    .await;
    assert_eq!(s, StatusCode::CREATED);
    let (s, b) = call(
        capped,
        "POST",
        &path(DB),
        &r.bob,
        Some(json!({ "kind": "terminal" })),
        &[("apelora-approval", &second)],
    )
    .await;
    assert_eq!(
        (s, code(&b)),
        (StatusCode::TOO_MANY_REQUESTS, Some("too_many_sessions")),
        "{b}"
    );
    let state: String = sqlx::query_scalar("SELECT state FROM approvals WHERE id = $1")
        .bind(second.parse::<Uuid>().unwrap())
        .fetch_one(&r.l.pool)
        .await
        .unwrap();
    assert_eq!(state, "approved", "the approval is still unspent");
    // Once there is room it still works.
    let (s, _) = r
        .open_with(&r.bob, DB, &[("apelora-approval", &second)])
        .await;
    assert_eq!(s, StatusCode::CREATED, "default limit allows three");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn removing_someone_from_the_group_that_gave_them_access_ends_their_session(pool: Pool) {
    let r = rig(pool).await;
    let gary_id = r.l.user("sub-gary", "gary").await;
    let gary = r.l.token("sub-gary", "gary");
    let (s, g) =
        r.l.req(
            "POST",
            "/api/v1/identity/groups",
            &r.s.w.admin,
            Some(json!({ "organizationId": r.s.w.acme, "name": "Night shift" })),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{g}");
    let gid: Uuid = g["id"].as_str().unwrap().parse().unwrap();
    let (s, b) = r.l.req("POST", "/api/v1/identity/assignments", &r.s.w.admin, Some(json!({ "principal": { "type": "group", "id": gid }, "role": { "type": "builtin", "key": "instance-access" }, "scope": { "type": "project", "id": r.s.w.acme_prod } }))).await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let (s, _) =
        r.l.req(
            "PUT",
            &format!("/api/v1/identity/groups/{gid}/members/{gary_id}"),
            &r.s.w.admin,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);

    let approval = r.approval(&gary, WEB).await;
    let (s, t) = r
        .open_with(&gary, WEB, &[("apelora-approval", &approval)])
        .await;
    assert_eq!(s, StatusCode::CREATED, "{t}");
    let addr = serve(r.l.state.clone().with_console(ConsoleSettings {
        recheck: Duration::from_millis(150),
        ..ConsoleSettings::default()
    }))
    .await;
    let mut ws = connect_ws(addr, &protocols(&t), None)
        .await
        .expect("connect");
    read_until(&mut ws, "$ ").await;
    let (s, _) =
        r.l.req(
            "DELETE",
            &format!("/api/v1/identity/groups/{gid}/members/{gary_id}"),
            &r.s.w.admin,
            None,
        )
        .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(closed_reason(&mut ws).await, "access-revoked");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn a_person_whose_access_ends_before_the_socket_opens_gets_no_shell(pool: Pool) {
    let r = rig(pool).await;
    let t = r.ticket(WEB).await;
    let (_, assignments) =
        r.l.req("GET", "/api/v1/identity/assignments", &r.s.w.admin, None)
            .await;
    let mine = assignments["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["principal"]["id"] == json!(r.bob_id) && a["revokedAt"].is_null())
        .unwrap()["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let (s, _) =
        r.l.req(
            "DELETE",
            &format!("/api/v1/identity/assignments/{mine}"),
            &r.s.w.admin,
            None,
        )
        .await;
    assert!(s.is_success());
    let addr = serve(r.l.state.clone()).await;
    let mut ws = connect_ws(addr, &protocols(&t), None)
        .await
        .expect("upgrade");
    assert_eq!(closed_reason(&mut ws).await, "access-revoked");
    assert!(
        r.audit("session.close").await.is_empty(),
        "nothing was ever opened"
    );
    let denied = r.audit("session.open").await;
    assert_eq!(denied.len(), 1, "{denied:?}");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn windows_modules_offer_powershell_only_when_turned_on_and_never_a_desktop_without_a_gateway(
    pool: Pool,
) {
    let (l, _mock) = mock_lab(pool);
    let s = shop(&l).await;
    let (st, v) = l
        .req(
            "POST",
            "/api/v1/applications/shop/versions",
            &s.owner,
            Some(json!({ "label": "first" })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    let spec = |id: &str| {
        json!({ "module": id, "availability": "singleton", "guestOs": if id == "win-01" { "windows" } else { "linux" }, "spec": {
        "project": "ignored", "name": "ignored", "kind": "container", "start": true,
        "source": { "protocol": "oci", "server": "https://registry.example", "alias": "nginx:1.29" } } })
    };
    let (st, d) = l.req("POST", "/api/v1/applications/shop/deployments", &s.owner, Some(json!({ "project": "acme-prod-live", "document": {}, "version": v["revision"], "modules": [spec("win-01"), spec("web-01")], "connections": [] }))).await;
    assert_eq!(st, StatusCode::ACCEPTED, "{d}");
    settle(&l).await;

    let kinds = |access: &Value, instance: &str| -> Vec<(String, bool, String)> {
        access["items"]
            .as_array()
            .unwrap()
            .iter()
            .find(|i| i["instance"] == json!(instance))
            .unwrap()["kinds"]
            .as_array()
            .unwrap()
            .iter()
            .map(|k| {
                (
                    k["kind"].as_str().unwrap().to_owned(),
                    k["allowed"].as_bool().unwrap(),
                    k["reason"].as_str().unwrap_or("").to_owned(),
                )
            })
            .collect()
    };
    let (_, a) = call(
        l.state.clone(),
        "GET",
        "/api/v1/applications/shop/access",
        &s.owner,
        None,
        &[],
    )
    .await;
    let win = kinds(&a, "shop-win-01");
    assert_eq!(
        win.iter().map(|k| k.0.as_str()).collect::<Vec<_>>(),
        vec!["powershell", "rdp"],
        "{a}"
    );
    assert!(!win[0].1 && win[0].2.contains("not turned on"), "{win:?}");
    assert!(!win[1].1 && win[1].2.contains("gateway"), "{win:?}");
    assert_eq!(
        kinds(&a, "shop-web-01")
            .iter()
            .map(|k| k.0.as_str())
            .collect::<Vec<_>>(),
        vec!["terminal"]
    );

    // Turned on: PowerShell becomes available (production: with approval); RDP stays unavailable.
    let on = l.state.clone().with_console(ConsoleSettings {
        powershell: true,
        ..ConsoleSettings::default()
    });
    let (_, a) = call(
        on.clone(),
        "GET",
        "/api/v1/applications/shop/access",
        &s.owner,
        None,
        &[],
    )
    .await;
    let win = kinds(&a, "shop-win-01");
    assert!(win[0].1, "{win:?}");
    assert!(!win[1].1, "{win:?}");
    // A Windows module has no Linux terminal, and a Linux one has no PowerShell.
    let path = |i: &str| format!("/api/v1/applications/shop/instances/{i}/sessions");
    let (st, b) = call(
        on.clone(),
        "POST",
        &path("shop-win-01"),
        &s.owner,
        Some(json!({ "kind": "terminal" })),
        &[],
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{b}");
    let (st, b) = call(
        on.clone(),
        "POST",
        &path("shop-web-01"),
        &s.owner,
        Some(json!({ "kind": "powershell" })),
        &[],
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{b}");
    let (st, b) = call(
        on,
        "POST",
        &path("shop-win-01"),
        &s.owner,
        Some(json!({ "kind": "rdp" })),
        &[],
    )
    .await;
    assert_eq!(
        (st, code(&b)),
        (StatusCode::CONFLICT, Some("session_unavailable")),
        "{b}"
    );
}

/// Everything the server says on the socket until it ends: (text frames, close reason).
async fn texts_until_closed(ws: &mut Client) -> (Vec<Value>, String) {
    let mut texts = Vec::new();
    loop {
        let m = tokio::time::timeout(Duration::from_secs(8), ws.next())
            .await
            .expect("the session ends in time");
        match m {
            Some(Ok(Message::Text(t))) => {
                let v: Value = serde_json::from_str(&t).unwrap_or(Value::Null);
                if v["type"] == "closed" {
                    let reason = v["reason"].as_str().unwrap_or_default().to_owned();
                    texts.push(v);
                    return (texts, reason);
                }
                texts.push(v);
            }
            Some(Ok(Message::Close(_))) | None | Some(Err(_)) => {
                return (texts, "socket-closed".into());
            }
            Some(Ok(_)) => {}
        }
    }
}

#[test]
fn a_terminal_without_a_size_is_eighty_by_twenty_four() {
    let n: super::sessions::NewSession = serde_json::from_str(r#"{"kind":"terminal"}"#).unwrap();
    assert_eq!((n.cols, n.rows), (80, 24));
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn an_instance_that_vanished_or_stopped_after_the_ticket_is_reported_as_such(pool: Pool) {
    let r = rig(pool).await;
    let addr = serve(r.l.state.clone()).await;

    // Stopped between ticket and connection.
    let t = r.ticket(WEB).await;
    {
        let mut all = r.mock.instances.lock().unwrap();
        all.iter_mut()
            .find(|i| i.instance.name == WEB)
            .unwrap()
            .instance
            .status = "Stopped".into();
    }
    let mut ws = connect_ws(addr, &protocols(&t), None).await.unwrap();
    let (texts, reason) = texts_until_closed(&mut ws).await;
    assert_eq!(reason, "error");
    assert_eq!(
        texts[0]["message"],
        json!("the instance is not running"),
        "{texts:?}"
    );

    // Gone altogether.
    let t = r.ticket(DB).await;
    r.mock
        .instances
        .lock()
        .unwrap()
        .retain(|i| i.instance.name != DB);
    let mut ws = connect_ws(addr, &protocols(&t), None).await.unwrap();
    let (texts, reason) = texts_until_closed(&mut ws).await;
    assert_eq!(reason, "error");
    assert_eq!(
        texts[0]["message"],
        json!("the instance no longer exists"),
        "{texts:?}"
    );
    let failed = r.audit("session.open").await;
    assert_eq!(failed.len(), 2, "both failures are on record: {failed:?}");
    assert!(failed.iter().all(|d| d["reason"].is_string()));
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn disabling_an_account_ends_its_running_session(pool: Pool) {
    let r = rig(pool).await;
    let addr = serve(r.l.state.clone().with_console(ConsoleSettings {
        recheck: Duration::from_millis(150),
        ..ConsoleSettings::default()
    }))
    .await;
    let mut ws = connect_ws(addr, &protocols(&r.ticket(WEB).await), None)
        .await
        .unwrap();
    read_until(&mut ws, "$ ").await;
    let (s, b) =
        r.l.req(
            "POST",
            &format!("/api/v1/identity/users/{}/disable", r.bob_id),
            &r.s.w.admin,
            None,
        )
        .await;
    assert!(s.is_success(), "{s} {b}");
    assert_eq!(closed_reason(&mut ws).await, "access-revoked");
}

#[sqlx::test(migrator = "apelora_persistence::MIGRATOR")]
async fn only_the_owner_or_an_operator_can_end_a_session(pool: Pool) {
    let r = rig(pool).await;
    // Another person who also holds Instance Access, but does not operate the application.
    let gary_id = r.l.user("sub-gary", "gary").await;
    let gary = r.l.token("sub-gary", "gary");
    let (s, b) =
        r.l.grant(
            &r.s.w.admin,
            gary_id,
            "instance-access",
            json!({ "type": "project", "id": r.s.w.acme_prod }),
        )
        .await;
    assert_eq!(s, StatusCode::CREATED, "{b}");
    let addr = serve(r.l.state.clone()).await;
    let mut ws = connect_ws(addr, &protocols(&r.ticket(WEB).await), None)
        .await
        .unwrap();
    read_until(&mut ws, "$ ").await;
    let (_, mine) = call(
        r.l.state.clone(),
        "GET",
        "/api/v1/sessions",
        &r.bob,
        None,
        &[],
    )
    .await;
    let id = mine["items"][0]["id"].as_str().unwrap().to_owned();
    // gary passes the route gate (he holds instance.console) but the session is not his: 404.
    let (s, _) = call(
        r.l.state.clone(),
        "DELETE",
        &format!("/api/v1/sessions/{id}"),
        &gary,
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (_, theirs) = call(
        r.l.state.clone(),
        "GET",
        "/api/v1/sessions",
        &gary,
        None,
        &[],
    )
    .await;
    assert_eq!(
        theirs["items"].as_array().unwrap().len(),
        0,
        "he does not see other people's sessions"
    );
    // The owner may end it without operating anything.
    let (s, _) = call(
        r.l.state.clone(),
        "DELETE",
        &format!("/api/v1/sessions/{id}"),
        &r.bob,
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::NO_CONTENT);
    assert_eq!(closed_reason(&mut ws).await, "terminated");
}
