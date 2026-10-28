// SPDX-License-Identifier: AGPL-3.0-only
//! `/api/v1` routes. Every route except `/health` needs an authenticated principal and names
//! the permission it requires. Inventory responses carry their source (mock or Incus) and the
//! time they were observed, so mock data is never mistaken for a real cluster.

use std::path::PathBuf;
use std::sync::Arc;

use apelora_domain::preflight::{Check, CheckStatus};
use apelora_domain::{
    API_VERSION, ClusterMember, Instance, Network, Permission, Project, ProviderMode, Role,
    ServerInfo,
};
use apelora_incus_adapter::{AdapterError, Provider};
use apelora_persistence::Pool;
use authz::require;
use axum::extract::{Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Extension, Json, Router};
use serde::Serialize;
use serde_json::Value;
use utoipa::openapi::security::{HttpAuthScheme, HttpBuilder, SecurityScheme};
use utoipa::{Modify, OpenApi, ToSchema};

/// How requests are authenticated.
#[derive(Clone)]
pub enum Auth {
    /// Bearer API tokens checked against the database.
    Tokens(Pool),
    /// Development only (loopback): every request acts as a fabric admin, and `/health` says so.
    DisabledForDevelopment,
}

#[derive(Clone)]
pub struct AppState {
    provider: Arc<Provider>,
    auth: Auth,
    /// Incus projects Apelora may change (default: `apelora`).
    deploy_projects: Arc<Vec<String>>,
    /// Unrestricted identity for cluster administration; `None` when not configured.
    admin: Option<Arc<Provider>>,
    /// SHA-256 fingerprint of the deploy identity's certificate (`APELORA_INCUS_CLIENT_CERT`):
    /// environments' Incus projects are added to its project list (ADR 0012). The mock has its own.
    deploy_fingerprint: Option<Arc<str>>,
    /// Uplink for application OVN networks (`APELORA_NETWORK_UPLINK`); `None`: connections
    /// are not enforced.
    network_uplink: Option<Arc<str>>,
    /// Routed IPv4 or IPv6 range for service addresses (`APELORA_SERVICE_ADDRESSES`); `None`: no
    /// load balancers. Used only together with the uplink.
    service_range: Option<Arc<str>>,
    /// Pool behind the `data-local` volume class (`APELORA_VOLUME_POOL`); `None`: designs with
    /// volumes are refused.
    volume_pool: Option<Arc<str>>,
    /// Shared pool behind the `data-replicated` volume class (`APELORA_SHARED_POOL`); modules
    /// whose volumes use it get their root disk there too (ADR 0013). `None`: refused.
    shared_pool: Option<Arc<str>>,
    /// Pool that receives volume backups (`APELORA_BACKUP_POOL`, also read by the worker);
    /// `None`: backups cannot be asked for and restores take no safety backup.
    backup_pool: Option<Arc<str>>,
    /// OIDC token validation; `None`: people cannot sign in with an identity provider yet.
    oidc: Option<Arc<apelora_identity::Verifier>>,
    /// How a browser signs people in with the identity provider (public client, PKCE);
    /// `None` when OIDC sign-in is not set up.
    sign_in: Option<Arc<OidcSignIn>>,
    /// Identity providers and settings saved in the Console (`auth_config`), reloaded on change.
    auth_runtime: Arc<std::sync::RwLock<Arc<auth_config::AuthRuntime>>>,
    /// Accept legacy platform-wide `apl_…` API tokens (default). Turn off to require OIDC
    /// sign-in or automation service identities, and to put every caller under approvals.
    legacy_tokens: bool,
    /// Limits and policy for terminal and PowerShell sessions.
    console: Arc<sessions::ConsoleSettings>,
    /// Tickets and running interactive sessions (in this process's memory).
    sessions: Arc<sessions::Sessions>,
    /// Password sign-in attempts per client address (in this process's memory).
    sign_in_attempts: Arc<sessions_auth::Attempts>,
    /// When this process started (the Health page's uptime).
    started_at: jiff::Timestamp,
    /// Rate limits on the public surface and on writes (in this process's memory).
    rate_limits: Arc<rate_limit::Limiter>,
    /// Security headers; HSTS when this process serves TLS.
    headers: headers::HeaderPolicy,
    /// The public health summary, refreshed at most every 15 s.
    health: Arc<health_cache::HealthCache>,
    /// Where the database backup and restore-drill scripts leave their records
    /// (`APELORA_DB_BACKUP_DIR`); read-only, shown on the Administration page.
    db_backup_dir: Option<Arc<PathBuf>>,
    /// Image mirrors from the environment (ADR 0010), shown read-only and checked by preflight.
    image_mirrors: Arc<apelora_domain::images::ImageMirrors>,
    /// Probes mirrors, at most once a minute each.
    prober: Arc<mirrors::Prober>,
    /// How metrics are exposed, and their scrape-time cache (`metrics`).
    monitoring: Arc<metrics::Monitoring>,
    /// The settings variables this process started with (`settings::snapshot`), for the
    /// effective-settings view and the support bundle.
    environment: Arc<Vec<(String, String)>>,
    /// The MAAS provisioner (`APELORA_MAAS_URL`); `Off`: only manual machines.
    maas: provisioners::Maas,
    /// How long evacuating or restoring a member waits for the replicas Incus will move to
    /// leave their service addresses (`APELORA_DRAIN_WAIT_SECS`); zero: no drain.
    drain_wait: std::time::Duration,
}

/// Seconds evacuating or restoring a member waits for its replicas to leave their service
/// addresses unless `APELORA_DRAIN_WAIT_SECS` says otherwise: a sync per application takes a few.
pub const DEFAULT_DRAIN_WAIT_SECS: u64 = 30;

/// The project Apelora deploys into unless `APELORA_DEPLOY_PROJECTS` says otherwise.
pub const DEFAULT_DEPLOY_PROJECT: &str = "apelora";

impl AppState {
    pub fn new(provider: Provider, auth: Auth) -> Self {
        Self {
            provider: Arc::new(provider),
            auth,
            deploy_projects: Arc::new(vec![DEFAULT_DEPLOY_PROJECT.to_owned()]),
            admin: None,
            deploy_fingerprint: None,
            network_uplink: None,
            service_range: None,
            volume_pool: None,
            shared_pool: None,
            backup_pool: None,
            oidc: None,
            sign_in: None,
            auth_runtime: Arc::default(),
            legacy_tokens: true,
            console: Arc::new(sessions::ConsoleSettings::default()),
            sessions: Arc::new(sessions::Sessions::default()),
            sign_in_attempts: Arc::new(sessions_auth::Attempts::default()),
            started_at: jiff::Timestamp::now(),
            rate_limits: Arc::default(),
            headers: headers::HeaderPolicy::default(),
            health: Arc::default(),
            db_backup_dir: None,
            image_mirrors: Arc::default(),
            prober: Arc::default(),
            monitoring: Arc::default(),
            environment: Arc::default(),
            maas: provisioners::Maas::Off,
            drain_wait: std::time::Duration::from_secs(DEFAULT_DRAIN_WAIT_SECS),
        }
    }

    /// The settings variables this process started with (only what `settings::snapshot` keeps).
    pub fn with_environment(mut self, vars: impl IntoIterator<Item = (String, String)>) -> Self {
        self.environment = Arc::new(settings::snapshot(vars));
        self
    }

    pub fn with_rate_limits(mut self, config: rate_limit::RateLimitConfig) -> Self {
        self.rate_limits = Arc::new(rate_limit::Limiter::new(config));
        self
    }

    /// This process terminates TLS itself: responses carry `Strict-Transport-Security`.
    pub fn with_tls(mut self, tls: bool) -> Self {
        self.headers.hsts = tls;
        self
    }

    pub fn with_maas(mut self, maas: provisioners::Maas) -> Self {
        self.maas = maas;
        self
    }

    pub fn with_drain_wait(mut self, wait: std::time::Duration) -> Self {
        self.drain_wait = wait;
        self
    }

    pub fn with_db_backup_dir(mut self, dir: Option<PathBuf>) -> Self {
        self.db_backup_dir = dir.map(Arc::new);
        self
    }

    pub fn with_image_mirrors(mut self, m: apelora_domain::images::ImageMirrors) -> Self {
        self.image_mirrors = Arc::new(m);
        self
    }

