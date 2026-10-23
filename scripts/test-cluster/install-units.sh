#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Run the install page's systemd units and NGINX example on one lab member (issue #17).
# Uses the draft release artefacts (not a published release) and a Lima VM standing in for a
# server. Dry-run unless APPLY=1. Does not close #17: that needs a published release on a
# production-like host. Certificates stay under /etc/apelora on the VM (ProtectHome=true).
#
#   bash scripts/test-cluster/install-units.sh          # print the plan
#   APPLY=1 bash scripts/test-cluster/install-units.sh  # install, start, check, leave running
#   APPLY=1 STOP=1 bash scripts/test-cluster/install-units.sh  # stop and disable the units
set -euo pipefail

APPLY=${APPLY:-0}
STOP=${STOP:-0}
TAG=${APELORA_INSTALL_TAG:-v0.1.0-alpha.1}
ARCH=${APELORA_INSTALL_ARCH:-aarch64}
VM=${APELORA_INSTALL_VM:-apelora-incus}
DIR=${APELORA_TEST_DIR:-$HOME/.config/apelora/test-cluster}
CACHE="$DIR/release-$TAG"
PROJECT=${APELORA_TEST_PROJECT:-apelora}

on() { limactl shell --workdir / "$VM" sudo bash -s; }
run() {
  if [ "$APPLY" = 1 ]; then
    "$@"
  else
    printf 'DRY-RUN %s\n' "$*"
  fi
}

echo "Install units plan: $TAG linux-$ARCH on $VM; systemd User=apelora; NGINX example"
[ "$APPLY" = 1 ] || echo "Dry-run. APPLY=1 to change $VM."

if [ "$STOP" = 1 ]; then
  echo "stop units on $VM"
  if [ "$APPLY" = 1 ]; then
    echo 'systemctl disable --now apelora-control-plane apelora-deployment-worker nginx 2>/dev/null || true
systemctl reset-failed apelora-control-plane apelora-deployment-worker 2>/dev/null || true' | on
  fi
  exit 0
fi

need() { command -v "$1" >/dev/null || { echo "missing: $1" >&2; exit 1; }; }
need gh
need limactl
vm_state=$(limactl list --format '{{.Name}} {{.Status}}' | awk -v n="$VM" '$1 == n { print $2; exit }')
[ "$vm_state" = Running ] || {
  echo "$VM is ${vm_state:-missing}" >&2
  exit 1
}

echo "release artefacts $TAG ($ARCH) → $CACHE"
if [ "$APPLY" = 1 ]; then
  umask 077
  mkdir -p "$CACHE"
  if [ ! -s "$CACHE/SHA256SUMS-services-$ARCH" ]; then
    gh release download "$TAG" --repo Apelora/apelora-fabric --dir "$CACHE" \
      --pattern "*-linux-$ARCH.tar.gz" \
      --pattern "SHA256SUMS-services-$ARCH" \
      --pattern "apelora-studio-*.tar.gz" \
      --pattern "apelora-fabric-console-*.tar.gz" \
      --pattern "SHA256SUMS-apps"
  fi
  (cd "$CACHE" && sha256sum -c "SHA256SUMS-services-$ARCH" SHA256SUMS-apps)
else
  echo "DRY-RUN gh release download $TAG (linux-$ARCH + apps) && sha256sum --check"
fi

echo "on $VM: apelora account, PostgreSQL, binaries, units, NGINX, /etc/apelora"
if [ "$APPLY" != 1 ]; then
  echo "DRY-RUN useradd apelora; apt-get install postgresql nginx"
  echo "DRY-RUN install binaries under /usr/local/bin and screens under /srv/apelora"
  echo "DRY-RUN copy Incus client certificates into /etc/apelora (not a home directory)"
  echo "DRY-RUN systemctl enable --now apelora-control-plane apelora-deployment-worker nginx"
  echo "DRY-RUN GET /api/v1/health and https://studio.example.internal/"
  exit 0
fi

