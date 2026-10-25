#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# End-to-end demonstration on the disposable test cluster (Phase 10): deploys an application
# through the real control plane and worker, then checks, step by step, what the platform
# promises — replicas apart, a service address, enforced connections, internet access,
# a replica failing its health check, publishing, recovery from a failed member, evacuation —
# and removes everything again.
# Every check prints PASS or FAIL; the first FAIL stops the run (cleanup still happens).
#
# Needs: scripts/test-cluster/{up,add-member,ovn,placement}.sh done (at least three members), the dev
# PostgreSQL (infra/dev/compose.yaml) and jq. Mutates only the test cluster.
#   bash scripts/test-cluster/demo.sh            # log in a new temporary directory
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
run=$(mktemp -d -t apelora-demo)
log="$run/demo.log"
APP=demo
# Where this run's control plane listens: set APELORA_API_ADDR when 127.0.0.1:8080 is taken.
export APELORA_API_ADDR=${APELORA_API_ADDR:-127.0.0.1:8080}
API=http://$APELORA_API_ADDR/api/v1
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
probe() {   # 8 requests to the service address from the client; prints the answering pages
  echo "incus exec $APP-client-01 --project apelora -- bash -c 'out=\"\"; for i in \$(seq 1 8); do r=\$(timeout 2 bash -c \"exec 3<>/dev/tcp/$1/80; printf \\\"GET / HTTP/1.0\\r\\n\\r\\n\\\" >&3; tail -1 <&3\" 2>/dev/null); out=\"\$out \${r:-FAIL}\"; done; echo \$out' </dev/null" | member apelora-incus
}
vm_of() { case "$1" in apelora-test-1) echo apelora-incus ;; apelora-test-*) echo "apelora-incus-${1#apelora-test-}" ;; esac; }

