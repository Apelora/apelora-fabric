#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Proves continuous archiving on a throwaway PostgreSQL: archive_command copies each completed
# segment, a base backup does not contain the write-ahead log, and a row written after that
# backup is present after recovery into a new data directory. The throwaway server is removed
# afterwards. It does not touch any other database.
#
#   bash scripts/ops/test-db-archive.sh
#
# Needs Docker and the postgres:18 image. Prints PASS/FAIL; exits non-zero when a check fails.
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
# shellcheck source=scripts/ops/db-common.sh
source "$here/db-common.sh"
command -v docker >/dev/null || die "docker is required (this test starts a throwaway PostgreSQL)"

image=postgres:18
work=$(mktemp -d "${TMPDIR:-/tmp}/apelora-db-archive.XXXXXX")
ops=$work/ops wal=$work/wal backups=$work/backups
src=apelora-wal-src-$$ restored=apelora-wal-new-$$
mkdir -p "$ops" "$wal" "$backups"
chmod 0777 "$wal" "$backups"
cp "$here/db-common.sh" "$here/wal-archive.sh" "$here/wal-restore.sh" "$here/db-basebackup.sh" "$ops/"
chmod 0755 "$ops"/*.sh
fails=0
pass() { echo "PASS  $1"; }
flunk() { echo "FAIL  $1"; fails=$((fails + 1)); }

cleanup() {
  local status=$?
  docker logs "$src" >"$work/src.log" 2>&1 || true
  docker logs "$restored" >"$work/restored.log" 2>&1 || true
  docker rm -f "$src" "$restored" >/dev/null 2>&1 || true
  if ((status != 0 || fails > 0)); then
    echo "--- source server (tail) ---"
    tail -n 30 "$work/src.log" || true
    echo "--- recovered server (tail) ---"
    tail -n 40 "$work/restored.log" || true
    echo "work directory kept: $work"
    exit 1
  fi
  rm -rf "$work"
  exit 0
}
trap cleanup EXIT

psql_src() { docker exec -u postgres "$src" psql -d apelora -v ON_ERROR_STOP=1 -At "$@"; }

echo "starting a throwaway PostgreSQL ($image) with archive_mode on"
docker run -d --name "$src" \
  -e POSTGRES_PASSWORD=apelora-archive-only \
  -e POSTGRES_DB=apelora \
  -e APELORA_WAL_DIR=/srv/apelora-wal \
  -e PGDATA=/var/lib/postgresql/data \
  -v "$ops":/opt/apelora/ops:ro \
  -v "$wal":/srv/apelora-wal \
  -v "$backups":/srv/apelora-backups \
  "$image" \
  postgres -c archive_mode=on \
  -c archive_command='/opt/apelora/ops/wal-archive.sh %p %f' \
  -c archive_timeout=60 \
  -c wal_level=replica >/dev/null

ready=0
for _ in $(seq 1 60); do
  if psql_src -c "SELECT 1" >/dev/null 2>&1; then ready=1; break; fi
  sleep 1
done
((ready == 1)) || die "the throwaway PostgreSQL did not accept connections"
version=$(psql_src -c "SHOW server_version")
pass "throwaway server is PostgreSQL $version"

psql_src -c "CREATE TABLE archive_proof (id int PRIMARY KEY, note text NOT NULL)" >/dev/null
psql_src -c "INSERT INTO archive_proof VALUES (1, 'before-base-backup')" >/dev/null
segment=$(psql_src -c "SELECT pg_walfile_name(pg_current_wal_lsn())")
psql_src -c "SELECT pg_switch_wal()" >/dev/null
archived=0
for _ in $(seq 1 60); do
  if [[ -f $wal/$segment ]]; then archived=1; break; fi
  sleep 1
done
if ((archived == 1)); then pass "archive_command stored segment $segment outside the data directory"; else flunk "archive_command stored the completed segment"; fi
failed_count=$(psql_src -c "SELECT failed_count FROM pg_stat_archiver")
if [[ $failed_count == 0 ]]; then pass "the archiver has not recorded a failure"; else flunk "the archiver has not recorded a failure (failed_count=$failed_count)"; fi

if docker exec -u postgres \
  -e PGHOST=/var/run/postgresql -e PGUSER=postgres -e PGDATABASE=apelora \
  -e APELORA_BACKUP_DIR=/srv/apelora-backups -e APELORA_WAL_DIR=/srv/apelora-wal \
  "$src" bash /opt/apelora/ops/db-basebackup.sh >"$work/basebackup.out" 2>&1; then
  pass "base backup completed and its closing segment was archived"
else
  flunk "base backup completed and its closing segment was archived"
  cat "$work/basebackup.out"
fi

psql_src -c "INSERT INTO archive_proof VALUES (2, 'after-base-backup')" >/dev/null
segment=$(psql_src -c "SELECT pg_walfile_name(pg_current_wal_lsn())")
psql_src -c "SELECT pg_switch_wal()" >/dev/null
archived=0
for _ in $(seq 1 60); do
  if [[ -f $wal/$segment ]]; then archived=1; break; fi
  sleep 1
done
if ((archived == 1)); then pass "the segment written after the base backup was archived"; else flunk "the segment written after the base backup was archived"; fi

mapfile -t slots < <(find "$backups" -mindepth 1 -maxdepth 1 -type d -name 'base-*' | sort)
slot=${slots[-1]:-}
[[ -n $slot ]] || die "no base backup directory was written"
# Recovery must not be able to read the source: stop it before the new data directory starts.
docker stop -t 30 "$src" >/dev/null
pass "the source server was stopped before recovery"

docker run --rm --user postgres --entrypoint touch -v "$slot":/data "$image" /data/recovery.signal
docker run -d --name "$restored" \
  -e APELORA_WAL_DIR=/srv/apelora-wal \
  -e PGDATA=/var/lib/postgresql/data \
  -v "$slot":/var/lib/postgresql/data \
  -v "$wal":/srv/apelora-wal:ro \
  -v "$ops":/opt/apelora/ops:ro \
  "$image" \
  postgres -c archive_mode=off \
  -c restore_command='/opt/apelora/ops/wal-restore.sh %f %p' >/dev/null

got=""
for _ in $(seq 1 90); do
  got=$(docker exec -u postgres "$restored" psql -d apelora -Atc "SELECT string_agg(note, ',' ORDER BY id) FROM archive_proof" 2>/dev/null || true)
  [[ $got == "before-base-backup,after-base-backup" ]] && break
  sleep 1
done
if [[ $got == "before-base-backup,after-base-backup" ]]; then
  pass "recovery into a new data directory returned the row written after the base backup"
else
  flunk "recovery into a new data directory returned the row written after the base backup (got: ${got:-nothing})"
fi
recovering=$(docker exec -u postgres "$restored" psql -d apelora -Atc "SELECT pg_is_in_recovery()" 2>/dev/null || true)
if [[ $recovering == f ]]; then pass "the recovered server has finished recovery and is open"; else flunk "the recovered server has finished recovery (pg_is_in_recovery=${recovering:-unknown})"; fi

if ((fails > 0)); then
  echo "$fails check(s) failed"
  exit 1
fi
echo "all checks passed (PostgreSQL $version)"
