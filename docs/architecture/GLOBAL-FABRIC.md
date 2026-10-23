# The global fabric: from three servers to tens of thousands

Status: **target design** · Date: 2026-10-21 · Decisions: [ADR 0008 — sectors](../adr/0008-sectors.md), [ADR 0009 — global fabric](../adr/0009-global-fabric.md) · Numbers: [capacity estimate](../design/capacity-global-fabric.md) · Today's limits and connections: [connections and scale](CONNECTIONS-AND-SCALE.md) · Where the product is going: [cloud roadmap](CLOUD-ROADMAP.md)

Apelora starts on three servers and is meant to grow into a cloud spread over cities, countries and continents. This page explains how the same building blocks scale to that, and what has to be added at each step. One control plane and one sector are what runs. The members report names that site and refuses to plan a second deployment until another site is connected; a second Incus cluster is not managed. Every section ends with what is built today.

![Regions, sites and sectors](../assets/diagrams/global-fabric.svg)

## 1. Levels

| Level | Is | Limit that defines it | Owns |
|---|---|---|---|
| Fabric | everything one operator runs | — | the global plane |
| Region | a country or nearby countries | jurisdiction; ~30 ms between its sites; up to ~200 sectors | a regional control plane and its PostgreSQL |
| Site | one data centre (a city) or an edge location | its own power, cooling and network; under 2 ms inside | a backup target for its sectors, ingress addresses, a backbone link |
| Sector | one Incus cluster | Incus: about 50 servers, under 10 ms | its cluster database, OVN, storage pools |
| Server | a physical server | hardware | instances |

How it grows:

| Step | Servers | Shape |
|---|---|---|
| Start | 3 | one sector, one site, one region |
| Rack | 3–50 | one sector |
| Data centre | 50–1,000 | up to 20 sectors in one site, linked by OVN interconnection |
| Country | 1,000–10,000 | several sites in one region |
| Continents | 10,000–100,000+ | many regions under one global plane |

## 2. Control planes