    pub fn with_monitoring(mut self, config: metrics::MonitoringConfig) -> Self {
        self.monitoring = Arc::new(metrics::Monitoring::new(config));
        self
    }

    pub fn with_console(mut self, settings: sessions::ConsoleSettings) -> Self {
        self.console = Arc::new(settings);
        self
    }

    pub fn with_oidc(mut self, verifier: Option<Arc<apelora_identity::Verifier>>) -> Self {
        self.oidc = verifier;
        self
    }

    pub fn with_sign_in(mut self, sign_in: Option<OidcSignIn>) -> Self {
        self.sign_in = sign_in.map(Arc::new);
        self
    }

    pub fn with_legacy_tokens(mut self, on: bool) -> Self {
        self.legacy_tokens = on;
        self
    }

    pub fn with_network_uplink(mut self, uplink: Option<String>) -> Self {
        self.network_uplink = uplink.map(Arc::from);
        self
    }

    pub fn with_service_range(mut self, range: Option<String>) -> Self {
        self.service_range = range.map(Arc::from);
        self
    }

    pub fn with_volume_pool(mut self, pool: Option<String>) -> Self {
        self.volume_pool = pool.map(Arc::from);
        self
    }

    pub fn with_shared_pool(mut self, pool: Option<String>) -> Self {
        self.shared_pool = pool.map(Arc::from);
        self
    }

    pub fn with_backup_pool(mut self, pool: Option<String>) -> Self {
        self.backup_pool = pool.map(Arc::from);
        self
    }

    pub fn with_admin(mut self, admin: Option<Provider>) -> Self {
        self.admin = admin.map(Arc::new);
        self
    }

    pub fn with_deploy_fingerprint(mut self, fingerprint: Option<String>) -> Self {
        self.deploy_fingerprint = fingerprint.map(Arc::from);
        self
    }

    pub fn with_deploy_projects(mut self, projects: Vec<String>) -> Self {
        self.deploy_projects = Arc::new(projects);
        self
    }
}

/// The caller of a request.
#[derive(Debug, Clone)]
pub struct Principal {
    pub name: String,
    /// The role of a legacy platform-wide API token. `None` for people and service identities,
    /// whose permissions come only from their role assignments (`caller`).
    pub role: Option<Role>,
    pub caller: Option<Arc<authz::Caller>>,
    /// The Apelora session (password or directory sign-in) the request was made with.
    pub session: Option<sessions_auth::SessionRef>,
    /// The personal token (`apt_…`) the request was made with: the person's own permissions,
    /// but never interactive sessions or new credentials (`account::REFUSED_TO_PERSONAL_TOKENS`).
    pub personal_token: Option<uuid::Uuid>,
}

