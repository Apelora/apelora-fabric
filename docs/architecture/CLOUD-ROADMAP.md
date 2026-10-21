# Roadmap: from a three-server fabric to a full cloud

Status: **direction** · Date: 2026-10-21 · Scale design: [global fabric](GLOBAL-FABRIC.md), [one design at every size](EVERY-SIZE.md) · Today: [current state](current-state.md)

Apelora starts small (three servers, one sector) and aims to be a complete cloud and virtualisation platform: everything an organisation needs to run its applications on its own servers, anywhere, with the services people expect from large public clouds and established virtualisation suites. This page lists the capabilities that takes, layer by layer, what exists, and the order to build the rest. It is a direction, not a promise: each item gets a design note and an ADR when its turn comes.

## Principles that stay true at every size

1. **Upstream does the heavy lifting.** Incus runs compute, storage pools and OVN networks. Apelora adds applications, policy, automation and the experience, and never forks or replaces what it builds on (ADR 0001).
2. **Everything is an API first.** The Console and the Studio use the same public API that automation and infrastructure-as-code tools use.
3. **Every service is a module.** Managed databases, caches, queues, load balancers and gateways are module classes with typed ports, deployed and operated by the same machinery as any application.
4. **Isolation by design.** Organisations, projects and environments map to Incus projects with limits; networks deny by default; every action is authorised and audited.
5. **Scale by repetition.** Sectors, sites and regions are copies of the same unit (ADR 0008, ADR 0009).
6. **Runs without the internet.** Every feature works in air-gapped and disconnected installations: images from local mirrors, sign-in inside, offline release bundles, no call home (ADR 0010).

## Capabilities

Status: ✅ built · 🟡 partly · ⬜ planned.

| Area | Capability | Status |
|---|---|---|
| **Compute** | System containers, OCI containers, VMs per module class | ✅ |
| | Replicas, health checks, restart and replacement of failed instances | ✅ |
| | Live migration of VMs; planned moves; evacuation | ✅ |
| | Restart on another server after a failure (shared storage, fencing) | 🟡 lab pool ready (HA slice B) |
| | GPUs and other devices per module class; SR-IOV and offloaded NICs | ⬜ |
| | Bare-metal servers as a resource (install, power, re-image) | ⬜ ADR 0002 |
| | Autoscaling replicas from metrics | ⬜ |
| **Storage** | Local volumes, grow, snapshots, schedules, backups | ✅ |
| | Shared block storage (clustered LVM on a SAN) | 🟡 lab |
| | Distributed block storage across servers | ⬜ |
| | File shares for modules (network file systems) | ⬜ |
| | Object storage as a service (a module class) | ⬜ |
| | Backup tiers across sites and regions; restore drills; immutable archives | ⬜ GLOBAL-FABRIC §5 |
| **Networking** | Private network per application, deny-by-default rules from the design, service addresses, published addresses | ✅ |
| | Load balancing across replicas | ✅ |
| | Routed addresses with BGP; anycast; hardware offload | ⬜ GLOBAL-FABRIC §3 |
| | Peering between applications and sectors; private links between regions | ⬜ |
| | Managed DNS for applications; certificates issued and renewed automatically | ⬜ |
| | Gateways to on-premises networks (site-to-site VPN) | ⬜ |
| | Web application firewall and DDoS protection at the edge | ⬜ |
| **Identity and security** | Identity providers, local accounts, directories, API tokens, service identities | ✅ / 🟡 local accounts and directory in progress |
| | Roles, scopes, groups, approvals, emergency access, audit | ✅ |
| | Multi-factor for local accounts | ⬜ |
| | Secrets service for applications (values never in designs) | ⬜ |
| | Key management and encryption at rest per tenant | ⬜ |
| | Compliance reports (who can do what, what changed) | ⬜ |
| **Applications** | Visual design, versions, publish, deploy, start/stop/restart | ✅ |
| | Module catalog with logos and typed ports | ✅ |
| | Managed services as modules (databases, caches, queues) with backups and upgrades | ⬜ |
| | A catalog of third-party modules, reviewed and signed | ⬜ |
| | Blue/green and canary deployments | ⬜ |
| **Operations** | Health, capacity, operations, audit, preflight, membership | ✅ / 🟡 health in progress |
| | Alerts with rules, notification channels and acknowledgement | ⬜ |
| | Metrics and logs per application, retained per region | ⬜ |
| | Terminal and PowerShell sessions into modules | ✅ Studio; ⬜ Console |
| | Support bundles and request tracing across planes | 🟡 request ids built |
| **Tenancy and business** | Organisations, projects, environments | ✅ |
| | Quotas per organisation, project and environment; a restricted Incus project per environment (ADR 0012) | ✅ |
| | Metering of usage (CPU, memory, storage, network) | ⬜ |
| | Pricing, showback and chargeback; invoices | ⬜ |
| | Self-service sign-up for tenants (for service providers) | ⬜ |
| **Ecosystem** | Command-line tool and an infrastructure-as-code provider on the public API | ⬜ |
| | Software development kits generated from the OpenAPI document | 🟡 TypeScript client |
| | Importing workloads from other virtualisation platforms (disk images, VM definitions) | ⬜ |
| | Partner program: certified hardware, modules and integrations | ⬜ |
| **Disconnected** | Image mirrors, mirrors-only mode | ✅ |
| | Preflight and Console page for mirrors; signed offline release bundles and update runbook | ✅ not yet run on an installation without internet |
| | Blocked-egress lab profile; real-Incus tests passing with egress blocked | 🟡 profile built (`egress.sh`), not yet run |
| **Scale** | One sector | ✅ |
| | Sectors, sites, regions, global plane | ⬜ ADR 0008, ADR 0009 |

## Order of work

Each step leaves a product someone can run.

1. **A dependable small cloud (1–3 sectors).**
   - Finish sign-in methods, health and capacity.
   - Restart after failure on shared storage.
   - Alerts.
   - A Console terminal.
   - Quotas.
   - Command-line tool.
2. **A data-centre cloud (many sectors in one site).**
   - Sectors.
   - Routed underlay with BGP.
   - Moves between sectors.
   - Backup tier 1 across sectors.
   - Metering.
   - Managed database and cache modules.
   - Infrastructure-as-code provider.
3. **A national cloud (several sites).**
   - Regions and sites.
   - Residency policies.
   - Backup tier 2.
   - Anycast.
   - Managed DNS and certificates.
   - Object storage module.
   - Showback.
4. **A global cloud.**
   - The global plane and regional planes.
   - Static stability.
   - Global traffic steering.
   - Tier-3 archives.
   - Third-party module catalog.
   - Tenant self-service.
   - Billing.
   - Workload import.
