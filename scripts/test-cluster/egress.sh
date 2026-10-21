#!/opt/homebrew/bin/bash
# SPDX-License-Identifier: AGPL-3.0-only
# Blocked-egress profile for the disposable test cluster (ADR 0010, docs/development/TEST-CLUSTER.md):
# outbound traffic from the member VMs, and from the instances they run, is dropped except to the
# lab's own networks (which include the host), the VMs' DNS resolvers, and the declared mirror.
# The real-Incus tests must pass like this before a release is called ready for installations
# without internet access.
#
#   egress.sh plan        # default: print the rules for each member, change nothing
#   egress.sh block       # load the rules (nftables table inet apelora_egress) on every member
#   egress.sh unblock     # remove that table on every member
#   egress.sh status      # whether the table is loaded, its drop counters, and what answers
#
# Environment:
#   APELORA_LAB_MIRROR   mirror address(es) the members may reach: IPv4[/prefix][:port], comma
#                        separated, or "none". Required for block.
#   APELORA_LAB_ALLOW    further IPv4[/prefix] destinations to allow (e.g. an NTP server).
#   APELORA_LAB_PROBE    URL status uses to show outbound access is blocked
#                        (default: the upstream simplestreams index).
#
# Lab only. The rules are not saved: a VM restart clears them. Run block/unblock only after
# reviewing `plan`; nothing here touches the SAN VM, which pulls no images.
set -euo pipefail

MODE=${1:-plan}
TABLE=apelora_egress
VM=${APELORA_TEST_VM:-apelora-incus}
MIRROR=${APELORA_LAB_MIRROR:-}
ALLOW=${APELORA_LAB_ALLOW:-}
PROBE=${APELORA_LAB_PROBE:-https://images.linuxcontainers.org/streams/v1/index.json}

die() { echo "egress: $*" >&2; exit 1; }
case $MODE in plan|block|unblock|status) ;; *) echo "usage: $0 plan|block|unblock|status" >&2; exit 2 ;; esac
command -v limactl >/dev/null || die "limactl is needed (brew install lima)"

