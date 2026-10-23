# ADR 0009 — A global fabric: regions, sites and sectors, with a control plane per region

Status: **accepted. The decision that names another site's sector is implemented and refuses while one site is connected. A plane per region, cross-site moves and cross-site networks are not built** · Date: 2026-10-21 · Extends: [ADR 0008 — sectors](0008-sectors.md) · Details: [global fabric](../architecture/GLOBAL-FABRIC.md), [capacity estimate](../design/capacity-global-fabric.md)

## Context

One Incus cluster is recommended to stay at about 50 servers (100–150 run, 250 has been tested) and needs its members within 10 ms of each other (ADR 0008). The owner wants installations of thousands to tens of thousands of servers spread across cities, countries and continents. That is hundreds to thousands of sectors.

ADR 0008 links sectors to one control plane. That stops working at this scale and distance:

- **Distance.** Incus API calls, job polling and terminal sessions from one control plane to sectors on other continents (100–250 ms away) are slow, and a wide-area outage would leave every remote sector unmanaged.
- **Blast radius.** One control plane and one database for the whole world means one fault stops every region's operations.
- **Jurisdiction.** Data that must stay in a country (designs, audit, backups, volumes) cannot all live in one central database.
- **Load.** Polling and jobs for a thousand sectors in one place (see the estimate) is possible, but it puts every region behind one database's write rate.

## Decision

1. **Five levels, each with one job:**

   | Level | What it is | Size | Latency inside |
   |---|---|---|---|
   | **Fabric** | everything one operator runs | — | — |
   | **Region** | a country or a group of nearby countries; a jurisdiction boundary | up to ~200 sectors (~10,000 servers) | up to ~30 ms between its sites |
   | **Site** | one data centre in one city: own power, cooling and network | up to ~20 sectors | under 2 ms |
   | **Sector** | one Incus cluster (ADR 0008) | 3–50 servers, a rack or a row | under 10 ms (Incus rule) |
   | **Server** | a physical server, an Incus member | — | — |

   Small **edge sites** (one sector of 3–10 servers in a city) belong to the nearest region.

2. **A control plane per region, and a small global plane.** The design follows the "cell" pattern:
   - The **regional plane** is today's control plane, scoped to one region. It owns that region's desired state, deployments, jobs, workers, health, capacity and audit, and its own PostgreSQL. It talks only to its own sectors' Incus APIs, all within ~30 ms.
   - The **global plane** owns what is shared and changes rarely:
     - people, groups, roles and sign-in methods;
     - module classes;
     - application designs and their versions;
     - placement and residency policies;
     - the address plan;
     - the list of regions and sites;
     - read-only roll-ups (inventory, health, capacity);
     - the archive of audit records.
   - **Regions stay up without the global plane** ("static stability"). A region keeps copies of everything it needs from the global plane: verification keys for identity-provider tokens, role assignments, classes, the designs it runs, and policies. Applications keep running and can be operated in their region while the global plane or the links between regions are down. Only cross-region actions wait.

3. **Placement works top-down.** The global plane picks the region: residency policy first, then distance to the application's users, then capacity. The regional plane picks the site and sector, spreading replicas across sites when the application asks for it. Incus places instances inside the sector.

4. **Networks get wider only by layer-3 links, and only as far as latency allows.**
   - Inside a sector: OVN (built).
   - Between sectors of a site: OVN interconnection (ADR 0008).
   - Between sites of a region: OVN interconnection over the regional backbone, encrypted.
   - Between regions: no shared application networks. Applications call each other through **published services** with mutual TLS, and people reach applications through global traffic steering (geographic DNS or anycast).
   - One **global address plan** gives every region, site, sector and application network its own non-overlapping IPv4 and IPv6 ranges, so any two sectors can be linked later without renumbering.

5. **Moves and copies use one mechanism, checked against distance and policy.** Within a sector: Incus moves (VM live migration is built). Between sectors, sites or regions: an Apelora job.
   1. Pre-copy with incremental refresh while the instance runs.
   2. Stop it.
   3. Do a final refresh.
   4. Start it in the target.
   5. Update its service address and published address.
   6. Delete the source only after the target is healthy.

   Moves across a residency boundary are refused unless the policy allows them.

6. **Backups follow a 3-2-1 shape inside the jurisdiction.**
   - Snapshots on the same pool.
   - A copy in another sector of the same site.
   - A copy in another site of the same region.
   - An encrypted, immutable archive (exported backups in object storage), outside the region only where policy allows.

   Each application gets a recovery class with a stated recovery-point and recovery-time target, and restores are tested on a schedule.

## Consequences

- Scale is by repetition. A region is a unit you can copy. The global plane stays small (rare writes, mostly reads), so tens of thousands of servers is a matter of more regions, not a bigger database.
- Two kinds of API. The Console stays one address: it asks the global plane for shared things, and the region's plane for operations, with the same sign-in.
- Some things become explicit. People choose a region, or let policy choose one. Cross-region availability means running in two regions; data is replicated by the application.
- Work, in order:
  1. Sectors (ADR 0008).
  2. Regions and sites as labels with placement policies.
  3. The per-process fixes so a plane runs as several processes.
  4. Splitting the global plane out of today's control plane, with replication to regional planes.
  5. Cross-sector and cross-site moves.
  6. The backup tiers.
  7. Global traffic steering.

## Alternatives considered

- **One control plane for the world, with sectors everywhere (ADR 0008 only).** Simplest, and fine up to a few hundred servers in one region. Beyond that it fails on distance, jurisdiction and blast radius.
- **Fully independent installations per region, with no global plane.** Strong isolation, but people, roles, designs and policies would be managed many times over, and there would be no single view.
- **Stretched networks between regions.** Convenient addressing, but it couples failure domains across continents and puts 100+ ms inside application networks.
