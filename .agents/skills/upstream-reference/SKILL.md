---
name: upstream-reference
description: What we know about the upstream projects Apelora uses, packages as modules, or learns from — role in Apelora, licence, interface, facts that feed the catalog and plans, practices we adopted, and each project's rules for AI-assisted contributions. Use before choosing a dependency, writing a module manifest, planning an integration, or contributing upstream; add an entry whenever the owner shares a new upstream repository or document.
---

# Upstream reference

Distilled knowledge from upstream repositories and documents the owner has shared. **Our own words only**: never paste or quote upstream files here or anywhere in the repository (AGENTS.md rule 9). Facts below were observed in the shared material on the date shown; verify versions and dates against the official source before relying on them in code or claims.

## How to maintain this skill

1. When the owner shares an upstream repo or document, add or update one entry: role, licence, how we use it, key facts, adopted practices, cautions. Note the date.
2. Record only what changes a decision. Skip build instructions for software we do not build.
3. Licence first: check compatibility with AGPL-3.0-only (`license-compliance`). Using something over its API or as a separately running program is different from linking or vendoring it.
4. Product names are fine for dependencies and catalog modules. Do not name products we only drew inspiration from; describe the concept instead.

## Platform and infrastructure

| Project | Role in Apelora | Licence | How we use it |
|---|---|---|---|
| **Incus** | Runtime and cluster engine (ADR 0001) | Apache-2.0 | REST API over mTLS. The host is the Incus package, or a whole-system image we do not install or manage. Never forked or vendored |
| **MAAS** | Proposed bare-metal provisioning (ADR 0002) | AGPL-3.0 | REST API only, if adopted |
| **Ceph** | Candidate shared storage for HA, via Incus | Mainly LGPL-2.1/3.0, some GPL headers | External system Incus talks to; never linked |
| **PostgreSQL** | Apelora state, jobs, leases | PostgreSQL licence | Via SQLx (Phase 03) |
| **OpenLDAP** (Alpine Linux package) | Development directory for directory sign-in (`infra/dev/directory`) | OpenLDAP Public License 2.8 (permissive); Alpine base image MIT | A separate program in a development container built from `alpine:3.24` and Alpine's maintained `openldap` packages; never linked or shipped |
| **Prometheus** | Example metrics store (`infra/observe`) | Apache-2.0 | Separate process; scrapes Apelora `/metrics` and (on members) Incus `/1.0/metrics` |
| **Loki** | Example log store | AGPL-3.0 | Separate process; Alloy tails JSON logs. Never linked |
| **Tempo** | Example trace store | AGPL-3.0 | Separate process; optional OTLP/HTTP from `APELORA_OTLP_ENDPOINT`. Never linked |
| **Pyroscope** | Example profile store | AGPL-3.0 | Separate process; empty until a profiler is added. Never linked |
| **Grafana** | Example metrics/logs/traces UI | AGPL-3.0 | Separate process; provisioned dashboards we wrote. Never linked |
| **Alloy** | Example log shipper | Apache-2.0 | Separate process; tails `APELORA_OBSERVE_LOG_DIR`. Never linked |

**Incus** (seen 2026-10): 7.0 LTS released May 2026, supported to June 2031 (2 years of fixes, then security only). Host minimums include kernel 6.12, cgroup v2, QEMU 8.2, LXC 6.0, nftables. Supports LXC system containers, OCI application containers and QEMU VMs; Raft-replicated cluster database; projects, profiles, images, storage pools (incl. Ceph, LINSTOR), networks (bridge, OVN with ACLs, DNS zones, forwards, load balancers), placement scriptlets (Starlark), evacuation/restore, metrics, OIDC and fine-grained authorisation, a low-level backup API and built-in basic S3. Live migration of ordinary LXC containers is limited; stopped moves are reliable. `cluster.healing_threshold` defaults to disabled. Local Unix-socket access equals root on the host.

