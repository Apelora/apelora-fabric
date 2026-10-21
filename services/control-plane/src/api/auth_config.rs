// SPDX-License-Identifier: AGPL-3.0-only
//! Sign-in configured in the Fabric Console: the OpenID Connect identity providers whose tokens
//! this control plane accepts, and whether API tokens are accepted.
//!
//! Providers set by environment variables (`APELORA_OIDC_*`) keep working and are shown
//! read-only; providers saved here are added to them and applied without a restart: every save
//! reloads the verifiers. A token is checked by the provider that issued it (`iss`). Changing
//! these settings needs `platform.security.manage` and is audited. Two guards stop an
//! administrator from locking everyone out: API tokens are not turned off unless an identity
//! provider that answers is enabled, and not by someone signed in with an API token.

use std::sync::Arc;

use apelora_identity::{OidcConfig, Verifier, discover, secure_url};
use apelora_persistence::auth_settings::{self as db, ProviderFields, ProviderRecord};
use axum::extract::rejection::JsonRejection;
use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use serde_json::json;
use utoipa::ToSchema;
use uuid::Uuid;

use super::authz::{allow_platform, audit_as, guard};
use super::deploy::{bad_request, store};
use super::{ApiError, AppState, ErrorBody, OidcSignIn, Principal};

/// The saved key for "API tokens are accepted".
const API_TOKENS: &str = "auth.api_tokens";

/// A provider saved in the Console, with its verifier (or why it has none).
#[derive(Clone)]
pub struct RuntimeProvider {
    pub record: ProviderRecord,
    pub verifier: Option<Arc<Verifier>>,
    pub error: Option<String>,
}

/// What the stored settings say right now; the environment's settings are applied on top.
#[derive(Clone, Default)]
pub struct AuthRuntime {
    pub providers: Vec<RuntimeProvider>,
    /// `None` until someone saves the setting: API tokens are accepted.
    pub api_tokens: Option<bool>,
}

