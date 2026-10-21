// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora control plane (Axum REST API, `/api/v1`).
//!
//! `apelora-control-plane`                 serve the API
//! `apelora-control-plane token create --name <n> --role <r> [--expires-in-days <1-365>]`
//!                                         (90 days by default; tokens always expire)
//! `apelora-control-plane token list`
//! `apelora-control-plane token revoke <id>`
//! `apelora-control-plane identity bootstrap --issuer <iss> --subject <sub> [--username <u>]`
//!                                         make the first platform super admin (needs DATABASE_URL)
//! `apelora-control-plane openapi`         print the OpenAPI document
//!
//! With `DATABASE_URL` the API requires bearer tokens. Without it, authentication is disabled
//! for development and the server only binds to loopback. Inventory comes from Incus when
//! `APELORA_INCUS_URL` is set, otherwise from labelled mock data. A non-loopback address needs
//! TLS (`APELORA_TLS_CERT`/`APELORA_TLS_KEY`) or a declared TLS-terminating proxy
//! (`APELORA_BEHIND_PROXY=1`); see `serve`.

mod api;
mod serve;

use std::net::SocketAddr;

use apelora_domain::Role;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    init_logging();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) == Some("openapi") {
        print!("{}", api::openapi_json());
        return Ok(());
    }
    let database_url = std::env::var("DATABASE_URL").ok().filter(|u| !u.is_empty());
    if args.first().map(String::as_str) == Some("identity") {
        let url = database_url.ok_or("identity commands need DATABASE_URL")?;
        return identity_command(&args[1..], &apelora_persistence::connect(&url).await?).await;
    }
    if args.first().map(String::as_str) == Some("token") {
        let url = database_url.ok_or("token commands need DATABASE_URL")?;
        return token_command(&args[1..], &apelora_persistence::connect(&url).await?).await;
    }
    if !args.is_empty() {
        return Err(format!("unknown arguments: {}", args.join(" ")).into());
    }

    // Loopback by default; exposing the API is a deliberate configuration change.
    let addr: SocketAddr = std::env::var("APELORA_API_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8080".to_owned())
        .parse()?;
    // Decided before anything else: a reachable address never serves tokens in clear text.
    let listening = serve::listening(addr, |k| std::env::var(k).ok())?;
    let tls = match &listening {
        serve::Listening::Tls { cert, key } => Some(serve::tls_acceptor(cert, key)?),
        _ => None,
    };
    let rate_limits = api::rate_limit::RateLimitConfig::from_env(|k| std::env::var(k).ok())?;
    if listening == serve::Listening::BehindProxy && rate_limits.trusted_proxies.is_empty() {
        tracing::warn!(
            "APELORA_BEHIND_PROXY is set without APELORA_TRUSTED_PROXIES: every client shares the proxy's address in the per-address rate limits"
        );
    }
    let auth = match database_url {
        Some(url) => api::Auth::Tokens(apelora_persistence::connect(&url).await?),
        None if addr.ip().is_loopback() => {
            tracing::warn!(
                "DATABASE_URL is not set: authentication is DISABLED (development only, loopback)"
            );
            api::Auth::DisabledForDevelopment
        }
        None => {
            return Err(format!(
                "refusing to serve on {addr} without authentication; set DATABASE_URL"
            )
            .into());
        }
    };
    let provider = apelora_incus_adapter::config::provider_from_env(|k| std::env::var(k).ok())?;
    let mirrors = apelora_incus_adapter::config::image_mirrors_from_env(|k| std::env::var(k).ok())?;
    if !mirrors.is_empty() {
        tracing::info!(
            mirrors = mirrors.len(),
            mirrors_only = mirrors.mirrors_only(),
            "images come from this installation's mirrors"
        );
    }
    apelora_incus_adapter::set_image_mirrors(mirrors.clone());
    let admin = apelora_incus_adapter::config::admin_provider_from_env(
        |k| std::env::var(k).ok(),
        &provider,
    )?;
    if admin.is_none() {
        tracing::info!(
            "no Incus admin identity: cluster administration (join tokens) is unavailable"
        );
    }
    let mode = provider.mode();
    let monitoring = api::metrics::MonitoringConfig::from_env(|k| std::env::var(k).ok())?;
    apelora_metrics::set_place(monitoring.place.clone());
    apelora_metrics::set_build("control-plane", env!("CARGO_PKG_VERSION"));
    apelora_metrics::set_provider(if mode == apelora_domain::ProviderMode::Mock {
        "mock"
    } else {
        "incus"
    });
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, provider = ?mode, tls = tls.is_some(), behind_proxy = listening == serve::Listening::BehindProxy, "control plane listening");
    let projects: Vec<String> = std::env::var("APELORA_DEPLOY_PROJECTS")
        .unwrap_or_else(|_| api::DEFAULT_DEPLOY_PROJECT.to_owned())
        .split(',')
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect();
    if let Some(bad) = projects.iter().find(|p| !apelora_domain::is_valid_name(p)) {
        return Err(format!("APELORA_DEPLOY_PROJECTS: invalid project name {bad:?}").into());
    }
    tracing::info!(?projects, "deployments allowed in these Incus projects");
    let uplink = std::env::var("APELORA_NETWORK_UPLINK")
        .ok()
        .filter(|u| !u.trim().is_empty());
    if let Some(u) = &uplink {
        if !apelora_domain::is_valid_name(u) {
            return Err(format!("APELORA_NETWORK_UPLINK: invalid network name {u:?}").into());
        }
        tracing::info!(uplink = %u, "application connections are enforced on OVN networks");
    }
    let services = std::env::var("APELORA_SERVICE_ADDRESSES")
        .ok()
        .filter(|r| !r.trim().is_empty());
    if let Some(r) = &services {
        let n = apelora_domain::network::service_addresses(r)
            .map_err(|e| format!("APELORA_SERVICE_ADDRESSES: {e}"))?
            .len();
        if uplink.is_none() {
            return Err("APELORA_SERVICE_ADDRESSES needs APELORA_NETWORK_UPLINK".into());
        }
        tracing::info!(range = %r, addresses = n, "stateless modules get service addresses");
    }
    let volume_pool = std::env::var("APELORA_VOLUME_POOL")
        .ok()
        .filter(|p| !p.trim().is_empty());
    if let Some(p) = &volume_pool {
        if !apelora_domain::is_valid_name(p) {
            return Err(format!("APELORA_VOLUME_POOL: invalid pool name {p:?}").into());
        }
        tracing::info!(pool = %p, "volumes of class data-local are created in this pool");
    }
    let shared_pool = std::env::var("APELORA_SHARED_POOL")
        .ok()
        .filter(|p| !p.trim().is_empty());
    if let Some(p) = &shared_pool {
        if !apelora_domain::is_valid_name(p) {
            return Err(format!("APELORA_SHARED_POOL: invalid pool name {p:?}").into());
        }
        tracing::info!(pool = %p, "volumes of class data-replicated, and their modules' root disks, are created in this pool");
    }
    // The worker reads the same setting for scheduled backups; the control plane needs it to
    // queue backups and the safety backup of a restore.
    let backup_pool = std::env::var("APELORA_BACKUP_POOL")
        .ok()
        .filter(|p| !p.trim().is_empty());
    if let Some(p) = &backup_pool {
        if !apelora_domain::is_valid_name(p) {
            return Err(format!("APELORA_BACKUP_POOL: invalid pool name {p:?}").into());
        }
        tracing::info!(pool = %p, "volume backups go to this pool");
    }
    let oidc = oidc_from_env(|k| std::env::var(k).ok())?;
    let sign_in = sign_in_from_env(|k| std::env::var(k).ok())?;
    if sign_in.is_some() && oidc.is_none() {
        return Err(
            "APELORA_OIDC_CLIENT_ID needs APELORA_OIDC_ISSUER and APELORA_OIDC_AUDIENCE".into(),
        );
    }
    let legacy_tokens = !matches!(
        std::env::var("APELORA_LEGACY_TOKENS").ok().as_deref(),
        Some("disabled" | "off" | "false" | "0")
    );
    if !legacy_tokens {
        tracing::info!(
            "legacy API tokens are disabled: callers use OIDC or service identities, and approvals apply to everyone"
        );
    } else if oidc.is_some() {
        tracing::warn!(
            "legacy platform-wide API tokens are still accepted; set APELORA_LEGACY_TOKENS=disabled once people sign in with OIDC"
        );
    }
    let console = api::sessions::ConsoleSettings::from_env(|k| std::env::var(k).ok())?;
    if console.enabled && console.uid == 0 {
        tracing::warn!(
            "terminal sessions run as root inside instances (APELORA_CONSOLE_UID=0); prefer an unprivileged user"
        );
    }
    // The deploy identity's fingerprint, as Incus names it in its trust store: environments'
    // Incus projects are added to its project list (ADR 0012).
    let deploy_fingerprint = match (mode, std::env::var("APELORA_INCUS_CLIENT_CERT")) {
        (apelora_domain::ProviderMode::Incus, Ok(path)) => {
            let pem =
                std::fs::read(&path).map_err(|e| format!("APELORA_INCUS_CLIENT_CERT: {e}"))?;
            Some(
                api::tenancy::certificate_fingerprint(&pem)
                    .map_err(|e| format!("APELORA_INCUS_CLIENT_CERT: {e}"))?,
            )
        }
        _ => None,
    };
    let state = api::AppState::new(provider, auth)
        .with_deploy_fingerprint(deploy_fingerprint)
        .with_console(console)
        .with_oidc(oidc)
        .with_sign_in(sign_in)
        .with_legacy_tokens(legacy_tokens)
        .with_deploy_projects(projects)
        .with_admin(admin)
        .with_network_uplink(uplink)
        .with_service_range(services)
        .with_volume_pool(volume_pool)
        .with_shared_pool(shared_pool)
        .with_backup_pool(backup_pool)
        .with_rate_limits(rate_limits)
        .with_tls(listening.tls())
        .with_image_mirrors(mirrors)
        .with_db_backup_dir(
            std::env::var("APELORA_DB_BACKUP_DIR")
                .ok()
                .filter(|d| !d.trim().is_empty())
                .map(std::path::PathBuf::from),
        )
        .with_monitoring(monitoring.clone())
        .with_environment(std::env::vars());
    // Identity providers saved in the Fabric Console (on top of the environment's).
    if let Err(e) = api::auth_config::reload(&state).await {
        tracing::warn!(error = %e.message(), "saved sign-in settings could not be read; using the environment's");
    }
    tokio::spawn(api::auth_config::retry_unreachable(state.clone()));
    // Optional: metrics without authentication on their own address (loopback or a management
    // network); GET /metrics on the API address always needs fabric.metrics.read.
    if let Some(at) = monitoring.listener {
        if !at.ip().is_loopback() {
            tracing::warn!(%at, "metrics are served without authentication on a non-loopback address; keep it on a management network");
        }
        let l = tokio::net::TcpListener::bind(at).await?;
        tracing::info!(%at, "metrics listener (no authentication)");
        let router = api::metrics::listener_router(state.clone());
        tokio::spawn(async move {
            if let Err(e) = axum::serve(l, router).await {
                tracing::error!(error = %e, "metrics listener stopped");
            }
        });
    }
    let app = api::router(state).into_make_service_with_connect_info::<SocketAddr>();
    let shutdown = async {
        let _ = tokio::signal::ctrl_c().await;
    };
    match tls {
        Some(acceptor) => {
            // `tap_io` only lends the listener axum's peer-address support.
            let tls_listener = axum::serve::ListenerExt::tap_io(
                serve::TlsListener::new(listener, acceptor)?,
                |_| {},
            );
            axum::serve(tls_listener, app)
                .with_graceful_shutdown(shutdown)
                .await?
        }
        None => {
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown)
                .await?
        }
    }
    Ok(())
}

