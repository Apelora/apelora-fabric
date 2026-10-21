# Back up and restore Apelora's own database

Decision: [ADR 0011](../adr/0011-one-design-every-size.md) decision 6 (backed up outside the cluster, restores drilled) and decision 7 (a small installation may run its plane on the cluster it manages, so the plane must be restorable onto any Linux host). Scripts: `scripts/ops/db-backup.sh`, `scripts/ops/db-restore-drill.sh`, self-test `scripts/ops/test-db-backup.sh`.

**Evidence:**

- **Tested against a real PostgreSQL 18.6** (the local dev database from `infra/dev/compose.yaml`), 2026-10-21:
  - backup;
  - restore drill into a scratch database;
  - refusal of a damaged file;
  - encryption with `openssl cms` and a drill that decrypts;
  - retention.

  It ran on macOS (client tools from a container, `APELORA_PG_CLIENT_IMAGE`) and on Linux (native client tools in a Debian container). CI runs the self-test on every Rust change.
- **NOT YET VERIFIED:**
  - encryption with `age` (not installed where the tests ran);
  - the systemd units below;
  - continuous archiving;
  - the full restore of the plane onto a new host.

## 1. What to back up, and what is not in the dump

| What | Where it lives | How it is protected |
|---|---|---|
| **Apelora's database**: designs and revisions, applications, jobs, operations, volumes and their backups' records, machines, identities, roles, approvals, API token hashes, sign-in settings, the audit trail (append-only) | PostgreSQL | `db-backup.sh` (logical dump), and continuous archiving in production (§6) |
| **Environment configuration** of the control plane and worker: the `APELORA_*` settings, `RUST_LOG`, unit files | the plane's host | keep a copy **with secret values removed** in your configuration management, next to the backups. `.env.example` lists every setting |
| **Secrets**: the database password, the Incus client certificates and keys (deploy and admin identity), the pinned Incus server certificate, the API's TLS key | the operator's secret store | **not in the dump** and never in the backup directory. A restore needs them, so check they can be fetched when you run a drill |
| Incus's own state (members, instances, pools) | Incus | Incus recovers its own database from the members; not Apelora's job |
| Application data (volumes) | Incus pools | volume snapshots and backups (Fabric Console → volumes; tiers in [STORAGE](../architecture/STORAGE.md)) |

The dump holds sensitive data: user names, token hashes, the whole audit trail and the cluster layout. Treat it like a secret:

- encrypt it (§4), or keep it on storage that is encrypted at rest;
- let only the backup operators read it.

## 2. Where backups go: outside the managed cluster

The backup directory must not depend on the cluster Apelora manages. It may not be:

- a member's disk;
- an Incus storage pool or volume;
- the SAN that holds the cluster's shared pool.

This matters most for a small installation whose plane runs as system containers on that same cluster: a cluster-wide outage takes the plane down, and the backup must survive it.

- Use a separate server or network share, mounted on the host that runs the backup, then copied to a second site with your usual tooling.
- `db-backup.sh` refuses a directory that does not exist, so a missing mount fails loudly instead of filling a local disk. Create the directory once, on the mounted storage, with mode `0700`.
- It also refuses relative paths and any directory inside a source checkout.

## 3. Set up