impl AppState {
    fn runtime(&self) -> Arc<AuthRuntime> {
        self.auth_runtime
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The verifier for a token from `issuer`: the environment's provider, or an enabled saved one.
    pub(super) fn verifier_for(&self, issuer: Option<&str>) -> Result<Arc<Verifier>, String> {
        let issuer = issuer.map(|i| i.trim_end_matches('/'));
        if let Some(v) = &self.oidc
            && issuer.is_none_or(|i| i == v.config().issuer.trim_end_matches('/'))
        {
            return Ok(v.clone());
        }
        let rt = self.runtime();
        let enabled: Vec<&RuntimeProvider> =
            rt.providers.iter().filter(|p| p.record.enabled).collect();
        if self.oidc.is_none() && enabled.is_empty() {
            return Err(
                "sign-in with an identity provider is not set up on this control plane".into(),
            );
        }
        let Some(issuer) = issuer else {
            return Err("the token names no issuer".into());
        };
        match enabled.iter().find(|p| p.record.issuer == issuer) {
            Some(p) => p.verifier.clone().ok_or_else(|| {
                format!(
                    "the identity provider {} cannot be reached: {}",
                    p.record.name,
                    p.error.as_deref().unwrap_or("no key set")
                )
            }),
            None => Err(format!(
                "no identity provider set up here issues tokens for {issuer}"
            )),
        }
    }

    /// Whether API tokens are accepted: never when the environment turned them off.
    pub(super) fn api_tokens_accepted(&self) -> bool {
        self.legacy_tokens && self.runtime().api_tokens.unwrap_or(true)
    }

    /// The providers people can sign in with from a browser.
    pub(super) fn sign_in_providers(&self) -> Vec<OidcSignIn> {
        let mut out: Vec<OidcSignIn> = self.sign_in.as_deref().cloned().into_iter().collect();
        for p in self
            .runtime()
            .providers
            .iter()
            .filter(|p| p.record.enabled && p.verifier.is_some())
        {
            if let Some(client_id) = &p.record.client_id {
                out.push(OidcSignIn {
                    issuer: p.record.issuer.clone(),
                    client_id: client_id.clone(),
                    scopes: p.record.scopes.clone(),
                    label: p.record.name.clone(),
                });
            }
        }
        out
    }
}

fn config_of(r: &ProviderRecord) -> OidcConfig {
    let mut cfg = OidcConfig::new(
        r.issuer.clone(),
        r.audiences.first().cloned().unwrap_or_default(),
    );
    cfg.audiences = r.audiences.clone();
    cfg.groups_claim = r.groups_claim.clone();
    cfg.username_claim = r.username_claim.clone();
    cfg.mfa_amr = r.mfa_amr.clone();
    cfg.mfa_acr = r.mfa_acr.clone();
    cfg.authorized_parties = r.authorized_parties.clone();
    cfg
}

/// Reads the saved settings and builds a verifier for every enabled provider (from its
/// discovery document). A provider that cannot be reached is kept, with the reason.
pub async fn reload(state: &AppState) -> Result<(), ApiError> {
    let super::Auth::Tokens(pool) = &state.auth else {
        return Ok(());
    };
    let mut providers = Vec::new();
    for record in db::list_providers(pool).await? {
        let (verifier, error) = if record.enabled {
            match discover(&record.issuer).await {
                Ok((doc, _)) => match Verifier::with_remote_keys(
                    config_of(&record),
                    &doc.jwks_uri,
                    std::time::Duration::from_secs(60),
                ) {
                    Ok(v) => (Some(v), None),
                    Err(e) => (None, Some(e.to_string())),
                },
                Err(e) => (None, Some(e.to_string())),
            }
        } else {
            (None, None)
        };
        match (&verifier, &error) {
            (Some(_), _) => {
                tracing::info!(provider = %record.name, issuer = %record.issuer, "identity provider accepted for sign-in")
            }
            (None, Some(e)) => {
                tracing::warn!(provider = %record.name, issuer = %record.issuer, error = %e, "identity provider not reachable: its tokens are refused until it answers (retried every minute)")
            }
            (None, None) => {
                tracing::debug!(provider = %record.name, "identity provider turned off")
            }
        }
        providers.push(RuntimeProvider {
            record,
            verifier,
            error,
        });
    }
    let api_tokens = db::setting(pool, API_TOKENS)
        .await?
        .and_then(|v| v.as_bool());
    tracing::info!(
        providers = providers.len(),
        api_tokens = ?api_tokens,
        "sign-in settings loaded"
    );
    *state
        .auth_runtime
        .write()
        .unwrap_or_else(|e| e.into_inner()) = Arc::new(AuthRuntime {
        providers,
        api_tokens,
    });
    Ok(())
}

/// Retries providers that did not answer (for example, down when the control plane started):
/// every minute while any enabled one has no keys, so its people can sign in once it is back.
pub async fn retry_unreachable(state: AppState) {
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(60));
    tick.tick().await;
    loop {
        tick.tick().await;
        let waiting = state
            .runtime()
            .providers
            .iter()
            .any(|p| p.record.enabled && p.verifier.is_none());
        if waiting && let Err(e) = reload(&state).await {
            tracing::warn!(error = %e.message(), "sign-in settings could not be reloaded");
        }
    }
}

// ---- API --------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "kebab-case")]
pub enum ProviderSource {
    /// Saved in the Console; editable.
    Console,
    /// Set by `APELORA_OIDC_*` environment variables; change it there.
    Environment,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    /// None for the environment's provider.
    pub id: Option<Uuid>,
    pub source: ProviderSource,
    pub name: String,
    pub issuer: String,
    pub audiences: Vec<String>,
    pub client_id: Option<String>,
    pub scopes: Vec<String>,
    pub groups_claim: String,
    pub username_claim: String,
    pub mfa_amr: Vec<String>,
    pub mfa_acr: Vec<String>,
    pub authorized_parties: Vec<String>,
    pub enabled: bool,
    /// Whether its key set could be read at the last reload; the reason when not.
    pub reachable: bool,
    pub error: Option<String>,
    pub updated_by: Option<String>,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ApiTokenSetting {
    pub enabled: bool,
    /// Turned off by `APELORA_LEGACY_TOKENS=disabled`: it cannot be turned on here.
    pub locked_by_environment: bool,
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SignInSettings {
    pub providers: Vec<ProviderView>,
    pub api_tokens: ApiTokenSetting,
}

/// The editable fields of a provider. Lists default to the provider defaults when left empty.
#[derive(Debug, Clone, Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInput {
    pub name: String,
    pub issuer: String,
    pub audiences: Vec<String>,
    #[serde(default)]
    pub client_id: Option<String>,
    #[serde(default)]
    pub scopes: Vec<String>,
    #[serde(default)]
    pub groups_claim: Option<String>,
    #[serde(default)]
    pub username_claim: Option<String>,
    #[serde(default)]
    pub mfa_amr: Option<Vec<String>>,
    #[serde(default)]
    pub mfa_acr: Vec<String>,
    #[serde(default)]
    pub authorized_parties: Vec<String>,
    #[serde(default = "yes")]
    pub enabled: bool,
}

fn yes() -> bool {
    true
}

const fn is_claim_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '/')
}

