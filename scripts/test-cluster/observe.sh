#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Example collector on the workstation (Prometheus, Loki, Tempo, Pyroscope, Grafana, Alloy).
# Reads Apelora metrics (pull), JSON logs (file tail) and OTLP traces (optional push to loopback).
# Dry-run unless APPLY=1. Binds 127.0.0.1 only. Development Grafana login is in infra/observe/compose.yaml.
#
#   bash scripts/test-cluster/observe.sh          # print the plan
#   APPLY=1 bash scripts/test-cluster/observe.sh  # start, wait until ready, leave running
#   APPLY=1 STOP=1 bash scripts/test-cluster/observe.sh  # stop
set -euo pipefail

APPLY=${APPLY:-0}
STOP=${STOP:-0}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
compose="$repo/infra/observe/compose.yaml"
DIR=${APELORA_TEST_DIR:-$HOME/.config/apelora/test-cluster}
LOGDIR=${APELORA_OBSERVE_LOG_DIR:-$DIR/logs}

echo "Observe plan: Prometheus :9090, Loki :3100, Tempo :3200/:4318, Pyroscope :4040, Grafana :3000, Alloy tails $LOGDIR"
[ "$APPLY" = 1 ] || echo "Dry-run. APPLY=1 to start the collector on this workstation."

if [ "$STOP" = 1 ]; then
  echo "stop collector"
  if [ "$APPLY" = 1 ]; then
    docker compose -f "$compose" down
  fi
  exit 0
fi

need() { command -v "$1" >/dev/null || { echo "missing: $1" >&2; exit 1; }; }
need docker

if [ "$APPLY" != 1 ]; then
  echo "DRY-RUN mkdir $LOGDIR"
  echo "DRY-RUN APELORA_OBSERVE_LOG_DIR=$LOGDIR docker compose -f infra/observe/compose.yaml up -d"
  echo "DRY-RUN wait for Grafana /api/health, Prometheus /-/ready, Loki /ready, Tempo /ready, Pyroscope /"
  echo "Then start Apelora with:"
  echo "  APELORA_LOG_FORMAT=json APELORA_METRICS_ADDR=127.0.0.1:9470 APELORA_OTLP_ENDPOINT=http://127.0.0.1:4318"
  echo "  … 2>&1 | tee $LOGDIR/control-plane.log"
  exit 0
fi

mkdir -p "$LOGDIR"
export APELORA_OBSERVE_LOG_DIR="$LOGDIR"
docker compose -f "$compose" up -d

wait_http() {
  local name=$1 url=$2
  local i
  for i in $(seq 1 60); do
    if curl -fsS -m 2 -o /dev/null "$url"; then
      echo "ready $name"
      return 0
    fi
    sleep 2
  done
  echo "FAIL  $name did not become ready ($url)" >&2
  docker compose -f "$compose" ps
  exit 1
}

wait_http grafana "http://127.0.0.1:3000/api/health"
wait_http prometheus "http://127.0.0.1:9090/-/ready"
# Loki and Tempo /ready stay 503 until their rings settle; the status URLs answer sooner.
wait_http loki "http://127.0.0.1:3100/loki/api/v1/status/buildinfo"
wait_http tempo "http://127.0.0.1:3200/status"
wait_http pyroscope "http://127.0.0.1:4040/"

echo "Grafana http://127.0.0.1:3000  (development login; password in infra/observe/compose.yaml)"
echo "Point Apelora at this collector:"
echo "  APELORA_LOG_FORMAT=json"
echo "  APELORA_METRICS_ADDR=127.0.0.1:9470"
echo "  APELORA_OTLP_ENDPOINT=http://127.0.0.1:4318"
echo "  tee JSON logs into $LOGDIR/*.log"
echo "Incus /1.0/metrics stays on each member (scripts/test-cluster/metrics.sh)."
echo "done: collector running on loopback. Hardware issues stay open."