Prerequisites on the host that runs the backup (often the plane's host, or a backup host that can reach PostgreSQL):

- **PostgreSQL client tools** (`pg_dump`, `pg_restore`, `psql`) of the **same or a newer major version** than the server.
  - Without them, set `APELORA_PG_CLIENT_IMAGE=postgres:18-alpine` (any image of that major version): the tools then run in a throwaway container on the host network.
  - Container mode passes only value variables; it refuses file-based ones such as `PGPASSFILE`.
- **bash 4 or newer** and `sha256sum` or `shasum`. On macOS, run the scripts with `/opt/homebrew/bin/bash`.
- **A role for backups that can read everything.** As a superuser:
  ```sql
  CREATE ROLE apelora_backup LOGIN PASSWORD '…';   -- set the password from the secret store
  GRANT pg_read_all_data TO apelora_backup;
  ```
- **The scripts** copied from `scripts/ops/` (all three files together) to e.g. `/opt/apelora/ops/`.
- **An environment file** `/etc/apelora/db-backup.env`, owned by root, mode `0600`:
  ```sh
  DATABASE_URL=postgres://apelora_backup:…@db.internal:5432/apelora?sslmode=verify-full
  # or the standard variables instead: PGHOST, PGPORT, PGUSER, PGDATABASE, PGPASSFILE=/etc/apelora/pgpass
  APELORA_BACKUP_DIR=/srv/apelora-backups
  APELORA_BACKUP_KEEP=30
  # APELORA_BACKUP_AGE_RECIPIENTS=/etc/apelora/backup-recipients.txt   (public keys only, §4)
  ```

Connection settings never appear on a command line, in output or in the files written: a URL is split into the standard `PG*` variables inside the script.

### Run a backup by hand

```sh
set -a; . /etc/apelora/db-backup.env; set +a
/opt/apelora/ops/db-backup.sh
```

It writes three files, named by UTC time, then removes the oldest complete backups beyond `--keep`:

| File | Contents |
|---|---|
| `apelora-db-<UTC time>.dump` | `pg_dump --format=custom` of the whole database |
| `….dump.sha256` | its SHA-256, checkable with `sha256sum -c` |
| `….dump.json` | the manifest, below |

The manifest holds:
- the migrations: count, latest version and a fingerprint;
- the exact row count of every table;
- whether the audit append-only trigger was present;
- the size and the tool versions.

Safety rules:
- A failed run keeps nothing.
- The dump is refused if its table of contents lacks the audit table's data or its append-only trigger.
- The dump is refused if a migration ran during the backup.
- Exit status 1 always comes with a message saying why.

## 4. Encryption (optional, operator's choice)

Only public material is needed on the backup host. The script refuses a file that contains a private key.

- **age:** `--age-recipients FILE` (or `APELORA_BACKUP_AGE_RECIPIENTS`) holds one or more public recipients. Writes `….dump.age`.
- **openssl:** `--openssl-cert FILE` (or `APELORA_BACKUP_OPENSSL_CERT`) is an X.509 certificate. Writes `….dump.cms` (CMS, AES-256).

The private key stays in the secret store and is used only where a drill or a restore runs. Losing it loses every encrypted backup; the drill proves it is still available.

With no recipient, the dump is written unencrypted (mode `0600`), and encryption at rest is the backup storage's job.

## 5. Schedule (systemd example)

`/etc/systemd/system/apelora-db-backup.service`:
```ini
[Unit]
Description=Back up Apelora's database
Wants=network-online.target
After=network-online.target
RequiresMountsFor=/srv/apelora-backups

[Service]
Type=oneshot
EnvironmentFile=/etc/apelora/db-backup.env
ExecStart=/opt/apelora/ops/db-backup.sh
UMask=0077
# OnFailure=apelora-alert@%n.service    # your alerting hook
```

`/etc/systemd/system/apelora-db-backup.timer`:
```ini
[Unit]
Description=Daily backup of Apelora's database

[Timer]
OnCalendar=*-*-* 02:15:00 UTC
RandomizedDelaySec=15m
Persistent=true

[Install]
WantedBy=timers.target
```

Add a drill the same way: `apelora-db-restore-drill.service` running `/opt/apelora/ops/db-restore-drill.sh`, with `OnCalendar=Sun *-*-* 04:00:00 UTC`. Enable both timers:

```sh
systemctl daemon-reload
systemctl enable --now apelora-db-backup.timer apelora-db-restore-drill.timer
```

Alert on symptoms, not only on unit failures:
- the newest `apelora-db-*.json` is older than 26 hours;
- `restore-drill-last-verified.json` is older than 35 days.

`ls -t /srv/apelora-backups/apelora-db-*.json | head -1` and the `drilled_at` field in the drill result give both ages.

## 6. Continuous archiving (recommended for production)

A daily dump can lose up to a day of changes. Production installations should also use PostgreSQL's continuous archiving, which keeps every change. It depends on how you run PostgreSQL, so it is not scripted here.

1. **Base backups and archived write-ahead log.** PostgreSQL writes every change to its write-ahead log (WAL).
   - With `archive_mode = on` and an `archive_command` (or `archive_library`), each completed WAL segment is copied to the off-cluster storage of §2.
   - `archive_timeout` (e.g. 60 s) bounds how long a quiet database waits before a segment is shipped, so it bounds the data at risk.
   - A **base backup** (`pg_basebackup`, weekly or daily) gives the starting point. The archive must hold every segment from the oldest base backup you keep onwards.
2. **Point-in-time recovery.** Restore the base backup into an empty data directory, set `restore_command` to fetch segments from the archive, and optionally `recovery_target_time`. Then start the server: it replays WAL up to the target and opens.
3. **Monitor the archiver.**
   - `SELECT archived_count, failed_count, last_archived_time, last_failed_time FROM pg_stat_archiver;`
   - Alert when `last_archived_time` falls behind or `failed_count` grows.
   - When archiving fails, WAL piles up on the server's disk until the disk fills.
4. **Drill it too.** At least each quarter, recover a base backup plus WAL to a chosen time on a separate host, then run the checks of §7 against it. Point `APELORA_DRILL_DATABASE_URL` at it and compare with a dump of the same time.
5. **Keep the dumps as well.** A logical dump restores across PostgreSQL major versions and is what `db-restore-drill.sh` verifies automatically. Archiving is physical and tied to one major version.

A standby server (streaming replication) protects against losing the primary's host. It is not a backup: a mistaken delete reaches the standby at once. Use both.

## 7. The restore drill

A backup that has never been restored does not count as one. The Console will show the age of the last **verified** restore (see *Open points*).

```sh
set -a; . /etc/apelora/db-backup.env; set +a
/opt/apelora/ops/db-restore-drill.sh                       # the newest backup
/opt/apelora/ops/db-restore-drill.sh --backup apelora-db-20261021T021500Z.dump
# encrypted: --age-identity FILE, or --openssl-key FILE --openssl-cert FILE
```

The drill never touches the live database.
1. It creates a new scratch database named `apelora_restore_drill_<time>_<pid>`.
2. It checks the file's SHA-256 against the `.sha256` file and the manifest.
3. It decrypts as a stream, so no plaintext copy is written to disk.
4. It restores with `pg_restore --exit-on-error --single-transaction`, without owners or grants.
5. It verifies:
   - the migrations (count, latest version, fingerprint) equal the manifest's;
   - every table that had rows still has rows;
   - `audit_events` has at least as many rows as recorded;
   - the `audit_events_no_change` trigger exists and is enabled;
   - an `UPDATE` and a `DELETE` of an audit row are refused in the scratch copy.
6. It drops the scratch database, whatever the outcome.

Results go to the backup directory:
- `restore-drill-last.json` after every drill: time, backup, its creation time, duration, verdict, reason, each check;
- `restore-drill-last-verified.json` after a passing one only.

The exit status is 0 only when every check passed.

- **Where to run it:** preferably against a separate PostgreSQL on another host (`APELORA_DRILL_DATABASE_URL`). That also proves the backup restores away from the plane. The drill role needs `CREATEDB`.
- **When:** weekly by timer; after every Apelora upgrade; after any change to the backup setup, keys or storage.
- **On failure:** treat it as having no backup. Read `reason` and the checks, fix the cause, run a new backup and drill again. A scratch database that could not be dropped is named in `warnings`; drop it by hand.

## 8. Restore the plane onto a new Linux host

Use this when the plane's host or database is lost. In a small installation, the plane may have run on the cluster it manages (ADR 0011 decision 7).

- If Incus is running, workloads keep running while the plane is down. Only changes wait.
- If the cluster itself is down, recover it first ([quorum-loss.md](quorum-loss.md)). The plane's restore does not depend on it, and can be done in parallel on a host outside the cluster.

Steps:

1. **Host.** Any Linux host that can reach the Incus API of the members (port 8443) and the backup storage. Install:
   - PostgreSQL of the same or a newer major version than the manifest's `server_version`;
   - the Apelora control plane and worker, at **the same release or newer** than the backup. The manifest's `migrations.latest` must not be above what the binaries know; a newer binary migrates forward when it starts.
2. **Secrets** from the secret store:
   - the database password;
   - the Incus deploy and admin client certificates and keys;
   - the pinned server certificate;
   - the API's TLS key;
   - for an encrypted backup, its private key.
3. **Pick and check the backup.** Use the newest one with a passing drill if possible:
   ```sh
   cd /srv/apelora-backups && sha256sum -c apelora-db-<time>.dump.sha256
   ```
4. **Empty database and role:**
   ```sh
   createuser --login apelora          # then set its password from the secret store
   createdb --owner apelora apelora
   ```
5. **Restore** (decrypt first if needed: `age --decrypt -i KEY` or `openssl cms -decrypt -binary -inform DER -inkey KEY -recip CERT`):
   ```sh
   pg_restore --dbname=apelora --no-owner --role=apelora --exit-on-error --single-transaction apelora-db-<time>.dump
   ```
6. **Check** it like the drill does:
   - `SELECT count(*), max(version) FROM _sqlx_migrations WHERE success` equals the manifest's `migrations`;
   - the audit trigger is present: `SELECT tgname FROM pg_trigger WHERE tgname = 'audit_events_no_change'`.
7. **Configuration:** restore the environment file from configuration management. Put the secrets back from the store, and set `DATABASE_URL` to the new database.
8. **Start the control plane first, then the worker.**
   - `GET /api/v1/health` answers.
   - The Fabric Console shows the members and instances, read live from Incus.
   - Jobs recorded as pending or running in the backup are claimed again by the worker once it starts, although some may have finished after the backup. Review them before starting the worker:
     ```sql
     SELECT id, kind, status, created_at FROM jobs WHERE status IN ('pending', 'running') ORDER BY created_at;
     ```
     Mark any that must not run again as `cancelled`, and record why.
9. **Point clients at the new host** (DNS name or load balancer, API TLS certificate).
10. **Reconcile what the backup missed** (§9). Then take a new backup and run a drill at once.

## 9. What is lost between backups

Running workloads are **not** lost: Incus runs them, and they do not depend on the plane. What is lost is Apelora's record of changes made after the backup:

| With a daily dump | With continuous archiving |
|---|---|
| up to ~24 hours | up to `archive_timeout` (about a minute) |

Changes in that window:

- **Designs, drafts, revisions, applications and lifecycle operations** saved after the backup. Designers re-save them from exported files where they have them.
- **Jobs and deployments.** Instances Incus created after the backup exist but are unknown to Apelora's records. Compare the Incus instances in the deploy projects (`APELORA_DEPLOY_PROJECTS`) with the Fabric Console, then redeploy or remove the extras deliberately. Nothing is deleted automatically.
- **Audit rows** written after the backup. Keep exports of the audit log if you need them beyond the backup's age.
- **Security changes: revocations come undone.** API tokens revoked, and role assignments or service identities removed, after the backup are valid again after a restore. Before reopening the API:
  - revoke again everything revoked since the backup ([access-revocation.md](access-revocation.md));
  - rotate tokens if unsure;
  - check sign-in settings.
- **Machines and join tokens.** Machines enrolled after the backup must be enrolled again. Unused join tokens issued since then should be revoked in Incus.

## Open points

- The Fabric Console view of "age of the last verified restore" (wave 4). The drill already writes the data, in `restore-drill-last-verified.json`. The control plane cannot yet read it, because the backup directory is off-cluster by design. Options are a small endpoint the drill reports to, or a row in the database written by the drill.
- The full restore of §8 and continuous archiving have not been exercised yet.