pub fn router(state: AppState) -> Router {
    let protected = Router::new()
        .route("/api/v1/whoami", get(whoami))
        .route("/api/v1/sign-out", post(sessions_auth::sign_out))
        .route("/api/v1/me/password", put(sessions_auth::change_password))
        .route(
            "/api/v1/me/tokens",
            get(account::my_tokens).post(account::create_my_token),
        )
        .route(
            "/api/v1/me/tokens/{id}/rotate",
            post(account::rotate_my_token),
        )
        .route("/api/v1/me/tokens/{id}", delete(account::revoke_my_token))
        .route("/api/v1/me/sign-ins", get(account::my_sign_ins))
        .route("/api/v1/me/sign-ins/{id}", delete(account::end_my_sign_in))
        .route(
            "/api/v1/identity/personal-tokens",
            get(account::personal_tokens),
        )
        .route(
            "/api/v1/identity/personal-tokens/{id}",
            delete(account::revoke_personal_token),
        )
        .route("/api/v1/identity/sign-ins", get(account::all_sign_ins))
        .route(
            "/api/v1/identity/sign-ins/{id}",
            delete(account::end_any_sign_in),
        )
        .route("/api/v1/settings", get(settings::effective_settings))
        .route("/api/v1/support-bundle", get(support::bundle))
        .route(
            "/api/v1/identity/local-accounts",
            post(sessions_auth::create_local_account),
        )
        .route(
            "/api/v1/identity/local-accounts/{id}/reset",
            post(sessions_auth::reset_local_account),
        )
        .route(
            "/api/v1/identity/sign-in/local-accounts",
            put(sessions_auth::set_local_accounts),
        )
        .route("/api/v1/cluster", get(cluster))
        .route("/api/v1/cluster/members", get(clustering::members))
        .route("/api/v1/cluster/groups", get(clustering::groups))
        .route(
            "/api/v1/cluster/settings",
            get(clustering::settings).patch(clustering::change_settings),
        )
        .route("/api/v1/cluster/recovery", get(recovery::recovery))
        .route("/api/v1/cluster/upgrade", post(clustering::upgrade))
        .route("/api/v1/sectors", get(sectors::list).post(sectors::add))
        .route("/api/v1/sectors/{name}", delete(sectors::remove))
        .route("/api/v1/health/components", get(fabric_health::components))
        .route("/api/v1/health/servers", get(fabric_health::servers))
        .route("/api/v1/capacity", get(fabric_health::capacity))
        .route(
            "/api/v1/instances",
            get(instances).post(deploy::create_instance),
        )
        .route(
            "/api/v1/instances/{project}/{name}/state",
            post(deploy::change_state),
        )
        .route(
            "/api/v1/instances/{project}/{name}",
            get(instance_detail::page).delete(deploy::delete_instance),
        )
        .route(
            "/api/v1/instances/{project}/{name}/move",
            get(mobility::preview).post(mobility::request),
        )
        .route(
            "/api/v1/instances/{project}/{name}/logs",
            get(instance_detail::logs),
        )
        .route(
            "/api/v1/instances/{project}/{name}/logs/{file}",
            get(instance_detail::log_file),
        )
        .route(
            "/api/v1/instances/{project}/{name}/console-log",
            get(instance_detail::console_log),
        )
        .route(
            "/api/v1/applications",
            get(manage::list).post(manage::create),
        )
        .route(
            "/api/v1/applications/{name}",
            get(manage::detail).patch(manage::update),
        )
        .route(
            "/api/v1/applications/{name}/draft",
            get(manage::get_draft).put(manage::put_draft),
        )
        .route(
            "/api/v1/applications/{name}/versions",
            get(manage::versions).post(manage::publish),
        )
        .route(
            "/api/v1/applications/{name}/versions/{revision}",
            get(manage::version),
        )
        .route("/api/v1/applications/{name}/archive", post(manage::archive))
        .route("/api/v1/applications/{name}/restore", post(manage::restore))
        .route(
            "/api/v1/applications/{name}/operations",
            get(manage::operations).post(manage::operate),
        )
        .route(
            "/api/v1/applications/{name}/operations/{id}",
            get(manage::operation),
        )
        .route(
            "/api/v1/applications/{name}/migration",
            get(manage::migration_preview),
        )
        .route("/api/v1/applications/{name}/events", get(manage::events))
        .route("/api/v1/applications/{name}/volumes", get(volumes::list))
        .route(
            "/api/v1/applications/{name}/volumes/{volume}",
            delete(volumes::delete).patch(volumes::grow),
        )
        .route(
            "/api/v1/applications/{name}/volumes/{volume}/snapshots",
            get(volumes::snapshots).post(volumes::take_snapshot),
        )
        .route(
            "/api/v1/applications/{name}/volumes/{volume}/snapshots/{snapshot}",
            delete(volumes::delete_snapshot),
        )
        .route(
            "/api/v1/applications/{name}/volumes/{volume}/snapshots/{snapshot}/restore",
            post(volumes::restore_snapshot),
        )
        .route("/api/v1/applications/{name}/access", get(sessions::access))
        .route(
            "/api/v1/applications/{name}/instances/{instance}/sessions",
            post(sessions::open),
        )
        .route("/api/v1/sessions", get(sessions::list))
        .route("/api/v1/sessions/{id}", delete(sessions::terminate))
        .route(
            "/api/v1/applications/{name}/plan",
            post(applications::preview),
        )
        .route(
            "/api/v1/applications/{name}/deployments",
            post(applications::deploy),
        )
        .route(
            "/api/v1/applications/{name}/status",
            get(applications::status),
        )
        .route("/api/v1/jobs", get(deploy::jobs))
        .route("/api/v1/jobs/{id}", get(deploy::job))
        .route("/api/v1/jobs/{id}/detail", get(jobs::detail))
        .route("/api/v1/jobs/{id}/retry", post(jobs::retry))
        .route("/api/v1/jobs/{id}/cancel", post(jobs::cancel))
        .route("/api/v1/tokens", get(admin::list).post(admin::create))
        .route("/api/v1/tokens/{id}", delete(admin::revoke))
        .route("/api/v1/system", get(admin::system))
        .route(
            "/api/v1/cluster/join-tokens",
            get(cluster_admin::list).post(cluster_admin::create),
        )
        .route(
            "/api/v1/cluster/join-tokens/{member}",
            delete(cluster_admin::revoke),
        )
        .route(
            "/api/v1/cluster/members/{member}/evacuation",
            get(cluster_admin::preview_evacuation),
        )
        .route(
            "/api/v1/cluster/members/{member}/evacuate",
            post(cluster_admin::evacuate),
        )
        .route(
            "/api/v1/cluster/members/{member}/restore",
            post(cluster_admin::restore),
        )
        .route(
            "/api/v1/cluster/members/{member}/fencing",
            post(recovery::confirm),
        )
        .route(
            "/api/v1/cluster/members/{member}/removal",
            get(cluster_admin::preview_removal),
        )
        .route(
            "/api/v1/cluster/members/{member}/remove",
            post(cluster_admin::remove),
        )
        .route(
            "/api/v1/machines",
            get(machines::list).post(machines::register),
        )
        .route("/api/v1/machines/{name}", delete(machines::remove))
        .route(
            "/api/v1/provisioners/maas/machines",
            get(provisioners::machines),
        )
        .route(
            "/api/v1/provisioners/maas/machines/{systemId}/commission",
            post(provisioners::commission),
        )
        .route(
            "/api/v1/provisioners/maas/machines/{systemId}/deploy",
            post(provisioners::deploy),
        )
        .route(
            "/api/v1/provisioners/maas/machines/{systemId}/release",
            post(provisioners::release),
        )
        .route(
            "/api/v1/provisioners/maas/machines/{systemId}/test",
            post(provisioners::test),
        )
        .route("/api/v1/machines/{name}/approve", post(machines::approve))
        .route(
            "/api/v1/machines/{name}/approval",
            delete(machines::withdraw),
        )
        .route(
            "/api/v1/machines/{name}/admission",
            get(machines::preview_admission),
        )
        .route("/api/v1/machines/{name}/admit", post(machines::admit))
        .route("/api/v1/preflight", get(preflight))
        .route("/api/v1/image-mirrors", get(mirrors::image_mirrors))
        .route(
            "/api/v1/database-backups",
            get(db_backups::database_backups),
        )
        .route("/api/v1/projects", get(projects))
        .route("/api/v1/networks", get(networks))
        .route("/api/v1/networks/settings", get(networks::settings))
        .route("/api/v1/networks/applications", get(networks::applications))
        .route("/api/v1/storage/pools", get(storage::pool_usage))
        .route("/api/v1/storage/volumes", get(storage::volumes))
        .route("/api/v1/storage/orphans", get(storage::orphans))
        .route(
            "/api/v1/applications/{name}/volumes/{volume}/backups",
            get(storage::backups).post(storage::back_up),
        )
        .route(
            "/api/v1/applications/{name}/volumes/{volume}/schedules",
            put(storage::set_schedules),
        )
        .route(
            "/api/v1/applications/{name}/volumes/{volume}/backups/{backup}/restoration",
            get(storage::preview_restore),
        )
        .route(
            "/api/v1/applications/{name}/volumes/{volume}/backups/{backup}/restore",
            post(storage::restore),
        )
        .route(
            "/api/v1/applications/{name}/volumes/{volume}/backups/{backup}",
            delete(storage::delete_backup),
        )
        .route("/api/v1/audit", get(audit::list))
        .route("/api/v1/audit/export", get(audit::export))
        .route("/api/v1/monitoring", get(metrics::monitoring))
        .route("/api/v1/alerts", get(alerts::list))
        .route("/api/v1/alerts/history", get(alerts::history))
        .route("/api/v1/alerts/{id}/acknowledge", post(alerts::acknowledge))
        .route("/api/v1/alerts/silences", post(alerts::create_silence))
        .route("/api/v1/alerts/silences/{id}", delete(alerts::end_silence))
        .route("/api/v1/alerts/rules", get(alerts::rules))
        .route(
            "/api/v1/alerts/rules/{rule}",
            axum::routing::put(alerts::update_rule),
        )
        .route(
            "/api/v1/alerts/channels",
            get(alerts::channels).post(alerts::create_channel),
        )
        .route(
            "/api/v1/alerts/channels/{id}",
            axum::routing::put(alerts::update_channel).delete(alerts::delete_channel),
        )
        .route(
            "/api/v1/alerts/channels/{id}/test",
            post(alerts::test_channel),
        )
        .route("/metrics", get(metrics::metrics))
        .route(
            "/api/v1/identity/organizations",
            get(identity::organizations).post(identity::create_organization),
        )
        .route("/api/v1/identity/projects", get(identity::projects))
        .route(
            "/api/v1/identity/organizations/{org}/projects",
            post(identity::create_project),
        )
        .route(
            "/api/v1/identity/projects/{project}/environments",
            get(identity::environments).post(identity::create_environment),
        )
        .route("/api/v1/identity/tenancy", get(tenancy::overview))
        .route(
            "/api/v1/identity/quotas/{scope}/{id}",
            get(tenancy::quota).put(tenancy::set_quota),
        )
        .route(
            "/api/v1/identity/environments/{id}/incus-project",
            post(tenancy::prepare_project),
        )
        .route("/api/v1/identity/users", get(identity::users))
        .route("/api/v1/identity/users/lookup", get(identity::lookup_user))
        .route(
            "/api/v1/identity/users/{user}/disable",
            post(identity::disable_user),
        )
        .route(
            "/api/v1/identity/users/{user}/enable",
            post(identity::enable_user),
        )
        .route(
            "/api/v1/identity/groups",
            get(identity::groups).post(identity::create_group),
        )
        .route(
            "/api/v1/identity/groups/{group}/members",
            get(identity::group_members),
        )
        .route(
            "/api/v1/identity/groups/{group}/members/{user}",
            axum::routing::put(identity::add_member).delete(identity::remove_member),
        )
        .route(
            "/api/v1/identity/groups/{group}/external",
            axum::routing::put(identity::set_external),
        )
        .route(
            "/api/v1/identity/roles",
            get(identity::roles).post(identity::create_role),
        )
        .route(
            "/api/v1/identity/roles/{role}",
            delete(identity::delete_role),
        )
        .route("/api/v1/identity/permissions", get(identity::permissions))
        .route(
            "/api/v1/identity/assignments",
            get(identity::assignments).post(identity::create_assignment),
        )
        .route(
            "/api/v1/identity/assignments/preview",
            post(identity::preview_assignment),
        )
        .route(
            "/api/v1/identity/assignments/{assignment}",
            delete(identity::revoke_assignment),
        )
        .route(
            "/api/v1/identity/emergency-access",
            post(identity::emergency_access),
        )
        .route(
            "/api/v1/identity/service-identities",
            get(identity::service_identities).post(identity::create_service_identity),
        )
        .route(
            "/api/v1/identity/service-identities/{id}/disable",
            post(identity::disable_service),
        )
        .route(
            "/api/v1/identity/service-identities/{id}/enable",
            post(identity::enable_service),
        )
        .route(
            "/api/v1/identity/policies",
            get(identity::policies).put(identity::save_policy),
        )
        .route("/api/v1/identity/sign-in", get(auth_config::get_settings))
        .route("/api/v1/identity/sign-in/check", post(auth_config::check))
        .route(
            "/api/v1/identity/sign-in/providers",
            post(auth_config::create),
        )
        .route(
            "/api/v1/identity/sign-in/providers/{id}",
            put(auth_config::update).delete(auth_config::delete),
        )
        .route(
            "/api/v1/identity/sign-in/api-tokens",
            put(auth_config::set_api_tokens),
        )
        .route(
            "/api/v1/identity/sign-in/directories",
            get(directories::list).post(directories::create),
        )
        .route(
            "/api/v1/identity/sign-in/directories/check",
            post(directories::check),
        )
        .route(
            "/api/v1/identity/sign-in/directories/{id}",
            put(directories::update).delete(directories::delete),
        )
        .route(
            "/api/v1/identity/policies/{id}",
            delete(identity::delete_policy),
        )
        .route("/api/v1/identity/audit", get(identity::audit_history))
        .route(
            "/api/v1/approvals",
            get(identity::approvals).post(identity::request_approval),
        )
        .route(
            "/api/v1/approvals/{id}/approve",
            post(identity::approve_request),
        )
        .route(
            "/api/v1/approvals/{id}/reject",
            post(identity::reject_request),
        )
        .route(
            "/api/v1/approvals/{id}/cancel",
            post(identity::cancel_request),
        )
        // Innermost first: the gate refuses unlisted routes, the audit layer records refusals,
        // authentication (outermost) decides who is calling.
        .route_layer(middleware::from_fn(authz::route_gate))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            authz::audit_denials,
        ))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit::per_principal,
        ))
        .route_layer(middleware::from_fn_with_state(state.clone(), authenticate));
    Router::new()
        .route("/api/v1/health", get(health))
        .route("/api/v1/sign-in", get(sign_in))
        // Public, limited per client address: how a person gets an Apelora session.
        .route(
            "/api/v1/sign-in/password",
            post(sessions_auth::sign_in_password),
        )
        .route("/api/v1/sign-in/setup", post(sessions_auth::complete_setup))
        .route("/api/v1/openapi.json", get(openapi))
        // Authenticated by its single-use ticket instead of a bearer token (browsers cannot set
        // headers on a websocket); see `sessions`.
        .route(sessions::CONNECT_PATH, get(sessions::connect))
        .merge(protected)
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit::per_address,
        ))
        .layer(middleware::from_fn_with_state(
            state.headers,
            headers::apply,
        ))
        // Outermost: every request (including refusals and unknown routes) gets a request id.
        .layer(middleware::from_fn(requests::trace))
}

