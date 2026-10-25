// SPDX-License-Identifier: AGPL-3.0-only
//! Interactive sessions into running modules: a terminal in a Linux module, PowerShell in
//! a Windows one. The contract is the `secure-remote-access` skill.
//!
//! - **Brokered.** The browser talks only to this API. The API opens the session to the
//!   instance through the Incus adapter; no Incus address, certificate or secret reaches the
//!   browser, and the instance never gets a route to Incus.
//! - **Authorised on the exact instance.** `instance.console` on the instance's application,
//!   for a signed-in *person* (service identities and legacy tokens never hold it). An instance
//!   the caller cannot see, or that is not Apelora's, answers 404. Production needs another
//!   person's approval, spent when the ticket is issued.
//! - **Short-lived single-use tickets.** `POST …/sessions` issues a ticket (60 s, bound to the
//!   person, instance and browser origin). The websocket upgrade presents it in the
//!   `Sec-WebSocket-Protocol` header, never in a URL, and checks `Origin`.
//! - **Bounded.** Idle and absolute time limits, a cap on concurrent sessions per person, a
//!   rate limit on tickets, small messages, an input rate limit. Access is re-checked while a
//!   session runs, so a revoked role ends it.
//! - **Audited without content.** Who, which instance, start, end, why it ended, how many bytes
//!   moved. Keystrokes and screen content are never recorded.
//!
//! Sessions live in this process's memory: they do not survive a restart and are not shared
//! between several control plane replicas.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use apelora_auth::Resource;
use apelora_domain::access::{Facts, GuestOs, SessionKind, availability};
use apelora_domain::{Instance, ProviderMode, is_valid_name};
use apelora_incus_adapter::{AdapterError, ShellInput, ShellRequest};
use apelora_persistence::lifecycle as db;
use axum::extract::ws::{CloseFrame, Message, WebSocket, WebSocketUpgrade};
use axum::extract::{FromRequestParts, Path, State};
use axum::http::header::{HOST, ORIGIN, SEC_WEBSOCKET_PROTOCOL};
use axum::http::request::Parts;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::oneshot;
use utoipa::ToSchema;
use uuid::Uuid;

use super::authz::{Caller, allow, audit_alone, audit_failed, guard_on};
use super::deploy::{bad_request, store};
use super::manage::{desired_of, load, pick_deployment, resource_of};
use super::{ApiError, AppState, Auth, ErrorBody, Principal};

type Res<T> = Result<T, ApiError>;

/// The websocket sub-protocol of a session; the ticket rides along as `ticket.<value>`.
pub const PROTOCOL: &str = "apelora.session.v1";
/// Where the browser opens the websocket after getting a ticket.
pub const CONNECT_PATH: &str = "/api/v1/sessions/connect";

const MAX_INBOUND: usize = 16 * 1024;
const MAX_INBOUND_BYTES_PER_SEC: usize = 256 * 1024;
const TICKETS_PER_MINUTE: usize = 10;
const RECHECK_FAILURES_TOLERATED: u32 = 3;
/// A peer that takes longer than this to accept a message has stopped reading.
const SEND_TIMEOUT: Duration = Duration::from_secs(5);

// ---- settings ---------------------------------------------------------------------------------

/// Limits and policy for interactive sessions (environment variables, all optional).
#[derive(Debug, Clone)]
pub struct ConsoleSettings {
    /// `APELORA_CONSOLE_ACCESS=disabled` turns the whole feature off.
    pub enabled: bool,
    /// The user and group a session runs as inside the instance (`APELORA_CONSOLE_UID`,
    /// `APELORA_CONSOLE_GID`). Never root unless the operator sets 0 on purpose.
    pub uid: u32,
    pub gid: u32,
    pub cwd: String,
    /// No typing for this long ends the session (`APELORA_CONSOLE_IDLE_SECS`).
    pub idle: Duration,
    /// A session never lasts longer than this (`APELORA_CONSOLE_MAX_SECS`).
    pub max: Duration,
    pub ticket_ttl: Duration,
    /// Open sessions plus unused tickets per person (`APELORA_CONSOLE_MAX_SESSIONS`).
    pub max_per_user: usize,
    /// How often a running session checks that its person may still have it.
    pub recheck: Duration,
    /// Browser origins besides the API's own host that may open a session
    /// (`APELORA_ALLOWED_ORIGINS`, comma separated, e.g. `https://studio.example.test`).
    pub allowed_origins: Vec<String>,
    /// An RDP gateway is configured. There is none yet, so this is always false.
    pub rdp_gateway: bool,
    /// PowerShell sessions into Windows modules are turned on (`APELORA_CONSOLE_POWERSHELL=enabled`).
    /// Off by default: they have not been verified against a Windows guest.
    pub powershell: bool,
}

impl Default for ConsoleSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            uid: 1000,
            gid: 1000,
            cwd: "/tmp".into(),
            idle: Duration::from_secs(15 * 60),
            max: Duration::from_secs(60 * 60),
            ticket_ttl: Duration::from_secs(60),
            max_per_user: 3,
            recheck: Duration::from_secs(30),
            allowed_origins: Vec::new(),
            rdp_gateway: false,
            powershell: false,
        }
    }
}

impl ConsoleSettings {
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let mut s = Self::default();
        let num = |key: &str, min: u64, max: u64| -> Result<Option<u64>, String> {
            match get(key).filter(|v| !v.trim().is_empty()) {
                None => Ok(None),
                Some(v) => v
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .filter(|n| (min..=max).contains(n))
                    .map(Some)
                    .ok_or_else(|| format!("{key} must be a number from {min} to {max}")),
            }
        };
        if let Some(v) = get("APELORA_CONSOLE_ACCESS") {
            match v.trim() {
                "" | "enabled" => {}
                "disabled" | "off" => s.enabled = false,
                other => return Err(format!("APELORA_CONSOLE_ACCESS: unknown value {other:?}")),
            }
        }
        if let Some(v) = get("APELORA_CONSOLE_POWERSHELL") {
            match v.trim() {
                "" | "disabled" | "off" => {}
                "enabled" => s.powershell = true,
                other => {
                    return Err(format!(
                        "APELORA_CONSOLE_POWERSHELL: unknown value {other:?}"
                    ));
                }
            }
        }
        if let Some(n) = num("APELORA_CONSOLE_UID", 0, 4_000_000_000)? {
            s.uid = n as u32;
        }
        if let Some(n) = num("APELORA_CONSOLE_GID", 0, 4_000_000_000)? {
            s.gid = n as u32;
        }
        if let Some(n) = num("APELORA_CONSOLE_IDLE_SECS", 30, 86_400)? {
            s.idle = Duration::from_secs(n);
        }
        if let Some(n) = num("APELORA_CONSOLE_MAX_SECS", 60, 28_800)? {
            s.max = Duration::from_secs(n);
        }
        if let Some(n) = num("APELORA_CONSOLE_MAX_SESSIONS", 1, 20)? {
            s.max_per_user = n as usize;
        }
        if let Some(list) = get("APELORA_ALLOWED_ORIGINS") {
            for o in list.split(',').map(str::trim).filter(|o| !o.is_empty()) {
                let o = o.trim_end_matches('/');
                let ok = (o.starts_with("https://") || o.starts_with("http://"))
                    && !o.contains(['?', '#'])
                    && o.matches('/').count() == 2;
                if !ok {
                    return Err(format!(
                        "APELORA_ALLOWED_ORIGINS: {o:?} is not an origin like https://studio.example.test"
                    ));
                }
                s.allowed_origins.push(o.to_owned());
            }
        }
        Ok(s)
    }
}

