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
- API tokens: `apl_` + 64 hex chars from the OS random source; only SHA-256 hashes are stored (`api_tokens`), with expiry and revocation; shown once by `apelora-control-plane token create --name <n> --role <r> [--expires-in-days <1-365>]` (90 days by default, never without an expiry); `token list`, `token revoke <id>`; or through the API (`manage-tokens` plus every permission the token's role carries; a `fabric-admin` token needs an approval of `token.create_admin`; `POST /api/v1/tokens` always sets an expiry of 1–365 days, the token is returned once with `Cache-Control: no-store`); token actions are audited (`token.create`, `token.revoke`), never with the token itself.
- With `DATABASE_URL` every route except `/api/v1/health` requires `Authorization: Bearer <token>` (401 with `WWW-Authenticate: Bearer`; 403 `forbidden` for a missing permission). Without a database the API runs with authentication disabled *only on loopback* and says so in `/health` (`auth: disabled-development`); it refuses to bind elsewhere.
- Fabric Console keeps the token in `sessionStorage` (this tab only), never `localStorage`; it never talks to Incus.
- Next: OIDC sign-in for people (tokens remain for automation), project-scoped permissions.

## Tenancy on Incus projects and quotas (ADR 0012, built 2026-10-21)

Design `docs/design/tenancy-isolation.md`; code `crates/domain/src/tenancy.rs` (pure: baseline, limits, quota arithmetic, findings), `crates/incus-adapter/src/tenancy.rs` (project, state, profile and trust calls; mock), `services/control-plane/src/api/tenancy.rs` (prepare, mirror, `check_deployment`, handlers), `crates/persistence/src/tenancy.rs` (migration 0027); runbook `docs/runbooks/tenancy-quotas.md`.

- **One environment, one restricted Incus project**, named `<org>-<project>-<env>`. Only the **admin identity, in the control plane**, creates projects, changes their configuration or profile, or changes a certificate's project list: Incus refuses all of that to a restricted certificate (REAL INCUS TESTED: `POST /1.0/projects` → 403 "Certificate is restricted"). The worker never gets the admin identity; nothing carrying it is queued.
- **Baseline** (`baseline_config`): `restricted=true`; containers unprivileged, no nesting, no low-level or interception; VMs no low-level; disks and NICs managed; GPU, PCI, USB, unix-*, InfiniBand and proxy devices blocked; `cluster.target` and `snapshots` allowed (Apelora moves instances and snapshots volumes); backups and network zones blocked; `features.images=false`, own profiles and volumes; with an uplink `features.networks=true`, `restricted.networks.uplinks`, `restricted.networks.subnets=<uplink>:<service range>`; `user.apelora.managed`/`environment` markers. Change it only with a lab run of `real_tenancy` and the design note updated.
- **Adoption**: an existing project is used only when `restricted=true`; its restrictions are not rewritten, differences are findings. Never adopt an unrestricted project.
- **Grant**: append to the deploy certificate's `projects` (PATCH merges) under the PostgreSQL advisory lock `lock_trust`, read back, then read the project with the deploy identity. Never make the deploy identity unrestricted.
- **Quotas**: Apelora's (`tenancy_quotas`), checked at plan time (`409 quota_exceeded`, `503 quota_unverifiable` fail-closed); each environment's project gets the tightest applicable quota as `limits.*`; Apelora removes only limit keys it wrote (`environments.incus_limits`). Instances need `limits.cpu`/`limits.memory` and a sized root disk once a project has those limits: the project's default profile provides `limits.cpu=1`, `limits.memory=1GiB`, root `size=10GiB`.
- Permissions: `quota.manage` (platform) or tenants dividing within the quota above (`organization.update`, `project.update`); `environment.manage` to prepare.
- Evidence: `cargo test -p apelora-control-plane real_tenancy -- --ignored --nocapture` prints a dry run of every Incus request; `APELORA_TEST_TENANCY_APPLY=1` runs it on the lab with one `apelora-test-tenancy-*` project and deletes it.
