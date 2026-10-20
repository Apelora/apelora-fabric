// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora control plane (Axum REST API, `/api/v1`).
//!
//! `apelora-control-plane`                 serve the API
//! `apelora-control-plane token create --name <n> --role <r> [--expires-days <d>]`
//! `apelora-control-plane token list`
//! `apelora-control-plane token revoke <id>`
//! `apelora-control-plane identity bootstrap --issuer <iss> --subject <sub> [--username <u>]`
//!                                         make the first platform super admin (needs DATABASE_URL)
//! `apelora-control-plane openapi`         print the OpenAPI document
//!
//! With `DATABASE_URL` the API requires bearer tokens. Without it, authentication is disabled
//! for development and the server only binds to loopback. Inventory comes from Incus when
//! `APELORA_INCUS_URL` is set, otherwise from labelled mock data.

mod api;

use std::net::SocketAddr;

use apelora_domain::Role;
use serde_json::json;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt().init();
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
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!(%addr, provider = ?mode, "control plane listening");
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
    let oidc = oidc_from_env(|k| std::env::var(k).ok())?;
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
    let state = api::AppState::new(provider, auth)
        .with_console(console)
        .with_oidc(oidc)
        .with_legacy_tokens(legacy_tokens)
        .with_deploy_projects(projects)
        .with_admin(admin)
        .with_network_uplink(uplink)
        .with_service_range(services)
        .with_volume_pool(volume_pool);
    axum::serve(
        listener,
        api::router(state).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await?;
    Ok(())
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
            let user = apelora_persistence::identity::upsert_user(pool, &issuer, &subject, flag("--username").as_deref(), flag("--email").as_deref(), None).await?;
            let id = apelora_persistence::identity::insert_assignment(
                pool,
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
                pool,
                &apelora_persistence::identity::AuditEntry { actor: "cli", actor_type: "cli", action: "identity.bootstrap", target: &user.id.to_string(), outcome: "success", scope: None, detail: &json!({ "assignment": id, "issuer": issuer }) },
            )
            .await?;
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
            let expires = flag("--expires-days").map(|d| d.parse::<i64>()).transpose()?.map(|d| time::OffsetDateTime::now_utc() + time::Duration::days(d));
            let (secret, rec) = apelora_persistence::create_token(pool, &name, &role, expires).await?;
            apelora_persistence::audit(pool, "cli", "token.create", &rec.name, &json!({ "id": rec.id, "role": rec.role })).await?;
            println!("Created token {} ({}, role {}). It is shown only once:\n{secret}", rec.id, rec.name, rec.role);
        }
        Some("list") => {
            for t in apelora_persistence::list_tokens(pool).await? {
                let state = if t.revoked_at.is_some() { "revoked" } else { "active" };
                println!("{}  {:<20} {:<16} {state}", t.id, t.name, t.role);
            }
        }
        Some("revoke") => {
            let id: uuid::Uuid = args.get(1).ok_or("usage: token revoke <id>")?.parse()?;
            apelora_persistence::revoke_token(pool, id).await?;
            apelora_persistence::audit(pool, "cli", "token.revoke", &id.to_string(), &json!({})).await?;
            println!("Revoked token {id}");
        }
        _ => return Err("usage: token create --name <n> --role <r> [--expires-days <d>] | token list | token revoke <id>".into()),
    }
    Ok(())
}
