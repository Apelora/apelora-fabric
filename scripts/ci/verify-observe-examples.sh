#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Checks the example collector compose file and Grafana dashboard JSON. Does not pull images
# or start the stack. Run in CI (scripts/ci/).
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
need() { command -v "$1" >/dev/null || { echo "missing: $1" >&2; exit 1; }; }
need docker
need python3

APELORA_OBSERVE_LOG_DIR=/tmp/apelora-logs docker compose -f "$root/infra/observe/compose.yaml" config >/dev/null
echo "observe compose: ok"

python3 - "$root" <<'PY'
import json, sys
from pathlib import Path
root = Path(sys.argv[1])
dash = json.loads((root / "infra/observe/grafana/dashboards/apelora-overview.json").read_text(encoding="utf-8"))
assert dash.get("uid") == "apelora-overview", dash.get("uid")
assert dash.get("title") == "Apelora", dash.get("title")
assert any(p.get("type") == "logs" for p in dash["panels"]), "need a logs panel"
assert any("apelora_http_request_duration_seconds" in str(p) for p in dash["panels"]), "need API latency"
print("observe dashboard: ok")

compose = (root / "infra/observe/compose.yaml").read_text(encoding="utf-8")
for port in ("9090", "3100", "3200", "4317", "4318", "4040", "3000"):
    assert f"127.0.0.1:{port}:{port}" in compose, port
assert compose.count("127.0.0.1:") >= 8
print("observe compose loopback: ok")

prom = (root / "infra/observe/prometheus.yml").read_text(encoding="utf-8")
assert "host.docker.internal:9470" in prom
assert "host.docker.internal:9471" in prom
print("observe prometheus scrape: ok")

ds = (root / "infra/observe/grafana/provisioning/datasources/datasources.yml").read_text(encoding="utf-8")
for name in ("Prometheus", "Loki", "Tempo", "Pyroscope"):
    assert name in ds, name
assert "uid: tempo" in ds and "uid: loki" in ds
print("observe grafana datasources: ok")

tempo = (root / "infra/observe/tempo.yml").read_text(encoding="utf-8")
assert "4318" in tempo and "4317" in tempo
print("observe tempo otlp: ok")
PY
