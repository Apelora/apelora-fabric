#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Creates (or restarts) the disposable Incus test cluster in a local Lima VM and writes the
# client credentials Apelora needs. Safe to run again. See docs/development/TEST-CLUSTER.md.
#
# Credentials stay outside the repository, in $APELORA_TEST_DIR (default
# ~/.config/apelora/test-cluster, mode 0700). The client certificate is restricted to the
# deploy project, so it cannot touch anything else on the test cluster.
set -euo pipefail

VM=${APELORA_TEST_VM:-apelora-incus}
DIR=${APELORA_TEST_DIR:-$HOME/.config/apelora/test-cluster}
PROJECT=${APELORA_TEST_PROJECT:-apelora}
MEMBER=apelora-test-1
here=$(cd "$(dirname "$0")" && pwd)

need() { command -v "$1" >/dev/null || { echo "missing: $1 ($2)" >&2; exit 1; }; }
need limactl "brew install lima"
need openssl "part of macOS and most Linux distributions"
vm() { limactl shell --workdir / "$VM" "$@"; }
incus() { vm sudo incus "$@"; }

state=$(limactl list --format '{{.Status}}' "$VM" 2>/dev/null || true)
case "$state" in
  "") limactl start --tty=false --name "$VM" "$here/incus-vm.yaml" ;;
  Running) ;;
  *) limactl start --tty=false "$VM" ;;
esac

addr=$(vm ip -4 -o addr show dev lima0 | awk '{print $4}' | cut -d/ -f1 | head -1)
[ -n "$addr" ] || { echo "the VM has no address on lima0" >&2; exit 1; }

if ! incus cluster list >/dev/null 2>&1; then
  echo "initialising Incus as a one-member cluster at $addr"
  vm sudo incus admin init --preseed <<EOF
config:
  core.https_address: "$addr:8443"
cluster:
  server_name: $MEMBER
  enabled: true
storage_pools:
  - name: local
    driver: btrfs
    config:
      size: 20GiB
networks:
  - name: incusbr0
    type: bridge
    config:
      ipv4.address: auto
      ipv6.address: none
profiles:
  - name: default
    devices:
      root: {type: disk, path: /, pool: local}
      eth0: {type: nic, name: eth0, network: incusbr0}
EOF
fi

if ! incus project show "$PROJECT" >/dev/null 2>&1; then
  # Images are shared with the default project; profiles are the project's own.
  incus project create "$PROJECT" -c restricted=true -c features.images=false -c features.profiles=true
fi
# Each device separately, so an interrupted earlier run is completed.
devices=$(incus profile device list default --project "$PROJECT")
grep -qx root <<<"$devices" || incus profile device add default root disk path=/ pool=local --project "$PROJECT"
grep -qx eth0 <<<"$devices" || incus profile device add default eth0 nic name=eth0 network=incusbr0 --project "$PROJECT"

umask 077
mkdir -p "$DIR"
chmod 700 "$DIR"
fingerprint_of() { openssl x509 -in "$1" -noout -fingerprint -sha256 | cut -d= -f2 | tr -d : | tr 'A-F' 'a-f'; }
# An X.509 v3 client-authentication certificate (without extensions, some openssl versions
# issue v1 certificates, which TLS libraries reject). An unusable pair (v1, or the key gone)
# is replaced and the old certificate is no longer trusted.
if [ -s "$DIR/client.crt" ] && { [ ! -s "$DIR/client.key" ] ||
  ! openssl x509 -in "$DIR/client.crt" -noout -text | grep -q 'Version: 3'; }; then
  incus config trust remove "$(fingerprint_of "$DIR/client.crt")" 2>/dev/null || true
  rm -f "$DIR/client.crt" "$DIR/client.key"
fi
if [ ! -s "$DIR/client.crt" ] || [ ! -s "$DIR/client.key" ]; then
  openssl ecparam -name secp384r1 -genkey -noout | openssl pkcs8 -topk8 -nocrypt -out "$DIR/client.key"
  openssl req -new -x509 -key "$DIR/client.key" -subj "/CN=apelora-test" -days 90 -out "$DIR/client.crt" \
    -config <(printf '[req]\ndistinguished_name=dn\nx509_extensions=v3\n[dn]\n[v3]\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=clientAuth\n')
fi
fingerprint=$(fingerprint_of "$DIR/client.crt")
if ! incus config trust list --format csv --columns f | grep -q "^${fingerprint:0:12}"; then
  limactl copy "$DIR/client.crt" "$VM:/tmp/apelora-client.crt"
  incus config trust add-certificate /tmp/apelora-client.crt --name apelora-test --restricted --projects "$PROJECT"
  vm rm -f /tmp/apelora-client.crt
fi
vm sudo cat /var/lib/incus/cluster.crt >"$DIR/server.crt"
# The certificate names the member's host name, not the address we connect to; the adapter
# verifies the pinned certificate against this name.
tls_name=$(openssl x509 -in "$DIR/server.crt" -noout -text | grep -o 'DNS:[^,]*' | head -1 | cut -d: -f2)
[ -n "$tls_name" ] || { echo "the server certificate names no host" >&2; exit 1; }

{
  echo "# Disposable Incus test cluster ($VM). Source this file; never commit it."
  printf 'export APELORA_INCUS_URL=%q\n' "https://$addr:8443"
  printf 'export APELORA_INCUS_CLIENT_CERT=%q\n' "$DIR/client.crt"
  printf 'export APELORA_INCUS_CLIENT_KEY=%q\n' "$DIR/client.key"
  printf 'export APELORA_INCUS_SERVER_CERT=%q\n' "$DIR/server.crt"
  printf 'export APELORA_INCUS_TLS_SERVER_NAME=%q\n' "$tls_name"
  printf 'export APELORA_DEPLOY_PROJECTS=%q\n' "$PROJECT"
} >"$DIR/env"

echo "Incus $(incus version | awk '/Server version/ {print $3}') on $(vm uname -r) at https://$addr:8443"
echo "credentials: $DIR (source $DIR/env)"
