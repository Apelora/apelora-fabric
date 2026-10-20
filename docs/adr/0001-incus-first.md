# ADR 0001 — Incus-first runtime and control-plane boundaries

Status: **accepted** · Date: 2026-10-20 · Supersedes: an earlier hypervisor-based node-agent design (never built)

## Context

Apelora was planned as a Rust controller plus a Rust node agent driving a hypervisor directly, with every module a dedicated VM. Nothing of that backend was built: the repository contains the browser-only Studio (`apps/studio`) and two TypeScript packages. The plan carried three costs:

- **Scope.** We would have had to write host agents, cluster membership, placement, image handling, storage and network plumbing ourselves.
- **Weight.** Most catalog modules (web servers, APIs, caches, workers, databases) do not need their own kernel; a VM per module wastes memory and boot time.
- **Licensing.** That hypervisor is GPLv2-only, which cannot be combined with AGPLv3 code in one work, forcing a strict process boundary and legal review.

Incus 7.0 LTS (released May 2026, supported to June 2031) manages clusters of Linux hosts and runs LXC system containers, OCI application containers and QEMU virtual machines behind one documented REST API, with clustering (Raft-replicated database), projects, profiles, images, storage pools, networks (bridge and OVN), evacuation and metrics. It is Apache-2.0 licensed. Minimum host requirements include Linux kernel 6.12 and cgroup v2.

## Decision

1. **Incus is the execution and cluster engine.** Modules run as Incus instances: LXC system containers by default, OCI application containers for single-process images, QEMU VMs only where a container is unsuitable (other kernels or OSes, firewall-style modules, stronger isolation).
2. **Authority is split, never shared.**
   - Incus owns cluster membership, quorum and leader election, its own database, instance placement and instance lifecycle.
   - Apelora owns application graphs, module classes, desired state, typed terminals and the connectivity they imply, replica and availability policy, application health, operations history and audit.
   - Apelora never writes Incus's database and never implements a competing cluster manager or scheduler. Stronger placement uses Incus mechanisms (cluster groups, targets, placement scriptlets) once verified.
3. **Rust control plane.** `services/control-plane` (Axum REST API) and `services/deployment-worker` (durable reconciler), with libraries under `crates/`. Apelora's own state and job coordination live in **PostgreSQL** (row locks, leases, generation numbers). No separate consensus service is added unless a measured need appears.
4. **Integration only through the documented Incus REST API** over mutually authenticated TLS with server verification. Incus is installed as an ordinary system package on each host; we do not fork, vendor or rebuild it. The tracked version lives in `upstream.lock.yaml` (Phase 02).
5. **Two React apps, one API.** `apps/studio` (designer, kept) and `apps/fabric-console` (administrators, new) talk only to the Apelora API; the browser never sees Incus credentials.
6. **Safety defaults.** Restricted Incus projects, unprivileged containers, no host mounts or raw configuration for tenants; Incus automatic healing stays disabled unless an operator enables it deliberately; no automatic stateful failover without verified fencing and storage ownership.
7. **Honest results.** Every test result is labelled MOCK VERIFIED, CONTRACT VERIFIED or REAL INCUS TESTED; the UI keeps saying "Mock · Design only" until it shows real backend data.

## Consequences

- The Studio and its data model are unaffected; a module class gains an optional `runtime` when deployment is added (Phase 06).
- The earlier design's documents and skill were first marked superseded, then removed (2026-10-20, owner's decision); the commercial licence drafts were updated at the owner's explicit request.
- Licensing simplifies: Apache-2.0 Incus may be combined with AGPLv3 work, and we talk to it over an API anyway. Upstream notices are kept for anything redistributed.
- Hosts need a recent kernel (≥ 6.12). Live migration of ordinary LXC containers is limited; stateless modules move by replace-then-retire, stateful ones by planned stop-and-move (see `docs/architecture/PLATFORM-ARCHITECTURE.md`).
- A single Incus cluster's quorum is not application availability. Application HA (replicas, health-gated routing, ingress redundancy, storage and fencing) remains Apelora's job and must be proven by tests.

## Alternatives considered

| Option | Why not |
|---|---|
| Keep driving a hypervisor directly with our own node agent | Large amount of infrastructure code to write and maintain; VM-per-module overhead; GPLv2 boundary |
| A VM-cluster manager over a hypervisor | Still VM-centric; another daemon stack; little help for containers |
| A container orchestrator built around pods and custom resources | Large operational surface for a small cluster; its model differs from modules wired by typed terminals; explicitly out of scope |
| Per-host container runtime plus our own clustering | We would re-implement membership, placement and migration that Incus already provides |

## Reversibility

The Incus dependency sits behind `crates/incus-adapter` and provider traits in `crates/domain`. Studio documents contain no Incus-specific data. Replacing the runtime later means a new adapter, not a rewrite of the graph model or the UI.
