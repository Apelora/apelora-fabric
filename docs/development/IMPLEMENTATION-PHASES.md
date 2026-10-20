# Implementation phases

Order of work for the Incus-first platform ([ADR 0001](../adr/0001-incus-first.md), [architecture](../architecture/PLATFORM-ARCHITECTURE.md)). One phase at a time; a phase is done only when its acceptance checks have evidence. Every result is labelled **MOCK VERIFIED**, **CONTRACT VERIFIED** or **REAL INCUS TESTED**; anything else is NOT TESTED.

Standing rules for every phase:
- Keep Studio working; run the full Studio checks (`npm run typecheck && npm run lint && npm test && npm run build && npm run test:e2e`) before and after.
- Rust: `cargo fmt --check`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`.
- No infrastructure mutation except on an explicitly designated, disposable test cluster, after a dry run.
- No fabricated telemetry, deployments or HA claims; no TLS verification bypass; no secrets in the repository.
- Update docs and record significant decisions as ADRs.

| # | Phase | Skills |
|---|---|---|
| 01 | Audit and monorepo restructure | rust-control-plane, security-tenancy, testing-operations |
| 02 | Upstream tracking | upstream-maintenance, license-compliance |
| 03 | Rust API, domain, Incus adapter (read-only + mock) | rust-control-plane, incus-platform, security-tenancy |
| 04 | Cluster management and appliance deployment | incus-platform, appliance-catalog, storage-recovery |
| 05 | Fabric Console | studio-design-system, daisyui, playwright-qa |
| 06 | Studio deployment integration | studio-reactflow, visual-designer, appliance-catalog |
| 07 | Network fabric, service discovery, publishing | fabric-networking, incus-networking |
| 08 | Coordination, HA and mobility | reconciliation-ha, storage-recovery, testing-operations |
| 09 | Bare-metal provisioning and node enrolment | incus-platform, security-tenancy (ADR 0002) |
| 10 | CI/CD, security hardening, releases, runbooks | testing-operations, security-review, observability, upstream-maintenance |

## 01 — Audit and monorepo restructure

Audit (`docs/architecture/current-state.md`), ADR 0001, superseded Xen material marked, project rules in `AGENTS.md` (Codex) imported by `CLAUDE.md`, skills shared under `.agents/skills`. Cargo workspace (toolchain pinned in `rust-toolchain.toml`) with `crates/domain`, `crates/incus-adapter`, `services/control-plane`, `services/deployment-worker`; `apps/fabric-console` placeholder; `.env.example`.
**Accept:** Studio and Fabric Console build; Rust workspace compiles, formats, lints and tests; Studio import/export/render unchanged; no credentials in the repo.

## 02 — Upstream tracking

Verify the Incus 7.0 LTS install method, host minimums, licence and the REST endpoints/extensions we need, with source links and dates. Create `upstream.lock.yaml` (never invent versions, hashes or CVEs). Scripts under `scripts/upstream/` (status, check-updates, verify; optional read-only source checkout into a git-ignored path). Scheduled/manual workflow that opens review-only update PRs with least-privilege permissions. The adapter negotiates `api_extensions` and returns `UnsupportedCapability` instead of guessing.
**Accept:** a clean checkout validates the tracked release; no build needs Incus source; no unattended upgrades.
**Status:** done — pin `v7.0.2` (`01f81e5e`), fork `Apelora/incus` with patch branch `apelora/7.0`, scripts and weekly workflow; procedure in `UPSTREAM-INCUS.md`. Adapter-side `api_extensions` negotiation is partly in place (`require_extension`) and is completed against real responses in Phase 03.

## 03 — Rust API, domain and Incus adapter

Domain types (cluster, member, role, instance and kind, project, profile, network, storage pool, image, appliance class/instance, application graph, deployment, operation, health evidence, role, audit event) with desired and observed state kept apart. `incus-adapter`: mTLS client, version discovery (`GET /`, `/1.0`), extension checks, read-only inventory (members, projects, instances, images, profiles, networks, storage pools, operations), typed errors (auth, connectivity, quota, capability, operation failure, unknown state), bounded timeouts, no blind retries of non-idempotent calls. Mock provider with fixtures. `control-plane`: `/api/v1`, auth middleware, project-scoped RBAC, OpenAPI → generated TS client. `persistence`: PostgreSQL migrations for Apelora state only. `deployment-worker`: durable jobs, leases, read-only mode.
Local development: PostgreSQL from a compose file bound to loopback on a non-standard port; SQLx offline query metadata committed so builds and CI need no database; a parallel test runner for the Rust suite.
**Accept:** contract tests against recorded Incus responses; read real member and instance inventory from an authorised test cluster (REAL INCUS TESTED) before any write operation exists.
**Status (slice 1 done):** read-only adapter over mTLS with pinned server certificate and the `unspecified` TLS-name case, mock provider, API inventory endpoints with source labels, Fabric Console members/instances views — CONTRACT VERIFIED (spec-shaped fixtures, real rustls handshakes), NOT yet REAL INCUS TESTED. Slice 2 done: PostgreSQL persistence (applications, immutable revisions, jobs, audit) with migrations, and the worker job loop with leases, fencing generations, idempotent enqueue and retry backoff — tested against real PostgreSQL 18. Slice 3 done: roles and permissions, hashed API tokens with expiry/revocation and audit, enforced on every route when a database is configured (development mode only on loopback), console sign-in. Slice 4 done: typed responses with an OpenAPI 3.1 document (`GET /api/v1/openapi.json`, `apelora-control-plane openapi`), committed as `packages/api-client/openapi.json`; `@apelora/api-client` (types and a typed `get` client) is generated from it and used by the Fabric Console. A Rust test and the CI generated-file check fail when either file is stale. Phase 03 is complete. Real inventory (members, projects, networks, storage pools, instances) was read from the disposable test cluster on 2026-10-20 — REAL INCUS TESTED (Incus 7.0.1).

## 04 — Cluster management and appliance deployment

Read-only host preflight (kernel ≥ 6.12, Incus version, clock drift, member latency ≤ 10 ms, DNS, TLS, pools, networks, project restrictions, capacity). Surface Incus bootstrap/join with short-lived tokens and operator approval. Instance create/start/stop/delete with async operation tracking for the three runtimes, image architecture and digest checks, restricted projects by default. Catalog runtime fields (see `appliance-catalog`). Local-development storage profile; cross-member networking only claimed after it is tested.
**Accept:** on the test cluster: members shown, project verified, disposable LXC and OCI instances created/started/stopped/deleted with operation ids, nothing unrelated touched, no secrets leaked.

**Status (slice 1 done, CONTRACT VERIFIED — not yet REAL INCUS TESTED):** instance create/start/stop/delete for system containers, OCI containers and VMs. The API (`POST /api/v1/instances`, `POST …/{project}/{name}/state`, `DELETE …/{project}/{name}`, `GET /api/v1/jobs/{id}`) checks the `deploy` permission, validates names, allows only the projects in `APELORA_DEPLOY_PROJECTS` (default `apelora`), honours `Idempotency-Key`, audits the request and queues a job. The worker converges level-triggered: it re-reads the instance before every call, records the Incus operation id before waiting on it (fenced), waits on an earlier attempt's operation after a crash, stops before deleting, and rejects without retrying anything lacking Apelora's ownership marker (`user.apelora.managed=true`). Tested with spec-shaped fixtures for the Incus calls and real PostgreSQL for the queue. In mock mode the control plane and the worker each hold their own in-memory mock, so mock deployments appear only in the worker's log. Slice 2 done: read-only preflight (`GET /api/v1/preflight`): Incus ≥ 7.0, kernel ≥ 6.12, required API extensions (`instance_create_start`, `operation_wait`; `instance_oci` for OCI), member status, deploy projects exist and are restricted, a ready storage pool and managed network. Clock drift and member latency are reported as not checked: the Incus API cannot measure them. Slice 3 done: image pinning and architecture checks. A simplestreams image is pinned by its Incus fingerprint (sent as `source.fingerprint`, which Incus prefers to the alias), an OCI image by a digest in the alias (`repository@sha256:…`); `architectures` lists what the image supports. Before creating, the worker refuses a spec no cluster member can run (retries while suitable members are offline); afterwards it compares Incus's `volatile.base_image` and architecture with the spec and refuses, without changing anything, an existing instance that does not match.

**REAL INCUS TESTED (2026-10-20, Incus 7.0.1, one-member cluster on Debian 13 arm64, [test cluster](TEST-CLUSTER.md)):** automated in `services/deployment-worker/tests/real_incus.rs` — system container, OCI container (by tag and by digest) and virtual machine created, started, stopped, started and deleted through the job queue; re-creates and repeated deletes converge without Incus calls; a new instance created from a fingerprint pin is built from exactly that image; an existing instance on the wrong architecture is refused; a spec no member can run is refused before anything is created; an instance Apelora did not create is refused (stop and delete) and left running. Manual run the same day: members, restricted project and passing preflight through the API; the lifecycle through the HTTP API and a separately running worker with a token (`Idempotency-Key` replay, `default` project refused, every request audited, operation ids in the worker log), the Fabric Console showing the real instances in a browser, and no secrets in logs or audit. Only unit-tested: retrying while suitable members are offline, and refusing aliases in mixed-architecture clusters (one member, one architecture). The real cluster found two defects the contract tests had not: an empty profile list removed the `default` profile (fixed: omitted when empty) and an X.509 v1 client certificate (fixed in the setup script; the adapter now names the problem).

Remaining: bootstrap/join with short-lived tokens and operator approval (needs a second member), catalog runtime fields, a local-development storage profile, and a run on 7.0.2 itself.

## 05 — Fabric Console

`apps/fabric-console` using the Studio's visual language (white shell, daisyUI tokens, no gradients). Views: overview, cluster and members (roles, groups, failure domains), instances, projects/tenants, networks, storage, catalog/images/profiles, operations, application health, administration (users/roles, Incus TLS status, upstream versions, audit). Only fields the API actually provides; unknown values shown as unavailable; mock data labelled; destructive actions confirmed and permission-checked; controls disabled during conflicting operations.
**Accept:** browser tests and API contract tests; every action goes through the Rust API.

## 06 — Studio deployment integration

Appliance classes gain runtime, image digest, resources, volumes, probes, secret refs, availability policy. Deployment flow: save graph revision → validate (types, digests, quotas, storage, network, capacity) → plan with candidates and warnings → designer approves → worker applies through the adapter → instance start **and** readiness checks → overlay live state on the saved graph without rewriting it. Deploy panel: runtime, CPU, memory, replicas, exposure.
**Demo:** web (2 replicas) → API (2 replicas) → PostgreSQL (1, recovery `manual-until-safe`), with warnings for single points of failure.
**Accept:** import/export round-trip, type validation, plan preview, instance mapping; a real LXC + OCI graph deployment on the test cluster when available; mock paths labelled.

## 07 — Network fabric, service discovery, publishing

Network intent compiler (graph → intents → diff → apply), stable service names mapped to healthy replicas, deny-by-default ACLs per project, MVP VLAN profile then optional OVN profile (networks, ACLs, DNS zones, forwards, load balancers). Publishing: shared HTTPS ingress (default), private, dedicated address, internal name; domains and TLS certificates; DNS integration.
**Accept:** compiled-intent snapshot tests; isolation tests; on the test cluster, a consumer reaches a service by name across members and keeps working when a replica is replaced.

## 08 — Coordination, HA and mobility

PostgreSQL leases/generations; separate member/instance/service/ingress/storage health with hysteresis, grace periods, cooldowns; stateless replica policy with anti-affinity via verified Incus mechanisms; replace-then-retire relocation; policy-aware evacuation with dry run and per-item reasons (M1), replica-aware recovery (M2), advanced HA with tested fencing and shared storage (M3). Stateful single writers blocked on ambiguity. One recovery authority per workload; Incus healing stays off unless enabled deliberately.
**Accept (disposable lab):** stop one stateless replica → ingress reroutes and the desired count is restored without duplicates; ambiguous database/host failure → alert and block, not a second writer; worker restart and duplicate requests handled; all actions audited.

## 09 — Bare-metal provisioning and node enrolment (after ADR 0002 is accepted)

`maas-adapter` (discovery, commissioning, deployment, power), `node-enrollment` (approval, preflight, cloud-init/preseed join with single-use token, validation). Physical servers page with lifecycle and power actions; evacuation offered before power-off. Separate provisioning, management, storage and BMC networks.
**Accept:** a test server is discovered, approved, installed, joined and becomes eligible for placement, with every step audited.

## 10 — CI/CD, security hardening, releases, runbooks

Path-aware GitHub Actions: Rust fmt/clippy/test with `--locked`, npm typecheck/lint/test/build, Playwright, OpenAPI compatibility, dependency audit and licence scan (`cargo deny` with our own allow-list of AGPL-compatible licences and the advisory database; the npm equivalent for JS), secret scanning, upstream checks. CI never holds production Incus credentials or runs destructive tests on pull requests. Independent artefacts for each app and service, pinned dependencies, staged upgrades and rollback. Runbooks: member join/removal, quorum loss, instance recovery, stateful restore, rolling upgrades, certificates, API outage, storage and network incidents, access revocation, security incidents.
**Accept:** end-to-end demonstration on a 3-member lab with logs, plus a final status matrix — implemented, mock-tested, real-tested, not implemented, known limitations.
