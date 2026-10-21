# Capacity estimate: a global fabric of 50,000 servers

Status: estimate · Date: 2026-10-21 · For: [ADR 0009](../adr/0009-global-fabric.md), [global fabric](../architecture/GLOBAL-FABRIC.md)

An order of magnitude to check that the shape in ADR 0009 holds, and where it must change. Every number is labelled *given* (owner or upstream), *measured* (our lab) or *guessed*.

## Assumptions

| # | Assumption | Value | Source |
|---|---|---|---|
| A1 | Servers in the fabric | 50,000 | given ("thousands to tens of thousands"), rounded up |
| A2 | Servers per sector | 50 | given (Incus guidance) |
| A3 | Sectors per site | 20 | guessed (one data hall) |
| A4 | Regions | 10 | guessed |
| A5 | Instances per server (mostly system containers) | 30 | guessed |
| A6 | Instances per application | 6 | guessed |
| A7 | People using the Console or Studio | 5,000 | guessed |
| A8 | API calls per person per working day | 200 | guessed |
| A9 | Deployments per application per week | 2 | guessed |
| A10 | Incus API calls per deployment | 10 | measured (lab jobs: create, start, network sync, volumes) |
| A11 | Status poll per sector | one instance list every 30 s | built (supervisor and health) |
| A12 | Instance record (desired + observed) | 5 KB | guessed |
| A13 | Design document with 10 versions, compressed | 400 KB | guessed (200 KB per design uncompressed, 10 versions, ~5× compression) |
| A14 | Audited events per day | 30 % of API calls plus sign-ins and jobs | guessed |
| A15 | Audit row | 1 KB | measured (lab audit rows ~0.5–1 KB) |
| A16 | Stateful instances | 20 %, 50 GB each | guessed |
| A17 | Daily changed data in stateful volumes | 2 % | guessed |

## Derived

- **Shape:** 50,000 ÷ 50 = **1,000 sectors**; ÷ 20 = **50 sites**; ÷ 10 = **5 sites, 100 sectors and 5,000 servers per region**.
- **Instances:** 50,000 × 30 = **1.5 million**; applications ≈ 1.5 M ÷ 6 = **250,000**.
- **API load (people):** 5,000 × 200 = 1 M calls/day ÷ 86,400 ≈ **12 per second**, peak ≈ 36/s, for the whole fabric. Per region ≈ 4/s at peak. Trivial for one API process.
- **Status polling:** 1,000 sectors ÷ 30 s ≈ **33 list calls/s** fabric-wide, **3.3/s per region**. Each list ≈ 1,500 instances × 1 KB ≈ 1.5 MB, so ≈ **5 MB/s per region** of control traffic, all inside the region. Fine. Replacing polling with Incus event streams (one per sector) cuts it further.
- **Jobs:** 250,000 × 2 per week ≈ 71,000 deployments/day ≈ **0.8/s fabric-wide**, about 0.1/s per region; × 10 Incus calls ≈ 1 call/s per region. Workers are not the bottleneck; Incus's own database write rate per sector is (A2).
- **Regional PostgreSQL:**
  - instance state 150,000 × 5 KB ≈ **0.75 GB**;
  - designs the region runs, 25,000 × 400 KB ≈ **10 GB**;
  - audit ≈ (100,000 calls × 30 % + jobs and sign-ins) × 1 KB ≈ **35 MB/day ≈ 13 GB/year**.

  One primary and a synchronous standby, with years of headroom.
- **Global PostgreSQL:**
  - all designs, 250,000 × 400 KB ≈ **100 GB**;
  - people, roles and policies are small;
  - roll-ups ≈ 1,000 sectors × 1 row per minute.

  One primary with a read replica per region.
- **Backup traffic:**
  - Stateful data: 1.5 M × 20 % × 50 GB ≈ **15 PB**.
  - Daily changes: 2 % ≈ **300 TB/day**, or **6 TB/day per site** ≈ 0.6 Gbit/s sustained per site for tier 2.
  - A 10 Gbit/s site-to-site link has ~15× headroom.
  - The weekly tier-3 archive goes to object storage, staggered.
- **Availability targets:**
  - global plane 99.9 % (it can be down without stopping regions);
  - regional plane 99.95 %;
  - application availability comes from replicas across sectors or sites, not from the planes.

## Conclusion

- **Decision.** The shape holds: ten regional planes of modest size (a few API processes, a few workers, one PostgreSQL pair each) and a small global plane cover 50,000 servers. Control traffic stays inside regions. Nothing needs a bespoke datastore.
- **Where it changes:**
  - A region beyond ~200 sectors (10,000 servers), or beyond ~10 status polls per second: split it into two regions.
  - Global-plane writes beyond ~100/s: shard designs by organisation.
  - Backup change rate beyond ~30 % of site-to-site link capacity: add links, or move tier 2 to a dedicated replication network.
- **Metrics to watch:**
  - per sector: Incus operations in flight and API latency at p95;
  - per region: reconcile lag at p95, job queue age, PostgreSQL commit latency, replication lag from the global plane;
  - per site: backup backlog in bytes.
- **Limits to put in code:**
  - 50 members per sector by default, with a warning and a configurable cap;
  - 200 sectors per region, warned;
  - status poll interval at least 15 s per sector;
  - concurrent cross-site copies per link capped.
