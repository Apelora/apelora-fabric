# Distributed key-value store (reference for the state store)

Use as a checklist when designing or choosing the Controller's state store, or any replicated store. Apelora starts with a single relational primary (simpler, strongly consistent). Read this when requirements push toward a distributed KV design.

## The CAP trade-off, in practice
In a partition you choose **consistency** (reject some requests) or **availability** (serve possibly stale data). Decide per data class:
- Desired state, ownership, locks, enrollment: **CP** (consistent; unavailable minority side)
- Metrics, heartbeats, caches, observed state: **AP** (available; eventually consistent)

## Building blocks
- **Partitioning:** consistent hashing with virtual nodes (see `consistent-hashing.md`).
- **Replication:** each key on R nodes (typically 3), spread across failure domains.
- **Quorum consistency:** N replicas, W write acks, R read acks. `W + R > N` gives strong reads; `W=1` is fast but weaker; `R=1` is fast reads. Typical N=3, W=2, R=2.
- **Versioning conflicts:** concurrent writes create divergent versions. Options: last-write-wins (simple, loses data), vector clocks (detect conflicts, client resolves), or avoid with a single writer per key (preferred for Apelora desired state).
- **Failure detection:** gossip with heartbeat counters; mark a node down only after several peers agree.
- **Temporary failure:** sloppy quorum plus **hinted handoff**: another node accepts writes for the down one and replays them later.
- **Permanent failure:** **anti-entropy** using Merkle trees to compare replicas and sync only differing ranges.
- **Write path:** commit log first (durable, append-only), then in-memory table, flushed to sorted immutable files (SSTables) and compacted. Bloom filters skip files lacking the key.
- **Read path:** check memory, then bloom filter, then files; merge versions.
- **No single coordinator:** any node can coordinate a request; clients or a routing tier find the owner.

## Decisions to record
- Consistency level per data class and the staleness clients may see
- Replication factor, quorum values, failure domains
- Conflict resolution rule
- Backup, restore, and disaster recovery targets (RPO/RTO)
- Upgrade path: rolling upgrades without downtime

## Tests
- Network partition with writes on both sides
- Node loss and replacement, replica repair
- Clock skew (do not depend on synchronized clocks for ordering)
