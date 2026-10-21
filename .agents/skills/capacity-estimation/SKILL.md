---
name: capacity-estimation
description: Do back-of-the-envelope capacity and performance estimates for Apelora Fabric — request rates, storage, reconcile load, event volume, member (server) counts, bandwidth, and availability targets (nines) — and turn them into limits and sizing decisions. Use when sizing the control plane, worker, store, queue, or the cluster's members, setting API/document limits, or checking whether a design fits its scale.
---

# Capacity estimation

The goal is a defensible order of magnitude and a clear next decision, not a precise number. The process matters more than the result: state assumptions, show the arithmetic, and say what the answer means.

**Inputs required:** assumptions (or permission to propose them). **Output:** `docs/design/capacity-<topic>.md`.

## Method

1. **Write assumptions**, each labeled with its unit and source (measured / given / guessed). Examples: members (servers), instances per member (system containers by default; OCI containers and VMs only where a class says so), modules per assembly, assemblies, users, API calls per user per day, reconcile interval, manifest size, assembly document size, event size, retention.
2. **Derive rates.**
   - Requests per second (QPS) = daily requests ÷ 86,400. Peak ≈ 2–3× average unless data says otherwise.
   - Control loops: objects ÷ interval = reconciles per second; multiply by calls to the Incus API per reconcile.
   - Writes versus reads: state the ratio; it drives replication and caching choices.
3. **Derive storage.** record size × count × retention, plus indexes (~1.5–2×), replicas, snapshots, audit logs, metrics.
4. **Derive bandwidth.** payload × rate; include image pulls (image size × members pulling, and whether pulls are staggered or cached).
5. **Derive fleet size.** peak load ÷ per-instance capacity, then add headroom (target ≤ 60–70% utilization at peak) and redundancy (N+1 or N+2).
6. **Sanity-check and conclude.** If one control plane, one worker and one database handle it with headroom, say so and stop. Name the threshold at which the design must change and the metric that will warn you.

## Rounding and hygiene

- Round to friendly numbers (99,987 ÷ 9.1 ≈ 100,000 ÷ 10). Keep one or two significant figures.
- Label every unit (5 KB vs 5 MB). Use powers of two loosely: 1 KB ≈ 10³ B, 1 MB ≈ 10⁶, 1 GB ≈ 10⁹, 1 TB ≈ 10¹², 1 PB ≈ 10¹⁵.
- Keep a running list of assumptions; unlabeled numbers are bugs.
- Cross-check with a second method (top-down vs bottom-up) when the stakes are high.

## Latency intuition

Memory ≈ nanoseconds to 100 ns; SSD read ≈ tens to hundreds of µs; spinning-disk seek ≈ ms; same-datacenter round trip ≈ 0.5 ms; cross-region ≈ tens to 150 ms. Conclusions: memory is fast and disk is slow; avoid disk seeks on hot paths; compress before sending across wide-area links; cross-region calls are expensive, so keep chatty protocols local.

## Availability budgets

| Target | Downtime per year | per month |
|---|---|---|
| 99% | ~3.65 days | ~7.3 h |
| 99.9% | ~8.8 h | ~44 min |
| 99.99% | ~53 min | ~4.4 min |
| 99.999% | ~5.3 min | ~26 s |

Serial dependencies multiply (two 99.9% components in series ≈ 99.8%); redundancy in parallel improves it. State the target **per component** and what the user-visible SLO is.

## Apelora worked example (template)

Assumptions: 200 members (physical servers), 20 instances per member = 4,000 instances (mostly system containers; VMs only for classes that need one); 500 applications; the supervisor checks each instance every 30 s; 3 calls to the Incus API per check; 20 operators, 500 API calls/day each; assembly doc 200 KB; manifest 10 KB. Member heartbeats are Incus's own business and cost Apelora nothing.

- Reconcile load: 4,000 ÷ 30 ≈ 133 checks/s → ≈ 400 Incus API calls/s in total. Fine for one worker; how many concurrent requests the Incus API accepts matters more than the total. Prefer one list call per project over one call per instance.
- API: 20 × 500 = 10,000/day ≈ 0.12 QPS average; peak ≈ 1 QPS. Trivial; no scaling needed for the API.
- Storage: 500 × 200 KB × 10 revisions = 1 GB. Trivial; one Postgres instance suffices.
- Events/logs: 4,000 instances × 1 event/min × 500 B ≈ 33 KB/s ≈ 2.9 GB/day → retention drives cost; choose 30 days ≈ 87 GB.

Conclusion: one control plane, one worker and one primary database cover this with large headroom. Revisit at ≈ 10× instances, when reconcile fan-out to the Incus API (not Apelora's own API) becomes the bottleneck; the signal is reconcile lag p95.

## Apelora questions to always ask

- Reconcile cost at 10× objects; concurrent calls to the Incus API; thundering herd after a worker restart (add jitter)
- Maximum assembly size and nesting depth (enforce in code to match the estimate)
- Event/log volume and retention; audit log growth
- Image distribution: first pull versus cached; registry bandwidth
- Hot spots: one huge assembly, one busy member, one noisy tenant

## Acceptance criteria

- Assumptions are listed with units and provenance
- Arithmetic is shown; results are rounded sensibly
- Conclusion states the decision, the headroom, the threshold for change, and the metric to watch
- Limits derived from the estimate are added to the design (and later to code and tests)