/// Checks a provider's fields and fills in the defaults. Pure, so it is tested directly.
pub fn validate(i: &ProviderInput) -> Result<ProviderFields, String> {
    let name = i.name.trim();
    if name.is_empty() || name.chars().count() > 60 || name.chars().any(char::is_control) {
        return Err("the name must be 1 to 60 characters".into());
    }
    let issuer = i.issuer.trim().trim_end_matches('/');
    if !secure_url(issuer) || issuer.contains(char::is_whitespace) {
        return Err("the issuer must be an https:// URL (plain http only to this computer)".into());
    }
    let tidy = |v: &[String]| -> Vec<String> {
        v.iter()
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let audiences = tidy(&i.audiences);
    if audiences.is_empty() {
        return Err(
            "name at least one audience: the identifier this API has at the provider".into(),
        );
    }
    let words = |v: &[String], what: &str| -> Result<Vec<String>, String> {
        let v = tidy(v);
        match v
            .iter()
            .find(|s| s.len() > 200 || !s.chars().all(is_claim_char))
        {
            Some(bad) => Err(format!(
                "{what}: {bad:?} has characters that are not allowed"
            )),
            None => Ok(v),
        }
    };
    let audiences = words(&audiences, "audience")?;
    let scopes = if i.scopes.iter().all(|s| s.trim().is_empty()) {
        vec!["openid".into(), "profile".into(), "email".into()]
    } else {
        words(&i.scopes, "scope")?
    };
    if !scopes.iter().any(|s| s == "openid") {
        return Err("the scopes must include openid".into());
    }
    let claim = |v: &Option<String>, default: &str, what: &str| -> Result<String, String> {
        let v = v
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .unwrap_or(default);
        if v.len() > 100 || !v.chars().all(is_claim_char) {
            Err(format!(
                "the {what} claim has characters that are not allowed"
            ))
        } else {
            Ok(v.to_owned())
        }
    };
    let client_id = i
        .client_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if let Some(c) = &client_id
        && (c.len() > 200 || !c.chars().all(is_claim_char))
    {
        return Err("the client ID has characters that are not allowed".into());
    }
    Ok(ProviderFields {
        name: name.to_owned(),
        issuer: issuer.to_owned(),
        audiences,
        client_id,
        scopes,
        groups_claim: claim(&i.groups_claim, "groups", "groups")?,
        username_claim: claim(&i.username_claim, "preferred_username", "username")?,
        mfa_amr: match &i.mfa_amr {
            None => vec!["mfa".into(), "otp".into(), "hwk".into()],
            Some(v) => words(v, "multi-factor method")?,
        },
        mfa_acr: words(&i.mfa_acr, "assurance level")?,
        authorized_parties: words(&i.authorized_parties, "authorized party")?,
        enabled: i.enabled,
    })
}

fn view(p: &RuntimeProvider) -> ProviderView {
    let r = &p.record;
    ProviderView {
        id: Some(r.id),
        source: ProviderSource::Console,
        name: r.name.clone(),
        issuer: r.issuer.clone(),
        audiences: r.audiences.clone(),
        client_id: r.client_id.clone(),
        scopes: r.scopes.clone(),
        groups_claim: r.groups_claim.clone(),
        username_claim: r.username_claim.clone(),
        mfa_amr: r.mfa_amr.clone(),
        mfa_acr: r.mfa_acr.clone(),
        authorized_parties: r.authorized_parties.clone(),
        enabled: r.enabled,
        reachable: p.verifier.is_some(),
        error: p.error.clone(),
        updated_by: Some(r.updated_by.clone()),
    }
}

fn settings(state: &AppState) -> SignInSettings {
    let mut providers: Vec<ProviderView> = Vec::new();
    if let Some(v) = &state.oidc {
        let c = v.config();
        let sign_in = state.sign_in.as_deref();
        providers.push(ProviderView {
            id: None,
            source: ProviderSource::Environment,
            name: sign_in.map_or_else(|| "Environment".to_owned(), |s| s.label.clone()),
            issuer: c.issuer.clone(),
            audiences: c.audiences.clone(),
            client_id: sign_in.map(|s| s.client_id.clone()),
            scopes: sign_in.map(|s| s.scopes.clone()).unwrap_or_default(),
            groups_claim: c.groups_claim.clone(),
            username_claim: c.username_claim.clone(),
            mfa_amr: c.mfa_amr.clone(),
            mfa_acr: c.mfa_acr.clone(),
            authorized_parties: c.authorized_parties.clone(),
            enabled: true,
            reachable: true,
            error: None,
            updated_by: None,
        });
    }
    providers.extend(state.runtime().providers.iter().map(view));
    SignInSettings {
        providers,
        api_tokens: ApiTokenSetting {
            enabled: state.api_tokens_accepted(),
            locked_by_environment: !state.legacy_tokens,
        },
    }
}

const MANAGE: &str = "platform.security.manage";

/// Sign-in settings: identity providers (saved and from the environment) and whether API
/// tokens are accepted. Needs `platform.security.manage`.
#[utoipa::path(get, path = "/api/v1/identity/sign-in", tag = "identity", operation_id = "sign_in_settings",
    responses((status = 200, body = SignInSettings), (status = 401, body = ErrorBody), (status = 403, body = ErrorBody)))]
