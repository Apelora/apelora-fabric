#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Adds a member to the disposable Incus test cluster: creates another Lima VM and joins it
# with a join token read from standard input (never from the command line, so it does not
# end up in shell history or process listings). Get the token from Apelora:
#
#   curl -s -X POST -H "Authorization: Bearer $APELORA_TOKEN" -H 'Content-Type: application/json' \
#     -d '{"memberName":"apelora-test-2"}' http://127.0.0.1:8080/api/v1/cluster/join-tokens |
#     jq -r .token | bash scripts/test-cluster/add-member.sh 2
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

# The preseed (with the token) goes over standard input as well.
vm sudo incus admin init --preseed <<PRESEED
cluster:
  enabled: true
  server_name: $MEMBER
  server_address: "$addr:8443"
  cluster_token: "$token"
  member_config:
    - entity: storage-pool
      name: local
      key: size
      value: 20GiB
PRESEED
vm sudo incus cluster list -c nusr
