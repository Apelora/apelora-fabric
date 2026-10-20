// SPDX-License-Identifier: AGPL-3.0-only
/**
 * Browser sign-in with the organisation's identity provider: OpenID Connect authorization code
 * with PKCE for a public client (no client secret in the browser). The control plane says which
 * provider and client to use (`GET /api/v1/sign-in`) and verifies the access token on every
 * request. The token is kept only for the browser tab (sessionStorage), like an API token.
 */

export interface OidcSettings {
  issuer: string;
  clientId: string;
  scopes: string[];
  label: string;
}

/** What survives the round trip to the provider, in this tab only. */
interface Pending {
  state: string;
  verifier: string;
  nonce: string;
  tokenEndpoint: string;
  clientId: string;
  redirectUri: string;
  returnTo: string;
}

const PENDING_KEY = "apelora.oidc.pending";

/** A secure URL: https, or plain http to this computer (development identity providers). */
const secure = (u: string) => {
  try {
    const url = new URL(u);
    return url.protocol === "https:" || (url.protocol === "http:" && ["127.0.0.1", "localhost", "[::1]"].includes(url.hostname));
  } catch {
    return false;
  }
};

const base64url = (bytes: Uint8Array) =>
  btoa(String.fromCharCode(...bytes)).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");

/** `bytes` random bytes, base64url: 32 bytes give a 43-character PKCE verifier. */
export function randomToken(bytes = 32): string {
  return base64url(crypto.getRandomValues(new Uint8Array(bytes)));
}

/** The S256 code challenge for a verifier. */
export async function codeChallenge(verifier: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(verifier));
  return base64url(new Uint8Array(digest));
}

/** The provider's authorization and token endpoints, from its discovery document. */
export async function discover(issuer: string, fetchImpl: typeof fetch = fetch): Promise<{ authorization: string; token: string }> {
  if (!secure(issuer)) throw new Error("The identity provider's address is not secure (https).");
  const res = await fetchImpl(`${issuer.replace(/\/$/, "")}/.well-known/openid-configuration`);
  if (!res.ok) throw new Error(`The identity provider did not answer (${res.status}).`);
  const doc = (await res.json()) as { issuer?: string; authorization_endpoint?: string; token_endpoint?: string };
  if (doc.issuer && doc.issuer.replace(/\/$/, "") !== issuer.replace(/\/$/, "")) {
    throw new Error("The identity provider names a different issuer than this control plane expects.");
  }
  if (!doc.authorization_endpoint || !doc.token_endpoint || !secure(doc.authorization_endpoint) || !secure(doc.token_endpoint)) {
    throw new Error("The identity provider's discovery document is incomplete or not secure.");
  }
  return { authorization: doc.authorization_endpoint, token: doc.token_endpoint };
}

/**
 * Starts sign-in: remembers the PKCE verifier, state and nonce for this tab and sends the
 * browser to the provider. `returnTo` is where the app goes back to afterwards (its own hash).
 */
export async function startOidcSignIn(
  s: OidcSettings,
  redirectUri: string,
  returnTo: string,
  deps: { storage?: Storage; fetchImpl?: typeof fetch; navigate?: (url: string) => void } = {},
): Promise<void> {
  const storage = deps.storage ?? sessionStorage;
  const endpoints = await discover(s.issuer, deps.fetchImpl);
  const pending: Pending = {
    state: randomToken(16),
    verifier: randomToken(32),
    nonce: randomToken(16),
    tokenEndpoint: endpoints.token,
    clientId: s.clientId,
    redirectUri,
    returnTo,
  };
  storage.setItem(PENDING_KEY, JSON.stringify(pending));
  const url = new URL(endpoints.authorization);
  url.search = new URLSearchParams({
    response_type: "code",
    client_id: s.clientId,
    redirect_uri: redirectUri,
    scope: s.scopes.join(" "),
    state: pending.state,
    nonce: pending.nonce,
    code_challenge: await codeChallenge(pending.verifier),
    code_challenge_method: "S256",
  }).toString();
  (deps.navigate ?? ((u: string) => window.location.assign(u)))(url.toString());
}

/** The `nonce` claim of an ID token, without verifying it (the control plane verifies the access token). */
function idTokenNonce(idToken: string): string | undefined {
  const payload = idToken.split(".")[1];
  if (!payload) return undefined;
  try {
    const json = atob(payload.replace(/-/g, "+").replace(/_/g, "/"));
    return (JSON.parse(json) as { nonce?: string }).nonce;
  } catch {
    return undefined;
  }
}

/**
 * Finishes sign-in when the provider sent the browser back here with `code` and `state`:
 * checks the state, exchanges the code (with the PKCE verifier) for tokens and checks the ID
 * token's nonce. Returns `null` when this page load is not a return from the provider.
 */
export async function completeOidcSignIn(
  url: URL,
  deps: { storage?: Storage; fetchImpl?: typeof fetch } = {},
): Promise<{ token: string; returnTo: string } | null> {
  const storage = deps.storage ?? sessionStorage;
  const params = url.searchParams;
  const error = params.get("error");
  const code = params.get("code");
  const state = params.get("state");
  if (!error && !(code && state)) return null;
  const raw = storage.getItem(PENDING_KEY);
  storage.removeItem(PENDING_KEY);
  if (error) throw new Error(`The identity provider did not sign you in: ${params.get("error_description") ?? error}.`);
  const pending = raw ? (JSON.parse(raw) as Pending) : null;
  if (!pending || pending.state !== state) throw new Error("This sign-in was not started in this tab. Sign in again.");
  const res = await (deps.fetchImpl ?? fetch)(pending.tokenEndpoint, {
    method: "POST",
    headers: { "Content-Type": "application/x-www-form-urlencoded" },
    body: new URLSearchParams({
      grant_type: "authorization_code",
      code: code!,
      redirect_uri: pending.redirectUri,
      client_id: pending.clientId,
      code_verifier: pending.verifier,
    }).toString(),
  });
  const body = (await res.json().catch(() => ({}))) as { access_token?: string; id_token?: string; error_description?: string; error?: string };
  if (!res.ok || !body.access_token) {
    throw new Error(`The identity provider did not issue a token: ${body.error_description ?? body.error ?? res.status}.`);
  }
  if (body.id_token && idTokenNonce(body.id_token) !== pending.nonce) {
    throw new Error("The identity provider's answer did not match this sign-in. Sign in again.");
  }
  return { token: body.access_token, returnTo: pending.returnTo };
}
