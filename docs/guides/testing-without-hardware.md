# Testing without buying hardware

You can build, run and test Apelora on one Mac or Linux workstation. This page shows two levels, what each one lets you try, and what only real servers can show. You need a copy of the repository; level 2 also needs an Apple Silicon Mac.

## Level 1: no Incus at all (about a minute)

```sh
docker compose -f infra/dev/compose.yaml up -d       # PostgreSQL on 127.0.0.1:15432
DATABASE_URL= cargo run -p apelora-control-plane     # API on :8080 with the mock provider, no sign-in
npm run dev:console                                  # Fabric Console on :5174
npm run dev                                          # Studio on :5173
```

Open the Console at <http://127.0.0.1:5174> and the Studio at <http://127.0.0.1:5173>. A simulated Incus (the mock provider) stands in for a real one, and every page labels what it shows with "Mock · …".

This level is enough to try the screens, the permissions and the deployment logic. It tells you nothing about how Incus, storage or the network behave.

## Level 2: real Incus in virtual machines on the Mac

```sh
brew install lima                                    # once
bash scripts/test-cluster/up.sh                      # one Incus member, safe to repeat
source ~/.config/apelora/test-cluster/env            # its address and client certificates
cargo run -p apelora-control-plane                   # now talks to the lab
```

Each Lima virtual machine (VM) is one "server". The full lab is four members, plus a fifth VM that plays a storage area network (SAN). Scripts add the rest: `add-member.sh`, `ovn.sh`, `shared-storage.sh`, `backup-pool.sh` and `placement.sh`. The [test cluster walkthrough](../development/TEST-CLUSTER.md) shows each one.

| Resource | Needed |
|---|---|
| Machine | Apple Silicon. Virtual machines inside the lab (QEMU) need an M3 or later with macOS 15; containers do not |
| Memory | 8 GiB per member VM, 2 GiB for the SAN VM, 6 GiB for the optional MAAS VM. Start with one or two members; the full lab is about 40 GiB |
| Disk | 40 GiB per VM, sparse |
| Safety | Only the lab VMs (`apelora-incus`, `-2`, `-3`, `-4`, `apelora-san`) are ever changed. Credentials live in `~/.config/apelora/test-cluster`, never in the repository |

## What the lab covers, and what it cannot

| Concern | On your Mac | What Apelora does |
|---|---|---|
| **Power and BMC (IPMI, Redfish)** | There is no baseboard management controller (BMC). A VM's power button is `limactl stop`. | Apelora fences a failed server (makes sure it is off) before restarting its modules elsewhere. Three things count: Incus reports it offline **and** its storage lease is dead; a person confirms it is off; or Apelora switches it off through its BMC ([ADR 0014](../adr/0014-bmc-power-off-fencing.md)). In the lab, a provider that force-stops the VM stands in for the BMC: `HA_DRILL_BMC=1 bash scripts/test-cluster/ha-drill.sh plan` prints the steps. A BMC makes fencing possible in more cases; on clustered LVM it does not make the restart faster ([member failure](../runbooks/member-failure.md)). BMC credentials never go in the database, logs, the browser or the repository. |
| **Provisioning (PXE, installing an operating system)** | Not needed: VMs boot from a cloud image. | A machine that is already installed joins with a one-time token ([Enrolling a server](../design/node-enrolment.md)). MAAS can find a machine, commission it, install an operating system, run hardware tests, release it, and power it off ([Installing a server with MAAS](../design/provisioner-maas.md)). The lab's optional MAAS VM holds machine records with manual power that do not boot. |
| **Module network** | Real. OVN runs across the VMs, one network per application, with deny-by-default rules compiled from the connections drawn in the Studio. | Apelora compiles connections into OVN networks and access rules through the Incus API. The Networks page shows the intended rules next to Incus's. |
| **Physical network** (switches, VLANs, bonds, uplinks) | Not covered. The VMs share a NAT bridge. | Stays with the operator. Preflight checks the uplink ranges Apelora needs. |
| **Shared storage** | Real. An iSCSI disk from the SAN VM backs a clustered LVM pool. | Same code path as a real SAN. |
| **Hardware faults** (disks, fans, temperatures, NIC errors) | Not covered. | Not collected yet ([health and capacity](../design/health-and-capacity.md)). |
| **Speed** (disk and network throughput, latency) | Meaningless: everything shares one laptop. | Do not size anything from lab numbers. |

## Simulating failures

| Real event | In the lab |
|---|---|
| A server loses power | `limactl stop -f apelora-incus-4`, then `limactl start apelora-incus-4`. `bash scripts/test-cluster/ha-drill.sh plan` prints every step of the supervised version and refuses to run without a fresh plan |
| A server loses the SAN or the other servers | Packet-filter rules on the VM (listed in the drill's plan) |
| The control plane or worker restarts | `scripts/test-cluster/drill-worker-restart.sh` |
| A certificate expires | `scripts/test-cluster/rotate-identity.sh` |
| A server is added | `scripts/test-cluster/add-member.sh <n>` after registering and approving the machine in Apelora (the same flow real servers use) |

## When real servers arrive

Nothing in the lab is thrown away: the same join-token flow adds the servers, and the same tests run against them. Check these again on real servers, because the lab cannot show them:

1. Fencing through a real BMC. The Redfish and IPMI providers have been checked only against test servers so far. To read a real BMC safely, `tests/real_ipmi.rs` reads the power state and refuses a wrong GUID; it switches the server off only with `APELORA_IPMI_TEST_FENCE=1`.
2. Installing machines with MAAS on real hardware.
3. The uplink and VLANs.
4. Storage and network speed.
5. The first hardware fault.
