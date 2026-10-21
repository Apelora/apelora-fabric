#!/opt/homebrew/bin/bash
# SPDX-License-Identifier: AGPL-3.0-only
# HA slice B drill on the disposable test cluster (ADR 0013): a stateful module on the shared
# pool restarts on another member after its member is force-stopped and fenced.
#
#   ha-drill.sh plan            # default: check the lab, print every step, write a plan file
#   ha-drill.sh run <planfile>  # do it; refuses without a fresh plan for the same lab state
#
# HA_DRILL_BMC=1 (set it for both plan and run) changes the failure and the fence (ADR 0014):
# instead of force-stopping the member, its Incus port is cut with nftables so the VM keeps
# running, keeps renewing its storage lease (the SAN is still reachable) and only Apelora's
# BMC fence provider (`lima`, which stops the VM) can fence it.
#
# What `run` changes, and only on the lab: a database `apelora_ha_drill_<ts>` on the dev
# PostgreSQL (dropped afterwards), a control plane and a worker of this checkout on 127.0.0.1:8095
# and :9495, one application `apelora-ha-test-<id>` (module `db`, root disk and volume on `shared`),
# and one member VM force-stopped with `limactl stop -f` and started again. Never `apelora-san`,
# never more than one member, never the member the API, the OVN database and the cluster database
# leader run on, and only when the voters left keep the quorum.
#
# Needs: up.sh, ovn.sh, shared-storage.sh, fence-probe.sh apply; the dev PostgreSQL; jq.
set -euo pipefail

MODE=${1:-plan}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
CONF=$HOME/.config/apelora/test-cluster
API_ADDR=127.0.0.1:8095
METRICS_ADDR=127.0.0.1:9495
POOL=${APELORA_SHARED_POOL:-shared}
declare -A VMS=([apelora-test-1]=apelora-incus [apelora-test-2]=apelora-incus-2 [apelora-test-3]=apelora-incus-3 [apelora-test-4]=apelora-incus-4)
CORE=apelora-test-1   # the API address, the OVN central database: never stopped
BMC=${HA_DRILL_BMC:-0}

# shellcheck disable=SC1091
source "$CONF/env"
member() { limactl shell --workdir / "$1" sudo bash -s 2>/dev/null; }   # script on stdin
incus_() { echo "incus $* </dev/null" | member apelora-incus; }
say() { printf '%s %s\n' "$(date +%T)" "$*" | tee -a "${log:-/dev/null}"; }

