# Servers and network

What the servers need, how many network cards, and which IP addresses Apelora Fabric and Incus use. Read this before [installing Incus](install-incus.md).

**Evidence:** the layouts below are what the four-server lab runs (virtual machines with one network card each, a private network between them, and a NAT bridge as the application uplink). Physical servers, separate management and storage networks and physical uplinks have **not** been tried. IPv6 has been tried inside application networks and for service addresses (`ipv6.routes`); the servers' own cluster addresses stay IPv4. Where a statement rests on Incus's documentation or is our recommendation, it says so. See [testing without hardware](testing-without-hardware.md) for what the lab can and cannot prove.

## The short answers

- **One network card is enough to start**, for a lab or a small evaluation: everything shares it. For anything you depend on, **two or more** is better: one for management and cluster traffic and a separate one for the networks your applications reach the outside through. Details in [network layouts](#network-layouts).
- **Use static IP addresses**, private or public, on a network where every server reaches every other server within **10 ms**. IPv4 is what has been tried.
- **Apelora needs three kinds of address ranges** besides the servers' own: a range the application networks' routers take on the uplink, a routed range for service addresses, and nothing else you have to plan: the networks inside applications are private and chosen by Incus. See [IP addresses](#ip-addresses).

## The servers

| What | Needed | Notes |
|---|---|---|
| **Number** | Three or more | Three servers keep the cluster database's majority when one fails. Two work, with no tolerance for a failure. Incus recommends at most about 50 servers per cluster, with at most 10 ms between them. |
| **Operating system** | A Linux distribution that provides the pinned Incus LTS series (`series` in `upstream.lock.yaml`) | The lab runs Debian 13. The daemon runs only on Linux. |
| **Kernel and tools** | The host minimums of the Incus series Apelora is pinned to: a recent Linux kernel, cgroup v2, QEMU (for virtual machines), LXC and nftables | The exact versions are recorded as `host_minimums` in `upstream.lock.yaml` (at the time of writing: kernel 6.12, QEMU 8.2, LXC 6.0, nftables 1.0). The enrolment script checks the kernel and Incus versions on a joining server. |
| **CPU** | x86-64 or ARM64, with hardware virtualisation if you run virtual machines | The lab is ARM64 only: nothing is proven for a cluster that mixes architectures. A module whose image supports one architecture needs a member of that architecture. |
| **Memory and disk** | Enough for what you will run | Lab figures (Lima VMs, idle Debian system containers, 2026-10-22): about **40 MiB RAM** and **15 MiB disk** per container as the kernel counts it, plus the OS and Incus. Not hardware, not busy modules, not VMs. The Studio's capacity check compares a design with what servers report free; use it. Details: [lab sizing](../design/capacity-lab-sizing.md). |
| **Local storage** | One or more disks for an Incus storage pool | Root disks and volumes live in pools. Apelora asks Incus to create, grow, snapshot and delete volumes in the pools it is told to use; it does not manage storage drivers. See [storage](../architecture/STORAGE.md). |
| **Shared storage** (optional) | A SAN reached over iSCSI, Fibre Channel or NVMe over Fabrics, with a clustered LVM pool | Only needed if modules with data should restart on another server after their server fails. Tried with an iSCSI target in the lab. |
| **Clocks** | Synchronised on every server (NTP or chrony) | Clock drift can make a member look offline or fail to join, and affects certificate validity and join-token expiry. |
| **Management controller** (optional) | A BMC with Redfish | Lets Apelora switch off a failed server to fence it. Tried against a test server only. |

Apelora itself (the control plane, the worker, PostgreSQL and the web server for the two screens) can run on a server of its own or on a cluster member. It is not in the data path: running instances keep running if it is down.

## Network layouts

Incus, OVN and Apelora use several kinds of traffic. Whether they share one network card or have their own is yours to decide; the table says what each carries and where it should be reachable from.

| Traffic | Protocol and port | Between | Where it should live |
|---|---|---|---|
| Incus API and cluster | TCP 8443, mutual TLS | the servers; the control plane and the worker to a server | A private management network |
| Cluster database and heartbeats | the same, TCP 8443 | the servers | The same network, at most 10 ms apart |
| Tunnels between application networks (OVN) | UDP 6081 (Geneve), not encrypted | the servers | A **private** network: nothing authenticates it on the wire |
| OVN databases | TCP 6641 (northbound) and 6642 (southbound), TLS | the servers; Incus to the northbound database | A private network. The default lab (`ovn.sh`) uses one database without TLS; `ovn-ha.sh APPLY=1` is the three-member TLS cluster (REAL INCUS TESTED 2026-10-22, including leader failover). A real installation should use TLS. |
| Application traffic to and from the outside | whatever your applications serve | the uplink and the outside | The **uplink network**: the one that is routed to your users |
| Volume copies and backups | through Incus (TCP 8443) | the servers | The management network |
| Shared storage (optional) | iSCSI TCP 3260, or Fibre Channel | each server and the SAN | A storage network of its own, if you have the cards |
| Fence probe (optional) | SSH, TCP 22 | the worker to each server | The management network |
| Apelora's database | PostgreSQL, TCP 5432 | the control plane and the worker | The management network |
| The two screens and the API | HTTPS, TCP 443 | people's browsers | Wherever your people are, behind your own TLS |
| Metrics (optional) | HTTPS to the API; the worker on `127.0.0.1:9471`; Incus metrics on `core.metrics_address` | your collector | A management network |

Open these between the servers, and from the machine that runs Apelora to the servers' port 8443, and nowhere else. Browsers never reach Incus, OVN, PostgreSQL or the SAN. The full list, with authentication, is in [connections and scale](../architecture/CONNECTIONS-AND-SCALE.md).

### One network card

Everything above shares the card. It is what the lab does, and it is enough to try Apelora on real hardware. The cost: a busy application can slow the cluster's own heartbeats, and the tunnels and the uplink share a network. One more thing to know: the application uplink must be a network Incus can attach OVN routers to. With a single physical card that means a **bridge** on that card that carries the server's own address; Incus's documentation requires the uplink's parent to be an unmanaged bridge or an unused physical interface. Apelora has not been tried on that layout (the lab uses a NAT bridge that Incus manages).

### Two network cards

The layout we recommend for a first real installation, **without having proven it**:

1. **Card one:** management and cluster: the servers' addresses, Incus's API and database, OVN's databases and tunnels, backups, the fence probe.
2. **Card two:** the uplink. Give it to Incus as an unused physical interface, or put it in a bridge, and use it as the OVN uplink.

### Three or more

Add a network of its own for **shared storage** (iSCSI or NVMe over Fabrics), and, if you want, separate the **tunnels** from management. Bonding, VLANs and jumbo frames are yours to arrange on the servers before Incus sees them; Apelora does not manage the physical network, and preflight only checks the uplink ranges it needs.

## IP addresses

| What | Which addresses | Notes |
|---|---|---|
| **The servers' own** | One **static** address each, on one network, public or private (RFC 1918), IPv4 | Use a specific address or a name that resolves: a cluster does not accept the wildcard address as its own. The same network must carry the cluster, so keep the servers within 10 ms. IPv6 and DNS names are Incus features that Apelora has not been tried with. |
| **Cluster address** | The same address, or a separate one if you give the cluster its own network (`cluster.https_address`) | Members reach each other on it. |
| **Uplink: DHCP and gateway** | The uplink network's own subnet and gateway | Whatever your network already gives the servers' neighbours. |
| **Uplink: addresses for application routers** | A range inside the uplink's subnet, not used by anything else (`ipv4.ovn.ranges`) | Each application network has an OVN router that takes one address from this range. Plan one per application. |
| **Service addresses** | An **IPv4** range **routed to the uplink**, for example a /28 (`ipv4.routes`; `APELORA_SERVICE_ADDRESSES`), or an **IPv6** range `/112`–`/126` (`ipv6.routes`) | A module that others connect to gets one address from this range, on an OVN load balancer; consumers use that address while replicas come and go. IPv6 `/64` cannot be enumerated, so the range must be `/112` or longer; there is no broadcast, so the last address is usable. Each module still gets one address (one family). Your network must route the range to the uplink; the OVN router does not answer ARP/ND for it. The lab takes its IPv4 range from the range reserved for network testing and an IPv6 ULA `/112` (`ovn.sh`; `APELORA_TEST_SERVICES_V6=` skips IPv6). IPv4 and IPv6 listen addresses are REAL INCUS TESTED. Service names (`APELORA_SERVICE_NAME_*`, `dns.domain`, `user.apelora.dns`) are REAL INCUS TESTED (`dns.sh`, 2026-10-23); OVN DNS still answers instance names only. Use addresses that are yours. |
| **Application networks** | Private addresses that Incus picks for each application's own network: an IPv4 subnet, and an IPv6 subnet of its own even when the uplink has no IPv6 | They are not routed outside, and you do not plan them. Connections are enforced over both: **REAL INCUS TESTED** (`scripts/test-cluster/ipv6.sh`, 2026-10-22: a connected module reached its provider over IPv4 and IPv6, an unconnected one was refused over both). |
| **Instance addresses** | Private, from the application network, IPv4 and IPv6 | Modules are told their providers' service addresses and names as settings (`APELORA_SERVICE_*`, `APELORA_SERVICE_NAME_*`). OVN DNS answers instance names; it does not answer the service names. |

**Public or private?** Both work for the servers. The addresses of the service range are the ones people or other systems connect to, so they follow your own rules: private addresses behind your firewall, or public addresses if you route them to the uplink. Apelora does not do network address translation for you.

**Without the uplink** Apelora still deploys, but it cannot enforce the connections you draw: instances then share the project's network and the dialog says so. Set `APELORA_NETWORK_UPLINK` and the service range to turn enforcement on.

## Checklist

- [ ] Three or more servers, a kernel that meets the host minimums, clocks synchronised.
- [ ] A static address on each, on a network with at most 10 ms between them.
- [ ] A card or VLAN for management, and (recommended) one for the uplink.
- [ ] A subnet and gateway for the uplink, a free range inside it for application routers, and a routed range for service addresses.
- [ ] The ports above open between the servers.
- [ ] A disk for each server's storage pool, and optionally a SAN.

Next: [install Incus and form the cluster](install-incus.md).
