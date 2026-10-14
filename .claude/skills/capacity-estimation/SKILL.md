---
name: capacity-estimation
description: Do back-of-the-envelope capacity and performance estimates for Apelora Fabric — request rates, storage, reconcile load, node counts, and availability targets. Use when sizing the Controller, store, or node fleet, setting limits, or checking whether a design fits its scale.
---

# Capacity estimation

Estimates are about the reasoning, not precision. Round aggressively, write down assumptions, and label every unit.

## Method

1. **List assumptions** (labeled): hosts, appliances per host, assemblies, users, API calls per user per day, reconcile interval, size of an assembly document, size of a manifest.
2. **Derive rates**: requests per second = daily calls / 86,400; peak ≈ 2–3× average. For control loops: objects × (1 / interval) = reconciles per second.
3. **Derive storage**: record size × count × retention, plus snapshots/audit logs/metrics. Add replication factor.
4. **Derive fleet size**: required throughput / per-node throughput, then add headroom and redundancy (N+1).
5. **Check the result against a sanity bound**: does one Controller instance and one database comfortably handle it? If yes, say so and stop; do not add complexity.

## Reference numbers to reason with

- Memory access is nanoseconds, SSD reads are tens to hundreds of microseconds, a disk seek is milliseconds, a cross-region round trip is tens to hundreds of milliseconds. Keep hot paths off disk and off the wide-area network.
- Powers of two: KB, MB, GB, TB, PB are successive factors of about 1,000 for estimates.
- Availability: 99.9% is about 8.8 hours of downtime a year; 99.99% is about 53 minutes. Say which target applies to which component.

## Apelora-specific questions

- Reconcile cost: objects per Controller, API calls to each node per cycle, and what happens at 10× objects
- Document size limits for imported assemblies (cap in code to match the estimate)
- Event/log volume per node and its retention
- Image storage and transfer when many nodes pull the same appliance

## Output

A table of assumptions, formulas, results, and the conclusion ("one Controller + one Postgres is enough up to X; revisit at Y"). Save as `docs/design/capacity-<topic>.md`.
