# Update Apelora without internet access

**Evidence:** the bundle part is tested locally (2026-10-21, macOS): `scripts/release/bundle.sh`
built and signed a bundle with a throwaway key, and `scripts/release/verify.sh` accepted it,
offline, and refused it after a file was changed, a file was added, the signature was removed,
`SHA256SUMS` was edited, the signer was not allowed, or the archive held a link.
`scripts/ops/upgrade.sh` prints the staged order (worker stopped before the control plane) and,
with `APPLY=1`, copies `bin/` and `web/` into a prefix while keeping the previous copies for
`ROLLBACK=1`. That copy and the file rollback are tested by `scripts/ops/test-upgrade.sh`.
Stopping the units, starting them, and restoring PostgreSQL are **NOT YET VERIFIED** on an
installation: follow them with care and record the result here.

Decision: [ADR 0010](../adr/0010-disconnected-installations.md). Checklist:
[running without internet access](../architecture/DISCONNECTED.md).

## What you need

- The release bundle `apelora-<version>.tar.gz`, built on a connected system with
  `scripts/release/bundle.sh` and carried in on approved media.
- An `allowed_signers` file that names the release keys you trust (OpenSSH format, one line per
  key: `release@your-organisation namespaces="apelora-release" ssh-ed25519 AAAA…`). Get it through
  a separate channel from the bundle, so one tampered medium cannot change both.
- `ssh-keygen` (OpenSSH 8.2 or later), `tar`, and `sha256sum` or `shasum` on the machine that checks it.
- A way to back up and restore PostgreSQL (`pg_dump` / `pg_restore` of the same major version).

Running instances are not affected by any step here: Incus runs them, and Apelora is not in the
data path ([api-outage.md](api-outage.md)).

## 1. Check the bundle

```sh
scripts/release/verify.sh apelora-<version>.tar.gz allowed_signers
```

Continue only on `VERIFIED: … signed by <who you expect>`. `NOT VERIFIED` names what failed; do not
install that bundle. Unpack it next to the current installation (`tar -xzf`) and read
`BUILD-INFO` (version, commit, build host architecture) and the release notes in `docs/`.

## 2. Back up the database

Migrations run when either service starts and cannot be undone, so the backup is the rollback.

```sh
pg_dump --format=custom --file=apelora-before-<version>.dump "$DATABASE_URL"
```

Keep the previous bundle's directory as well: it is the other half of the rollback.

## 3. Stop the worker, then the API

Stop the deployment worker first, so it takes no new job while the API changes, and wait until it
has exited. A job it was holding is taken over by the next worker once its lease expires, fenced
by its generation, and resumes waiting on any Incus operation already started
([api-outage.md](api-outage.md)). Then stop the control plane. Deployments asked for meanwhile are refused with
"API not reachable", not lost.

## 4. Install

See the order first, without copying anything:

```sh
BUNDLE=/path/to/apelora-<version> PREFIX=/usr/local/share/apelora \
  BACKUP=/srv/apelora-backups/apelora-db-<ts>.dump scripts/ops/upgrade.sh
```

`APPLY=1` with the same variables copies `bin/` and `web/` into `PREFIX` and keeps what was
there under `PREFIX/previous`. It refuses to run without a non-empty `BACKUP`, and it never
stops systemd. Then copy those files on to the paths the units actually run
(`/usr/local/bin/apelora-control-plane`, `/usr/local/bin/apelora-deployment-worker`, and the
web roots), with the same owner and permissions as before.

- Keep the configuration (environment, certificates, `APELORA_IMAGE_MIRRORS`) as it is, unless the
  release notes say a setting changed. Never copy configuration from the bundle; it has none.

## 5. Start the API, then the worker

Start the control plane first. It applies new migrations (the bundle's `migrations/` lists them)
before it serves; if a migration fails it does not start, and the error is in its log. Then start
the worker.

## 6. Check

- `GET /api/v1/health` reports the new version; the Fabric Console's Administration → System shows it.
- Preflight passes as before, including the image-mirror checks; Administration → Image mirrors
  shows every mirror answering.
- A small deployment or a Restart of a test application completes (Operations shows the job done).
- The audit log shows normal activity.

## 7. Roll back (only if the check fails)

1. Stop the worker, then the control plane.
2. Restore the database from step 2:
   `pg_restore --clean --if-exists --dbname="$DATABASE_URL" apelora-before-<version>.dump`.
   Changes made after the backup (new deployments, audit entries) are lost; record what they were.
3. Put back the previous files: `APPLY=1 ROLLBACK=1 PREFIX=/usr/local/share/apelora scripts/ops/upgrade.sh`,
   then copy them back onto the unit paths.
4. Start the control plane, then the worker, and run the checks above.

An older control plane refuses to start on a database migrated by a newer one, which is why the
database is restored before the old binaries start.
