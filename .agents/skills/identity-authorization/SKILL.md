---
name: identity-authorization
description: Add or change anything protected by Apelora's identity and authorization model — routes, permissions, roles, scopes, approvals, service identities, emergency access, audit. Use before adding an API route, a permission, a role, or a UI action that depends on who the caller is.
---

# Identity and authorization

The design is in `docs/architecture/ACCESS-AND-ROLES.md` and `docs/adr/0005-access-model.md`. This skill is the working checklist; if code and this file disagree, the code and tests win — fix the file.

## Where things live

| What | Where |
|---|---|
| Permission catalogue (id, class, floor, approval flag) | `crates/auth/src/permission.rs` |
| Predefined and custom roles | `crates/auth/src/role.rs` |
| Scopes and inheritance (platform → organisation → project → application → pool) | `crates/auth/src/scope.rs` |
| Evaluator (deny by default) | `crates/auth/src/evaluate.rs` |
| Escalation guard for assignments (`check_assignment`) | `crates/auth/src/assign.rs` |
| Four-eyes approvals | `crates/auth/src/approval.rs` |
| OIDC token validation, discovery | `crates/identity` |
| Identity providers and API-token setting (Console-configured, lockout guards) | `services/control-plane/src/api/auth_config.rs`, `crates/persistence/src/auth_settings.rs` |
| Directories (LDAP over TLS): settings API, check, `verify_directory_sign_in`, `upsert_directory_user` | `services/control-plane/src/api/directories.rs`, LDAP client and escaping `services/control-plane/src/api/ldap.rs`, `crates/persistence/src/directories.rs` (migration 0018), Console `apps/fabric-console/src/views/identity/Directories.tsx` |
| Browser sign-in (PKCE) | `packages/api-client/src/oidc.ts`, `apps/fabric-console/src/views/SignIn.tsx` |
| Local accounts, Argon2id, Apelora sessions (`apu_`), `issue_session`, per-address limit | `services/control-plane/src/api/sessions_auth.rs` (+ `sessions_auth_tests.rs`), `crates/persistence/src/local_accounts.rs`, `crates/persistence/src/user_sessions.rs`, migration `0017_sign_in_methods.sql` |
| Set-password page, user menu (Change password, Sign out), local-account dialogs | `apps/fabric-console/src/views/SetPassword.tsx`, `views/Account.tsx`, `views/identity/LocalAccounts.tsx`; Studio `components/applications/ConnectDialog.tsx` |
| Route gate (`GATES`) and per-handler checks | `services/control-plane/src/api/authz.rs` |
| Identity API | `services/control-plane/src/api/identity.rs` |
| Tables and scoped queries | `crates/persistence/migrations/0010_identity.sql`, `crates/persistence/src/identity.rs` |

## Rules

