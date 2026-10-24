#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Restores a logical backup onto another PostgreSQL, starts the control plane and then the
# worker against that database only, and checks two facts from the runbook:
#   a job that was pending in the backup is claimed;
#   a job inserted after the backup is not in the restored database.
# Neither database is the development database. Incus is not contacted: the provider is the mock.
#
#   bash scripts/ops/test-db-plane.sh
#
# Needs Docker. Prints PASS/FAIL; exits non-zero when a check fails.
set -euo pipefail
here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)
root=$(cd "$here/../.." && pwd -P)
# shellcheck source=scripts/ops/db-common.sh
source "$here/db-common.sh"
command -v docker >/dev/null || die "docker is required"
command -v cargo >/dev/null || die "cargo is required"

export APELORA_PG_CLIENT_IMAGE=${APELORA_PG_CLIENT_IMAGE:-postgres:18-alpine}
# These processes must not inherit a lab Incus or the development database.
unset APELORA_INCUS_URL APELORA_INCUS_CLIENT_CERT APELORA_INCUS_CLIENT_KEY APELORA_INCUS_SERVER_CERT \
  APELORA_INCUS_ADMIN_CLIENT_CERT APELORA_INCUS_ADMIN_CLIENT_KEY APELORA_INCUS_TLS_SERVER_NAME \
  APELORA_BACKUP_DIR DATABASE_URL

work=$(mktemp -d "${TMPDIR:-/tmp}/apelora-db-plane.XXXXXX")
dir=$work/backups
mkdir -p "$dir"
source_name=apelora-plane-source-$$
target_name=apelora-plane-target-$$
source_port=15444
target_port=15445
api_port=8096
metrics_port=9476
source_url=postgres://apelora:apelora-plane-drill@127.0.0.1:${source_port}/apelora
target_url=postgres://apelora:apelora-plane-drill@127.0.0.1:${target_port}/apelora
in_backup='11111111-1111-4111-8111-111111111111'
after_backup='22222222-2222-4222-8222-222222222222'
api_pid="" worker_pid=""
fails=0
pass() { echo "PASS  $1"; }
flunk() { echo "FAIL  $1"; fails=$((fails + 1)); }

cleanup() {
  local status=$?
  if [[ -n $api_pid ]]; then kill "$api_pid" 2>/dev/null || true; wait "$api_pid" 2>/dev/null || true; fi
  if [[ -n $worker_pid ]]; then kill "$worker_pid" 2>/dev/null || true; wait "$worker_pid" 2>/dev/null || true; fi
  docker rm -f "$source_name" "$target_name" >/dev/null 2>&1 || true
  if ((status != 0 || fails > 0)); then
    echo "--- control plane ---"
    tail -n 40 "$work/api.log" 2>/dev/null | grep -v 'postgres://' || true
    echo "--- worker ---"
    tail -n 40 "$work/worker.log" 2>/dev/null | grep -v 'postgres://' || true
    echo "work directory kept: $work"
    exit 1
  fi
  rm -rf "$work"
  exit 0
}
trap cleanup EXIT