// ---- the store of tickets and running sessions ------------------------------------------------

struct Ticket {
    id: Uuid,
    principal: Principal,
    actor: String,
    resource: Resource,
    application: String,
    project: String,
    instance: String,
    module: String,
    kind: SessionKind,
    cols: u16,
    rows: u16,
    origin: Option<String>,
    approval: Option<Uuid>,
    expires: Instant,
}

struct Running {
    actor: String,
    person: String,
    application: String,
    instance: String,
    kind: SessionKind,
    resource: Resource,
    started: time::OffsetDateTime,
    ends_by: time::OffsetDateTime,
    /// The last time the person typed (or the start). Idle time counts from here.
    last_input: time::OffsetDateTime,
    idle: Duration,
    stop: Option<oneshot::Sender<&'static str>>,
}

#[derive(Default)]
struct Inner {
    tickets: HashMap<[u8; 32], Ticket>,
    running: HashMap<Uuid, Running>,
    issued: HashMap<String, VecDeque<Instant>>,
}

#[derive(Default)]
pub struct Sessions {
    inner: Mutex<Inner>,
}

fn digest(ticket: &str) -> [u8; 32] {
    Sha256::digest(ticket.as_bytes()).into()
}

fn random_ticket() -> Result<String, ApiError> {
    let mut b = [0u8; 32];
    getrandom::fill(&mut b).map_err(|_| {
        ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "no_randomness",
            "no secure randomness is available to make a session ticket",
        )
    })?;
    Ok(b.iter().map(|x| format!("{x:02x}")).collect())
}

impl Sessions {
    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Stores a ticket unless the person is over a limit. Returns why not, if so.
    fn issue(&self, hash: [u8; 32], t: Ticket, cfg: &ConsoleSettings) -> Result<(), &'static str> {
        self.issue_at(hash, t, cfg, Instant::now())
    }

    /// [`Sessions::issue`] at a given moment (so the one-minute window can be tested).
    fn issue_at(
        &self,
        hash: [u8; 32],
        t: Ticket,
        cfg: &ConsoleSettings,
        now: Instant,
    ) -> Result<(), &'static str> {
        let mut g = self.lock();
        g.tickets.retain(|_, t| t.expires > now);
        g.issued.retain(|_, q| {
            q.back()
                .is_some_and(|at| now.duration_since(*at) <= Duration::from_secs(60))
        });
        let recent = g.issued.entry(t.actor.clone()).or_default();
        while recent
            .front()
            .is_some_and(|at| now.duration_since(*at) > Duration::from_secs(60))
        {
            recent.pop_front();
        }
        if recent.len() >= TICKETS_PER_MINUTE {
            return Err("rate");
        }
        let open = g.running.values().filter(|r| r.actor == t.actor).count()
            + g.tickets.values().filter(|x| x.actor == t.actor).count();
        if open >= cfg.max_per_user {
            return Err("cap");
        }
        g.issued.entry(t.actor.clone()).or_default().push_back(now);
        g.tickets.insert(hash, t);
        Ok(())
    }

    /// Takes back a ticket that was stored but must not be used (a later check failed).
    fn discard(&self, hash: &[u8; 32]) {
        self.lock().tickets.remove(hash);
    }

    /// Records the approval that was spent for a stored ticket.
    fn set_approval(&self, hash: &[u8; 32], approval: Option<Uuid>) {
        if let Some(t) = self.lock().tickets.get_mut(hash) {
            t.approval = approval;
        }
    }

    /// Spends a ticket (the same value never works twice) and, in the same step, counts the
    /// session as running, so the person's limit holds while Incus is still being asked.
    fn activate(
        &self,
        hash: &[u8; 32],
        cfg: &ConsoleSettings,
    ) -> Option<(Ticket, oneshot::Receiver<&'static str>)> {
        let mut g = self.lock();
        let t = g.tickets.remove(hash)?;
        if t.expires <= Instant::now() {
            return None;
        }
        let (tx, rx) = oneshot::channel();
        let now = time::OffsetDateTime::now_utc();
        g.running.insert(
            t.id,
            Running {
                actor: t.actor.clone(),
                person: t.principal.name.clone(),
                application: t.application.clone(),
                instance: t.instance.clone(),
                kind: t.kind,
                resource: t.resource.clone(),
                started: now,
                ends_by: now + cfg.max,
                last_input: now,
                idle: cfg.idle,
                stop: Some(tx),
            },
        );
        Some((t, rx))
    }

    /// Notes that the person typed, for the sessions list (the idle timer itself runs in the
    /// session's task).
    fn touch(&self, id: Uuid) {
        if let Some(r) = self.lock().running.get_mut(&id) {
            r.last_input = time::OffsetDateTime::now_utc();
        }
    }

    fn finish(&self, id: Uuid) {
        self.lock().running.remove(&id);
    }
}

/// Releases a running session's slot however its task ends: normally, by a panic, or because
/// the websocket upgrade was never carried out.
struct RunGuard {
    sessions: Arc<Sessions>,
    id: Uuid,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.sessions.finish(self.id);
    }
}

// ---- wire types -------------------------------------------------------------------------------

fn rfc3339(t: time::OffsetDateTime) -> String {
    t.format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default()
}

