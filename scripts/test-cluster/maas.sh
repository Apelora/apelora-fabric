#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# A throwaway MAAS in a Lima VM, to check Apelora's MAAS adapter against the real API
# (docs/design/provisioner-maas.md, docs/guides/testing-without-hardware.md).
#
#   maas.sh up        create the VM, install and initialise MAAS, enable TLS, write maas.env
#   maas.sh machines  create two test machines (manual power; they are records, not hardware)
#   maas.sh test      run the adapter's ignored real-API test against it
#   maas.sh down      delete the VM and the credentials
#
# Credentials stay outside the repository, in $APELORA_TEST_DIR (default
# ~/.config/apelora/test-cluster, mode 0700): maas-admin-password, maas-ca.pem (the certificate,
# public) and maas.env (APELORA_MAAS_URL, APELORA_MAAS_API_KEY, APELORA_MAAS_CA). Nothing here
# prints the password or the API key. MAAS manages no real hardware here.
set -euo pipefail

VM=${APELORA_MAAS_VM:-apelora-maas}
DIR=${APELORA_TEST_DIR:-$HOME/.config/apelora/test-cluster}
here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../.." && pwd)
vm() { limactl shell --workdir / "$VM" "$@"; }
need() { command -v "$1" >/dev/null || { echo "missing: $1 ($2)" >&2; exit 1; }; }
need limactl "brew install lima"
need openssl "part of macOS and most Linux distributions"
umask 077
mkdir -p "$DIR"

case "${1:-}" in
  up)
    state=$(limactl list --format '{{.Status}}' "$VM" 2>/dev/null || true)
    case "$state" in
      Running) ;;
      Stopped) limactl start "$VM" ;;
      "") limactl start --name="$VM" --tty=false "$here/maas-vm.yaml" ;;
      *) echo "the VM is $state" >&2; exit 1 ;;
    esac
    addr=$(vm ip -4 -o addr show dev lima0 | awk '{print $4}' | cut -d/ -f1 | head -1)
    [ -n "$addr" ] || { echo "the VM has no lima0 address" >&2; exit 1; }
    if ! vm snap list maas >/dev/null 2>&1; then
      vm sudo snap install maas-test-db
      vm sudo snap install maas
      vm sudo maas init region+rack --database-uri maas-test-db:/// --maas-url "http://$addr:5240/MAAS"
    fi
    if [ ! -f "$DIR/maas-admin-password" ]; then
      openssl rand -hex 16 | tr -d '\n' > "$DIR/maas-admin-password"
      vm sudo maas createadmin --username apelora-lab --password "$(cat "$DIR/maas-admin-password")" --email lab@example.invalid >/dev/null
    fi
    # TLS: the adapter requires https. A self-signed certificate for the VM's address; the
    # adapter and curl verify it against that file (APELORA_MAAS_CA).
    if ! vm sudo test -f /var/snap/maas/common/lab-tls/cert.pem; then
      vm sudo bash -s <<TLS
set -e
mkdir -p /var/snap/maas/common/lab-tls && cd /var/snap/maas/common/lab-tls
# A leaf certificate (CA:FALSE): the Rust TLS stack refuses a CA certificate as a server's.
openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:prime256v1 -nodes -days 30 -subj "/CN=$VM" -addext "subjectAltName=IP:$addr,DNS:$VM" -addext "basicConstraints=critical,CA:FALSE" -addext "extendedKeyUsage=serverAuth" -keyout key.pem -out cert.pem 2>/dev/null
chmod 600 key.pem
maas config-tls enable key.pem cert.pem --port 5443 --yes
TLS
    fi
    vm sudo cat /var/snap/maas/common/lab-tls/cert.pem > "$DIR/maas-ca.pem"
    key=$(vm sudo maas apikey --username apelora-lab)
    {
      echo "APELORA_MAAS_URL=https://$addr:5443/MAAS"
      echo "APELORA_MAAS_API_KEY=$key"
      echo "APELORA_MAAS_CA=$DIR/maas-ca.pem"
    } > "$DIR/maas.env"
    chmod 600 "$DIR/maas.env"
    for _ in $(seq 1 30); do
      curl -fsS --cacert "$DIR/maas-ca.pem" -o /dev/null "https://$addr:5443/MAAS/api/2.0/version/" && break
      sleep 2
    done
    echo "MAAS answers on https://$addr:5443/MAAS (version: $(curl -fsS --cacert "$DIR/maas-ca.pem" "https://$addr:5443/MAAS/api/2.0/version/" | jq -r .version 2>/dev/null || echo unknown))"
    echo "Credentials: $DIR/maas.env (source it; never commit it). Boot images import in the background; 'maas.sh machines' needs them."
    ;;
  machines)
    # shellcheck disable=SC1091
    source "$DIR/maas.env"
    addr=$(vm ip -4 -o addr show dev lima0 | awk '{print $4}' | cut -d/ -f1 | head -1)
    vm sudo bash -s <<LOGIN >/dev/null
maas login --cacerts /var/snap/maas/common/lab-tls/cert.pem lab https://$addr:5443/MAAS "$APELORA_MAAS_API_KEY"
LOGIN
    vm sudo maas lab boot-resources import >/dev/null 2>&1 || true
    for _ in $(seq 1 90); do
      [ "$(vm sudo maas lab boot-resources is-importing 2>/dev/null)" = false ] && break
      sleep 10
    done
    for n in 1 2; do
      vm sudo maas lab machines read | jq -e --arg h "apelora-lab-$n" '.[] | select(.hostname == $h)' >/dev/null 2>&1 && continue
      vm sudo maas lab machines create architecture=amd64/generic "mac_addresses=52:54:00:ab:cd:0$n" "hostname=apelora-lab-$n" power_type=manual \
        | jq -r '"created \(.system_id) \(.hostname) \(.status_name)"'
    done
    vm sudo maas lab machines read | jq -r '.[] | "\(.system_id) \(.hostname) \(.status_name) power=\(.power_state) type=\(.power_type)"'
    ;;
  test)
    # shellcheck disable=SC1091
    source "$DIR/maas.env"
    export APELORA_TEST_MAAS_URL=$APELORA_MAAS_URL APELORA_TEST_MAAS_KEY=$APELORA_MAAS_API_KEY APELORA_TEST_MAAS_CA=$APELORA_MAAS_CA
    cd "$repo" && cargo test -p apelora-maas-adapter --test real_maas -- --ignored --nocapture
    ;;
  down)
    limactl delete -f "$VM" 2>/dev/null || true
    rm -f "$DIR/maas-admin-password" "$DIR/maas-ca.pem" "$DIR/maas.env" "$DIR/maas-api-key.tmp"
    echo "deleted $VM and its credentials"
    ;;
  *) echo "usage: $0 up | machines | test | down" >&2; exit 2 ;;
esac
