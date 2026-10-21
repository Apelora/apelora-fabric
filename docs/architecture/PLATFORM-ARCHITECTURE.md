# Apelora platform architecture (target)

Status: **target design** — describes where we are going. Much of it now runs on a disposable test cluster. Date: 2026-10-20, runtime and layer mapping corrected 2026-10-21. Decisions: [ADR 0001 Incus-first](../adr/0001-incus-first.md) (accepted), [ADR 0002 MAAS](../adr/0002-bare-metal-provisioning-maas.md) (proposed). Plan: [implementation phases](../development/IMPLEMENTATION-PHASES.md). Today's state: [current-state audit](current-state.md).

## 1. Goal

A user composes an application from small modules in **Apelora Studio**, connects typed terminals, chooses resources, replicas and exposure, reviews a deployment plan and deploys it onto a cluster of Linux servers. Administrators run the servers, networks, storage, tenants and failures in **Apelora Fabric Console**. Apelora adds the application layer; it does not re-implement cluster management, packet forwarding or hardware provisioning.

## 2. Three management levels, one authority each

| Level | Authority | Responsibility |
|---|---|---|
| Application | **Apelora** (Rust control plane + PostgreSQL) | Module classes, application graphs, desired state, connectivity intents, service names, publishing, replicas, health, mobility, recovery policy, audit |
| Compute | **Incus** cluster | Membership, quorum (Raft-replicated database), instance placement and lifecycle, projects, profiles, images, storage and network objects, evacuation |
| Network dataplane | **OVN**, managed through Incus (one network per application, built), over the physical network that carries the uplink | Forwarding, logical networks, ACL enforcement, load balancing |
| Bare metal (proposed) | **MAAS** | Discovery, network boot, commissioning, OS install, power control |

Rules: one owner per piece of state; Apelora observes the others and acts only through their APIs; nothing writes another system's database.

## 3. Layers

![Apelora on Incus, layer by layer](../assets/diagrams/platform-layers.svg)


```text
 LAYER 1  USER INTERFACES (React + TypeScript, independent builds)
 +------------------------------+  +------------------------------+
 | apps/fabric-console          |  | apps/studio                  |
 | servers, members, instances, |  | catalog, React Flow graph,   |
 | networks, storage, tenants,  |  | typed terminals, replicas,   |
 | operations, health, RBAC     |  | exposure, plan, live status  |
 +--------------+---------------+  +---------------+--------------+
                +---------------+------------------+
                                | HTTPS /api/v1 (OpenAPI)
                                v
 LAYER 2  APELORA CONTROL PLANE (Rust)
 +------------------------------------------------------------------+
 | services/control-plane   Axum API, authn/RBAC, catalog, graphs,  |
 |                          validation, plans, operations, audit    |
 | services/deployment-worker  reconciler, jobs, health, recovery   |
 |   libraries: domain, persistence, auth, orchestration,           |
 |   network-fabric, service-discovery, coordination, placement,    |
 |   health, mobility, recovery, storage-policy,                    |
 |   incus-adapter, (maas-adapter, node-enrollment)                 |
 +------------------------+-------------------------+---------------+
                          |                         |
              PostgreSQL (Apelora state)      Incus REST API (mTLS)
                                                    |
 LAYER 3  INCUS CLUSTER  (Raft DB, members, placement, instances)
 +------------------+   +------------------+   +------------------+
 | member 1         |   | member 2         |   | member 3         |
 | Linux + Incus    |   | Linux + Incus    |   | Linux + Incus    |
 | shop-web shop-db |   | shop-web-r2      |   | shop-web-r3      |
 +------------------+   +------------------+   +------------------+
      \____ one OVN network per application: ACLs, service addresses ____/
 LAYER 4  INFRASTRUCTURE: physical servers, switches, the uplink network;
          storage pools (local per member, backup; shared on a SAN, unused yet),
          service address range, DNS, PKI, metrics
```

The network fabric, coordination and HA engines are **libraries inside the two Rust services**, not separate products. In Apelora a server is an Incus cluster member, called a server (or member) in the UI; there is no extra Apelora host agent unless an ADR introduces one.

## 4. Modules and runtimes

A module class says how it runs in `runtime.kind` (`packages/assembly-model/src/schema.ts`; turned into an Incus request by `instanceRequestFor` in `runtime.ts`). A class without a runtime is design-only.