/// What a person may open on one instance, with the reason when not.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct KindAccess {
    pub kind: SessionKind,
    pub allowed: bool,
    /// Why it is not available (permission, state, set-up). Absent when allowed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Another person must approve first (production). The request goes through `/approvals`.
    pub approval_required: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct InstanceAccess {
    pub instance: String,
    pub module: String,
    pub guest_os: GuestOs,
    pub status: String,
    pub kinds: Vec<KindAccess>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AccessList {
    pub items: Vec<InstanceAccess>,
    /// Incus could be read just now. When false, `items` is empty and nothing may be opened.
    pub observed: bool,
    pub provider: ProviderMode,
    pub idle_secs: u64,
    pub max_secs: u64,
}

#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewSession {
    pub kind: SessionKind,
    #[serde(default = "default_cols")]
    pub cols: u16,
    #[serde(default = "default_rows")]
    pub rows: u16,
}

fn default_cols() -> u16 {
    80
}
fn default_rows() -> u16 {
    24
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionTicket {
    pub id: Uuid,
    /// Shown once. Present it as the `ticket.<value>` sub-protocol when opening the websocket;
    /// never put it in a URL.
    pub ticket: String,
    pub expires_at: String,
    /// Path of the websocket, relative to the API.
    pub connect_path: String,
    pub protocol: String,
    pub kind: SessionKind,
    pub instance: String,
    pub idle_secs: u64,
    pub max_secs: u64,
    /// Where the instance lives: a `mock` provider's sessions talk to a labelled fake shell.
    pub provider: ProviderMode,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SessionInfo {
    pub id: Uuid,
    pub application: String,
    pub instance: String,
    pub kind: SessionKind,
    pub person: String,
    pub started_at: String,
    /// The absolute limit: the session ends then whatever happens.
    pub ends_by: String,
    /// When the person last typed (the start until they do).
    pub last_activity_at: String,
    /// When the session ends unless the person types before then.
    pub idle_ends_at: String,
    /// The caller's own session.
    pub mine: bool,
}

#[derive(Serialize, ToSchema)]
pub struct SessionList {
    pub items: Vec<SessionInfo>,
}

// ---- helpers ----------------------------------------------------------------------------------

fn conflict(code: &'static str, msg: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::CONFLICT, code, msg)
}

fn not_found(what: &str) -> ApiError {
    ApiError::new(
        StatusCode::NOT_FOUND,
        "not_found",
        format!("not found: {what}"),
    )
}

fn enabled(state: &AppState) -> Res<()> {
    if state.console.enabled {
        Ok(())
    } else {
        Err(ApiError::new(
            StatusCode::SERVICE_UNAVAILABLE,
            "console_disabled",
            "interactive sessions are turned off on this control plane",
        ))
    }
}

/// The application's own instances Incus reports, with each module's guest system from the
/// recorded deployment. `None` when Incus cannot be read.
async fn instances_of(
    state: &AppState,
    row: &db::ApplicationRow,
) -> Res<Option<Vec<(Instance, GuestOs)>>> {
    let pool = store(state)?;
    let deployments = db::deployments_of(pool, row.id).await?;
    let dep = pick_deployment(row, &deployments);
    let project = dep.as_ref().map(|d| d.project.clone());
    let desired = dep.as_ref().map(desired_of).unwrap_or_default();
    let Ok(all) = state.provider.instances().await else {
        return Ok(None);
    };
    let mut out: Vec<(Instance, GuestOs)> = all
        .into_iter()
        .filter(|i| {
            i.managed
                && project.as_deref() == Some(i.project.as_str())
                && i.owner.as_ref().is_some_and(|o| o.application == row.name)
        })
        .map(|i| {
            let os = i
                .owner
                .as_ref()
                .and_then(|o| desired.iter().find(|d| d.module == o.module))
                .map(|d| d.guest_os)
                .unwrap_or_default();
            (i, os)
        })
        .collect();
    out.sort_by(|a, b| a.0.name.cmp(&b.0.name));
    Ok(Some(out))
}

fn facts_of(i: &Instance, state: &AppState) -> Facts {
    Facts {
        observed: true,
        found: true,
        status: i.status.clone(),
        unreachable: i.unreachable,
        rdp_gateway: state.console.rdp_gateway,
        powershell: state.console.powershell,
    }
}

/// Why the caller cannot use this kind of session on this application, or `None` if they can.
fn permission_problem(p: &Principal, kind: SessionKind, res: &Resource) -> Option<String> {
    if !p
        .caller
        .as_ref()
        .is_some_and(|c| c.kind == super::authz::CallerKind::User)
    {
        return Some("interactive sessions are for signed-in people".into());
    }
    if p.personal_token.is_some() {
        return Some(
            "interactive sessions need you to sign in; a personal token cannot open one".into(),
        );
    }
    allow(p, kind.permission(), res)
        .err()
        .map(|_| format!("you do not have {} on this application", kind.permission()))
}

fn needs_approval(p: &Principal, kind: SessionKind, res: &Resource) -> bool {
    p.caller
        .as_ref()
        .is_some_and(|c| c.engine.needs_approval_on(kind.permission(), res))
}

/// The approval target for one instance (`instance:<application>/<instance>`).
pub(super) fn approval_target(application: &str, instance: &str) -> String {
    format!("instance:{application}/{instance}")
}

/// The application an approval target (`instance:<application>/<instance>`) refers to, as a
/// resource. `None` when the target is malformed or the application does not exist.
pub(super) async fn approval_resource(state: &AppState, target: &str) -> Res<Option<Resource>> {
    let Some(rest) = target.strip_prefix("instance:") else {
        return Ok(None);
    };
    let Some((application, instance)) = rest.split_once('/') else {
        return Ok(None);
    };
    if !is_valid_name(application) || !is_valid_name(instance) {
        return Ok(None);
    }
    let pool = store(state)?;
    Ok(db::application_row(pool, application)
        .await?
        .map(|r| resource_of(&r)))
}

// ---- GET /applications/{name}/access ----------------------------------------------------------

/// What the caller may open on each instance of the application now, with the reason when not.
/// This is the source for the interface's menu entries. Needs `application.view`.
#[utoipa::path(get, path = "/api/v1/applications/{name}/access", tag = "sessions", operation_id = "application_access",
    params(("name" = String, Path, description = "Application name")),
    responses((status = 200, body = AccessList), (status = 404, body = ErrorBody)))]