/// API errors with a stable machine code and a readable message.
#[derive(Debug)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: String,
}

impl ApiError {
    /// The readable message (for logs).
    pub fn message(&self) -> &str {
        &self.message
    }

    fn new(status: StatusCode, code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        // Logged inside the request span (request id, method, path); never the request body.
        let (code, error) = (self.code, &self.message);
        match self.status {
            StatusCode::INTERNAL_SERVER_ERROR => tracing::error!(code, %error, "request failed"),
            s if s.is_server_error() => tracing::warn!(code, %error, "request failed"),
            StatusCode::FORBIDDEN => tracing::warn!(code, %error, "request denied"),
            StatusCode::CONFLICT if code == "approval_required" => {
                tracing::info!(code, %error, "request waits for an approval")
            }
            StatusCode::CONFLICT if code.starts_with("approval_") => {
                tracing::warn!(code, %error, "approval refused")
            }
            _ => tracing::debug!(code, %error, status = self.status.as_u16(), "request refused"),
        }
        let mut res = (
            self.status,
            Json(ErrorBody {
                error: ErrorDetail {
                    code: self.code.into(),
                    message: self.message.clone(),
                },
                current_version: None,
            }),
        )
            .into_response();
        if self.status == StatusCode::UNAUTHORIZED {
            res.headers_mut()
                .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
        }
        res.extensions_mut().insert(requests::ErrorCode {
            code,
            message: self.message,
        });
        res
    }
}

impl From<AdapterError> for ApiError {
    fn from(e: AdapterError) -> Self {
        let (status, code) = match &e {
            AdapterError::NotConfigured => {
                (StatusCode::SERVICE_UNAVAILABLE, "incus_not_configured")
            }
            AdapterError::InvalidConfig(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, "incus_invalid_config")
            }
            AdapterError::Connectivity(_) => (StatusCode::BAD_GATEWAY, "incus_unreachable"),
            AdapterError::Authentication(_) => (StatusCode::BAD_GATEWAY, "incus_authentication"),
            AdapterError::NotFound(_) => (StatusCode::BAD_GATEWAY, "incus_not_found"),
            AdapterError::Api { .. } => (StatusCode::BAD_GATEWAY, "incus_api_error"),
            AdapterError::Decode(_) => (StatusCode::BAD_GATEWAY, "incus_bad_response"),
            AdapterError::UnsupportedCapability(_) => {
                (StatusCode::BAD_GATEWAY, "incus_unsupported_capability")
            }
        };
        ApiError::new(status, code, e.to_string())
    }
}

impl From<apelora_persistence::StoreError> for ApiError {
    fn from(e: apelora_persistence::StoreError) -> Self {
        use apelora_persistence::StoreError as E;
        match &e {
            E::NotFound(_) => ApiError::new(StatusCode::NOT_FOUND, "not_found", e.to_string()),
            E::Conflict(_) => ApiError::new(StatusCode::CONFLICT, "conflict", e.to_string()),
            // The database's own text stays in the server log; the client gets a next step.
            _ if e.is_unavailable() => {
                apelora_metrics::STORE_ERRORS.inc(&["store_unavailable"]);
                tracing::warn!(error = %e, "store: the database is not reachable");
                ApiError::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "store_unavailable",
                    "the database is not reachable; check DATABASE_URL and that PostgreSQL is running",
                )
            }
            _ => {
                apelora_metrics::STORE_ERRORS.inc(&["store_error"]);
                tracing::error!(error = %e, "store: database error");
                let id = requests::current();
                ApiError::new(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "store_error",
                    format!(
                        "the database refused the request; the server log has the details (request id {})",
                        id.as_deref().unwrap_or("-")
                    ),
                )
            }
        }
    }
}

fn unauthenticated(message: impl Into<String>) -> ApiError {
    ApiError::new(StatusCode::UNAUTHORIZED, "unauthenticated", message)
}

async fn caller_for(
    pool: &Pool,
    kind: authz::CallerKind,
    id: uuid::Uuid,
    groups: std::collections::BTreeSet<uuid::Uuid>,
    mfa: bool,
) -> Result<Arc<authz::Caller>, ApiError> {
    use apelora_auth::{PrincipalRef, Subject};
    let mut refs = vec![match kind {
        authz::CallerKind::User => PrincipalRef::User { id },
        authz::CallerKind::Service => PrincipalRef::Service { id },
    }];
    refs.extend(groups.iter().map(|g| PrincipalRef::Group { id: *g }));
    let engine = apelora_persistence::identity::load_engine(pool, &refs).await?;
    let emergency = engine
        .assignments
        .iter()
        .any(|a| a.kind == apelora_auth::AssignmentKind::Emergency);
    let subject = match kind {
        authz::CallerKind::User => Subject {
            user: Some(id),
            groups,
            mfa,
            ..Subject::default()
        },
        authz::CallerKind::Service => Subject {
            service: Some(id),
            ..Subject::default()
        },
    };
    Ok(Arc::new(authz::Caller {
        kind,
        id,
        subject,
        engine,
        emergency,
    }))
}

