---
name: upstream-reference
description: What we know about the upstream projects Apelora uses, packages as appliances, or learns from — role in Apelora, licence, interface, facts that feed the catalog and plans, practices we adopted, and each project's rules for AI-assisted contributions. Use before choosing a dependency, writing an appliance manifest, planning an integration, or contributing upstream; add an entry whenever the owner shares a new upstream repository or document.
---

# Upstream reference

Distilled knowledge from upstream repositories and documents the owner has shared. **Our own words only**: never paste or quote upstream files here or anywhere in the repository (AGENTS.md rule 9). Facts below were observed in the shared material on the date shown; verify versions and dates against the official source before relying on them in code or claims.

## How to maintain this skill

1. When the owner shares an upstream repo or document, add or update one entry: role, licence, how we use it, key facts, adopted practices, cautions. Note the date.
2. Record only what changes a decision. Skip build instructions for software we do not build.
3. Licence first: check compatibility with AGPL-3.0-only (`license-compliance`). Using something over its API or as a separately running program is different from linking or vendoring it.
4. Product names are fine for dependencies and catalog appliances. Do not name products we only drew inspiration from; describe the concept instead.

## Platform and infrastructure

| Project | Role in Apelora | Licence | How we use it |
|---|---|---|---|
| **Incus** | Runtime and cluster engine (ADR 0001) | Apache-2.0 | REST API over mTLS; installed as a host package; never forked or vendored |
| **MAAS** | Proposed bare-metal provisioning (ADR 0002) | AGPL-3.0 | REST API only, if adopted |
| **Ceph** | Candidate shared storage for HA, via Incus | Mainly LGPL-2.1/3.0, some GPL headers | External system Incus talks to; never linked |
| **PostgreSQL** | Apelora state, jobs, leases | PostgreSQL licence | Via SQLx (Phase 03) |

**Incus** (seen 2026-10): 7.0 LTS released May 2026, supported to June 2031 (2 years of fixes, then security only). Host minimums include kernel 6.12, cgroup v2, QEMU 8.2, LXC 6.0, nftables. Supports LXC system containers, OCI application containers and QEMU VMs; Raft-replicated cluster database; projects, profiles, images, storage pools (incl. Ceph, LINSTOR), networks (bridge, OVN with ACLs, DNS zones, forwards, load balancers), placement scriptlets (Starlark), evacuation/restore, metrics, OIDC and fine-grained authorisation, a low-level backup API and built-in basic S3. Live migration of ordinary LXC containers is limited; stopped moves are reliable. `cluster.healing_threshold` defaults to disabled. Local Unix-socket access equals root on the host.

**MAAS** (seen 2026-10): region and rack controllers; DHCP, TFTP/HTTP network boot, commissioning (hardware inventory), OS deployment with cloud-init, power control through BMCs (Redfish, IPMI, vendor interfaces); REST API. Its own development uses Python and Go; supports FIPS/STIG-hardened environments. Never run its DHCP on a network that already has one.

**Ceph** (seen 2026-10): object, block (RBD) and file storage; needs its own cluster, monitoring and repair runbooks. A storage cluster, not a feature of Incus.

### General infrastructure concepts (no single source)

- Separate networks by role: provisioning, management, storage, out-of-band power (BMC); VLANs or equivalent with access control.
- Give each cluster its own private address block so several clusters can share a backbone; reject public ranges that overlap reserved private ones.
- Pools of public addresses split by use (hardware vs. applications).
- Generated MAC addresses use the IEEE locally administered bit; derive them deterministically from our own ids with our own scheme.
- On multi-homed hosts, answer ARP only on the receiving interface (Linux `arp_ignore`).
- Import new platform versions/hotfixes into a controlled store before rolling them out.

## Catalog appliances (facts that shape manifests)

| Appliance | Licence | Key concepts for settings and ports |
|---|---|---|
| **NGINX** | BSD-2-Clause | Master/worker processes (one worker per CPU); stable vs mainline branches; reverse proxy and load balancer to upstream pools; TLS, HTTP/2; rate limiting; content cache; static and dynamic modules |
| **HAProxy** | GPL-2.0+ (LGPL-2.1 headers) | Layer-7 (http) or layer-4 (tcp) proxying; health checks; sticky sessions; timeouts; TLS termination; response compression; stats/metrics endpoint; even-numbered spring releases are long-term supported, odd-numbered autumn releases are short-lived |
| **Apache httpd** | Apache-2.0 | Processing models (event, worker, prefork); modules; reverse proxy; TLS and HTTP/2 |
| **Keycloak** | Apache-2.0 | Realms, SSO (OIDC, SAML), user federation (LDAP), MFA, fine-grained authorisation; development vs production start mode; needs a database; hostname and proxy-header settings matter behind a proxy |
| **InfluxDB** | MIT or Apache-2.0 | Version 3 Core: time-series database, Parquet files on local disk or object storage, SQL and InfluxQL queries, compatible 1.x/2.x write APIs, embedded Python processing engine, HTTP API on port 8181 |

Running an appliance image does not link its code into ours; record the licence in the manifest's provenance anyway.

## Toolchain and practices adopted

| Project | Licence | What we took (as practice, not code) |
|---|---|---|
| **Vite** | MIT | Already our build tool; it uses pnpm itself, we keep npm workspaces by decision |
| **TypeScript** | Apache-2.0 | Already our language. Version 7 is the native (Go) compiler; as of 2026-10 typescript-eslint 8.71 only supports TypeScript < 7, so Dependabot ignores TypeScript majors until it does (CI caught this on PR #2) |
| **Cargo** | MIT or Apache-2.0 | `cargo deny` for licence allow-list and advisories (Phase 10) |
| **rustfmt** | MIT or Apache-2.0 | `rustfmt.toml` with `edition` and `style_edition` so editors and `cargo fmt` agree; `--check` in CI |
| **docs.rs** | MIT | Same stack (Axum, SQLx, PostgreSQL): local PostgreSQL via compose on loopback, SQLx offline metadata committed, parallel test runner (Phase 03) |
| **rust-analyzer** | MIT or Apache-2.0 | One rules file shared by agents; working habits now in AGENTS.md ("How to change things") |
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
