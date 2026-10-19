# ADR 0002 — Bare-metal provisioning through MAAS

Status: **proposed** (decide at the start of Phase 09) · Date: 2026-10-20

## Context

Operators want to plug a new physical server into the network and have it become usable capacity: network boot (PXE/iPXE), hardware discovery, OS installation, remote power control, then enrolment into the Incus cluster. Writing DHCP/PXE services, hardware inventory, OS installers and BMC drivers ourselves is out of proportion to the project.

MAAS manages physical machines: rack controllers serve DHCP and network boot, commissioning collects hardware inventory (CPU, RAM, disks, NICs, BMC), deployment installs an OS and runs cloud-init, and power is controlled through Redfish, IPMI and vendor BMC interfaces. It has a documented REST API. It is licensed AGPL-3.0, compatible with our licence; we integrate over its API only.

## Proposal

Three management levels, each with one authority:

| Level | Authority | Responsibility |
|---|---|---|
| Bare metal | MAAS | Discovery, network boot, commissioning, OS install, power, provisioning addresses |
| Compute | Incus | Cluster membership, instances, storage and network objects, instance lifecycle |
| Application | Apelora | Appliances, graphs, connectivity, publishing, replicas, health, mobility |

- `crates/maas-adapter` talks to the MAAS API; `crates/node-enrollment` hands a freshly installed host to Incus: preflight (kernel, time sync, latency, storage, network), a single-use Incus join token delivered through cloud-init / a preseed, then validation before the member is eligible for placement.
- **Approval gates.** New hardware is discovered automatically but is never wiped, provisioned or admitted to a cluster without an administrator's approval. Powering off a host with running appliances first offers a planned evacuation; immediate power-off is a separately permissioned emergency action.
- **Network separation.** Provisioning (DHCP/PXE), management (Apelora and Incus APIs), storage and BMC traffic use separate networks or VLANs with access controls. MAAS DHCP never runs on a network that already has a DHCP server without coordination.
- **Address authority stays with its owner:** provisioning and host addresses with MAAS (or the site's IPAM), container addresses with Incus/OVN, service endpoints with Apelora's ingress controller. Apelora shows them all but does not become a second DHCP or IPAM.
- **Independence.** MAAS and the Apelora control plane run on management infrastructure that survives the loss of the compute cluster, so the cluster can be repaired or reprovisioned.
- Hardware without a BMC (mini PCs, workstations) may only support Wake-on-LAN or manual power; capabilities are recorded per hardware profile and verified, not assumed.

## Alternatives

- **A provisioning engine built on container-orchestrator custom resources** — capable, but brings an orchestrator dependency we excluded in ADR 0001.
- **Write our own PXE/DHCP/inventory/BMC stack** — rejected: large, security-sensitive, hardware-specific.
- **Manual OS installation plus scripted Incus join** — acceptable for the first lab clusters and the fallback when MAAS is not deployed; Phase 04 works without MAAS.

## Open questions

Minimum MAAS version and API stability guarantees; where MAAS runs in small (homelab) deployments; image/OS choice per hardware profile; how BMC credentials are stored (never in the repository, never in the browser).