async fn resolve_principal(
    state: &AppState,
    pool: &Pool,
    token: &str,
) -> Result<Principal, ApiError> {
    let fail = |e: ApiError| e;
    if apelora_identity::looks_like_jwt(token) {
        let verifier = state
            .verifier_for(apelora_identity::unverified_issuer(token).as_deref())
            .map_err(unauthenticated)?;
        let identity = verifier
            .verify(token, authz::now())
            .await
            .map_err(|e| ApiError::new(StatusCode::UNAUTHORIZED, "invalid_token", e.to_string()))?;
        let user = apelora_persistence::identity::upsert_user(
            pool,
            &identity.issuer,
            &identity.subject,
            identity.username.as_deref(),
            identity.email.as_deref(),
            identity.name.as_deref(),
        )
        .await
        .map_err(|e| fail(e.into()))?;
        if user.disabled_at.is_some() {
            let mut detail = serde_json::json!({});
            requests::tag(&mut detail);
            authz::audit_failed(
                "auth.refused",
                apelora_persistence::identity::audit_event(
                    pool,
                    &apelora_persistence::identity::AuditEntry {
                        actor: &format!("user:{}", user.id),
                        actor_type: "user",
                        actor_name: user.username.as_deref(),
                        action: "auth.refused",
                        target: "disabled account",
                        outcome: "denied",
                        scope: None,
                        detail: &detail,
                    },
                )
                .await,
            );
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "account_disabled",
                "this account has been disabled",
            ));
        }
        // The token's groups map only onto groups mapped to this issuer (ADR 0016).
        let groups = apelora_persistence::identity::groups_of(
            pool,
            user.id,
            Some(&identity::oidc_source(&identity.issuer)),
            &identity.groups,
        )
        .await
        .map_err(|e| fail(e.into()))?;
        let caller = caller_for(pool, authz::CallerKind::User, user.id, groups, identity.mfa)
            .await
            .map_err(fail)?;
        authz::audit_provider_sign_in(pool, user.id, &identity.issuer, identity.mfa).await;
        let name = user.username.or(user.email).unwrap_or(identity.subject);
        return Ok(Principal {
            name,
            role: None,
            caller: Some(caller),
            session: None,
            personal_token: None,
        });
    }
    if token.starts_with(apelora_persistence::user_sessions::PREFIX) {
        return sessions_auth::principal_for_session(state, pool, token).await;
    }
    if token.starts_with(apelora_persistence::personal_tokens::PREFIX) {
        return account::principal_for_personal_token(state, pool, token).await;
    }
    if token.starts_with("aps_") {
        return match apelora_persistence::identity::authenticate_service(pool, token).await {
            Ok(Some(svc)) => {
                let caller = caller_for(
                    pool,
                    authz::CallerKind::Service,
                    svc.id,
                    Default::default(),
                    false,
                )
                .await
                .map_err(fail)?;
                Ok(Principal {
                    name: svc.name,
                    role: None,
                    caller: Some(caller),
                    session: None,
                    personal_token: None,
                })
            }
            Ok(None) => Err(unauthenticated(
                "the service token is unknown, disabled or expired",
            )),
            Err(e) => Err(fail(e.into())),
        };
    }
    if !state.api_tokens_accepted() {
        return Err(unauthenticated(
            "legacy API tokens are disabled; sign in with your identity provider or use a service identity",
        ));
    }
    match apelora_persistence::authenticate(pool, token).await {
        Ok(Some(rec)) => match Role::parse(&rec.role) {
            Some(role) => Ok(Principal {
                name: rec.name,
                role: Some(role),
                caller: None,
                session: None,
                personal_token: None,
            }),
            None => Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "forbidden",
                "token has an unknown role",
            )),
        },
        Ok(None) => Err(unauthenticated(
            "the API token is unknown, revoked or expired",
        )),
        Err(e) => Err(fail(e.into())),
    }
}

/// The sign-in method of a credential, as a metrics label.
fn auth_method(token: &str) -> &'static str {
    if apelora_identity::looks_like_jwt(token) {
        "oidc"
    } else if token.starts_with(apelora_persistence::personal_tokens::PREFIX) {
        "personal_token"
    } else if token.starts_with("aps_") {
        "service"
    } else if token.starts_with("apl_") {
        "api_token"
    } else {
        "unrecognised"
    }
}

/// What kind of credential was presented, for logs: never any part of the token.
fn credential_kind(token: &str) -> &'static str {
    if apelora_identity::looks_like_jwt(token) {
        "identity-provider token"
    } else if token.starts_with(apelora_persistence::user_sessions::PREFIX) {
        "Apelora session"
    } else if token.starts_with(apelora_persistence::personal_tokens::PREFIX) {
        "personal token"
    } else if token.starts_with("aps_") {
        "service token"
    } else if token.starts_with("apl_") {
        "API token"
    } else {
        "unrecognised"
    }
}

async fn authenticate(State(state): State<AppState>, mut req: Request, next: Next) -> Response {
    let principal = match &state.auth {
        Auth::DisabledForDevelopment => {
            apelora_metrics::AUTH_ATTEMPTS.inc(&["development", "ok"]);
            Principal {
                name: "development".into(),
                role: Some(Role::FabricAdmin),
                caller: None,
                session: None,
                personal_token: None,
            }
        }
        Auth::Tokens(pool) => {
            let presented = req
                .headers()
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.strip_prefix("Bearer "))
                .map(str::trim)
                .filter(|t| !t.is_empty());
            let Some(token) = presented else {
                // Method, path and request id come from the request span.
                tracing::debug!("request without a bearer token");
                apelora_metrics::AUTH_ATTEMPTS.inc(&["none", "refused"]);
                return unauthenticated(
                    "a bearer token is required (Authorization: Bearer <token>)",
                )
                .into_response();
            };
            let result = resolve_principal(&state, pool, token).await;
            apelora_metrics::AUTH_ATTEMPTS.inc(&[
                auth_method(token),
                match &result {
                    Ok(_) => "ok",
                    Err(e) if e.status.is_server_error() => "error",
                    Err(_) => "refused",
                },
            ]);
            match result {
                Ok(p) => p,
                Err(e) => {
                    // Refused credentials are logged (never the token itself), so a sign-in
                    // problem can be traced from the server side. They are not audited: an
                    // audit row per bad request would let anyone fill the log.
                    if !e.status.is_server_error() {
                        let issuer = apelora_identity::looks_like_jwt(token)
                            .then(|| apelora_identity::unverified_issuer(token))
                            .flatten();
                        tracing::warn!(
                            credential = credential_kind(token),
                            issuer = issuer.as_deref().unwrap_or("-"),
                            code = e.code,
                            reason = %e.message,
                            "credential refused"
                        );
                    }
                    return e.into_response();
                }
            }
        }
    };
    if principal.caller.as_ref().is_some_and(|c| c.emergency)
        && let Auth::Tokens(pool) = &state.auth
    {
        // Everything done while holding emergency access is on the record.
        authz::audit_failed(
            "emergency_access.request",
            authz::audit_alone(
                pool,
                &principal,
                "emergency_access.request",
                &format!("{} {}", req.method(), req.uri().path()),
                "success",
                None,
                &serde_json::json!({}),
            )
            .await,
        );
    }
    req.extensions_mut().insert(principal);
    next.run(req).await
}

/// Where inventory data came from and when it was read.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Source {
    pub provider: ProviderMode,
    /// RFC 3339 timestamp (UTC).
    pub observed_at: String,
}

fn source(mode: ProviderMode) -> Source {
    Source {
        provider: mode,
        observed_at: jiff::Timestamp::now().to_string(),
    }
}

/// Error body for every non-2xx response.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ErrorBody {
    pub error: ErrorDetail,
    /// With `draft_conflict` only: the version someone else saved, to start again from.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_version: Option<i32>,
}

#[derive(Serialize, ToSchema)]
pub struct ErrorDetail {
    /// Stable machine-readable code, e.g. `incus_unreachable`, `unauthenticated`, `forbidden`.
    pub code: String,
    pub message: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum AuthMode {
    Tokens,
    DisabledDevelopment,
}

/// What the control plane knows about its Incus connection.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IncusStatus {
    /// True only when talking to a real Incus server (never for mock data).
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub api_version: Option<String>,
    #[schema(required = true)]
    pub server_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub clustered: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(nullable = false)]
    pub error: Option<String>,
}

/// The public health answer: coarse status and Apelora's version (shown at the foot of the
/// sign-in page). Incus versions and error text are on the authenticated `GET /api/v1/system`.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct Health {
    /// `ok`: the control plane answers.
    pub status: String,
    pub service: String,
    /// Apelora's version.
    pub version: String,
    pub api_version: String,
    pub provider: ProviderMode,
    pub auth: AuthMode,
    pub incus: IncusSummary,
    /// When Incus was last asked (RFC 3339, UTC); answers are reused for up to 15 s.
    pub checked_at: String,
}

/// Whether the control plane can reach Incus, and nothing else.
#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IncusSummary {
    /// True only when a real Incus server answered (never for mock data).
    pub connected: bool,
}

