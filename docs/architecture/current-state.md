# Current state

Updated: 2026-10-20 (after Phase 09 slice 1, Phase 07 slice 5, Phase 10 slice 3) · Repository: `github.com/Apelora/apelora-fabric`, branch `main` · Related: [ADR 0001](../adr/0001-incus-first.md), [implementation phases](../development/IMPLEMENTATION-PHASES.md), [runbooks](../runbooks/README.md). The Phase 01 audit this file started as is summarised under *Earlier Xen-based assumptions* below.

Evidence labels: **REAL INCUS TESTED** (on the disposable test cluster, docs/development/TEST-CLUSTER.md), **MOCK VERIFIED** (against the mock provider), **CONTRACT VERIFIED** (against recorded Incus responses).

## What exists and works

| Area | Location | State |
|---|---|---|
| Apelora Studio (designer) | `apps/studio` | Catalog, canvas with typed terminals, cables and link tags, properties (resources, availability and replicas, internet access, configuration, ports), components, zones, undo/redo, import/export, validation; deploy dialog (plan preview, network rules, published ports, jobs) and a live overlay of every replica and service address. |
| Fabric Console | `apps/fabric-console` | Overview, instances (deploy, stop, delete), operations, preflight, networks and storage, membership (machines: add, approve, check and admit; members with evacuate/restore and dry run; unused join tokens), audit log, administration (API tokens, versions). |
| Control plane | `services/control-plane` | Axum API (OpenAPI 3 document, generated TypeScript client), API tokens and roles, inventory, preflight, instances, applications (plan, deploy, revisions, status with services), cluster administration (machine enrolment, join tokens, evacuation) through a separate admin identity, audit log. |
| Deployment worker | `services/deployment-worker` | Job queue with leases and fencing; instance lifecycle; `network.sync` (OVN networks, ACLs, address sets, load balancers with health-check repair, publishing, internet access); replica supervisor (recreate, restart, substitute on failed members, retire, sync retry, health with hysteresis and restart of failing replicas). |
| Domain, persistence, Incus adapter | `crates/` | Pure planning (replica slots, evacuation dry run, network rules), PostgreSQL 18 with migrations 0001–0009, Incus REST client with TLS pinning and a mock provider. |
| Catalog and model | `packages/` | 83 original appliance classes (NGINX and Debian deployable), zod schemas and generated JSON Schema, network intent compiler. |
| Tests | Rust, `packages/*/src/*.test.ts`, `tests/e2e`, `services/deployment-worker/tests/real_incus.rs` | 128 Rust tests, 76 unit tests, 86 Playwright tests (2 skipped) pass; 9 real Incus tests pass on the four-member test cluster. |
| CI | `.github/workflows/` | Frontend, Rust, generated-file drift, Playwright (pull requests run only the areas they change), dependencies and secrets (`cargo deny`, npm licences and audit, secret scan), dependency review, weekly upstream check; release workflow (draft releases with checksums). |
| Test cluster and drills | `scripts/test-cluster/`, `scripts/enrol/` | Four Lima VMs with Incus 7.0.1 (three database voters, one standby), OVN, placement scriptlet; `demo.sh` runs the end-to-end demonstration; `enrol-member.sh` enrols a server by hand. |

## Earlier Xen-based assumptions

The previous plan (Rust controller + Rust node agent driving Xen, a VM per appliance) exists only in documents and agent instructions. Status after Phase 01:

| Item | Action |
|---|---|
| `docs/ARCHITECTURE.md` | Marked superseded; points to the platform architecture |
| `docs/XEN-LICENSING-STRATEGY.md` | Marked superseded (kept for history) |
| `xen-integration` skill | Marked superseded; replaced by `incus-platform` |
| Other skills and agents mentioning Xen/dom0/hypervisor | Rewritten for Incus (security-review, mcp-security, license-compliance, quality-assurance, system-design-framework, observability, fabric-networking, rust-control-plane, ui-engineer, qa-reviewer) |
| `CLAUDE.md`, `README.md` | Updated to Incus-first; project rules moved to `AGENTS.md` (read by Codex) and imported by `CLAUDE.md` |
| `SECURITY.md`, `docs/development/MCP-HOSTING.md`, `docs/architecture/STUDIO-MVP.md`, `prompts/01-build-studio.md` | Updated for Incus (prompt marked historical) |
| `COMMERCIAL-LICENSE.md`, `COMMERCIAL-AGREEMENT-DRAFT.md` | Xen references replaced with Incus/upstream platform software at the owner's explicit request (rule 8); meaning unchanged |

## What carries over unchanged

