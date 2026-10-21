# Current state

Updated: 2026-10-20 (after Phase 09 slice 1, Phase 07 slice 5, Phase 10 slice 3) · Repository: `github.com/Apelora/apelora-fabric`, branch `main` · Related: [ADR 0001](../adr/0001-incus-first.md), [implementation phases](../development/IMPLEMENTATION-PHASES.md), [runbooks](../runbooks/README.md). The Phase 01 audit this file started as is summarised under *Earlier design* below.

Evidence labels: **REAL INCUS TESTED** (on the disposable test cluster, docs/development/TEST-CLUSTER.md), **MOCK VERIFIED** (against the mock provider), **CONTRACT VERIFIED** (against recorded Incus responses).

## What exists and works

The parts of an application are called **modules** (formerly *appliances*; [ADR 0006](../adr/0006-modules.md)). Designs, API requests, stored records and Incus labels written before the rename are still read. Illustrations: [platform layers](../assets/diagrams/platform-layers.svg), [from a design to running instances](../assets/diagrams/design-to-cluster.svg), [storage layers](../assets/diagrams/storage-layers.svg).

| Area | Location | State |
|---|---|---|
| Apelora Studio (designer) | `apps/studio` | Catalog, canvas with typed terminals, cables and link tags, properties (resources, availability and replicas, internet access, configuration, ports), components, zones, undo/redo, import/export, validation; deploy dialog (plan preview, network rules, published ports, jobs) and a live overlay of every replica and service address. |
| Fabric Console | `apps/fabric-console` | Overview, Health (each component's state with reason and next step; every server's CPU, memory and storage with warnings; [runbook](../runbooks/health-page.md)), applications (instances, port checks, service addresses), instances (deploy, stop, delete), operations, preflight, networks and storage, membership (machines: add, approve, check and admit; members with evacuate/restore and dry run; unused join tokens), audit log, administration (API tokens, versions, certificate expiry). |
| Control plane | `services/control-plane` | Axum API (OpenAPI 3 document, generated TypeScript client), API tokens and roles, inventory, preflight, instances, applications (plan, deploy, revisions, status with services), cluster administration (machine enrolment, join tokens, evacuation) through a separate admin identity, audit log. |
| Deployment worker | `services/deployment-worker` | Job queue with leases and fencing, woken by `LISTEN/NOTIFY` when a job is queued (2 s check kept as the fallback); the replica supervisor runs only in the holder of a leader lease in PostgreSQL, with fenced writes, so several workers can run (MOCK VERIFIED with a real PostgreSQL, 2026-10-21); instance lifecycle; `network.sync` (OVN networks, ACLs, address sets, load balancers with health-check repair, publishing, internet access); replica supervisor (recreate, restart, substitute on failed members, retire, sync retry, health with hysteresis and restart of failing replicas). |
| Domain, persistence, Incus adapter | `crates/` | Pure planning (replica slots, evacuation dry run, network rules), PostgreSQL 18 with migrations 0001–0009, Incus REST client with TLS pinning and a mock provider. |
| Catalog and model | `packages/` | 83 original module classes, three deployable: NGINX (OCI container), Debian (system container), Linux base (VM, for live migration); zod schemas and generated JSON Schema, network intent compiler. |
| Tests | Rust, `packages/*/src/*.test.ts`, `tests/e2e`, `services/deployment-worker/tests/real_incus.rs` | 131 Rust tests, 76 unit tests, 88 Playwright tests (2 skipped) pass; 9 real Incus tests pass on the four-member test cluster. |
| CI | `.github/workflows/` | Frontend, Rust, generated-file drift, Playwright (pull requests run only the areas they change), dependencies and secrets (`cargo deny`, npm licences and audit, secret scan), dependency review, weekly upstream check; release workflow (draft releases with checksums). |
| Test cluster and drills | `scripts/test-cluster/`, `scripts/enrol/` | Four Lima VMs standing in for physical servers, with Incus 7.0.1 (three database voters, one standby), plus a Lima VM `apelora-san` standing in for a SAN (iSCSI target); OVN, placement scriptlet; pools `local` (btrfs), `backup` (dir) and `shared` (clustered LVM on the SAN, `shared-storage.sh`, 2026-10-21, not used by Apelora yet); `demo.sh` runs the end-to-end demonstration; `enrol-member.sh` enrols a server by hand. |
| Metrics | `crates/metrics`, both services, Console → Administration → Monitoring | `GET /metrics` in the text exposition format (OpenMetrics on request): API requests by route template, auth outcomes, authorisation denials, jobs and queue age, worker attempts and lease losses, supervisor actions and passes, scheduled snapshots and backups, Incus calls by endpoint template with reachability, database pool, inventory and application status counts, certificate expiry, process. The control plane's endpoint needs `fabric.metrics.read` (built-in role Metrics Reader); the worker serves metrics and a liveness probe on `127.0.0.1:9471`; constant `region`/`site`/`sector` labels. Incus call metrics CONTRACT VERIFIED; the rest MOCK VERIFIED against a real PostgreSQL; not REAL INCUS TESTED. Collecting Incus's own `/1.0/metrics` with a metrics certificate is documented, not yet run on the test cluster. [Monitoring](MONITORING.md). |
| Logs and troubleshooting | both services | `RUST_LOG` levels per crate, `APELORA_LOG_FORMAT=json`, an `x-request-id` on every API response that is on every log line of the request and in its audit detail, a `job` span on every worker line with the `request_id` that queued the job (stored on the job with every Incus operation it started, shown in the Console's Operations view), refusals (403, spent approvals) audited with their code, audit write failures logged as errors (MOCK VERIFIED). [Troubleshooting runbook](../runbooks/troubleshooting.md). |
| Public surface | control plane, `infra/serve/` | Security headers on every API response; public `/health` coarse and cached (Incus asked at most every 15 s; versions and Incus errors only on the signed-in `/system`); per-address and per-caller rate limits with `429` + `Retry-After` (per process); TLS of its own or a declared proxy before a non-loopback address; an example proxy and static serving configuration whose content security policy is proven by loading both production builds in Chromium (no violation, no request outside the origin). MOCK VERIFIED (API tests with a real TLS handshake) and REAL-BROWSER TESTED (policy); the example proxy configuration itself has not been run. |

## Identity and application management (2026-10-20)

| Area | State |
|---|---|
| Identity, tenancy, authorization | Organisations, projects, environments, users, groups, eleven predefined (Metrics Reader added 2026-10-21) and custom roles, scoped assignments with an escalation guard, service identities, approvals, emergency access, append-only audit, OIDC token validation, identity providers configured and edited in the Fabric Console with lockout guards, a designed sign-in page with PKCE (tested in a real browser against a local development provider), a default-deny route gate with exact-resource checks (MOCK VERIFIED; 46 auth, 10 identity, 29 persistence and 22 API tests). Credential containment (2026-10-21): an API token never carries more than its creator holds, a `fabric-admin` token needs another person's approval, every token expires within a year (API and command line), legacy tokens pass the route gate, revoking a role needs the power to grant it, and the last enabled super admin cannot be disabled, revoked or removed from their group, race-safely (MOCK VERIFIED against PostgreSQL; 8 API, 2 persistence and 1 auth tests). Live identity provider, MAAS 3.8 mapping and Incus restricted projects are NOT VERIFIED. [Details](IDENTITY-AND-AUTHORIZATION.md). |
| Fabric Console → Identity & Access | Eight pages plus assignment preview with effective-permission origins, emergency access dialog and copy-once service tokens; Playwright with a stubbed API and axe (no serious violations). |
| Application management | Drafts with conflict detection, immutable published versions, server-derived status with per-caller permitted actions, Start/Restart/Stop as dependency-ordered operations carried out by the worker (MOCK VERIFIED). Studio: Applications page (cards and list, search, filters, sorting, pagination), per-application workspace (Overview, Designer, Deployments, Monitoring, Versions, Settings), icon picker using the editor's logos, publish and deploy-version flows. Run against the real control plane and worker with the mock provider: sign-in, draft save, publish, deploy through the worker, and a refused Start that showed the server's real error ("none of the application's instances exist in Incus"). The mock inventory is static, so Start/Stop on running instances is not yet REAL INCUS TESTED. |
| Interactive access to modules | Terminal sessions (and PowerShell for Windows modules) brokered by the control plane: permissions `instance.console`/`instance.desktop`, the **Instance Access** role, production approval, single-use tickets, limits, re-checked access, audit without content; Studio right-click and Monitoring entries with xterm. MOCK VERIFIED and CONTRACT VERIFIED; not run against a real Incus, a Windows guest or a live identity provider. SSH daemons are deliberately not used. **RDP is not built** (no gateway). [Details](IDENTITY-AND-AUTHORIZATION.md#7-interactive-access-to-modules-terminal-powershell). |
| Mutation testing | `cargo mutants` on the authorisation crate (230 caught, 0 missed), the access and lifecycle rules, and Stryker on the model rules. [How and results](../development/MUTATION-TESTING.md). |

## Earlier design

An earlier hypervisor-based design (a Rust node agent and a VM per module) was never built and was replaced by [ADR 0001](../adr/0001-incus-first.md). Its documents and skill were removed on 2026-10-20; skills, agents, README, CLAUDE.md, SECURITY.md and the commercial licence drafts describe the Incus-first platform.

## What carries over unchanged

- The whole Studio app and both packages. The Incus runtime is an *addition* to a module class (Phase 06), not a change to existing documents.
- Port/terminal model, catalog rules, port placement convention, visual design contract.
- Licence: AGPL-3.0-only for our code. Incus (Apache-2.0) and MAAS (AGPL-3.0, if adopted per ADR 0002) are used over their APIs.

## Gaps against the target architecture

| Target | Gap | Phase |
|---|---|---|
| Cargo workspace, control-plane and worker services | Skeleton created in Phase 01 (compiles, no behaviour) | 01 → 03 |
| Upstream tracking (`upstream.lock.yaml`, update checks, fork `Apelora/incus`) | Done in Phase 02 (`docs/development/UPSTREAM-INCUS.md`) | 02 |
| Incus adapter (read-only + mock) and inventory API | Done; REAL INCUS TESTED on the disposable test cluster (Incus 7.0.1) | 03 |
| PostgreSQL persistence and worker job loop (leases, fencing) | Done, tested on real PostgreSQL 18 | 03 |
| Instance lifecycle (create/start/stop/delete), ownership marker, project allow-list | Done; REAL INCUS TESTED for LXC, OCI and VM (one member) | 04 |
| Read-only preflight (`/api/v1/preflight`) | Done; REAL INCUS TESTED; clock and latency not measurable via the API | 04 |
| Disposable test cluster (`scripts/test-cluster`, Lima VMs) | Done; four members (Lima VMs standing in for physical servers) and a SAN VM, OVN, placement scriptlet, see `docs/development/TEST-CLUSTER.md` | 04 |
| Image pinning (fingerprint / OCI digest) and architecture checks | Done; REAL INCUS TESTED on one member (mixed-architecture and offline-member paths unit-tested only) | 04 |
| Catalog runtime fields (`runtime`, `instanceRequestFor`); NGINX and Debian deployable | Done; REAL INCUS TESTED | 04 |
| Class metrics descriptions (`ModuleClass.metrics`, all 83 classes; `mon` only where metrics text is served on a port; a `mon` connection opens the class's own port for the collector) | Done; unit-tested (MOCK VERIFIED); facts researched from project documentation, unverified ones marked; no runtime turns metrics on yet | 07 |
| Cluster join tokens with approval (ADR 0003), second test member | Done; REAL INCUS TESTED (two members) | 04 |
| Auth (API tokens) and roles | Done, with OIDC sign-in configured in the Console | 03 |
| OpenAPI document + generated TS client (`packages/api-client`) | Done; console uses it | 03 |
| Fabric Console views (instances and deploy, operations, preflight, networks and storage, membership, administration, audit) | Done (slices 1–2); verified in a real browser against the test cluster | 05 |
| Application deployment backend (plan, deploy, revisions, status by module) | Done; REAL INCUS TESTED (two members) | 06 |
| Studio deploy panel and live status overlay | Done; verified in a real browser against the test cluster | 06 |
| Network intent compiler (rules, service names, segments) | Done (TypeScript, tested); shown in the Studio, not enforced | 07 |
| Enforcing connections (per-application OVN network, ACLs and address sets) | Done; REAL INCUS TESTED across two members | 07 |
| Cleanup of network objects with the modules (owner-labelled) | Done; REAL INCUS TESTED | 07 |
| Publishing (gateway connections → service address + ingress rule) | Done; REAL INCUS TESTED (outside route required) | 07 |
| Internet access per module (`@external` egress) | Done; REAL INCUS TESTED | 07 |
| Service discovery: `APELORA_SERVICE_<PROVIDER>` in every consumer (ADR 0004) | Done; REAL INCUS TESTED | 07 |
| Stateless replicas, anti-affinity (placement scriptlet), supervisor restoring lost replicas | Done; REAL INCUS TESTED (two members) | 08 |
| Service addresses (OVN load balancers with health checks) and restart of crashed replicas | Done; REAL INCUS TESTED end to end (two members) | 07/08 |
| Evacuation with dry run (API, console), audited; supervisor stays out of the way | Done; REAL INCUS TESTED (two members) and verified in a real browser | 08 |
| Recovery from a failed member (substitute, then retire), health-check repair, sync retry | Done; REAL INCUS TESTED (member VM force-stopped) | 08 |
| Health with hysteresis from the load balancer's port check; restart of failing stateless replicas (paced, audited); health in status and Studio | Done; REAL INCUS TESTED (four members, `demo.sh`) | 08 |
| Application-level (HTTP) readiness, service names (DNS), rebalancing | Missing | 08 |
| Resources per module as Incus limits (CPU, memory, fractional CPU quota) | Done; REAL INCUS TESTED (catalog runtimes) | 06 |
| Volumes per module (backend): `data-local` class, pool from `APELORA_VOLUME_POOL`, one volume per instance created on the member Incus placed it on, attached before start, kept when the instance is deleted, never adopted | Done; REAL INCUS TESTED (`volumes_follow_the_instance_and_outlive_it`) | 06 |
| Volume records (`volumes`, migration 0012), listing with live state (attached, kept, missing), explicit delete with the name typed (`application.delete`, audited); Fabric Console Volumes panel | Done; REAL INCUS TESTED (four members) and verified in a real browser | 06 |
| Volumes in designs: class defaults (`runtime.volumes`), per-module volumes in the Studio Properties window (checked against the schema, refused for stateless classes), listed in the deploy review and sent with the plan | Done; MOCK VERIFIED in the browser (stubbed control plane); the request shape is the one REAL INCUS TESTED through the API | 06 |
| Volume grow (online, grow-only: a shrink is refused before it reaches Incus), API and Fabric Console | Done; REAL INCUS TESTED (adapter, running instance) | 06 |
| Volume snapshots: take (named by time, says whether crash-consistent), list, restore with the name typed and only while every user is stopped, delete; audited; Fabric Console Snapshots dialog | Done; REAL INCUS TESTED (adapter and in a real browser against the cluster) | 06 |
| Scheduled snapshots (kept to what the schedule says; manual ones never pruned) and backups copied into a backup pool on another member, scheduled or on request (worker) | Done; REAL INCUS TESTED (backup copy across pools and members) | 06 |
| Backup API and console, restore from a backup | Missing | 06 |
| Backup of Apelora's own database (ADR 0011 decision 6): `scripts/ops/db-backup.sh` (custom-format dump to a directory outside the managed cluster, SHA-256, manifest, retention, optional `age`/`openssl cms` encryption) and `scripts/ops/db-restore-drill.sh` (restore into a scratch database, verify migrations, row counts and the audit append-only trigger, drop it, write `restore-drill-last*.json`); self-test `scripts/ops/test-db-backup.sh` in CI; [runbook](../runbooks/apelora-database-backup.md) with continuous archiving and a full restore of the plane | Done; tested against a real PostgreSQL 18.6 (local dev database, macOS and Linux, 2026-10-21); `age`, the systemd units, continuous archiving and the full plane restore NOT YET VERIFIED; the Console view of the last verified restore is missing | 10 |
| Moving an instance to another member (`instance.move` job; dry run, typed name): VMs with live migration move running; containers and VMs with `data-local` volumes stop, their volumes are copied and the old copies deleted once it runs there; Studio *Keep running when moved* for VM modules; Fabric Console *Move…* | Done; REAL INCUS TESTED (container with a volume; VM live, 0.26 s longest pause, no reboot, from the console in a real browser) | 08 |
| Restart of instances on shared storage (containers or VMs) on another member once the failed one is fenced; rebalancing | Missing (design: docs/design/mobility-and-ha.md, slice B); the lab's `shared` pool exists on all four members (REAL INCUS TESTED, 2026-10-21), Apelora places nothing on it yet | 08 |
| Resizing existing instances | Missing | 06 |
| Machine enrolment (manual provider): register, approve, enrolment script, held until admitted after checks; console | Done; REAL INCUS TESTED (fourth member) and verified in a real browser | 09 |
| Member removal: check (evacuated, nothing left on it, recorded volumes, cluster keeps running), typed confirmation, admin identity, audited; Fabric Console *Remove…* | Done; REAL INCUS TESTED (member 4 removed and enrolled again) | 09 |
| MAAS adapter (discovery, installation, power) | Missing; ADR 0002 accepted | 09 |
| CI: frontend, Rust, generated-file drift, Playwright, dependency licences/advisories (`cargo deny`, npm), upstream check | Done | 10 |
| Runbooks (join, maintenance, failure, deploy identity, revocation, outage); test cluster grown to three members (four since Phase 09) | Done (partly verified, labelled per step) | 10 |
| Secret scanning in CI; release workflow (draft releases; `v0.1.0-alpha.1` built); end-to-end demonstration (`demo.sh`); certificate rotation; path-aware CI | Done; demonstration and rotation REAL INCUS TESTED (three members, three runs) | 10 |
| Staged upgrades with rollback, remaining runbooks | Missing | 10 |
| Installations without internet access (ADR 0010): image mirrors and mirrors-only mode where every instance is created; preflight checks for mirrors and `GET /api/v1/image-mirrors` (catalog servers from the generated `packages/module-catalog/catalog-images.json`, mirrors probed at most once a minute, upstream servers never); Fabric Console Administration → Image mirrors; signed offline release bundle (`scripts/release/bundle.sh`, `verify.sh`) and [offline-update runbook](../runbooks/offline-update.md); blocked-egress lab profile (`scripts/test-cluster/egress.sh`) | Mirrors and the preflight/API CONTRACT VERIFIED (unit and API tests, probes against local test servers); Console MOCK VERIFIED (Playwright, stubbed API); bundle built, signed and verified locally (tampering refused); egress rules accepted by `nft --check` on all four members, `block` not run; nothing REAL INCUS TESTED with egress blocked or with a mirror | 10 |

## Known limitations

- **Incus 7.0.2** is pinned but not yet tested: the package repository still ships 7.0.1, which all evidence was produced with.
- **One architecture** (aarch64) on the test cluster: no mixed-architecture evidence.
- **Member failure:** OVN health checks cannot see a dead member, so a service address keeps sending a share of requests to its replicas there until Apelora substitutes them (`APELORA_RECOVER_AFTER_SECS`, default 120 s). Single-instance and stateful modules are not recovered automatically.
- **Publishing** needs the network outside to route service addresses to the application network's router; the OVN router does not answer ARP for them.
- **Service names (DNS)** inside applications are not provided (ADR 0004): consumers read `APELORA_SERVICE_<PROVIDER>`, set when they are created; an instance that already existed keeps the environment it was created with.
- **Resources** apply when an instance is created; changing them in the Studio does not resize instances that already exist.
- **Health** is Incus status plus the load balancer's TCP port check, with hysteresis; an application that hangs behind an open port is not detected, and instances without a service address are not checked. Health trackers live in the worker's memory: after a worker restart they start over.
- **Load balancer quirks (Incus 7.0.1):** updates may be refused while a member is down — in one drill for the whole outage (retried; the substitute joins once they succeed) — and may drop the OVN health check (detected through the state endpoint and re-applied).
- **Offline detection:** in one drill Incus briefly reported a member that was not stopped as Offline; keep the recovery delay well above that (default 120 s).
- **Bare-metal provisioning** (Phase 09): servers installed by hand are enrolled, held and admitted through Apelora; the MAAS adapter (discovery, installation, power) is not implemented yet. An unreachable member is not removed through Apelora (Incus can only force it; see the runbook). Incus cannot hold a member before it joins, so the enrolment script holds it right after the join; a member that joins another way is flagged, not blocked.
- **Health and capacity** (2026-10-21): the page shows the present only (no history or alerts yet); identity providers are reported from the control plane's last check, not probed per read; the per-server instance list covers only the projects Apelora's Incus identity may see; the Studio's capacity fit is advice (Incus reserves nothing for limits; root disks are not counted).
- **Releases:** `v0.1.0-alpha.1` exists as a draft release (artefacts and checksums built by the release workflow); no staged upgrade procedure yet.

## Deviations from the incoming specification (deliberate)

- **npm workspaces kept** instead of pnpm (owner decision): no tooling churn; Fabric Console is another workspace.
- **Crates created when first needed**, not all at once: Phase 01 adds `crates/domain` and `crates/incus-adapter` plus the two services; the other planned crates are listed in the architecture but created by the phase that gives them code.
- **No other product names** in repository text (rule 9); the incoming documents were rewritten in our own words and the `files/` drop folder removed.
- **No gradients** in UI themes (design rule 5), although the incoming spec mentions gradient themes.
- Repository stays `Apelora/apelora-fabric` (the spec says `Apelora/apelora`).
