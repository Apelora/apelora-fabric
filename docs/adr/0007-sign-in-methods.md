# ADR 0007 — Sign-in methods: identity providers, local accounts and a directory

Status: **accepted; local accounts implemented, directory next** · Date: 2026-10-21 · Related: [ADR 0005](0005-access-model.md) (access model), [identity and authorization](../architecture/IDENTITY-AND-AUTHORIZATION.md), [design note](../design/sign-in-methods.md)

## Context

People sign in to the Fabric Console and the Studio with an OpenID Connect identity provider or an API token. The owner asked for the usual alternatives as well: a username and password kept by Apelora, and sign-in against an existing directory over LDAP with TLS. Directory federation servers that speak OpenID Connect are already covered by identity providers. Small installations and lab clusters often have no identity provider; organisations with a directory but no federation server should not need to deploy one first.

Whatever we add must not weaken what exists: authorization stays the same model (users, groups, scoped roles), every sign-in and every refusal is audited or logged, secrets never reach logs or the browser beyond the session, and nobody can be locked out by a settings change.

## Decision

1. **Four ways in, one identity model.** OpenID Connect providers, local accounts, a directory (LDAP over TLS) and API tokens. Every person is a row in `users`, identified by `(issuer, subject)`: providers use their issuer and `sub`; local accounts use the issuer `apelora:local` and the username; a directory uses its address and the entry's stable identifier. Roles, groups and approvals work the same for all of them.
2. **Apelora sessions for password sign-ins.** A successful local or directory sign-in returns an Apelora session token (`apu_`, 32 random bytes), stored only as a SHA-256 hash, with an idle limit of one hour and an absolute limit of eight hours, revoked by signing out, by disabling the person and by a password reset. Provider sign-ins keep using the provider's own short-lived access token.
3. **Passwords are hashed with Argon2id** (memory 19 MiB, 2 passes, 1 lane — the current recommended minimum), verified off the request thread. Policy by length, not composition: 12 to 128 characters, not the username. A failed sign-in answers the same, in about the same time, whether or not the account exists. Five failures lock an account for 15 minutes; sign-in attempts are also limited per client address. Failures and lockouts are audited with the username, never the password.
4. **Administrators never know a person's password.** Creating a local account (or resetting one) returns a one-time setup link valid for 24 hours, shown once; the person chooses the password. A reset revokes the password and every session.
5. **A directory is reached with the person's own credentials.** Apelora binds as the person (a DN template such as `uid={username},ou=people,…`, or a user-principal form `{username}@example.org`), over `ldaps://` or StartTLS only, verifying the server certificate. It reads the person's entry and groups with that same bind, so Apelora keeps no directory password. Groups map onto Apelora groups by external name, as identity-provider groups already do.
6. **Multi-factor.** Identity providers and directories keep their own multi-factor. Local accounts have none in the first slice, so access policies that require multi-factor refuse them; time-based one-time codes for local accounts are the next slice.
7. **Not built:** SAML. Organisations that only federate with SAML put an OpenID Connect broker in front; Apelora does not parse SAML.

## Consequences

- A lab or small installation can run with local accounts only, and turn API tokens off.
- The lockout guards count every way in: API tokens cannot be turned off, and the last answering provider cannot be removed, while nothing else (an answering provider, or enabled local accounts with at least one set-up account) would let people sign in.
- Password hashing costs about 50 ms of CPU per attempt; the per-address limit keeps that from becoming a denial of service.
- Local accounts are another credential store to protect; the database already holds API-token and session hashes and is treated as sensitive.

## Alternatives considered

- **Only identity providers, and run a broker for everything else.** Least code in Apelora, but every small installation would have to deploy and operate a second system before its first sign-in.
- **Directory sign-in through a service account** (search, then bind as the person). Works where people cannot be found by a DN template, but Apelora would have to hold the service account's password; it can be added later with a secret reference if a directory needs it.
- **Signed session tokens (no database lookup).** Cheaper per request, but they cannot be revoked before they expire, and disabling a person must take effect at once.
