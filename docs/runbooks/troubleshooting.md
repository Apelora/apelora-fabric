# Troubleshooting with logs and the audit trail

**Evidence:** the log lines and request ids below were produced by the control plane with the
mock provider (2026-10-21); the worker and Incus lines are covered by unit and contract tests
(MOCK VERIFIED / CONTRACT VERIFIED). They have not yet been read off a real cluster incident.

Both services, `apelora-control-plane` and `apelora-deployment-worker`, write one line per event
to standard output. The audit trail (`GET /api/v1/audit`, Fabric Console → Audit) is a
separate, append-only record of who did what. Use the logs to find out **why** something
happened; use the audit trail to find out **who** asked for it and what was decided.

## Choosing what is logged

`RUST_LOG` selects what is written; without it both services log at `info`. Directives are
comma-separated `target=level` pairs, the most specific one wins:

| Goal | `RUST_LOG` |
|---|---|
| Normal operation | unset, or `info` |
| Every API request with status and latency | `info,apelora_control_plane::api::requests=debug` |
| Why the API refused something (code and message of every 4xx) | `info,apelora_control_plane=debug` |
| Every Incus request (method, path, status, latency) | `info,apelora_incus_adapter=debug` |
| Worker decisions (lifecycle steps, moves, supervisor skips) | `info,apelora_deployment_worker=debug` |
| Only problems | `warn` |
| Database driver detail | add `sqlx=debug` (verbose: every statement) |

An invalid `RUST_LOG` is reported on standard error and the service logs at `info`.

`APELORA_LOG_FORMAT=json` writes one JSON object per line (`timestamp`, `level`, `target`,
`fields` with `message` and the structured fields, and `span` with the request or job the line
belongs to). The default, `text`, is meant for a terminal. Ship JSON to a log store.

## The request id

Every API response has an `x-request-id` header. A caller may send its own (at most 64
characters of letters, digits, `.`, `_`, `-`); anything else is replaced by a new UUID. The id:

- is on every log line written while the request is handled, in the `request` span together
  with the method and the path (never the query string);
- is added as `requestId` to the detail of every audit event recorded during the request;
- is named in the message of a `500 store_error`, so a person reporting a failure can quote it.

To follow one request: take the id from the browser's network panel (or the error message) and
search the logs for it, e.g. `grep 'request_id=3f2a…'`, or `jq 'select(.span.request_id=="3f2a…")'`
for JSON logs. Then look for the same id in the audit detail.

Worker lines carry a `job` span instead: `job` (id), `kind`, `target` (`project/name` or the
application) and `request_id`, the id of the request that queued the job (`-` for jobs the
supervisor or the scheduler queued). `GET /api/v1/jobs/{id}` shows the same job's status and
last error.

### From a request id to the job and the Incus operation

**Evidence:** CONTRACT VERIFIED with the mock provider and a real PostgreSQL (2026-10-21,
`one_request_id_links_the_request_its_audit_row_the_job_and_the_incus_operation`); not yet
followed on a real cluster incident.

1. **Request → job.** The audit row of the request (`<kind>.requested`, `application.deploy.requested`,
   `application.<action>.requested`) has `requestId` and, for single jobs, `jobId` in its detail.
   The job itself stores the id: `GET /api/v1/jobs/{id}` and `GET /api/v1/jobs` return
   `requestId`, and the Console's Operations view shows it under the change, with a copy
   button. In the database: `SELECT id, kind, status FROM jobs WHERE request_id = '3f2a…'`
   (indexed). An application start, stop or restart queues a coordinating
   `application.operation` job, and every instance job it queues carries the same request id.
2. **Job → worker log lines.** Search for the job id or the request id: the worker's `job` span
   has both, e.g. `jq 'select(.span.request_id=="3f2a…")'`.
3. **Job → Incus operation.** `incusOperations` on the job lists every Incus operation it started,
   oldest first; `incusOperation` is the one the current attempt is still waiting on. The worker
   logs `started operation operation=…` (lifecycle jobs) or `move: waiting on the Incus operation
   operation=…` (moves) inside the job span. On a member, `incus operation show <id>` shows what
   Incus did while it still keeps the operation; Incus forgets finished operations after a
   while, the id in Apelora stays. Operations an adapter call waits on internally (volume
   copies, backups) are logged at debug as `incus: operation succeeded operation=…` in the job
   span but are not yet listed on the job.

## Logged or audited

| Event | Log | Audit |
|---|---|---|
| A request completes | debug (2xx/3xx), info (4xx), warn (5xx), with status, latency and error `code` | — |
| No or unknown credential (401) | debug (no token) / warn `credential refused` with the credential kind and, for provider tokens, the issuer | not audited: anyone could fill the trail with them |
| Refused by permission (403: `forbidden`, `mfa_required`, `route_not_authorizable`, `account_disabled`) | warn `request denied` | `access.denied` with `code` and `reason` |
| Action that needs an approval, none presented (`approval_required`) | info | not audited: it is the normal first step of the approval flow |
| Approval presented that is spent, someone else's or for another action (`approval_invalid`) | warn `approval refused` | `access.denied` with `code: approval_invalid`, the approval id and permission |
| Every change made through the API | info where useful | always, with actor, target and outcome |
| The audit write itself fails | error `audit: could not record <action>` | — (that is the failure) |