pub(super) async fn access(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(name): Path<String>,
) -> Res<Json<AccessList>> {
    let (row, res) = load(&state, &p, &name).await?;
    let found = instances_of(&state, &row).await?;
    let observed = found.is_some();
    let items = found
        .unwrap_or_default()
        .into_iter()
        .map(|(i, os)| {
            let facts = facts_of(&i, &state);
            let kinds = SessionKind::offered_for(os)
                .iter()
                .map(|&kind| {
                    let problem = if !state.console.enabled {
                        Some("interactive sessions are turned off on this control plane".to_owned())
                    } else {
                        permission_problem(&p, kind, &res)
                            .or_else(|| availability(kind, &facts).err())
                    };
                    KindAccess {
                        kind,
                        allowed: problem.is_none(),
                        reason: problem,
                        approval_required: needs_approval(&p, kind, &res),
                    }
                })
                .collect();
            InstanceAccess {
                module: i
                    .owner
                    .as_ref()
                    .map(|o| o.module.clone())
                    .unwrap_or_default(),
                instance: i.name,
                guest_os: os,
                status: i.status,
                kinds,
            }
        })
        .collect();
    Ok(Json(AccessList {
        items,
        observed,
        provider: state.provider.mode(),
        idle_secs: state.console.idle.as_secs(),
        max_secs: state.console.max.as_secs(),
    }))
}

// ---- POST /applications/{name}/instances/{instance}/sessions ----------------------------------

fn origin_of(headers: &HeaderMap) -> Option<String> {
    headers
        .get(ORIGIN)
        .and_then(|v| v.to_str().ok())
        .map(|o| o.trim_end_matches('/').to_ascii_lowercase())
}

/// Opens a session: checks the caller, the instance and the approval rule, and answers with a
/// single-use ticket for the websocket. Needs `instance.console` (terminal, PowerShell) or
/// `instance.desktop` (RDP) on the application; in production also an approval
/// (`Apelora-Approval: <id>`) of `instance.console` on `instance:<application>/<instance>`.
#[utoipa::path(post, path = "/api/v1/applications/{name}/instances/{instance}/sessions", tag = "sessions", operation_id = "open_session",
    params(("name" = String, Path, description = "Application name"), ("instance" = String, Path, description = "Instance name")),
    request_body = NewSession,
    responses((status = 201, body = SessionTicket), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody),
        (status = 404, body = ErrorBody), (status = 409, body = ErrorBody), (status = 429, body = ErrorBody)))]
pub(super) async fn open(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path((name, instance)): Path<(String, String)>,
    headers: HeaderMap,
    body: Result<Json<NewSession>, axum::extract::rejection::JsonRejection>,
) -> Res<(StatusCode, Json<SessionTicket>)> {
    enabled(&state)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    if !is_valid_name(&instance) {
        return Err(bad_request("invalid instance name"));
    }
    if !(1..=1000).contains(&req.cols) || !(1..=1000).contains(&req.rows) {
        return Err(bad_request("the terminal size is out of range"));
    }
    let (row, res) = load(&state, &p, &name).await?;
    let pool = store(&state)?;
    let target = approval_target(&name, &instance);
    let deny = |outcome_reason: &str| {
        let (pool, p, res, target) = (pool.clone(), p.clone(), res.clone(), target.clone());
        let reason = outcome_reason.to_owned();
        async move {
            audit_failed(
                "session.denied",
                audit_alone(
                    &pool,
                    &p,
                    "session.denied",
                    &target,
                    "denied",
                    Some(&res),
                    &json!({ "reason": reason, "kind": req.kind.as_str() }),
                )
                .await,
            );
        }
    };

    // The exact instance: Apelora's, in this application, and visible to the caller (404 otherwise).
    let Some(found) = instances_of(&state, &row).await? else {
        deny("incus unreadable").await;
        return Err(conflict(
            "instance_state_unknown",
            "the control plane cannot read Incus, so it does not know whether the instance is running; no session was opened",
        ));
    };
    let Some((inst, os)) = found.into_iter().find(|(i, _)| i.name == instance) else {
        return Err(not_found(&format!(
            "instance {instance} of application {name}"
        )));
    };
    if !SessionKind::offered_for(os).contains(&req.kind) {
        return Err(bad_request(format!(
            "a {} module does not offer a {} session",
            match os {
                GuestOs::Linux => "Linux",
                GuestOs::Windows => "Windows",
            },
            req.kind.as_str()
        )));
    }
    if let Some(problem) = permission_problem(&p, req.kind, &res) {
        deny(&problem).await;
        return Err(ApiError::new(StatusCode::FORBIDDEN, "forbidden", problem));
    }
    if let Err(reason) = availability(req.kind, &facts_of(&inst, &state)) {
        deny(&reason).await;
        return Err(conflict("session_unavailable", reason));
    }
    // Reserve the slot before spending the approval, so a refusal by a limit never burns a
    // single-use approval another person had to grant.
    let raw = random_ticket()?;
    let hash = digest(&raw);
    let id = Uuid::new_v4();
    let ticket = Ticket {
        id,
        principal: p.clone(),
        actor: p.actor_ref(),
        resource: res.clone(),
        application: name.clone(),
        project: inst.project.clone(),
        instance: instance.clone(),
        module: inst
            .owner
            .as_ref()
            .map(|o| o.module.clone())
            .unwrap_or_default(),
        kind: req.kind,
        cols: req.cols,
        rows: req.rows,
        origin: origin_of(&headers),
        approval: None,
        expires: Instant::now() + state.console.ticket_ttl,
    };
    if let Err(why) = state.sessions.issue(hash, ticket, &state.console) {
        deny(why).await;
        let msg = if why == "rate" {
            "too many session requests; wait a minute"
        } else {
            "you already have the most sessions allowed; close one first"
        };
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_sessions",
            msg,
        ));
    }
    let approval = match guard_on(&state, &p, &headers, req.kind.permission(), &target, &res).await
    {
        Ok(a) => a,
        Err(e) => {
            state.sessions.discard(&hash);
            deny("approval").await;
            return Err(e);
        }
    };
    state.sessions.set_approval(&hash, approval);
    if let Err(e) = audit_alone(
        pool,
        &p,
        "session.request",
        &target,
        "success",
        Some(&res),
        &json!({ "session": id, "kind": req.kind.as_str(), "approval": approval }),
    )
    .await
    {
        // No record, no session.
        state.sessions.discard(&hash);
        return Err(e.into());
    }
    let expires = time::OffsetDateTime::now_utc() + state.console.ticket_ttl;
    Ok((
        StatusCode::CREATED,
        Json(SessionTicket {
            id,
            ticket: raw,
            expires_at: rfc3339(expires),
            connect_path: CONNECT_PATH.into(),
            protocol: PROTOCOL.into(),
            kind: req.kind,
            instance,
            idle_secs: state.console.idle.as_secs(),
            max_secs: state.console.max.as_secs(),
            provider: state.provider.mode(),
        }),
    ))
}

// ---- GET /sessions, DELETE /sessions/{id} -----------------------------------------------------

/// Open sessions the caller started, and those on applications they may operate.
#[utoipa::path(get, path = "/api/v1/sessions", tag = "sessions", operation_id = "sessions",
    responses((status = 200, body = SessionList), (status = 401, body = ErrorBody)))]
