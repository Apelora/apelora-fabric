#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Proves a logical backup restores onto an empty database on another PostgreSQL, and that the
# live database is refused and left unchanged. The other server is a throwaway container and is
# removed afterwards. The live database (DATABASE_URL) is only read.
#
#   DATABASE_URL=postgres://… bash scripts/ops/test-db-restore-host.sh
#
# Needs Docker. Prints PASS/FAIL; exits non-zero when a check fails.
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
# shellcheck source=scripts/ops/db-common.sh
source "$here/db-common.sh"
[[ -n ${DATABASE_URL:-} ]] || die "set DATABASE_URL to a migrated Apelora database (it is only read)"
command -v docker >/dev/null || die "docker is required (the new host is a throwaway PostgreSQL)"

work=$(mktemp -d "${TMPDIR:-/tmp}/apelora-db-new-host.XXXXXX")
dir=$work/backups log=$work/log
mkdir -p "$dir"
target=apelora-restore-target-$$
port=15442
restore_url=postgres://apelora:apelora-restore-only@127.0.0.1:${port}/apelora
backup=("$BASH" "$here/db-backup.sh")
restore=("$BASH" "$here/db-restore-new-host.sh")
fails=0
pass() { echo "PASS  $1"; }
flunk() { echo "FAIL  $1"; fails=$((fails + 1)); }
expect() {
  local want=$1 what=$2 got=0
  shift 2
  "$@" >>"$log" 2>&1 || got=$?
  if [[ $got == "$want" ]]; then pass "$what"; else flunk "$what (exit $got, wanted $want)"; tail -n 8 "$log" | sed 's/^/      /'; fi
}

cleanup() {
  local status=$?
  docker rm -f "$target" >/dev/null 2>&1 || true
  if ((status != 0 || fails > 0)); then
    echo "--- log ---"
    cat "$log" || true
    echo "work directory kept: $work"
    exit 1
  fi
  rm -rf "$work"
  exit 0
}
trap cleanup EXIT

pg_connect_setup "$DATABASE_URL"
live=$(pg_endpoint)
before=$(pg_psql -c "SELECT count(*) FROM audit_events") || die "cannot read the live database (it is only counted)"

expect 1 "restore refuses the live database's server" \
  env APELORA_RESTORE_DATABASE_URL="$DATABASE_URL" "${restore[@]}" --dir "$dir"

echo "starting a throwaway PostgreSQL on 127.0.0.1:$port"
docker run -d --name "$target" \
  -e POSTGRES_PASSWORD=apelora-restore-only \
  -e POSTGRES_USER=apelora \
  -e POSTGRES_DB=apelora \
  -p "127.0.0.1:${port}:5432" \
  postgres:18-alpine >/dev/null
ready=0
for _ in $(seq 1 60); do
  if docker exec "$target" pg_isready -U apelora -d apelora >/dev/null 2>&1; then ready=1; break; fi
  sleep 1
done
((ready == 1)) || die "the throwaway PostgreSQL did not become ready"

expect 0 "backup of the live database" "${backup[@]}" --dir "$dir" --keep 2
expect 0 "restore onto the empty database on the other server" \
  env APELORA_RESTORE_DATABASE_URL="$restore_url" "${restore[@]}" --dir "$dir"
if grep -q '"verdict":"pass"' "$dir/restore-new-host-last-verified.json" 2>/dev/null; then
  pass "the new host result records a pass"
else
  flunk "the new host result records a pass"
fi
live_server=${live%/*}
if grep -q "$live_server -> 127.0.0.1:${port}" "$dir/restore-new-host-last-verified.json" \
  && grep -q "127.0.0.1:${port}/apelora" "$dir/restore-new-host-last-verified.json"; then
  pass "the result names the live server and the other server"
else
  flunk "the result names the live server and the other server"
fi

# The other server, read with its own URL. Save and clear the live connection first.
for v in "${OPS_PG_VALUE_VARS[@]}"; do unset "$v"; done
pg_connect_setup "$restore_url"
restored_migrations=$(pg_psql -c "SELECT count(*) FROM _sqlx_migrations WHERE success") || restored_migrations=0
if [[ ${restored_migrations:-0} -gt 0 ]]; then
  pass "the other server has the restored migrations ($restored_migrations)"
else
  flunk "the other server has the restored migrations"
fi
for v in "${OPS_PG_VALUE_VARS[@]}"; do unset "$v"; done
pg_connect_setup "$DATABASE_URL"
after=$(pg_psql -c "SELECT count(*) FROM audit_events") || after=""
if [[ $before == "$after" && $(pg_endpoint) == "$live" ]]; then
  pass "the live database was not changed ($before audit rows before and after)"
else
  flunk "the live database was not changed (before $before, after $after)"
fi

expect 1 "a second restore into the now-populated database is refused" \
  env APELORA_RESTORE_DATABASE_URL="$restore_url" "${restore[@]}" --dir "$dir"
if tail -n 20 "$log" | grep -q 'not empty'; then
  pass "the refusal says the target is not empty"
else
  flunk "the refusal says the target is not empty"
fi

if ((fails > 0)); then
  echo "$fails check(s) failed"
  exit 1
fi
echo "all checks passed"
