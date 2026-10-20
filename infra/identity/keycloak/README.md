# Keycloak realm for Apelora (setup notes)

Status: **NOT VERIFIED.** Written from the identity provider's documentation; no realm in this repository has been run against the control plane yet. Nothing here is a secret; never commit client secrets, admin passwords or exported realms containing them (`bash scripts/security/scan-secrets.sh` runs in CI).

Apelora only needs an OpenID Connect provider that issues signed JWT access tokens. See [identity and authorization](../../../docs/architecture/IDENTITY-AND-AUTHORIZATION.md) for how they are validated.

## Realm

1. Create a realm (for example `apelora`). Require TLS (`external`), disable self-registration, set a password policy and brute-force protection.
2. **Require MFA** for administrators: a conditional OTP/WebAuthn flow; the access token must then carry `amr` containing `otp` (or an `acr` you configure in `APELORA_OIDC_MFA_ACR`).
3. Short lifetimes: access token 5 minutes; refresh tokens rotated and bound to the session; SSO session idle ≤ 30 minutes for the console.

## Clients

| Client | Type | Notes |
|---|---|---|
| `apelora-console`, `apelora-studio` | Public, authorization code + PKCE (S256) | Exact redirect URIs only (no wildcards). Web origins pinned. |
| `apelora-api` | Audience only | Add an **audience mapper** so access tokens contain `aud: apelora-api`. Set `APELORA_OIDC_AUDIENCE=apelora-api`. |

Set `APELORA_OIDC_AUTHORIZED_PARTIES=apelora-console,apelora-studio` to reject tokens issued to other clients.

## Claims

- **Groups**: a *Group Membership* mapper, "Full group path" off, claim name `groups`. Groups appear in Apelora and can be assigned roles.
- **Username**: `preferred_username` (default). Override with `APELORA_OIDC_USERNAME_CLAIM` if needed.
- Do **not** put roles or permissions in the token; Apelora assigns them itself.

## Control plane environment

```
APELORA_OIDC_ISSUER=https://id.example.test/realms/apelora
APELORA_OIDC_AUDIENCE=apelora-api
APELORA_OIDC_AUTHORIZED_PARTIES=apelora-console,apelora-studio
APELORA_OIDC_MFA_AMR=otp,hwk
```

The key set is read from the issuer over TLS and cached; a plain `http` issuer is accepted only on loopback.

## Before trusting it

Verify with a real realm: a valid token signs in; a token for another audience is refused; an expired token is refused; MFA claims satisfy the access policy; a removed group membership stops granting access at the next token. Record the result in `docs/architecture/current-state.md`.
