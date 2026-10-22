#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# IPv6 on the disposable test cluster (issue #14): deploys a client connected to a web module
# on port 80 and a module with no connection, then checks over IPv4 and IPv6 that the
# connection is allowed and the unconnected module is refused. The lab uplink routes
# `fd42:198:100::/112` (`ovn.sh` `ipv6.routes`; the bridge itself stays `ipv6.address=none`),
# so this also checks an IPv6 service address (OVN load balancer, `/128` backends). The
# servers' own cluster addresses stay IPv4. Every check prints PASS or FAIL; the first FAIL
# stops the run (cleanup still happens).
#
# Needs: scripts/test-cluster/{up,ovn}.sh done, the dev PostgreSQL (infra/dev/compose.yaml), jq,
# and nothing else listening on 127.0.0.1:8080. Mutates only the test cluster.
#   bash scripts/test-cluster/ipv6.sh
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
run=$(mktemp -d -t apelora-ipv6)
log="$run/ipv6.log"
APP=v6check
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
# Opens a TCP connection from instance $1 to address $2 port 80: prints OPEN or blocked.
reach() {
  echo "incus exec $1 --project apelora -- bash -c \"timeout 5 bash -c 'exec 3<>/dev/tcp/$2/80' 2>/dev/null && echo OPEN || echo blocked\" </dev/null" | member apelora-incus
}
# The instance's global address of one family on eth0 (4 or 6). Instance state, not `incus list`
# CSV: a list of addresses is quoted, and `head -1` would return the opening quote.
addr() {
  local fam; [ "$2" = 6 ] && fam=inet6 || fam=inet
  incus_ query "/1.0/instances/$1/state?project=apelora" | jq -r --arg fam "$fam" \
    '(.network // .metadata.network).eth0.addresses[]? | select(.family == $fam and .scope == "global") | .address' | head -1
}

cp_pid="" worker_pid=""
cleanup() {
  set +e
  say "cleanup"
  if [ -s "$run/token" ]; then
    res=$(api -X POST -H 'Idempotency-Key: v6check-cleanup' \
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
    # The token first and on its own: a failing statement must never keep a credential alive.
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

say "IPv6 connections and service address — run directory $run"
if curl -s -m 2 -o /dev/null http://127.0.0.1:8080/; then fail "something already listens on 127.0.0.1:8080"; fi
(cd "$repo" && cargo build -q -p apelora-control-plane -p apelora-deployment-worker)
umask 077
out=$("$repo/target/debug/apelora-control-plane" token create --name v6check --role fabric-admin 2>&1)
echo "$out" | tail -1 | tr -d '[:space:]' > "$run/token"
echo "$out" | grep -o '^Created token [a-f0-9-]*' | awk '{print $3}' > "$run/token-id"
export APELORA_NETWORK_UPLINK=incusbr0 APELORA_SERVICE_ADDRESSES=fd42:198:100::/112 RUST_LOG=info
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
    {module: \"other-01\", spec: $debian},
    {module: \"web-01\", availability: \"stateless\", spec: {project: \"\", name: \"\", kind: \"container\", start: true,
      source: {protocol: \"oci\", server: \"https://docker.io\", alias: \$nginx}}}],
  connections: [{from: \"client-01\", to: \"web-01\", transport: \"tcp\", port: 80}]}")
res=$(api -X POST -H 'Idempotency-Key: v6check-1' -d "$body" "$API/applications/$APP/deployments")
say "plan: $(echo "$res" | jq -c '[.plan.items[] | [.instance, .action]]')"
[ "$(wait_job "$(echo "$res" | jq -r .networkJob)")" = succeeded ] || fail "network sync"
say "application network: $(incus_ network list --project apelora --format csv -c n46 | grep "^$APP," || true)"

web4=$(addr "$APP-web-01" 4); web6=$(addr "$APP-web-01" 6)
[ -n "$web4" ] && pass "web-01 has IPv4 $web4" || fail "web-01 has no IPv4 address"
[ -n "$web6" ] && pass "web-01 has IPv6 $web6 from the application network" || fail "web-01 has no global IPv6 address"
# The address sets the rules match on carry both families.
# The worker fills the sets once the instances have addresses (a later sync if not at first).
for _ in $(seq 1 24); do
  set_members=$(incus_ network address-set show "$APP-web-01" --project apelora | tr -d ' ')
  [[ $set_members == *"$web4/32"* ]] && break
  sleep 5
done
[[ $set_members == *"$web4/32"* ]] && pass "web-01's address set holds $web4/32" || fail "address set lacks the IPv4 address: $set_members"
[[ $set_members == *"$web6/128"* ]] && pass "web-01's address set holds $web6/128" || fail "address set lacks the IPv6 address: $set_members"

for fam in 4 6; do
  dst=$web4; [ "$fam" = 6 ] && dst=$web6
  r=$(reach "$APP-client-01" "$dst"); [ "$r" = OPEN ] && pass "IPv$fam: client-01 reaches web-01:80 (connected)" || fail "IPv$fam: client-01 → web-01:80 is $r"
  r=$(reach "$APP-other-01" "$dst"); [ "$r" = blocked ] && pass "IPv$fam: other-01 cannot reach web-01:80 (no connection)" || fail "IPv$fam: other-01 → web-01:80 is $r"
done

svc=$(api "$API/applications/$APP/status" | jq -r '.services[0].address // empty')
[[ $svc == *:* ]] && pass "service address is IPv6 $svc" || fail "service address is not IPv6: ${svc:-none}"
lb=$(incus_ network load-balancer show "$APP" "$svc" --project apelora)
echo "$lb" | grep -q "$web6" && pass "load balancer backend is IPv6 $web6" || fail "load balancer lacks IPv6 backend: $lb"
r=$(reach "$APP-client-01" "$svc")
[ "$r" = OPEN ] && pass "client-01 reaches the IPv6 service address on port 80" || fail "client-01 → service $svc:80 is $r"
r=$(reach "$APP-other-01" "$svc")
[ "$r" = blocked ] && pass "other-01 cannot reach the IPv6 service address (no connection)" || fail "other-01 → service $svc:80 is $r"
say "done: connections and the service address hold over IPv4 and IPv6"
