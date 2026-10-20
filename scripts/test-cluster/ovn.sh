#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Optional: adds OVN to the disposable Incus test cluster so Apelora can enforce network
# intents (per-application OVN networks, ACLs between appliances, DNS by name). Run after
# up.sh and add-member.sh. Safe to repeat. See docs/development/TEST-CLUSTER.md.
#
# Test-only shortcuts, documented: one OVN database (on the first member, so OVN itself is
# not highly available here) reachable without TLS on the members' private network.
set -euo pipefail

VM=${APELORA_TEST_VM:-apelora-incus}
PROJECT=${APELORA_TEST_PROJECT:-apelora}

members=$(limactl list --format '{{.Name}}' | grep -E "^${VM}(-[0-9]+)?$" | sort)
[ -n "$members" ] || { echo "no test cluster VMs; run up.sh first" >&2; exit 1; }
first=$(echo "$members" | head -1)
sh_in() { local v=$1; shift; limactl shell --workdir / "$v" "$@"; }
eth0() { sh_in "$1" ip -4 -o addr show dev eth0 | awk '{print $4}' | cut -d/ -f1 | head -1; }
incus() { sh_in "$first" sudo incus "$@"; }
central=$(eth0 "$first")

for v in $members; do
  pkgs=ovn-host
  [ "$v" = "$first" ] && pkgs="ovn-central ovn-host"
  # shellcheck disable=SC2086 # word splitting intended for the package list
  sh_in "$v" sudo env DEBIAN_FRONTEND=noninteractive apt-get install -yq $pkgs >/dev/null
done
sh_in "$first" sudo ovn-nbctl set-connection "ptcp:6641:$central"
sh_in "$first" sudo ovn-sbctl set-connection "ptcp:6642:$central"
for v in $members; do
  sh_in "$v" sudo ovs-vsctl set open_vswitch . \
    "external_ids:ovn-remote=tcp:$central:6642" \
    external_ids:ovn-encap-type=geneve \
    "external_ids:ovn-encap-ip=$(eth0 "$v")"
done
incus config set "network.ovn.northbound_connection=tcp:$central:6641"

# incusbr0 is the uplink: part of its subnet for DHCP, part for OVN routers.
subnet=$(incus network get incusbr0 ipv4.address)   # e.g. 10.213.140.1/24
prefix=${subnet%.*}
incus network set incusbr0 "ipv4.dhcp.ranges=$prefix.2-$prefix.99" "ipv4.ovn.ranges=$prefix.100-$prefix.199"
# Service addresses (OVN load balancers) come from a routed range of the uplink. 198.18.0.0/15
# is reserved for network testing, so it never collides with a real network on the lab.
SERVICES=${APELORA_TEST_SERVICES:-198.18.100.0/28}
incus network set incusbr0 "ipv4.routes=$SERVICES"

# The deploy project owns its OVN networks and ACLs (features.networks), so the restricted
# deploy identity can manage them; it may only use incusbr0 as uplink. Features can only
# change on an empty project, so an empty project without them is recreated.
if [ "$(incus project get "$PROJECT" features.networks 2>/dev/null)" != "true" ]; then
  if [ -n "$(incus list --project "$PROJECT" --format csv -c n)" ]; then
    echo "project $PROJECT has instances; delete them before enabling OVN networks" >&2
    exit 1
  fi
  # Deleting a project also removes it from every restricted identity's project list (the
  # running server may keep authorising by name until its identity cache is rebuilt, e.g. at a
  # member outage). Remember who had it and grant it again below.
  grants=$(incus query "/1.0/certificates?recursion=1" |
    jq -r --arg p "$PROJECT" '.[] | select(.restricted and ((.projects // []) | index($p))) | .fingerprint')
  incus profile device remove default eth0 --project "$PROJECT" 2>/dev/null || true
  incus profile device remove default root --project "$PROJECT" 2>/dev/null || true
  incus project delete "$PROJECT"
  incus project create "$PROJECT" -c restricted=true -c features.images=false -c features.profiles=true \
    -c features.networks=true -c restricted.networks.uplinks=incusbr0
  incus profile device add default root disk path=/ pool=local --project "$PROJECT"
  for fp in $grants; do
    projects=$(incus query "/1.0/certificates/$fp" | jq -c --arg p "$PROJECT" '(.projects // []) + [$p] | unique')
    incus query -X PATCH "/1.0/certificates/$fp" --data "{\"projects\": $projects}" >/dev/null
  done
fi
# Check the deploy identity can still use the project (see above).
for fp in $(incus query "/1.0/certificates?recursion=1" | jq -r '.[] | select(.restricted and (.name | test("-admin$") | not)) | .fingerprint'); do
  incus query "/1.0/certificates/$fp" | jq -e --arg p "$PROJECT" '(.projects // []) | index($p)' >/dev/null ||
    echo "warning: restricted identity ${fp:0:12} has no access to project $PROJECT" >&2
done
# The restricted project may use the service range for load balancers, and nothing else.
incus project set "$PROJECT" "restricted.networks.subnets=incusbr0:$SERVICES"
# Volume snapshots are refused in a restricted project unless allowed (a recreated project loses it).
incus project set "$PROJECT" restricted.snapshots=allow
# Instances without an application network get one shared OVN network.
if ! incus network show "$PROJECT-default" --project "$PROJECT" >/dev/null 2>&1; then
  incus network create "$PROJECT-default" --type=ovn network=incusbr0 --project "$PROJECT"
fi
devices=$(incus profile device list default --project "$PROJECT")
grep -qx eth0 <<<"$devices" || incus profile device add default eth0 nic name=eth0 "network=$PROJECT-default" --project "$PROJECT"

echo "OVN ready: northbound tcp:$central:6641; project $PROJECT owns its networks (uplink incusbr0, service addresses $SERVICES)"
incus network list --project "$PROJECT" -c ntsu