pub(super) async fn get_settings(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
) -> Result<Json<SignInSettings>, ApiError> {
    allow_platform(&p, MANAGE)?;
    Ok(Json(settings(&state)))
}

#[derive(Serialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct ProviderCheck {
    pub authorization_endpoint: String,
    pub token_endpoint: String,
    pub jwks_uri: String,
    /// Signing keys the provider publishes now.
    pub keys: usize,
}

#[derive(Deserialize, ToSchema)]
pub struct CheckRequest {
    pub issuer: String,
}

/// Reads a provider's discovery document and key set, without saving anything: what an
/// administrator checks before saving. Needs `platform.security.manage`.
#[utoipa::path(post, path = "/api/v1/identity/sign-in/check", tag = "identity", operation_id = "check_identity_provider",
    request_body = CheckRequest,
    responses((status = 200, body = ProviderCheck), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 502, body = ErrorBody)))]
pub(super) async fn check(
    Extension(p): Extension<Principal>,
    body: Result<Json<CheckRequest>, JsonRejection>,
) -> Result<Json<ProviderCheck>, ApiError> {
    allow_platform(&p, MANAGE)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    let issuer = req.issuer.trim().trim_end_matches('/');
    if !secure_url(issuer) {
        return Err(bad_request(
            "the issuer must be an https:// URL (plain http only to this computer)",
        ));
    }
    let (doc, keys) = discover(issuer).await.map_err(|e| {
        ApiError::new(
            StatusCode::BAD_GATEWAY,
            "provider_unreachable",
            e.to_string(),
        )
    })?;
    Ok(Json(ProviderCheck {
        authorization_endpoint: doc.authorization_endpoint,
        token_endpoint: doc.token_endpoint,
        jwks_uri: doc.jwks_uri,
        keys,
    }))
}

async fn save(
    state: &AppState,
    p: &Principal,
    headers: &HeaderMap,
    id: Option<Uuid>,
    input: ProviderInput,
) -> Result<ProviderView, ApiError> {
    allow_platform(p, MANAGE)?;
    let fields = validate(&input).map_err(bad_request)?;
    if state
        .oidc
        .as_ref()
        .is_some_and(|v| v.config().issuer.trim_end_matches('/') == fields.issuer)
    {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "provider_exists",
            "this issuer is already set up by environment variables",
        ));
    }
    let pool = store(state)?;
    let approval = guard(
        state,
        p,
        headers,
        MANAGE,
        &format!("identity-provider:{}", fields.issuer),
    )
    .await?;
    let id = db::save_provider(pool, id, &fields, &p.actor_ref())
        .await
        .map_err(|e| match e {
            apelora_persistence::StoreError::Database(e)
                if e.to_string().contains("duplicate key") =>
            {
                ApiError::new(
                    StatusCode::CONFLICT,
                    "provider_exists",
                    "another provider has this name or issuer",
                )
            }
            other => other.into(),
        })?;
    reload(state).await?;
    let rt = state.runtime();
    let saved = rt
        .providers
        .iter()
        .find(|x| x.record.id == id)
        // Only a concurrent removal (another administrator, another control plane) gets here.
        .ok_or_else(|| {
            tracing::warn!(provider = %fields.name, issuer = %fields.issuer, "sign-in: a saved provider was gone when the settings were reloaded");
            ApiError::new(
                StatusCode::CONFLICT,
                "conflict",
                "the provider was saved, but another change removed it before it could be shown; reload the sign-in settings and check the list",
            )
        })?;
    audit_as(pool, p, "identity.provider.saved", &fields.issuer,
        &json!({ "id": id, "name": fields.name, "enabled": fields.enabled, "audiences": fields.audiences, "clientId": fields.client_id, "reachable": saved.verifier.is_some(), "approval": approval }))
        .await?;
    Ok(view(saved))
}