/// What the control plane observes of Incus right now, with versions and the error text (for
/// signed-in callers; see `GET /api/v1/system`).
pub(crate) async fn incus_status(provider: &Provider) -> IncusStatus {
    match provider.server_info().await {
        Ok(info) => IncusStatus {
            connected: provider.mode() == ProviderMode::Incus,
            api_version: Some(info.api_version),
            server_version: info.server_version,
            clustered: Some(info.clustered),
            error: None,
        },
        Err(err) => IncusStatus {
            connected: false,
            api_version: None,
            server_version: None,
            clustered: None,
            error: Some(err.to_string()),
        },
    }
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct WhoAmI {
    pub name: String,
    /// `user` (OIDC sign-in), `service` (automation identity), `legacy-token` or `development`.
    pub kind: String,
    /// The user or service identity id; absent for legacy tokens.
    pub id: Option<uuid::Uuid>,
    /// The role of a legacy API token; `null` for people and service identities.
    pub role: Option<Role>,
    /// What the older, coarse permissions amount to for this caller at platform level.
    pub permissions: Vec<Permission>,
    /// Permission ids held at some scope (what the UI may offer; the server decides each request).
    pub capabilities: Vec<String>,
    /// Permission ids held across the whole platform.
    pub platform_permissions: Vec<String>,
    /// The role assignments in force for this caller.
    pub roles: Vec<RoleGrant>,
    /// Incus projects this control plane deploys into (`APELORA_DEPLOY_PROJECTS`).
    pub deploy_projects: Vec<String>,
    /// Signed in with a local account (username and password): may change their password here.
    pub local_account: bool,
    /// The credential this request was made with: `identity-provider` (an OIDC access token),
    /// `session` (an Apelora session after a password or directory sign-in), `personal-token`,
    /// `service-token`, `api-token` (legacy) or `development`.
    pub credential: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct RoleGrant {
    pub role: String,
    #[schema(value_type = Object)]
    pub scope: apelora_auth::Scope,
    pub via_group: bool,
    pub emergency: bool,
    /// RFC 3339.
    pub expires_at: Option<String>,
}

#[derive(Serialize, ToSchema)]
pub struct ClusterResponse {
    pub source: Source,
    pub server: ServerInfo,
    /// Empty when the Incus server is not clustered.
    pub members: Vec<ClusterMember>,
}

macro_rules! list_response {
    ($name:ident, $item:ty) => {
        #[derive(Serialize, ToSchema)]
        pub struct $name {
            pub source: Source,
            pub items: Vec<$item>,
        }
    };
}
list_response!(InstanceList, Instance);
list_response!(ProjectList, Project);
list_response!(NetworkList, Network);

#[derive(Serialize, ToSchema)]
pub struct PreflightReport {
    pub source: Source,
    /// The worst status among the checks.
    pub overall: CheckStatus,
    pub checks: Vec<Check>,
}

/// Read-only readiness checks of the Incus server or cluster, of the projects Apelora deploys
/// into, and of the image mirrors (whether each image server the catalog uses is mirrored and
/// whether the mirrors answer). Changes nothing. Needs `read-inventory`.
#[utoipa::path(get, path = "/api/v1/preflight", tag = "inventory", responses(
    (status = 200, body = PreflightReport), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
async fn preflight(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<PreflightReport>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let pr = &state.provider;
    let server = pr.server_info().await?;
    let (members, projects, pools, networks) = tokio::try_join!(
        pr.cluster_members(),
        pr.projects(),
        pr.storage_pools(),
        pr.networks()
    )?;
    let mut checks = apelora_domain::preflight::evaluate(&apelora_domain::preflight::Observed {
        server: &server,
        members: &members,
        projects: &projects,
        pools: &pools,
        networks: &networks,
        deploy_projects: &state.deploy_projects,
        volume_pool: state.volume_pool.as_deref(),
    });
    checks.extend(mirrors::report(&state).await.checks);
    Ok(Json(PreflightReport {
        source: source(pr.mode()),
        overall: apelora_domain::preflight::overall(&checks),
        checks,
    }))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct AuditItem {
    pub id: i64,
    /// RFC 3339 timestamp.
    pub at: String,
    /// Stable id of who acted, e.g. `user:<uuid>`, `service:<uuid>`, `token:<name>`.
    pub actor: String,
    /// `user`, `service`, `token`, `development`, `cli`, `system`, or `anonymous` (a failed
    /// password sign-in for an unknown username).
    pub actor_type: String,
    /// The name the actor had when they acted (a username, a token's or service identity's
    /// name); absent on events recorded before names were kept.
    pub actor_name: Option<String>,
    pub action: String,
    pub target: String,
    /// `success`, `denied` or `failed`.
    pub outcome: String,
    /// The organisation, project and application the event concerns, when it concerns any.
    #[schema(value_type = Option<Object>)]
    pub scope: Option<Value>,
    #[schema(value_type = Object)]
    pub detail: Value,
}

impl AuditItem {
    pub(crate) fn from_row(e: apelora_persistence::identity::AuditRow) -> Self {
        Self {
            id: e.id,
            at: e
                .at
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default(),
            actor: e.actor,
            actor_type: e.actor_type,
            actor_name: e.actor_name,
            action: e.action,
            target: e.target,
            outcome: e.outcome,
            scope: e.scope,
            detail: e.detail,
        }
    }
}

#[derive(Serialize, ToSchema)]
pub struct AuditList {
    pub items: Vec<AuditItem>,
}

/// Public. Reports that the service answers, the inventory provider, the authentication mode
/// and whether Incus is reachable. Incus is asked at most every 15 s whatever the request rate;
/// versions and error text are only on `GET /api/v1/system` (signed in).
#[utoipa::path(get, path = "/api/v1/health", tag = "system", security(()),
    responses((status = 200, body = Health), (status = 429, body = ErrorBody)))]
async fn health(State(state): State<AppState>) -> Json<Health> {
    let snap = state.health.summary(&state.provider).await;
    Json(Health {
        status: "ok".into(),
        service: "control-plane".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        api_version: API_VERSION.into(),
        provider: state.provider.mode(),
        auth: match state.auth {
            Auth::Tokens(_) => AuthMode::Tokens,
            Auth::DisabledForDevelopment => AuthMode::DisabledDevelopment,
        },
        incus: IncusSummary {
            connected: snap.connected,
        },
        checked_at: snap.checked_at.to_string(),
    })
}

/// What a browser needs to sign a person in with the identity provider: an OpenID Connect
/// authorization-code flow with PKCE for a public client. Nothing here is secret.
#[derive(Debug, Clone, Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct OidcSignIn {
    /// The provider's issuer; its discovery document is at `<issuer>/.well-known/openid-configuration`.
    pub issuer: String,
    /// The public client registered for the Fabric Console and the Studio (no client secret).
    pub client_id: String,
    pub scopes: Vec<String>,
    /// What the button says after "Sign in with", e.g. the organisation's name.
    pub label: String,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignInOptions {
    /// The organisation's identity providers people can sign in with, in order.
    pub providers: Vec<OidcSignIn>,
    /// Whether API tokens are accepted (legacy platform-wide tokens).
    pub api_tokens: bool,
    /// Whether people can sign in with a local account (username and password).
    pub local_accounts: bool,
    /// Directories people can sign in with (username and password), enabled ones only.
    pub directories: Vec<directories::DirectoryOption>,
}

/// Public. How people can sign in to this control plane.
#[utoipa::path(get, path = "/api/v1/sign-in", tag = "system", security(()), operation_id = "sign_in_options",
    responses((status = 200, body = SignInOptions)))]
async fn sign_in(State(state): State<AppState>) -> Json<SignInOptions> {
    Json(SignInOptions {
        providers: state.sign_in_providers(),
        api_tokens: state.api_tokens_accepted(),
        local_accounts: state.local_accounts_enabled(),
        directories: directories::sign_in_options(&state).await,
    })
}

/// Who is calling, with the role assignments and permissions in force.
#[utoipa::path(get, path = "/api/v1/whoami", tag = "system",
    responses((status = 200, body = WhoAmI), (status = 401, body = ErrorBody)))]
