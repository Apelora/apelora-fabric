# One design at every size: communication, control and recovery from three servers to the global fabric

Status: **reference and plan** · Date: 2026-10-21 · Decision: [ADR 0011](../adr/0011-one-design-every-size.md) · Builds on: [connections and scale](CONNECTIONS-AND-SCALE.md), [global fabric](GLOBAL-FABRIC.md), [disconnected installations](DISCONNECTED.md)

[Connections and scale](CONNECTIONS-AND-SCALE.md) lists every connection. [Global fabric](GLOBAL-FABRIC.md) covers distance: regions, sites, sectors. This page covers the third axis: how each concern behaves as the installation grows, and what to change in the code so that the three-server installation and the global fabric run **the same software, the same mechanisms and the same rules**, with only configuration and the number of copies differing.

"Built" below is checked against the code on the date above. Everything else is a plan.

## 1. Sizes (profiles)

| Profile | Servers | Sectors | Control plane | Database | What a failure costs |
|---|---|---|---|---|---|
| **Small** | 3–10 | 1 | 1 API process and 1 worker; may run as system containers on the cluster it manages (§6) | 1 primary, a standby recommended | one member: nothing (Incus quorum 2 of 3); the plane: changes wait, workloads keep running |
| **Data centre** | 10s–1,000 | 2–20 in one site | 2+ API processes behind a load balancer, 2+ workers; outside the sectors (a management sector) | primary + synchronous standby | one sector: only its applications |
| **National** | 1,000s | 20–200 over several sites | one regional plane | primary in one site, standby in another | one site: its sectors; the region keeps operating |
| **Global** | 10,000s | 1,000s | regional planes + a small global plane | per region; the global plane is replicated read-only to every region | one region: only that region; the global plane down: nothing stops (static stability) |

A profile is a **deployment shape, not a product edition**. No size has a feature switched off for convenience. Authentication, authorisation, audit and TLS are the same at three servers as at fifty thousand.

## 2. How the parts talk to each other

Eight rules hold at every size. The first three already hold; the rest is the plan in §5.

1. **The plane calls in; nothing calls out to the plane.** Members, instances and SANs never open connections to Apelora (built). This keeps the attack surface to one direction and lets a plane restart without anything waiting on it.
2. **Desired state in PostgreSQL, changes as jobs.** A request records intent and queues a job. Workers claim jobs with leases and fencing generations (built). This holds at every size, from one worker to hundreds sharded by sector.
3. **Static stability.** When a higher level is down, a lower level keeps doing its job with what it last knew (built for the plane over Incus: instances keep running, ACLs keep enforcing, OVN keeps forwarding). The regional and global planes follow the same rule (ADR 0009).
4. **Events for speed, a full re-check for correctness.** Today the supervisor and status views poll Incus every 30 seconds, and the worker checks for jobs every 2 seconds. Polling is cheap even at the global estimate: one list per sector every 30 s ≈ 1 request/s per region. The cost is **latency**, not load. The plan:
   - subscribe to each sector's Incus event stream (instance and operation events) to react within a second;
   - keep the periodic re-check as the safety net that corrects anything an event missed, because events can be lost on a reconnect;
   - wake workers through PostgreSQL `LISTEN/NOTIFY` when a job is queued, and keep the 2-second check as the fallback.
5. **One transaction, then an outbox.** A change, its audit row and every message it must cause (a notification, a copy to the global plane, a webhook) are written **in the same transaction**. A relay then delivers the outbox rows at least once, and receivers ignore duplicates by id. One mechanism solves three problems at once:
   - audit integrity: no change without its audit row, gap S4;
   - replication between planes;
   - alert and webhook delivery.
6. **Summaries go up, intent goes down.**
   - Each level sends the level above counts, worst states and capacity, each with the time it was measured. It never sends raw lists.
   - A person who needs detail is sent to the level that owns it.
   - Intent (a design, a policy, a role) flows down and is cached below with a version number.
7. **Every call has a budget.** A timeout, retries with backoff and jitter, and a concurrency limit **per sector** (a bulkhead), so one slow or broken sector cannot use up the workers or connections the others need. After repeated failures a sector is marked *unreachable*: its jobs wait, they do not pile up as retries, and the Console says so.
8. **One correlation id end to end.** The request id (built in the API) is stored on the job it queues, set on the job's log span, and recorded next to the Incus operation id and on the audit row. Then one id answers "what happened to my click", from the browser down to Incus. Across planes it travels as the standard `traceparent` header.

## 3. Concern by concern

Each row says what holds at every size, what changes as the installation grows, and what is built.

### Auditing

- **Every size:**
  - append-only, written in the same transaction as the change (§2.5);
  - denials are audited as well as successes;
  - no content of terminal sessions;
  - who, what, on what, outcome, request id.
