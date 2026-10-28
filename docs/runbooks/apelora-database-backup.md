# Back up and restore Apelora's own database

This page backs up Apelora's own database outside the cluster, proves each backup with a restore drill, and restores the control plane onto a new host when its host is lost. You need shell access to a host that reaches PostgreSQL, and storage that does not depend on the cluster.

Decision: [ADR 0011](../adr/0011-one-design-every-size.md) decision 6 (backed up outside the cluster, restores drilled) and decision 7 (a small installation may run its plane on the cluster it manages, so the plane must be restorable onto any Linux host). Scripts: `scripts/ops/db-backup.sh`, `scripts/ops/db-restore-drill.sh`, `scripts/ops/db-restore-new-host.sh`, `scripts/ops/wal-archive.sh`, `scripts/ops/wal-restore.sh`, `scripts/ops/db-basebackup.sh`.

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

Example units are in [`infra/systemd/`](../../infra/systemd/): `apelora-db-backup.service` with `apelora-db-backup.timer` (02:15 UTC, 15 minutes of jitter), and `apelora-db-restore-drill.service` with `apelora-db-restore-drill.timer` (Sunday 04:00 UTC). Copy them to `/etc/systemd/system/`.

Enable both timers:

```sh
systemctl daemon-reload
systemctl enable --now apelora-db-backup.timer apelora-db-restore-drill.timer
```

**See it in the Console.** Set `APELORA_DB_BACKUP_DIR` on the control plane to the directory the backup script writes to (read access is enough; the dumps themselves may live elsewhere). *Administration → Apelora's database backups* then shows the newest backup (age, size, encryption), the last verified restore, and any failed drill, with warnings at 26 hours without a backup, 35 days without a verified restore, an unencrypted backup or an empty one, and a failing state at 72 hours, 70 days or a drill that failed after the last pass (`GET /api/v1/database-backups`, needs `cluster.view`). It only reads the scripts' records; it runs nothing.

Alert on symptoms, not only on unit failures:
- the newest `apelora-db-*.json` is older than 26 hours;
- `restore-drill-last-verified.json` is older than 35 days.

`ls -t /srv/apelora-backups/apelora-db-*.json | head -1` and the `drilled_at` field in the drill result give both ages.

## 6. Continuous archiving (recommended for production)

A daily dump can lose up to a day of changes. Production installations should also use PostgreSQL's continuous archiving, which keeps every change. The scripts are `wal-archive.sh` (the server's `archive_command`), `db-basebackup.sh` (a base backup that waits until the segment it needs is in the archive) and `wal-restore.sh` (the new server's `restore_command`). The server still has to be configured where PostgreSQL runs:

```
archive_mode = on
archive_command = '/opt/apelora/ops/wal-archive.sh %p %f'
archive_timeout = 60
```

`APELORA_WAL_DIR` is the off-cluster directory from §2, and it must be set in the server's environment. The role that runs `db-basebackup.sh` needs replication. A logical dump is still required: it restores across a major version, and the base backup does not.

1. **Base backups and archived write-ahead log.** PostgreSQL writes every change to its write-ahead log (WAL).
   - With `archive_mode = on` and an `archive_command` (or `archive_library`), each completed WAL segment is copied to the off-cluster storage of §2.
   - `archive_timeout` (e.g. 60 s) bounds how long a quiet database waits before a segment is shipped, so it bounds the data at risk.
   - A **base backup** (`pg_basebackup`, weekly or daily) gives the starting point. The archive must hold every segment from the oldest base backup you keep onwards.
2. **Point-in-time recovery.** Restore the base backup into an empty data directory. Put an empty `recovery.signal` file there, and start the server with `restore_command = '/opt/apelora/ops/wal-restore.sh %f %p'` and `APELORA_WAL_DIR` in its environment. A missing file makes `wal-restore.sh` exit 1 and print nothing, which is how recovery knows it has reached the end. Optionally set `recovery_target_time`. The server replays the archived segments and then opens.
3. **Monitor the archiver.**
   - `SELECT archived_count, failed_count, last_archived_time, last_failed_time FROM pg_stat_archiver;`
   - Alert when `last_archived_time` falls behind or `failed_count` grows.
   - When archiving fails, WAL piles up on the server's disk until the disk fills.
4. **Drill it too.** At least each quarter, recover a base backup plus WAL to a chosen time on a separate host, then run the checks of §7 against it. Point `APELORA_DRILL_DATABASE_URL` at it and compare with a dump of the same time.
5. **Keep the dumps as well.** A logical dump restores across PostgreSQL major versions and is what `db-restore-drill.sh` verifies automatically. Archiving is physical and tied to one major version.

A standby server (streaming replication) protects against losing the primary's host. It is not a backup: a mistaken delete reaches the standby at once. Use both.

## 7. The restore drill

A backup that has never been restored does not count as one. The Console shows the age of the last verified restore when `APELORA_DB_BACKUP_DIR` points at this directory.

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
5. **Restore** with `db-restore-new-host.sh`. It refuses the live server (same host and port) and a database that already has tables, decrypts when you give the key, restores, runs the drill's checks, and keeps the database.
   ```sh
   DATABASE_URL=postgres://…live… APELORA_RESTORE_DATABASE_URL=postgres://…new-host…/apelora \
     APELORA_BACKUP_DIR=/srv/apelora-backups /opt/apelora/ops/db-restore-new-host.sh
   ```
   Decrypt an encrypted backup with `--age-identity` or `--openssl-key` and `--openssl-cert`, as for the drill.
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

## Before you rely on it

- The scripts have been run against PostgreSQL 18, and the example units have been started on a test host. The timers have not yet run on their own schedule on a production host. Check the first scheduled backup and drill yourself (Administration → Apelora's database backups shows them).
- Restoring onto a new host has been tried with a second PostgreSQL on the same machine, not on a second machine.
- Starting the control plane and the worker on a restored database has been rehearsed on one machine without a cluster (`scripts/ops/test-db-plane.sh`): the jobs in the backup were picked up and later ones were absent. Reconciling what the backup missed against a live cluster (§9) has not been rehearsed. Rehearse the whole path once before you need it.