/// Browser sign-in settings: the public client the Console and the Studio use
/// (`APELORA_OIDC_CLIENT_ID`), the scopes they ask for (`APELORA_OIDC_SCOPES`, default
/// `openid profile email`) and the button's label (`APELORA_OIDC_LABEL`).
fn sign_in_from_env(
    get: impl Fn(&str) -> Option<String>,
) -> Result<Option<api::OidcSignIn>, Box<dyn std::error::Error>> {
    let Some(client_id) = get("APELORA_OIDC_CLIENT_ID").filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };
    let issuer = get("APELORA_OIDC_ISSUER").unwrap_or_default();
    if !issuer.starts_with("https://")
        && !issuer.starts_with("http://127.0.0.1")
        && !issuer.starts_with("http://localhost")
    {
        return Err("APELORA_OIDC_ISSUER must be an https:// URL for browser sign-in".into());
    }
    let scopes: Vec<String> = get("APELORA_OIDC_SCOPES")
        .unwrap_or_else(|| "openid profile email".into())
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    if !scopes.iter().any(|s| s == "openid") {
        return Err("APELORA_OIDC_SCOPES must include openid".into());
    }
    Ok(Some(api::OidcSignIn {
        issuer: issuer.trim_end_matches('/').to_owned(),
        client_id: client_id.trim().to_owned(),
        scopes,
        label: get("APELORA_OIDC_LABEL")
            .filter(|v| !v.trim().is_empty())
            .unwrap_or_else(|| "your organization".into()),
    }))
}