/// Adds an identity provider; it is checked and applied at once. Needs
/// `platform.security.manage` (and an approval where the policy asks for one); audited.
#[utoipa::path(post, path = "/api/v1/identity/sign-in/providers", tag = "identity", operation_id = "create_identity_provider",
    request_body = ProviderInput,
    responses((status = 201, body = ProviderView), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn create(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    body: Result<Json<ProviderInput>, JsonRejection>,
) -> Result<(StatusCode, Json<ProviderView>), ApiError> {
    let Json(input) = body.map_err(|e| bad_request(e.body_text()))?;
    Ok((
        StatusCode::CREATED,
        Json(save(&state, &p, &headers, None, input).await?),
    ))
}

/// Changes a provider saved in the Console. Needs `platform.security.manage`; audited.
#[utoipa::path(put, path = "/api/v1/identity/sign-in/providers/{id}", tag = "identity", operation_id = "update_identity_provider",
    params(("id" = Uuid, Path, description = "Provider id")), request_body = ProviderInput,
    responses((status = 200, body = ProviderView), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn update(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    body: Result<Json<ProviderInput>, JsonRejection>,
) -> Result<Json<ProviderView>, ApiError> {
    let Json(input) = body.map_err(|e| bad_request(e.body_text()))?;
    allow_platform(&p, MANAGE)?;
    let existing = db::get_provider(store(&state)?, id).await?.ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "not found: identity provider",
        )
    })?;
    if existing.enabled && !input.enabled {
        lockout_guard(&state, &p, Some(id))?;
    }
    Ok(Json(save(&state, &p, &headers, Some(id), input).await?))
}

#[derive(Deserialize, ToSchema)]
pub struct ConfirmProvider {
    /// The provider's name again.
    pub confirm: String,
}

/// Removes a provider saved in the Console: people stop being able to sign in with it.
/// Its name must be typed again. Needs `platform.security.manage`; audited.
#[utoipa::path(delete, path = "/api/v1/identity/sign-in/providers/{id}", tag = "identity", operation_id = "delete_identity_provider",
    params(("id" = Uuid, Path, description = "Provider id")), request_body = ConfirmProvider,
    responses((status = 204), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 404, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn delete(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    body: Result<Json<ConfirmProvider>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    allow_platform(&p, MANAGE)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    let pool = store(&state)?;
    let existing = db::get_provider(pool, id).await?.ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "not_found",
            "not found: identity provider",
        )
    })?;
    if req.confirm != existing.name {
        return Err(ApiError::new(
            StatusCode::BAD_REQUEST,
            "confirmation_mismatch",
            format!("type the provider's name ({}) to confirm", existing.name),
        ));
    }
    if existing.enabled {
        lockout_guard(&state, &p, Some(id))?;
    }
    let approval = guard(
        &state,
        &p,
        &headers,
        MANAGE,
        &format!("identity-provider:{}", existing.issuer),
    )
    .await?;
    db::delete_provider(pool, id).await?;
    reload(&state).await?;
    audit_as(
        pool,
        &p,
        "identity.provider.deleted",
        &existing.issuer,
        &json!({ "id": id, "name": existing.name, "approval": approval }),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Refuses a change that would leave nobody able to sign in: removing or disabling `leaving`
/// while API tokens are off and no other provider answers.
fn lockout_guard(state: &AppState, p: &Principal, leaving: Option<Uuid>) -> Result<(), ApiError> {
    if state.api_tokens_accepted() {
        return Ok(());
    }
    let others = state.oidc.is_some()
        || state
            .runtime()
            .providers
            .iter()
            .any(|x| x.record.enabled && x.verifier.is_some() && Some(x.record.id) != leaving);
    if others {
        return Ok(());
    }
    let _ = p;
    Err(ApiError::new(
        StatusCode::CONFLICT,
        "would_lock_out",
        "nothing was changed: API tokens are off and no other identity provider answers, so nobody could sign in",
    ))
}

#[derive(Deserialize, ToSchema)]
pub struct ApiTokensRequest {
    pub enabled: bool,
}

/// Turns API tokens on or off. Turning them off is refused while no identity provider that
/// answers is enabled, and for someone signed in with an API token (they would lock themselves
/// out). Needs `platform.security.manage`; audited.
#[utoipa::path(put, path = "/api/v1/identity/sign-in/api-tokens", tag = "identity", operation_id = "set_api_tokens",
    request_body = ApiTokensRequest,
    responses((status = 200, body = ApiTokenSetting), (status = 400, body = ErrorBody), (status = 403, body = ErrorBody), (status = 409, body = ErrorBody)))]
