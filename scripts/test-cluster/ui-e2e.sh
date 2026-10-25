#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# End-to-end test through the screens, on the disposable test cluster: the real control plane,
# worker, Fabric Console and Studio, driven by Playwright (tests/lab/). It signs in, sets up a
# local account, builds applications on the canvas, deploys them, checks their addresses and
# traffic from inside the cluster, operates them, and removes everything again.
#
# Isolation: its own database (created and dropped here), its own ports, and only processes it
# started are stopped. Mutates only the test cluster (Incus project apelora, applications e2e-*).
#
# Needs: the test cluster with at least three members (scripts/test-cluster/{up,add-member,ovn,
# placement}.sh), the dev PostgreSQL (infra/dev/compose.yaml), jq, and Playwright's Chromium.
#   bash scripts/test-cluster/ui-e2e.sh            # run the tests
#   bash scripts/test-cluster/ui-e2e.sh --serve    # only start everything, for trying the screens
set -euo pipefail

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
run=$(mktemp -d -t apelora-ui-e2e)
log="$run/ui-e2e.log"
mode=${1:-test}
export APELORA_API_ADDR=${APELORA_API_ADDR:-127.0.0.1:18383}
STUDIO_PORT=${LAB_STUDIO_PORT:-5373}
CONSOLE_PORT=${LAB_CONSOLE_PORT:-5374}
DB_CONTAINER=${LAB_DB_CONTAINER:-apelora-dev-db}
db="apelora_ui_e2e_$(date +%s)"
export DATABASE_URL="postgres://apelora:apelora-dev-only@127.0.0.1:15432/$db"
API=http://$APELORA_API_ADDR/api/v1
# shellcheck disable=SC1091
source "$HOME/.config/apelora/test-cluster/env"

say() { printf '%s %s\n' "$(date +%T)" "$*" | tee -a "$log"; }
fail() { say "FAIL  $*"; exit 1; }
member() { limactl shell --workdir / "$1" sudo bash -s 2>>"$log"; }   # script on stdin
incus_() { echo "incus $* </dev/null" | member apelora-incus; }
api() { curl -sS -H "Authorization: Bearer $(cat "$run/token")" -H 'Content-Type: application/json' "$@"; }
free() { ! curl -s -m 2 -o /dev/null "http://$1/"; }

# Background servers get their own process groups: stopping this script's group then leaves them
# up for the cleanup, which removes the applications through the control plane.
set -m
pids=()
cleanup() {
  set +e +m
  say "cleanup"
  if [ -s "$run/token" ]; then
    apps=$(api "$API/applications" | jq -r '.items[]?.name | select(startswith("e2e-"))')
    for app in $apps; do
      project=$(api "$API/applications/$app/status" | jq -r '.items[0].project // "apelora"')
      api -X POST -H "Idempotency-Key: ui-e2e-cleanup-$app" \
        -d "{\"project\":\"$project\",\"document\":{\"apiVersion\":\"fabric.apelora/v1alpha1\"},\"modules\":[],\"connections\":[],\"removeMissing\":true}" \
        "$API/applications/$app/deployments" >/dev/null
    done
    # Instances go first; the application's network goes with the network sync that follows.
    for _ in $(seq 1 48); do
      { incus_ list --project apelora -c n --format csv; incus_ network list --project apelora -c n --format csv; } | grep -q '^e2e-' || break
      sleep 5
    done
    # Volumes outlive their module on purpose (data is never deleted by a redeploy); delete them by name.
    for app in $apps; do
      for v in $(api "$API/applications/$app/volumes" | jq -r '.items[]?.incusName'); do
        api -X DELETE -d "{\"confirm\":\"$v\"}" "$API/applications/$app/volumes/$v" >/dev/null
      done
    done
  fi
  for p in "${pids[@]}"; do kill "$p" 2>/dev/null; done
  sleep 2
  docker exec -i "$DB_CONTAINER" dropdb -U apelora --if-exists --force "$db" >/dev/null
  rm -f "$run/token"
  say "left on the cluster: $(incus_ list --project apelora -c n --format csv | tr '\n' ' ')/ networks: $(incus_ network list --project apelora -c n --format csv | tr '\n' ' ')/ e2e volumes: $(incus_ storage volume list "${APELORA_VOLUME_POOL:-local}" --project apelora -c n --format csv | grep -c '^e2e-')"
  say "log: $log"
}
trap cleanup EXIT
trap 'exit 130' INT TERM

