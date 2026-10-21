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
| Overview | service status, members; alerts that need attention (B) | "needs attention": failed jobs, waiting approvals, expiring certificates (P1); recent changes (P2); capacity (IP) |
| Health | — | per-component health, worker heartbeats, queue, member resources (IP); history and objectives (P2) |
| Servers and clustering | Servers area (B, 2026-10-21): members with database role, leader and quorum, failure domain, groups, Incus release and capacity; evacuate, restore and remove with a dry run; enrolment, admission and join tokens; cluster groups; cluster settings read-only with defaults; certificate expiry; version skew against the pin with upgrade blockers and the written rolling procedure (read-only API REAL INCUS TESTED; Console MOCK VERIFIED) | changing cluster settings with an approval (P1); an upgrade assistant that walks the rolling procedure member by member (P0); member detail (P1); certificate rotation (P1) |
| Instances | list, deploy, start/stop, delete, move with a dry run; terminal (MOCK VERIFIED); detail page `#/instances/{project}/{name}` with restart, resources, addresses, disks, snapshots, recent jobs (B); instance and console logs, last 1–1024 KiB (B; MOCK and CONTRACT VERIFIED, REAL INCUS TESTED 2026-10-21 on a lab container: detail, log files, a bounded tail and the console log; VM logs not read) | resize (P1); snapshots of a whole instance (P2); terminal (U) |
| Applications | list, status, port checks, service addresses, volumes | start/restart/stop, versions, drafts and events in the Console (U, P1); archive and restore (A) |
| Operations | job list; retry (a new linked job) and cancel at safe points (B); job detail `#/operations/{id}` with attempts, timings, request id, Incus operations, who queued it and linked jobs (B); filters by state, kind and application kept in the address, and paging with older operations (B, 2026-10-21; MOCK VERIFIED) | |
| Alerts and events | per-application events (Studio); alerts (B, built 2026-10-21, MOCK VERIFIED; [design](../design/alerts.md), [runbook](../runbooks/alerts.md)): fifteen built-in rules evaluated by the worker under the `alert-evaluator` lease with hysteresis, thresholds, durations and severities editable, acknowledge and silence (audited), history for 90 days, webhook channels with a secret reference and HMAC-signed bodies through the outbox, test send and delivery state; `#/alerts` and an Overview row | Incus warnings and events (P1); a firing count in the navigation (with the navigation regrouping); notifications on acknowledgement and per-application channel routing (P2) |
| Networks | Incus network list; `#/networks` (B, read-only, 2026-10-21): per application its OVN network, the ACL rules compiled from the stored deployment (`SyncRequest::intended_acls`, the same function `network.sync` writes with) next to Incus's rules with drift per rule, address sets, service and published addresses with their backends; settings in effect: uplink, service range and use, each Apelora project's network limits, the uplink's routes and ranges (administrator identity) with findings (MOCK VERIFIED; shapes checked read-only on the lab) | network forwards and DNS zones (not used by Apelora yet); peers (P2) |
| Storage | pools; volumes: grow, delete, snapshots, restore; `#/storage` (B, 2026-10-21): pool use per server (a shared pool once), every application volume with its schedules and backups, backups per volume, back up now, restore from a backup (a `volume.restore` job: dry run, volume name typed, safety backup first, refused while an instance using it runs), schedules editing, volumes no application uses (read-only, with size and why) (MOCK VERIFIED; restore REAL INCUS TESTED as a job (2026-10-21)) | deleting a backup (P1); quotas (P1); volume classes (P2); restoring onto another member when the volume's member is lost (P1) |
| Images and catalog | catalog in the Studio | Incus images: list, pin, prune (P2) |
| Tenancy | organisations, projects, environments; each environment's restricted Incus project created or adopted by Apelora (admin identity), its default profile and the deploy identity's grant; quotas on organisations, projects and environments, checked at plan time and mirrored as Incus project limits; Identity & Access → Tenancy & quotas with each project's restrictions and limit use; quota use in the Studio's deploy review (B, 2026-10-21, ADR 0012, [design](../design/tenancy-isolation.md), [runbook](../runbooks/tenancy-quotas.md); MOCK VERIFIED, Incus side REAL INCUS TESTED) | delete and rename (P2); per-tenant uplinks and address ranges (P2); quotas on networks and snapshots (P2) |
| Identity and access | users, groups, roles, custom roles, assignments with preview, service identities, policies, approvals, emergency access, sign-in providers; personal tokens bound to a person, with rotation (an overlap window), under *Your tokens* in the user menu, and everyone's under Identity & Access → Sign-ins & tokens (B, 2026-10-21, MOCK VERIFIED); a person's active sign-ins, ended by them or by an administrator (B, 2026-10-21, MOCK VERIFIED) | local accounts, sign-out, change password (IP); directory sign-in (IP); access-review export (P2) |
| Audit | search by action and outcome, paged; filters by actor, target and time, kept in the page address; export as CSV or JSON lines, itself audited (built 2026-10-21) | retention and archive (P1) |
| Debugging | request ids; preflight (now Servers → Preflight); support bundle: versions, effective settings without secrets, health, preflight, failed jobs with attempts, metrics, a manifest; audited, nothing uploaded (B, 2026-10-21, MOCK VERIFIED; [runbook](../runbooks/troubleshooting.md#support-bundle-and-effective-settings)) | log lookup by request id (P1); the last hour of errors in the bundle needs a log store the control plane does not have (it says so) |
| Security | certificate expiry | TLS on the API (P0); security headers (P0); rate limits on public routes (P0); a security-posture page (P1) |
| Administration | API tokens; versions; backup of Apelora's own database with a restore drill: scripts and runbook, no Console view yet ([runbook](../runbooks/apelora-database-backup.md)); effective settings, read-only, values by allow-list (B, 2026-10-21); licence notices: Apelora's licence with its source offer and every third-party package, made at build time and served with both apps at `/licenses` (B, 2026-10-21) | upgrade (P0); age of the last verified restore in the Console (P0, the drill writes `restore-drill-last-verified.json`) |
| Terminal | Studio sessions; Console: Terminal… on Instances and Applications, production approval in the dialog, Sessions page with End session and history from the audit trail (MOCK and CONTRACT VERIFIED; [guide](../guides/terminal-sessions.md)) | run against real Incus (P0); recording, opt-in (P2) |
| Disconnected (ADR 0010) | image mirrors, mirrors-only mode | preflight for mirrors; mirrors page; release bundle (P1) |

## Security and audit gaps in the code

1. **API tokens could grant more than their creator holds (P0). Fixed (S1, B).**
   - Token creation now refuses (403 `token_exceeds_caller`, audited as a denied `token.create` and as `access.denied`) a role that carries any permission the caller does not hold at platform level (`admin.rs` `create`, `authz::beyond_caller`).
   - A `fabric-admin` token needs another person's approval of `token.create_admin` on `token:fabric-admin` (a new catalogue permission with the approval flag, so the existing approval requests, `Apelora-Approval` header and single use apply). A legacy token cannot present an approval, so it cannot create `fabric-admin` tokens through the API (403 `approval_unavailable`); the command line on the server still can.
   - Legacy tokens now pass the same route gate as people and services (through their role's legacy mapping). They stay exempt from approvals and multi-factor rules, as documented, except for creating `fabric-admin` tokens; `APELORA_LEGACY_TOKENS=disabled` removes the exemption.
   - The command line always sets an expiry: `--expires-in-days` (`--expires-days` still accepted), 90 by default, 1–365.
   - Open: the Console's token dialog cannot send an approval yet, so a person creates a `fabric-admin` token through the API or the command line.
2. **Revoking a role** checked only `role.assign` at the scope (P1). **Fixed (S2, B):** the revoker must be able to grant the role there (`check_revocation`: `role.assign` and every permission the role confers at that scope); refusals are 403 `revocation_refused`, audited as a denied `role.revoke`. Ending an emergency grant still needs only `role.assign` or `platform.emergency_access`.
3. **Disabling a person** had no guard against disabling the last super administrator (P1). **Fixed (S2, B):** disabling a person, revoking an assignment and removing a group member are refused (409 `last_super_admin`, audited as denied) when they would leave no enabled person holding `platform-super-admin` at platform level, directly or through a group. The change and the count run in one transaction under an advisory lock, so two concurrent changes cannot both pass (`change_keeping_a_super_admin`). Membership through an identity provider's group claim is not counted.
4. **Audit integrity.**
   - Audit rows are written after the change, outside its transaction (P0).
   - The append-only trigger does not cover `TRUNCATE`.
   - There is no test that every state-changing route audits (P0).
   - Successful sign-ins are not audited.
   - Security settings taken from the environment are only logged.

   **Fixed (S4, built 2026-10-21, MOCK VERIFIED with a real PostgreSQL)**, except the last point, which stays open: a change and its audit row are written in one transaction (`authz::begin`/`commit`, `audit_scoped(&mut tx, …)`), with a short list of exceptions (changes made in Incus or in memory, approvals spent before the change; [IDENTITY-AND-AUTHORIZATION.md](../architecture/IDENTITY-AND-AUTHORIZATION.md) §8); a statement trigger refuses `TRUNCATE` (migration 0022); `audit_coverage_tests.rs` drives every state-changing route, checks its row and, with the audit table made unwritable, that nothing changed; a new route without a check or an exemption fails it; provider-token sign-ins are audited once per person per hour (`auth.oidc.signed_in`), password and directory sign-ins with their session. Every audit row also writes an outbox message, relayed by the worker.
5. **Public surface.**
   - No security headers or content security policy.
   - The API serves plain HTTP and can listen on a non-local address.
   - Public `/health` calls Incus on every request and returns Incus error text and its version.
   - No rate limits except on terminal tickets.

   All P0. **Fixed (S3, built 2026-10-21):** headers on every API response (`api/headers.rs`); `/health` answers a coarse status from a cache refreshed at most every 15 s, with Incus versions and error text moved to the signed-in `/system` (`api/health_cache.rs`); per-address and per-caller token buckets with `429` and `Retry-After` (`api/rate_limit.rs`, `APELORA_RATE_LIMIT_*`, `APELORA_TRUSTED_PROXIES`); TLS of its own (`APELORA_TLS_CERT`/`KEY`) or a declared proxy (`APELORA_BEHIND_PROXY=1`) before listening on a non-loopback address (`serve.rs`); a serving configuration with a content security policy proven in a browser (`infra/serve/`, `tests/e2e/web-serving.spec.ts`).
6. **Tokens.** Secrets are shown once and stored as hashes (good). Personal tokens rotate with an overlap window (built 2026-10-21); legacy API tokens still do not (replace them with personal tokens or service identities). A service token's lifetime outside 1–365 days is now refused with 400 instead of clamped (fixed, B).
7. **Sessions.** Disabling takes effect on the next request (good). Terminal tickets live in one process. Identity-provider tokens cannot be revoked before they expire (at most an hour).
8. **Clean:** no secrets in logs; no CSRF exposure (bearer tokens, no cookies); the terminal websocket checks `Origin`.

## Waves

Packages in one wave touch different files and can be built in parallel.

**Wave 0 — security fixes (backend):**
- S1 token containment (done).
- S2 guards on revoke and disable (done).
- S3 public surface (done): headers, `/health` redaction and cache, per-address limits, TLS or a declared proxy for non-local addresses, a serving configuration for the web apps with a content security policy.
- S4 audit integrity (done): audit in the same transaction; `TRUNCATE` blocked; a structural test that every state-changing route audits or is listed as exempt; the outbox and its relay.

**Wave 1 — operations core:**
- Navigation regrouped: Overview, Health, Servers, Workloads, Networks, Storage, Alerts, Identity & Access, Audit, Administration. **Done (B, 2026-10-21)**, without Alerts until there is something to show; old addresses lead to the new places.
- Audit filters and export (done): actor, target, action, outcome, from and to in the URL; CSV or JSON lines, audited as `audit.exported`.
- Job detail, retry and cancel (built).
- Instance detail with logs and restart (built).

**Wave 2 — clustering and infrastructure:**
- Servers area: members with database roles and quorum, groups, settings with approvals, upgrades, certificates. **Built read-only (B, 2026-10-21)**; settings can now be changed with a review, another person's approval and an audit row (`PATCH /api/v1/cluster/settings`, built, MOCK and CONTRACT VERIFIED; the Incus change REAL INCUS TESTED 2026-10-21).
- Upgrade assistant. Version skew, blockers and the written procedure are on Servers → Upgrades ([runbook](../runbooks/incus-rolling-upgrade.md)); the step-by-step assistant is not built.
- Networks page (read-only first) (built): per-application networks with the rules compiled from the design next to what Incus has, drift, uplink and address ranges.
- Storage page with backups, restore from backup, schedules and orphans (built).

**Wave 3 — alerts, terminal, identity:**
- Alerts: an evaluator in the worker, acknowledgement, a webhook channel holding only a secret reference (built, MOCK VERIFIED; migration 0025).
- Terminal from the Console, reusing the Studio's brokered sessions, with a sessions page (built, MOCK VERIFIED).
- Personal tokens and rotation (built, MOCK VERIFIED); a person's active sign-ins (built).
- Audited sign-ins (done): provider tokens once per person per hour; password and directory sign-ins were already audited, now in the session's transaction.

**Wave 4 — administration and tenancy:**
- Effective settings, licence notices and a support bundle (built 2026-10-21); a database backup check (built: Administration → Apelora's database backups, read-only, from the scripts' records).
- Restricted Incus projects per environment with limits, and quotas (built 2026-10-21, migration 0027).
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