**Incus load balancers while a member is offline** (seen 2026-10-25, v7.0.2, `internal/server/network/driver_ovn.go` `LoadBalancerUpdate`, `internal/server/cluster/notify.go`): after changing an OVN load balancer, Incus notifies every other member to refresh BGP prefixes with the `NotifyAll` policy, which fails with "peer node … is down" when any member is offline, and the change is reverted. Create and delete use the same policy. So no load balancer in the cluster can change while one member is offline: Apelora cannot move a service address's traffic to a substitute replica. OVN's health check does not reliably help either: on the lab the dead replica's check stayed online, then had no status, and OVN kept sending it traffic. Reproduced on the lab; v7.0.1 has the same check. Candidate upstream fix: use `NotifyAlive` for that BGP refresh (an offline member refreshes its prefixes when it returns). Report it upstream as a human (Incus policy below). Until then the operator removes a member that will not return soon; updates then succeed.

**IncusOS** (seen 2026-10-22): a whole operating system that boots Incus, not a package Apelora installs. Its management API is published as Apache-2.0 and is a separate API, usually reached through the Incus listener under an `/os` prefix. We do not call it, flash its images, enroll its boot keys, or store its secrets. The lab stays the Lima cluster. The upgrade assistant still records a package upgrade on a general-purpose host; on this OS the equivalent is an image update that keeps the previous image bootable, and we do not perform it.

What changes a decision:

- A client certificate has to be trusted at install time, before the first boot. That certificate stays in the operator's key store, as our Incus client certificates already do.
- The install expects UEFI Secure Boot and a TPM 2.0, and a system disk of at least 50 GiB. In a virtual machine the disk has to be presented as SCSI (VirtIO SCSI). A VirtIO block disk does not appear as a physical drive, so the installer cannot use it. A guest whose disks have no stable device id cannot be used. Clearing every Secure Boot key is convenient and lets the installer enroll its own, but it also drops keys some storage and network cards need in order to boot.
- The system drive is partitioned into an immutable pair of root images plus encrypted swap, an encrypted system partition, and an encrypted ZFS pool named `local` that takes the remaining space. Incus uses the dataset on that pool as its default storage. The system-drive slice cannot leave `local`, the pool cannot be deleted, and it can hold one or two drives only, as a stripe or a mirror. A mirror needs a second drive of the same size and gives up roughly 35 GiB to the system layout. Replacing a drive compares the device list in order. If the system drive of a mirror dies, a reinstall can recover the pool only when the operator supplies the pool key. That key, and the install-drive recovery passphrase, are operator secrets. Apelora does not retrieve, log, or store them. `local` is not shared storage: a module on it is not the shared-pool restart case.
- The first cluster member is installed with the default networks and pools. Every member that joins is installed without them, because a joiner cannot already have networks or pools. Clustering is enabled from a client after the cluster address is set. There is no host-local init command on this OS. Joiners use the dataset Incus already created on `local`.
- It can stand up its own storage cluster from container images on three or more members, encrypting raw disks before handing them to the daemons, on a private network with IPv6. That remains a separate storage system. We do not drive it. Its status can include key material, which is another reason not to proxy that API into the console or the audit log.
- When the main application fails, a fallback listener may open on every interface on a port chosen at boot, and its trusted certificates are not kept in sync after first boot. A recovery stick can run a signed repair. Debug endpoints have no stability promise. None of these are an Apelora interface. Powering off a failed member stays on the BMC or MAAS path; asking this OS to power itself off is not a fence.

**MAAS** (seen 2026-10-22): region and rack controllers; DHCP, TFTP/HTTP network boot, commissioning (hardware inventory and tests, not the OS install), OS deployment with a short series name and cloud-init, then release back to the pool; power through BMCs (Redfish, IPMI, vendor interfaces) or a webhook when the machine has no controller. REST API only. We list machines and call commission, deploy, release and power-off. Release sends `erase=false` unless the person asked to erase the disks; a secure wipe is not offered. We do not install the controllers, sync images, run its DHCP, DNS or time service, set storage layouts, or manage zones, pools and tags.

Images live in a stream (stable for production, candidate only when a newer series is required). The region syncs them; rack controllers cache them for deployment. The series has to be selected and present before a deploy can finish. A deploy request can be accepted before the image exists. An air-gapped site needs its own image mirror; we do not build one.

