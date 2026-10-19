---
name: security-tenancy
description: Use for authentication, TLS, Incus projects, unprivileged containers and protected operations.
---

# Incus Tenancy and Security Boundaries

## When to use

Use for authentication, TLS, Incus projects, unprivileged containers and protected operations.

## Required workflow and guardrails

- Distinguish FabricAdmin, Operator, StudioDesigner, Auditor; authorize every endpoint on server and scope by project/tenant.
- Remote Incus API only on protected management network with verified TLS, short-lived/rotated credentials where feasible, secrets stored outside source.
- Use restricted Incus projects, default unprivileged containers, quotas and namespace isolation; deny nesting, raw LXC, privileged flags, host mount and device access by default.
- Explicit confirmations and audit for deletion, member removal, storage/network changes, and enabling auto healing.
- Browser only talks to Apelora API, never raw Incus API/certificates.
- Validate inputs/IDs/image provenance and avoid arbitrary shell execution pipelines.
- Test horizontal project isolation and privilege escalation cases, including log secret leaks.
- Keep upstream Apache-2.0 Incus license notices; do not relicense upstream as AGPL.

## Evidence required before calling a feature complete

Explain what changed, actual tests and commands run, source/API reference, remaining safety constraints, and whether tested against real Incus or only mocks.

## Primary references

- https://linuxcontainers.org/incus/docs/main/
- https://github.com/lxc/incus
- https://doc.rust-lang.org/cargo/reference/workspaces.html

## Apelora implementation (Phase 03)

- Roles and permissions are defined once in `crates/domain` (`Role::allows`): fabric-admin (all), operator (read, design, deploy), studio-designer (read, design), auditor (read, audit). Every API route calls `require(principal, Permission::…)`; add new permissions there, never ad hoc checks.
- API tokens: `apl_` + 64 hex chars from the OS random source; only SHA-256 hashes are stored (`api_tokens`), with expiry and revocation; shown once by `apelora-control-plane token create --name <n> --role <r> [--expires-days <d>]`; `token list`, `token revoke <id>`; token actions are audited.
- With `DATABASE_URL` every route except `/api/v1/health` requires `Authorization: Bearer <token>` (401 with `WWW-Authenticate: Bearer`; 403 `forbidden` for a missing permission). Without a database the API runs with authentication disabled *only on loopback* and says so in `/health` (`auth: disabled-development`); it refuses to bind elsewhere.
- Fabric Console keeps the token in `sessionStorage` (this tab only), never `localStorage`; it never talks to Incus.
- Next: OIDC sign-in for people (tokens remain for automation), project-scoped permissions.