pub(super) async fn list(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Res<Json<SessionList>> {
    let actor = p.actor_ref();
    let g = state.sessions.lock();
    let mut items: Vec<SessionInfo> = g
        .running
        .iter()
        .filter(|(_, r)| r.actor == actor || allow(&p, "application.operate", &r.resource).is_ok())
        .map(|(id, r)| SessionInfo {
            id: *id,
            application: r.application.clone(),
            instance: r.instance.clone(),
            kind: r.kind,
            person: r.person.clone(),
            started_at: rfc3339(r.started),
            ends_by: rfc3339(r.ends_by),
            last_activity_at: rfc3339(r.last_input),
            idle_ends_at: rfc3339(r.last_input + r.idle),
            mine: r.actor == actor,
        })
        .collect();
    items.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    Ok(Json(SessionList { items }))
}

/// Ends a session now. Allowed for the person who has it and for whoever may operate its
/// application. Anything else answers 404.
#[utoipa::path(delete, path = "/api/v1/sessions/{id}", tag = "sessions", operation_id = "terminate_session",
    params(("id" = Uuid, Path, description = "Session id")),
    responses((status = 204), (status = 404, body = ErrorBody)))]
pub(super) async fn terminate(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    Path(id): Path<Uuid>,
) -> Res<StatusCode> {
    let pool = store(&state)?;
    let actor = p.actor_ref();
    let (stop, res, target) = {
        let mut g = state.sessions.lock();
        let Some(r) = g.running.get_mut(&id) else {
            return Err(not_found("session"));
        };
        if r.actor != actor && allow(&p, "application.operate", &r.resource).is_err() {
            return Err(not_found("session"));
        }
        (
            r.stop.take(),
            r.resource.clone(),
            approval_target(&r.application, &r.instance),
        )
    };
    audit_alone(
        pool,
        &p,
        "session.terminate",
        &target,
        "success",
        Some(&res),
        &json!({ "session": id }),
    )
    .await?;
    if let Some(tx) = stop {
        let _ = tx.send("terminated");
    }
    Ok(StatusCode::NO_CONTENT)
}

// ---- GET /sessions/connect (websocket) --------------------------------------------------------

/// The client's address: the connection's peer, or the address a trusted proxy
/// (`APELORA_TRUSTED_PROXIES`) forwarded for, as the rate limits see it
/// (`rate_limit::client_ip`). `None` when the server runs without connection information.
pub(super) struct ClientAddress(pub(super) Option<IpAddr>);

impl FromRequestParts<AppState> for ClientAddress {
    type Rejection = std::convert::Infallible;
    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        Ok(ClientAddress(
            state
                .rate_limits
                .client_of(&parts.extensions, &parts.headers),
        ))
    }
}

fn ticket_from(headers: &HeaderMap) -> Option<String> {
    headers
        .get_all(SEC_WEBSOCKET_PROTOCOL)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(str::trim)
        .find_map(|p| p.strip_prefix("ticket.").map(str::to_owned))
}

/// A browser's `Origin` is acceptable when it is this API's own host or one of the configured
/// origins. A request without `Origin` (not a browser) passes this test but must then match a
/// ticket that was also issued without one.
fn origin_allowed(origin: Option<&str>, host: Option<&str>, allowed: &[String]) -> bool {
    let Some(o) = origin else { return true };
    if allowed.iter().any(|a| a.eq_ignore_ascii_case(o)) {
        return true;
    }
    let authority = o
        .strip_prefix("https://")
        .or_else(|| o.strip_prefix("http://"));
    matches!((authority, host), (Some(a), Some(h)) if a.eq_ignore_ascii_case(h))
}

fn refuse(status: StatusCode, code: &'static str, msg: &'static str) -> Response {
    ApiError::new(status, code, msg).into_response()
}

/// The websocket of a session. Authenticated by its ticket alone, which is why it sits outside
/// the bearer-token layer: browsers cannot set headers on a websocket.
pub(super) async fn connect(
    State(state): State<AppState>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
    ClientAddress(client): ClientAddress,
) -> Response {
    if !state.console.enabled {
        return refuse(
            StatusCode::SERVICE_UNAVAILABLE,
            "console_disabled",
            "interactive sessions are turned off on this control plane",
        );
    }
    let origin = origin_of(&headers);
    let host = headers.get(HOST).and_then(|v| v.to_str().ok());
    if !origin_allowed(origin.as_deref(), host, &state.console.allowed_origins) {
        return refuse(
            StatusCode::FORBIDDEN,
            "origin_not_allowed",
            "this page may not open sessions on this control plane",
        );
    }
    let Some(raw) = ticket_from(&headers) else {
        return refuse(
            StatusCode::UNAUTHORIZED,
            "ticket_required",
            "a session ticket is required",
        );
    };
    // Spending the ticket also counts the session as running, until the guard is dropped.
    let Some((ticket, stop)) = state.sessions.activate(&digest(&raw), &state.console) else {
        return refuse(
            StatusCode::UNAUTHORIZED,
            "ticket_invalid",
            "the session ticket is unknown, used or expired",
        );
    };
    let guard = RunGuard {
        sessions: state.sessions.clone(),
        id: ticket.id,
    };
    if ticket.origin != origin {
        // Spent above, so a stolen ticket cannot be retried from the right place either.
        if let Auth::Tokens(pool) = &state.auth {
            audit_failed(
                "session.denied",
                audit_alone(
                    pool,
                    &ticket.principal,
                    "session.denied",
                    &approval_target(&ticket.application, &ticket.instance),
                    "denied",
                    Some(&ticket.resource),
                    &json!({ "reason": "origin differs from the one the ticket was issued to" }),
                )
                .await,
            );
        }
        return refuse(
            StatusCode::FORBIDDEN,
            "origin_not_allowed",
            "the session ticket was issued to another page",
        );
    }
    ws.protocols([PROTOCOL])
        .max_message_size(MAX_INBOUND)
        .max_frame_size(MAX_INBOUND)
        .on_upgrade(move |socket| run(state, ticket, stop, guard, socket, client))
}

fn closing(reason: &'static str) -> Message {
    Message::Text(
        json!({ "type": "closed", "reason": reason })
            .to_string()
            .into(),
    )
}

/// Sends to the browser, giving up if it has stopped reading: a stalled client must not hold the
/// session's timers, limits and cleanup hostage.
async fn ws_send(socket: &mut WebSocket, m: Message) -> bool {
    matches!(
        tokio::time::timeout(SEND_TIMEOUT, socket.send(m)).await,
        Ok(Ok(()))
    )
}

