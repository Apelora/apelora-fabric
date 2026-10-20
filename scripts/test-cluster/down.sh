#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Deletes the disposable Incus test VM and its client credentials. Nothing else is touched.
set -euo pipefail

VM=${APELORA_TEST_VM:-apelora-incus}
DIR=${APELORA_TEST_DIR:-$HOME/.config/apelora/test-cluster}

if limactl list --format '{{.Name}}' | grep -qx "$VM"; then
  limactl delete --force "$VM"
else
  echo "no VM named $VM"
fi
rm -f "$DIR/client.crt" "$DIR/client.key" "$DIR/server.crt" "$DIR/env"
rmdir "$DIR" 2>/dev/null || true
echo "removed $VM and its credentials"