# ---- the lab as it is now (read-only) ----------------------------------------------------------
check() {
  local ok=1
  for vm in apelora-incus apelora-incus-2 apelora-incus-3 apelora-incus-4 apelora-san; do
    st=$(limactl list --format '{{.Name}} {{.Status}}' | awk -v v="$vm" '$1==v {print $2}')
    [ "$st" = Running ] || { echo "NOT READY: VM $vm is ${st:-missing}"; ok=0; }
  done
  json=$(incus_ cluster list -f json)
  members=$(echo "$json" | jq -r '.[] | "\(.server_name) \(.status) \(.roles | join("+"))"')
  echo "$json" | jq -e 'all(.status == "Online")' >/dev/null || { echo "NOT READY: not every member is Online:"; echo "$members"; ok=0; }
  voters=$(echo "$json" | jq '[.[] | select(.roles | index("database"))] | length')
  leader=$(echo "$json" | jq -r '.[] | select(.roles | index("database-leader")) | .server_name')
  pool=$(incus_ storage list -f csv | awk -F, -v p="$POOL" '$1==p {print $2" "$NF}')
  [ "$pool" = "lvmcluster CREATED" ] || { echo "NOT READY: pool $POOL is '${pool:-missing}'"; ok=0; }
  if [ -f "$CONF/fence-probe.env" ]; then
    # Lima makes new SSH host keys at every boot: a VM restarted since `fence-probe.sh apply`
    # fails the pinned host key, and the worker could not read its lease.
    probes=$("$here/fence-probe.sh" status 2>&1 || true)
    echo "$probes" | grep -q "probe failed" && { echo "NOT READY: the fence probe fails on a member (run fence-probe.sh apply again):"; echo "$probes" | grep -B1 "probe failed"; ok=0; }
  else
    echo "NOT READY: no fence probe (fence-probe.sh apply)"; ok=0
  fi
  docker exec apelora-dev-db psql -U apelora -d apelora -Atqc 'select 1' >/dev/null 2>&1 || { echo "NOT READY: the dev PostgreSQL (apelora-dev-db) does not answer"; ok=0; }
  for a in $API_ADDR $METRICS_ADDR; do
    lsof -iTCP:"${a#*:}" -sTCP:LISTEN >/dev/null 2>&1 && { echo "NOT READY: port ${a#*:} is in use"; ok=0; }
  done
  left=$(incus_ list --project apelora -c n -f csv | grep -c '^apelora-ha-test-' || true)
  [ "$left" -eq 0 ] || { echo "NOT READY: $left apelora-ha-test-* instances exist already"; ok=0; }
  # A member may be stopped when it is not the core member, not the leader, and the voters left
  # still form a majority of all voters.
  victims=()
  while read -r name voter; do
    [ "$name" = "$CORE" ] || [ "$name" = "$leader" ] && continue
    after=$voters
    [ "$voter" = true ] && after=$((voters - 1))
    [ $((after * 2)) -gt "$voters" ] && victims+=("$name")
  done < <(echo "$json" | jq -r '.[] | "\(.server_name) \(.roles | index("database") != null)"')
  [ ${#victims[@]} -gt 0 ] || { echo "NOT READY: no member can be stopped without risking the quorum"; ok=0; }
  fingerprint=$(echo "$members" | sort | shasum | cut -c1-12)
  [ $ok = 1 ]
}

plan_text() {
  if [ "$BMC" = 1 ]; then
    step7=" 7. cut the member's Incus port (tcp 8443, both directions) with nftables inside its VM: the VM keeps
    running and renewing its storage lease; the worker has APELORA_BMC_MAP (lima provider for the four
    members), APELORA_BMC_ALLOW_LAB=1, APELORA_BMC_FENCE_AFTER_SECS=30. After 30 s Offline the worker
    must read its power state (On), switch it off (limactl stop --force) and read Off back   <-- the one destructive step"
  else
    step7=" 7. limactl stop -f <VM of the member it runs on>      <-- the one destructive step"
  fi
  cat <<EOF
HA drill plan ($(date -u +%FT%TZ))
Lab: members
$(echo "$members" | sed 's/^/  /')
Database voters: $voters, leader: $leader. Members that may be stopped (quorum kept, not $CORE, not the leader): ${victims[*]}
Fingerprint of the member list: $fingerprint

Steps of 'run':
 1. createdb apelora_ha_drill_<ts> on apelora-dev-db; cargo build the control plane and the worker of $repo
 2. token create --role fabric-admin (that database only); start the control plane on $API_ADDR
    (APELORA_SHARED_POOL=$POOL, APELORA_VOLUME_POOL=local) and the worker (deploy identity only,
    fence probe from $CONF/fence-probe.env, APELORA_SUPERVISE_SECS=5, metrics $METRICS_ADDR)
 3. POST /api/v1/applications/apelora-ha-test-<id>/deployments: module db, stateful, debian/13 container,
    volume data 256 MiB at /srv/data, class data-replicated (root disk and volume on $POOL)
 4. incus exec apelora-ha-test-<id>-db -- write a marker file into /srv/data
 5. if it does not run on one of [${victims[*]}]: incus stop / incus move --target ${victims[0]} / incus start
 6. wait until the worker has learned all four members' storage leases (member_lock_hosts)
$step7
 8. every 2 s: Incus member status, GET /api/v1/cluster/recovery, sanlock client gets -h 1 on $CORE,
    the instance's member and status; record the time of each change
 9. check the marker file on the new member
10. limactl start <that VM>; fence-probe.sh apply (Lima makes new host keys at boot); wait until
    ONLINE; check its lease (new generation, learned by the worker) and the pool;
    restore it if Incus marked it Evacuated (it should not); wait until Apelora closes the failure
11. clean up (also on failure): start the VM if still stopped, remove the application, delete its
    volume, stop the control plane and the worker, drop the database
EOF
}

case $MODE in
  plan)
    if ! check; then echo "The lab is not ready; nothing was planned."; exit 1; fi
    out=$(mktemp -t apelora-ha-drill-plan)
    plan_text | tee "$out"
    echo "fingerprint=$fingerprint" >> "$out"
    echo
    echo "Nothing was changed. To run it: $0 run $out"
    exit 0 ;;
  run)
    planfile=${2:-}
    [ -n "$planfile" ] && [ -f "$planfile" ] || { echo "run needs the plan file that '$0 plan' wrote" >&2; exit 2; }
    [ $(( $(date +%s) - $(stat -f %m "$planfile") )) -lt 1800 ] || { echo "the plan is older than 30 minutes: plan again" >&2; exit 2; }
    check || { echo "the lab is not ready any more: nothing was changed" >&2; exit 1; }
    grep -qx "fingerprint=$fingerprint" "$planfile" || { echo "the members changed since the plan: plan again" >&2; exit 1; } ;;
  *) echo "usage: $0 plan | run <planfile>" >&2; exit 2 ;;
