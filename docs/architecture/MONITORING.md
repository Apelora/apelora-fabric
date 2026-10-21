# Monitoring: metrics for every layer, collected inside the installation

Status: **built for Apelora's own processes (CONTRACT/MOCK VERIFIED); collection from lower layers documented** · Date: 2026-10-21 · Builds on: [connections and scale](CONNECTIONS-AND-SCALE.md), [one design at every size](EVERY-SIZE.md) (ADR 0011), [disconnected installations](DISCONNECTED.md) (ADR 0010)

Operators watch an Apelora installation with the metrics collector they already run **inside** it. Apelora exposes its own metrics in the standard text exposition format. Every layer below it (Incus, instances, PostgreSQL, OVN, shared storage) is collected **directly from that layer**, never through Apelora. That way Apelora is never a bottleneck or a single point of failure for metrics, and nothing leaves the site.

## 1. Principles

1. **Each layer reports for itself.** Apelora measures what only Apelora knows: requests, decisions, jobs, its calls to Incus and its database pool. Incus measures servers and instances. PostgreSQL measures itself.
2. **Cheap at every size.** Instrumentation sits at choke points only: the request middleware, the job runner, the Incus HTTP client, the supervisor loop and the scrape-time readers. State read from PostgreSQL or Incus for metrics is refreshed **when a collector scrapes, at most every 15 s (database) and 30 s (inventory) per process**, never per API request.
3. **Low cardinality, no personal data.** Label values come from small fixed sets: route templates (`/api/v1/applications/{name}`), endpoint templates (`/1.0/instances/{id}/state`), status classes (`2xx`), outcome classes (`ok`, `unreachable`), catalogue permission ids and job kinds. A label never carries a raw path, a name, an id, an email address, a token or an error text. Each family keeps at most 1,000 label sets; anything beyond that is counted under one series whose labels are `_overflow`, so a mistake shows up instead of using unbounded memory.
4. **Summaries roll up (ADR 0011 §2.6).** Every series carries the constant labels `region`, `site` and `sector` (`APELORA_REGION`, `APELORA_SITE`, `APELORA_SECTOR`; default `default`). A regional collector aggregates by them without relabelling, and the global level only ever sees the aggregates.
5. **Honest about the source.** `apelora_provider_info{mode="mock"}` marks a process that talks to the mock, so mock numbers are never mistaken for a cluster.

## 2. Where to scrape Apelora

| Endpoint | Default | Authentication | Contents |
|---|---|---|---|
| Control plane `GET /metrics` on the API address | always on | bearer token holding **`fabric.metrics.read`** | everything below except the worker's own counters |
| Control plane separate listener `GET /metrics` | off; `APELORA_METRICS_ADDR=host:port` turns it on | **none** | the same exposition |
| Worker `GET /metrics` and `GET /healthz` | `127.0.0.1:9471` (`APELORA_WORKER_METRICS_ADDR`; `off` turns it off) | **none** | job runner, supervisor, schedules, Incus calls and pool of that worker |

**Default and why.** The safer default is the authenticated endpoint. It uses the API's TLS and the route gate, and refusals are audited like any other 403. Unauthenticated listeners exist only for collectors that cannot hold a token; they bind to loopback by default, and a non-loopback address is logged as a warning at start-up. Put them on a management network only.

**The collector's identity.** Create an *automation* service identity and give it the built-in role **Metrics Reader** (`metrics-reader`) at platform scope. That role holds only `fabric.metrics.read`: the collector cannot read inventory, change anything or see people. Fabric Administrator, Viewer and Auditor (at platform scope) also hold the permission. A legacy `apl_` token can scrape when its role reads inventory (every legacy role does), because metrics reveal less than `cluster.view`. The Console's **Administration → Monitoring** panel shows the endpoints, whether the signed-in caller may scrape, and the labels in use (`GET /api/v1/monitoring`, which needs `fabric.metrics.read` or `cluster.view`).

**Format.** By default the answer is `text/plain; version=0.0.4`. A collector that sends `Accept: application/openmetrics-text` gets the OpenMetrics variant (counter families named without `_total`, ending with `# EOF`). Answers carry `Cache-Control: no-store`.

**Why our own registry.** The crate `apelora-metrics` (a few hundred lines with tests, no dependencies) holds counters, gauges, histograms and the exposition writer. Three things led to it:
- Three crates (the Incus adapter, the identity crate and the persistence crate) must record into one registry, and Apelora needs neither push, exemplars nor dynamic registration.
- A hand-written registry adds no third-party code to review, licence or mirror for air-gapped sites.
- The cardinality cap is part of the registry itself.