| Runtime (`runtime.kind`) | Incus instance | Use | Catalog classes today |
|---|---|---|---|
| `incus-system-container` | LXC system container | The default: web servers, APIs, workers, caches, databases | Debian |
| `incus-oci-container` | OCI application container | Where the class ships as a single-process image, pinned by digest | NGINX |
| `incus-vm` | QEMU virtual machine | Only where needed: another kernel or operating system (Windows, `guestOs`), kernel modules, firewall-style modules, or live migration (`migration.stateful`) | Linux base (a VM so it can move while running) |

A component (a class with `composition`) is not a runtime: it is a Studio grouping whose parts are deployed in a later version; today it stays in the design. A Studio node is a *logical* module; its replicas are separate Incus instances mapped by stable ids. Saved graphs never contain host names or IP addresses.

### 4.1 From an application to running instances

The whole chain in one place; other documents link here.

| Step | What it is | Names and example | Decided by |
|---|---|---|---|
| Application | A design deployed under a name, into one Incus project | `shop` (from the design's name) in project `apelora` | The person deploying |
| Module | A node on the canvas: one use of a module class (`ModuleInstance` in the model, not an Incus instance) | `web`, class NGINX | The design |
| Replicas | Stateless classes only (`availability: stateless`): 1–9 interchangeable instances. Singleton and stateful classes run one | `shop-web`, `shop-web-r2`, `shop-web-r3`; a stand-in made while a replica's member is down gets `-s1` (`shop-web-r2-s1`) | The design, within the class's availability (`replicasOf`) |
| Instance kind | From the class's runtime, never from availability: system container by default, OCI application container or VM where the class says | NGINX → OCI container; Debian → system container; Linux base → VM | The module class |
| Member | The server Incus places the instance on: a physical server in production, a Lima VM standing in for one in the lab. Apelora's placement scriptlet (`infra/incus/placement.star`) keeps replicas of a module apart | `apelora-test-1` … `-4` in the lab | Incus, steered by the scriptlet; a person for a planned move (`instance.move`) |
| Storage | Root disk from the project's default profile on `local`. Each volume is `<instance>-<volume>` on the pool in `APELORA_VOLUME_POOL` (`local`, btrfs in the lab), on the instance's member, attached as `vol-<volume>`. Backups are copies in `backup` (dir), on another member where there is one. `shared` (clustered LVM on a SAN) exists in the lab, but Apelora places nothing on it yet | `shop-db-data` | The class or the design (volume name, size, path); the administrator (pools) |
| Network | One OVN network per application with one ACL and one address set per module; each drawn connection becomes an allow rule; a stateless module others connect to, or a published one, gets a service address (OVN load balancer); a published port is open from outside. A bridge (`incusbr0`) is only the lab's uplink | Consumers read `APELORA_SERVICE_WEB` | The design's cables, link tags and gateway; the worker's `network.sync` |

Replica count comes from availability, not from the instance kind: a VM class marked stateless could have replicas; a system container class marked singleton cannot. Details: [application deployment](../design/application-deployment.md), [service discovery](../adr/0004-service-discovery.md), [storage](STORAGE.md), [moving modules](../design/mobility-and-ha.md).

## 5. Virtual network fabric

![Networking for modules, layer by layer](../assets/diagrams/network-options.svg)

**A connection is a service dependency plus an authorisation**, compiled into network intents:

```text
Studio:   web.api ──http──> backend.http
Intent:   source=web target=backend proto=tcp port=8080 access=allow
          discovery=backend.<app>.internal
Apply:    resolve backend replicas → ensure endpoint → apply ACLs →
          configure consumer → check connectivity/readiness
Runtime:  web-01, web-02 ──> backend.<app>.internal:8080 ──> api-01, api-02
```

| Studio element | Becomes |
|---|---|
| Module | Incus instance(s), one per replica ([§4.1](#41-from-an-application-to-running-instances)) |
| Input terminal | Service endpoint |
| Output terminal | Dependency |
| Connection | Allowed traffic + name resolution |
| Public terminal | Ingress endpoint |
| Volume connection | Persistent volume attachment |

- **Stable service names** (Apelora-managed) map to healthy replicas, so replacing or moving an instance never breaks consumers.
- **Deterministic compile → diff → apply**, deny by default, scoped by Incus project.
- **One OVN network per application** (built, REAL INCUS TESTED): Incus-managed OVN with an ACL and an address set per module, health-checked load balancers for service addresses, and egress to `@external` for internet access. OVN runs its own services and needs an uplink network that spans the members; in the lab a bridge (`incusbr0`) plays that uplink. DNS zones and forwards are not used yet.

### Publishing applications

| Exposure | Reachable by | Use |
|---|---|---|
| Internal service name | Connected modules | Front end → API |
| Private address | Authorised internal networks | API, databases, caches |
| Shared public HTTPS (default) | Internet, via domain + TLS on shared ingress | Most web apps |
| Dedicated public address | Internet, own address | Special TCP services |
| Private VPN endpoint | VPN users | Internal business apps |

Ingress is a layer-7 proxy for domains and TLS in front of health-checked backends (OVN load balancers or the proxy's own checks). Several replicas behind a single unreplicated ingress is **not** HA.

**Address authority:** provisioning/host addresses — MAAS or site IPAM; container addresses — Incus/OVN; service endpoints — Apelora ingress controller; public addresses — router/provider pool; DNS names — Apelora's DNS integration. Apelora displays all of them and allocates none it does not own.

## 6. Coordination

| Responsibility | Owner | Mechanism |
|---|---|---|
| Cluster membership, leader, instance state | Incus | Its Raft-replicated database |
| Network forwarding state | OVN | Its own databases |
| Deployments, replicas, operations, leases | Apelora | PostgreSQL |
| Health and readiness decisions | Apelora | Health controller |
| Storage ownership | Storage system + policy | Backend + fencing |

- API instances are stateless; the operation queue, leases and generations live in PostgreSQL. Workers claim work with row locks (`FOR UPDATE SKIP LOCKED`) or expiring leases; one reconciliation flow per application at a time; a generation number stops a stale worker from committing old decisions; a recovering worker reads actual Incus state before retrying.
- Locks alone do not prevent split-brain against external APIs: also idempotency keys, Incus operation ids, ownership checks, fencing.
- Control-plane HA needs a redundant entrypoint **and** a tested PostgreSQL failover plan. MVP: one primary with tested backups. No separate consensus service until a measured need.
- MAAS and the control plane run on management infrastructure that survives the loss of the compute cluster.

## 7. Health, HA and mobility

Separate detectors, each with source, timestamp and freshness: member (Incus heartbeat/quorum), instance (Incus state), service (readiness/liveness probes), ingress, storage. **Running ≠ ready ≠ available ≠ recoverable.**

| Workload | Default replicas | Recovery |
|---|---|---|
| Stateless HTTP / API | 2+ on different eligible members | Health-gated routing; replace failed replicas |
| Worker | By queue semantics | Restart/replace; idempotent jobs |
| Cache | 1 unless replicated by design | Recreate |
| Database (single writer) | 1 | **Blocked** on ambiguous failure until fencing + storage ownership proven; operator decides |
| Replicated database | By design | Application-specific failover adapter |

Replicas follow the class's availability, not its instance kind: a VM is not limited to one, and a container is not stateless by default. A VM with live migration on moves running in planned moves (`instance.move`, evacuation); after a member failure, only instances on shared storage can restart elsewhere, after fencing ([moving modules](../design/mobility-and-ha.md), slice B, not built).

**Relocating stateless modules (replace, then retire):** plan (policy + capacity) → start replacement on another member → wait for readiness → switch the service name's routing → drain and retire the old instance. No container live migration is needed; downtime depends on replica count, draining, sessions and ingress, and is measured, not promised.

**Stateful single instances:** planned stop-and-move with storage checks and an announced interruption. Ordinary LXC live migration is limited and is never assumed.

**Policy-aware evacuation (Mobility & HA engine):** discover workloads on a member → classify (stateless, replicated, singleton, stateful) → check capacity, anti-affinity and minimum-healthy rules → dry-run plan with a reason for every blocked item → stop new placements → drain → relocate one by one with verification → maintenance only when all checks pass → controlled restore (Incus can restore a member without moving instances back). It refuses to remove the last healthy instance of a service whose policy requires continuous service.

**Two recovery modes, mutually exclusive per workload:** native Incus healing (Apelora observes) **or** Apelora-managed recovery (Apelora coordinates fencing and replacement). Never both. Incus healing (`cluster.healing_threshold`) stays at its default (disabled) unless an operator enables it deliberately.

**Placement:** Incus places instances (by default favouring members with fewer instances). Apelora adds replica anti-affinity and failure domains through Incus cluster groups, targets and placement scriptlets once verified — never a competing scheduler.

Milestones: **M1** intelligent evacuation (plan, dry run, drain, move, verify, restore); **M2** replica-aware recovery; **M3** advanced HA (tested fencing, stateful recovery, shared storage, automatic policies).

## 8. Storage

![Where module data can live](../assets/diagrams/storage-options.svg)

Details — how Incus models pools and volumes, local versus shared storage, the volume-class abstraction and its lifecycle — are in [storage](STORAGE.md).

- Today: a local pool on each member (btrfs `local` in the lab) for root disks and `data-local` volumes; snapshots, scheduled snapshots and backups copied to a `backup` pool on another member; no cross-member stateful recovery promises.
- HA: a shared pool that every member reaches, operated deliberately with its own monitoring: clustered LVM (`lvmcluster`) on a SAN, or distributed storage that Incus supports (e.g. Ceph RBD, LINSTOR). The lab has a `shared` lvmcluster pool on an iSCSI LUN (created 2026-10-21, REAL INCUS TESTED on all four members); Apelora does not place anything on it yet. These are external systems that Incus talks to; Apelora does not link or vendor them (Ceph is mainly LGPL), so their licences do not affect our code.
- Volume classes: ephemeral root, persistent exclusive, shared read-only, backup, secret — each with ownership, relocation policy and RPO/RTO. Snapshots are not backups; restores are tested.

## 9. Bare metal (proposed, ADR 0002)

New server → network boot → MAAS commissioning (CPU, RAM, disks, NICs, BMC) → "new server found" in Fabric → **administrator approval** → OS install + cloud-init → preflight → single-use Incus join token → validation → eligible for placement. Power on/off/reboot through MAAS; powering off a member with workloads first proposes an evacuation; emergency power-off is separately permissioned. Networks: provisioning, management, storage and BMC separated (VLANs or equivalent); MAAS DHCP never on a network that already has one without coordination.

## 10. Security and tenancy

Details — roles, scopes, the request path and best practice for every layer — are in [access and roles](ACCESS-AND-ROLES.md) and [ADR 0005](../adr/0005-access-model.md) (proposed).

- Roles: Fabric admin, operator, Studio designer, auditor; every endpoint authorised server-side and scoped by project. Designers cannot change physical infrastructure.
- Incus over mTLS with server verification on a management network; client certificates and BMC credentials only in the backend's secret store — never in the browser, logs or repository.
- Restricted Incus projects, unprivileged containers, no nesting/raw config/host mounts/devices for tenants; image provenance by digest.
- Destructive or risky actions (delete, member removal, storage/network changes, enabling healing, power-off) need confirmation and are audited.

## 10a. Applications as a first-class object

The Studio opens on an **Applications** view (list, create, open a canvas) backed by server-side drafts; see [applications view](../design/applications-view.md).

## 11. Upstream

Incus and (if adopted) MAAS are installed as system packages and tracked in `upstream.lock.yaml`: URL, licence, intended release, verified version/commit, required API extensions, tested host compatibility, verification date. Update checks open reviewable pull requests; nothing upgrades production automatically. An optional read-only source checkout (git-ignored) is for reading only and is never compiled into Apelora.

Contributing back to Incus follows its own rules: contributions must be Apache-2.0 compatible (none of our AGPL code), carry a human author's DCO sign-off and must not credit AI tools; Incus also asks that AI tooling not run on machines holding its repository credentials.

## 12. Non-goals (for now)

A container orchestrator; writing a hypervisor, cluster manager, scheduler, storage engine or distributed filesystem; container live-migration claims; automatic split-brain-prone stateful failover; arbitrary privileged host commands from users; unattended upstream upgrades; a separate consensus service.

## 13. Repository layout

```text
apps/            studio, fabric-console
packages/        assembly-model, module-catalog, api-client (generated from the API's OpenAPI document)
services/        control-plane (Axum API), deployment-worker (jobs, network sync, replica supervisor)
crates/          domain, persistence, incus-adapter, auth, identity; further crates are
                 created by the phase that needs them
infra/           dev/ (local PostgreSQL and identity provider), identity/, incus/ (placement scriptlet)
scripts/         test-cluster/ (disposable lab), enrol/, upstream/, ci/, security/, licenses/
docs/            adr/, architecture/, design/, development/, runbooks/, guides/, assets/
.agents/skills/  shared agent skills (Claude via .claude/skills symlinks, Codex natively)
```
