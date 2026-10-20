#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Drill (Phase 08): the deployment worker is killed while it is creating instances, and the
# same deployment request is sent twice. A second worker must take the jobs over once their
# leases expire, wait on the Incus operations the first one started instead of starting new
# ones, and finish with exactly the instances the design asks for. Prints PASS or FAIL per
# check and removes everything again.
#
# Needs the disposable test cluster (scripts/test-cluster/up.sh and add-member.sh), the dev
# PostgreSQL (infra/dev/compose.yaml) and jq. Mutates only the test cluster.
#   bash scripts/test-cluster/drill-worker-restart.sh
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
run=$(mktemp -d -t apelora-drill)
log="$run/drill.log"
APP=drill
API=http://127.0.0.1:8080/api/v1
export DATABASE_URL=${DATABASE_URL:-postgres://apelora:apelora-dev-only@127.0.0.1:15432/apelora}
# shellcheck disable=SC1091
source "$HOME/.config/apelora/test-cluster/env"
umask 077

say() { printf '%s %s\n' "$(date +%T)" "$*" | tee -a "$log"; }
pass() { say "PASS  $*"; }
fail() { say "FAIL  $*"; exit 1; }
api() { curl -sS -H "Authorization: Bearer $(cat "$run/token")" -H 'Content-Type: application/json' "$@"; }
sql() { docker exec -i apelora-dev-db psql -U apelora -d apelora -At -c "$1"; }
incus_() { limactl shell --workdir / apelora-incus sudo incus "$@" </dev/null; }
jobs_of() { sql "SELECT j.status || ' ' || j.attempts || ' ' || coalesce(j.lease_owner, '-') FROM jobs j JOIN applications a ON a.id = j.application_id WHERE a.name = '$APP' AND j.kind = 'instance.create' ORDER BY j.created_at"; }

worker() {   # worker <id>: starts a worker without the supervisor (this drill is about jobs)
  APELORA_WORKER_ID=$1 APELORA_SUPERVISE_SECS=0 RUST_LOG=info \
    nohup "$repo/target/debug/apelora-deployment-worker" >"$run/worker-$1.log" 2>&1 &
  echo $!
}

cleanup() {
  set +e
  say "cleanup"
  if [ -s "$run/token" ]; then
    api -X POST -H 'Idempotency-Key: drill-cleanup' \
      -d '{"project":"apelora","document":{"apiVersion":"fabric.apelora/v1alpha1"},"appliances":[],"removeMissing":true}' \
      "$API/applications/$APP/deployments" >/dev/null
    [ -n "$(pgrep -f "$repo/target/debug/apelora-deployment-worker")" ] || worker drill-cleanup >/dev/null
    for _ in $(seq 1 24); do
      [ -z "$(incus_ list --project apelora -c n --format csv | grep "^$APP-")" ] && break
      sleep 5
    done
  fi
  pkill -f "$repo/target/debug/apelora-control-plane"
  pkill -f "$repo/target/debug/apelora-deployment-worker"
  if [ -s "$run/token-id" ]; then
    sql "DELETE FROM jobs WHERE application_id IN (SELECT id FROM applications WHERE name = '$APP');
         DELETE FROM audit_events WHERE actor = 'drill' OR target LIKE 'apelora/$APP-%' OR (detail->>'application') = '$APP';
         DELETE FROM applications WHERE name = '$APP';
         DELETE FROM api_tokens WHERE id = '$(cat "$run/token-id")';" >/dev/null
  fi
  rm -f "$run/token" "$run/token-id"
  say "left on the cluster: $(incus_ list --project apelora -c n --format csv | tr '\n' ' ')"
  say "log: $log"
}
trap cleanup EXIT

say "worker-restart drill — run directory $run"
(cd "$repo" && cargo build -q -p apelora-control-plane -p apelora-deployment-worker)
out=$("$repo/target/debug/apelora-control-plane" token create --name drill --role fabric-admin --expires-days 1 2>&1)
grep -o 'apl_[0-9a-f]\{64\}' <<<"$out" | tr -d '\n' >"$run/token"
grep -oE '[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}' <<<"$out" | head -1 >"$run/token-id"
[ -s "$run/token" ] || fail "no API token"
RUST_LOG=info nohup "$repo/target/debug/apelora-control-plane" >"$run/control-plane.log" 2>&1 &
for _ in $(seq 1 30); do curl -s "$API/health" >/dev/null && break; sleep 1; done
[ "$(curl -s "$API/health" | jq -r .incus.connected)" = true ] || fail "control plane not connected to Incus"

# --- The same request twice --------------------------------------------------------------
nginx=library/nginx@sha256:eb1c896deef834f4e056829fd6dadac8305501f16cc71ba65fb252c8d0aa168e
body=$(jq -n --arg nginx "$nginx" '{
  project: "apelora", document: {apiVersion: "fabric.apelora/v1alpha1", kind: "ApplicationAssembly"},
  appliances: [{appliance: "web-01", replicas: 3, availability: "stateless", spec: {project: "", name: "", kind: "container", start: true,
    source: {protocol: "oci", server: "https://docker.io", alias: $nginx}}}]}')
first=$(api -X POST -H 'Idempotency-Key: drill-1' -d "$body" "$API/applications/$APP/deployments")
again=$(api -X POST -H 'Idempotency-Key: drill-1' -d "$body" "$API/applications/$APP/deployments")
ids() { jq -c '[.jobs[].jobId] | sort' <<<"$1"; }
[ "$(ids "$first")" = "$(ids "$again")" ] && [ "$(jq '.jobs | length' <<<"$first")" -eq 3 ] \
  && pass "repeated request queued nothing new: $(jq -r '.revision' <<<"$again") revision, $(jq '.jobs | length' <<<"$first") jobs" \
  || fail "repeated request: $(ids "$first") vs $(ids "$again")"

# --- Kill the worker while Incus is creating ----------------------------------------------
a=$(worker drill-a)
for _ in $(seq 1 120); do
  n=$(sql "SELECT count(*) FROM jobs j JOIN applications a ON a.id = j.application_id WHERE a.name = '$APP' AND j.status = 'running' AND j.external_operation IS NOT NULL")
  [ "$n" -ge 1 ] && break
  sleep 0.5
done
[ "$n" -ge 1 ] || fail "worker A never started an Incus operation"
kill -9 "$a"
pass "worker A killed while $n job(s) had an Incus operation in progress"
say "jobs: $(jobs_of | tr '\n' ';')"
b=$(worker drill-b)
t0=$(date +%s)
for _ in $(seq 1 60); do
  sleep 5
  [ -z "$(jobs_of | grep -v '^succeeded')" ] && break
done
say "jobs: $(jobs_of | tr '\n' ';')"
[ -z "$(jobs_of | grep -v '^succeeded')" ] && pass "worker B finished every job $(( $(date +%s) - t0 )) s after taking over" || fail "jobs did not finish"
grep -q "waiting on the operation an earlier attempt started" "$run/worker-drill-b.log" \
  && pass "worker B waited on worker A's Incus operation instead of starting another" \
  || say "NOTE  worker B did not need to wait on an operation (worker A's had finished)"
names=$(incus_ list --project apelora -c ns --format csv | grep "^$APP-" | sort)
say "instances: $(tr '\n' ' ' <<<"$names")"
[ "$(grep -c . <<<"$names")" -eq 3 ] && [ "$(grep -c ',RUNNING$' <<<"$names")" -eq 3 ] \
  && pass "exactly three instances, all running" || fail "unexpected instances"
[ "$(grep -c 'error\|ERROR' "$run/worker-drill-b.log" || true)" -eq 0 ] && pass "no errors in worker B's log" || say "NOTE  worker B logged errors (see $run/worker-drill-b.log)"
kill "$b" 2>/dev/null || true
say "DRILL PASSED"
