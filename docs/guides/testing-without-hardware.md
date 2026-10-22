# Testing without buying hardware

You can build, run and test Apelora on one Mac (or Linux workstation). Two levels, and a plain statement of what each one proves and what only real servers can prove.

## Level 1: no Incus at all (about a minute)

```sh
docker compose -f infra/dev/compose.yaml up -d       # PostgreSQL on 127.0.0.1:15432
DATABASE_URL= cargo run -p apelora-control-plane     # API on :8080 with the mock provider, no sign-in
npm run dev:console                                  # Fabric Console on :5174
npm run dev                                          # Studio on :5173
```

A mock stands in for Incus. Every page labels what it shows ("Mock · …"). This proves the logic, the permissions and the screens. Evidence label: **MOCK VERIFIED**. It proves nothing about how Incus, storage or the network behave.

## Level 2: real Incus in VMs on the Mac

```sh
brew install lima                                    # once
bash scripts/test-cluster/up.sh                      # one Incus member, safe to repeat
source ~/.config/apelora/test-cluster/env            # its address and client certificates
cargo run -p apelora-control-plane                   # now talks to the lab
```

Each Lima VM is one "server". The full lab is four members plus a fifth VM that plays a SAN (`add-member.sh`, `ovn.sh`, `shared-storage.sh`, `backup-pool.sh`, `placement.sh`; the walkthrough is in [TEST-CLUSTER.md](../development/TEST-CLUSTER.md)). Evidence label: **REAL INCUS TESTED**.

| Resource | Needed |
|---|---|
| Machine | Apple Silicon. Virtual machines inside the lab (QEMU) need an M3 or later with macOS 15; containers do not |
| Memory | 8 GiB per member VM, 2 GiB for the SAN VM, 6 GiB for the optional MAAS VM. Start with one or two members; the full lab is about 40 GiB |
| Disk | 40 GiB per VM, sparse |
| Safety | Only the lab VMs (`apelora-incus`, `-2`, `-3`, `-4`, `apelora-san`) are ever changed. Credentials live in `~/.config/apelora/test-cluster`, never in the repository |

## What the lab covers, and what it cannot

| Concern | On your Mac | What Apelora does |
|---|---|---|
| **Power and BMC (IPMI, Redfish)** | There is no BMC. A VM's power button is `limactl stop`. | Apelora fences a failed server by checking that Incus reports it offline **and** its storage lease is dead, or by asking a person to confirm it is off. Powering a server off through its BMC is the third way ([ADR 0014](../adr/0014-bmc-power-off-fencing.md), built): the lab drives it with a provider that force-stops the VM (`HA_DRILL_BMC=1 bash scripts/test-cluster/ha-drill.sh plan`, **REAL INCUS TESTED**: a server cut off but still running was fenced at 61 s), and the Redfish and IPMI providers are **CONTRACT VERIFIED** (Redfish against a Redfish-shaped test server; IPMI against a test BMC on loopback UDP and an independent IPMI implementation acting as one) until a real BMC runs them. For a real BMC, `tests/real_ipmi.rs` reads the power state and refuses a wrong GUID, and switches the server off only with `APELORA_IPMI_TEST_FENCE=1`. A BMC makes fencing possible in more cases; on clustered LVM it does not make the restart faster ([the measurement](../runbooks/member-failure.md)). BMC credentials never go in the database, logs, the browser or the repository. |
| **Provisioning (PXE, installing an operating system)** | Not needed: VMs boot from a cloud image. | Installing the operating system is not Apelora's job ([node enrolment](../design/node-enrolment.md)). A machine you have already installed joins with a one-time join token. The MAAS provisioner adapter has its first slice ([design](../design/provisioner-maas.md)): the Console lists what MAAS manages and registers a machine with its MAAS id, and MAAS can be the fence provider with the BMC credentials staying in MAAS. It is **REAL MAAS TESTED** against a throwaway MAAS in a Lima VM (`bash scripts/test-cluster/maas.sh up`; the machines are records with manual power, not hardware). Commissioning, hardware tests (burn-in), installing an operating system and release are built but only CONTRACT VERIFIED: the lab's records never boot, so none of them can run there. |
| **Module network** | Real. OVN runs across the VMs, one network per application, deny-by-default rules compiled from the connections drawn in the Studio. | Apelora compiles connections into OVN networks and ACLs through the Incus API. Networks shows the intended rules next to Incus's. |
| **Physical network** (switches, VLANs, bonds, uplinks) | Not covered. The VMs share a NAT bridge. | Stays with the operator. Preflight checks the uplink ranges Apelora needs. |
| **Shared storage** | Real. An iSCSI LUN from the SAN VM backs a clustered LVM pool; the failover drills used it. | Same code path as a real SAN. |
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

Nothing in the lab is thrown away: the same join-token flow adds them, and the same tests run against them. What is new, and must be re-proved on them, is exactly the right-hand column above: the BMC fence provider against a real BMC, the uplink and VLANs, storage speed and the first hardware fault. Until then, the documents say CONTRACT VERIFIED or NOT YET VERIFIED, never more.
