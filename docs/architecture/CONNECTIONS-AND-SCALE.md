# Connections between layers, and how far Apelora scales

Status: **reference** · Date: 2026-10-21 · Decision: [ADR 0008 — sectors](../adr/0008-sectors.md) · Layers: [platform architecture](PLATFORM-ARCHITECTURE.md)

This page lists every connection between the layers of an Apelora installation — who opens it, over what, how it is authenticated and what happens when it breaks — and then how far each layer scales, with the source of each number. Numbers marked *estimate* are ours; the others come from the upstream projects.

## 1. What runs where

| Layer | Runs on | Instances of it |
|---|---|---|
| Fabric Console, Studio | the person's browser (static files served next to the API) | any |
| Control plane (API) | one or more Linux hosts or containers outside the clusters it manages | 1 today; several behind a load balancer once the per-process state below is shared |
| Deployment worker | same | several (jobs are claimed with leases) — except the replica supervisor, see §4 |
| PostgreSQL | a managed or self-run PostgreSQL | one primary, optional replicas |
| Incus | every **physical server** in a cluster (a *member*) | one cluster per **sector** (ADR 0008) |
| Modules | Incus instances on the members: **LXC system containers by default**, OCI application containers where the module class asks for one, **virtual machines only where needed** (another kernel or OS, kernel modules, live migration) | replicas of one module are separate instances, usually on different members |
| OVN (when used) | central databases on members of the same cluster; `ovn-controller` on every member | one OVN deployment per sector |
| Shared storage (when used) | a SAN (iSCSI, Fibre Channel or NVMe over Fabrics) reachable from every member of the sector | one shared volume group per pool |

In the disposable test cluster, Lima virtual machines stand in for the physical servers; nothing in Apelora depends on that.

## 2. Every connection

![Connections between the layers](../assets/diagrams/connections.svg)