repo=$(cd "$(dirname "$0")/../.." && pwd)
# Copy artefacts and example files onto the VM.
limactl copy "$CACHE/apelora-control-plane-$TAG-linux-$ARCH.tar.gz" "$VM:/tmp/cp.tgz"
limactl copy "$CACHE/apelora-deployment-worker-$TAG-linux-$ARCH.tar.gz" "$VM:/tmp/worker.tgz"
limactl copy "$CACHE/apelora-studio-$TAG.tar.gz" "$VM:/tmp/studio.tgz"
limactl copy "$CACHE/apelora-fabric-console-$TAG.tar.gz" "$VM:/tmp/console.tgz"
limactl copy "$repo/infra/systemd/apelora-control-plane.service" "$VM:/tmp/apelora-control-plane.service"
limactl copy "$repo/infra/systemd/apelora-deployment-worker.service" "$VM:/tmp/apelora-deployment-worker.service"
limactl copy "$repo/infra/serve/apelora.conf" "$VM:/tmp/apelora.conf"
limactl copy "$repo/infra/serve/api-proxy.conf" "$VM:/tmp/api-proxy.conf"
limactl copy "$repo/infra/serve/web-security-headers.conf" "$VM:/tmp/web-security-headers.conf"
limactl copy "$repo/docs/guides/installing-apelora.md" "$VM:/tmp/installing-apelora.md"
limactl copy "$DIR/client.crt" "$VM:/tmp/deploy.crt"
limactl copy "$DIR/client.key" "$VM:/tmp/deploy.key"
limactl copy "$DIR/admin.crt" "$VM:/tmp/admin.crt"
limactl copy "$DIR/admin.key" "$VM:/tmp/admin.key"
limactl copy "$DIR/server.crt" "$VM:/tmp/cluster.crt"

pw=$(openssl rand -base64 24 | tr -d '/+=' | head -c 32)
# The password lives only in /etc/apelora/apelora.env on the VM (0600, root:apelora).
echo "
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
if ! getent passwd apelora >/dev/null; then
  useradd --system --no-create-home --shell /usr/sbin/nologin apelora
fi
apt-get install -yq postgresql nginx >/dev/null
install -d -m 0755 /usr/local/bin /usr/local/share/doc/apelora /etc/apelora /etc/apelora/serve /etc/apelora/tls \\
  /srv/apelora/studio /srv/apelora/console
tar -xzf /tmp/cp.tgz -C /tmp
tar -xzf /tmp/worker.tgz -C /tmp
install -m 0755 /tmp/apelora-control-plane-$TAG-linux-$ARCH/apelora-control-plane /usr/local/bin/apelora-control-plane
install -m 0755 /tmp/apelora-deployment-worker-$TAG-linux-$ARCH/apelora-deployment-worker /usr/local/bin/apelora-deployment-worker
if [ -f /tmp/apelora-deployment-worker-$TAG-linux-$ARCH/apelora-dns ]; then
  install -m 0755 /tmp/apelora-deployment-worker-$TAG-linux-$ARCH/apelora-dns /usr/local/bin/apelora-dns