/// OIDC settings from the environment. Absent `APELORA_OIDC_ISSUER`, sign-in by identity provider is off.
fn oidc_from_env(
    get: impl Fn(&str) -> Option<String>,
) -> Result<Option<std::sync::Arc<apelora_identity::Verifier>>, Box<dyn std::error::Error>> {
    let Some(issuer) = get("APELORA_OIDC_ISSUER").filter(|v| !v.trim().is_empty()) else {
        return Ok(None);
    };
    let audience = get("APELORA_OIDC_AUDIENCE").filter(|v| !v.trim().is_empty()).ok_or("APELORA_OIDC_AUDIENCE is required with APELORA_OIDC_ISSUER (the identifier of this API in the identity provider)")?;
    let list = |name: &str| {
        get(name)
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_owned())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    };
    let mut cfg = apelora_identity::OidcConfig::new(issuer.trim(), audience.trim());
    cfg.audiences = list("APELORA_OIDC_AUDIENCE");
    if let Some(v) = get("APELORA_OIDC_GROUPS_CLAIM").filter(|v| !v.is_empty()) {
        cfg.groups_claim = v;
    }
    if let Some(v) = get("APELORA_OIDC_USERNAME_CLAIM").filter(|v| !v.is_empty()) {
        cfg.username_claim = v;
    }
    let amr = list("APELORA_OIDC_MFA_AMR");
    if !amr.is_empty() {
        cfg.mfa_amr = amr;
    }
    cfg.mfa_acr = list("APELORA_OIDC_MFA_ACR");
    cfg.authorized_parties = list("APELORA_OIDC_AUTHORIZED_PARTIES");
    let jwks = get("APELORA_OIDC_JWKS_URL")
        .filter(|v| !v.trim().is_empty())
        .ok_or(
            "APELORA_OIDC_JWKS_URL is required with APELORA_OIDC_ISSUER (the provider's jwks_uri)",
        )?;
    let verifier = apelora_identity::Verifier::with_remote_keys(
        cfg,
        jwks.trim(),
        std::time::Duration::from_secs(60),
    )?;
    tracing::info!(issuer = %issuer, "OIDC sign-in is enabled (not yet validated against a running identity provider)");
    Ok(Some(verifier))
}