- **Grows into:**
  - monthly partitions once the table reaches tens of millions of rows;
  - the regional audit stays in the region (residency);
  - the global plane keeps an archive of exported partitions;
  - a daily digest (a hash of the day's rows, chained to the previous day's) makes tampering detectable without special hardware;
  - export to write-once storage where the operator requires it.
- **Built:** the append-only trigger for `UPDATE` and `DELETE`; search by action and outcome.
- **Missing:**
  - same-transaction writes;
  - blocking `TRUNCATE`;
  - a test that every state-changing route audits;
  - audit rows for successful sign-ins;
  - filters by actor, target and time;
  - export;
  - partitions;
  - the digest.

### Security

- **Every size:**
  - mutual TLS to Incus with a certificate restricted to the deploy projects;
  - a second, unrestricted certificate only for cluster administration;
  - no secrets in the browser, logs or repository;
  - deny-by-default application networks;
  - instances cannot reach the Incus API;
  - TLS on the API, security headers and rate limits on public routes (wave 0, S3, in progress);
  - token roles limited to the creator's permissions (S1, in progress).
- **Grows into:**
  - one Incus client certificate per sector;
  - an intermediate certificate authority per region, so taking one region's credentials reaches nothing else;
  - rate limits shared per region instead of per process;
  - a security-posture page per region, rolled up globally.

### Roles and permissions

- **Every size:**
  - the same roles, scopes, approvals and emergency access;
  - deny wins;
  - assignments are audited;
  - nobody can grant what they do not hold (S1/S2, in progress).
- **Grows into: two scope trees, not one.**
  - Tenancy: organisation → project → environment (built).
  - Infrastructure: fabric → region → site → sector (planned with sectors).

  An operator of one site's sectors must not see or touch another region's servers. A tenant's environment must not care which sector it lands on. A permission is checked against the scope tree its target belongs to.
- **Speed:**
  - each process caches a person's effective permissions, keyed by a generation number;
  - any change to roles, assignments or groups bumps the number;
  - `NOTIFY` tells every process to drop its cache;
  - in a region, the cached copy of global assignments is invalidated the same way when replication delivers a change.

  Revocation reaches every process within seconds at every size.

### Performance and efficiency