async fn whoami(State(state): State<AppState>, Extension(p): Extension<Principal>) -> Json<WhoAmI> {
    let platform = apelora_auth::Resource::platform();
    let credential = p.credential();
    let permissions = [
        Permission::ReadInventory,
        Permission::ReadAudit,
        Permission::ManageTokens,
        Permission::ManageInfrastructure,
        Permission::DesignApplications,
        Permission::Deploy,
    ]
    .into_iter()
    .filter(|x| require(&p, *x).is_ok())
    .collect();
    let ids = || apelora_auth::PERMISSIONS.iter().map(|d| d.id);
    let platform_permissions = ids()
        .filter(|id| authz::allow(&p, id, &platform).is_ok())
        .map(str::to_owned)
        .collect();
    let capabilities = match &p.caller {
        Some(c) => ids()
            .filter(|id| c.engine.holds_anywhere(&c.subject, id, authz::now()))
            .map(str::to_owned)
            .collect(),
        None => ids()
            .filter(|id| authz::allow(&p, id, &platform).is_ok())
            .map(str::to_owned)
            .collect(),
    };
    let local = p
        .session
        .is_some_and(|s| s.method == sessions_auth::SessionMethod::Password);
    let now = authz::now();
    let roles = p
        .caller
        .as_ref()
        .map(|c| {
            c.engine
                .assignments
                .iter()
                .filter(|a| !a.revoked && a.expires_at.is_none_or(|t| t > now))
                .map(|a| RoleGrant {
                    role: c
                        .engine
                        .resolve(&a.role)
                        .map(|r| r.name)
                        .unwrap_or_default(),
                    scope: a.scope.clone(),
                    via_group: matches!(a.principal, apelora_auth::PrincipalRef::Group { .. }),
                    emergency: a.kind == apelora_auth::AssignmentKind::Emergency,
                    expires_at: a
                        .expires_at
                        .and_then(|t| time::OffsetDateTime::from_unix_timestamp(t).ok())
                        .and_then(|t| {
                            t.format(&time::format_description::well_known::Rfc3339)
                                .ok()
                        }),
                })
                .collect()
        })
        .unwrap_or_default();
    Json(WhoAmI {
        kind: match (&p.caller, p.actor_type()) {
            (Some(_), t) => t.to_owned(),
            (None, "development") => "development".to_owned(),
            (None, _) => "legacy-token".to_owned(),
        },
        id: p.caller.as_ref().map(|c| c.id),
        name: p.name,
        role: p.role,
        permissions,
        capabilities,
        platform_permissions,
        roles,
        deploy_projects: state.deploy_projects.to_vec(),
        local_account: local,
        credential: credential.into(),
    })
}

/// The Incus server and its cluster members. Needs `read-inventory`.
#[utoipa::path(get, path = "/api/v1/cluster", tag = "inventory", responses(
    (status = 200, body = ClusterResponse), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
async fn cluster(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<ClusterResponse>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let server = state.provider.server_info().await?;
    let members = state.provider.cluster_members().await?;
    Ok(Json(ClusterResponse {
        source: source(state.provider.mode()),
        server,
        members,
    }))
}

/// Instances in all projects. Needs `read-inventory`.
#[utoipa::path(get, path = "/api/v1/instances", tag = "inventory", responses(
    (status = 200, body = InstanceList), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
async fn instances(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<InstanceList>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let items = state.provider.instances().await?;
    Ok(Json(InstanceList {
        source: source(state.provider.mode()),
        items,
    }))
}

/// Incus projects. Needs `read-inventory`.
#[utoipa::path(get, path = "/api/v1/projects", tag = "inventory", responses(
    (status = 200, body = ProjectList), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
async fn projects(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<ProjectList>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let items = state.provider.projects().await?;
    Ok(Json(ProjectList {
        source: source(state.provider.mode()),
        items,
    }))
}

/// Networks in all projects. Needs `read-inventory`.
#[utoipa::path(get, path = "/api/v1/networks", tag = "inventory", responses(
    (status = 200, body = NetworkList), (status = 401, body = ErrorBody),
    (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
async fn networks(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<NetworkList>, ApiError> {
    require(&p, Permission::ReadInventory)?;
    let items = state.provider.networks().await?;
    Ok(Json(NetworkList {
        source: source(state.provider.mode()),
        items,
    }))
}

struct BearerAuth;

impl Modify for BearerAuth {
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        api.components
            .get_or_insert_with(Default::default)
            .add_security_scheme(
                "bearer",
                SecurityScheme::Http(
                    HttpBuilder::new()
                        .scheme(HttpAuthScheme::Bearer)
                        .description(Some("An OIDC access token from the identity provider, an Apelora session (`apu_…`, after a password sign-in), a personal token (`apt_…`), an automation service token (`aps_…`) or a legacy API token (`apl_…`)."))
                        .build(),
                ),
            );
    }
}

/// Every string property named `…At` is an RFC 3339 timestamp; says so (`format: date-time`) so
/// clients can parse it as one. A test checks the whole document.
struct Timestamps;

impl Timestamps {
    fn mark(schema: &mut utoipa::openapi::RefOr<utoipa::openapi::schema::Schema>) {
        use utoipa::openapi::schema::{
            ArrayItems, KnownFormat, Schema, SchemaFormat, SchemaType, Type,
        };
        let utoipa::openapi::RefOr::T(schema) = schema else {
            return;
        };
        match schema {
            Schema::Object(o) => {
                for (name, p) in o.properties.iter_mut() {
                    if let utoipa::openapi::RefOr::T(Schema::Object(field)) = p {
                        let string = match &field.schema_type {
                            SchemaType::Type(t) => *t == Type::String,
                            SchemaType::Array(ts) => ts.contains(&Type::String),
                            SchemaType::AnyValue => false,
                        };
                        if name.ends_with("At") && string {
                            field.format = Some(SchemaFormat::KnownFormat(KnownFormat::DateTime));
                        }
                    }
                    Self::mark(p);
                }
            }
            Schema::Array(a) => {
                if let ArrayItems::RefOrSchema(items) = &mut a.items {
                    Self::mark(items);
                }
            }
            Schema::OneOf(c) => c.items.iter_mut().for_each(Self::mark),
            Schema::AllOf(c) => c.items.iter_mut().for_each(Self::mark),
            Schema::AnyOf(c) => c.items.iter_mut().for_each(Self::mark),
            _ => {}
        }
    }
}

impl Modify for Timestamps {
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        if let Some(c) = api.components.as_mut() {
            c.schemas.values_mut().for_each(Self::mark);
        }
    }
}

/// Error statuses every operation shares, and what each error status means.
///
/// The middleware can refuse any request with `429 rate_limited` (`rate_limit`), and any request
/// to a route outside [`rate_limit::PUBLIC_PATHS`] with `401`; both are added to every operation
/// that does not list them. Error responses without a description get the meaning of their status.
struct ErrorResponses;

impl ErrorResponses {
    fn meaning(status: &str) -> Option<&'static str> {
        Some(match status {
            "200" => "Done; the response holds the result.",
            "201" => "Created; the response describes what was created.",
            "202" => "Accepted: the work is queued; the response says how to follow it.",
            "204" => "Done; nothing is returned.",
            "400" => "The request is not valid; the error says which part.",
            "401" => "Not signed in: no token, or one that is unknown, expired or revoked.",
            "403" => "The caller may not do this.",
            "404" => "Not found, or not visible to the caller.",
            "409" => "Conflicts with the current state; the error code says how.",
            "410" => "No longer possible: it expired.",
            "422" => "The request is well formed but cannot be carried out; the error says why.",
            "429" => {
                "Too many requests from this address or caller; retry after `Retry-After` seconds."
            }
            "500" => "Apelora failed; the response's `x-request-id` finds it in the server log.",
            "502" => {
                "A service Apelora relies on (Incus, an identity provider, a provisioner) refused or did not answer."
            }
            "503" => {
                "Something this needs is not configured or not reachable yet; the error code says what."
            }
            _ => return None,
        })
    }

    fn error(status: &str) -> utoipa::openapi::RefOr<utoipa::openapi::Response> {
        let mut r = utoipa::openapi::ResponseBuilder::new()
            .description(Self::meaning(status).unwrap_or_default())
            .content(
                "application/json",
                utoipa::openapi::ContentBuilder::new()
                    .schema(Some(utoipa::openapi::Ref::from_schema_name("ErrorBody")))
                    .build(),
            )
            .build();
        if status == "429" {
            r.headers.insert(
                "Retry-After".into(),
                utoipa::openapi::RefOr::T(
                    utoipa::openapi::HeaderBuilder::new()
                        .schema(Some(
                            utoipa::openapi::ObjectBuilder::new()
                                .schema_type(utoipa::openapi::schema::Type::Integer),
                        ))
                        .description(Some("Seconds to wait before trying again."))
                        .build(),
                ),
            );
        }
        r.into()
    }
}