say "Apelora end-to-end test through the screens — run directory $run"
for a in "$APELORA_API_ADDR" "127.0.0.1:$STUDIO_PORT" "127.0.0.1:$CONSOLE_PORT"; do
  free "$a" || fail "something already listens on $a (set APELORA_API_ADDR, LAB_STUDIO_PORT or LAB_CONSOLE_PORT)"
done
[ "$(incus_ cluster list --format csv -c s | grep -c ONLINE)" -ge 3 ] || fail "three online members are needed"

# --- Database, control plane, worker ------------------------------------------------------
docker exec -i "$DB_CONTAINER" createdb -U apelora "$db" || fail "cannot create database $db in $DB_CONTAINER"
(cd "$repo" && cargo build -q -p apelora-control-plane -p apelora-deployment-worker)
# The worker installs this into a design's resolver module.
APELORA_DNS_BINARY=$(bash "$here/build-dns.sh") || fail "could not build the Linux resolver"
export APELORA_DNS_BINARY
umask 077
"$repo/target/debug/apelora-control-plane" token create --name ui-e2e --role fabric-admin --expires-in-days 1 2>>"$log" \
  | tail -1 | tr -d '[:space:]' > "$run/token"
[ -s "$run/token" ] || fail "no administrator token"
export APELORA_NETWORK_UPLINK=${APELORA_NETWORK_UPLINK:-incusbr0} \
  APELORA_SERVICE_ADDRESSES=${APELORA_SERVICE_ADDRESSES:-198.18.100.0/28} \
  APELORA_VOLUME_POOL=${APELORA_VOLUME_POOL:-local} \
  APELORA_SUPERVISE_SECS=10 RUST_LOG=info
"$repo/target/debug/apelora-control-plane" > "$run/control-plane.log" 2>&1 & pids+=($!)
"$repo/target/debug/apelora-deployment-worker" > "$run/worker.log" 2>&1 & pids+=($!)
for _ in $(seq 1 30); do [ "$(curl -s "$API/health" | jq -r .incus.connected 2>/dev/null)" = true ] && break; sleep 1; done
[ "$(curl -s "$API/health" | jq -r .incus.connected)" = true ] || fail "control plane not connected to Incus"
say "control plane on $APELORA_API_ADDR against Incus $(api "$API/system" | jq -r .incus.serverVersion), database $db"

# --- Studio and Fabric Console, proxying /api to this control plane --------------------------
export APELORA_API_URL="http://$APELORA_API_ADDR"
# `exec`, so the recorded PID is Vite's own and stopping it stops the server.
for app in studio:"$STUDIO_PORT" fabric-console:"$CONSOLE_PORT"; do
  (cd "$repo" && exec node_modules/.bin/vite --config "apps/${app%%:*}/vite.config.ts" "apps/${app%%:*}" \
    --host 127.0.0.1 --port "${app##*:}" --strictPort) > "$run/${app%%:*}.log" 2>&1 & pids+=($!)
done
for _ in $(seq 1 60); do curl -s -o /dev/null "http://127.0.0.1:$STUDIO_PORT/" && curl -s -o /dev/null "http://127.0.0.1:$CONSOLE_PORT/" && break; sleep 1; done
say "Studio http://127.0.0.1:$STUDIO_PORT · Fabric Console http://127.0.0.1:$CONSOLE_PORT · token in $run/token"

export LAB_E2E=1 LAB_RUN_DIR="$run" LAB_TOKEN_FILE="$run/token" LAB_API_URL="$API" \
  LAB_STUDIO_URL="http://127.0.0.1:$STUDIO_PORT" LAB_CONSOLE_URL="http://127.0.0.1:$CONSOLE_PORT"
if [ "$mode" = --serve ]; then
  say "serving until interrupted (Ctrl-C cleans up)"
  wait
else
  (cd "$repo" && npx playwright test -c tests/lab/playwright.config.ts) 2>&1 | tee -a "$log"
fi
