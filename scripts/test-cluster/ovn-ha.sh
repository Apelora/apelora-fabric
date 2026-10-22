#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Converts the disposable lab's one OVN database (ovn.sh) into a three-member clustered
# northbound and southbound pair with TLS between OVN components and from Incus to them.
# Dry-run unless APPLY=1. See docs/development/TEST-CLUSTER.md and issue #13.
#
# The first three Lima members run ovn-central (the raft cluster); every member keeps
# ovn-host. Certificates live in ~/.config/apelora/test-cluster/ovn-pki (never the repo).
set -euo pipefail

APPLY=${APPLY:-0}
VM=${APELORA_TEST_VM:-apelora-incus}
DIR=${APELORA_TEST_DIR:-$HOME/.config/apelora/test-cluster}
PKI="$DIR/ovn-pki"
REMOTE=/etc/ovn/apelora

members=$(limactl list --format '{{.Name}}' | grep -E "^${VM}(-[0-9]+)?$" | sort)
[ -n "$members" ] || { echo "no test cluster VMs; run up.sh first" >&2; exit 1; }
all=()
while IFS= read -r m; do
  [ -n "$m" ] && all+=("$m")
done <<<"$members"
[ "${#all[@]}" -ge 3 ] || { echo "OVN HA needs three members; have ${#all[@]}" >&2; exit 1; }
central=("${all[0]}" "${all[1]}" "${all[2]}")

sh_in() { local v=$1; shift; limactl shell --workdir / "$v" "$@"; }
eth0() { sh_in "$1" ip -4 -o addr show dev eth0 | awk '{print $4}' | cut -d/ -f1 | head -1; }

ips=()
for v in "${central[@]}"; do
  ip=$(eth0 "$v")
  [ -n "$ip" ] || { echo "$v has no address on eth0" >&2; exit 1; }
  ips+=("$ip")
done

nb=$(printf 'ssl:%s:6641,' "${ips[@]}" | sed 's/,$//')
sb=$(printf 'ssl:%s:6642,' "${ips[@]}" | sed 's/,$//')

run() {
  if [ "$APPLY" = 1 ]; then
    "$@"
  else
    printf 'DRY-RUN %s\n' "$*"
  fi
}

echo "OVN HA plan: databases on ${central[*]} (${ips[*]}); TLS; Incus northbound $nb"
[ "$APPLY" = 1 ] || echo "Dry-run. APPLY=1 to change the lab."

# --- certificates (v3, client+server) stay on the workstation and are copied in -------------
issue() {
  local name=$1 cn=$2 san=$3
  local key="$PKI/$name.key" crt="$PKI/$name.crt"
  [ -s "$key" ] && [ -s "$crt" ] && return 0
  openssl ecparam -name secp384r1 -genkey -noout | openssl pkcs8 -topk8 -nocrypt -out "$key"
  openssl req -new -key "$key" -subj "/CN=$cn" -out "$PKI/$name.csr"
  openssl x509 -req -in "$PKI/$name.csr" -CA "$PKI/ca.crt" -CAkey "$PKI/ca.key" -CAcreateserial \
    -days 90 -out "$crt" -extfile <(printf 'basicConstraints=critical,CA:FALSE\nkeyUsage=critical,digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth,clientAuth\nsubjectAltName=%s\n' "$san")
  rm -f "$PKI/$name.csr"
}