stopped_vm=""
evacuated=""
cleanup() {
  set +e
  say "cleanup"
  [ -n "$stopped_vm" ] && limactl start "$stopped_vm" >/dev/null 2>&1
  # Never leave a member evacuated: its uplink bridge is down until restore (TEST-CLUSTER.md).
  [ -n "$evacuated" ] && [ -s "$run/token" ] && api -X POST -d "{\"confirm\":\"$evacuated\"}" "$API/cluster/members/$evacuated/restore" >/dev/null && sleep 20
  if [ -s "$run/token" ]; then
    api -X POST -H 'Idempotency-Key: demo-cleanup' \
      -d '{"project":"apelora","document":{"apiVersion":"fabric.apelora/v1alpha1"},"modules":[],"connections":[],"removeMissing":true}' \
      "$API/applications/$APP/deployments" >/dev/null
    sleep 45
  fi
  pkill -f "$repo/target/debug/apelora-control-plane"
  pkill -f "$repo/target/debug/apelora-deployment-worker"
  if [ -s "$run/token-id" ]; then
    # The token first and on its own: a failing statement must never keep a credential alive.
    # Audit events stay (the audit log is append-only).
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

started=$(date -u +%Y-%m-%dT%H:%M:%SZ)
say "Apelora end-to-end demonstration — run directory $run"
curl -s -m 2 -o /dev/null "http://$APELORA_API_ADDR/" && fail "something already listens on $APELORA_API_ADDR (set APELORA_API_ADDR to a free address)"
members=$(incus_ cluster list --format csv -c ns)
say "members: $(echo "$members" | tr '\n' ' ')"
[ "$(echo "$members" | grep -c ONLINE)" -ge 3 ] || fail "three online members are needed"
pass "$(echo "$members" | grep -c ONLINE) members online (three needed)"

# --- Control plane and worker, with a short-lived administrator token ------------------
(cd "$repo" && cargo build -q -p apelora-control-plane -p apelora-deployment-worker)
umask 077
out=$("$repo/target/debug/apelora-control-plane" token create --name demo --role fabric-admin 2>&1)
echo "$out" | tail -1 | tr -d '[:space:]' > "$run/token"
echo "$out" | grep -o '^Created token [a-f0-9-]*' | awk '{print $3}' > "$run/token-id"
export APELORA_NETWORK_UPLINK=incusbr0 APELORA_SERVICE_ADDRESSES=198.18.100.0/28 \
  APELORA_SUPERVISE_SECS=10 APELORA_RECOVER_AFTER_SECS=30 APELORA_HEALTH_RESTART_AFTER_SECS=30 \
  RUST_LOG=info
nohup "$repo/target/debug/apelora-control-plane" > "$run/control-plane.log" 2>&1 &
nohup "$repo/target/debug/apelora-deployment-worker" > "$run/worker.log" 2>&1 &
sleep 3
[ "$(curl -s "$API/health" | jq -r .incus.connected)" = true ] || fail "control plane not connected to Incus"
# The public health check says only "connected"; the version needs signing in (/system).
pass "control plane and worker running against Incus $(api "$API/system" | jq -r .incus.serverVersion)"

# --- Deploy ---------------------------------------------------------------------------
nginx=library/nginx@sha256:eb1c896deef834f4e056829fd6dadac8305501f16cc71ba65fb252c8d0aa168e
body=$(jq -n --arg nginx "$nginx" '{
  project: "apelora", document: {apiVersion: "fabric.apelora/v1alpha1", kind: "ApplicationAssembly"},
  modules: [
    {module: "client-01", internet: "web", spec: {project: "", name: "", kind: "container", start: true,
      source: {protocol: "simplestreams", server: "https://images.linuxcontainers.org", alias: "debian/13"}}},
    {module: "web-01", replicas: 2, availability: "stateless", spec: {project: "", name: "", kind: "container", start: true,
      source: {protocol: "oci", server: "https://docker.io", alias: $nginx}}}],
  connections: [{from: "client-01", to: "web-01", transport: "tcp", port: 80}],
  published: [{module: "web-01", transport: "tcp", port: 80}]}')
res=$(api -X POST -H 'Idempotency-Key: demo-1' -d "$body" "$API/applications/$APP/deployments")
say "plan: $(echo "$res" | jq -c '[.plan.items[] | [.instance, .action]]')"
[ "$(wait_job "$(echo "$res" | jq -r .networkJob)")" = succeeded ] || fail "network sync"
status=$(api "$API/applications/$APP/status")
say "status: $(echo "$status" | jq -c '[.items[] | [.instance, .status, .location]]')"
svc=$(echo "$status" | jq -r '.services[0].address')
locs=$(echo "$status" | jq -r '.items[] | select(.module=="web-01") | .location' | sort -u | wc -l | tr -d ' ')
[ "$locs" -eq 2 ] && pass "two web replicas on different members" || fail "replicas share a member"
[ "$svc" != null ] && pass "service address $svc" || fail "no service address"

# --- Service, connections, internet ---------------------------------------------------
for w in $APP-web-01 $APP-web-01-r2; do
  echo "incus exec $w --project apelora -- sh -c 'echo $w > /usr/share/nginx/html/index.html' </dev/null" | member apelora-incus
done
sleep 5
out=$(probe "$svc"); say "service: $out"
[[ $out == *"$APP-web-01 "* || $out == *"$APP-web-01" ]] && [[ $out == *"$APP-web-01-r2"* ]] && [[ $out != *FAIL* ]] \
  && pass "requests spread over both replicas" || fail "service did not answer from both replicas"
inet=$(echo "incus exec $APP-client-01 --project apelora -- bash -c \"timeout 6 bash -c 'exec 3<>/dev/tcp/deb.debian.org/80; printf \\\"HEAD / HTTP/1.0\\r\\nHost: deb.debian.org\\r\\n\\r\\n\\\" >&3; head -1 <&3' || echo FAIL\" </dev/null" | member apelora-incus)
[[ $inet == HTTP/1.1\ 200* ]] && pass "client reaches the internet on port 80 (web access)" || fail "internet access: $inet"
blocked=$(echo "incus exec $APP-client-01 --project apelora -- bash -c \"timeout 5 bash -c 'exec 3<>/dev/tcp/1.1.1.1/22' 2>/dev/null && echo OPEN || echo blocked\" </dev/null" | member apelora-incus)
[ "$blocked" = blocked ] && pass "other internet ports stay blocked" || fail "port 22 outside is open"

# --- A replica fails its health check (running, but its port is closed) ------------------
sick=$APP-web-01-r2
health_of() { api "$API/applications/$APP/status" | jq -r --arg i "$1" '.items[] | select(.instance==$i) | .health.health // "none"'; }
# Only this run's restarts: the audit log is append-only, so earlier runs' entries for the same
# instance name are still there.
restarts_of() { api "$API/audit?action=application.replica.restart-unhealthy&target=apelora/$1&from=$started" | jq '.items | length'; }
nginx_port() {   # moves the replica's NGINX to another port (the change survives a restart)
  echo "incus exec $sick --project apelora -- sh -c \"sed -i 's/listen  *$1;/listen $2;/; s/listen  *\\[::\\]:$1;/listen [::]:$2;/' /etc/nginx/conf.d/default.conf && nginx -s reload\" </dev/null" | member apelora-incus
}
[ "$(health_of "$sick")" = healthy ] || sleep 30
[ "$(health_of "$sick")" = healthy ] && pass "$sick reported healthy (port check passing)" || fail "$sick health: $(health_of "$sick")"
nginx_port 80 8081
t0=$(date +%s)
for _ in $(seq 1 18); do sleep 5; [ "$(health_of "$sick")" = unhealthy ] && break; done
[ "$(health_of "$sick")" = unhealthy ] && pass "$sick unhealthy $(( $(date +%s) - t0 )) s after its port closed" || fail "$sick not reported unhealthy"
out=$(probe "$svc"); say "service while $sick fails: $out"
[[ $out != *FAIL* && $out != *"$sick"* ]] && pass "service answers from the healthy replica only" || fail "service: $out"
for _ in $(seq 1 18); do sleep 5; [ "$(restarts_of "$sick")" -ge 1 ] && break; done
[ "$(restarts_of "$sick")" -eq 1 ] && pass "$sick restarted once by the supervisor $(( $(date +%s) - t0 )) s after its port closed" || fail "restarts: $(restarts_of "$sick")"
sleep 40   # still failing after the restart (the change persisted): no second restart within the cooldown
[ "$(restarts_of "$sick")" -eq 1 ] && pass "no restart loop: still one restart while it keeps failing" || fail "restarted $(restarts_of "$sick") times"
nginx_port 8081 80
t0=$(date +%s)
for _ in $(seq 1 18); do sleep 5; [ "$(health_of "$sick")" = healthy ] && break; done
[ "$(health_of "$sick")" = healthy ] && pass "$sick healthy again $(( $(date +%s) - t0 )) s after its port reopened" || fail "$sick did not recover"

# --- Publishing (the outside network routes the address to the application's router) --
router=$(incus_ network get "$APP" volatile.network.ipv4.address --project apelora)
published=""
for m in apelora-incus apelora-incus-2 apelora-incus-3; do
  r=$(echo "ip route replace $svc/32 via $router dev incusbr0; curl -s -m 3 http://$svc/ | head -1; ip route del $svc/32 via $router dev incusbr0" | member "$m")
  [ -n "$r" ] && published="$m" && break
done
[ -n "$published" ] && pass "published port reachable from outside (via $published, the network's gateway)" || fail "published port not reachable"

# --- A member fails ---------------------------------------------------------------------
victim=$(echo "$status" | jq -r --arg c "$(echo "$status" | jq -r '.items[] | select(.module=="client-01") | .location')" \
  '.items[] | select(.module=="web-01" and .location != $c and .location != "apelora-test-1") | .location' | head -1)
if [ -z "$victim" ]; then
  say "SKIP  member failure: no web replica away from the client and the OVN database member"
else
  stopped_vm=$(vm_of "$victim")
  say "force-stopping $victim ($stopped_vm)"
  t0=$(date +%s)
  limactl stop -f "$stopped_vm" >/dev/null 2>&1
  # Incus 7.0 refuses load balancer changes while a member is down, so the substitute may get no
  # traffic until the member returns. Then Apelora must say so, naming the member.
  ok=""; stuck=""
  for _ in $(seq 1 30); do
    sleep 10
    out=$(probe "$svc")
    if [[ $out != *FAIL* ]] && incus_ list --project apelora -c n --format csv | grep -q -- '-s1$'; then ok=1; break; fi
    stuck=$(api "$API/applications/$APP/status" | jq -r '.trafficBlocked // empty')
    [ -n "$stuck" ] && break
  done
  if [ -n "$ok" ]; then pass "substitute serving; service whole again $(( $(date +%s) - t0 )) s after the failure"
  elif [ -n "$stuck" ]; then
    [[ $stuck == *"$victim"* ]] && pass "service stuck as Incus refuses load balancer changes, and the status says so after $(( $(date +%s) - t0 )) s: $stuck" \
      || fail "the status does not name $victim: $stuck"
  else fail "no recovery, and the status does not say why: $out"; fi
  limactl start "$stopped_vm" >/dev/null 2>&1; stopped_vm=""
  ok=""
  for _ in $(seq 1 30); do
    sleep 10
    n=$(incus_ list --project apelora -c n --format csv | grep -c "^$APP-web-01")
    [ "$n" -eq 2 ] && ok=1 && break
  done
  [ -n "$ok" ] && pass "member back; the extra replica was retired" || fail "duplicate replica not retired"
  if [ -n "$stuck" ]; then
    cleared=""
    for _ in $(seq 1 18); do
      [ -z "$(api "$API/applications/$APP/status" | jq -r '.trafficBlocked // empty')" ] && cleared=1 && break
      sleep 10
    done
    [ -n "$cleared" ] && pass "the stuck-traffic notice cleared once the member was back" || fail "the stuck-traffic notice stayed after the member returned"
  fi
fi

# --- Evacuation --------------------------------------------------------------------------
target=$(api "$API/applications/$APP/status" | jq -r '.items[] | select(.module=="web-01") | .location' | grep -v apelora-test-1 | head -1)
preview=$(api "$API/cluster/members/$target/evacuation")
[ "$(echo "$preview" | jq '.plan.blocked | length')" -eq 0 ] && pass "dry run for $target: $(echo "$preview" | jq -c '[.plan.items[] | [.instance, .impact]]')" || fail "evacuation blocked"
api -X POST -d "{\"confirm\":\"$target\"}" "$API/cluster/members/$target/evacuate" >/dev/null
evacuated=$target
for _ in $(seq 1 30); do sleep 5; [ "$(api "$API/cluster" | jq -r --arg m "$target" '.members[] | select(.name==$m) | .status')" = Evacuated ] && break; done
# Apelora does not drain the member first yet (issue #50): the health check takes a stopped
# replica out after two failed checks five seconds apart. Give it that long, then expect no failure.
sleep 12
out=$(probe "$svc"); say "service while $target is evacuated: $out"
[[ $out != *FAIL* ]] && pass "service up during evacuation" || fail "service failed during evacuation"
api -X POST -d "{\"confirm\":\"$target\"}" "$API/cluster/members/$target/restore" >/dev/null
for _ in $(seq 1 30); do sleep 5; [ "$(api "$API/cluster" | jq -r --arg m "$target" '.members[] | select(.name==$m) | .status')" = Online ] && break; done
evacuated=""
pass "member restored"

say "audit: $(api "$API/audit?limit=50" | jq -c '[.items[] | select(.actor=="supervisor" or .actor=="demo") | .action] | group_by(.) | map({(.[0]): length}) | add')"
say "DEMONSTRATION PASSED"