The worker's listener uses the HTTP stack the control plane already uses (no new crate in the tree). If we later need features we do not have, a permissive, well-maintained crate can replace the registry behind the same family declarations.

## 3. Apelora's metric families

All names start with `apelora_`. Counters end in `_total`, durations are in seconds, and timestamps are Unix seconds. The declarations, with their help text, are in `crates/metrics/src/lib.rs`.

| Layer | Family (labels) | Type | Source |
|---|---|---|---|
| API | `http_requests_total` (route, method, status) · `http_request_duration_seconds` (route, method) · `http_requests_in_flight` · `http_rate_limited_total` (route) | counter · histogram · gauge · counter | request middleware (`api/requests.rs`); every 429 counts as rate limited, whichever limiter sent it |
| API | `auth_attempts_total` (method = oidc, service, api_token, none, development, unrecognised; result = ok, refused, error) · `authz_denials_total` (permission; `no_route_rule`; `other`) | counters | authentication and the authorisation refusals (`api.rs`, `authz.rs`) |
| Database | `store_errors_total` (class = store_unavailable, store_error) | counter | the API's error mapping and the worker's job loop |
| Database | `db_pool_connections` (state = in_use, idle, max) · `db_pool_acquire_seconds` · `db_up` | gauge · histogram · gauge | sampled at scrape (one acquire, 2 s limit) in each process |
| Jobs | `jobs` (kind, status = pending, running) · `jobs_oldest_runnable_age_seconds` · `jobs_expired_leases` | gauges | PostgreSQL at scrape (control plane; the same in every API process, so aggregate with `max`) |
| Worker | `worker_job_attempts_total` (kind, outcome = succeeded, retry, failed, rejected, waiting, lost) · `worker_job_duration_seconds` (kind, outcome) · `worker_lease_lost_total` (stage = renew, handler, complete) | counters · histogram | job runner (`deployment-worker/src/lib.rs`). A takeover shows on the old worker as `lease_lost` and in the queue as `jobs_expired_leases` |
| Supervisor | `supervisor_actions_total` (action = recreate, restart, substitute, retire, health_restart; outcome = queued, deferred) · `supervisor_pass_duration_seconds` (result) · `supervisor_last_pass_timestamp_seconds` | counter · histogram · gauge | supervisor loop |
| Storage | `volume_schedule_runs_total` (kind = snapshot, backup; outcome = ok, failed, not_configured) · `volume_last_backup_timestamp_seconds` (pool) | counter · gauge | storage schedule; newest backup per pool from PostgreSQL |
| Incus calls | `incus_requests_total` (endpoint, method, outcome) · `incus_request_duration_seconds` (endpoint, method) · `incus_operation_wait_seconds` · `incus_up` · `incus_last_success_timestamp_seconds` | counter · histograms · gauges | the one HTTP client to Incus (`incus-adapter/src/client.rs`), in both services |
| Inventory | `incus_members` (status) · `incus_instances` (status, kind) · `applications` (status = derived status) · `inventory_last_read_timestamp_seconds` | gauges | control plane at scrape, at most every 30 s: one member list and one instance list, the same derivation as the Applications page |
| Certificates | `certificate_expiry_timestamp_seconds` (certificate = deploy, admin, server) | gauge | the certificates the System panel shows |
| Identity | `identity_key_fetches_total` (source = discovery, key_set; result) | counter | the identity provider client (`crates/identity`) |
| Process | `build_info` (component, version, commit) · `provider_info` (mode) · `process_start_time_seconds`; on Linux also `process_cpu_seconds_total`, `process_resident_memory_bytes`, `process_open_fds`, `process_max_fds` | gauges, counter | `/proc/self` at scrape |

Health and capacity views (per-member load, pool fill) belong to the Health work and to Incus's own metrics (§4). These families do not copy them.

## 4. Layers Apelora does not own: collect them directly

**Incus servers and instances.** Each Incus server exposes `GET /1.0/metrics` in the same text format. It reports CPU, memory, disk, file system and network per instance, plus internal figures: member status, roles, running operations, active warnings, storage pool size and use, and daemon uptime. **In a cluster each server reports only the instances it runs, so scrape every server.** Set-up, from the Incus documentation:
1. Expose only the metrics endpoint on a management address: `incus config set core.metrics_address=<address>:8444` on each server. `core.https_address` would expose the whole API instead.
2. Create a key and a self-signed client certificate for the collector (`metrics.key`, `metrics.crt`) with your usual X.509 tooling; an elliptic-curve key is fine. Keep the key on the collector's host only.
3. Trust it **for metrics only**: `incus config trust add-certificate metrics.crt --type=metrics`. A metrics certificate cannot manage instances or anything else, so a leaked collector credential reads numbers and nothing more.
4. Point the collector at `https://<server>:8444/1.0/metrics` on every server. Use the cluster certificate as the CA and the name it carries as the TLS server name. A `project` parameter limits the answer to one project; without it, all projects are reported.

