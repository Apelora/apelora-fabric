#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Installs Incus on a throwaway Debian or Ubuntu machine (a CI runner) from the channel of the
# packaging repository that `channel` names in upstream.lock.yaml (APELORA_INCUS_CHANNEL overrides
# it), as the lab VM does (scripts/test-cluster/incus-vm.yaml). The stable channel offers its
# newest release, which may be ahead of the pin; the run prints the version it got. Refuses a signing key whose fingerprint differs from the
# published one. Apelora itself never installs Incus; this is test-fixture setup only.
set -euo pipefail

[ "$(id -u)" = 0 ] || exec sudo -E bash "$0" "$@"

CHANNEL=${APELORA_INCUS_CHANNEL:-}
if [ -z "$CHANNEL" ]; then
  # shellcheck source=../upstream/lib.sh
  source "$(dirname "$0")/../upstream/lib.sh"
  CHANNEL=$(lock_get incus channel)
fi
[[ $CHANNEL =~ ^(stable|lts-[0-9]+\.[0-9]+)$ ]] || { echo "channel is stable or lts-<series>, got: $CHANNEL" >&2; exit 1; }

# Docker on CI runners drops forwarded traffic by default, which cuts instances on incusbr0 off
# from image servers. Only this bridge, and only on this throwaway machine.
forward_incusbr0() {
  iptables -L DOCKER-USER >/dev/null 2>&1 || return 0
  for dir in i o; do
    iptables -C DOCKER-USER -"$dir" incusbr0 -j ACCEPT 2>/dev/null && continue
    iptables -I DOCKER-USER -"$dir" incusbr0 -j ACCEPT
  done
}

if command -v incus >/dev/null; then
  v=$(incus version)
  echo "$v" | grep -qE '7\.[0-9]+' || { echo "need Incus 7, got: $v" >&2; exit 1; }
  forward_incusbr0
  incus version
  exit 0
fi
export DEBIAN_FRONTEND=noninteractive

# A new guest is still installing its own packages. apt then exits at once with the lists
# lock held, which is what happened on three of four guests (2026-10-24).
if command -v cloud-init >/dev/null 2>&1; then
  cloud-init status --wait >&2 || true
fi
apt_get() {
  local i
  for i in 1 2 3 4 5 6 7 8 9 10 11 12; do
    if apt-get "$@"; then return 0; fi
    echo "package install is busy, waiting ($i)" >&2
    sleep 5
  done
  return 1
}
apt_get update -q
apt_get install -yq curl gpg btrfs-progs
install -d -m 0755 /etc/apt/keyrings
curl -fsSL https://pkgs.zabbly.com/key.asc -o /etc/apt/keyrings/zabbly.asc
want=4EFC590696CB15B87C73A3AD82CC8797C838DCFD
got=$(gpg --show-keys --with-colons /etc/apt/keyrings/zabbly.asc | awk -F: '/^fpr/ {print $10; exit}')
[ "$got" = "$want" ] || { echo "unexpected package signing key: $got" >&2; rm -f /etc/apt/keyrings/zabbly.asc; exit 1; }
cat >/etc/apt/sources.list.d/zabbly-incus-$CHANNEL.sources <<EOF
Enabled: yes
Types: deb
URIs: https://pkgs.zabbly.com/incus/$CHANNEL
Suites: $(. /etc/os-release && echo "$VERSION_CODENAME")
Components: main
Architectures: $(dpkg --print-architecture)
Signed-By: /etc/apt/keyrings/zabbly.asc
EOF
apt_get update -q
apt_get install -yq incus
forward_incusbr0
incus version
