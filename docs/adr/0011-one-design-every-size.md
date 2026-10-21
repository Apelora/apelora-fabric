# ADR 0011 — One design at every size: same mechanisms from three servers to the global fabric

Status: **accepted; partly built** · Date: 2026-10-21 · Details: [one design at every size](../architecture/EVERY-SIZE.md) · Related: [ADR 0008](0008-sectors.md), [ADR 0009](0009-global-fabric.md), [ADR 0010](0010-disconnected-installations.md)

## Context

Apelora starts on three servers and is meant to grow to tens of thousands across regions (ADR 0009). There are two easy mistakes:
- build the small installation with shortcuts that the large one must undo: no audit in transactions, state kept in one process, polling where events are needed, a single scope tree for permissions;
- build the large one first and make the small one carry machinery it does not need.

Today the code has a few of the first kind:
- audit rows are written after the change, outside its transaction;
- sign-in settings, terminal tickets and the supervisor's work are tied to one process;
- the request id stops at the API: jobs and Incus operations do not carry it;
- Apelora's own database has no backup or restore procedure.

## Decision

1. **One set of mechanisms at every size.** Profiles (small, data centre, national, global) differ only in how many copies of each part run and where. No feature or control is switched off for small installations.
2. **Communication rules** that every part follows:
   - the plane calls in;
   - desired state is kept in PostgreSQL and changes run as jobs;
   - static stability;
   - events for speed and a periodic re-check for correctness;
   - one transaction, then an outbox;
   - summaries go up and intent goes down;
   - a budget and a per-sector bulkhead on every call;
   - one correlation id from the browser to Incus.

   Details are in the reference page, §2.
3. **The outbox** is the single way a change causes anything outside its transaction:
   - audit (written in the same transaction);
   - notifications and webhooks;
   - copies to other planes.

   Delivery is at least once; receivers ignore duplicates by id.
4. **Two scope trees** for permissions, checked against the tree the target belongs to:
   - tenancy: organisation → project → environment;
   - infrastructure: fabric → region → site → sector.
5. **Failure detection has one owner per layer**:
   - Incus for members;
   - the supervisor leader for instances;
   - leases for workers;
   - the database's own tooling for PostgreSQL;
   - the regional plane for sectors;
   - people for sites and regions.

   Nothing that writes to shared storage is restarted before fencing.
6. **Apelora's own database is backed up outside the cluster it manages, and restores are drilled.** The Console shows the age of the last verified restore.
7. **A small installation may run its control plane as system containers on the cluster it manages.** Workloads do not depend on the plane (static stability), and its database backup lives outside the cluster. From the data-centre profile on, the plane runs outside the sectors it manages.

## Consequences

- Wave 0 S4 becomes "audit in the same transaction, then the outbox"; alerts (wave 3) and replication between planes (ADR 0009) reuse the outbox instead of adding their own delivery.
- The supervisor gets a leader lease before a second worker is run.
- Jobs gain a `request_id`; the job detail page shows the chain.
- Sectors (ADR 0008) arrive with bulkheads and the infrastructure scope tree.
- Running the plane on its own cluster in the small profile means a cluster-wide outage also stops the plane. The runbook restores the plane from its off-cluster backup onto any Linux host.

## Alternatives considered

- **Separate small and large editions.** Less work at first, but two code paths to secure and test, and every customer who grows would need a migration.
- **A message broker between the parts.** It adds a component to run, secure, back up and mirror for air-gapped sites. PostgreSQL already provides transactions, `LISTEN/NOTIFY` and `SKIP LOCKED` queues, which cover the rates in the capacity estimate. A broker can be added per region later, behind the outbox, if a measured rate requires it.
- **Events only, no periodic re-check.** Faster and cheaper, but a missed event (a reconnect, a restart) would leave state wrong until something else changed it.