| Resource | Small | Large | Optimisation |
|---|---|---|---|
| API | 1 process, tens of requests/s | several processes per region | stateless processes. Move the remaining per-process state to PostgreSQL (sign-in settings reload, terminal tickets, the supervisor's leadership), see [scaling issues](CONNECTIONS-AND-SCALE.md#4-scaling-issues-found-and-the-fix-for-each) |
| Worker | 1 | sharded by sector | per-sector bulkheads; `LISTEN/NOTIFY` wake-up; claim with `SKIP LOCKED` (built) |
| PostgreSQL | tiny: designs and jobs are kilobytes | per region; a primary handles the global estimate's write rate (capacity estimate) | indexes on every filter the Console offers; partitions for audit and events; a connection pool per process, sized from the worker count |
| Incus API | one sector | per sector, bounded by its database's write rate | events instead of tight polling; batch status reads (one list per project, not one call per instance); concurrency limit per sector |
| Network | one site | between sites and regions | summaries up, not lists; image caches per region; incremental copies for moves and backups (`--refresh`) |
| Browser | — | — | Console lists are paged on the server; detail is loaded on demand; live views refresh by interval with back-off when the tab is hidden |

Efficiency for the servers themselves:
- system containers by default, VMs only where a class needs them (built);
- capacity views and placement that refuse what does not fit (health and capacity work in progress);
- later, consolidation advice: members that could be emptied and powered down.

### Debugging

- **Every size:**
  - request ids (built);
  - structured logs with `APELORA_LOG_FORMAT=json` (built);
  - the correlation chain (§2.8);
  - a job detail page with each step, its duration and the Incus operation it waited on (wave 1);
  - log level changed at run time by an administrator, audited, and reverting by itself after an hour;
  - a support bundle: versions, effective settings with secrets removed, the last hour of errors, health, preflight, recent failed jobs (wave 4).
- **Grows into:**
  - logs and metrics stay in the region;
  - "look up a request id" asks the regional plane that owns it, which the id itself names (a region prefix);
  - the global plane keeps only roll-ups.

### Failure detection

Each layer detects its own failures, and each layer has one owner of the decision.

| What fails | Detected by | Within | Decided by | Action |
|---|---|---|---|---|
| A member | Incus heartbeats (`cluster.offline_threshold`, default 20 s) | ~20 s | Incus marks it offline; Apelora's supervisor acts on that | stateless replicas replaced on other members (built); stateful instances on shared storage restarted elsewhere **only after fencing** (slice B, not built) |
| An instance | Apelora health checks (supervisor, every 30 s) | 30–60 s; seconds once events are used | supervisor (one leader per plane, planned) | restart, then replace (built) |
| A worker | its lease expires (30 s); heartbeats (in progress) | 30 s | any other worker | takes the job over; the fencing generation stops the old one writing (built) |
| An API process | load-balancer health check | seconds | load balancer | traffic to the others |
| PostgreSQL primary | the database's own failover tooling | seconds | that tooling, never Apelora | standby promoted; Apelora reconnects and retries |
| A sector | calls to its Incus API fail repeatedly | ~1 min | the regional plane | the sector is marked unreachable; its jobs wait; its applications keep running |
| A site or region | the level above stops receiving its summaries | minutes | people, with a runbook | no automatic move of workloads across sites: a wrong guess over a wide-area network is worse than waiting |

Two rules prevent false alarms:
- one missed signal is never enough: a member is acted on only when Incus has marked it offline;
- an instance is replaced only after consecutive failed checks.

Two rules prevent split brain:
- fencing comes before restarting anything that writes to shared storage;
- every job writes with its fencing generation.

### High availability

- **Small (three servers):**
  - Incus keeps working with one member down (quorum two of three);
  - replicas are spread by anti-affinity (built);
  - stateful modules on the shared pool restart after fencing (slice B);
  - the plane itself can be two processes on two members with a database standby;
  - if the whole plane is down, workloads keep running and only changes wait (§2.3).
- **Large:**
  - a site's sectors in different failure domains (power, network);
  - a regional database standby in another site;
  - the global plane is never in a request's critical path (ADR 0009).

### Adaptability

The same build adapts to what the installation has, by **detection, not configuration flags**:
- preflight and the Console show which capabilities are present: OVN, shared pools, BGP, mirrors, a directory, GPUs;
- module classes declare what they need;
- placement and the Studio offer only what can run where.

Configuration changes take effect without a restart wherever possible, and are always audited. A capability that is absent says so, with the reason, and what to set up to get it.

### Backups and recovery

| What | Small | Large | Status |
|---|---|---|---|
| Application volumes | tier 0 snapshots on the member, tier 1 backups on another member's pool | tiers 2–3 in other sites and regions, inside the jurisdiction | tiers 0–1 built |
| **Apelora's own database** | a scheduled dump and continuous archiving to storage **outside the cluster it manages**; a restore drill that proves it | per region; global plane separately | dump with checksum, retention and optional encryption, and a restore drill into a scratch database: built, tested against a real PostgreSQL 18 (`scripts/ops/`, [runbook](../runbooks/apelora-database-backup.md)); continuous archiving: documented, not scripted |
| Incus cluster state | Incus recovers its own database from the members; instance definitions can be re-imported from the pools | per sector | Incus |
| Designs | versions in the database, export to files | same | built |
| Restore drills | a scheduled scripted restore into a scratch database, with the result in the Console | per region, results rolled up | script built (writes `restore-drill-last-verified.json`); Console view planned (wave 4) |

A backup that has never been restored is not counted as a backup. The Console shows the age of the last **verified** restore, not the last copy.

## 4. What stays the same, and what changes

| Stays the same at every size | Changes with size |
|---|---|
| API, roles, audit format, job model, module classes, designs, the Console and Studio | the number of processes and databases |
| the connection directions and their authentication | the number of certificates and authorities (per sector, per region) |
| static stability, fencing, deny-by-default | where summaries are aggregated |
| sign-in methods and air-gap support | which scope trees an operator is assigned in |

## 5. Work, in order

Each item is small enough to review on its own. **(N)** marks items needed now, at three servers.

1. **(N)** Audit in the same transaction, `TRUNCATE` blocked, a test that every state-changing route audits (S4). Then the outbox table and relay, so the next items reuse it.
2. **(N)** Correlation: `request_id` on jobs, carried into the job's span, the Incus operation id recorded beside it, both on the audit row.
3. **(N)** Backup and restore of Apelora's own database: a script, a restore drill, a runbook (built: `scripts/ops/db-backup.sh`, `scripts/ops/db-restore-drill.sh`, [runbook](../runbooks/apelora-database-backup.md)), and the age of the last verified restore in the Console (not yet).
4. **(N)** Supervisor leadership through a lease in PostgreSQL, so two workers can run (the first item before HA of the plane itself).
5. `LISTEN/NOTIFY`:
   - job wake-up;
   - sign-in settings reload;
   - permission cache invalidation.
6. Incus event subscription per sector, with the periodic re-check kept.
7. Per-sector bulkheads and the *unreachable* state, with sectors (ADR 0008).
8. The infrastructure scope tree for roles, with sectors.
9. Audit partitions, the daily digest and export (wave 1 adds filters; this adds integrity and retention).
10. Run-time log level, support bundle (wave 4).

Items 1, 3 and 4 are P0 in the [Console plan](../development/CONSOLE-PLAN.md). The rest follow sectors (ADR 0008) and the global plane (ADR 0009).