- **Regional plane** (today's control plane, scoped to a region):
  - desired state, deployments, jobs, workers, health, capacity, terminal sessions and audit for its region;
  - its own PostgreSQL: a primary plus a synchronous standby in another site of the region;
  - workers sharded by sector;
  - it talks to its sectors' Incus APIs over at most ~30 ms.
- **Global plane:**
  - people, groups, roles, sign-in methods;
  - module classes, application designs and versions;
  - placement and residency policies;
  - the address plan; regions and sites;
  - read-only roll-ups; the audit archive.

  It sees few writes and many reads. Its PostgreSQL has a primary in one region and read replicas in every region.
- **Static stability.** Each region keeps local copies of what it needs from the global plane, so it keeps operating when the global plane or the links between regions are down:
  - verification keys for identity-provider tokens;
  - role assignments;
  - module classes;
  - the designs it runs;
  - policies.

  Revoking a role takes effect in a region as soon as the copy reaches it (seconds). Identity-provider tokens are short-lived (at most an hour), which bounds the rest.
- **One Console.** It asks the global plane for shared things and the region's plane for operations, with the same sign-in. Lists across regions come from the global roll-ups and say how fresh they are.

*Built today:* one control plane, one sector. Servers → Members names that site and sector and says a second deployment is not planned (MOCK VERIFIED). A second Incus cluster is not connected.

## 3. Networking

### What is there and how far it carries

| Scope | Technology | Carries to |
|---|---|---|
| Inside a sector | OVN: a logical network per application, ACLs, load balancers; Geneve overlay between servers (built) | the sector's 50 servers (OVN itself runs at hundreds of hypervisors) |
| Between sectors of a site | OVN interconnection: transit switches and gateway servers, layer-3 routes | a site's ~20 sectors |
| Between sites of a region | OVN interconnection over the regional backbone, encrypted (IPsec, or MACsec on owned fibre) | ~30 ms between sites |
| Between regions | no shared application networks; published services with mutual TLS; private backbone or encrypted tunnels | the world |
| People → applications | published addresses per site; geographic DNS or anycast to the nearest healthy region | the world |

### What to augment, and when

The current stack (OVN on Geneve, driven through Incus) does not need replacing to grow. These additions make it carry more, run faster and need fewer hand-made steps. All of them are standards-based and can be driven through Incus or the switches' own APIs.

| Augmentation | What it gives | When |
|---|---|---|
| **Routed underlay** (leaf-spine, BGP to every server, equal-cost multipath) | no large layer-2 domains, no spanning tree; any server reaches any other over several paths; failures reroute in under a second | from the first data centre beyond one rack |
| **BGP from Incus** (`core.bgp_*` on members; OVN uplinks announce external subnets and forward addresses) | published and service addresses are routed, not NATed; an address can move with its application between sectors by being re-announced | with the routed underlay |
| **Anycast for published addresses** (the same address announced from several sectors or sites) | load spreads across sites; a failed site stops announcing and traffic moves | multi-site applications |
| **EVPN with VXLAN in the underlay** | tenant isolation and layer-2 extension *between racks* where an application truly needs it (rare; layer 3 is the default) | only on request |
| **Hardware offload** (`acceleration=sriov` or `vdpa` on OVN NICs; switchdev NICs, DPUs) | line-rate virtual networking with almost no host CPU; needed for high-throughput VMs | performance tiers |
| **Separate traffic classes** (storage and replication, migration, tenant traffic, management) on separate VLANs or queues | backups and moves never starve applications | from the first SAN or cross-site copy |
| **Jumbo frames in the underlay** (MTU 9000; tenant MTU 1500 stays intact under Geneve's ~50-byte overhead) | no fragmentation; faster copies | from the start in new sites |
| **Global address management** (in the global plane) | non-overlapping IPv4 and IPv6 for every region, site, sector and application network | before the second sector |

The address plan is hierarchical, for example:
- IPv6: a /32 per fabric → /40 per region → /48 per site → /56 per sector → /64 per application network.
- IPv4: private space split the same way; small public blocks only for published addresses.

Apelora allocates; Incus and OVN apply.

*Built today:* OVN networks per application with ACLs, service addresses and published addresses in one sector (REAL INCUS TESTED on the lab).

## 4. Moving things

One mechanism, used at every distance; what changes is bandwidth, downtime and policy.

| Distance | How | Downtime | Built |
|---|---|---|---|
| Between servers of a sector | Incus move. VMs with live migration move running (0.26 s longest pause measured). Others stop, copy and start. | ~0 s for live VMs; seconds to minutes otherwise | yes, REAL INCUS TESTED |
| Between sectors of a site | Apelora job. Pre-copy with `incus copy --refresh` while running, stop, final refresh, start in the target, re-point service and published addresses, delete the source once healthy. | the final refresh: seconds to minutes | no |
| Between sites of a region | the same job, over the regional backbone | the same, plus the backbone's speed | no |
| Between regions | the same job, **only if the residency policy allows it**; usually a second deployment plus application-level replication instead | — | no |
| A whole sector or site (maintenance, migration of hardware) | the job in waves, capped so the backbone keeps headroom | per application, as above | no |

How long a copy takes (pre-copy, before any downtime):

| Data | 1 Gbit/s | 10 Gbit/s | 100 Gbit/s |
|---|---|---|---|
| 100 GB | ~14 min | ~1.5 min | ~8 s |
| 1 TB | ~2.3 h | ~14 min | ~1.5 min |
| 10 TB | ~23 h | ~2.3 h | ~14 min |

The final refresh only moves what changed since the pre-copy (typically 1–5 %), so downtime stays in the seconds-to-minutes range even for large volumes on 10 Gbit/s links.

## 5. Backups and recovery

| Tier | Where | Protects against | Built |
|---|---|---|---|
| 0 | snapshots on the volume's own pool, on a schedule | mistakes, bad updates | yes |
| 1 | a copy in the `backup` pool on another server; later in another sector of the same site | a server or a sector lost | another server: yes; another sector: no |
| 2 | a copy in another site of the same region | a site lost (fire, power, flood) | no |
| 3 | an exported, encrypted, immutable archive in object storage, outside the region only where policy allows | ransomware, a region lost, legal retention | no |

- **Recovery classes** per application, chosen in the Studio and shown in the Console:
  - Gold: recovery point 15 minutes, recovery time 1 hour (tiers 0–2 plus database-level replication in the application).
  - Silver: 4 h / 8 h.
  - Bronze: 24 h / 72 h.
- **Restore drills.** A scheduled job restores a sample of backups into an isolated project and checks that they start. The result is part of the Health page.
- **Losing a region.** The global plane holds the designs and the last desired state. The standby region redeploys from them and from tier 2 or 3 copies. People see what is older than the recovery point before anything starts.

## 6. Other things that change with distance

- **Residency.** Policies per organisation or project list allowed regions or countries. Placement, moves, backups and the audit archive obey them; a refusal says which policy and which country.
- **Identity.**
  - Identity-provider tokens are checked in each region with cached keys, so no call to the global plane is needed.
  - Local accounts and their sessions live in the global plane, with regional read copies; revocation reaches regions within seconds.
  - Directories can be per region (a country's own directory).
- **Images.** Each region keeps a cache of module images; a new image version is pulled once per region, not once per server.
- **Time.** Every site runs NTP (or PTP for timing-sensitive workloads). Incus and the Raft database need synchronised clocks.
- **Upgrades.** In waves:
  1. a canary sector;
  2. one site;
  3. one region;
  4. the rest.

  Incus is upgraded per sector (LTS releases; schema changes only between LTS releases). Regional planes are upgraded region by region and must accept one version of skew with the global plane.
- **Observability.** Metrics and logs stay in the region. The global plane keeps roll-ups and alert states. Every request carries a request id across planes.
- **Security.** Each sector has its own Incus client certificates; each region has its own intermediate certificate authority. Taking one region's credentials does not reach another.
- **Operations.** On-call per region, with a follow-the-sun handover. A region can be put in maintenance as a whole.

## 7. Order of work

1. Sectors (ADR 0008): a `sectors` table, an adapter per sector, `sector` on applications.
2. Regions and sites as labels, residency policies, global address management.
3. The per-process fixes (supervisor leader, sign-in reload, shared session tickets), so a plane runs as several processes.
4. Moves between sectors, then between sites (§4).
5. Backup tiers 1–3 and restore drills (§5).
6. A routed underlay with BGP from Incus, anycast for published addresses (§3).
7. Splitting the global plane from the regional plane, with replication and static stability (§2).
8. Global traffic steering.