A machine has one zone and one resource pool. A zone is a label until some other system spreads work across zones; it is not an Incus failure domain. A pool is the access-control bucket (who may use those machines); it is not an Apelora role. Tags are many persistent labels, used to filter commissioning and deployment, and can carry kernel options that apply at the next boot. A note is a durable description. Owner-data is key-value metadata that exists only while the machine is allocated or deployed and disappears on release, so it is not a place to keep a join token. We show zone and pool when the machine list already has them. We do not create, assign or delete any of these.

A newly added machine can start commissioning on its own. Release sends disk erasure only when the caller sets it; a secure wipe is not offered. Never run its DHCP on a network that already has one: a second server that does not point machines at the boot service leaves them with an address and no install. A guest composed on a virtualization host can commission with no BMC. Do not adopt a published all-in-one walkthrough: it uses a well-known administrator password and its own network. Our disposable lab stays `scripts/test-cluster/maas.sh` (Lima). Its own development uses Python and Go; supports FIPS/STIG-hardened environments.

**Ceph** (seen 2026-10): object, block (RBD) and file storage; needs its own cluster, monitoring and repair runbooks. A storage cluster, not a feature of Incus.

**OpenLDAP dev image** (chosen 2026-10): the popular prebuilt OpenLDAP images are either unmaintained for years or no longer freely published, so we build a small image from Alpine's maintained packages (2.6.x). Facts that mattered: the `memberof` overlay does not run for `slapadd` (seed `memberOf` explicitly or load online); `security simple_bind=64` refuses simple binds without TLS (result 13); `entryUUID` is operational and must be requested by name; Alpine's `slappasswd` produces salted SHA-1 by default.

### General infrastructure concepts (no single source)

- Separate networks by role: provisioning, management, storage, out-of-band power (BMC); VLANs or equivalent with access control.
- Give each cluster its own private address block so several clusters can share a backbone; reject public ranges that overlap reserved private ones.
- Pools of public addresses split by use (hardware vs. applications).
- Generated MAC addresses use the IEEE locally administered bit; derive them deterministically from our own ids with our own scheme.
- On multi-homed hosts, answer ARP only on the receiving interface (Linux `arp_ignore`).
- Import new platform versions/hotfixes into a controlled store before rolling them out.

## Catalog modules (facts that shape manifests)

| Module | Licence | Key concepts for settings and ports |
|---|---|---|
| **NGINX** | BSD-2-Clause | Master/worker processes (one worker per CPU); stable vs mainline branches; reverse proxy and load balancer to upstream pools; TLS, HTTP/2; rate limiting; content cache; static and dynamic modules |
| **HAProxy** | GPL-2.0+ (LGPL-2.1 headers) | Layer-7 (http) or layer-4 (tcp) proxying; health checks; sticky sessions; timeouts; TLS termination; response compression; stats/metrics endpoint; even-numbered spring releases are long-term supported, odd-numbered autumn releases are short-lived |
| **Apache httpd** | Apache-2.0 | Processing models (event, worker, prefork); modules; reverse proxy; TLS and HTTP/2 |
| **Keycloak** | Apache-2.0 | Realms, SSO (OIDC, SAML), user federation (LDAP), MFA, fine-grained authorisation; development vs production start mode; needs a database; hostname and proxy-header settings matter behind a proxy |
| **InfluxDB** | MIT or Apache-2.0 | Version 3 Core: time-series database, Parquet files on local disk or object storage, SQL and InfluxQL queries, compatible 1.x/2.x write APIs, embedded Python processing engine, HTTP API on port 8181 |

Running a module image does not link its code into ours; record the licence in the manifest's provenance anyway.

### Catalog additions (checked 2026-10-25, design-only)

Licences from each project's own repository or documentation, for the 59 classes added with the two-level taxonomy. All are open-source licences unless the caution says otherwise.

