#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Deletes the disposable Incus test VMs (the first member and any added with add-member.sh)
# and the client credentials. Nothing else is touched.
set -euo pipefail

VM=${APELORA_TEST_VM:-apelora-incus}
DIR=${APELORA_TEST_DIR:-$HOME/.config/apelora/test-cluster}

vms=$(limactl list --format '{{.Name}}' | grep -E "^${VM}(-[0-9]+)?\$" || true)
[ -n "$vms" ] || echo "no VM named $VM"
for v in $vms; do limactl delete --force "$v"; done
rm -f "$DIR"/client.crt "$DIR"/client.key "$DIR"/admin.crt "$DIR"/admin.key "$DIR/server.crt" "$DIR/env"
rmdir "$DIR" 2>/dev/null || true
echo "removed $VM and its credentials"
