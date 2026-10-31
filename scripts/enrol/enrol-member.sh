#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Enrols this server as a member of an Apelora cluster (Phase 09, docs/design/node-enrolment.md).
# Run as root on the new server after installing its operating system and Incus (the release in docs/guides/install-incus.md), with
# the join token from Apelora's "Approve" on standard input (or pasted at the prompt, which does
# not echo it). The token never appears on a command line, in a file or in the output.
#
#   sudo bash enrol-member.sh --name m4 --address 10.0.0.14 \
#        [--pool-size 20GiB | --member-config pool.yaml] \
#        [--ovn-remote tcp:10.0.0.11:6642 --ovn-encap-ip 10.0.0.14]
#
# Steps: checks this server; configures its OVN controller (when asked); joins the Incus cluster
# with the token; then holds the new member out of automatic placement
# (scheduler.instance=manual) until an administrator admits it in Apelora.
#
# --pool-size SIZE: the cluster's "local" pool is a sized loop (btrfs). The join has to answer
# both questions Incus asks: the size, and an empty source so Incus creates the loop file.
# A real disk uses --member-config instead (YAML list items), for example:
#   - entity: storage-pool
#     name: local
#     key: source
#     value: /dev/nvme1n1
set -euo pipefail

usage() {
  echo "usage: enrol-member.sh --name NAME --address IP [--pool-size 20GiB | --member-config FILE] [--ovn-remote tcp:HOST:6642 --ovn-encap-ip IP]" >&2
  echo "The join token is read from standard input and is never a command argument." >&2
  exit 2
}
die() { echo "enrol: $*" >&2; exit 1; }

# YAML for a sized loop pool named local. An empty source is the answer that creates the file.
write_pool_config() {
  printf '%s\n' \
    "- entity: storage-pool" \
    "  name: local" \
    "  key: size" \
    "  value: \"$pool_size\"" \
    "- entity: storage-pool" \
    "  name: local" \
    "  key: source" \
    "  value: \"\""
}

name="" address="" member_config="" pool_size="" ovn_remote="" encap_ip="" print_config=0
while [ $# -gt 0 ]; do
  case "$1" in
    --name) name=${2:-}; shift 2 ;;
    --address) address=${2:-}; shift 2 ;;
    --member-config) member_config=${2:-}; shift 2 ;;
    --pool-size) pool_size=${2:-}; shift 2 ;;
    --ovn-remote) ovn_remote=${2:-}; shift 2 ;;
    --ovn-encap-ip) encap_ip=${2:-}; shift 2 ;;
    --print-member-config) print_config=1; shift ;;
    *) usage ;;
  esac
done
[ -n "$name" ] && [ -n "$address" ] || usage
[[ $name =~ ^[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?$ ]] || die "invalid member name: $name"
[[ $address =~ ^[0-9A-Fa-f.:]+$ ]] || die "the address must be this server's IP address"
[ -z "$pool_size" ] || [ -z "$member_config" ] || die "pass --pool-size or --member-config, not both"
[ -z "$pool_size" ] || [[ $pool_size =~ ^[1-9][0-9]{0,5}(KiB|MiB|GiB|TiB)$ ]] || die "pool size must look like 20GiB"
[ -z "$member_config" ] || [ -r "$member_config" ] || die "cannot read $member_config"
[ -z "$ovn_remote" ] || [ -n "$encap_ip" ] || die "--ovn-remote needs --ovn-encap-ip"
if [ "$print_config" -eq 1 ]; then
  if [ -n "$pool_size" ]; then write_pool_config; exit 0; fi
  if [ -n "$member_config" ]; then cat "$member_config"; exit 0; fi
  die "--print-member-config needs --pool-size or --member-config"
fi
[ "$(id -u)" -eq 0 ] || die "run as root"
if [ -n "$pool_size" ]; then
  member_config=$(mktemp)
  trap 'rm -f "$member_config"' EXIT
  write_pool_config >"$member_config"
fi

# --- This server ------------------------------------------------------------------------------
command -v incus >/dev/null || die "Incus is not installed"
server=$(incus version 2>/dev/null | awk -F': *' '/^Server version/ {print $2}')
[ -n "$server" ] || die "the Incus daemon is not running"
[ "${server%%.*}" -ge 7 ] 2>/dev/null || die "Incus $server is too old; Apelora needs 7.0 or later"
kernel=$(uname -r)
IFS=. read -r kmaj kmin _ <<<"$kernel"
kmin=${kmin%%[!0-9]*}
{ [ "$kmaj" -gt 6 ] || { [ "$kmaj" -eq 6 ] && [ "$kmin" -ge 12 ]; }; } ||
  die "kernel $kernel is too old; Apelora needs 6.12 or later"
if incus cluster list >/dev/null 2>&1; then
  die "this server is already a cluster member"
fi
[ -z "$(incus list --all-projects --format csv -c n 2>/dev/null)" ] ||
  die "this server already has instances; a joining server must be empty"

# --- The join token ---------------------------------------------------------------------------
if [ -t 0 ]; then
  printf 'Join token for %s (input hidden): ' "$name" >&2
  IFS= read -rs token; echo >&2
else
  IFS= read -r token || true
fi
[[ $token =~ ^[A-Za-z0-9+/=]{20,}$ ]] || die "expected a join token"

# --- OVN controller (before joining, so a failure leaves nothing half done) --------------------
if [ -n "$ovn_remote" ]; then
  command -v ovs-vsctl >/dev/null || die "--ovn-remote needs Open vSwitch and the OVN host package installed"
  ovs-vsctl set open_vswitch . "external_ids:ovn-remote=$ovn_remote" \
    external_ids:ovn-encap-type=geneve "external_ids:ovn-encap-ip=$encap_ip"
  echo "enrol: OVN controller points at $ovn_remote (tunnel endpoint $encap_ip)"
fi

# --- Join, then hold ---------------------------------------------------------------------------
{
  printf 'cluster:\n  enabled: true\n  server_name: %s\n  server_address: "%s"\n' "$name" "$address:8443"
  printf '  cluster_token: "%s"\n' "$token"
  if [ -n "$member_config" ]; then
    printf '  member_config:\n'
    sed 's/^/    /' "$member_config"
  fi
} | incus admin init --preseed
unset token
echo "enrol: joined the cluster as $name"

# Incus places nothing on a held member unless an instance is targeted at it.
incus cluster set "$name" scheduler.instance=manual
echo "enrol: $name is held from placement; admit it in Apelora (Fabric Console → Membership → Machines)"
