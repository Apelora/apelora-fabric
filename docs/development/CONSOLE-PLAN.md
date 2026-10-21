# Fabric Console: what is missing, and the build plan

Status: **plan** · Date: 2026-10-21 · Source: a full review of the Console, the API and the worker against what a complete operations console needs · Direction: [cloud roadmap](../architecture/CLOUD-ROADMAP.md), [one design at every size](../architecture/EVERY-SIZE.md) (S4 grows into the outbox, ADR 0011)

Statuses:
- **B** — built: API, UI and tests;
- **A** — API only;
- **U** — UI only, or only in the Studio;
- **IP** — in progress;
- **M** — missing.

Priorities:
- **P0** — needed for safe operation;
- **P1** — expected;
- **P2** — nice to have.

## Area by area

| Area | Built | Missing (priority) |
|---|---|---|
| Overview | service status, members | "needs attention": failed jobs, waiting approvals, expiring certificates, open alerts (P1); recent changes (P2); capacity (IP) |
| Health | — | per-component health, worker heartbeats, queue, member resources (IP); history and objectives (P2) |
| Servers and clustering | member list; evacuate and restore with a dry run; remove; enrolment, admission and join tokens (REAL INCUS TESTED) | database roles, leader and quorum (P0); rolling upgrade with a version-skew check (P0); member detail (P1); cluster groups and failure domains (P1); cluster settings: offline and healing thresholds (P1); certificate rotation (P1) |
| Instances | list, deploy, start/stop, delete, move with a dry run | instance and console logs (P0); restart in the Console (A → P1); detail page (P1); resize (P1); snapshots of a whole instance (P2); terminal (U) |
| Applications | list, status, port checks, service addresses, volumes | start/restart/stop, versions, drafts and events in the Console (U, P1); archive and restore (A) |
| Operations | job list | retry and cancel (P0); job detail and step log (A, P1) |
| Alerts and events | per-application events (Studio) | rules, notifications, acknowledgement, history (P0); Incus warnings and events (P1) |
| Networks | Incus network list | per-application networks, rules and address sets (P1); uplink and address-range settings in the Console (P1); peers (P2) |
| Storage | pools; volumes: grow, delete, snapshots, restore | backups: list and restore from a backup (P0); schedules (P1); pool usage and quotas (IP, P1); orphaned volumes (P1); volume classes (P2) |
| Images and catalog | catalog in the Studio | Incus images: list, pin, prune (P2) |
| Tenancy | organisations, projects, environments | create the environment's restricted Incus project with limits (P1); quotas (P1); delete and rename (P2) |
| Identity and access | users, groups, roles, custom roles, assignments with preview, service identities, policies, approvals, emergency access, sign-in providers | local accounts, sign-out, change password (IP); directory sign-in (IP); personal tokens bound to a person (P1); token rotation (P1); a person's active sign-ins (P1); access-review export (P2) |
| Audit | search by action and outcome, paged | filters by actor, target and time (P0); export (P1); retention and archive (P1) |
| Debugging | request ids; preflight | log lookup by request id (P1); support bundle (P1) |
| Security | certificate expiry | TLS on the API (P0); security headers (P0); rate limits on public routes (P0); a security-posture page (P1) |
| Administration | API tokens; versions; backup of Apelora's own database with a restore drill: scripts and runbook, no Console view yet ([runbook](../runbooks/apelora-database-backup.md)) | upgrade (P0); age of the last verified restore in the Console (P0, the drill writes `restore-drill-last-verified.json`); effective settings (P1); licence notices (P1) |
| Terminal | Studio sessions (MOCK and CONTRACT VERIFIED) | open from the Console and a sessions page (P1); run against real Incus (P0); recording, opt-in (P2) |
| Disconnected (ADR 0010) | image mirrors, mirrors-only mode | preflight for mirrors; mirrors page; release bundle (P1) |

## Security and audit gaps in the code