| Area | Modules and licences |
|---|---|
| Routing, DNS, DHCP | FRRouting GPL-2.0-or-later; BIRD GPL-2.0-or-later; Keepalived GPL-2.0-or-later; CoreDNS Apache-2.0; PowerDNS Authoritative GPL-2.0-only; Unbound BSD-3-Clause; BIND 9 MPL-2.0; Kea MPL-2.0; NetBox Apache-2.0 |
| VPN, firewall | strongSwan GPL-2.0-or-later; Headscale BSD-3-Clause; OpenWrt GPL-2.0-only (its metrics package Apache-2.0) |
| Runtimes, systems | Go BSD-3-Clause; openSUSE (distribution, mixed open licences) |
| Data | YugabyteDB Apache-2.0 (the database only; its management product is not open source); PgBouncer ISC; Valkey BSD-3-Clause; FerretDB Apache-2.0; Meilisearch MIT for the Community Edition (enterprise parts BUSL-1.1); Trino Apache-2.0; Superset Apache-2.0; Garage AGPL-3.0; SeaweedFS Apache-2.0; Samba GPL-3.0-or-later |
| Messaging | Debezium Apache-2.0; Flink Apache-2.0 |
| Observability | VictoriaMetrics Apache-2.0; node and blackbox exporters Apache-2.0; Zabbix AGPL-3.0-only (7.0 and later); Loki AGPL-3.0-only (some folders Apache-2.0); Vector MPL-2.0; Fluent Bit Apache-2.0; Tempo AGPL-3.0; OpenSearch Dashboards Apache-2.0; Alertmanager Apache-2.0; Uptime Kuma MIT |
| Security | FreeIPA GPL-3.0-or-later; OpenBao MPL-2.0; step-ca Apache-2.0; Vaultwarden AGPL-3.0-only; CrowdSec MIT; Suricata GPL-2.0-only; Wazuh mostly GPL-2.0-only, newer engine code AGPL-3.0-or-later (differs by version) |
| Developer tools | Forgejo GPL-3.0-or-later from v9 (MIT before); Woodpecker Apache-2.0; Harbor Apache-2.0; Distribution (registry) Apache-2.0 |
| Applications | Mattermost source AGPL-3.0 with Apache-2.0 parts, binaries MIT; Synapse AGPL-3.0-or-later (or a commercial licence); Jitsi Meet Apache-2.0; Ghost MIT; Drupal GPL-2.0-or-later; MediaWiki GPL-2.0-or-later; Postfix EPL-2.0 or IPL-1.0; Dovecot MIT and LGPL-2.1; Roundcube GPL-3.0-or-later; Odoo Community LGPL-3.0-only; Matomo GPL-3.0-or-later |

