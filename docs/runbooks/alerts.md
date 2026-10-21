# Alerts

Status: **NOT YET VERIFIED on the test cluster** (MOCK VERIFIED: evaluator, API, webhook delivery to a local HTTPS receiver, Console) · Design: [alerts](../design/alerts.md) · Related: [monitoring](../architecture/MONITORING.md), [troubleshooting](troubleshooting.md)

Apelora watches a fixed set of conditions about itself and its applications and raises **alerts** in the Fabric Console (`#/alerts`) and, if set up, at **webhooks**. This page says how to set them up, how to respond, and what to check for each rule. Built-in alerts do not replace your metrics collector ([MONITORING.md](../architecture/MONITORING.md)); they cover what Apelora itself knows.

## Set up

1. **Evaluator.** Every deployment worker can evaluate alerts; one at a time does, under the `alert-evaluator` lease. It is on by default every 30 s: `APELORA_ALERT_EVALUATE_SECS` (5–3600, `0` turns it off on that worker). If no worker evaluates, the Alerts page says so ("Alerts may be out of date").
2. **Restore drill record (optional).** To be warned when Apelora's own database restore has not been proven, make the drill's `restore-drill-last-verified.json` readable on the worker's host (for example the backup directory mounted read-only) and set `APELORA_RESTORE_DRILL_RECORD=/path/to/restore-drill-last-verified.json`. Without it the rule shows "Not set up".
3. **Webhook (optional).** Needs `alert.configure`.
   1. Choose a name for the signing secret's variable, e.g. `APELORA_ALERT_SECRET_OPS`. It must start with `APELORA_ALERT_SECRET_`.
   2. Put a long random secret into that variable on **every worker** (your secret manager, a systemd credential or the service's environment file; never in the repository). Restart the workers.
   3. Console → Alerts → Channels → **Add webhook**: name, `https://` URL, the variable's **name**, which severities to send. Apelora stores the name only.
   4. **Send test**. Within seconds the channel shows **Delivered**, or **Failing** with the reason.
4. **Receiver.** For each request, check:
   - `Apelora-Signature: v1=<hex>` equals HMAC-SHA256(secret, `<Apelora-Timestamp>.<raw body>`), compared in constant time;
   - `Apelora-Timestamp` is within five minutes;
   - `Apelora-Event-Id` was not seen before (delivery is at least once; drop duplicates).
   The body is JSON: `version`, `event` (`fired`, `resolved`, `test`), `alert` (id, rule, title, subject, severity, summary, value, threshold, unit, times) and `installation` (region, site, sector). Answer 2xx quickly; anything else is retried with backoff (2 s doubling to 5 min). Redirects are not followed.

## Respond

- **Acknowledge** (`alert.respond`): tells others you are handling it; notifications continue. Recorded with your name, the time and your note; audited as `alert.acknowledge`.
- **Silence** (`alert.respond`): pauses notifications for one subject or the whole rule for 30 minutes to a week (planned maintenance, a known issue). The alert is still evaluated and shown. **End silence** stops it early. Audited.
- **Tune a rule** (`alert.configure`): Console → Alerts → Rules → Edit. Change the threshold (within the rule's bounds), the wait (seconds the condition must hold before firing and stay clear before resolving), the severity, or turn it off. Turning a rule off resolves its alerts. Audited as `alert.rule.update` with before and after.
- An alert **resolves on its own** once its condition has stayed clear for the rule's wait. Resolved alerts stay in the history for 90 days.

## When a channel shows "Failing"

| Reason shown | Do |
|---|---|
| `secret APELORA_ALERT_SECRET_… is not set on the worker` | Set the variable on every worker and restart them. The waiting messages are retried. |
| `could not connect to the receiver` / `did not answer within 5 s` | Check the receiver is up and reachable from the workers (firewall, DNS, proxy). |
| `the receiver answered 401/403` | The receiver refused the signature or token: compare secrets; check its clock (timestamps older than five minutes are refused). |
| `the receiver answered 3xx (a redirect…)` | Use the final URL; Apelora does not follow redirects. |
| TLS errors | The receiver's certificate must be valid for its name and trusted by the workers' system trust store. |

Messages for a deleted or turned-off channel are dropped. Undelivered messages are never dropped otherwise; the `outbox-stuck` rule fires when they wait more than 10 minutes.

## Rules

### Jobs are failing
`job-failures` · subject: job kind. Jobs of that kind ended failed in the last 15 minutes. Open Operations, filter by the kind, read the job page's attempts and Incus operations; retry once the cause is fixed. See [troubleshooting](troubleshooting.md#retrying-and-cancelling-a-job).

### Jobs are waiting too long
`job-queue-wait`. No worker is claiming jobs, or all are busy. Check the Health page (workers), the worker logs, and the `jobs_oldest_runnable_age_seconds` metric. See [API outage](api-outage.md).

### Running jobs lost their worker
`job-leases-expired`. A worker stopped renewing a running job's lease (crash, pause, lost database). Another worker takes it over; if none runs, start one.

### A worker stopped reporting
`worker-silent` · subject: worker id. The worker did not stop cleanly and has not written its heartbeat. Check its host and service; if it is gone for good, its row disappears after a day.

### Nobody holds a leader role
`leader-missing` · subject: `replica-supervisor`, `outbox-relay` or `alert-evaluator`. No worker plays that role. Start a worker, or check why the role is off (`APELORA_SUPERVISE_SECS=0`, `APELORA_OUTBOX_SINK=off`, `APELORA_ALERT_EVALUATE_SECS=0`). If it is off on purpose, turn this rule off.

### The supervisor is acting often
`supervisor-actions`. The supervisor recreated, restarted, substituted or retired replicas several times in an hour, which usually means one cause keeps coming back. Read the audit trail (actor `supervisor`) and [replica unhealthy](replica-unhealthy.md).

### A replica is unhealthy
`replica-unhealthy` · subject: `project/instance`. Its port check keeps failing. See [replica unhealthy](replica-unhealthy.md).

### An application is failed or degraded
`application-status` · subject: application. Its last start, shutdown, stop, restart or migration failed, or fewer instances run than it should have. Open the application in the Console; start or restart it once the cause is fixed.

### Incus cannot be reached
`incus-unreachable`. The worker cannot read the cluster. Deployments, moves and recovery wait. Rules that read Incus keep their alerts as they were. See [member failure](member-failure.md), [quorum loss](quorum-loss.md) and the worker's `APELORA_INCUS_*` settings.

### A cluster member is not online
`member-offline` · subject: member. See [member failure](member-failure.md); two of three members offline puts quorum at risk ([quorum loss](quorum-loss.md)).

### A failed server is waiting for fencing
`member-unfenced` · subject: member. The server is Offline and holds modules on shared storage that Apelora restarts elsewhere only once it is fenced (ADR 0013). Normally fenced about 140 s after it stopped (its storage lease expires). Servers → Members says why it waits: the lease is still renewed (the server may be cut off from the others but still running), the probe cannot reach a healthy server, or no probe is configured. See [member failure](member-failure.md#modules-on-shared-storage).

### A certificate expires soon
`certificate-expiry` · subject: `deploy` (Apelora's Incus identity) or `server` (the pinned Incus server certificate). See [certificates](certificates.md). The admin identity's certificate is loaded only by the control plane; watch it with the `apelora_certificate_expiry_timestamp_seconds` metric.

### A scheduled snapshot or backup failed
`backup-failed` · subject: `project/volume snapshot|backup`. Read the reason on the alert; see [volumes](volumes.md). "Not configured" means no backup pool is set (`APELORA_BACKUP_POOL`).

### A backup is overdue
`backup-age` · subject: `project/volume`. The newest backup is older than the threshold in schedule periods. Check the volume's schedule and the backup pool's space; see [volumes](volumes.md).

### Messages are not being delivered
`outbox-stuck`. The oldest undelivered outbox message (audit copies, alert notifications) has waited too long. Check that a worker holds `outbox-relay`, that `APELORA_OUTBOX_SINK` is not `off`, and the Channels tab for failing webhooks. See [troubleshooting](troubleshooting.md#the-outbox-backlog-grows).

### Apelora's database restore is not proven
`restore-drill-age`. No restore drill has passed for the threshold in days. Run the drill: [database backup](apelora-database-backup.md).
