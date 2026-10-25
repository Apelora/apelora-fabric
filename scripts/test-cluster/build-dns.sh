#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Builds apelora-dns as a static Linux executable for the test cluster's architecture and prints
# its path. The deployment worker installs it into a design's resolver module
# (APELORA_DNS_BINARY). Used by dns.sh and ui-e2e.sh.
#   APELORA_DNS_BINARY=$(bash scripts/test-cluster/build-dns.sh)
set -euo pipefail

repo=$(cd "$(dirname "$0")/../.." && pwd)
arch=${LAB_ARCH:-$(limactl shell --workdir / "${LAB_MEMBER_VM:-apelora-incus}" uname -m)}
target="$arch-unknown-linux-musl"
if ! rustup target list --installed | grep -qx "$target"; then rustup target add "$target" >&2; fi
host=$(rustc -vV | awk '/^host:/{print $2}')
linker=$(rustc --print sysroot)/lib/rustlib/$host/bin/rust-lld
[ -x "$linker" ] || linker=$(rustc --print sysroot)/lib/rustlib/$host/bin/gcc-ld/ld.lld
linker_var="CARGO_TARGET_$(echo "$target" | tr '[:lower:]-' '[:upper:]_')_LINKER"
(cd "$repo" && env "$linker_var=$linker" cargo build -q --release -p apelora-dns-resolver --target "$target") >&2
out="$repo/target/$target/release/apelora-dns"
[ -x "$out" ] || { echo "no Linux resolver at $out" >&2; exit 1; }
echo "$out"
