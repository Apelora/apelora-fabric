#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Service names on the disposable test cluster (ADR 0004): deploys a client connected to a
# stateless web module, then checks that the consumer received APELORA_SERVICE_NAME_* and
# that network.sync wrote dns.domain and user.apelora.dns on the application network.
# OVN's own DNS still answers instance names only; the service name is not expected to
# resolve inside the application. Every check prints PASS or FAIL; the first FAIL stops
# the run (cleanup still happens).
#
# Needs: scripts/test-cluster/{up,ovn}.sh done, the dev PostgreSQL (infra/dev/compose.yaml), jq,
# and nothing else listening on 127.0.0.1:8080. Mutates only the test cluster.
#   bash scripts/test-cluster/dns.sh
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
run=$(mktemp -d -t apelora-dns)
log="$run/dns.log"
APP=dnscheck
API=http://127.0.0.1:8080/api/v1
export DATABASE_URL=${DATABASE_URL:-postgres://apelora:apelora-dev-only@127.0.0.1:15432/apelora}
# shellcheck disable=SC1091
source "$HOME/.config/apelora/test-cluster/env"

say() { printf '%s %s\n' "$(date +%T)" "$*" | tee -a "$log"; }
pass() { say "PASS  $*"; }
fail() { say "FAIL  $*"; exit 1; }
member() { limactl shell --workdir / "$1" sudo bash -s 2>>"$log"; }   # script on stdin
incus_() { echo "incus $* </dev/null" | member apelora-incus; }
api() { curl -sS -H "Authorization: Bearer $(cat "$run/token")" -H 'Content-Type: application/json' "$@"; }
wait_job() {
  local id=$1 st=""
  for _ in $(seq 1 60); do
    st=$(api "$API/jobs/$id" | jq -r .status)
    case "$st" in succeeded|failed) break ;; esac
    sleep 5
  done
  echo "$st"
}
# Prints OPEN or blocked for a TCP connection from instance $1 to address $2 port 80.
reach() {
  echo "incus exec $1 --project apelora -- bash -c \"timeout 5 bash -c 'exec 3<>/dev/tcp/$2/80' 2>/dev/null && echo OPEN || echo blocked\" </dev/null" | member apelora-incus
}
# getent hosts from the client; empty when the name does not resolve.
lookup() {
  echo "incus exec $APP-client-01 --project apelora -- getent hosts $1 </dev/null" | member apelora-incus || true
}

cp_pid="" worker_pid=""
cleanup() {
  set +e
  say "cleanup"
  if [ -s "$run/token" ]; then
    res=$(api -X POST -H 'Idempotency-Key: dnscheck-cleanup' \
      -d '{"project":"apelora","document":{"apiVersion":"fabric.apelora/v1alpha1"},"modules":[],"connections":[],"removeMissing":true}' \
      "$API/applications/$APP/deployments")
    id=$(echo "$res" | jq -r '.networkJob // empty')
    [ -n "$id" ] && wait_job "$id" >/dev/null
    for _ in $(seq 1 36); do
      left=$(incus_ list --project apelora -c n --format csv | grep -c "^$APP" || true)
      [ "$left" = 0 ] && break
      sleep 5
    done
  fi
  [ -n "$cp_pid" ] && kill "$cp_pid"
  [ -n "$worker_pid" ] && kill "$worker_pid"
  if [ -s "$run/token-id" ]; then
    docker exec -i apelora-dev-db psql -U apelora -d apelora -q -c "DELETE FROM api_tokens WHERE id = '$(cat "$run/token-id")';" >/dev/null
    docker exec -i apelora-dev-db psql -U apelora -d apelora -q <<SQL >/dev/null
DELETE FROM jobs WHERE application_id IN (SELECT id FROM applications WHERE name = '$APP');
DELETE FROM applications WHERE name = '$APP';
SQL
  fi
  rm -f "$run/token" "$run/token-id"
  say "left on the cluster: $(incus_ list --project apelora -c n --format csv | tr '\n' ' ')"
  say "log: $log"
}
trap cleanup EXIT