async fn send_close(socket: &mut WebSocket, reason: &'static str) {
    let _ = ws_send(socket, closing(reason)).await;
    let _ = ws_send(
        socket,
        Message::Close(Some(CloseFrame {
            code: 1000,
            reason: reason.into(),
        })),
    )
    .await;
}

/// Does the person still hold the permission, and is the account still enabled? Re-reads the
/// account, their group memberships recorded in Apelora, and their assignments. Groups that
/// mirror an identity-provider group keep the value from sign-in: the provider cannot be asked
/// again without a token.
async fn still_allowed(state: &AppState, t: &Ticket) -> Option<bool> {
    let Auth::Tokens(pool) = &state.auth else {
        return Some(false);
    };
    let c: &Caller = t.principal.caller.as_deref()?;
    // `None` (cannot tell) is logged: repeated, it ends the session as `access-unverifiable`.
    let unverifiable = |what: &str, e: &dyn std::fmt::Display| {
        tracing::warn!(session = %t.id, error = %e, "session: cannot re-check access ({what})");
    };
    match apelora_persistence::identity::user(pool, c.id).await {
        Ok(Some(u)) if u.disabled_at.is_none() => {}
        Ok(_) => return Some(false),
        Err(e) => {
            unverifiable("account", &e);
            return None;
        }
    }
    let mut groups: BTreeSet<Uuid> = apelora_persistence::identity::member_groups(pool, c.id)
        .await
        .map_err(|e| unverifiable("groups", &e))
        .ok()?;
    groups.extend(
        apelora_persistence::identity::external_groups(pool, &c.subject.groups)
            .await
            .map_err(|e| unverifiable("identity-provider groups", &e))
            .ok()?,
    );
    match super::caller_for(pool, c.kind, c.id, groups, c.subject.mfa).await {
        Ok(fresh) => Some(fresh.check(t.kind.permission(), &t.resource).allowed),
        Err(e) => {
            unverifiable("role assignments", &e.message);
            None
        }
    }
}

