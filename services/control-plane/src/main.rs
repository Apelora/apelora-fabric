// SPDX-License-Identifier: AGPL-3.0-only
//! Apelora control plane (Axum REST API, `/api/v1`).
//!
//! `apelora-control-plane`                 serve the API
//! `apelora-control-plane token create --name <n> --role <r> [--expires-days <d>]`
//! `apelora-control-plane token list`
//! `apelora-control-plane token revoke <id>`
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
    let state = api::AppState::new(provider, auth)
        .with_deploy_projects(projects)
        .with_admin(admin)
        .with_network_uplink(uplink);
    axum::serve(listener, api::router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
    Ok(())
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
