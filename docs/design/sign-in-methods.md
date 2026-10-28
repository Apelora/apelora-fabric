# Sign-in methods: local accounts and a directory

Status: agreed · Date: 2026-10-21 · Decision record: [ADR 0007](../adr/0007-sign-in-methods.md)

## 1. Scope

**Goals.** People sign in with a username and password kept by Apelora (local accounts), or with their directory account over LDAP with TLS, next to identity providers and API tokens. Same users, groups, roles, audit and lockout guards as today.

**Non-goals (this round).** SAML; directory search through a service account; one-time codes for local accounts (next slice); self-service sign-up; password recovery by email.

**Users and scale.** Tens to a few hundred people per installation; sign-ins are rare compared with API requests (one per person per working session).

## 2. Contracts

### Users and sessions

| Way in | `users.issuer` | `users.subject` |
|---|---|---|
| Local account | `apelora:local` | the username (lowercase) |
| Directory | the directory's URL, e.g. `ldaps://ldap.example.org` | the entry's stable id (`entryUUID`, or `objectGUID` as hex), the DN if neither exists |

Migration **0017** (`0017_sign_in_methods.sql`):

- `local_credentials (user_id PK → users, password_hash text NULL, setup_token_hash bytea NULL UNIQUE, setup_expires_at, failed_attempts int, locked_until, password_changed_at, created_by, created_at)`.
- `user_sessions (id PK, user_id → users, token_hash bytea UNIQUE, method 'password'|'directory', external_groups text[], created_at, last_used_at, expires_at, revoked_at)`.

Migration **0018** (`0018_directories.sql`): `directories (id, name UNIQUE, url, start_tls bool, ca_pem text NULL, bind_template text, base_dn text NULL, user_filter text NULL, id_attribute, username_attribute, email_attribute, name_attribute, group_attribute NULL, group_base_dn NULL, group_filter NULL, group_name_attribute, enabled, created_by, updated_by, created_at, updated_at)`.

Session tokens are `apu_` + 64 hex characters. **One function issues them** (control plane, `api/sessions_auth.rs`):

```rust
pub async fn issue_session(tx: &mut PgConnection, user_id: Uuid, method: SessionMethod, groups: SessionGroups<'_>) -> Result<IssuedSession, ApiError>
// IssuedSession { token: String, expires_at: OffsetDateTime }
```

`resolve_principal` accepts `apu_` tokens: live (not revoked, `expires_at` in the future, `last_used_at` within the idle hour), person not disabled; groups are the person's Apelora groups plus groups mapped to the session's `group_source` whose external name is among the session's directory groups (`SessionGroups`, ADR 0016); `mfa` is false.

### API

| Method and path | Who | Body → answer |
|---|---|---|
| `POST /api/v1/sign-in/password` | public, limited per address | `{ username, password, directory?: uuid }` → `{ token, expiresAt, name }`; `401 invalid_credentials` "The username or password is incorrect." (also when locked or unknown); `400 local_accounts_disabled` when sign-in with a local password is turned off |
| `POST /api/v1/sign-in/setup` | public | `{ token, password }` → 204; `400 setup_invalid` when spent, expired or unknown; `400 weak_password` with the rule |
| `POST /api/v1/sign-out` | signed in | revokes the presented `apu_` session; 204 for other credentials |
| `PUT /api/v1/me/password` | a local account | `{ current, new }` → 204, other sessions revoked |
| `POST /api/v1/identity/local-accounts` | `user.manage` | `{ username, displayName?, email? }` → `{ user, setupToken, setupExpiresAt }` (token shown once) |
| `POST /api/v1/identity/local-accounts/{id}/reset` | `user.manage` | → `{ setupToken, setupExpiresAt }`; password and sessions revoked |
| `PUT /api/v1/identity/sign-in/local-accounts` | `platform.security.manage` | `{ enabled }` (setting `auth.local_accounts`, default off) |
| `GET/POST/PUT/DELETE /api/v1/identity/sign-in/directories[/{id}]`, `POST …/directories/check` | `platform.security.manage` | directory settings; *check* binds with a test username and password given in the request (never stored) and reports what it found |

