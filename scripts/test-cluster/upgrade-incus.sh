#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Upgrades the Incus packages on the disposable lab's members from the package channel in
# INCUS_CHANNEL (stable = the newest feature release; lts-7.0 = the 7.0 long-term series), one
# member at a time in the order Incus documents: stand-by, voters, the database leader last.
# Dry-run unless APPLY=1: the dry run reads the channel's package index and prints, per
# member, what would be installed; it changes nothing. See docs/development/TEST-CLUSTER.md and
# docs/development/UPSTREAM-INCUS.md.
#
# Incus upgrades its cluster database schema on the first member to run the new release and
# blocks the others until they match, so a member is not upgraded until the one before it is
# Online again. The schema change is one way: the lab cannot go back to an older release
# (rebuild it with down.sh and up.sh).
set -euo pipefail

APPLY=${APPLY:-0}
CHANNEL=${INCUS_CHANNEL:-stable}
VM=${APELORA_TEST_VM:-apelora-incus}
WAIT=${APELORA_UPGRADE_WAIT_SECS:-300}
[[ $CHANNEL =~ ^(stable|lts-[0-9]+\.[0-9]+)$ ]] || { echo "INCUS_CHANNEL is stable or lts-<series>" >&2; exit 1; }

sh_in() { local v=$1; shift; limactl shell --workdir / "$v" "$@" </dev/null; }
vms=$(limactl list --format '{{.Name}}' | grep -E "^${VM}(-[0-9]+)?$" | sort)
[ -n "$vms" ] || { echo "no test cluster VMs; run up.sh first" >&2; exit 1; }
first=$(head -1 <<<"$vms")

# Member name, address and roles, from the cluster itself.
rows=$(sh_in "$first" sudo incus cluster list -f json)
[ -n "$rows" ] || { echo "cannot read the cluster from $first" >&2; exit 1; }
if [ "$(jq '[.[] | select(.status != "Online")] | length' <<<"$rows")" != 0 ]; then
  echo "every member must be Online before an upgrade:" >&2
  jq -r '.[] | "\(.server_name) \(.status)"' <<<"$rows" >&2; exit 1
fi

# The VM that holds each member's address.
vm_of() {
  local ip=$1 v
  for v in $vms; do
    if sh_in "$v" ip -4 -o addr show dev eth0 | grep -q " ${ip}/"; then echo "$v"; return 0; fi
  done
  return 1
}

# stand-by first, then plain voters, the database leader last.
order=$(jq -r '.[] | [(if (.roles | index("database-standby")) then 0
                       elif (.roles | index("database-leader")) then 2 else 1 end),
                      .server_name, (.url | sub("^https://"; "") | sub(":[0-9]+$"; ""))] | join(" ")' <<<"$rows" | sort -n)

arch=$(sh_in "$first" dpkg --print-architecture)
suite=$(sh_in "$first" bash -c '. /etc/os-release && echo "$VERSION_CODENAME"')
newest=$(curl -fsS "https://pkgs.zabbly.com/incus/$CHANNEL/dists/$suite/main/binary-$arch/Packages.gz" | gunzip |
  awk '/^Package: incus$/ {p=1} p && /^Version:/ {print $2} /^$/ {p=0}' | sort -V | tail -1)
[ -n "$newest" ] || { echo "the $CHANNEL channel lists no incus package for $suite/$arch" >&2; exit 1; }
want=${newest#*:}; want=${want%%-*}   # 1:7.5.1-debian13-… → 7.5.1

echo "Upgrade plan: channel $CHANNEL, package $newest (Incus $want)"
while read -r rank name ip; do
  v=$(vm_of "$ip") || { echo "no VM has $ip" >&2; exit 1; }
  have=$(sh_in "$v" sudo incus version | awk '/Server version/ {print $3}')
  echo "  $name ($v, $ip): $have -> $want"
done <<<"$order"
[ "$APPLY" = 1 ] || { echo "Dry-run: nothing changed. APPLY=1 to upgrade the lab."; exit 0; }

wait_online() {  # <member> <vm> <version>
  local member=$1 vm=$2 version=$3 end=$((SECONDS + WAIT)) status ran
  while [ "$SECONDS" -lt "$end" ]; do
    status=$(sh_in "$vm" sudo incus cluster list -f json 2>/dev/null | jq -r --arg m "$member" '.[] | select(.server_name == $m) | .status') || true
    ran=$(sh_in "$vm" sudo incus version 2>/dev/null | awk '/Server version/ {print $3}') || true
    if [ "$status" = Online ] && [ "$ran" = "$version" ]; then return 0; fi
    sleep 5
  done
  echo "$member did not become Online on $version within ${WAIT}s" >&2
  return 1
}

while read -r rank name ip; do
  v=$(vm_of "$ip")
  echo "== $name ($v)"
  sh_in "$v" sudo bash -c "set -e
    rm -f /etc/apt/sources.list.d/zabbly-incus-*.sources
    cat >/etc/apt/sources.list.d/zabbly-incus-$CHANNEL.sources <<EOF
Enabled: yes
Types: deb
URIs: https://pkgs.zabbly.com/incus/$CHANNEL
Suites: $suite
Components: main
Architectures: $arch
Signed-By: /etc/apt/keyrings/zabbly.asc
EOF
    DEBIAN_FRONTEND=noninteractive apt-get update -q
    DEBIAN_FRONTEND=noninteractive apt-get install -yq incus incus-base incus-client"
  # The first member to restart blocks the others until they match, so only this member's own
  # answer and the cluster's agreement at the end are asserted.
  sh_in "$v" sudo incus info >/dev/null
  echo "$name now runs $(sh_in "$v" sudo incus version | awk '/Server version/ {print $3}')"
done <<<"$order"

echo "== waiting for every member to be Online on $want"
while read -r rank name ip; do wait_online "$name" "$(vm_of "$ip")" "$want"; done <<<"$order"
sh_in "$first" sudo incus cluster list