impl Modify for ErrorResponses {
    fn modify(&self, api: &mut utoipa::openapi::OpenApi) {
        for (path, item) in api.paths.paths.iter_mut() {
            for op in [
                &mut item.get,
                &mut item.put,
                &mut item.post,
                &mut item.delete,
                &mut item.patch,
            ]
            .into_iter()
            .flatten()
            {
                let operation = op.operation_id.clone().unwrap_or_default();
                let responses = &mut op.responses.responses;
                // The statuses the handler can answer, from the table of its codes.
                if let Some((_, by)) = error_codes::ERROR_CODES
                    .iter()
                    .find(|(id, _)| *id == operation)
                {
                    for (status, _) in by.iter() {
                        responses
                            .entry((*status).into())
                            .or_insert_with(|| Self::error(status));
                    }
                }
                let mut shared = vec!["429"];
                if !rate_limit::is_public_path(path) {
                    shared.push("401");
                }
                for status in shared {
                    responses
                        .entry(status.into())
                        .or_insert_with(|| Self::error(status));
                }
                for (status, r) in responses.iter_mut() {
                    let utoipa::openapi::RefOr::T(r) = r else {
                        continue;
                    };
                    if let Some(meaning) = Self::meaning(status)
                        && r.description.is_empty()
                    {
                        r.description = meaning.into();
                    }
                    let codes = error_codes::codes(&operation, status);
                    if !codes.is_empty() && !r.description.contains("`error.code`") {
                        let list = codes
                            .iter()
                            .map(|c| format!("`{c}`"))
                            .collect::<Vec<_>>()
                            .join(", ");
                        r.description = format!("{} `error.code`: {list}.", r.description);
                    }
                }
            }
        }
    }
}

/// The `/api/v1` contract. `packages/api-client/openapi.json` is generated from this and the
/// TypeScript client from that file; a test fails when either is stale.
#[derive(OpenApi)]
#[openapi(
    info(title = "Apelora control plane API", description = "Inventory, audit and identity for an Apelora fabric. Inventory responses name their source (mock or Incus) and when they were observed.", license(name = "AGPL-3.0-only", identifier = "AGPL-3.0-only")),
    paths(health, sign_in, sessions_auth::sign_in_password, sessions_auth::complete_setup, sessions_auth::sign_out, sessions_auth::change_password,
        account::my_tokens, account::create_my_token, account::rotate_my_token, account::revoke_my_token,
        account::personal_tokens, account::revoke_personal_token, account::my_sign_ins, account::end_my_sign_in,
        account::all_sign_ins, account::end_any_sign_in, settings::effective_settings, support::bundle, whoami, cluster, instances, projects, networks, preflight, audit::list, audit::export,
        deploy::create_instance, deploy::change_state, deploy::delete_instance, mobility::preview, mobility::request, deploy::job, deploy::jobs,
        jobs::detail, jobs::retry, jobs::cancel,
        instance_detail::page, instance_detail::logs, instance_detail::log_file, instance_detail::console_log,
        admin::list, admin::create, admin::revoke, admin::system, mirrors::image_mirrors, db_backups::database_backups, metrics::metrics, metrics::monitoring,
        alerts::list, alerts::history, alerts::acknowledge, alerts::create_silence, alerts::end_silence,
        alerts::rules, alerts::update_rule, alerts::channels, alerts::create_channel, alerts::update_channel,
        alerts::delete_channel, alerts::test_channel,
        manage::list, manage::create, manage::detail, manage::update, manage::get_draft, manage::put_draft, manage::versions, manage::publish, manage::version, manage::archive, manage::restore, manage::operations, manage::operate, manage::operation, manage::migration_preview, manage::events, volumes::list, volumes::grow, volumes::delete, volumes::snapshots, volumes::take_snapshot, volumes::restore_snapshot, volumes::delete_snapshot, applications::preview, applications::deploy, applications::status,
        cluster_admin::create, cluster_admin::list, cluster_admin::revoke,
        cluster_admin::preview_evacuation, cluster_admin::evacuate, cluster_admin::restore,
        cluster_admin::preview_removal, cluster_admin::remove,
        clustering::members, clustering::groups, clustering::settings, clustering::change_settings, clustering::upgrade,
        sectors::list, sectors::add, sectors::remove,
        recovery::recovery, recovery::confirm,
        machines::list, provisioners::machines, provisioners::commission, provisioners::deploy, provisioners::release, provisioners::test, machines::register, machines::approve, machines::withdraw,
        machines::preview_admission, machines::admit, machines::remove,
        identity::organizations, identity::create_organization, identity::projects, identity::create_project,
        identity::environments, identity::create_environment, identity::users, identity::lookup_user,
        tenancy::overview, tenancy::quota, tenancy::set_quota, tenancy::prepare_project,
        identity::disable_user, identity::enable_user, identity::groups, identity::create_group,
        identity::group_members, identity::add_member, identity::remove_member, identity::set_external, identity::roles,
        identity::create_role, identity::delete_role, identity::permissions, identity::assignments,
        identity::create_assignment, identity::preview_assignment, identity::revoke_assignment,
        identity::emergency_access, identity::service_identities, identity::create_service_identity,
        identity::disable_service, identity::enable_service, identity::policies, identity::save_policy,
        identity::delete_policy, auth_config::get_settings, auth_config::check, auth_config::create, auth_config::update, auth_config::delete, auth_config::set_api_tokens,
        sessions_auth::create_local_account, sessions_auth::reset_local_account, sessions_auth::set_local_accounts,
        directories::list, directories::create, directories::update, directories::delete, directories::check,
        identity::audit_history, identity::approvals, identity::request_approval,
        identity::approve_request, identity::reject_request, identity::cancel_request,
        sessions::access, sessions::open, sessions::list, sessions::terminate,
        fabric_health::components, fabric_health::servers, fabric_health::capacity,
        networks::settings, networks::applications,
        storage::pool_usage, storage::volumes, storage::orphans, storage::backups, storage::back_up,
        storage::delete_backup, storage::set_schedules, storage::preview_restore, storage::restore),
    modifiers(&BearerAuth, &Timestamps, &ErrorResponses),
    security(("bearer" = [])),
    tags((name = "system"), (name = "inventory"), (name = "deployment"), (name = "applications"), (name = "cluster"), (name = "administration"), (name = "audit"), (name = "identity"), (name = "sessions"), (name = "alerts"))
)]
pub struct ApiDoc;

/// The OpenAPI document as pretty JSON with a trailing newline.
pub fn openapi_json() -> String {
    let mut doc = ApiDoc::openapi();
    doc.info.version = API_VERSION.into();
    let mut s = doc
        .to_pretty_json()
        .expect("the OpenAPI document serialises");
    s.push('\n');
    s
}

async fn openapi() -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], openapi_json())
}

mod account;
mod admin;
mod alerts;
#[cfg(test)]
mod alerts_tests;
#[cfg(test)]
mod provisioners_tests;
pub(crate) use admin::token_lifetime_days;
mod applications;
mod audit;
pub mod auth_config;
mod authz;
mod cluster_admin;
mod clustering;
mod db_backups;
mod deploy;
mod directories;
mod document;
mod error_codes;
mod fabric_health;
pub mod headers;
mod health_cache;
mod identity;
mod sectors;
pub use identity::bind_unsourced_mappings;
mod instance_detail;
mod jobs;
mod ldap;
mod machines;
mod manage;
pub mod metrics;
mod mirrors;
mod mobility;
mod networks;
pub mod provisioners;
pub mod rate_limit;
mod recovery;
pub mod requests;
pub mod sessions;
pub mod sessions_auth;
pub mod settings;
mod storage;
mod support;
pub mod tenancy;
mod volumes;

#[cfg(test)]
mod account_tests;
#[cfg(test)]
mod audit_coverage_tests;
#[cfg(test)]
mod clustering_tests;
#[cfg(test)]
mod containment_tests;
#[cfg(test)]
mod db_backups_tests;
#[cfg(test)]
mod directories_tests;
#[cfg(test)]
mod fabric_health_tests;
#[cfg(test)]
mod identity_tests;
#[cfg(test)]
mod jobs_tests;
#[cfg(test)]
mod manage_tests;
#[cfg(test)]
mod metrics_tests;
#[cfg(test)]
mod recovery_tests;
#[cfg(test)]
mod sectors_tests;
#[cfg(test)]
mod sessions_auth_tests;
#[cfg(test)]
mod sessions_tests;
#[cfg(test)]
mod storage_tests;
#[cfg(test)]
mod surface_tests;
#[cfg(test)]
mod tenancy_real_tests;
#[cfg(test)]
mod tenancy_tests;
#[cfg(test)]
mod tests;