- The whole Studio app and both packages. The Incus runtime is an *addition* to an appliance class (Phase 06), not a change to existing documents.
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
| Disposable test cluster (`scripts/test-cluster`, Lima VMs) | Done; three members, OVN, placement scriptlet, see `docs/development/TEST-CLUSTER.md` | 04 |
| Image pinning (fingerprint / OCI digest) and architecture checks | Done; REAL INCUS TESTED on one member (mixed-architecture and offline-member paths unit-tested only) | 04 |
| Catalog runtime fields (`runtime`, `instanceRequestFor`); NGINX and Debian deployable | Done; REAL INCUS TESTED | 04 |
| Cluster join tokens with approval (ADR 0003), second test member | Done; REAL INCUS TESTED (two members) | 04 |
| Auth (API tokens) and roles | Done; OIDC sign-in and project scoping later | 03 |
| OpenAPI document + generated TS client (`packages/api-client`) | Done; console uses it | 03 |
| Fabric Console views (instances and deploy, operations, preflight, networks and storage, membership, administration, audit) | Done (slices 1–2); verified in a real browser against the test cluster | 05 |
| Application deployment backend (plan, deploy, revisions, status by appliance) | Done; REAL INCUS TESTED (two members) | 06 |
| Studio deploy panel and live status overlay | Done; verified in a real browser against the test cluster | 06 |
| Network intent compiler (rules, service names, segments) | Done (TypeScript, tested); shown in the Studio, not enforced | 07 |
| Enforcing connections (per-application OVN network, ACLs and address sets) | Done; REAL INCUS TESTED across two members | 07 |
| Cleanup of network objects with the appliances (owner-labelled) | Done; REAL INCUS TESTED | 07 |
| Publishing (gateway connections → service address + ingress rule) | Done; REAL INCUS TESTED (outside route required) | 07 |
| Internet access per appliance (`@external` egress) | Done; REAL INCUS TESTED | 07 |
| Service discovery: `APELORA_SERVICE_<PROVIDER>` in every consumer (ADR 0004) | Done; REAL INCUS TESTED | 07 |
| Stateless replicas, anti-affinity (placement scriptlet), supervisor restoring lost replicas | Done; REAL INCUS TESTED (two members) | 08 |
| Service addresses (OVN load balancers with health checks) and restart of crashed replicas | Done; REAL INCUS TESTED end to end (two members) | 07/08 |
| Evacuation with dry run (API, console), audited; supervisor stays out of the way | Done; REAL INCUS TESTED (two members) and verified in a real browser | 08 |
| Recovery from a failed member (substitute, then retire), health-check repair, sync retry | Done; REAL INCUS TESTED (member VM force-stopped) | 08 |
| Health with hysteresis from the load balancer's port check; restart of failing stateless replicas (paced, audited); health in status and Studio | Done; REAL INCUS TESTED (four members, `demo.sh`) | 08 |
| Application-level (HTTP) readiness, service names (DNS), rebalancing | Missing | 08 |
| Resources per appliance as Incus limits (CPU, memory, fractional CPU quota) | Done; REAL INCUS TESTED (catalog runtimes) | 06 |
| Volumes per appliance; resizing existing instances | Missing | 06 |
| Machine enrolment (manual provider): register, approve, enrolment script, held until admitted after checks; console | Done; REAL INCUS TESTED (fourth member) and verified in a real browser | 09 |
| MAAS adapter (discovery, installation, power), member removal | Missing; ADR 0002 accepted | 09 |
| CI: frontend, Rust, generated-file drift, Playwright, dependency licences/advisories (`cargo deny`, npm), upstream check | Done | 10 |
| Runbooks (join, maintenance, failure, deploy identity, revocation, outage); three-member test cluster | Done (partly verified, labelled per step) | 10 |
| Secret scanning in CI; release workflow (draft releases; `v0.1.0-alpha.1` built); end-to-end demonstration (`demo.sh`); certificate rotation; path-aware CI | Done; demonstration and rotation REAL INCUS TESTED (three members, three runs) | 10 |
| Staged upgrades with rollback, remaining runbooks | Missing | 10 |

## Known limitations

- **Incus 7.0.2** is pinned but not yet tested: the package repository still ships 7.0.1, which all evidence was produced with.
- **One architecture** (aarch64) on the test cluster: no mixed-architecture evidence.
- **Member failure:** OVN health checks cannot see a dead member, so a service address keeps sending a share of requests to its replicas there until Apelora substitutes them (`APELORA_RECOVER_AFTER_SECS`, default 120 s). Single-instance and stateful appliances are not recovered automatically.
- **Publishing** needs the network outside to route service addresses to the application network's router; the OVN router does not answer ARP for them.
- **Service names (DNS)** inside applications are not provided (ADR 0004): consumers read `APELORA_SERVICE_<PROVIDER>`, set when they are created; an instance that already existed keeps the environment it was created with.
- **Resources** apply when an instance is created; changing them in the Studio does not resize instances that already exist.
- **Health** is Incus status plus the load balancer's TCP port check, with hysteresis; an application that hangs behind an open port is not detected, and instances without a service address are not checked. Health trackers live in the worker's memory: after a worker restart they start over.
- **Load balancer quirks (Incus 7.0.1):** updates may be refused while a member is down — in one drill for the whole outage (retried; the substitute joins once they succeed) — and may drop the OVN health check (detected through the state endpoint and re-applied).
- **Offline detection:** in one drill Incus briefly reported a member that was not stopped as Offline; keep the recovery delay well above that (default 120 s).
- **Bare-metal provisioning** (Phase 09): servers installed by hand are enrolled, held and admitted through Apelora; the MAAS adapter (discovery, installation, power) is not implemented yet, and removing a member from the cluster is not offered. Incus cannot hold a member before it joins, so the enrolment script holds it right after the join; a member that joins another way is flagged, not blocked.
- **Releases:** `v0.1.0-alpha.1` exists as a draft release (artefacts and checksums built by the release workflow); no staged upgrade procedure yet.

## Deviations from the incoming specification (deliberate)

- **npm workspaces kept** instead of pnpm (owner decision): no tooling churn; Fabric Console is another workspace.
- **Crates created when first needed**, not all at once: Phase 01 adds `crates/domain` and `crates/incus-adapter` plus the two services; the other planned crates are listed in the architecture but created by the phase that gives them code.
- **No other product names** in repository text (rule 9); the incoming documents were rewritten in our own words and the `files/` drop folder removed.
- **No gradients** in UI themes (design rule 5), although the incoming spec mentions gradient themes.
- Repository stays `Apelora/apelora-fabric` (the spec says `Apelora/apelora`).