async fn run(
    state: AppState,
    ticket: Ticket,
    mut stop: oneshot::Receiver<&'static str>,
    _guard: RunGuard,
    mut socket: WebSocket,
    client: Option<IpAddr>,
) {
    let cfg = state.console.clone();
    let target = approval_target(&ticket.application, &ticket.instance);
    let pool = match &state.auth {
        Auth::Tokens(pool) => pool.clone(),
        Auth::DisabledForDevelopment => {
            send_close(&mut socket, "error").await;
            return;
        }
    };
    let Some(command) = ticket.kind.command() else {
        send_close(&mut socket, "unsupported").await;
        return;
    };
    let source = client.map(|a| a.to_string());
    // The ticket can be up to a minute old: check the person once more before opening anything.
    match still_allowed(&state, &ticket).await {
        Some(true) => {}
        other => {
            let reason = if other == Some(false) {
                "access-revoked"
            } else {
                "access-unverifiable"
            };
            audit_failed(
                "session.open",
                audit_alone(
                    &pool,
                    &ticket.principal,
                    "session.open",
                    &target,
                    "denied",
                    Some(&ticket.resource),
                    &json!({ "session": ticket.id, "reason": reason, "source": source }),
                )
                .await,
            );
            send_close(&mut socket, reason).await;
            return;
        }
    }
    // Numeric users mean nothing in a Windows guest; do not pretend to drop privileges there.
    let linux = ticket.kind == SessionKind::Terminal;
    let req = ShellRequest {
        project: ticket.project.clone(),
        name: ticket.instance.clone(),
        command: command.iter().map(|s| (*s).to_owned()).collect(),
        uid: linux.then_some(cfg.uid),
        gid: linux.then_some(cfg.gid),
        cwd: linux.then(|| cfg.cwd.clone()),
        env: if linux {
            [
                ("TERM".to_owned(), "xterm-256color".to_owned()),
                ("HOME".to_owned(), cfg.cwd.clone()),
                ("LANG".to_owned(), "C.UTF-8".to_owned()),
            ]
            .into()
        } else {
            [("TERM".to_owned(), "xterm-256color".to_owned())].into()
        },
        cols: ticket.cols,
        rows: ticket.rows,
    };
    let mut shell = match state.provider.open_shell(&req).await {
        Ok(s) => s,
        Err(e) => {
            let why = match &e {
                AdapterError::NotFound(_) => "the instance no longer exists",
                AdapterError::Api { status: 409, .. } => "the instance is not running",
                _ => "the control plane could not reach the instance",
            };
            tracing::warn!(session = %ticket.id, error = %e, "session could not be opened");
            audit_failed(
                "session.open",
                audit_alone(
                    &pool,
                    &ticket.principal,
                    "session.open",
                    &target,
                    "failed",
                    Some(&ticket.resource),
                    &json!({ "session": ticket.id, "kind": ticket.kind.as_str(), "reason": why, "source": source }),
                )
            .await,
            );
            let _ = ws_send(
                &mut socket,
                Message::Text(
                    json!({ "type": "error", "message": why })
                        .to_string()
                        .into(),
                ),
            )
            .await;
            send_close(&mut socket, "error").await;
            return;
        }
    };
    // A session that cannot be recorded is not allowed to run.
    if audit_alone(
        &pool,
        &ticket.principal,
        "session.open",
        &target,
        "success",
        Some(&ticket.resource),
        &json!({
            "session": ticket.id, "kind": ticket.kind.as_str(), "approval": ticket.approval,
            "source": source, "module": ticket.module, "runsAs": linux.then_some(cfg.uid),
            "provider": state.provider.mode(),
        }),
    )
    .await
    .map_err(|e| tracing::error!(session = %ticket.id, error = %e, "audit: could not record session.open; the session is refused"))
    .is_err()
    {
        drop(shell);
        send_close(&mut socket, "error").await;
        return;
    }

    let started = Instant::now();
    let max_at = tokio::time::Instant::now() + cfg.max;
    let mut idle_at = tokio::time::Instant::now() + cfg.idle;
    let mut recheck = tokio::time::interval(cfg.recheck);
    recheck.reset();
    let (mut bytes_in, mut bytes_out) = (0usize, 0usize);
    let (mut window_start, mut window_bytes) = (Instant::now(), 0usize);
    let mut recheck_failures = 0u32;
    // The sessions list shows the last input; noting it at most once a second keeps the shared
    // lock off the typing path.
    let mut noted = Instant::now();

    // `biased`: the limits and the stop signal are looked at before any traffic, and every send
    // is bounded, so neither a flood of output nor a peer that stopped reading can keep a
    // session alive past its limits.
    let reason: &'static str = loop {
        tokio::select! {
            biased;
            _ = tokio::time::sleep_until(max_at) => break "max-duration",
            why = &mut stop => break why.unwrap_or("terminated"),
            _ = tokio::time::sleep_until(idle_at) => break "idle",
            _ = recheck.tick() => {
                match still_allowed(&state, &ticket).await {
                    Some(true) => recheck_failures = 0,
                    Some(false) => break "access-revoked",
                    None => {
                        recheck_failures += 1;
                        if recheck_failures >= RECHECK_FAILURES_TOLERATED { break "access-unverifiable"; }
                    }
                }
            }
            out = shell.output.recv() => match out {
                Some(bytes) => {
                    bytes_out += bytes.len();
                    if !ws_send(&mut socket, Message::Binary(bytes.into())).await { break "stalled"; }
                }
                None => break "exited",
            },
            msg = socket.recv() => {
                let size = match &msg {
                    Some(Ok(Message::Binary(b))) => b.len(),
                    Some(Ok(Message::Text(t))) => t.len(),
                    _ => 0,
                };
                if size > 0 {
                    if window_start.elapsed() >= Duration::from_secs(1) {
                        (window_start, window_bytes) = (Instant::now(), 0);
                    }
                    window_bytes += size;
                    if window_bytes > MAX_INBOUND_BYTES_PER_SEC { break "rate-limited"; }
                }
                match msg {
                    Some(Ok(Message::Binary(b))) => {
                        bytes_in += b.len();
                        idle_at = tokio::time::Instant::now() + cfg.idle;
                        if noted.elapsed() >= Duration::from_secs(1) {
                            noted = Instant::now();
                            state.sessions.touch(ticket.id);
                        }
                        let sent = tokio::time::timeout(SEND_TIMEOUT, shell.input.send(ShellInput::Data(b.to_vec()))).await;
                        match sent {
                            Ok(Ok(())) => {}
                            Ok(Err(_)) => break "exited",
                            Err(_) => break "stalled",
                        }
                    }
                    Some(Ok(Message::Text(t))) => {
                        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&t)
                            && v["type"] == "resize"
                        {
                            let dim = |k: &str| v[k].as_u64().filter(|n| (1..=1000).contains(n)).map(|n| n as u16);
                            if let (Some(cols), Some(rows)) = (dim("cols"), dim("rows")) {
                                // Never wait on a resize; a full queue drops it.
                                let _ = shell.input.try_send(ShellInput::Resize { cols, rows });
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => break "client-closed",
                    Some(Ok(_)) => {}
                }
            },
        }
    };

    // Dropping the session hangs the process up and closes both channels to Incus; nothing waits
    // on a peer that may have stopped reading.
    drop(shell);
    send_close(&mut socket, reason).await;
    audit_failed(
        "session.close",
        audit_alone(
            &pool,
            &ticket.principal,
            "session.close",
            &target,
            "success",
            Some(&ticket.resource),
            &json!({
                "session": ticket.id, "reason": reason,
                "seconds": started.elapsed().as_secs(), "bytesIn": bytes_in, "bytesOut": bytes_out,
            }),
        )
        .await,
    );
}

#[cfg(test)]
#[allow(clippy::unwrap_used)] // test fixtures: a failed unwrap is the failure
mod tests {
    use super::*;

    #[test]
    fn origins_must_match_the_host_or_the_allow_list() {
        let allowed = vec!["https://studio.example.test".to_owned()];
        assert!(origin_allowed(None, Some("api.example.test"), &allowed));
        assert!(origin_allowed(
            Some("https://api.example.test"),
            Some("api.example.test"),
            &allowed
        ));
        assert!(origin_allowed(
            Some("https://studio.example.test"),
            Some("api.example.test"),
            &allowed
        ));
        assert!(!origin_allowed(
            Some("https://evil.example.test"),
            Some("api.example.test"),
            &allowed
        ));
        assert!(!origin_allowed(
            Some("null"),
            Some("api.example.test"),
            &allowed
        ));
        assert!(!origin_allowed(
            Some("https://api.example.test.evil.test"),
            Some("api.example.test"),
            &[]
        ));
    }

    /// The Studio and the Fabric Console are two doors into the same sessions. When they reach
    /// the API on a host other than their own, each is listed by name; nothing else gets in.
    #[test]
    fn the_studio_and_the_console_are_listed_explicitly_and_a_third_origin_is_refused() {
        let s = ConsoleSettings::from_env(|k| {
            (k == "APELORA_ALLOWED_ORIGINS")
                .then(|| "http://127.0.0.1:5173, http://127.0.0.1:5174".to_owned())
        })
        .unwrap();
        let api = Some("127.0.0.1:8080");
        let ok = |o: &str| origin_allowed(Some(o), api, &s.allowed_origins);
        assert!(ok("http://127.0.0.1:5173"), "Studio");
        assert!(ok("http://127.0.0.1:5174"), "Fabric Console");
        assert!(ok("http://127.0.0.1:8080"), "the API's own host");
        for third in [
            "http://127.0.0.1:5175",
            "http://localhost:5174",
            "https://127.0.0.1:5174",
            "http://127.0.0.1:51740",
            "http://127.0.0.1",
        ] {
            assert!(!ok(third), "{third} is not configured");
        }
        // No wildcard: a `*` entry is refused as configuration.
        assert!(
            ConsoleSettings::from_env(|k| (k == "APELORA_ALLOWED_ORIGINS").then(|| "*".to_owned()))
                .is_err()
        );
    }

    #[test]
    fn the_ticket_is_read_only_from_the_protocol_header() {
        let mut h = HeaderMap::new();
        h.append(
            SEC_WEBSOCKET_PROTOCOL,
            "apelora.session.v1, ticket.abc123".parse().unwrap(),
        );
        assert_eq!(ticket_from(&h).as_deref(), Some("abc123"));
        assert_eq!(ticket_from(&HeaderMap::new()), None);
    }

    #[test]
    fn settings_are_validated() {
        let env = |pairs: &'static [(&'static str, &'static str)]| {
            ConsoleSettings::from_env(move |k| {
                pairs
                    .iter()
                    .find(|(n, _)| *n == k)
                    .map(|(_, v)| (*v).to_owned())
            })
        };
        let d = env(&[]).unwrap();
        assert!(
            d.enabled && d.uid == 1000 && d.max_per_user == 3,
            "not root by default"
        );
        assert!(
            !env(&[("APELORA_CONSOLE_ACCESS", "disabled")])
                .unwrap()
                .enabled
        );
        assert!(env(&[("APELORA_CONSOLE_ACCESS", "maybe")]).is_err());
        assert!(env(&[("APELORA_CONSOLE_IDLE_SECS", "5")]).is_err());
        assert!(env(&[("APELORA_CONSOLE_MAX_SECS", "999999")]).is_err());
        assert!(env(&[("APELORA_ALLOWED_ORIGINS", "studio.example.test")]).is_err());
        assert!(env(&[("APELORA_ALLOWED_ORIGINS", "https://a.test/path")]).is_err());
        assert_eq!(
            env(&[(
                "APELORA_ALLOWED_ORIGINS",
                "https://a.test/, http://b.test:5173"
            )])
            .unwrap()
            .allowed_origins,
            vec!["https://a.test".to_owned(), "http://b.test:5173".to_owned()]
        );
    }

    #[test]
    fn tickets_are_long_random_and_never_repeat() {
        let Ok(a) = random_ticket() else {
            panic!("randomness")
        };
        let Ok(b) = random_ticket() else {
            panic!("randomness")
        };
        assert_ne!(a, b);
        for t in [&a, &b] {
            assert_eq!(t.len(), 64, "256 bits as hex");
            assert!(t.bytes().all(|c| c.is_ascii_hexdigit()));
        }
        assert_ne!(digest(&a), digest(&b));
        assert_eq!(digest("x"), digest("x"));
    }

    #[test]
    fn the_limits_are_what_the_documentation_says() {
        assert_eq!(MAX_INBOUND, 16 * 1024);
        assert_eq!(MAX_INBOUND_BYTES_PER_SEC, 256 * 1024);
        assert_eq!(TICKETS_PER_MINUTE, 10);
        let d = ConsoleSettings::default();
        assert_eq!(d.idle, Duration::from_secs(15 * 60));
        assert_eq!(d.max, Duration::from_secs(60 * 60));
        assert_eq!(d.ticket_ttl, Duration::from_secs(60));
        assert_eq!(d.recheck, Duration::from_secs(30));
    }

    #[test]
    fn a_ticket_stops_counting_the_moment_it_expires() {
        let store = Sessions::default();
        let cfg = ConsoleSettings {
            max_per_user: 1,
            ..ConsoleSettings::default()
        };
        let t0 = Instant::now();
        let ticket = |expires: Instant| Ticket {
            id: Uuid::new_v4(),
            principal: Principal {
                name: "a".into(),
                role: None,
                caller: None,
                session: None,
                personal_token: None,
            },
            actor: "user:1".into(),
            resource: Resource::default(),
            application: "shop".into(),
            project: "p".into(),
            instance: "web".into(),
            module: "web".into(),
            kind: SessionKind::Terminal,
            cols: 80,
            rows: 24,
            origin: None,
            approval: None,
            expires,
        };
        let ttl = Duration::from_secs(60);
        assert!(store.issue_at([1; 32], ticket(t0 + ttl), &cfg, t0).is_ok());
        let second = |at: Duration| store.issue_at([2; 32], ticket(t0 + at + ttl), &cfg, t0 + at);
        assert_eq!(
            second(ttl - Duration::from_secs(1)),
            Err("cap"),
            "still valid, still counted"
        );
        assert_eq!(
            second(ttl),
            Ok(()),
            "expired at that very moment, so it no longer counts"
        );
    }

    #[test]
    fn ten_tickets_a_minute_and_the_minute_is_exactly_sixty_seconds() {
        let store = Sessions::default();
        let cfg = ConsoleSettings {
            max_per_user: 100,
            ..ConsoleSettings::default()
        };
        let t0 = Instant::now();
        let make = |n: usize| Ticket {
            id: Uuid::new_v4(),
            principal: Principal {
                name: "a".into(),
                role: None,
                caller: None,
                session: None,
                personal_token: None,
            },
            actor: "user:1".into(),
            resource: Resource::default(),
            application: "shop".into(),
            project: "p".into(),
            instance: "web".into(),
            module: "web".into(),
            kind: SessionKind::Terminal,
            cols: 80,
            rows: 24,
            origin: None,
            approval: None,
            // Long enough that expiry never interferes with the window under test.
            expires: t0 + Duration::from_secs(10_000 + n as u64),
        };
        for n in 0..TICKETS_PER_MINUTE {
            assert!(
                store.issue_at([n as u8; 32], make(n), &cfg, t0).is_ok(),
                "{n}"
            );
        }
        let eleventh = |at: Duration| store.issue_at([200; 32], make(99), &cfg, t0 + at);
        assert_eq!(eleventh(Duration::from_secs(59)), Err("rate"));
        assert_eq!(
            eleventh(Duration::from_secs(60)),
            Err("rate"),
            "still inside the minute"
        );
        assert_eq!(
            eleventh(Duration::from_secs(61)),
            Ok(()),
            "the minute has passed"
        );
    }

    #[test]
    fn a_ticket_works_once_and_the_limits_hold() {
        let store = Sessions::default();
        let cfg = ConsoleSettings {
            max_per_user: 2,
            ..ConsoleSettings::default()
        };
        let principal = Principal {
            name: "alice".into(),
            role: None,
            caller: None,
            session: None,
            personal_token: None,
        };
        let make = |actor: &str| Ticket {
            id: Uuid::new_v4(),
            principal: principal.clone(),
            actor: actor.into(),
            resource: Resource::default(),
            application: "shop".into(),
            project: "p".into(),
            instance: "web".into(),
            module: "web".into(),
            kind: SessionKind::Terminal,
            cols: 80,
            rows: 24,
            origin: None,
            approval: None,
            expires: Instant::now() + Duration::from_secs(60),
        };
        assert!(store.issue(digest("a"), make("user:1"), &cfg).is_ok());
        assert!(store.issue(digest("b"), make("user:1"), &cfg).is_ok());
        assert_eq!(
            store.issue(digest("c"), make("user:1"), &cfg),
            Err("cap"),
            "a third open ticket is over the cap"
        );
        assert!(store.issue(digest("d"), make("user:2"), &cfg).is_ok());
        assert!(store.activate(&digest("a"), &cfg).is_some());
        assert!(store.activate(&digest("a"), &cfg).is_none(), "single use");
        assert!(store.activate(&digest("zzz"), &cfg).is_none());
        // A session that is starting still counts against the person's limit.
        assert_eq!(
            store.issue(digest("e"), make("user:1"), &cfg),
            Err("cap"),
            "one running plus one waiting is the limit"
        );
        let mut late = make("user:3");
        late.expires = Instant::now() - Duration::from_secs(1);
        let _ = store.issue(digest("late"), late, &cfg);
        assert!(store.activate(&digest("late"), &cfg).is_none(), "expired");
    }
}