Never turn off metrics authentication (`core.metrics_authentication=false`). Never give the collector Apelora's deploy or admin certificates.

**PostgreSQL.** Read its statistics views with a dedicated read-only role:
- connections by state (`pg_stat_activity`);
- transactions, rollbacks, conflicts and deadlocks (`pg_stat_database`);
- replication lag per standby (`pg_stat_replication`) and on the standby (`pg_last_xact_replay_timestamp()`);
- WAL archiving success and failures (`pg_stat_archiver`);
- database and table size;
- long-running transactions.

Apelora's own view of the database is `apelora_db_*` and `apelora_store_errors_total`.

**OVN.** On the members that run the central databases, check cluster health with each database's cluster status (`ovn-appctl -t <database control socket> cluster/status <database>`): role, leader, term, and connected servers. On every member, check that `ovn-controller` is running and connected to the southbound database, and read the datapath's flow and drop counters. Apelora only sees OVN through Incus: `network.sync` job failures show up as `apelora_worker_job_attempts_total{kind="network.sync",outcome!="succeeded"}`.

**SAN and shared storage.** On every member, read: iSCSI or Fibre Channel session state, multipath path counts, the lock manager's lockspace state (sanlock) and the watchdog. Read pool use from Incus (`incus_storage_pool_used_bytes`). Use the same node agent your collector already uses for host metrics. Apelora adds nothing here until HA slice B uses the shared pool.

**Image mirrors.** Probe each mirror's HTTPS endpoint from inside the site, and watch its disk. Failed pulls appear in Apelora as failed `instance.create` attempts.

## 5. Which signals cover each layer

