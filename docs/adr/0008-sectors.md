# ADR 0008 — Sectors: linking Incus clusters into one fabric

Status: **accepted. The survival decision is implemented and refuses while one site is connected. A sectors table, a second Incus adapter, access scope and the links are not built. Extended by [ADR 0009](0009-global-fabric.md) (regions, sites and a plane per region)** · Date: 2026-10-21 · Related: [ADR 0001](0001-incus-first.md), [connections and scale](../architecture/CONNECTIONS-AND-SCALE.md)

## Context

Apelora manages one Incus cluster today. The owner asked how many physical servers one installation can use. The Incus maintainers recommend at most about 50 servers per cluster (clusters of 100–150 run in practice; 250 has been tested), require at most 10 ms between members, and advise running several standalone clusters rather than one very large one: a single control-plane fault, or a bad database update, stops the whole cluster's API. The real limit is the cluster database's write rate, not the number of running instances.

So a large installation is several clusters, and Apelora needs a name and a model for them. *Zone* is taken (Studio zones), *pool* is taken (hardware pools in the access model), *region* suggests geography only.

## Decision

1. **A sector is one Incus cluster** that Apelora manages: one site (members within 10 ms), **at most 50 members by default** (configurable; Apelora warns at the limit and refuses to enrol beyond it unless an administrator raises it), its own cluster database, OVN deployment and storage pools.
2. **One Apelora control plane manages many sectors.** Each sector has its own Incus endpoint and client certificates (restricted and administrative, as in ADR 0003); connections 8 and 9 exist once per sector. A sector that is down affects only its own applications.
3. **An application lives in one sector.** Its replicas, application network and stateful storage stay inside it. Cross-site availability is two deployments of the same application in two sectors, published behind one address, with data replicated by the application.
4. **Sectors appear in the access model** as a scope under the platform (like hardware pools), so a team can be given operations on one sector only.
5. **The fabric is all sectors together.** Sectors are joined by four kinds of link, each optional and added in this order:
   1. **Management link** — the one control plane reaches every sector's Incus API (connections 8 and 9 per sector). One Console, one audit log, one set of people and roles; inventory, health and capacity are summed across sectors; placement chooses the sector.
   2. **Network link** — application networks in two sectors are peered at layer 3 through OVN interconnection (Incus *network integrations* and remote network peers). It needs an interconnection database shared by the linked sectors, at least one gateway member per sector, one certificate authority for the OVN databases, and **non-overlapping subnets**: Apelora allocates each sector its own ranges for application networks and service addresses. A connection drawn in the Studio from a module in one sector to an application in another becomes an *export* on one side and an allow rule on the other; ACLs still allow only what was drawn. Where interconnection is not available, the same connection goes to the other application's published address over the routed uplink.
   3. **Data link** — instances and volumes are copied or moved between sectors through Incus migration between clusters, as an Apelora job with a dry run (the target sector needs pools, networks and profiles with the same names; Apelora checks that first). A move across sectors stops the instance unless live migration between those two sectors has been tested; we do not claim it until then.
   4. **Recovery link** — backups are copied to the `backup` pool of another sector; when a whole sector is lost, an operator redeploys its applications in another sector from Apelora's desired state and the latest backups. Apelora shows what is out of date before it does anything.
6. **Sites group sectors** (for failure domains, latency and recovery targets) and are only a label. Workers can be assigned to the sectors near them (`APELORA_WORKER_SECTORS`), so job traffic stays local and a busy sector cannot starve another.

### How far that goes

| Step | Servers | Shape |
|---|---|---|
| One sector | 3–50 | one cluster, one rack; what is built today |
| Several sectors in one site | 50–500 | a sector per rack, linked by management and network links; one control plane (two or more API processes and workers) |
| Sectors across sites | 500–2,000+ | sites linked by management and recovery links (and network links where the WAN allows); applications that must survive a site run in two sectors in two sites |

These are *estimates*: the per-sector limit comes from Incus; above that the limits are Apelora's own (PostgreSQL and the job rate), see [connections and scale §3](../architecture/CONNECTIONS-AND-SCALE.md#3-how-far-each-layer-scales).

## Consequences

- Capacity grows by adding sectors: 20 sectors of 50 servers is 1,000 servers under one Console, each sector small enough to keep its blast radius to one rack.
- Placement first chooses a sector (by the application's setting, capacity and labels), then Incus places instances inside it.
- The Console groups servers, health and capacity by sector; the Studio asks for a sector only when there is more than one.
- Work needed: a `sectors` table and API, an adapter per sector, `sector` on applications, the scope in the access model, sector-aware health and capacity views, and the per-process state fixes in [connections and scale §4](../architecture/CONNECTIONS-AND-SCALE.md#4-scaling-issues-found-and-the-fix-for-each) so the control plane itself can run as several processes. The first decision is in: `apelora_domain::sites` names the sector a second deployment would use, and the members report refuses that plan while only one site is connected. It does not start the deployment.

## Alternatives considered

- **Stretch one application network across sectors at layer 2.** Simpler addressing, but it couples the sectors' failure domains again; layer-3 peering keeps a fault inside its sector.
- **One very large Incus cluster.** Simplest for Apelora, but against the upstream recommendation, and one fault takes everything down.
- **Stretch one cluster across sites.** Breaks the 10 ms rule; Raft and heartbeats become unreliable.
- **A cluster of control planes, one per sector, federated.** More moving parts than needed: Apelora's own state is small and lives in PostgreSQL; one control plane with one adapter per sector is enough.