say "Service names — run directory $run"
if curl -s -m 2 -o /dev/null http://127.0.0.1:8080/; then fail "something already listens on 127.0.0.1:8080"; fi
(cd "$repo" && cargo build -q -p apelora-control-plane -p apelora-deployment-worker)
umask 077
out=$("$repo/target/debug/apelora-control-plane" token create --name dnscheck --role fabric-admin 2>&1)
echo "$out" | tail -1 | tr -d '[:space:]' > "$run/token"
echo "$out" | grep -o '^Created token [a-f0-9-]*' | awk '{print $3}' > "$run/token-id"
export APELORA_NETWORK_UPLINK=incusbr0 APELORA_SERVICE_ADDRESSES=198.18.100.0/28 RUST_LOG=info
"$repo/target/debug/apelora-control-plane" > "$run/control-plane.log" 2>&1 & cp_pid=$!
"$repo/target/debug/apelora-deployment-worker" > "$run/worker.log" 2>&1 & worker_pid=$!
sleep 3
[ "$(curl -s "$API/health" | jq -r .incus.connected)" = true ] || fail "control plane not connected to Incus"

nginx=library/nginx@sha256:eb1c896deef834f4e056829fd6dadac8305501f16cc71ba65fb252c8d0aa168e
debian='{project: "", name: "", kind: "container", start: true, source: {protocol: "simplestreams", server: "https://images.linuxcontainers.org", alias: "debian/13"}}'
body=$(jq -n --arg nginx "$nginx" "{
  project: \"apelora\", document: {apiVersion: \"fabric.apelora/v1alpha1\", kind: \"ApplicationAssembly\"},
  modules: [
    {module: \"client-01\", spec: $debian},
    {module: \"web-01\", availability: \"stateless\", spec: {project: \"\", name: \"\", kind: \"container\", start: true,
      source: {protocol: \"oci\", server: \"https://docker.io\", alias: \$nginx}}}],
  connections: [{from: \"client-01\", to: \"web-01\", transport: \"tcp\", port: 80}]}")
res=$(api -X POST -H 'Idempotency-Key: dnscheck-1' -d "$body" "$API/applications/$APP/deployments")
say "plan: $(echo "$res" | jq -c '[.plan.items[] | [.instance, .action]]')"
[ "$(wait_job "$(echo "$res" | jq -r .networkJob)")" = succeeded ] || fail "network sync"

svc=$(api "$API/applications/$APP/status" | jq -r '.services[0].address // empty')
[ -n "$svc" ] && pass "service address $svc" || fail "no service address"

domain=$(incus_ network get "$APP" dns.domain --project apelora)
[ "$domain" = "$APP.internal" ] && pass "dns.domain is $domain" || fail "dns.domain is ${domain:-empty}"
records=$(incus_ network get "$APP" user.apelora.dns --project apelora)
echo "$records" | grep -q "web-01.$APP.internal $svc" && pass "user.apelora.dns names web-01 at $svc" || fail "user.apelora.dns is ${records:-empty}"

name=$(incus_ config get "$APP-client-01" environment.APELORA_SERVICE_NAME_WEB_01 --project apelora)
[ "$name" = "web-01.$APP.internal" ] && pass "client has APELORA_SERVICE_NAME_WEB_01=$name" || fail "APELORA_SERVICE_NAME_WEB_01 is ${name:-empty}"
addr=$(incus_ config get "$APP-client-01" environment.APELORA_SERVICE_WEB_01 --project apelora)
[ "$addr" = "$svc" ] && pass "client has APELORA_SERVICE_WEB_01=$addr" || fail "APELORA_SERVICE_WEB_01 is ${addr:-empty}"

r=$(reach "$APP-client-01" "$svc")
[ "$r" = OPEN ] && pass "client-01 reaches the service address on port 80" || fail "client-01 → $svc:80 is $r"

hosts=$(lookup "$APP-web-01")
echo "$hosts" | grep -q . && pass "OVN DNS answers the instance name: $hosts" || fail "OVN DNS does not answer $APP-web-01"
svc_hosts=$(lookup "web-01.$APP.internal")
[ -z "$svc_hosts" ] && pass "OVN DNS does not answer the service name (records are for a resolver)" || fail "OVN DNS unexpectedly answered web-01.$APP.internal: $svc_hosts"
say "done: service names are published; OVN still answers instance names only"