if [ "$APPLY" = 1 ]; then
  mkdir -p "$PKI"
  chmod 700 "$DIR" "$PKI"
  if [ ! -s "$PKI/ca.key" ]; then
    openssl ecparam -name secp384r1 -genkey -noout | openssl pkcs8 -topk8 -nocrypt -out "$PKI/ca.key"
    openssl req -new -x509 -key "$PKI/ca.key" -subj "/CN=Apelora lab OVN CA" -days 90 -out "$PKI/ca.crt" \
      -config <(printf '[req]\ndistinguished_name=dn\nx509_extensions=v3\n[dn]\n[v3]\nbasicConstraints=critical,CA:TRUE\nkeyUsage=critical,keyCertSign,cRLSign\n')
  fi
  i=0
  for v in "${central[@]}"; do
    issue "$v" "$v" "IP:${ips[$i]},DNS:$v"
    i=$((i + 1))
  done
  for v in "${all[@]}"; do
    # Hosts that are not database members still run ovn-controller and need a client cert.
    grep -qx "$v" <<<"$(printf '%s\n' "${central[@]}")" && continue
    ip=$(eth0 "$v")
    issue "$v" "$v" "IP:$ip,DNS:$v"
  done
  issue incus incus-ovn "DNS:incus"
else
  echo "DRY-RUN would issue a CA and per-member certificates in $PKI"
fi

install_certs() {
  local v=$1
  run sh_in "$v" sudo mkdir -p "$REMOTE"
  if [ "$APPLY" = 1 ]; then
    limactl copy "$PKI/ca.crt" "$v:/tmp/ovn-ca.crt"
    limactl copy "$PKI/$v.crt" "$v:/tmp/ovn-node.crt"
    limactl copy "$PKI/$v.key" "$v:/tmp/ovn-node.key"
    sh_in "$v" sudo install -m 0644 /tmp/ovn-ca.crt "$REMOTE/ca.crt"
    sh_in "$v" sudo install -m 0644 /tmp/ovn-node.crt "$REMOTE/cert.crt"
    sh_in "$v" sudo install -m 0600 /tmp/ovn-node.key "$REMOTE/key.key"
    sh_in "$v" sudo rm -f /tmp/ovn-ca.crt /tmp/ovn-node.crt /tmp/ovn-node.key
  else
    echo "DRY-RUN copy certs onto $v:$REMOTE"
  fi
}

ssl_opts() {
  cat <<EOF
     --ovn-nb-db-ssl-key=$REMOTE/key.key \\
     --ovn-nb-db-ssl-cert=$REMOTE/cert.crt \\
     --ovn-nb-db-ssl-ca-cert=$REMOTE/ca.crt \\
     --ovn-sb-db-ssl-key=$REMOTE/key.key \\
     --ovn-sb-db-ssl-cert=$REMOTE/cert.crt \\
     --ovn-sb-db-ssl-ca-cert=$REMOTE/ca.crt \\
     --ovn-northd-ssl-key=$REMOTE/key.key \\
     --ovn-northd-ssl-cert=$REMOTE/cert.crt \\
     --ovn-northd-ssl-ca-cert=$REMOTE/ca.crt
EOF
}

write_central() {
  local v=$1 local_ip=$2 remote_ip=${3:-}
  local extra=""
  if [ -n "$remote_ip" ]; then
    extra="--db-nb-cluster-remote-addr=$remote_ip --db-nb-cluster-remote-proto=ssl --db-sb-cluster-remote-addr=$remote_ip --db-sb-cluster-remote-proto=ssl"
  fi
  local body
  body=$(cat <<EOF
OVN_CTL_OPTS=" \\
     --db-nb-addr=$local_ip \\
     --db-nb-create-insecure-remote=no \\
     --db-sb-addr=$local_ip \\
     --db-sb-create-insecure-remote=no \\
     --db-nb-cluster-local-addr=$local_ip \\
     --db-sb-cluster-local-addr=$local_ip \\
     --db-nb-cluster-local-proto=ssl --db-sb-cluster-local-proto=ssl \\
     $extra \\
     --ovn-northd-nb-db=$nb \\
     --ovn-northd-sb-db=$sb \\
$(ssl_opts)"
EOF
)
  if [ "$APPLY" = 1 ]; then
    sh_in "$v" sudo tee /etc/default/ovn-central >/dev/null <<<"$body"
  else
    echo "DRY-RUN write /etc/default/ovn-central on $v"
    echo "$body" | sed 's/^/  /'
  fi
}

