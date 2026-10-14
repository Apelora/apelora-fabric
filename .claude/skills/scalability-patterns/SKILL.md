---
name: scalability-patterns
description: Choose and apply scaling and resilience patterns in Apelora Fabric — stateless services, replication, caching, queues, sharding, consistent hashing, multi-site operation, rate limiting, and unique ID generation. Use when a component must scale beyond one instance or survive failures.
---

# Scalability patterns

Apply a pattern only when the estimate (see `capacity-estimation`) or a failure requirement demands it. Start simple, name the trigger for adding each pattern.

## Patterns and where they fit Apelora

| Pattern | Use when | Apelora application |
|---|---|---|
| **Stateless service tier** | More than one instance needed | Controller API holds no session state; state lives in the store. Instances sit behind a load balancer and can be added or removed freely |
| **Replication** | Need read scale or failover | Primary plus replicas for the store; define promotion procedure and accept replication lag on reads |
| **Cache** | Data read often, changed rarely | Catalog manifests, validated schema results. Always define expiry, invalidation, and what happens on a cache failure |
| **Message queue / job queue** | Slow or bursty work, decoupling producers from consumers | Deploys, image pulls, snapshots. Scale workers on queue depth; jobs are idempotent and retried with backoff |
| **Sharding** | One store cannot hold the data or writes | Last resort. Choose a key that spreads load (e.g. assembly or tenant id); plan resharding up front |
| **Consistent hashing** | Many nodes added or removed, minimal data movement wanted | Placing assemblies or volumes across nodes; use virtual nodes to even out load |
| **Rate limiting** | Protect APIs and nodes | Per-user and per-token limits at the API; token bucket; return clear 429 with retry info |
| **Unique IDs** | IDs created on many machines | Prefer UUIDv7 or time-ordered IDs generated without coordination; never rely on a single counter |
| **Multi-site** | Region/datacenter failure must be survived | Route to the nearest healthy site, replicate state asynchronously, test failover regularly |

## Rules

1. Remove single points of failure at each tier before adding features; state which ones remain and why.
2. Design every retried operation to be idempotent.
3. For caches and replicas, write down the consistency a caller can expect.
4. Add logging, metrics, and alerts with each pattern (see `observability`).
5. Do not shard, cache, or add queues "just in case"; record the measured trigger in the design note.

## Checklist

- Failure of each component described and tested with a fake
- Load test or estimate backs the choice
- Rollback plan for the change
