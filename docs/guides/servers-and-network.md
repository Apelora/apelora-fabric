# Servers and network

This page says what the servers need, how many network cards to give them, and which IP addresses Apelora Fabric and Incus use. Read it before you [install Incus](install-incus.md).

## The short answers

- **One network card is enough to start**, for a lab or a small evaluation: everything shares it. For anything you depend on, use **two or more**. One carries management and cluster traffic; another carries the networks your applications use to reach the outside. Details in [network layouts](#network-layouts).
- **Use static IPv4 addresses**, private or public, on a network where every server reaches every other server within **10 ms**.
- **Plan two address ranges** besides the servers' own: a range the application networks' routers take on the uplink, and a routed range for service addresses. The networks inside applications are private, and Incus chooses them. See [IP addresses](#ip-addresses).

## The servers

| What | Needed | Notes |
|---|---|---|
| **Number** | Three or more | Three servers keep the cluster database's majority when one fails. Two work, with no tolerance for a failure. Incus recommends at most about 50 servers per cluster, with at most 10 ms between them. |
| **Operating system** | A Linux distribution that provides the pinned Incus long-term support (LTS) series (`series` in `upstream.lock.yaml`) | Debian 13 works. The Incus daemon runs only on Linux. |
| **Kernel and tools** | The host minimums of the Incus series Apelora is pinned to: a recent Linux kernel, cgroup v2, QEMU (for virtual machines), LXC and nftables | The exact versions are `host_minimums` in `upstream.lock.yaml` (at the time of writing: kernel 6.12, QEMU 8.2, LXC 6.0, nftables 1.0). The enrolment script checks the kernel and Incus versions on a joining server. |
| **CPU** | x86-64 or ARM64, with hardware virtualisation if you run virtual machines | A module whose image supports one architecture needs a member of that architecture. |
| **Memory and disk** | Enough for what you will run | As a floor, an idle Debian system container uses about **40 MiB of memory** and **15 MiB of disk**, plus the operating system and Incus. Busy modules and virtual machines use much more. The Studio's capacity check compares a design with what the servers report free; use it. Details: [lab sizing](../design/capacity-lab-sizing.md). |
| **Local storage** | One or more disks for an Incus storage pool | Root disks and volumes live in pools. Apelora asks Incus to create, grow, snapshot and delete volumes in the pools it is told to use. It does not manage storage drivers. See [storage](../architecture/STORAGE.md). |
| **Shared storage** (optional) | A storage area network (SAN) reached over iSCSI, Fibre Channel or NVMe over Fabrics, with a clustered LVM pool | Needed only if modules with data should restart on another server after their server fails. |
| **Clocks** | Synchronised on every server (NTP or chrony) | Clock drift can make a member look offline or fail to join. It also affects certificate validity and join-token expiry. |
| **Management controller** (optional) | A baseboard management controller (BMC) with Redfish | Lets Apelora switch off a failed server to fence it. |

Apelora itself (the control plane, the worker, PostgreSQL and the web server for the two screens) can run on a server of its own or on a cluster member. It is not in the data path: running instances keep running if it is down.

## Network layouts

Incus, OVN (Open Virtual Network) and Apelora use several kinds of traffic. You decide whether they share one network card or each have their own. The table says what each carries and where it should be reachable from.

| Traffic | Protocol and port | Between | Where it should live |
|---|---|---|---|
| Incus API and cluster | TCP 8443, mutual TLS | the servers; the control plane and the worker to a server | A private management network |
| Cluster database and heartbeats | the same, TCP 8443 | the servers | The same network, at most 10 ms apart |
| Tunnels between application networks (OVN) | UDP 6081 (Geneve), not encrypted | the servers | A **private** network: nothing authenticates it on the wire |
| OVN databases | TCP 6641 (northbound) and 6642 (southbound), TLS | the servers; Incus to the northbound database | A private network, with TLS on the database connections |
| Application traffic to and from the outside | whatever your applications serve | the uplink and the outside | The **uplink network**: the one that is routed to your users |
| Volume copies and backups | through Incus (TCP 8443) | the servers | The management network |
| Shared storage (optional) | iSCSI TCP 3260, or Fibre Channel | each server and the SAN | A storage network of its own, if you have the cards |
| Fence probe (optional) | SSH, TCP 22 | the worker to each server | The management network |
| Apelora's database | PostgreSQL, TCP 5432 | the control plane and the worker | The management network |
| The two screens and the API | HTTPS, TCP 443 | people's browsers | Wherever your people are, behind your own TLS |
| Metrics (optional) | HTTPS to the API; the worker on `127.0.0.1:9471`; Incus metrics on `core.metrics_address` | your collector | A management network |

Open these ports between the servers, and from the machine that runs Apelora to the servers' port 8443, and nowhere else. Browsers never reach Incus, OVN, PostgreSQL or the SAN. [Connections and scale](../architecture/CONNECTIONS-AND-SCALE.md) has the full list, with how each connection is authenticated.

### One network card

Everything above shares the card. This is enough to try Apelora on real hardware.

- **The cost:** a busy application can slow the cluster's own heartbeats, and the tunnels and the uplink share a network.
- **The uplink:** it must be a network Incus can attach OVN routers to. With a single physical card, that means a **bridge** on that card that carries the server's own address. Incus's documentation requires the uplink's parent to be an unmanaged bridge or an unused physical interface.

### Two network cards

The layout we recommend for a first real installation:

1. **Card one:** management and cluster: the servers' addresses, Incus's API and database, OVN's databases and tunnels, backups, the fence probe.
2. **Card two:** the uplink. Give it to Incus as an unused physical interface, or put it in a bridge, and use it as the OVN uplink.

### Three or more

Add a network of its own for **shared storage** (iSCSI or NVMe over Fabrics). If you want, also separate the **tunnels** from management.

Bonding, VLANs and jumbo frames are yours to set up on the servers before Incus sees them. Apelora does not manage the physical network; preflight only checks the uplink ranges it needs.

## IP addresses

| What | Which addresses | Notes |
|---|---|---|
| **The servers' own** | One **static** IPv4 address each, on one network, public or private (RFC 1918) | Use a specific address or a name that resolves: a cluster does not accept the wildcard address as its own. The same network carries the cluster, so keep the servers within 10 ms of each other. |
| **Cluster address** | The same address, or a separate one if you give the cluster its own network (`cluster.https_address`) | Members reach each other on it. |
| **Uplink: DHCP and gateway** | The uplink network's own subnet and gateway | Whatever your network already gives the servers' neighbours. |
| **Uplink: addresses for application routers** | A range inside the uplink's subnet, not used by anything else (`ipv4.ovn.ranges`) | Each application network has an OVN router that takes one address from this range. Plan one per application. |
| **Service addresses** | An **IPv4** range **routed to the uplink**, for example a /28 (`ipv4.routes`; `APELORA_SERVICE_ADDRESSES`), or an **IPv6** range `/112`–`/126` (`ipv6.routes`) | A module that others connect to gets one address from this range, on an OVN load balancer. Consumers use that address while replicas come and go. Each module gets one address, in one family. An IPv6 `/64` cannot be enumerated, so the range must be `/112` or longer; IPv6 has no broadcast, so the last address is usable. Your network must route the range to the uplink: the OVN router does not answer ARP or neighbour discovery for it. A design that includes the resolver module also answers the service names; without it, OVN's DNS answers instance names only. The address in the environment stays the contract. Use addresses that are yours. |
| **Application networks** | Private addresses that Incus picks for each application's own network: an IPv4 subnet, and an IPv6 subnet of its own even when the uplink has no IPv6 | They are not routed outside, and you do not plan them. Connections are enforced over both IPv4 and IPv6. |
| **Instance addresses** | Private, from the application network, IPv4 and IPv6 | Modules receive their providers' service addresses and names as settings (`APELORA_SERVICE_*`, `APELORA_SERVICE_NAME_*`). OVN's DNS answers instance names. The resolver module, when the design includes it, answers the service names and forwards the rest. |

**Public or private?** Both work for the servers. People and other systems connect to the addresses of the service range, so those follow your own rules: private addresses behind your firewall, or public addresses that you route to the uplink. Apelora does not do network address translation for you.

**Without the uplink**, Apelora still deploys, but it cannot enforce the connections you draw: instances then share the project's network, and the deploy dialog says so. Set `APELORA_NETWORK_UPLINK` and the service range to turn enforcement on.

## Checklist

- [ ] Three or more servers, a kernel that meets the host minimums, clocks synchronised.
- [ ] A static address on each, on a network with at most 10 ms between them.
- [ ] A card or VLAN for management, and (recommended) one for the uplink.
- [ ] A subnet and gateway for the uplink, a free range inside it for application routers, and a routed range for service addresses.
- [ ] The ports above open between the servers.
- [ ] A disk for each server's storage pool, and optionally a SAN.

Next: [install Incus and form the cluster](install-incus.md).

## Before you rely on it

- Apelora has run on a lab of virtual machines with one network card each, a private network between them, and a network bridge that Incus manages as the uplink. Physical servers, a bridge on a single physical card, separate management and storage networks, and physical uplinks have not been tried yet. The two-card layout is a recommendation. Plan a trial on your own servers before production.
- IPv6 addresses and DNS names for the servers themselves have not been tried. IPv6 inside application networks and for service addresses works.
- Clusters that mix x86-64 and ARM64 servers have not been tried.
- Shared storage has been tried with an iSCSI target only. Fencing through a BMC has been tried against test servers only, not real hardware.
