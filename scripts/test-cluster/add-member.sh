#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Adds a member to the disposable Incus test cluster: creates another Lima VM and enrols it with
# scripts/enrol/enrol-member.sh (the same script a hand-installed server runs), passing the join
# token on standard input (never on a command line, so it stays out of shell history and process
# listings). The new member is held from placement until it is admitted in Apelora. Get the
# token by registering and approving the machine:
#
#   api() { curl -s -H "Authorization: Bearer $APELORA_TOKEN" -H 'Content-Type: application/json' "$@"; }
#   api -d '{"name":"apelora-test-4"}' http://127.0.0.1:8080/api/v1/machines >/dev/null
#   api -X POST http://127.0.0.1:8080/api/v1/machines/apelora-test-4/approve |
#     jq -r .token | bash scripts/test-cluster/add-member.sh 4
#   api -X POST http://127.0.0.1:8080/api/v1/machines/apelora-test-4/admit
#
# See docs/development/TEST-CLUSTER.md.
set -euo pipefail

n=${1:?usage: add-member.sh <member number ≥ 2>  (join token on stdin)}
[[ $n =~ ^[2-9]$ ]] || { echo "member number must be 2–9" >&2; exit 1; }
VM=${APELORA_TEST_VM:-apelora-incus}-$n
MEMBER=apelora-test-$n
here=$(cd "$(dirname "$0")" && pwd)

IFS= read -r token || true
[[ $token =~ ^[A-Za-z0-9+/=]{20,}$ ]] || { echo "expected a join token on standard input" >&2; exit 1; }

vm() { limactl shell --workdir / "$VM" "$@"; }

state=$(limactl list --format '{{.Status}}' "$VM" 2>/dev/null || true)
case "$state" in
  "") limactl start --tty=false --name "$VM" "$here/incus-vm.yaml" ;;
  Running) ;;
  *) limactl start --tty=false "$VM" ;;
esac

if vm sudo incus cluster list >/dev/null 2>&1; then
  echo "$MEMBER is already a cluster member"
  exit 0
fi
# Cluster traffic uses eth0 (Lima user-v2); vzNAT (lima0) isolates VMs from each other.
addr=$(vm ip -4 -o addr show dev eth0 | awk '{print $4}' | cut -d/ -f1 | head -1)
[ -n "$addr" ] || { echo "the VM has no address on eth0" >&2; exit 1; }

# The lab's storage pool is a sized loop file. --pool-size answers size and an empty source.
args=(--name "$MEMBER" --address "$addr" --pool-size 20GiB)
# When the cluster uses OVN (ovn.sh), the new member needs its OVN controller.
first=${APELORA_TEST_VM:-apelora-incus}
if [ -n "$(limactl shell --workdir / "$first" sudo incus config get network.ovn.northbound_connection </dev/null)" ]; then
  vm sudo env DEBIAN_FRONTEND=noninteractive apt-get install -yq ovn-host </dev/null >/dev/null
  # Prefer the live southbound remotes (clustered TLS after ovn-ha.sh, else ovn.sh's one tcp:).
  ovn_remote=$(limactl shell --workdir / "$first" sudo ovs-vsctl get open_vswitch . external_ids:ovn-remote </dev/null | tr -d '"')
  [ -n "$ovn_remote" ] || {
    central=$(limactl shell --workdir / "$first" ip -4 -o addr show dev eth0 </dev/null | awk '{print $4}' | cut -d/ -f1 | head -1)
    ovn_remote="tcp:$central:6642"
  }
  args+=(--ovn-remote "$ovn_remote" --ovn-encap-ip "$addr")
fi
limactl copy "$here/../enrol/enrol-member.sh" "$VM:/tmp/apelora-enrol-member.sh"
printf '%s\n' "$token" | vm sudo bash /tmp/apelora-enrol-member.sh "${args[@]}"
unset token
vm rm -f /tmp/apelora-enrol-member.sh
vm sudo incus cluster list -c nusr </dev/null