`GET /api/v1/sign-in` gains `localAccounts: bool` and `directories: [{ id, name }]` (enabled ones only). `GET /api/v1/identity/sign-in` gains the local-accounts setting and the directories with their reachability.

Audit actions: `auth.password.succeeded`, `auth.password.failed` (username, reason `unknown|wrong|locked|disabled`), `auth.locked`, `auth.setup.completed`, `auth.password.changed`, `identity.local_account.created`, `identity.local_account.reset`, `auth.directory.succeeded|failed`, `identity.directory.saved|deleted`, `identity.local_accounts.enabled|disabled`, `auth.signed_out`.

A password sign-in, with a local account or a directory, and the requests after it:

```mermaid
sequenceDiagram
  autonumber
  actor B as Browser
  participant C as Control plane
  participant D as Directory (LDAP over TLS)
  participant DB as PostgreSQL
  B->>C: POST /api/v1/sign-in/password (username, password, optional directory)
  Note over C: at most 10 attempts a minute per client address
  alt local account
    C->>DB: read the login by username
    C->>C: Argon2id check (a dummy hash for unknown names)
    Note over C,DB: wrong: count the failure, five lock the account for 15 min
  else directory
    C->>D: bind as the person, 10 s in all
    C->>D: read the entry and groups with that bind
    C->>DB: create or refresh the user
  end
  C->>DB: issue_session: store the token's hash, audit the sign-in
  C-->>B: token apu_..., expiresAt, name
  B->>C: later requests with Authorization Bearer apu_...
  C->>DB: session live (idle under 1 h, under 8 h old), groups
```

### Browser

- Sign-in page: when local accounts or a directory are on, a username and password form (with a choice between "Apelora account" and each directory only when there is more than one), next to the provider buttons and the API-token link.
- `#/set-password?token=…` (the token stays in the URL fragment, never sent to a server log): new password twice, the rule, then back to sign-in.
- Users page: *Create local account…* and, for local accounts, *Reset password…*, each showing the setup link once.
- Header: the person's name with *Change password* (local accounts) and *Sign out*.
- Identity & Access → Sign-in: sections for local accounts (on/off) and directories (add, check, edit, turn off, remove), next to identity providers and API tokens.

## 3. Deep dive: the risky parts

**Brute force.** Per account: five failures lock for 15 minutes (`locked_until`), success resets the counter. Per client address: at most 10 attempts a minute (in memory, 429 `rate_limited` with `Retry-After`); behind a proxy listed in `APELORA_TRUSTED_PROXIES` the address is the one it forwarded for, and IPv6 clients count per /64. Unknown usernames verify against a fixed dummy hash so the timing does not reveal accounts.

**Hashing cost.** Argon2id in `spawn_blocking`; about 50 ms per attempt on the lab hardware.

**Directory TLS.** `ldaps://` or `ldap://` with StartTLS required; plain LDAP is refused at save time. The CA certificate (PEM) is optional (system roots otherwise); certificate and host name are always verified. Usernames are escaped for DN and filter syntax (RFC 4514 / RFC 4515) before they reach the directory.

## 4. Wrap-up

| Component | Failure | Behaviour |
|---|---|---|
| Directory unreachable | bind times out (10 s) | 503 `directory_unreachable` with the directory's name; other ways in keep working; logged at warn |
| Database down | session lookup fails | 503 `store_unavailable` (as today) |
| Clock skew | — | sessions are checked against the database clock |

Rollout: local accounts are off by default; turning them on is audited. Directories are added in the Console and take effect at once.

## 5. Evidence plan

Unit and API tests against PostgreSQL; the directory against a real local directory server in Docker (`infra/dev`, profile `directory`), labelled REAL-BROWSER TESTED / CONTRACT VERIFIED as they are.