ipv4='([0-9]{1,3}\.){3}[0-9]{1,3}(/[0-9]{1,2})?'
# Mirrors: "addr[:port]" → nft match fragments ("ip daddr A tcp dport P" or "ip daddr A").
mirror_rules=()
if [[ -n $MIRROR && $MIRROR != none ]]; then
  IFS=, read -r -a items <<< "$MIRROR"
  for m in "${items[@]}"; do
    m=${m// /}
    [[ $m =~ ^($ipv4)(:([0-9]{1,5}))?$ ]] || die "APELORA_LAB_MIRROR: $m is not IPv4[/prefix][:port]"
    if [[ -n ${BASH_REMATCH[5]} ]]; then
      mirror_rules+=("ip daddr ${BASH_REMATCH[1]} tcp dport ${BASH_REMATCH[5]}")
    else
      mirror_rules+=("ip daddr ${BASH_REMATCH[1]}")
    fi
  done
fi
extra=()
if [[ -n $ALLOW ]]; then
  IFS=, read -r -a items <<< "$ALLOW"
  for a in "${items[@]}"; do
    a=${a// /}
    [[ $a =~ ^$ipv4$ ]] || die "APELORA_LAB_ALLOW: $a is not IPv4[/prefix]"
    extra+=("$a")
  done
fi

members=$(limactl list --format '{{.Name}} {{.Status}}' 2>/dev/null | grep -E "^${VM}(-[0-9]+)? " | sort || true)
[[ -n $members ]] || die "no lab member VMs (${VM}, ${VM}-2, …) found"

on() { local vm=$1; shift; limactl shell --workdir / "$vm" sudo bash -c "$*" </dev/null; }

# What a member's rules depend on, read (never changed) from inside it.
facts() {
  on "$1" '
    echo "nets $(ip -4 route show proto kernel scope link | awk "{print \$1}" | sort -u | tr "\n" " ")"
    echo "host $(getent hosts host.lima.internal | awk "{print \$1}" | head -1)"
    echo "dns $( { awk "/^nameserver/{print \$2}" /etc/resolv.conf; resolvectl dns 2>/dev/null | grep -oE "([0-9]{1,3}\.){3}[0-9]{1,3}"; } | grep -vE "^127\." | sort -u | tr "\n" " ")"
    echo "uplinks $(ip -4 route show default | awk "{print \$5}" | sort -u | tr "\n" " ")"
    command -v nft >/dev/null && echo "nft yes" || echo "nft no"'
}

# The ruleset for one member, from its facts. One transaction: the table is replaced whole.
ruleset() {
  local nets=$1 host=$2 dns=$3 uplinks=$4
  local lab
  lab=$(echo "$nets ${host} ${extra[*]:-}" | tr ' ' '\n' | grep -E . | sort -u | paste -sd, - | sed 's/,/, /g')
  local ifs
  ifs=$(echo "$uplinks" | tr ' ' '\n' | grep -E . | sed 's/.*/"&"/' | paste -sd, - | sed 's/,/, /g')
  cat <<EOF
table inet $TABLE
delete table inet $TABLE
table inet $TABLE {
  # Lab networks (their kernel routes), the host, and APELORA_LAB_ALLOW.
  set lab4 { type ipv4_addr; flags interval; auto-merge; elements = { $lab } }
  chain from_member {
    type filter hook output priority 0; policy drop;
    oif "lo" accept
    ct state established,related accept
    ip daddr @lab4 accept
    ip daddr 255.255.255.255 udp dport 67 accept
EOF
  for d in $dns; do echo "    ip daddr $d meta l4proto { tcp, udp } th dport 53 accept"; done
  for r in ${mirror_rules[@]+"${mirror_rules[@]}"}; do echo "    $r accept"; done
  cat <<EOF
    ip6 daddr { fe80::/10, ff02::/16 } accept
    icmpv6 type { nd-router-solicit, nd-neighbor-solicit, nd-neighbor-advert } accept
    limit rate 6/minute log prefix "apelora-egress-drop " level info
    counter comment "dropped: from the member itself"
  }
  chain from_instances {
    # Instances: traffic between them and to the lab passes; leaving through the VM's uplink does not.
    type filter hook forward priority 0; policy accept;
    ct state established,related accept
    ip daddr @lab4 accept
EOF
  for r in ${mirror_rules[@]+"${mirror_rules[@]}"}; do echo "    $r accept"; done
  if [[ -n $ifs ]]; then
    echo "    oifname { $ifs } counter drop comment \"dropped: from instances\""
  else
    echo "    # no default route found: nothing leaves the VM"
  fi
  echo "  }"
  echo "}"
}

case $MODE in
  status)
    while read -r name state; do
      echo "== $name ($state)"
      [[ $state == Running ]] || continue
      on "$name" "
        if nft list table inet $TABLE >/dev/null 2>&1; then
          echo 'egress: blocked'
          nft list table inet $TABLE | grep -E 'counter packets' | sed 's/^ */  /'
        else
          echo 'egress: open (no $TABLE table)'
        fi
        if command -v curl >/dev/null; then
          code=\$(curl -s -o /dev/null -m 4 -w '%{http_code}' '$PROBE' || true)
          [ \"\$code\" = 000 ] && echo '  $PROBE: no answer' || echo \"  $PROBE: HTTP \$code (reachable)\"
        fi"
    done <<< "$members"
    exit 0 ;;
  unblock)
    while read -r name state; do
      [[ $state == Running ]] || { echo "== $name: $state, skipped"; continue; }
      on "$name" "nft list table inet $TABLE >/dev/null 2>&1 && nft delete table inet $TABLE; true"
      echo "== $name: outbound traffic open again"
    done <<< "$members"
    exit 0 ;;
esac

if [[ $MODE == block && -z $MIRROR ]]; then
  die "set APELORA_LAB_MIRROR to the mirror's address (or none)"
fi
echo "Blocked egress for the test cluster ($MODE). Mirror: ${MIRROR:-not set}. Allowed besides: ${ALLOW:-nothing}."
while read -r name state; do
  echo
  echo "== $name"
  if [[ $state != Running ]]; then
    echo "  $state: skipped"
    continue
  fi
  f=$(facts "$name")
  nets=$(sed -n 's/^nets //p' <<< "$f")
  host=$(sed -n 's/^host //p' <<< "$f")
  dns=$(sed -n 's/^dns //p' <<< "$f")
  uplinks=$(sed -n 's/^uplinks //p' <<< "$f")
  grep -q '^nft yes' <<< "$f" || die "$name has no nft (apt-get install nftables)"
  [[ -n ${nets// /} ]] || die "$name: no lab networks found; refusing to block everything"
  rules=$(ruleset "$nets" "$host" "$dns" "$uplinks")
  encoded=$(base64 <<< "$rules" | tr -d '\n')
  if [[ $MODE == plan ]]; then
    sed 's/^/  /' <<< "$rules"
    # nft --check parses and validates the whole transaction without loading anything.
    if on "$name" "echo $encoded | base64 -d | nft --check -f -"; then
      echo "  (nft --check: the rules are valid on $name)"
    else
      die "$name: nft refused these rules"
    fi
  else
    on "$name" "echo $encoded | base64 -d | nft -f -"
    echo "  loaded: outbound traffic now goes only to the lab, its resolvers and the mirror"
  fi
done <<< "$members"
if [[ $MODE == plan ]]; then
  echo
  echo "Nothing was changed. Review the rules, then run '$0 block' (and '$0 unblock' to undo)."
fi