fi
rm -rf /tmp/apelora-control-plane-$TAG-linux-$ARCH /tmp/apelora-deployment-worker-$TAG-linux-$ARCH /tmp/cp.tgz /tmp/worker.tgz
tar -xzf /tmp/studio.tgz -C /srv/apelora/studio
tar -xzf /tmp/console.tgz -C /srv/apelora/console
rm -f /tmp/studio.tgz /tmp/console.tgz
install -m 0644 /tmp/installing-apelora.md /usr/local/share/doc/apelora/installing-apelora.md
install -m 0644 /tmp/apelora.conf /tmp/api-proxy.conf /tmp/web-security-headers.conf /etc/apelora/serve/
install -m 0644 /tmp/apelora-control-plane.service /tmp/apelora-deployment-worker.service /etc/systemd/system/
install -m 0644 /tmp/deploy.crt /tmp/admin.crt /tmp/cluster.crt /etc/apelora/
install -m 0600 /tmp/deploy.key /tmp/admin.key /etc/apelora/
chown root:apelora /etc/apelora/deploy.crt /etc/apelora/deploy.key /etc/apelora/admin.crt /etc/apelora/admin.key /etc/apelora/cluster.crt
chmod 0640 /etc/apelora/deploy.crt /etc/apelora/admin.crt /etc/apelora/cluster.crt
chmod 0640 /etc/apelora/deploy.key /etc/apelora/admin.key
rm -f /tmp/deploy.crt /tmp/deploy.key /tmp/admin.crt /tmp/admin.key /tmp/cluster.crt /tmp/*.service /tmp/*.conf /tmp/installing-apelora.md
if ! sudo -u postgres psql -tAc \"SELECT 1 FROM pg_roles WHERE rolname='apelora'\" | grep -q 1; then
  sudo -u postgres psql -c \"CREATE ROLE apelora LOGIN PASSWORD '$pw'\"
fi
sudo -u postgres psql -c \"ALTER ROLE apelora PASSWORD '$pw'\"
sudo -u postgres psql -tc \"SELECT 1 FROM pg_database WHERE datname='apelora'\" | grep -q 1 || sudo -u postgres createdb --owner apelora apelora
umask 077
cat >/etc/apelora/apelora.env <<ENV
DATABASE_URL=postgres://apelora:$pw@127.0.0.1:5432/apelora
APELORA_INCUS_URL=https://127.0.0.1:8443
APELORA_INCUS_CLIENT_CERT=/etc/apelora/deploy.crt
APELORA_INCUS_CLIENT_KEY=/etc/apelora/deploy.key
APELORA_INCUS_SERVER_CERT=/etc/apelora/cluster.crt
APELORA_INCUS_TLS_SERVER_NAME=lima-apelora-incus
APELORA_DEPLOY_PROJECTS=$PROJECT
APELORA_INCUS_ADMIN_CLIENT_CERT=/etc/apelora/admin.crt
APELORA_INCUS_ADMIN_CLIENT_KEY=/etc/apelora/admin.key
APELORA_API_ADDR=127.0.0.1:8080
APELORA_NETWORK_UPLINK=incusbr0
APELORA_SERVICE_ADDRESSES=198.18.100.0/28
APELORA_LOG_FORMAT=json
APELORA_METRICS_ADDR=127.0.0.1:9470
ENV
chown root:apelora /etc/apelora/apelora.env
chmod 0640 /etc/apelora/apelora.env
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 30 \\
  -subj '/CN=studio.example.internal' -addext 'subjectAltName=DNS:studio.example.internal' \\
  -keyout /etc/apelora/tls/studio.example.internal.key -out /etc/apelora/tls/studio.example.internal.crt 2>/dev/null
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 30 \\
  -subj '/CN=console.example.internal' -addext 'subjectAltName=DNS:console.example.internal' \\
  -keyout /etc/apelora/tls/console.example.internal.key -out /etc/apelora/tls/console.example.internal.crt 2>/dev/null
chmod 0644 /etc/apelora/tls/*.crt
chmod 0600 /etc/apelora/tls/*.key
grep -q studio.example.internal /etc/hosts || printf '127.0.0.1 studio.example.internal console.example.internal\\n' >> /etc/hosts
ln -sfn /etc/apelora/serve/apelora.conf /etc/nginx/sites-enabled/apelora.conf
rm -f /etc/nginx/sites-enabled/default
nginx -t
systemctl daemon-reload
systemctl enable --now postgresql
systemctl enable --now apelora-control-plane apelora-deployment-worker nginx
# apt may have started nginx on :80 before apelora.conf was linked; reload picks up :443.
systemctl reload nginx
" | on

echo 'sleep 3
systemctl is-active apelora-control-plane apelora-deployment-worker nginx
curl -sS -m 10 http://127.0.0.1:8080/api/v1/health
echo
curl -sS -m 10 -o /dev/null -w "studio %{http_code}\\n" -k --resolve studio.example.internal:443:127.0.0.1 https://studio.example.internal/
curl -sS -m 10 -o /dev/null -w "console %{http_code}\\n" -k --resolve console.example.internal:443:127.0.0.1 https://console.example.internal/
' | on
echo "done: units started on $VM from $TAG (draft). Issue #17 stays open until a published release on a production host."