Facts that shaped these manifests: Mattermost's metrics endpoint needs a commercial licence (the free edition has no `mon` output); Meilisearch's metrics are experimental; Kea has no built-in metrics text (its project's monitoring agent exports them); BIND's statistics channel answers JSON/XML, not metrics text; Uptime Kuma's metrics depend on the version (newer ones add an off switch); step-ca's metrics listener is in the source but not in its documentation. Already in the catalog with a source-available licence: Vault and Consul (BUSL-1.1) and recent ScyllaDB releases; CockroachDB, ArangoDB and MongoDB are believed to be source-available too (not rechecked on this date). OpenBao (MPL-2.0) and Valkey (BSD-3-Clause) are the open alternatives to Vault and Redis; FerretDB the open MongoDB-protocol one.

### Metrics facts that shaped the catalog (researched 2026-10)

Full per-class data: `packages/module-catalog/src/metrics.ts` (with confidence). Separate exporters and agents are described generically, never by project name.

- **Incus** serves per-member metrics text at `/1.0/metrics` (HTTPS, a client certificate of type `metrics`, or a metrics-only listener via `core.metrics_address`); instance values are cached about 8 s and each member reports only its own instances, so every member must be read. For VMs the values come from the agent inside the guest; without it, QEMU-level data only (memory estimated from the host, no filesystems or processes). Network state carries interface counters; load-balancer state carries backend health. **OVN** has no HTTP metrics endpoint (statistics only through its control socket and southbound database) and no counters for refused traffic.
- **Moved or split listeners:** Keycloak 25+ serves metrics and health only on the management port 9000. Vault answers metrics only on its active node (standbys redirect; health 200/429/503 for active/standby/sealed). Etcd's client listener, Logstash's API, the OTel Collector's own telemetry, Neo4j's endpoint, Pocket ID and Vault bind to localhost by default; Nextcloud 33+ and GitLab allow only localhost until an allow-list is set.
- **No built-in endpoint:** PostgreSQL, MySQL/MariaDB, Redis, MongoDB, Memcached, Elasticsearch, Varnish, WireGuard, OpenVPN and Windows (performance counters) need a separate exporter or are read locally; JVM software (Tomcat, Kafka, Cassandra, ActiveMQ, plain Java) exposes JMX and needs an in-JVM metrics agent whose port is chosen in its argument. NGINX and Apache httpd have status pages, not metrics text. PHP-FPM status is reachable only over FastCGI (never expose that port). Airflow pushes instead of serving.
- **Built in, on a port of its own:** ScyllaDB 9180, CockroachDB 8080, Authentik 9300, Authelia 9959 (off by default), ClickHouse 9363 (off), RabbitMQ 15692 (plugin; on in official images only), Ceph manager 9283 (active manager only), Jaeger v2 and OTel Collector 8888, Traefik on its internal entry point 8080. On the service port: Grafana, Prometheus, Gitea (off, optional token), Pulsar, CouchDB (admin), ArangoDB, MinIO (bearer token), InfluxDB (2.7 on 8086, 3.0 on 8181 with a token), Consul (token, retention must be above 0), Solr 9.8.
- **FreeBSD-based firewalls** (OPNsense, pfSense) run only as VMs; pfSense offers SNMP (UDP 161, community string in clear), OPNsense an optional node metrics plugin on 9100.
- **Licensing points found on the way:** MinIO's community repository is archived; ScyllaDB recent releases and Consul/Vault use source-available licences.
- **Endpoints without authentication** (most of them) must stay on the application network; Apelora opens a metrics port only to the collector a drawn `mon` connection names.

## Toolchain and practices adopted

| Project | Licence | What we took (as practice, not code) |
|---|---|---|
| **Vite** | MIT | Already our build tool; it uses pnpm itself, we keep npm workspaces by decision |
| **TypeScript** | Apache-2.0 | Already our language. Version 7 is the native (Go) compiler; as of 2026-10 typescript-eslint 8.71 only supports TypeScript < 7, so Dependabot ignores TypeScript majors until it does (CI caught this on PR #2) |
| **Cargo** | MIT or Apache-2.0 | `cargo deny` for licence allow-list and advisories (Phase 10) |
| **rustfmt** | MIT or Apache-2.0 | `rustfmt.toml` with `edition` and `style_edition` so editors and `cargo fmt` agree; `--check` in CI |
| **docs.rs** | MIT | Same stack (Axum, SQLx, PostgreSQL): local PostgreSQL via compose on loopback, SQLx offline metadata committed, parallel test runner (Phase 03) |
| **rust-analyzer** | MIT or Apache-2.0 | One rules file shared by agents; working habits now in AGENTS.md ("How to change things") |
| **ldap3** (Rust) | MIT or Apache-2.0 | LDAP client for directory sign-in (0.12, `tls-rustls-aws-lc-rs` only, no default features): rustls 0.23 and tokio-rustls 0.26 as already used by the Incus adapter. Its rustls connector verifies the certificate against the `ClientConfig` we pass and the URL's host name; `SearchEntry::construct` panics on malformed replies, so we call it under `catch_unwind` |
| **regex** (Rust) | MIT or Apache-2.0 | Linear-time matching: use it if we ever match patterns from untrusted input (avoids ReDoS) |

Repository-level results so far: `rust-toolchain.toml` pin, `rustfmt.toml`, `--locked` builds in CI, `cargo deny` planned.

## Contributing upstream (AI policies seen)

If we ever send changes upstream, the human owner submits them and must understand them. Observed rules:

- **Incus:** contributions must be Apache-2.0 compatible (none of our AGPL code); human DCO sign-off; AI tools may not be credited; do not run AI agents on machines that hold its repository credentials.
- **TypeScript:** disclose AI use in the pull request; no bulk or queue-driven agent pull requests.
- **rust-analyzer, regex:** no autonomous agents opening issues or pull requests; disclose AI use; comments and replies written by humans.
- **Vite:** contributors write comments and descriptions in their own words and must understand the change.
- **Cargo:** follows the Rust project's LLM usage policy.

Never open upstream issues or pull requests from an agent session.
