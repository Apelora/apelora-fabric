---
name: scalability-patterns
description: Choose, justify, and apply scaling and resilience patterns in Apelora Fabric — stateless service tiers, database replication, caching, message/job queues, sharding, consistent hashing, rate limiting, unique ID generation, key-value state stores, notification delivery, and multi-site operation. Use when a component must scale past one instance, survive failures, or when evaluating a proposal that adds one of these patterns.
---

# Scalability patterns

Scaling is iterative. Start with the simplest design that meets the estimate (`capacity-estimation`), then add a pattern only when a **measured trigger** or a **failure requirement** demands it. Record the trigger in the design note.

**Detailed guides (read the one you need):**
- [rate-limiter](references/rate-limiter.md) — algorithms, placement, distributed counters
- [consistent-hashing](references/consistent-hashing.md) — ring, virtual nodes, placement of assemblies/volumes
- [kv-store](references/kv-store.md) — replication, quorum, versioning, failure handling for the state store
- [unique-ids](references/unique-ids.md) — ID schemes without coordination
- [jobs-and-notifications](references/jobs-and-notifications.md) — queues, retries, delivery of events to users and nodes

## Pattern selector

| Symptom or requirement | Pattern | Apelora application |
|---|---|---|
| Need more than one service instance | **Stateless tier + load balancer** | Controller API keeps no session state; instances are interchangeable |
| Read load or failover needed | **Replication** | Primary + replicas for the store; define promotion and tolerated lag |
| Repeated expensive reads | **Cache** | Catalog manifests, validation results, resolved nested assemblies |
| Slow, bursty, or failure-prone work | **Queue + workers** | Deploys, image pulls, snapshots, notification delivery |
| Dataset or write rate exceeds one store | **Sharding** | Last resort; key by tenant or assembly |
| Nodes join and leave often | **Consistent hashing** | Placing assemblies, volumes, or work partitions on nodes |
| Abusive or runaway callers | **Rate limiting** | API per token and per IP; node RPC per caller |
| IDs minted on many machines | **Unique ID scheme** | Time-ordered UUIDs for resources, jobs, events |
| Datacenter or region failure must be survived | **Multi-site** | Geo-routing, async replication, tested failover |
| Observability gaps while scaling | **Logging/metrics/automation** | See `observability` |

## Core patterns in depth

### Stateless service tier
- Move session/auth state out of the process: signed tokens or a shared store. Any instance can serve any request.
- Put instances behind a load balancer on private IPs only; health-check them (`/readyz`) and drain gracefully on shutdown.
- Autoscale on a load signal (request rate, queue depth), not CPU alone.
- Avoid sticky sessions; they make failure handling and scale-in harder.
- **Test:** kill an instance mid-request; the client retry must succeed on another.

### Replication and failover
- One primary takes writes; replicas serve reads and standby. Reads from replicas may be stale: label which endpoints tolerate that (read-your-writes needs the primary or a session token with a version).
- If a replica fails: route reads elsewhere and rebuild. If the primary fails: promote the most up-to-date replica, fence the old primary (prevent split-brain), and run recovery for the unreplicated tail.
- Decide sync vs async replication explicitly: async risks data loss on failover, sync costs latency and availability.
- **Test:** scripted failover drill with data-loss measurement.

### Caching
- Pick a strategy and write it down: cache-aside/read-through for reads, write-through or write-around for writes.
- Always define: **expiry** (not too short: reload storm; not too long: stale), **invalidation** (on write, on version change, by versioned keys), **eviction** (LRU default), and **failure behavior** (cache down means slower, not broken; protect the store from a stampede with request coalescing and jittered TTLs).
- A cache is not a source of truth; never put the only copy of important data there.
- Version cache keys with the schema or manifest version to avoid cross-version poisoning.

### Queues and workers
- Decouple producers from consumers; producers publish when consumers are down, consumers drain when producers are down.
- At-least-once delivery is the realistic default, so every job handler must be **idempotent** (idempotency key = job id or resource generation).
- Retry with exponential backoff and jitter; cap attempts; send exhausted jobs to a dead-letter queue with an alert.
- Scale workers on queue depth and age of oldest message. Preserve ordering only where required (per-resource key).
- Persist job state (`pending | running | succeeded | failed | cancelled`) so it survives restarts.

### Sharding (last resort)
- Trigger: one primary cannot hold the data or the write rate even after vertical scaling, caching, and read replicas.
- Choose a **shard key** that spreads load evenly and keeps common queries on one shard. Avoid keys that create hot spots (a celebrity/huge tenant); split hot keys further.
- Plan **resharding** up front (use consistent hashing or a directory service), and denormalize to avoid cross-shard joins.
- Accept the cost: distributed transactions, cross-shard queries, operational complexity.

### Multi-site
- Route users to the nearest healthy site (geo-DNS or anycast); on a site failure shift all traffic to a healthy site.
- Replicate data asynchronously across sites; define RPO/RTO and conflict resolution up front.
- Keep test and deployment identical across sites; run failover game days.

## Rules

1. Remove single points of failure at each tier before adding features; list the ones that remain and why.
2. Every retried operation is idempotent.
3. Document the consistency each caller can expect for every cache and replica.
4. Instrument each pattern (queue depth, cache hit rate, replication lag) before relying on it.
5. Do not add sharding, caching, or queues "just in case".
6. Each pattern ships with a failure test using fakes or fault injection (`quality-assurance`).

## Acceptance criteria

- The design cites the estimate or failure requirement that justifies the pattern
- Failure behavior and consistency semantics are written down
- Metrics and alerts for the pattern exist
- A test demonstrates behavior under the relevant failure