esac

# ---- run ----------------------------------------------------------------------------------------
run=$(mktemp -d -t apelora-ha-drill)
log=$run/drill.log
id=$(uuidgen | tr 'A-Z' 'a-z' | cut -c1-6)
APP=apelora-ha-test-$id
INST=$APP-db
DB=apelora_ha_drill_$(date +%s)
API=http://$API_ADDR/api/v1
export DATABASE_URL=postgres://apelora:apelora-dev-only@127.0.0.1:15432/$DB
stopped_vm="" cp_pid="" worker_pid=""
api() { curl -sS -H "Authorization: Bearer $(cat "$run/token")" -H 'Content-Type: application/json' "$@"; }
psql_() { docker exec -i apelora-dev-db psql -U apelora -d "$DB" -Atq "$@"; }

cleanup() {
  set +e
  say "cleanup"
  [ -n "$stopped_vm" ] && { say "starting $stopped_vm again"; limactl start "$stopped_vm" >/dev/null 2>&1; }
  if [ -s "$run/token" ] && [ -n "$cp_pid" ]; then
    api -X POST -H "Idempotency-Key: $APP-cleanup" \
      -d '{"project":"apelora","document":{"apiVersion":"fabric.apelora/v1alpha1"},"modules":[],"connections":[],"removeMissing":true}' \
      "$API/applications/$APP/deployments" >/dev/null
    for _ in $(seq 1 30); do incus_ list --project apelora -c n -f csv | grep -q "^$INST\$" || break; sleep 2; done
  fi
  incus_ list --project apelora -c n -f csv | grep -q "^$INST\$" && incus_ delete --force "$INST" --project apelora
  incus_ storage volume delete "$POOL" "$INST-data" --project apelora 2>/dev/null
  [ -n "$worker_pid" ] && kill "$worker_pid" 2>/dev/null
  [ -n "$cp_pid" ] && kill "$cp_pid" 2>/dev/null
  sleep 2
  docker exec apelora-dev-db psql -U apelora -d apelora -q -c "DROP DATABASE IF EXISTS $DB WITH (FORCE);" >/dev/null
  rm -f "$run/token"
  say "left in project apelora: $(incus_ list --project apelora -c n -f csv | tr '\n' ' ')"
  say "log and timeline: $run"
}
trap cleanup EXIT

