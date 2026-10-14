# Rate limiter

**Why:** prevent resource starvation from abusive or buggy clients, control cost, and protect nodes and the Controller. Apelora uses it on the public API, authentication endpoints, and Controller→Node RPC.

## Clarify first
- Client-side or server-side? (Server-side; clients cannot be trusted.)
- Throttle by what: user, API token, IP, endpoint, tenant?
- Hard limit (reject) or soft (queue/slow)? Distributed (many API instances)? Must users be told (headers)?

## Placement
- **Middleware / API gateway layer** in front of handlers is the default. Keep rule logic out of handlers.
- Return `429 Too Many Requests` with `Retry-After`, and `X-RateLimit-Limit`, `X-RateLimit-Remaining`, `X-RateLimit-Reset` headers.
- Rules live in configuration (per route, per role), cached in memory, reloadable without restart.

## Algorithms

| Algorithm | Idea | Pros | Cons |
|---|---|---|---|
| **Token bucket** | Bucket of N tokens refills at rate r; each request takes one | Allows bursts, memory-light, simple | Two parameters to tune |
| **Leaking bucket** | Fixed-rate queue drains at constant rate | Smooth output | Bursts fill queue; old requests delay new ones |
| **Fixed window counter** | Count per time window | Simplest | Edge bursts up to 2× at window boundaries |
| **Sliding window log** | Store request timestamps | Accurate | Memory-heavy |
| **Sliding window counter** | Weighted blend of current and previous window | Accurate enough, cheap | Approximate |

Default recommendation: **token bucket** for API calls (burst-friendly), sliding window counter for login attempts.

## Distributed concerns
- Use a shared fast store (Redis-like) with **atomic** operations (Lua script or `INCR` + `EXPIRE`) to avoid read-modify-write races.
- Synchronize across sites eventually, not synchronously; each site may enforce a local share of the limit.
- Sticky routing is not required if the counter store is shared.
- Fail open or closed? Decide per endpoint: login and write endpoints fail closed, read-only endpoints may fail open. Alert on limiter-store failures.

## Checklist
- Rule definition format and defaults documented
- Identity chosen (token id preferred over IP behind NAT)
- Headers and 429 behavior tested
- Race test with parallel requests
- Metrics: allowed, rejected, per-rule, and limiter latency
