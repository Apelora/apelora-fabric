# Consistent hashing

**Why:** with `hash(key) % N`, adding or removing a node remaps almost every key. Consistent hashing remaps only about `K/N` keys (K keys, N nodes). Use it to place work or data across a changing set of nodes.

## Mechanism
1. Hash both **nodes** and **keys** onto the same circular space (e.g. 0 … 2^64−1) with a stable, well-distributed hash (xxHash/SipHash/SHA-256 truncated). Do not use language-default hashers whose output changes between versions.
2. A key belongs to the first node found going clockwise from the key's position.
3. **Add a node:** it takes over keys between its predecessor and itself; only those move.
4. **Remove a node:** its keys move to its clockwise successor.

## Problems and fixes
- **Uneven partitions and load:** with few nodes, arcs are unequal. Use **virtual nodes**: each physical node appears at many ring positions (e.g. 100–500). More vnodes means better balance but a bigger ring/metadata. Weight vnodes by node capacity.
- **Hot keys:** one huge key still lands on one node; split or replicate the key.
- **Replication:** store each key on the next R distinct physical nodes clockwise (skip vnodes of the same host and, ideally, the same failure domain/rack).

## Where it fits Apelora
- Assigning assemblies or reconcile partitions to Controller instances
- Choosing candidate nodes for volume placement or image caching
- Partitioning job queues and event streams

Not for scheduling VMs by capacity: use the scheduler (`rust-control-plane`) with constraints. Hashing ignores resource fit.

## Implementation notes (Rust)
- Keep a sorted `BTreeMap<u64, NodeId>` of vnode positions; lookup with `range(hash..).next()` wrapping to the first entry.
- Deterministic across processes and versions; test with fixtures.
- Provide `add_node`, `remove_node`, `lookup(key)`, `replicas(key, r)`.

## Tests
- Property: removing a node moves only keys it owned
- Balance: with 200 vnodes, max load within ~10–15% of mean for 10k keys
- Stability: identical results across runs and platforms