say "HA drill $APP; run directory $run"
docker exec apelora-dev-db psql -U apelora -d apelora -q -c "CREATE DATABASE $DB;" >/dev/null
(cd "$repo" && cargo build -q -p apelora-control-plane -p apelora-deployment-worker)
umask 077
"$repo/target/debug/apelora-control-plane" token create --name ha-drill --role fabric-admin 2>/dev/null | tail -1 | tr -d '[:space:]' > "$run/token"
(
  export APELORA_API_ADDR=$API_ADDR APELORA_SHARED_POOL=$POOL APELORA_VOLUME_POOL=local RUST_LOG=info
  exec "$repo/target/debug/apelora-control-plane" > "$run/control-plane.log" 2>&1
) &
cp_pid=$!
(
  unset APELORA_INCUS_ADMIN_CLIENT_CERT APELORA_INCUS_ADMIN_CLIENT_KEY
  # shellcheck disable=SC1091
  source "$CONF/fence-probe.env"
  export APELORA_SUPERVISE_SECS=5 APELORA_WORKER_METRICS_ADDR=$METRICS_ADDR APELORA_ALERT_EVALUATE_SECS=0 RUST_LOG=info
  if [ "$BMC" = 1 ]; then
    for n in "${!VMS[@]}"; do printf '"%s":{"provider":"lima","vm":"%s"},' "$n" "${VMS[$n]}"; done | sed 's/^/{/; s/,$/}/' > "$run/bmc-map.json"
    export APELORA_BMC_MAP=$run/bmc-map.json APELORA_BMC_ALLOW_LAB=1 APELORA_BMC_FENCE_AFTER_SECS=30
  fi
  exec "$repo/target/debug/apelora-deployment-worker" > "$run/worker.log" 2>&1
) &
worker_pid=$!
for _ in $(seq 1 30); do curl -s "$API/health" >/dev/null 2>&1 && break; sleep 1; done
say "control plane (pid $cp_pid) and worker (pid $worker_pid) running on database $DB"

body=$(jq -n '{ project: "apelora", document: {apiVersion: "fabric.apelora/v1alpha1", kind: "ApplicationAssembly"},
  modules: [{ module: "db", availability: "stateful", spec: { project: "", name: "", kind: "container", start: true,
    source: { protocol: "simplestreams", server: "https://images.linuxcontainers.org", alias: "debian/13" },
    volumes: [{ name: "data", sizeMib: 256, path: "/srv/data", class: "data-replicated" }] } }], connections: [] }')
res=$(api -X POST -H "Idempotency-Key: $APP-1" -d "$body" "$API/applications/$APP/deployments")
say "deployed: $(echo "$res" | jq -c '[.plan.items[]? | [.instance, .action]]')"
for _ in $(seq 1 90); do
  st=$(incus_ list "$INST" --project apelora -c sL -f csv)
  [[ $st == RUNNING,* ]] && break
  sleep 2
