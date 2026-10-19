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

## 03 — Rust API, domain and Incus adapter

Domain types (cluster, member, role, instance and kind, project, profile, network, storage pool, image, appliance class/instance, application graph, deployment, operation, health evidence, role, audit event) with desired and observed state kept apart. `incus-adapter`: mTLS client, version discovery (`GET /`, `/1.0`), extension checks, read-only inventory (members, projects, instances, images, profiles, networks, storage pools, operations), typed errors (auth, connectivity, quota, capability, operation failure, unknown state), bounded timeouts, no blind retries of non-idempotent calls. Mock provider with fixtures. `control-plane`: `/api/v1`, auth middleware, project-scoped RBAC, OpenAPI → generated TS client. `persistence`: PostgreSQL migrations for Apelora state only. `deployment-worker`: durable jobs, leases, read-only mode.
Local development: PostgreSQL from a compose file bound to loopback on a non-standard port; SQLx offline query metadata committed so builds and CI need no database; a parallel test runner for the Rust suite.
**Accept:** contract tests against recorded Incus responses; read real member and instance inventory from an authorised test cluster (REAL INCUS TESTED) before any write operation exists.

## 04 — Cluster management and appliance deployment

Read-only host preflight (kernel ≥ 6.12, Incus version, clock drift, member latency ≤ 10 ms, DNS, TLS, pools, networks, project restrictions, capacity). Surface Incus bootstrap/join with short-lived tokens and operator approval. Instance create/start/stop/delete with async operation tracking for the three runtimes, image architecture and digest checks, restricted projects by default. Catalog runtime fields (see `appliance-catalog`). Local-development storage profile; cross-member networking only claimed after it is tested.
**Accept:** on the test cluster: members shown, project verified, disposable LXC and OCI instances created/started/stopped/deleted with operation ids, nothing unrelated touched, no secrets leaked.

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
