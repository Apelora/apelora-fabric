#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Incus /1.0/metrics on the disposable test cluster, with a metrics-only certificate
# (docs/architecture/MONITORING.md §4). Dry-run unless APPLY=1. Certificates stay in
# ~/.config/apelora/test-cluster (never the repository). Never uses Apelora's deploy or
# admin identities to scrape, and never turns metrics authentication off.
#
#   bash scripts/test-cluster/metrics.sh          # print the plan
#   APPLY=1 bash scripts/test-cluster/metrics.sh  # trust, listen, scrape, then leave the listener
#
# Each member's server certificate names only 127.0.0.1, so the scrape is from that member
# against loopback. A remote collector needs a certificate that names the management address.
set -euo pipefail

APPLY=${APPLY:-0}
DIR=${APELORA_TEST_DIR:-$HOME/.config/apelora/test-cluster}
PROJECT=${APELORA_TEST_PROJECT:-apelora}
STEM=metrics
NAME=apelora-metrics
PORT=8444
MEMBERS="apelora-test-1 apelora-test-2 apelora-test-3 apelora-test-4"
FIRST=apelora-test-1
crt="$DIR/$STEM.crt"
key="$DIR/$STEM.key"

vm_of() {
  case $1 in
    apelora-test-1) echo apelora-incus ;;
    apelora-test-2) echo apelora-incus-2 ;;
    apelora-test-3) echo apelora-incus-3 ;;
    apelora-test-4) echo apelora-incus-4 ;;
    *) echo "$1" ;;
  esac
}
on() { local vm; vm=$(vm_of "$1"); shift; limactl shell --workdir / "$vm" sudo bash -s; }
incus_() { echo "incus $* </dev/null" | on "$FIRST"; }
fingerprint_of() { openssl x509 -in "$1" -noout -fingerprint -sha256 | cut -d= -f2 | tr -d : | tr 'A-F' 'a-f'; }
# The metrics listener presents the cluster certificate (same on every member), not
# the per-member server.crt. Pin that public key: OpenSSL 3 will not treat the
# self-signed leaf as a CA, so `curl --cacert` fails.
cluster_pin() {
  echo "openssl x509 -in /var/lib/incus/cluster.crt -noout -pubkey </dev/null" | on "$1" |
    openssl pkey -pubin -outform DER | openssl dgst -sha256 -binary | openssl enc -base64 -A
}
tls_name() {
  echo "openssl x509 -in /var/lib/incus/cluster.crt -noout -text </dev/null" | on "$1" |
    grep -o 'DNS:[^,]*' | head -1 | cut -d: -f2
}

echo "Incus metrics plan: type=metrics certificate $NAME; core.metrics_address=127.0.0.1:$PORT on $MEMBERS"
[ "$APPLY" = 1 ] || echo "Dry-run. APPLY=1 to change the lab."

umask 077
mkdir -p "$DIR"
chmod 700 "$DIR"
if [ ! -s "$crt" ] || [ ! -s "$key" ]; then
  echo "issue $crt (secp384r1, clientAuth, 90 days)"
  if [ "$APPLY" = 1 ]; then
    openssl ecparam -name secp384r1 -genkey -noout | openssl pkcs8 -topk8 -nocrypt -out "$key"
    openssl req -new -x509 -key "$key" -subj "/CN=$NAME" -days 90 -out "$crt" \
      -config <(printf '[req]\ndistinguished_name=dn\nx509_extensions=v3\n[dn]\n[v3]\nbasicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature\nextendedKeyUsage=clientAuth\n')
  fi
else
  echo "reuse $crt"
fi

trusted=0
if [ -s "$crt" ]; then
  fp=$(fingerprint_of "$crt")
  if incus_ config trust list --format csv --columns f | grep -q "^${fp:0:12}"; then
    trusted=1
  fi
fi
if [ "$trusted" = 1 ]; then
  echo "already trusted $NAME"
elif [ "$APPLY" = 1 ]; then
  limactl copy "$crt" "$(vm_of "$FIRST"):/tmp/apelora-$STEM.crt"
  incus_ config trust add-certificate "/tmp/apelora-$STEM.crt" --name "$NAME" --type metrics
  echo "rm -f /tmp/apelora-$STEM.crt" | on "$FIRST"
else
  echo "DRY-RUN incus config trust add-certificate … --name $NAME --type metrics"
fi

auth=$(incus_ config get core.metrics_authentication || true)
[ -z "$auth" ] || [ "$auth" = true ] || { echo "core.metrics_authentication is $auth; refuse to scrape" >&2; exit 1; }

for m in $MEMBERS; do
  current=$(echo "incus config get core.metrics_address --target $m </dev/null" | on "$FIRST" || true)
  want="127.0.0.1:$PORT"
  if [ "$current" = "$want" ]; then
    echo "$m already listens on $want"
  else
    echo "$m core.metrics_address ${current:-empty} -> $want"
    if [ "$APPLY" = 1 ]; then
      echo "incus config set core.metrics_address=$want --target $m </dev/null" | on "$FIRST"
    else
      echo "DRY-RUN incus config set core.metrics_address=$want --target $m"
    fi
  fi
