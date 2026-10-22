#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Checks the example systemd units and the NGINX configuration parse. Run in CI on
# ubuntu-24.04 (scripts/ci/). Does not start the services or talk to a cluster.
# On macOS (no systemd, no nginx as the example) the checks are skipped unless
# APELORA_REQUIRE_INSTALL_CHECKS=1, which CI sets.
set -euo pipefail

root=$(cd "$(dirname "$0")/../.." && pwd)
need_linux=${APELORA_REQUIRE_INSTALL_CHECKS:-0}

have() { command -v "$1" >/dev/null 2>&1; }

if ! have systemd-analyze || ! have nginx; then
  if [ "$need_linux" = 1 ]; then
    echo "systemd-analyze and nginx are required (ubuntu-24.04 CI)" >&2
    exit 1
  fi
  echo "skip: systemd-analyze/nginx not on this machine (CI runs them on ubuntu-24.04)"
  exit 0
fi

if ! getent passwd apelora >/dev/null 2>&1; then
  sudo useradd --system --no-create-home --shell /usr/sbin/nologin apelora
fi
sudo mkdir -p /usr/local/bin /etc/apelora /opt/apelora/ops /srv/apelora-backups \
  /etc/apelora/serve /etc/apelora/tls /srv/apelora/studio /srv/apelora/console
sudo install -m 0755 /bin/true /usr/local/bin/apelora-control-plane
sudo install -m 0755 /bin/true /usr/local/bin/apelora-deployment-worker
sudo install -m 0755 /bin/true /opt/apelora/ops/db-backup.sh
sudo install -m 0755 /bin/true /opt/apelora/ops/db-restore-drill.sh
printf 'DATABASE_URL=postgres://apelora@127.0.0.1/apelora\n' | sudo tee /etc/apelora/apelora.env >/dev/null
printf 'APELORA_DB_BACKUP_DIR=/srv/apelora-backups\n' | sudo tee /etc/apelora/db-backup.env >/dev/null
sudo chmod 0640 /etc/apelora/apelora.env /etc/apelora/db-backup.env
sudo chown root:apelora /etc/apelora/apelora.env /etc/apelora/db-backup.env
sudo chown apelora:apelora /srv/apelora-backups

tmp=$(mktemp -d)
trap 'if [ -d "$tmp" ]; then sudo rm -rf "$tmp"; fi' EXIT
cp "$root"/infra/systemd/*.service "$root"/infra/systemd/*.timer "$tmp/"
# systemd-analyze verify looks next to the unit for drop-ins; run it on copies so a
# missing Documentation file is not an error on the runner.
sed -i '/^Documentation=/d' "$tmp"/*.service
systemd-analyze verify "$tmp"/apelora-*.service "$tmp"/apelora-*.timer
echo "systemd units: ok"

sudo cp "$root"/infra/serve/*.conf /etc/apelora/serve/
# Dummy certificates at the paths the example names; nginx -t reads them.
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
  -subj "/CN=studio.example.internal" \
  -addext "subjectAltName=DNS:studio.example.internal" \
  -keyout "$tmp/studio.key" -out "$tmp/studio.crt" 2>/dev/null
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 1 \
  -subj "/CN=console.example.internal" \
  -addext "subjectAltName=DNS:console.example.internal" \
  -keyout "$tmp/console.key" -out "$tmp/console.crt" 2>/dev/null
sudo install -m 0644 "$tmp/studio.crt" /etc/apelora/tls/studio.example.internal.crt
sudo install -m 0600 "$tmp/studio.key" /etc/apelora/tls/studio.example.internal.key
sudo install -m 0644 "$tmp/console.crt" /etc/apelora/tls/console.example.internal.crt
sudo install -m 0600 "$tmp/console.key" /etc/apelora/tls/console.example.internal.key
printf '<!doctype html><title>ok</title>\n' | sudo tee /srv/apelora/studio/index.html >/dev/null
printf '<!doctype html><title>ok</title>\n' | sudo tee /srv/apelora/console/index.html >/dev/null

cat >"$tmp/nginx.conf" <<EOF
error_log stderr;
pid $tmp/nginx.pid;
events { worker_connections 32; }
http {
    include /etc/apelora/serve/apelora.conf;
}
EOF
# Keys are 0600 root:root, as on a host. nginx -t as the runner cannot read them
# (Permission denied on fopen); the master process on a host runs as root, so
# this check does too.
sudo nginx -t -c "$tmp/nginx.conf" -e stderr
echo "nginx example: ok"
