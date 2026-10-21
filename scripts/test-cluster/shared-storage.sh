#!/opt/homebrew/bin/bash
# SPDX-License-Identifier: AGPL-3.0-only
# Shared block storage for the disposable test cluster: every member logs in to the lab SAN
# (the apelora-san VM, scripts/test-cluster/san-vm.yaml, iSCSI), one shared LVM volume group is
# created on its LUN with lvmlockd + sanlock coordinating access, and Incus gets an `lvmcluster`
# pool named `shared` on it. Instances on that pool can be restarted on another member when
# theirs fails. See docs/design/mobility-and-ha.md (slice B) and docs/development/TEST-CLUSTER.md.
#
#   shared-storage.sh plan       # default: print every step, change nothing
#   shared-storage.sh apply      # do it (safe to repeat; finished steps are skipped)
#   shared-storage.sh status     # sessions, locks, volume group and pool as each member sees them
#
# Lab only: the target has no authentication (generate_node_acls) and must never be used beyond
# this cluster.
set -euo pipefail

MODE=${1:-plan}
SAN_IP=${APELORA_SAN_IP:-192.168.104.6}
IQN=${APELORA_SAN_IQN:-iqn.2026-10.org.apelora:lab-shared}
VG=${APELORA_SHARED_VG:-apelora-shared}
POOL=${APELORA_SHARED_POOL:-shared}
# Member name → Lima VM. host_id for sanlock is the member's number (1..2000, unique).
declare -A VMS=([apelora-test-1]=apelora-incus [apelora-test-2]=apelora-incus-2 [apelora-test-3]=apelora-incus-3 [apelora-test-4]=apelora-incus-4)
MEMBERS=(apelora-test-1 apelora-test-2 apelora-test-3 apelora-test-4)
FIRST=${MEMBERS[0]}

on() { local vm=${VMS[$1]}; shift; limactl shell --workdir / "$vm" sudo bash -c "$*" </dev/null; }
step() { echo "  - $*"; }
run() { local m=$1; shift; if [[ $MODE == apply ]]; then on "$m" "$*"; fi; }
device() { echo "/dev/disk/by-path/ip-$SAN_IP:3260-iscsi-$IQN-lun-0"; }

case $MODE in
  plan|apply) ;;
  status)
    for m in "${MEMBERS[@]}"; do
      echo "== $m"
      on "$m" "iscsiadm -m session 2>/dev/null || echo 'no iSCSI session'; lvmlockctl --info 2>/dev/null | head -5 || true; vgs --noheadings -o vg_name,vg_lock_type,vg_size,vg_free 2>/dev/null | grep -F '$VG' || echo 'volume group not visible'"
    done
    limactl shell --workdir / "${VMS[$FIRST]}" sudo incus storage list -f csv </dev/null
    exit 0 ;;
  *) echo "usage: $0 plan|apply|status" >&2; exit 2 ;;
esac

echo "Shared storage for the test cluster ($MODE): target $IQN at $SAN_IP, volume group $VG, Incus pool $POOL"
n=0
for m in "${MEMBERS[@]}"; do
  n=$((n + 1))
  echo "$m (${VMS[$m]}), sanlock host_id $n:"
  step "install open-iscsi, lvm2-lockd and sanlock; load the software watchdog (softdog) now and at boot"
  run "$m" "DEBIAN_FRONTEND=noninteractive apt-get install -y -qq open-iscsi lvm2-lockd sanlock >/dev/null
    modprobe softdog; echo softdog > /etc/modules-load.d/apelora-softdog.conf"
  step "discover the target and log in, automatically at boot"
  run "$m" "systemctl enable --now iscsid >/dev/null 2>&1
    iscsiadm -m discovery -t sendtargets -p $SAN_IP:3260 >/dev/null
    iscsiadm -m node -T $IQN -p $SAN_IP:3260 --op update -n node.startup -v automatic
    iscsiadm -m session 2>/dev/null | grep -qF '$IQN' || iscsiadm -m node -T $IQN -p $SAN_IP:3260 --login >/dev/null
    udevadm settle; test -b $(device)"
  step "lvm (lvmlocal.conf, the original kept): use_lvmlockd = 1, host_id = $n; start wdmd, sanlock and lvmlockd"
  run "$m" "test -f /etc/lvm/lvmlocal.conf.apelora-orig || cp /etc/lvm/lvmlocal.conf /etc/lvm/lvmlocal.conf.apelora-orig
    printf 'global {\\n\\tuse_lvmlockd = 1\\n}\\nlocal {\\n\\thost_id = $n\\n}\\n' > /etc/lvm/lvmlocal.conf
    systemctl enable --now wdmd sanlock lvmlockd >/dev/null 2>&1"
done

echo "$FIRST: create the shared volume group $VG on $(device) (only if it does not exist)"
run "$FIRST" "vgs $VG >/dev/null 2>&1 || vgcreate --shared $VG $(device)"
for m in "${MEMBERS[@]}"; do
  echo "$m: start the volume group's lock space"
  run "$m" "vgchange --lockstart $VG >/dev/null"
done

echo "Incus: pool $POOL (lvmcluster) on $VG, defined on every member, then created"
if [[ $MODE == apply ]]; then
  incus() { limactl shell --workdir / "${VMS[$FIRST]}" sudo incus "$@" </dev/null; }
  if incus storage show "$POOL" >/dev/null 2>&1; then
    echo "  pool $POOL exists"
  else
    for m in "${MEMBERS[@]}"; do incus storage create "$POOL" lvmcluster source="$VG" --target "$m" >/dev/null; done
    incus storage create "$POOL" lvmcluster
  fi
  incus storage list -f csv
else
  for m in "${MEMBERS[@]}"; do step "incus storage create $POOL lvmcluster source=$VG --target $m"; done
  step "incus storage create $POOL lvmcluster"
  echo "Nothing was changed. Run '$0 apply' to do it."
fi
