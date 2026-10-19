# Unique ID generation

IDs are created by many Controller instances, nodes, and browsers without a central coordinator. Requirements for Apelora: globally unique, roughly time-ordered (helps indexes and debugging), generated without coordination, safe to expose.

## Options

| Scheme | Unique without coordination | Time-ordered | Size | Notes |
|---|---|---|---|---|
| Auto-increment per DB | No (single writer) | Yes | 8 B | Does not scale across writers; leaks counts |
| Multi-master increment (step k) | Partly | No | 8 B | Breaks when nodes added or across sites |
| **UUIDv4** | Yes | No | 16 B | Random; poor index locality |
| **UUIDv7** | Yes | Yes (ms) | 16 B | Recommended default |
| **ULID** | Yes | Yes (ms) | 16 B | Lexicographically sortable text form |
| **Snowflake-style** (timestamp + machine id + sequence) | Yes, needs unique machine ids | Yes | 8 B | Smaller; requires machine-id assignment and clock discipline |
| Ticket server | Central | Yes | 8 B | Single point of failure |

## Recommendation
- Resources, jobs, events: **UUIDv7** generated locally.
- Where 64-bit is required or wanted: Snowflake layout `1 sign bit | 41 bits ms since custom epoch | 10 bits machine id | 12 bits sequence`, giving ≈ 69 years, 1,024 machines, 4,096 IDs per ms per machine.
- Human-facing names are separate from IDs and are mutable; IDs are immutable and opaque.

## Pitfalls
- **Clock skew / going backwards:** a Snowflake generator must refuse or wait if the clock moves back; use a monotonic guard. UUIDv7 libraries should use a counter within the same millisecond.
- **Machine-id collisions:** assign via enrollment or lease, never random.
- **Information leaks:** time-ordered IDs reveal creation time; do not use them as secrets or capability tokens (use random 128-bit tokens for those).
- **Index locality vs. unpredictability:** pick time-ordered for primary keys; random for tokens.

## Tests
- Uniqueness under parallel generation (millions of IDs, multiple threads/processes)
- Monotonic ordering within one generator
- Behavior on clock regression