pub(super) async fn set_api_tokens(
    State(state): State<AppState>,
    Extension(p): Extension<Principal>,
    headers: HeaderMap,
    body: Result<Json<ApiTokensRequest>, JsonRejection>,
) -> Result<Json<ApiTokenSetting>, ApiError> {
    allow_platform(&p, MANAGE)?;
    let Json(req) = body.map_err(|e| bad_request(e.body_text()))?;
    if req.enabled && !state.legacy_tokens {
        return Err(ApiError::new(
            StatusCode::CONFLICT,
            "locked_by_environment",
            "API tokens are turned off by APELORA_LEGACY_TOKENS=disabled; change it there",
        ));
    }
    if !req.enabled {
        if p.caller.is_none() {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "would_lock_out",
                "you signed in with an API token: sign in with your identity provider first, then turn API tokens off",
            ));
        }
        let answering = state.oidc.is_some()
            || state
                .runtime()
                .providers
                .iter()
                .any(|x| x.record.enabled && x.verifier.is_some());
        if !answering {
            return Err(ApiError::new(
                StatusCode::CONFLICT,
                "would_lock_out",
                "nothing was changed: no identity provider that answers is enabled, so nobody could sign in",
            ));
        }
    }
    let pool = store(&state)?;
    let approval = guard(&state, &p, &headers, MANAGE, "api-tokens").await?;
    db::set_setting(pool, API_TOKENS, &json!(req.enabled), &p.actor_ref()).await?;
    reload(&state).await?;
    audit_as(
        pool,
        &p,
        if req.enabled {
            "identity.api_tokens.enabled"
        } else {
            "identity.api_tokens.disabled"
        },
        "api-tokens",
        &json!({ "approval": approval }),
    )
    .await?;
    Ok(Json(settings(&state).api_tokens))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input() -> ProviderInput {
        ProviderInput {
            name: "Example Corp".into(),
            issuer: "https://id.example.test/realms/apelora/".into(),
            audiences: vec![" apelora-api ".into(), "".into()],
            client_id: Some("apelora-console".into()),
            scopes: vec![],
            groups_claim: None,
            username_claim: Some("email".into()),
            mfa_amr: None,
            mfa_acr: vec![],
            authorized_parties: vec![],
            enabled: true,
        }
    }

    #[test]
    fn fields_are_tidied_and_defaults_filled_in() {
        let f = validate(&input()).expect("valid");
        assert_eq!(f.issuer, "https://id.example.test/realms/apelora");
        assert_eq!(f.audiences, vec!["apelora-api".to_owned()]);
        assert_eq!(f.scopes, vec!["openid", "profile", "email"]);
        assert_eq!(
            (f.groups_claim.as_str(), f.username_claim.as_str()),
            ("groups", "email")
        );
        assert_eq!(f.mfa_amr, vec!["mfa", "otp", "hwk"]);
    }

    #[test]
    fn unsafe_or_incomplete_settings_are_refused() {
        let bad = |f: fn(&mut ProviderInput)| {
            let mut i = input();
            f(&mut i);
            validate(&i).is_err()
        };
        assert!(
            bad(|i| i.issuer = "http://id.example.test".into()),
            "plain http to another host"
        );
        assert!(bad(|i| i.issuer = "https://id.example.test/a b".into()));
        assert!(bad(|i| i.audiences = vec![]));
        assert!(bad(|i| i.name = "".into()));
        assert!(
            bad(|i| i.scopes = vec!["profile".into()]),
            "openid is required"
        );
        assert!(bad(|i| i.groups_claim = Some("groups; drop".into())));
        assert!(bad(|i| i.client_id = Some("a b".into())));
        assert!(
            !bad(|i| i.issuer = "http://127.0.0.1:5556/dex".into()),
            "a local development provider"
        );
    }
}