| # | From → to | Protocol and port | Authentication | Carries | When it breaks | Status |
|---|---|---|---|---|---|---|
| 1 | Browser → Console / Studio | HTTPS 443 | none (static files) | the applications, under a content security policy that allows only their own origin (`infra/serve/`; proven in a browser by `tests/e2e/web-serving.spec.ts`); nothing is loaded from elsewhere | nothing to sign in to | built |
| 2 | Browser → control plane | HTTPS 443, `/api/v1` (same origin): TLS terminated by the control plane (`APELORA_TLS_CERT`/`KEY`) or by a declared proxy (`APELORA_BEHIND_PROXY=1`); plain HTTP only on loopback | bearer token: identity-provider access token, Apelora session (`apu_`), service token (`aps_`) or API token (`apl_`); public routes and refused tokens are rate-limited per client address, writes per caller (`429` + `Retry-After`) | every read and change; terminal websockets | the UI shows the control plane as down and retries | built |
| 3 | Browser → identity provider | HTTPS 443 | the person signs in there; PKCE, `state`, `nonce` | authorization code, token exchange (cross-origin) | sign-in with that provider fails with the provider's reason; other ways in keep working | built, real-browser tested |
| 4 | Control plane → identity provider | HTTPS 443 | none (public discovery and keys) | discovery document, signing keys (cached 60 s) | that provider's tokens are refused; retried every minute | built |
| 5 | Control plane → directory | LDAPS 636, or LDAP 389 with StartTLS | binds as the person signing in | that person's entry and groups | 503 `directory_unreachable`; other ways in keep working | being built (ADR 0007) |
| 6 | Control plane ↔ PostgreSQL | PostgreSQL 5432 (TLS recommended) | database user | all Apelora state: designs, desired state, jobs, users, audit | 503 `store_unavailable`; reads that need Incus only keep working | built |
| 7 | Worker ↔ PostgreSQL | PostgreSQL 5432 | database user | job claims with leases and fencing generations, schedules, health records, the supervisor's leader lease; one extra long-lived connection per worker `LISTEN`s on `apelora_jobs` | jobs wait; a claimed job's lease expires and another worker takes it; the leader lease expires and another worker supervises; a lost listener is re-established and the 2 s check covers the gap | built |
| 8 | Control plane → Incus API (per sector) | HTTPS 8443, mutual TLS | a client certificate **restricted to the deploy projects**; a second, unrestricted certificate only for cluster administration (join tokens, evacuation) — ADR 0003 | inventory, plans, previews, terminal sessions (websocket) | the Console says Incus is unreachable and shows the error, never stale data as current | built, REAL INCUS TESTED |
| 9 | Worker → Incus API (per sector) | HTTPS 8443, mutual TLS | the same restricted certificate | creating, starting, stopping, moving instances; volumes, snapshots, copies; waiting on operations | the job fails and is retried with backoff; nothing is assumed done | built, REAL INCUS TESTED |
| 10 | Incus member ↔ member | HTTPS 8443, mutual TLS (cluster certificate) | cluster membership | the replicated cluster database (Raft), API forwarding, instance and volume migration | a member that misses heartbeats for `cluster.offline_threshold` (default 20 s) is marked offline | Incus |
| 11 | `ovn-controller` on each member → OVN southbound database; Incus → OVN northbound database | TCP 6642 / 6641 with TLS | OVN certificates | logical networks, ACLs, load balancers | existing flows keep forwarding; changes wait | lab (`scripts/test-cluster/ovn.sh`) |
| 12 | Member ↔ member overlay | Geneve, UDP 6081 | none on the wire (keep it on a private network) | traffic between instances of one application network on different members | instances on that member lose their application network | lab |
| 13 | Clients → published services | the service's port on an ingress address | the application's own | requests to an application's gateway (OVN load balancer or network forward) | the address answers from the remaining replicas | built |
| 14 | Instance ↔ instance | the module's ports on its service address | the application's own | calls between modules; ACLs allow only the connections drawn in the Studio | refused by the ACL if not drawn | built |
| 15 | Member → SAN | iSCSI 3260 (lab), or Fibre Channel / NVMe over Fabrics | the SAN's own (initiator names, CHAP) — the lab target has none | blocks of the `shared` pool; locks live **on the LUN** (sanlock), no lock server | instances on `shared` stop on that member; with fencing they restart elsewhere (HA slice B) | lab, REAL INCUS TESTED (pool created) |
| 16 | Member → member (volume copies, backups) | through Incus, connection 10 | cluster | volume copies for moves and backups to the `backup` pool on another member | the backup job fails and is retried | built, REAL INCUS TESTED |
| 17 | Control plane → bare-metal provisioner; provisioner → server management controllers | HTTPS; IPMI / Redfish | API key; controller credentials in the provisioner | installing and powering servers | servers are added by hand | planned (ADR 0002) |
| 20 | Collector → control plane `GET /metrics`; collector → worker `GET /metrics` | HTTPS 443 (the API address); worker HTTP on `127.0.0.1:9471` or a management address; optionally a separate control-plane listener (`APELORA_METRICS_ADDR`) | API: a service identity's token with `fabric.metrics.read` (role Metrics Reader). Listeners: none, so loopback or a management network only | Apelora's own metrics: requests, auth, jobs, supervisor, Incus calls, database pool, inventory counts ([monitoring](MONITORING.md)) | gaps in the collector's data; Apelora keeps working (nothing is pushed) | built (MOCK VERIFIED) |
| 21 | Collector → every Incus server `/1.0/metrics` | HTTPS on `core.metrics_address` (for example 8444), mutual TLS | a certificate trusted with type `metrics` (reads metrics only) | CPU, memory, disk, network per instance (only that server's instances); member status, warnings, operations, pool use | gaps for that server; nothing else is affected | documented (Incus) |

The collector runs inside the installation; Apelora never sends metrics anywhere (connections 20 and 21 are opened by the collector).

**Never:** browsers do not reach Incus, OVN, PostgreSQL, the SAN or the directory; instances do not reach the Incus API (no `security.nesting` for tenants, no API socket in instances); members do not reach Apelora (the control plane always calls in). Each sector is reached only through connections 8 and 9.

## 3. How far each layer scales

| Layer | Limit | Source | What happens near the limit | What Apelora does |
|---|---|---|---|---|
| Members per Incus cluster | **recommended at most about 50** (roughly one rack); clusters of 100–150 servers run in practice and 250 has been tested; no hard limit | Incus maintainers ([clustering](https://linuxcontainers.org/incus/docs/main/explanation/clustering/), [forum: cluster size](https://discuss.linuxcontainers.org/t/how-many-nodes-can-an-incus-cluster-support/21177), [forum: largest clusters](https://discuss.linuxcontainers.org/t/largest-incus-cluster-in-the-wild-general-scaling-pain-points/26242)) | the cluster database's write rate (operations in flight, not running instances) and the blast radius: one control-plane fault stops the whole cluster's API | keep a sector at or under 50 members; warn above it; more servers means more sectors (ADR 0008) |
| Distance between members | network latency **at most 10 ms**; synchronised clocks | Incus clustering docs | Raft and heartbeats fail | one sector = one site (one data centre, or rooms within 10 ms) |
| Cluster database replicas | 3 voters + 2 stand-bys by default (`cluster.max_voters`, `cluster.max_standby`); the cluster stays up while a majority of voters is online | Incus clustering docs | — | at least 3 members per sector; spread voters across failure domains |
| OVN | one OVN deployment per sector, so at most the sector's members as chassis; OVN has run with about 700 hypervisors in one deployment | [OVN architecture](https://www.man7.org/linux/man-pages/man7/ovn-architecture.7.html); [700-node report](https://lists.openstack.org/pipermail/openstack-discuss/2020-August/016851.html) | southbound database load grows with chassis and flows | a sector is far below OVN's range; application networks do not span sectors (§5) |
| Shared volume group (sanlock) | **2,000 hosts** per lockspace (`host_id` 1–2000 with the default 1 MiB alignment) | [lvmlockd(8)](https://man7.org/linux/man-pages/man8/lvmlockd.8.html), sanlock | — | never the limit inside a sector; `host_id` = member number |
| Instances per member | hardware: hundreds of containers or tens of VMs per server, by memory | *estimate* | memory and disk fill | capacity views (Health page) and placement refuse what does not fit |
| Apelora control plane | stateless API apart from the items in §4; PostgreSQL holds everything | *estimate*: 20 sectors × 50 members × 50 instances = 50,000 instances; status polling one list per sector every 30 s ≈ 1 request/s; a few writes per deployment | PostgreSQL size and the audit log grow with use (append-only) | one primary is enough for this; partition or archive the audit log by month when it reaches tens of millions of rows |
| Deployment workers | as many as needed; jobs are claimed with `FOR UPDATE SKIP LOCKED`, leases and fencing | code (`claim_job`) | Incus operation rate per sector | run 2+ workers for availability once the supervisor has a leader (§4) |

## 4. Scaling issues found, and the fix for each

| Issue | Effect today | Fix | When |
|---|---|---|---|
| Sign-in settings are reloaded only in the API process that saved them | with two API processes, the other one keeps old providers until it restarts | PostgreSQL `LISTEN/NOTIFY` on a `sign_in_settings` channel; every process reloads | before running two API processes |
| Terminal-session tickets, the per-address rate limits (`api/rate_limit.rs`: public routes, refused tokens) and the per-caller write limit live in process memory | a ticket made by one API process is unknown to another; each limit is per process, so N processes allow N times as much | sticky routing for `/sessions` websockets, or keep tickets in PostgreSQL; the limits stay per process × N (acceptable: divide the configured rates by N) or move to PostgreSQL | before running two API processes |
| The public `/health` cache lives in process memory | each API process asks Incus at most every 15 s for `/health` (N processes: N probes per 15 s) | nothing needed at this scale | — |
| The replica supervisor runs in every worker | two workers could both restart or replace the same failed instance | a leader lease in PostgreSQL (one row, renewed every few seconds, fenced by a generation like jobs) | **fixed** (2026-10-21, MOCK VERIFIED with a real PostgreSQL): `leader_leases`, lease 15 s renewed every 5 s, every supervisor write fenced; workers are also woken by `LISTEN/NOTIFY` instead of only polling |
| One Incus endpoint per installation (`APELORA_INCUS_URL`) | one sector only | sectors (ADR 0008): a `sectors` table, one adapter per sector, `sector` on applications and placement | phase after HA |
| The audit log only grows | slower queries after years | monthly partitions; export and archive old ones | when it reaches tens of millions of rows |

## 5. Beyond one cluster: sectors linked into one fabric

A **sector** is one Incus cluster: one site, at most about 50 servers, its own cluster database, its own OVN and its own storage pools. The **fabric** is all sectors together. Decision and details: [ADR 0008](../adr/0008-sectors.md). Past a few hundred servers, or across countries and continents, sectors are grouped into sites and regions, each region with its own control plane: [global fabric](GLOBAL-FABRIC.md) and [ADR 0009](../adr/0009-global-fabric.md).

![Sectors linked into one fabric](../assets/diagrams/sectors.svg)

| Link | Connection | What it gives | Needs |
|---|---|---|---|
| Management | 8 and 9, once per sector | one Console, one audit log, one set of people and roles; inventory, health and capacity across sectors; placement chooses a sector | the sector's Incus endpoint and its restricted and administrative certificates |
| Network | 18 — OVN interconnection between the sectors' OVN deployments (TCP 6645/6646 to the interconnection databases; Geneve between gateway members) | a module in one sector calls an application in another over a peered, routed network; ACLs still allow only what was drawn | an interconnection database shared by the linked sectors, gateway members, one certificate authority for OVN, non-overlapping subnets (Apelora allocates them per sector) |
| Data | 19 — Incus migration between clusters (HTTPS 8443) | copy or move instances and volumes between sectors, as a job with a dry run | pools, networks and profiles with the same names in the target sector |
| Recovery | 19 — backup copies to another sector | redeploy a lost sector's applications elsewhere from desired state and the latest backups | a `backup` pool in the other sector |

- An application is placed in one sector; its replicas stay inside it (they share an application network and, for stateful modules, storage).
- For availability across sites, the same application is deployed to two sectors and published behind one address (DNS or an external load balancer); data is replicated by the application (database replication), not by Apelora.
- People see one Console: servers, health and capacity are grouped by sector; the Studio asks for a sector only when there is more than one.

Connections added by linking (to the table in §2):

| # | From → to | Protocol and port | Authentication | Carries | When it breaks | Status |
|---|---|---|---|---|---|---|
| 18 | Each sector's OVN ↔ interconnection databases; gateway member ↔ gateway member | TCP 6645/6646 with TLS; Geneve UDP 6081 | one certificate authority shared by the linked OVN deployments | routes between peered application networks | calls between sectors fail; calls inside each sector keep working | planned |
| 19 | Sector ↔ sector (Incus migration) | HTTPS 8443, mutual TLS | a short-lived trust created by Apelora for the copy, removed afterwards | instance and volume data for cross-sector moves and backups | the job fails and is retried; the source is untouched until the copy is verified | planned |