1. **API tokens can grant more than their creator holds (P0).**
   - Token creation checks `manage-tokens` but never the role of the token it creates (`services/control-plane/src/api/admin.rs`, `create`).
   - Legacy tokens skip the route gate, approvals and multi-factor.
   - The command line can create tokens that never expire.
   - Fix: refuse a role whose permissions the caller does not hold; require an approval for `fabric-admin` tokens; make expiry mandatory.
2. **Revoking a role** checks only `role.assign` at the scope, not that the revoker holds the role (P1).
3. **Disabling a person** has no guard against disabling the last super administrator (P1).
4. **Audit integrity.**
   - Audit rows are written after the change, outside its transaction (P0).
   - The append-only trigger does not cover `TRUNCATE`.
   - There is no test that every state-changing route audits (P0).
   - Successful sign-ins are not audited.
   - Security settings taken from the environment are only logged.
5. **Public surface.**
   - No security headers or content security policy.
   - The API serves plain HTTP and can listen on a non-local address.
   - Public `/health` calls Incus on every request and returns Incus error text and its version.
   - No rate limits except on terminal tickets.

   All P0.
6. **Tokens.** Secrets are shown once and stored as hashes (good). There is no rotation, and a service token's lifetime is clamped silently instead of refused (P1).
7. **Sessions.** Disabling takes effect on the next request (good). Terminal tickets live in one process. Identity-provider tokens cannot be revoked before they expire (at most an hour).
8. **Clean:** no secrets in logs; no CSRF exposure (bearer tokens, no cookies); the terminal websocket checks `Origin`.

## Waves

Packages in one wave touch different files and can be built in parallel.

**Wave 0 — security fixes (backend):**
- S1 token containment.
- S2 guards on revoke and disable.
- S3 public surface: headers, `/health` redaction and cache, per-address limits, TLS or a declared proxy for non-local addresses, a serving configuration for the web apps with a content security policy.
- S4 audit integrity: audit in the same transaction; `TRUNCATE` blocked; a structural test that every state-changing route audits or is listed as exempt.

**Wave 1 — operations core:**
- Navigation regrouped: Overview, Health, Servers, Workloads, Networks, Storage, Alerts, Identity & Access, Audit, Administration.
- Audit filters and export.
- Job detail, retry and cancel.
- Instance detail with logs and restart.

**Wave 2 — clustering and infrastructure:**
- Servers area: members with database roles and quorum, groups, settings with approvals, upgrades, certificates.
- Upgrade assistant.
- Networks page (read-only first).
- Storage page with backups, restore from backup, schedules and orphans.

**Wave 3 — alerts, terminal, identity:**
- Alerts: an evaluator in the worker, acknowledgement, a webhook channel holding only a secret reference.
- Terminal from the Console, reusing the Studio's brokered sessions, with a sessions page.
- Personal tokens and rotation.
- Audited sign-ins.

**Wave 4 — administration and tenancy:**
- Effective settings, licence notices, a database backup check, a support bundle.
- Restricted Incus projects per environment with limits.
- Then sectors (ADR 0008) and HA slice B.

Every UI package:
- keeps one primary action per view;
- passes the Better Design review (`get-review-rules`, `check-comprehension`, `inspect-spacing` at 1440, 1728 and 390);
- passes axe and phone layout checks;
- labels its evidence;
- shows only data the API returns.

## Two answers

**Clustering in the Console.** Build a **Servers** area with these tabs:
- **Members:** status, database role, leader and quorum, failure domain, group, Incus version, capacity; evacuate, restore and remove.
- **Machines and join tokens.**
- **Cluster settings**, changed with an approval.
- **Upgrades:** version skew and a rolling evacuate → upgrade → restore flow.
- **Certificates.**

Sectors later group all of this per cluster.

**A terminal from the Console.** Yes, as a second door into the existing brokered sessions, with no new backend path. The rules stay as they are:
- `instance.console` permission, never service identities or API tokens;
- approval by another person in production;
- single-use 60-second tickets bound to the person, the instance and the origin;
- no root by default;
- 15-minute idle and 60-minute absolute limits;
- access re-checked every 30 seconds;
- every step audited, without content.

Add a sessions page to list and end sessions. Recording is off by default and, where required, opt-in per environment, with a visible banner. There is no shell on the servers themselves.