Layers as in [connections and scale §1](CONNECTIONS-AND-SCALE.md#1-what-runs-where). The thresholds are starting points; tune them to the installation.

| Layer | Source | Key signals | Alert when | Runbook |
|---|---|---|---|---|
| Browser (Console, Studio) | control plane | `http_requests_total{status="5xx"}` ratio; `http_request_duration_seconds` p95 for the routes pages load | 5xx above 2 % of requests for 5 min; p95 above 2 s for 10 min | [API outage](../runbooks/api-outage.md) |
| Control plane (API) | control plane `/metrics`; the load balancer's health check | scrape success; `http_requests_in_flight`; `auth_attempts_total{result="error"}`; `authz_denials_total` rate; `http_rate_limited_total`; `identity_key_fetches_total{result="error"}` | scrape fails for 2 min; in-flight above 100 for 5 min; any auth `error` for 5 min; a denial or 429 rate 10 times its weekly normal; key fetch errors for 10 min | [API outage](../runbooks/api-outage.md), [access revocation](../runbooks/access-revocation.md), [troubleshooting](../runbooks/troubleshooting.md) |
| Deployment worker | worker `/metrics`, `/healthz`; control plane queue gauges | `healthz`; `jobs{status="pending"}`; `jobs_oldest_runnable_age_seconds`; `jobs_expired_leases`; `worker_job_attempts_total{outcome=~"failed\|rejected"}`; `worker_lease_lost_total`; `supervisor_last_pass_timestamp_seconds` | liveness fails for 1 min; oldest runnable job waits over 5 min; expired leases for over 2 min; any failed or rejected attempt; supervisor silent for 3 passes (90 s by default) | [troubleshooting](../runbooks/troubleshooting.md), [replica unhealthy](../runbooks/replica-unhealthy.md) |
| PostgreSQL | its statistics views; `apelora_db_*`, `apelora_store_errors_total` | `db_up`; `db_pool_connections{state="in_use"}` versus `max`; `db_pool_acquire_seconds` p95; `store_errors_total`; replication lag; archiving failures | `db_up` is 0 for 1 min; in-use connections above 80 % of max for 10 min; acquire p95 above 0.5 s; any `store_unavailable`; lag above 30 s; any archiving failure | [troubleshooting](../runbooks/troubleshooting.md#the-database); backup and restore runbook still to write (EVERY-SIZE §5 item 3) |
| Incus members | each server's `/1.0/metrics`; Apelora's `incus_*` | `incus_up`, `incus_last_success_timestamp_seconds`; `incus_requests_total{outcome=~"server_error\|unreachable\|auth_error"}`; `incus_request_duration_seconds` p95; `incus_members{status!="Online"}`; from Incus: member status, warnings, running operations | `incus_up` is 0 for 1 min; no success for 5 min; any `auth_error`; p95 above 5 s; a member not Online for 2 min; two members not Online (quorum at risk on three); a certificate within 30 days of expiry | [member failure](../runbooks/member-failure.md), [quorum loss](../runbooks/quorum-loss.md), [member maintenance](../runbooks/member-maintenance.md), [certificates](../runbooks/certificates.md) |
| Instances (modules) | each server's `/1.0/metrics`; Apelora's `applications`, `supervisor_actions_total` | per instance: CPU, memory against its limit, out-of-memory kills, file system free, network errors; `applications{status=~"degraded\|failed\|unknown"}`; `supervisor_actions_total{outcome="queued"}` rate | memory above 90 % of its limit for 10 min, or any out-of-memory kill; file system above 90 % full; an application degraded for 10 min or failed at all; more than 3 supervisor actions per hour on one installation | [replica unhealthy](../runbooks/replica-unhealthy.md), [volumes](../runbooks/volumes.md) |
| OVN | OVN cluster status and controller state on the members; `worker_job_attempts_total{kind="network.sync"}` | database cluster role and leader; controller connected; failed `network.sync` attempts | no leader, or a database server disconnected, for 1 min; a controller disconnected for 2 min; `network.sync` failed with no attempts left | [network incident](../runbooks/network-incident.md) |
| SAN and shared storage | node agent on each member; Incus pool use | sessions and paths per member; lockspace state; watchdog; `incus_storage_pool_used_bytes` against size | a member lost all paths to the LUN; lockspace not joined; pool above 85 % | [volumes](../runbooks/volumes.md); fencing runbook arrives with HA slice B |
| Backups | `volume_schedule_runs_total`, `volume_last_backup_timestamp_seconds` | failed and `not_configured` runs; age of the newest backup per pool | any failed run; a backup pool's newest backup older than twice its schedule (36 h for a daily one) | [volumes](../runbooks/volumes.md) |
| Image mirrors | HTTPS probe from inside; mirror host disk | probe success; disk use; failed `instance.create` attempts | probe fails for 5 min; disk above 85 % | [troubleshooting](../runbooks/troubleshooting.md) |

## 6. At every size, and without internet access

- **Small installation:** one collector inside the site scrapes the control plane, the worker, every Incus server, PostgreSQL and the members' node agents. At 15–30 s intervals the extra load is negligible.
- **Several sectors, sites or regions:** each site's collector scrapes only its own site. The `region`, `site` and `sector` labels make roll-ups a plain aggregation. The regional level keeps aggregates; per-instance series stay in the site (ADR 0011 §2.6, residency).
- **Disconnected sites (ADR 0010):** the collector, its storage and its alerting run inside the site. Apelora sends nothing anywhere: there is no push and no telemetry, and metrics are only answered when the collector asks.

## 7. Adding a metric

See the observability skill (`.agents/skills/observability/SKILL.md`, "Metrics in Apelora today"): declare the family in `crates/metrics/src/lib.rs` and add it to `FAMILIES`. Record it only at a choke point. Use labels from small fixed sets, and extend the allowed label list in the crate's test only for a new small set. Add a test that the family appears.

## 8. Evidence and open points

- **Evidence:**
  - unit tests for the registry (escaping, histograms, the overflow cap, label rules);
  - contract tests for the Incus client (endpoint templates, outcomes, reachability);
  - API tests against a real PostgreSQL with the mock provider: who may scrape; the expected families after a request and a job through the real worker loop; no token, name, email address, id or key text in the output;
  - a worker test for the listener;
  - a Console Playwright test with a stubbed API.

  Labels: **CONTRACT VERIFIED** for the Incus call metrics, **MOCK VERIFIED** for the rest. Nothing here is REAL INCUS TESTED yet. The Incus collection steps in §4 come from the Incus documentation and have not been run on the test cluster.
- **Pool acquire time is sampled**, once per scrape. The database library offers no hook for every acquire, and a pool wrapper would touch every query. The sample shows a starved pool, but not a short burst.
- **Several API processes** expose the same queue and inventory gauges. Aggregate them with `max`, not `sum`.
- **Rate limiting:** any 429 is counted per route. A dedicated limiter can add its own reason label when it lands.
- **Request id ↔ metrics:** metrics carry no request id (that would be unbounded). Use the logs for single requests (EVERY-SIZE §2.8).