Never in logs, audit detail or error messages: bearer tokens, API and service tokens, Incus join
tokens, certificates and keys, passwords, the `Authorization` header, request bodies. Only the
**kind** of a refused credential is logged. Values read from an unverified token (issuer,
audience, key id) are public identifiers; they are shown without control characters and
shortened to 120 characters.

## Common problems and the lines to look for

### Cannot sign in

- `WARN credential refused credential="identity-provider token" issuer=… code="invalid_token" reason=…`
  The reason says what did not match, for example
  `the token was issued by a different identity provider (https://id.example/realms/x; expected https://id.example/realms/main)`
  (a trailing slash or another realm), `the token is not meant for this API (audience apelora-console; expected one of apelora-api)`
  (the provider puts the client id, not the API's identifier, in the audience), or
  `signed with a key the identity provider does not publish (key id …)` (keys rotated; they are re-read at most once a minute).
- `credential="API token" … the API token is unknown, revoked or expired`, or `legacy API tokens are disabled`.
- `request denied code="account_disabled"`: the person's account is turned off (also audited as `auth.refused`).

### The identity provider is not reachable

- At start and after a change: `WARN identity provider not reachable: its tokens are refused until it answers (retried every minute) provider=… issuer=… error=…`.
- When it answers again: `INFO identity provider accepted for sign-in`.

### Incus is unreachable or refuses Apelora

- API answers `502 incus_unreachable` / `incus_authentication` / `incus_api_error`; the access log
  line is `WARN request failed code="incus_unreachable"` and the message names the Incus request,
  e.g. `cannot reach the Incus server: GET /1.0/instances?recursion=1&all-projects=true: … Connection refused`.
- `HTTP 401`/`403` in the message: the client certificate is not trusted or not allowed in the
  project (see [deploy-identity.md](deploy-identity.md), [certificates.md](certificates.md)).
- `HTTP 502 with a reply that is not from the Incus API`: something between Apelora and Incus
  (a proxy or load balancer) answered instead of Incus.
- With `RUST_LOG=info,apelora_incus_adapter=debug`: `DEBUG incus: response request="GET /1.0/…" status=200 latency_ms=12`
  for every call, and `incus: request not sent or not answered` with the cause chain (TLS
  verification, DNS, timeout).

### `429 rate_limited` (too many requests)

The answer has `Retry-After` (seconds) and the code `rate_limited`. The first refusal while a
limit is spent is logged at warn in the request's span (with its request id; never a token):
`WARN rate limit: refusing requests limit="public" client=203.0.113.7 retry_after_secs=1`.
Further refusals are logged at debug, so a flood does not flood the log.

- `limit="public"`: one address called the public routes (`/health`, `/sign-in`, the OpenAPI
  document, the terminal websocket) or sent requests without a token too often
  (`APELORA_RATE_LIMIT_PUBLIC_*`). A load balancer probing `/health` every 2 s uses 30 a minute.
- `limit="failed-sign-in"`: tokens from this address were refused repeatedly; until the bucket
  refills, every request with a token from it is refused before the token is checked
  (`APELORA_RATE_LIMIT_FAILED_AUTH_*`). Look for the `credential refused` lines before it.
- `limit="writes"` with `client=user:…`/`service:…`/`token:…`: one caller sent many changes
  (`APELORA_RATE_LIMIT_WRITES_*`), typically a script in a loop.
- **Every browser is limited together:** the control plane sits behind a proxy and sees only the
  proxy's address. Set `APELORA_TRUSTED_PROXIES` to the proxy's address; the start-up warning
  `APELORA_BEHIND_PROXY is set without APELORA_TRUSTED_PROXIES` says so too.
- The limits are per API process (in memory); with several processes the effective limit is
  that many times larger.

### The control plane refuses to start (listening and TLS)

- `refusing to serve plain HTTP on 0.0.0.0:8080: bearer tokens would cross the network unencrypted …`:
  `APELORA_API_ADDR` is not a loopback address. Set `APELORA_TLS_CERT` and `APELORA_TLS_KEY`
  (PEM files), or put a TLS-terminating proxy in front (`infra/serve/apelora.conf`) and set
  `APELORA_BEHIND_PROXY=1` (and `APELORA_TRUSTED_PROXIES`), or listen on `127.0.0.1`.
- `APELORA_TLS_CERT is set but APELORA_TLS_KEY is not` (or the reverse): set both or neither.
- `APELORA_TLS_CERT (/path): cannot read a PEM certificate chain: …` / `APELORA_TLS_KEY (/path): cannot read a PEM private key: …`:
  the file is missing, unreadable for the service user, or not PEM.
- `APELORA_TLS_CERT/APELORA_TLS_KEY do not form a usable pair`: the key does not belong to the
  certificate, or the key type is not supported.
- `APELORA_BEHIND_PROXY must be 1 … or 0`, `APELORA_TRUSTED_PROXIES: "…" is not an address or CIDR range`,
  `APELORA_RATE_LIMIT_… must be a whole number …`: a setting could not be read.
- After start, `control plane listening … tls=true` confirms HTTPS. Failed handshakes are logged
  at debug (`tls: handshake failed`), so a client that cannot connect is visible with
  `RUST_LOG=info,apelora_control_plane=debug`.

### `/health` says Incus is not connected, but not why

The public `/api/v1/health` says only whether Incus answered (asked at most every 15 s). The
reason and the versions are on `GET /api/v1/system` (Fabric Console → Administration → System,
signed in) and in the log: `WARN health: Incus is not reachable error=…` once when it stops
answering, `INFO health: Incus is reachable again` when it is back.

### A job fails or keeps being retried

- `INFO job: claimed attempt=2 max_attempts=5` in the `job{job=… kind=… target=…}` span.
- `WARN job: attempt failed; will be retried attempt=2 max_attempts=5 retry_in_secs=4 error=…`:
  retried with exponential back-off (at most 300 s).
- `ERROR job: failed; no attempts left` or `ERROR job: rejected; retrying cannot help` (for
  example the instance is not Apelora's): the job is `failed` and `last_error` holds the reason.
- `WARN incus: operation failed operation=… error=…` or `incus: operation still running when the wait ended`
  just before: Incus itself reported the failure, or the operation is slower than the wait; the
  next attempt waits on the same operation.
- `INFO network: not everything has an address yet; the sync is retried`: normal while
  instances start; a `network.sync` job that ends `failed` with `waiting for …` means an
  instance never got an address.
- `ERROR job: database error while running the job`: PostgreSQL was unavailable; the job is
  taken over once its 30 s lease expires.
- `WARN job: stopped working on it (another worker holds it now)`: the lease was lost (a pause
  longer than the lease); the other worker's result counts.

### An operation (start, stop, restart) reports a failed phase

- `WARN operation: phase failed; later phases skipped operation=… phase=0 action=start instance=apelora/shop-db-01-r1 reason=…`.
  The coordinating job still ends `succeeded`: the operation, not the job, records the failure
  (`GET /api/v1/applications/{name}/operations/{id}`).
- `WARN operation: cannot read the instance from Incus to confirm its state; looking again`:
  the phase waits instead of guessing.

### Queued jobs start only after a delay

Workers are woken by PostgreSQL `NOTIFY apelora_jobs` when a job is queued, and check every 2 s
in any case. A job that starts about 2 s late means the wake-up is not working:

- `WARN job wake-up: cannot listen for queued jobs; checking for work on the idle interval until it can`
  (at start) or `WARN job wake-up: the listener failed; …`: the listening connection was refused
  or lost; `INFO job wake-up: listening for queued jobs again` follows once it is back.
- `WARN job wake-up: the listener's connection dropped and was re-established; checking for work now`:
  a connection reaper or a database restart cut it; nothing to do unless it repeats.
- A connection pooler in transaction mode between the worker and PostgreSQL does not deliver
  notifications: connect the worker directly or in session mode.

### The supervisor does not restore something

- Only one worker supervises: the holder of the `replica-supervisor` leader lease. Look for
  `INFO leader: lease acquired; acting as the leader lease=replica-supervisor holder=<worker> generation=<n>`
  and `INFO leader: lease lost …`. In the database: `SELECT holder, generation, expires_at FROM
  leader_leases`. Workers that do not hold it skip their passes (debug: `another worker holds
  the leader lease`). A holder that stops renewing is replaced 15 s after its last renewal; a
  worker that stops cleanly gives the lease up by letting it expire, so expect up to 15 s plus
  one supervision interval before the new leader acts. Set a distinct `APELORA_WORKER_ID` per
  worker so the holder names the process.
- `WARN supervisor: pass stopped by a refused write`: a leader that stalled past its lease
  tried to write after another worker took over; nothing was written. Normal after a long pause
  (a suspended VM, a database outage); the new leader's next pass acts instead.
- `WARN leader: cannot renew the lease; writes stay fenced until it is renewed`: the database
  is not reachable; the supervisor writes nothing until it is.

- `WARN supervisor: instance is missing or stopped; only stateless replicas are restored automatically`
  (logged again when the state changes, at most every 10 minutes otherwise).
- `WARN supervisor: cannot read the cluster members; offline detection and substitutes are skipped until they can be read`.
- With `apelora_deployment_worker=debug`: why a replica was skipped (`a job for the instance is in flight or failed recently`,
  `stopped on purpose through Apelora`).

### The database

- `503 store_unavailable` and `WARN store: the database is not reachable`: PostgreSQL is down,
  refusing connections or out of connections; check `DATABASE_URL` and the server.
- `500 store_error` (the message names the request id) and `ERROR store: database error error=…`
  in the log line with that id: the database refused one statement; the log has its text.

### The audit trail is not being written

- `ERROR audit: could not record access.denied error=…` (or another action). API changes
  answer with an error when their audit event cannot be written (the change itself may already
  be stored: look before repeating it), and terminal sessions are not opened. Refusals and
  session ends are still carried out and only logged. Check the database and the
  `audit_events` table.