done

[ "$APPLY" = 1 ] || exit 0

# Scrape from each member against loopback. TLS: pin the cluster certificate's public
# key (`--insecure` without a pin would skip verification; the pin is required).
tmp=$(mktemp -d -t apelora-metrics)
trap 'rm -rf "$tmp"' EXIT
pass=0
for m in $MEMBERS; do
  vm=$(vm_of "$m")
  name=$(tls_name "$m")
  pin=$(cluster_pin "$m")
  [ -n "$name" ] || { echo "FAIL  $m cluster certificate names no host" >&2; exit 1; }
  [ -n "$pin" ] || { echo "FAIL  $m cluster certificate has no public key" >&2; exit 1; }
  limactl copy "$crt" "$vm:/tmp/apelora-$STEM.crt"
  limactl copy "$key" "$vm:/tmp/apelora-$STEM.key"
  echo "chmod 600 /tmp/apelora-$STEM.key /tmp/apelora-$STEM.crt" | on "$m"
  body=$(echo "curl -sS -m 15 --fail --cert /tmp/apelora-$STEM.crt --key /tmp/apelora-$STEM.key --pinnedpubkey sha256//$pin --insecure --resolve $name:$PORT:127.0.0.1 https://$name:$PORT/1.0/metrics" | on "$m") || {
    echo "FAIL  $m GET /1.0/metrics" >&2
    echo "rm -f /tmp/apelora-$STEM.crt /tmp/apelora-$STEM.key" | on "$m"
    exit 1
  }
  echo "rm -f /tmp/apelora-$STEM.crt /tmp/apelora-$STEM.key" | on "$m"
  echo "$body" | grep -q '^incus_' || { echo "FAIL  $m metrics are not Incus exposition" >&2; exit 1; }
  echo "$body" >"$tmp/$m.txt"
  echo "PASS  $m /1.0/metrics ($(echo "$body" | grep -c '^incus_') families)"
  pass=$((pass + 1))
done

# The metrics listener must not answer instance management, even with this certificate.
limactl copy "$crt" "$(vm_of "$FIRST"):/tmp/apelora-$STEM.crt"
limactl copy "$key" "$(vm_of "$FIRST"):/tmp/apelora-$STEM.key"
name=$(tls_name "$FIRST")
pin=$(cluster_pin "$FIRST")
code=$(echo "curl -sS -m 15 -o /dev/null -w '%{http_code}' --cert /tmp/apelora-$STEM.crt --key /tmp/apelora-$STEM.key --pinnedpubkey sha256//$pin --insecure --resolve $name:$PORT:127.0.0.1 https://$name:$PORT/1.0/instances?project=$PROJECT" | on "$FIRST" || true)
echo "rm -f /tmp/apelora-$STEM.crt /tmp/apelora-$STEM.key" | on "$FIRST"
# 000 = connection refused / empty; 403/404 = listener does not serve instances.
case "$code" in
  403|404) echo "PASS  metrics listener refused /1.0/instances ($code)" ;;
  *) echo "FAIL  metrics listener answered /1.0/instances with $code" >&2; exit 1 ;;
esac

# On the main API the same certificate reads metrics and is refused for instances (type=metrics).
# shellcheck disable=SC1091
source "$DIR/env"
host=${APELORA_INCUS_URL#https://}
tls=${APELORA_INCUS_TLS_SERVER_NAME:-${host%:*}}
code_inst=$(curl -sS -m 15 -o /dev/null -w '%{http_code}' --cert "$crt" --key "$key" \
  --cacert "$APELORA_INCUS_SERVER_CERT" --resolve "${tls}:${host##*:}:${host%:*}" \
  "https://${tls}:${host##*:}/1.0/instances?project=$PROJECT")
code_met=$(curl -sS -m 15 -o /dev/null -w '%{http_code}' --cert "$crt" --key "$key" \
  --cacert "$APELORA_INCUS_SERVER_CERT" --resolve "${tls}:${host##*:}:${host%:*}" \
  "https://${tls}:${host##*:}/1.0/metrics")
[ "$code_met" = 200 ] && echo "PASS  type=metrics reads /1.0/metrics on the API ($code_met)" || {
  echo "FAIL  type=metrics GET /1.0/metrics on the API was $code_met" >&2
  exit 1
}
[ "$code_inst" = 403 ] && echo "PASS  type=metrics cannot list instances on the API ($code_inst)" || {
  echo "FAIL  type=metrics GET /1.0/instances on the API was $code_inst" >&2
  exit 1
}

echo "done: scraped $pass members with a metrics certificate; authentication stayed on"
