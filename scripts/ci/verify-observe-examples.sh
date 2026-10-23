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

python3 - "$root/infra/observe/grafana/dashboards/apelora-overview.json" <<'PY'
import json, sys
path = sys.argv[1]
dash = json.load(open(path, encoding="utf-8"))
assert dash.get("uid") == "apelora-overview", dash.get("uid")
assert dash.get("title") == "Apelora", dash.get("title")
assert any(p.get("type") == "logs" for p in dash["panels"]), "need a logs panel"
assert any("apelora_http_request_duration_seconds" in str(p) for p in dash["panels"]), "need API latency"
print("observe dashboard: ok")
PY