async fn identity_command(
    args: &[String],
    pool: &apelora_persistence::Pool,
) -> Result<(), Box<dyn std::error::Error>> {
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    match args.first().map(String::as_str) {
        Some("bootstrap") => {
            let issuer = flag("--issuer").ok_or("--issuer is required (the identity provider's issuer URL)")?;
            let subject = flag("--subject").ok_or("--subject is required (the person's `sub` claim)")?;
            if apelora_persistence::identity::super_admin_count(pool).await? > 0 {
                return Err("a platform super admin already exists; use the Fabric Console to grant roles".into());
            }
            let mut tx = apelora_persistence::begin(pool).await?;
            let user = apelora_persistence::identity::upsert_user(&mut *tx, &issuer, &subject, flag("--username").as_deref(), flag("--email").as_deref(), None).await?;
            let id = apelora_persistence::identity::insert_assignment(
                &mut *tx,
                &apelora_persistence::identity::NewAssignment {
                    principal: apelora_auth::PrincipalRef::User { id: user.id },
                    role: apelora_auth::RoleRef::Builtin { key: apelora_auth::Builtin::PlatformSuperAdmin.key().into() },
                    scope: apelora_auth::Scope::Platform,
                    conditions: apelora_auth::Conditions::default(),
                    kind: apelora_auth::AssignmentKind::Standard,
                    expires_at: None,
                    reason: Some("bootstrap".into()),
                    created_by: "cli".into(),
                },
            )
            .await?;
            apelora_persistence::identity::audit_event(
                &mut *tx,
                &apelora_persistence::identity::AuditEntry { actor: "cli", actor_type: "cli", actor_name: None, action: "identity.bootstrap", target: &user.id.to_string(), outcome: "success", scope: None, detail: &json!({ "assignment": id, "issuer": issuer }) },
            )
            .await?;
            apelora_persistence::commit(tx).await?;
            println!("Platform super admin granted to {issuer} / {subject} (user {}).", user.id);
            Ok(())
        }
        _ => Err("usage: identity bootstrap --issuer <iss> --subject <sub> [--username <u>] [--email <e>]".into()),
    }
}

