#!/opt/homebrew/bin/bash
# SPDX-License-Identifier: AGPL-3.0-only
# The fence probe on the disposable test cluster (ADR 0013): an unprivileged account
# `apelora-fence` on every member, in group `sanlock`, whose only authorised key runs exactly
# `sanlock client gets -h 1` (read-only: the shared volume group's lockspace and each host's
# lease state). The worker reaches it over SSH on the member's lima0 address with a key and
# pinned host keys kept in ~/.config/apelora/test-cluster/fence-probe (mode 0700).
#
#   fence-probe.sh plan     # default: print every step, change nothing
#   fence-probe.sh apply    # do it (safe to repeat)
#   fence-probe.sh status   # run the probe against every member, as the worker would
#   fence-probe.sh remove   # delete the account on every member and the local key
#
# Afterwards: source ~/.config/apelora/test-cluster/fence-probe.env (worker only).
set -euo pipefail

MODE=${1:-plan}
DIR=${APELORA_FENCE_DIR:-$HOME/.config/apelora/test-cluster/fence-probe}
ENV_FILE=$HOME/.config/apelora/test-cluster/fence-probe.env
USER_NAME=apelora-fence
declare -A VMS=([apelora-test-1]=apelora-incus [apelora-test-2]=apelora-incus-2 [apelora-test-3]=apelora-incus-3 [apelora-test-4]=apelora-incus-4)
MEMBERS=(apelora-test-1 apelora-test-2 apelora-test-3 apelora-test-4)

on() { local vm=${VMS[$1]}; shift; limactl shell --workdir / "$vm" sudo bash -c "$*" </dev/null; }
addr() { on "$1" "ip -4 -o addr show lima0 | awk '{print \$4}' | cut -d/ -f1"; }
probe() {   # member address → what the worker would read
  ssh -F /dev/null -T -i "$DIR/id_ed25519" -o IdentitiesOnly=yes -o BatchMode=yes \
    -o StrictHostKeyChecking=yes -o "UserKnownHostsFile=$DIR/known_hosts" -o GlobalKnownHostsFile=/dev/null \
    -o ConnectTimeout=5 -o LogLevel=ERROR -l "$USER_NAME" "$1" lockspaces </dev/null
}

case $MODE in
  plan|apply) ;;
  status)
    for m in "${MEMBERS[@]}"; do
      a=$(addr "$m")
      echo "== $m ($a)"
      probe "$a" || echo "probe failed"
    done
    # The forced command must win over what the client asks for.
    a=$(addr "${MEMBERS[0]}")
    out=$(ssh -F /dev/null -T -i "$DIR/id_ed25519" -o IdentitiesOnly=yes -o BatchMode=yes -o StrictHostKeyChecking=yes \
      -o "UserKnownHostsFile=$DIR/known_hosts" -o LogLevel=ERROR -l "$USER_NAME" "$a" id </dev/null || true)
    if [[ $out == uid=* ]]; then echo "FAIL: the account ran another command"; exit 1; fi
    echo "the account runs only the forced command"
    exit 0 ;;
  remove)
    for m in "${MEMBERS[@]}"; do
      echo "$m: delete the account $USER_NAME"
      on "$m" "id $USER_NAME >/dev/null 2>&1 && userdel -r $USER_NAME 2>/dev/null; true"
    done
    rm -rf "$DIR" "$ENV_FILE"
    echo "local key, known hosts and $ENV_FILE removed"
    exit 0 ;;
  *) echo "usage: $0 plan|apply|status|remove" >&2; exit 2 ;;
esac

echo "Fence probe for the test cluster ($MODE): account $USER_NAME, key in $DIR"
echo "local: an ed25519 key without passphrase in $DIR/id_ed25519 (0600), unless it exists"
if [[ $MODE == apply ]]; then
  install -d -m 700 "$DIR"
  [[ -f $DIR/id_ed25519 ]] || ssh-keygen -q -t ed25519 -N "" -C apelora-fence-probe -f "$DIR/id_ed25519"
  : > "$DIR/known_hosts.new"
fi
targets=()
for m in "${MEMBERS[@]}"; do
  echo "$m (${VMS[$m]}):"
  echo "  - system account $USER_NAME in group sanlock (no password; shell /bin/sh, needed to run the forced command)"
  echo "  - ~$USER_NAME/.ssh/authorized_keys: restrict,command=\"<sanlock> client gets -h 1\" <the probe's public key>"
  echo "  - pin its host key (/etc/ssh/ssh_host_ed25519_key.pub) for its lima0 address"
  if [[ $MODE == apply ]]; then
    pub=$(cat "$DIR/id_ed25519.pub")
    on "$m" "set -e
      id $USER_NAME >/dev/null 2>&1 || useradd --system --create-home --home-dir /var/lib/$USER_NAME --shell /bin/sh $USER_NAME
      usermod -a -G sanlock $USER_NAME
      sl=\$(command -v sanlock)
      install -d -m 700 -o $USER_NAME -g $USER_NAME /var/lib/$USER_NAME/.ssh
      printf 'restrict,command=\"%s client gets -h 1\" %s\n' \"\$sl\" '$pub' > /var/lib/$USER_NAME/.ssh/authorized_keys
      chown $USER_NAME:$USER_NAME /var/lib/$USER_NAME/.ssh/authorized_keys
      chmod 600 /var/lib/$USER_NAME/.ssh/authorized_keys"
    a=$(addr "$m")
    echo "$a $(on "$m" "cut -d' ' -f1,2 /etc/ssh/ssh_host_ed25519_key.pub")" >> "$DIR/known_hosts.new"
    targets+=("$m=$a")
  fi
done
if [[ $MODE == apply ]]; then
  mv "$DIR/known_hosts.new" "$DIR/known_hosts"
  chmod 600 "$DIR/known_hosts"
  umask 077
  cat > "$ENV_FILE" <<EOF
# The worker's fence probe for the disposable test cluster (fence-probe.sh). Source it for the worker only; never commit it.
export APELORA_FENCE_PROBE=ssh
export APELORA_FENCE_PROBE_KEY=$DIR/id_ed25519
export APELORA_FENCE_PROBE_KNOWN_HOSTS=$DIR/known_hosts
export APELORA_FENCE_PROBE_TARGETS=$(IFS=,; echo "${targets[*]}")
EOF
  echo "wrote $ENV_FILE"
  "$0" status
else
  echo "Nothing was changed. Run '$0 apply' to do it."
fi