for spec in "${source_port}:PostgreSQL" "${target_port}:PostgreSQL" "${api_port}:the control plane" "${metrics_port}:worker metrics"; do
  port=${spec%%:*}
  what=${spec#*:}
  if (: >/dev/tcp/127.0.0.1/"$port") 2>/dev/null; then
    die "$what port $port is already in use"
  fi
done

start_pg() {
  local name=$1 port=$2
  docker run -d --name "$name" \
    -e POSTGRES_PASSWORD=apelora-plane-drill \
    -e POSTGRES_USER=apelora \
    -e POSTGRES_DB=apelora \
    -p "127.0.0.1:${port}:5432" \
    postgres:18-alpine >/dev/null
  local i
  for i in $(seq 1 60); do
    if docker exec "$name" pg_isready -U apelora -d apelora >/dev/null 2>&1; then return 0; fi
    sleep 1
  done
  die "$name did not become ready"
}

echo "starting two throwaway PostgreSQL servers"
start_pg "$source_name" "$source_port"
start_pg "$target_name" "$target_port"

echo "applying migrations on the source (control plane token list)"
DATABASE_URL=$source_url cargo run -q --locked --manifest-path "$root/Cargo.toml" -p apelora-control-plane -- token list \
  >"$work/migrate.log" 2>&1 || { tail -n 20 "$work/migrate.log" | grep -v 'postgres://'; die "migrations did not apply"; }

export DATABASE_URL=$source_url
for v in "${OPS_PG_VALUE_VARS[@]}"; do unset "$v"; done
pg_connect_setup "$source_url"
pg_psql -c "INSERT INTO jobs (id, kind, payload, status) VALUES ('$in_backup', 'instance.stop', '{\"project\":\"apelora\",\"name\":\"from-backup\"}', 'pending')" \
  >/dev/null

"$BASH" "$here/db-backup.sh" --dir "$dir" --keep 2

pg_psql -c "INSERT INTO jobs (id, kind, payload, status) VALUES ('$after_backup', 'instance.stop', '{\"project\":\"apelora\",\"name\":\"after-backup\"}', 'pending')" \
  >/dev/null

DATABASE_URL=$source_url APELORA_RESTORE_DATABASE_URL=$target_url \
  "$BASH" "$here/db-restore-new-host.sh" --dir "$dir"

for v in "${OPS_PG_VALUE_VARS[@]}"; do unset "$v"; done
pg_connect_setup "$target_url"
restored=$(pg_psql -c "SELECT count(*) FROM jobs WHERE id IN ('$in_backup', '$after_backup')")
if [[ $restored == 1 ]] && [[ $(pg_psql -c "SELECT count(*) FROM jobs WHERE id = '$in_backup'") == 1 ]] \
  && [[ $(pg_psql -c "SELECT count(*) FROM jobs WHERE id = '$after_backup'") == 0 ]]; then
  pass "the restored database has the job from the backup and not the one written after it"
else
  flunk "the restored database has the job from the backup and not the one written after it (rows matching either id: $restored)"
fi

echo "starting the control plane, then the worker, on the restored database"
DATABASE_URL=$target_url APELORA_API_ADDR=127.0.0.1:${api_port} \
  cargo run -q --locked --manifest-path "$root/Cargo.toml" -p apelora-control-plane \
  >"$work/api.log" 2>&1 &
api_pid=$!
ready=0
for _ in $(seq 1 90); do
  if curl -sf -m 2 "http://127.0.0.1:${api_port}/api/v1/health" >"$work/health.json" 2>/dev/null; then ready=1; break; fi
  if ! kill -0 "$api_pid" 2>/dev/null; then break; fi
  sleep 1
done
if [[ $ready == 1 ]] && grep -q '"provider":"mock"' "$work/health.json"; then
  pass "the control plane answered /api/v1/health on the restored database (provider mock)"
else
  flunk "the control plane answered /api/v1/health on the restored database (provider mock)"
fi

DATABASE_URL=$target_url APELORA_WORKER_METRICS_ADDR=127.0.0.1:${metrics_port} \
  APELORA_SUPERVISE_SECS=0 APELORA_ALERT_EVALUATE_SECS=0 \
  cargo run -q --locked --manifest-path "$root/Cargo.toml" -p apelora-deployment-worker \
  >"$work/worker.log" 2>&1 &
worker_pid=$!

claimed=""
for _ in $(seq 1 45); do
  claimed=$(pg_psql -c "SELECT status FROM jobs WHERE id = '$in_backup'" || true)
  if [[ -n $claimed && $claimed != pending ]]; then break; fi
  if ! kill -0 "$worker_pid" 2>/dev/null; then break; fi
  sleep 1
done
detail=$(pg_psql -c "SELECT attempts || ' ' || coalesce(last_error, '') FROM jobs WHERE id = '$in_backup'" || true)
if [[ -n $claimed && $claimed != pending ]]; then
  pass "the worker claimed the pending job from the backup (status $claimed, attempts and error: $detail)"
else
  flunk "the worker claimed the pending job from the backup (status ${claimed:-missing})"
fi
if [[ $(pg_psql -c "SELECT count(*) FROM jobs WHERE id = '$after_backup'") == 0 ]]; then
  pass "the job written after the backup is still absent"
else
  flunk "the job written after the backup is still absent"
fi

if ((fails > 0)); then
  echo "$fails check(s) failed"
  exit 1
fi
echo "all checks passed"