# ovn-host on every member: controller talks to the clustered southbound over TLS.
write_host() {
  local v=$1
  local body
  body=$(cat <<EOF
OVN_CTL_OPTS=" \\
     --ovn-controller-ssl-key=$REMOTE/key.key \\
     --ovn-controller-ssl-cert=$REMOTE/cert.crt \\
     --ovn-controller-ssl-ca-cert=$REMOTE/ca.crt"
EOF
)
  if [ "$APPLY" = 1 ]; then
    sh_in "$v" sudo tee /etc/default/ovn-host >/dev/null <<<"$body"
  else
    echo "DRY-RUN write /etc/default/ovn-host on $v"
  fi
}

for v in "${all[@]}"; do
  install_certs "$v"
  pkgs=ovn-host
  grep -qx "$v" <<<"$(printf '%s\n' "${central[@]}")" && pkgs="ovn-central ovn-host"
  run sh_in "$v" sudo env DEBIAN_FRONTEND=noninteractive apt-get install -yq $pkgs
  write_host "$v"
done

# Stop the single-node database before rewriting it as a cluster member.
run sh_in "${central[0]}" sudo systemctl stop ovn-central
write_central "${central[0]}" "${ips[0]}"
run sh_in "${central[0]}" sudo systemctl enable --now ovn-central
write_central "${central[1]}" "${ips[1]}" "${ips[0]}"
run sh_in "${central[1]}" sudo systemctl enable --now ovn-central
write_central "${central[2]}" "${ips[2]}" "${ips[0]}"
run sh_in "${central[2]}" sudo systemctl enable --now ovn-central

for v in "${all[@]}"; do
  ip=$(eth0 "$v")
  run sh_in "$v" sudo ovs-vsctl set open_vswitch . \
    "external_ids:ovn-remote=$sb" \
    external_ids:ovn-encap-type=geneve \
    "external_ids:ovn-encap-ip=$ip"
  run sh_in "$v" sudo systemctl restart ovn-host
done

# Incus uses the same client certificate on every member (cluster config). PEM
# files are copied onto the first member and read there: they cannot go on the
# limactl command line (newlines).
if [ "$APPLY" = 1 ]; then
  limactl copy "$PKI/ca.crt" "${central[0]}:/tmp/ovn-incus-ca.crt"
  limactl copy "$PKI/incus.crt" "${central[0]}:/tmp/ovn-incus.crt"
  limactl copy "$PKI/incus.key" "${central[0]}:/tmp/ovn-incus.key"
  printf '%s\n' \
    'incus config set network.ovn.northbound_connection="$1" \' \
    '  network.ovn.ca_cert="$(cat /tmp/ovn-incus-ca.crt)" \' \
    '  network.ovn.client_cert="$(cat /tmp/ovn-incus.crt)" \' \
    '  network.ovn.client_key="$(cat /tmp/ovn-incus.key)"' \
    'rm -f /tmp/ovn-incus-ca.crt /tmp/ovn-incus.crt /tmp/ovn-incus.key' \
    >"$PKI/set-incus-ovn.sh"
  limactl copy "$PKI/set-incus-ovn.sh" "${central[0]}:/tmp/ovn-set-incus.sh"
  sh_in "${central[0]}" sudo bash /tmp/ovn-set-incus.sh "$nb"
  sh_in "${central[0]}" sudo rm -f /tmp/ovn-set-incus.sh
else
  echo "DRY-RUN copy Incus OVN client certs onto ${central[0]} and set network.ovn.northbound_connection=$nb"
fi

echo "Next: ovn-appctl -t /var/run/ovn/ovnnb_db.ctl cluster/status OVN_Northbound"
echo "Failover: stop ovn-central on the leader and check ovn-nbctl still answers from another member."
[ "$APPLY" = 1 ] || exit 0

echo "Cluster status (northbound):"
sh_in "${central[0]}" sudo ovn-appctl -t /var/run/ovn/ovnnb_db.ctl cluster/status OVN_Northbound || true
sh_in "${central[0]}" sudo incus config get network.ovn.northbound_connection
echo "OVN HA with TLS applied."
