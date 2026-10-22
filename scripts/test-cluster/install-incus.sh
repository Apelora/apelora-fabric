#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Installs Incus 7.0 LTS on a throwaway Debian or Ubuntu machine (a CI runner) from the
# packaging repository named in Incus's installation guide, as the lab VM does
# (scripts/test-cluster/incus-vm.yaml). Refuses a signing key whose fingerprint differs from the
# published one. Apelora itself never installs Incus; this is test-fixture setup only.
set -euo pipefail

[ "$(id -u)" = 0 ] || exec sudo -E bash "$0" "$@"
command -v incus >/dev/null && { incus version; exit 0; }
export DEBIAN_FRONTEND=noninteractive
apt-get update -q
apt-get install -yq curl gpg btrfs-progs
install -d -m 0755 /etc/apt/keyrings
curl -fsSL https://pkgs.zabbly.com/key.asc -o /etc/apt/keyrings/zabbly.asc
want=4EFC590696CB15B87C73A3AD82CC8797C838DCFD
got=$(gpg --show-keys --with-colons /etc/apt/keyrings/zabbly.asc | awk -F: '/^fpr/ {print $10; exit}')
[ "$got" = "$want" ] || { echo "unexpected package signing key: $got" >&2; rm -f /etc/apt/keyrings/zabbly.asc; exit 1; }
cat >/etc/apt/sources.list.d/zabbly-incus-lts-7.0.sources <<EOF
Enabled: yes
Types: deb
URIs: https://pkgs.zabbly.com/incus/lts-7.0
Suites: $(. /etc/os-release && echo "$VERSION_CODENAME")
Components: main
Architectures: $(dpkg --print-architecture)
Signed-By: /etc/apt/keyrings/zabbly.asc
EOF
apt-get update -q
apt-get install -yq incus
# Docker on CI runners drops forwarded traffic by default, which cuts instances on incusbr0 off
# from image servers; let forwarded traffic through on this throwaway machine only.
if iptables -L DOCKER-USER >/dev/null 2>&1; then
  iptables -I DOCKER-USER -j ACCEPT
fi
incus version
