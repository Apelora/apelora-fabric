#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Measures Apelora on the disposable test cluster (issue #16): deploys APPS applications of
# MODULES idle Debian system containers each through the real control plane and WORKERS
# deployment workers (release builds, connections enforced on OVN), then reports how long they
# took to run, how long each kind of job took, what the control plane, the workers and
# PostgreSQL use, how long supervisor passes and Incus requests take, and what the containers
# add on the members. Removes everything it deployed.
# These are lab figures (Lima virtual machines on one workstation), not hardware sizing.
#
# Needs: scripts/test-cluster/{up,ovn}.sh done, the dev PostgreSQL (infra/dev/compose.yaml), jq,
# and nothing else listening on 127.0.0.1:8080 or 127.0.0.1:9471 and up. Mutates only the test
# cluster.
#   APPS=4 MODULES=10 WORKERS=1 bash scripts/test-cluster/measure.sh
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
APPS=${APPS:-4}
MODULES=${MODULES:-10}
WORKERS=${WORKERS:-1}
SETTLE=${SETTLE:-120}   # seconds of steady state over which supervisor passes are averaged
run=$(mktemp -d -t apelora-measure)
log="$run/measure.log"
API=http://127.0.0.1:8080/api/v1
export DATABASE_URL=${DATABASE_URL:-postgres://apelora:apelora-dev-only@127.0.0.1:15432/apelora}
# shellcheck disable=SC1091
source "$HOME/.config/apelora/test-cluster/env"

say() { printf '%s %s\n' "$(date +%T)" "$*" | tee -a "$log"; }
fail() { say "FAIL  $*"; exit 1; }
member() { limactl shell --workdir / "$1" sudo bash -s 2>>"$log"; }
incus_() { echo "incus $* </dev/null" | member apelora-incus; }
api() { curl -sS -H "Authorization: Bearer $(cat "$run/token")" -H 'Content-Type: application/json' "$@"; }
psql_() { docker exec -i apelora-dev-db psql -U apelora -d apelora -qtA -c "$1"; }
rss_mib() { ps -o rss= -p "$@" | awk '{s += $1} END {printf "%.0f", s / 1024}'; }
metrics_port() { echo $((9470 + $1)); }
# Every worker's /metrics page, one after another.
scrape() { for w in $(seq 1 "$WORKERS"); do curl -s "http://127.0.0.1:$(metrics_port "$w")/metrics"; done; }
# Sum and count of a histogram family over all label sets matching $3, from a /metrics page.
hist() { awk -v f="$2" -v l="${3:-}" 'index($1, l) && $1 ~ "^"f"_sum" {s += $2} index($1, l) && $1 ~ "^"f"_count" {c += $2} END {printf "%s %s", s + 0, c + 0}' <<<"$1"; }
mean_ms() { awk -v s="$1" -v c="$2" 'BEGIN {if (c > 0) printf "%.0f", s / c * 1000; else print "n/a"}'; }
app_name() { printf 'msr%02d' "$1"; }
# Instances and application networks of this script; networks go last (the final network.sync).
left_on_cluster() {
  { incus_ list --project apelora -c n --format csv; incus_ network list --project apelora --format csv | cut -d, -f1; } | grep -c '^msr' || true
}

cp_pid=""
worker_pids=()
cleanup() {
  set +e
  say "cleanup"
  if [ -s "$run/token" ]; then
    for a in $(seq 1 "$APPS"); do
      api -X POST -H "Idempotency-Key: measure-cleanup-$a" \
        -d '{"project":"apelora","document":{"apiVersion":"fabric.apelora/v1alpha1"},"modules":[],"connections":[],"removeMissing":true}' \
        "$API/applications/$(app_name "$a")/deployments" >/dev/null
    done
    # Deletions are jobs too: give them as long as the creations may take.
    for _ in $(seq 1 $((60 + APPS * MODULES))); do
      [ "$(left_on_cluster)" = 0 ] && break
      sleep 5
    done
  fi
  [ -n "$cp_pid" ] && kill "$cp_pid"
  [ "${#worker_pids[@]}" -gt 0 ] && kill "${worker_pids[@]}"
  if [ -s "$run/token-id" ]; then
    # The token first and on its own: a failing statement must never keep a credential alive.
    psql_ "DELETE FROM api_tokens WHERE id = '$(cat "$run/token-id")';" >/dev/null
    psql_ "DELETE FROM jobs WHERE application_id IN (SELECT id FROM applications WHERE name LIKE 'msr%'); DELETE FROM applications WHERE name LIKE 'msr%';" >/dev/null
  fi
  rm -f "$run/token" "$run/token-id"
  say "left on the cluster: $(incus_ list --project apelora -c n --format csv | tr '\n' ' ')"
  say "log: $log"
}
trap cleanup EXIT

for port in 8080 $(for w in $(seq 1 "$WORKERS"); do metrics_port "$w"; done); do
  if curl -s -m 2 -o /dev/null "http://127.0.0.1:$port/"; then fail "something already listens on 127.0.0.1:$port"; fi
done
[ "$(left_on_cluster)" = 0 ] || fail "instances of an earlier run are still on the cluster"
total=$((APPS * MODULES))
say "measuring $APPS applications × $MODULES modules = $total system containers, $WORKERS worker(s) — run directory $run"
members=$(incus_ cluster list --format csv -c n | wc -l | tr -d ' ')
say "members: $members"
(cd "$repo" && cargo build -q --release -p apelora-control-plane -p apelora-deployment-worker)
bin="$repo/target/release"
umask 077
out=$("$bin/apelora-control-plane" token create --name measure --role fabric-admin 2>&1)
echo "$out" | tail -1 | tr -d '[:space:]' > "$run/token"
echo "$out" | grep -o '^Created token [a-f0-9-]*' | awk '{print $3}' > "$run/token-id"
export APELORA_NETWORK_UPLINK=incusbr0 APELORA_SUPERVISE_SECS=30 RUST_LOG=warn
"$bin/apelora-control-plane" > "$run/control-plane.log" 2>&1 & cp_pid=$!
for w in $(seq 1 "$WORKERS"); do
  APELORA_WORKER_METRICS_ADDR="127.0.0.1:$(metrics_port "$w")" "$bin/apelora-deployment-worker" > "$run/worker-$w.log" 2>&1 &
  worker_pids+=($!)
done
sleep 3
[ "$(curl -s "$API/health" | jq -r .incus.connected)" = true ] || fail "control plane not connected to Incus"
db_before=$(psql_ "SELECT pg_database_size('apelora')")
say "idle: control plane $(rss_mib "$cp_pid") MiB, workers $(rss_mib "${worker_pids[@]}") MiB together, database $((db_before / 1048576)) MiB"
vms="apelora-incus apelora-incus-2 apelora-incus-3 apelora-incus-4"
used_mib() { echo "free -m | awk '/Mem:/ {print \$3}'" | member "$1"; }
mem_before=0
for vm in $vms; do mem_before=$((mem_before + $(used_mib "$vm"))); done
say "members' memory in use before: $mem_before MiB"

debian='{project: "", name: "", kind: "container", start: true, source: {protocol: "simplestreams", server: "https://images.linuxcontainers.org", alias: "debian/13"}}'
start=$(date +%s)
for a in $(seq 1 "$APPS"); do
  body=$(jq -n --argjson n "$MODULES" "{project: \"apelora\",
    document: {apiVersion: \"fabric.apelora/v1alpha1\", kind: \"ApplicationAssembly\"},
    modules: [range(1; \$n + 1) | {module: \"m-\\(. | tostring)\", spec: $debian}],
    connections: [range(2; \$n + 1) | {from: \"m-1\", to: \"m-\\(. | tostring)\", transport: \"tcp\", port: 80}]}")
  api -X POST -H "Idempotency-Key: measure-$a" -d "$body" "$API/applications/$(app_name "$a")/deployments" >/dev/null
done
running=0
for _ in $(seq 1 $((60 + total * 2))); do
  running=0
  for a in $(seq 1 "$APPS"); do
    n=$(api "$API/applications/$(app_name "$a")/status" | jq '[.items[]? | select(.status == "Running")] | length')
    running=$((running + n))
  done
  [ "$running" -ge "$total" ] && break
  sleep 5
done
converged=$(( $(date +%s) - start ))
m=$(scrape)
for kind in instance.create network.sync; do
  read -r s c <<<"$(hist "$m" apelora_worker_job_duration_seconds "kind=\"$kind\"")"
  say "jobs $kind: $c attempts, mean $(mean_ms "$s" "$c") ms each"
done
read -r s c <<<"$(hist "$m" apelora_incus_request_duration_seconds 'endpoint="/1.0/network-acls/{id}",method="PUT"')"
say "ACL rule writes: $c, mean $(mean_ms "$s" "$c") ms each"
say "oldest wait of a create job: $(psql_ "SELECT round(max(extract(epoch FROM (j.updated_at - j.created_at)))) FROM jobs j JOIN applications a ON a.id = j.application_id WHERE a.name LIKE 'msr%' AND j.kind = 'instance.create'") s from queued to done"
[ "$running" -ge "$total" ] || fail "only $running of $total running after $converged s"
say "converged: $total running in $converged s ($(awk -v t="$converged" -v n="$total" 'BEGIN {printf "%.1f", t / n}') s per instance)"

m0=$(scrape)
sleep "$SETTLE"
m1=$(scrape)
read -r s0 c0 <<<"$(hist "$m0" apelora_supervisor_pass_duration_seconds)"
read -r s1 c1 <<<"$(hist "$m1" apelora_supervisor_pass_duration_seconds)"
read -r is in_ <<<"$(hist "$m1" apelora_incus_request_duration_seconds)"
passes=$(awk -v a="$c0" -v b="$c1" 'BEGIN {print b - a}')
read -r _ syncs <<<"$(hist "$m1" apelora_worker_job_duration_seconds 'kind="network.sync"')"
read -r _ writes <<<"$(hist "$m1" apelora_incus_request_duration_seconds 'endpoint="/1.0/network-acls/{id}",method="PUT"')"
say "in all: $syncs network.sync attempts wrote ACL rules $writes times for $total modules (unchanged rules are not written)"
say "steady state over ${SETTLE} s: $passes supervisor passes (leader only), mean $(mean_ms "$(awk -v a="$s0" -v b="$s1" 'BEGIN {print b - a}')" "$passes") ms each; Incus requests so far: $in_, mean $(mean_ms "$is" "$in_") ms"
db_after=$(psql_ "SELECT pg_database_size('apelora')")
say "loaded: control plane $(rss_mib "$cp_pid") MiB, workers $(rss_mib "${worker_pids[@]}") MiB together, database $((db_after / 1048576)) MiB (+$(( (db_after - db_before) / 1024 )) KiB)"
say "rows: jobs $(psql_ "SELECT count(*) FROM jobs"), audit $(psql_ "SELECT count(*) FROM audit_events")"
say "Incus's figures for a few containers (name, memory with page cache, disk): $(incus_ list --project apelora -c nmD --format csv | grep '^msr' | head -3 | tr '\n' ' ')"
placed=$(for a in $(seq 1 "$APPS"); do api "$API/applications/$(app_name "$a")/status" | jq -r '.items[].location'; done | sort | uniq -c | awk '{printf "%s %s, ", $2, $1}')
say "placement: $placed"
mem_after=0
for vm in $vms; do mem_after=$((mem_after + $(used_mib "$vm"))); done
say "members' memory in use after: $mem_after MiB (+$((mem_after - mem_before)) MiB, $(( (mem_after - mem_before) / total )) MiB per container)"