done
[[ $st == RUNNING,* ]] || { say "FAIL: $INST did not start: $st"; exit 1; }
root=$(incus_ config device get "$INST" root pool --project apelora)
say "$INST runs on ${st#RUNNING,}; root disk on pool $root; volume $(incus_ storage volume list "$POOL" --project apelora -f csv -c n | tr '\n' ' ')"
[ "$root" = "$POOL" ] || { say "FAIL: the root disk is not on $POOL"; exit 1; }
marker=$(uuidgen)
echo "incus exec $INST --project apelora -- sh -c 'echo $marker > /srv/data/marker; sync' </dev/null" | member apelora-incus
loc=${st#RUNNING,}
if [[ " ${victims[*]} " != *" $loc "* ]]; then
  say "$INST is on $loc, which must not be stopped: moving it to ${victims[0]}"
  incus_ stop "$INST" --project apelora
  incus_ move "$INST" --target "${victims[0]}" --project apelora
  incus_ start "$INST" --project apelora
  loc=${victims[0]}
fi
victim=$loc
vm=${VMS[$victim]}
for _ in $(seq 1 60); do
  n=$(psql_ -c "select count(*) from member_lock_hosts")
  [ "$n" -ge 4 ] && break
  sleep 2
done
[ "$n" -ge 4 ] || { say "FAIL: the worker knows only $n members' storage leases; nothing was stopped"; exit 1; }
say "worker knows $n members' storage leases: $(psql_ -c "select string_agg(member||'=host '||host_id||' gen '||generation, ', ' order by member) from member_lock_hosts")"

if [ "$BMC" = 1 ]; then
  say "STEP: cutting $victim ($vm) off from the cluster (tcp 8443) with $INST on it; the VM keeps running"
  t0=$(date +%s)
  stopped_vm=$vm
  member "$vm" <<'CUT'
nft add table inet apelora_drill_cut
nft add chain inet apelora_drill_cut in '{ type filter hook input priority -300; }'
nft add chain inet apelora_drill_cut out '{ type filter hook output priority -300; }'
nft add rule inet apelora_drill_cut in tcp dport 8443 drop
nft add rule inet apelora_drill_cut in tcp sport 8443 drop
nft add rule inet apelora_drill_cut out tcp dport 8443 drop
nft add rule inet apelora_drill_cut out tcp sport 8443 drop
CUT
  say "the cut: $(echo 'nft list table inet apelora_drill_cut | grep -c drop' | member "$vm") drop rules in place; VM state: $(limactl list --format '{{.Status}}' "$vm")"
else
  say "STEP: force-stopping $victim ($vm) with $INST on it"
  t0=$(date +%s)
  stopped_vm=$vm
  limactl stop -f "$vm" >/dev/null 2>&1
fi
declare -A seen=()
mark() { if [ -z "${seen[$1]:-}" ]; then seen[$1]=$(( $(date +%s) - t0 )); say "t+${seen[$1]}s $1${2:+: $2}"; fi; }
host_id=$(psql_ -c "select host_id from member_lock_hosts where member = '$victim'")
gen_before=$(psql_ -c "select generation from member_lock_hosts where member = '$victim'")
for _ in $(seq 1 450); do
  cl=$(incus_ cluster list -f json)
  m=$(echo "$cl" | jq -r --arg v "$victim" '.[] | select(.server_name==$v) | .status')
  [ "$m" = Offline ] && mark "Incus reports $victim Offline"
  for o in $(echo "$cl" | jq -r --arg v "$victim" '.[] | select(.server_name!=$v and .status!="Online") | .server_name'); do
    mark "Incus also reports $o (not stopped) as not Online"
  done
  lease=$(echo "sanlock client gets -h 1" | member apelora-incus | awk -v h="$host_id" '$1=="h" && $2==h {print $7}')
  [ "$lease" = FAIL ] && mark "sanlock on $CORE: host $host_id FAIL"
  [ "$lease" = DEAD ] && mark "sanlock on $CORE: host $host_id DEAD"
  r=$(api "$API/cluster/recovery")
  f=$(echo "$r" | jq -r --arg v "$victim" '.items[] | select(.member==$v)')
  [ -n "$(echo "$f" | jq -r '.failure // empty' 2>/dev/null)" ] && mark "Apelora opened the failure" "$(echo "$f" | jq -r .fencing.reason)"
  [ "$(psql_ -c "select count(*) from audit_events where action = 'cluster.member.power_off' and outcome = 'success'")" -ge 1 ] && mark "Apelora switched $victim off through its BMC and read the state back" "VM now: $(limactl list --format '{{.Status}}' "$vm")"
  [ "$(echo "$f" | jq -r '.fencing.condition' 2>/dev/null)" = fenced ] && mark "Apelora: fenced" "$(echo "$f" | jq -r '.evidence.summary')"
  [ -n "$(echo "$f" | jq -r '.decidedAt // empty' 2>/dev/null)" ] && mark "Apelora: decision recorded, restart queued"
  st=$(incus_ list "$INST" --project apelora -c sL -f csv)
  [[ $st != *,"$victim" && -n $st ]] && mark "relinked to ${st#*,}" "$st"
  if [[ $st == RUNNING,* && $st != *,"$victim" ]]; then mark "$INST RUNNING on ${st#RUNNING,}"; break; fi
  sleep 2
done
[[ $st == RUNNING,* && $st != *,"$victim" ]] || { say "FAIL: $INST did not restart elsewhere: $st"; exit 1; }
got=$(echo "incus exec $INST --project apelora -- cat /srv/data/marker </dev/null" | member apelora-incus)
[ "$got" = "$marker" ] && say "PASS: the marker written before the failure is there on ${st#RUNNING,}" || { say "FAIL: marker '$got'"; exit 1; }
psql_ -c "select 'failure '||id||' detected '||detected_at||' heartbeat '||coalesce(last_heartbeat::text,'-')||' fenced '||coalesce(fenced_at::text,'-')||' decided '||coalesce(decided_at::text,'-') from member_failures" | tee -a "$log"
psql_ -c "select at||' '||actor||' '||action||' '||target from audit_events where action like 'cluster.member.%' or action like 'application.instance.%' order by at" | tee -a "$log"
psql_ -c "select 'job '||kind||' '||status||' attempts '||attempts||' operations '||coalesce(array_length(incus_operations, 1), 0) from jobs where kind = 'instance.recover'" | tee -a "$log"

say "STEP: starting $vm again"
t7=$(date +%s)
limactl start "$vm" >/dev/null 2>&1
stopped_vm=""
# Lima gave the VM new SSH host keys: pin them again so the probe reads its lease.
"$here/fence-probe.sh" apply >/dev/null 2>&1 || say "fence-probe.sh apply failed"
for _ in $(seq 1 120); do
  m=$(incus_ cluster list -f json | jq -r --arg v "$victim" '.[] | select(.server_name==$v) | .status')
  [ "$m" = Online ] && break
  sleep 2
done
say "t+$(( $(date +%s) - t7 ))s after start: $victim is $m"
[ "$m" = Evacuated ] && { say "restoring $victim (Incus evacuated it)"; incus_ cluster restore "$victim" --force; }
for _ in $(seq 1 30); do
  own=$(echo "sanlock client gets -h 1" | member "$vm" | awk -v h="$host_id" '$1=="h" && $2==h {print "gen "$4" "$7}')
  [[ $own == *LIVE ]] && break
  sleep 2
done
say "its own lease, as it sees it: host $host_id ${own:-not listed yet}"
say "pool $POOL: $(incus_ storage list -f csv | grep "^$POOL,")"
for _ in $(seq 1 30); do
  [ "$(api "$API/cluster/recovery" | jq '.items | length')" = 0 ] && [ "$(psql_ -c "select count(*) from member_failures where resolved_at is null")" = 0 ] && break
  sleep 2
done
say "open failures: $(psql_ -c "select count(*) from member_failures where resolved_at is null")"
for _ in $(seq 1 60); do
  g=$(psql_ -c "select generation from member_lock_hosts where member = '$victim'")
  [ "${g:-0}" -gt "$gen_before" ] && break
  sleep 2
done
[ "${g:-0}" -gt "$gen_before" ] && say "t+$(( $(date +%s) - t7 ))s after start: the worker learned $victim's new lease: generation $g (was $gen_before)" \
  || say "FAIL: the worker did not learn $victim's new lease (generation still ${g:-unknown})"
say "instances named $INST: $(incus_ list --project apelora -c nsL -f csv | grep "^$INST," | tr '\n' ' ')"
say "PASS: drill finished"