1. **Deny by default.** A route not listed in `GATES` is refused for every caller, legacy tokens included. Adding a route means adding its gate entry *and* an exact check in the handler.
2. **Check the exact resource.** Every handler calls the scoped `allow(...)` for the specific application, project or pool it touches. A route-level check alone is an IDOR waiting to happen. Objects the caller may not see answer **404**, not 403, so existence does not leak.
3. **One permission per action**, named in the catalogue. Never branch on role names in handlers or in the UI. Add the permission, give it a class (`Tenant`, `Infrastructure`, `Security`) and the lowest scope it can be granted at (`floor`). Only `Tenant` permissions can be granted below platform level.
4. **Service identities never hold security or people-management permissions.** Keep that rule in the catalogue, not in callers.
5. **Escalation guard.** Nobody can grant more than they hold, at a wider scope than they hold it. Every assignment path goes through `check_assignment`; the preview endpoint and the create endpoint must use the same checks. Revoking goes through `check_revocation` (you may only take away what you could grant). A credential is the same rule: an API token's role may carry only permissions its creator holds at platform level (`authz::beyond_caller`), and a `fabric-admin` token needs an approval of `token.create_admin`. Any new way to mint a credential or a role must apply the same containment.
6. **Approvals** (`approval: true` in the catalogue): sensitive actions need a recorded approval from a *different* person, for the exact action and target, single use, 30 minutes, sent as `Apelora-Approval`. Self-approval is refused.
7. **Emergency access** is time-limited (≤ 4 h), needs a written reason, cannot be self-granted, and every request made under it is audited.
8. **Audit is append-only** (database trigger). Record actor, action, target, outcome (`success`/`denied`/`failed`) and scope. Never put secrets, tokens, or request bodies with secrets in audit detail.
9. **Legacy `apl_` tokens** stay platform-wide and exempt from approvals and multi-factor rules, but pass the route gate like everyone else (through `legacy_requirement`), cannot create `fabric-admin` tokens, and always expire (1–365 days, API and command line). `APELORA_LEGACY_TOKENS=disabled` or the Console setting turns them off. Do not extend what they can do. Sign-in changes must keep the lockout guards in `auth_config.rs` (never leave nobody able to sign in).
10. **The UI only avoids dead ends.** Hide or disable actions from `whoami.capabilities` / the `actions` object returned per resource, always with the reason; the server enforces regardless.
11. **Directories (ADR 0007):** bind as the person only (no stored service password); `ldaps://` or StartTLS only, certificate and host name always verified; escape usernames with `escape_dn_value` / `escape_filter_value` (RFC 4514 / 4515), never by hand; refuse an empty password before connecting (it is an anonymous bind); 10 s deadline (`503 directory_unreachable`); a refused bind is `401 invalid_credentials` with the same words as local accounts. Directory groups map by `groups.external_name`. `POST /api/v1/sign-in/password` with `directory` signs people in through it (wired). Directories are deliberately not counted as a way in by the lockout guards: a directory can be reachable while nobody in it holds an administrator role.
12. **Troubleshooting without leaks.** Log refused credentials and denied actions with method, path, credential kind, issuer and reason; never a token, certificate, password or the Authorization header.
13. **Passwords and sessions** (ADR 0007). Only `sessions_auth::issue_session` creates `apu_` sessions; only hashes are stored (SHA-256 for tokens, Argon2id 19 MiB/2/1 for passwords, hashed in `spawn_blocking`). Every password path verifies something (a dummy hash for unknown, disabled, locked or unset accounts) and answers the same `invalid_credentials`. Public password routes sit next to `GET /api/v1/sign-in` outside the auth layer, go through the per-address `Attempts` limit, and are listed as public in the route-gate test. Responses carrying a secret send `Cache-Control: no-store`; unreadable bodies get a fixed message (serde's text could echo a password). Anything that changes credentials revokes sessions (reset, password change, disable, turning local accounts off). New ways in must be counted by the lockout guards.
14. **Never leave no super admin.** Any change that can take `platform-super-admin` away from someone (disabling, deleting or demoting a person, revoking an assignment, removing a group member) goes through `identity::change_keeping_a_super_admin` with a new `SuperAdminChange` variant if needed: it runs the change and the count of enabled super admins in one transaction under an advisory lock and refuses (409 `last_super_admin`, audited as denied) when none would be left. A check-then-act count outside that transaction is a race.

## Adding a protected route — checklist

1. Permission exists (or add it with class, floor, approval flag, description).
2. `GATES` entry (method, path pattern, permissions).
3. Handler resolves the resource in the caller's scope first (404 if not visible), then `allow(...)`.
4. Input validated (length, charset, enums); no free-form strings reach Incus, MAAS or a shell.
5. Audit written for state changes and for denials of sensitive actions.
6. Tests: allowed, denied (wrong scope), unauthenticated, other-tenant object (404), escalation attempt, approval required where flagged; for anything that mints credentials or removes access, a containment test (`containment_tests.rs`).
7. `npm run api:generate` after OpenAPI changes; UI uses the generated client.

## Not yet verified against real systems

Browser sign-in with PKCE is REAL-BROWSER TESTED against the local development provider (`infra/dev/dex.yaml`); local accounts (setup link, password sign-in, sign-out) are REAL-BROWSER TESTED against a real control plane and the development PostgreSQL; enterprise providers in production, the MAAS 3.8 entitlement mapping, and Incus restricted-project enforcement are written from documentation and tested with stubs. Say so in reports; do not label them as verified. Directory: directory bind, groups, escaping and TLS checks are REAL DIRECTORY TESTED against the development OpenLDAP (`infra/dev`, profile `directory`; `cargo test -p apelora-control-plane real_directory -- --ignored`), while principal-name directories (`objectGUID`, `userPrincipalName`) are written from documentation; enterprise providers in production, the MAAS 3.8 entitlement mapping, and Incus restricted-project enforcement are written from documentation and tested with stubs. Say so in reports; do not label them as verified.