async fn token_command(
    args: &[String],
    pool: &apelora_persistence::Pool,
) -> Result<(), Box<dyn std::error::Error>> {
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    match args.first().map(String::as_str) {
        Some("create") => {
            let name = flag("--name").ok_or("--name is required")?;
            let role = flag("--role").ok_or("--role is required (fabric-admin, operator, studio-designer, auditor)")?;
            if Role::parse(&role).is_none() {
                return Err(format!("unknown role {role}").into());
            }
            let days = cli_token_days(flag("--expires-in-days").or_else(|| flag("--expires-days")).as_deref())?;
            let expires = time::OffsetDateTime::now_utc() + time::Duration::days(days);
            let mut tx = apelora_persistence::begin(pool).await?;
            let (secret, rec) = apelora_persistence::create_token(&mut *tx, &name, &role, Some(expires)).await?;
            apelora_persistence::audit(&mut *tx, "cli", "token.create", &rec.name, &json!({ "id": rec.id, "role": rec.role, "expiresInDays": days })).await?;
            apelora_persistence::commit(tx).await?;
            // The command exists to hand the new token to the person at the terminal, once. It is
            // never logged and never in the audit row (which holds only the token's id).
            println!("Created token {} ({}, role {}, expires in {days} days). It is shown only once:\n{secret}", rec.id, rec.name, rec.role);
        }
        Some("list") => {
            for t in apelora_persistence::list_tokens(pool).await? {
                let state = if t.revoked_at.is_some() { "revoked" } else { "active" };
                println!("{}  {:<20} {:<16} {state}", t.id, t.name, t.role);
            }
        }
        Some("revoke") => {
            let id: uuid::Uuid = args.get(1).ok_or("usage: token revoke <id>")?.parse()?;
            let mut tx = apelora_persistence::begin(pool).await?;
            apelora_persistence::revoke_token(&mut *tx, id).await?;
            apelora_persistence::audit(&mut *tx, "cli", "token.revoke", &id.to_string(), &json!({})).await?;
            apelora_persistence::commit(tx).await?;
            println!("Revoked token {id}");
        }
        _ => return Err("usage: token create --name <n> --role <r> [--expires-in-days <1-365, default 90>] | token list | token revoke <id>".into()),
    }
    Ok(())
}

/// Days a token made on the command line lives: like the API, 90 by default and 1–365 at most.
/// A token that never expires cannot be made. `--expires-days` is the earlier spelling.
fn cli_token_days(arg: Option<&str>) -> Result<i64, String> {
    let requested = arg
        .map(|d| {
            d.trim()
                .parse::<i64>()
                .map_err(|_| format!("--expires-in-days takes a whole number of days, not {d:?}"))
        })
        .transpose()?;
    api::token_lifetime_days(requested).map_err(|e| format!("--expires-in-days: {e}"))
}

/// Logging: `RUST_LOG` chooses what is written (default `info`, e.g.
/// `RUST_LOG=info,apelora_control_plane=debug`); `APELORA_LOG_FORMAT=json` writes one JSON object per line
/// instead of text. Kept the same in both binaries (`apelora-control-plane`,
/// `apelora-deployment-worker`).
fn init_logging() {
    use tracing_subscriber::EnvFilter;
    let filter = match std::env::var("RUST_LOG") {
        Ok(v) if !v.trim().is_empty() => EnvFilter::try_new(&v).unwrap_or_else(|e| {
            eprintln!("RUST_LOG={v:?} is not a valid filter ({e}); logging at info");
            EnvFilter::new("info")
        }),
        _ => EnvFilter::new("info"),
    };
    let builder = tracing_subscriber::fmt().with_env_filter(filter);
    match std::env::var("APELORA_LOG_FORMAT").as_deref() {
        // The current span (request or job) only: one copy of its fields per line.
        Ok("json") => builder.json().with_span_list(false).init(),
        Ok("" | "text") | Err(_) => builder.init(),
        Ok(other) => {
            builder.init();
            tracing::warn!(
                format = other,
                "APELORA_LOG_FORMAT must be text or json; writing text"
            );
        }
    }
}

#[cfg(test)]
mod cli_tests {
    #[test]
    fn command_line_tokens_always_expire_within_a_year() {
        use super::cli_token_days as days;
        assert_eq!(days(None), Ok(90));
        assert_eq!(days(Some("1")), Ok(1));
        assert_eq!(days(Some(" 365 ")), Ok(365));
        for refused in ["0", "366", "-5", "never", ""] {
            assert!(days(Some(refused)).is_err(), "{refused}");
        }
        assert_eq!(
            days(Some("400")),
            Err("--expires-in-days: tokens expire after 1–365 days".to_owned())
        );
    }
}
