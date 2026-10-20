#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Installs Apelora's replica anti-affinity scriptlet (infra/incus/placement.star) on the
# disposable test cluster, or removes it with --remove. Safe to repeat.
# See docs/development/TEST-CLUSTER.md.
set -euo pipefail

VM=${APELORA_TEST_VM:-apelora-incus}
here=$(cd "$(dirname "$0")" && pwd)
incus() { limactl shell --workdir / "$VM" sudo incus "$@"; }

if [ "${1:-}" = "--remove" ]; then
  incus config unset instances.placement.scriptlet
  echo "placement scriptlet removed; Incus's built-in placement applies"
  exit 0
fi
# limactl shell joins and re-splits arguments, so the file and the command travel on stdin.
limactl shell --workdir / "$VM" sudo tee /var/tmp/apelora-placement.star >/dev/null <"$here/../../infra/incus/placement.star"
# key=value form: with "key value", a value containing "=" makes the key read as an instance.
echo 'incus config set "instances.placement.scriptlet=$(cat /var/tmp/apelora-placement.star)"' |
  limactl shell --workdir / "$VM" sudo bash -s
echo "placement scriptlet installed:"
incus config get instances.placement.scriptlet | head -3
