---
name: fabric-networking
description: Compile Studio connections into network intents and apply them through Incus — stable service names, endpoints, deny-by-default ACLs, ingress, and one OVN network per application (built). Use when changing how edges in the port graph become real connectivity, or when working on crates/network-fabric or crates/service-discovery.
---

# Fabric networking (Apelora virtual network fabric)

A line drawn in Studio is a **service dependency plus an authorisation**, not a cable or a fixed IP binding. The network fabric is a Rust library used by the control plane and deployment worker; Incus (and OVN, when enabled) does the actual packet handling. See also `incus-networking`.

## From graph to intent

| Studio element | Meaning |
|---|---|
| Module | One or more Incus instances (replicas) |
| Input terminal | A service endpoint (protocol, port) |
| Output terminal | A dependency on another service |
| Connection | Allow that traffic and resolve the destination by **service name** |
| Public terminal | Ingress: an externally reachable endpoint |

Each connection compiles to an intent such as: source service `web`, target service `backend`, protocol TCP, port 8080, access allow, discovery name `backend.<app>.internal` (an Apelora-managed name, not something Incus provides).

## Rules

1. **Stable service identity.** Consumers address a service name that maps to its *healthy* replicas. Replacing or relocating an instance must not change what consumers connect to.
2. **Deterministic compilation.** The same graph revision always yields the same intents and the same diff; compile, show the diff (dry run), then apply.
3. **Deny by default.** Instances talk only over declared connections, only on the declared protocol/port and direction. Scope everything by Incus project; no cross-tenant paths.
4. **One OVN network per application** (built, REAL INCUS TESTED): Incus-managed OVN with an ACL and an address set per module, health-checked load balancers for service addresses, and `@external` egress for internet access. OVN brings its own services and needs an uplink network that spans the members; in the lab a bridge (`incusbr0`) is only that uplink. A local managed bridge on one member does **not** reach other members, so it never carries module traffic. DNS zones and forwards are not used yet.
5. **Ingress is part of availability.** Several replicas behind one unreplicated proxy or address is not HA.
6. Never bind unauthorised host ports; never let an instance reach the management network or the Incus API.
7. Mark a path ready only after a connectivity/readiness check passes.

## Checklist

- Snapshot tests of compiled intents for representative graphs; re-compiling an unchanged graph yields no diff
- Isolation tests: unconnected services have no path; other projects unreachable
- Mock-provider tests labelled MOCK VERIFIED; real connectivity only claimed after a test-cluster run (`testing-operations`)

## Current implementation

- Compiler: `packages/assembly-model/src/network.ts` — `compileNetwork(assembly, application)` → `{ services, intents, segments, warnings }`; `allows(plan, from, to, port)`. Default ports per protocol in `DEFAULT_PORTS` (generic `tcp` has none: the rule allows any TCP port and the plan warns). Tests: `packages/module-catalog/src/network.test.ts` (snapshot of the commerce stack, order independence, isolation, tags, generic TCP).
- Links are cables plus every pair of terminals sharing a link tag; the output terminal is the consumer, the input the provider; `network` terminals only form segments.
- **Metrics are the exception:** a `metrics` output (`mon`) is an endpoint, so the collector at the input end (`scrape`) is the consumer and reaches the module on its class's metrics port (`compileNetwork(a, app, lookupClass)` → `metricsEndpoint`, including `portBy`). Without a lookup, or for a class without a metrics port, it falls back to 9100 and warns. Tests: `network.test.ts`, `metrics.test.ts` (every catalog class's `mon` compiles to its own port, never the reverse). A stateless module with a scraped `mon` gets that port on its service address too, so a collector reading through it sees one replica at a time (open point).
- Enforcement (`crates/domain/src/network.rs`, worker `network.sync`, `APELORA_NETWORK_UPLINK`): one OVN network per application, per module an ACL and an address set named `<application>-<module>`; NICs reject unmatched traffic both ways; each connection writes the consumer's egress and the provider's ingress rule between address sets (`acl_rules`). Never use ACL subject selectors (they failed across members) and never relax a default action. REAL INCUS TESTED across two members.
- Ownership: every network object carries `user.apelora.managed=true` and `user.apelora.application=<app>`; listing, adopting and deleting check both — never a name prefix (`shop-` would match `shop-eu`). `network.sync` removes stale ACLs before the address sets they reference, and the network with the last module.
- Service addresses (`APELORA_SERVICE_ADDRESSES`, a routed IPv4 `/16`–`/30` or IPv6 `/112`–`/126` range of the uplink, needs the uplink): every *stateless* module that is reached on a known port gets one OVN load balancer on the application network — listen address allocated in PostgreSQL (`service_addresses`, unique per address and per module; one family per module), backends `r<n>` = replicas' addresses of the **same family** (`/32` or `/128`) with description `<instance> <volatile.uuid>`, ports from the connections, health checks on. The load balancer translates before ACLs are evaluated, so no extra rules; health probes are not filtered by ACLs. `network.sync` creates/updates it and deletes it (freeing the address) when the module leaves; status API lists `services`; the Studio overlay shows it. IPv4 REAL INCUS TESTED (two members): traffic spread over both replicas, a crashed replica drained within seconds, restarted by the supervisor and back in rotation; a deleted replica recreated at the same address rebinds through the changed description. IPv6 listen addresses MOCK VERIFIED (not yet run on the lab uplink).
- Publishing: a gateway module in the design is the outside world. `publishedPorts(plan, gateways, deployed)` (assembly-model) → API `published` → `SyncRequest.published` → a service address for the module (any availability) and `published_rules` (ingress allow from any source on that port only). Outside traffic must be *routed* to the application network's router (`volatile.network.ipv4.address`); the OVN router does not answer ARP for listen addresses. REAL INCUS TESTED.
- Internet access: `Internet` (none/web/any) per module (Studio Properties → Internet access; `DesiredModule.internet`; `SyncRequest.internet`) → `internet_rules` = egress allow to `@external` only. Never write an any-destination egress allow: egress and ingress allows combine with OR, so it would open every other module. REAL INCUS TESTED.
- Service discovery (ADR 0004): the API allocates service addresses at deployment (`allocate_services`), and every consumer's create spec gets `environment.APELORA_SERVICE_<PROVIDER>` (`discovery_env`, `with_discovery`) — service address, else instance name; the supervisor applies the same to recreated/substituted instances. `environment.APELORA_*` is reserved for Apelora (`InstanceSpec::validate_from_caller`). REAL INCUS TESTED.
- Intended rules have one source: `SyncRequest::intended_acls()` (connections both sides, published ports, internet access) is what `network.sync` writes and what the Console's Networks page (`GET /api/v1/networks/applications`) compares with Incus (`rule_drift`, order-insensitive), rebuilt from the recorded deployment. Never compute rules for display separately. Settings in effect: `GET /api/v1/networks/settings` (uplink, service range, project `restricted.networks.*`, the uplink's `ipv4.routes` and `ipv6.routes` through the administrator identity; the deploy identity cannot read the default project's uplink, REAL INCUS checked read-only 2026-10-21).
- Service names (ADR 0004 step 4): `APELORA_SERVICE_NAME_<PROVIDER>` = `<module>.<application>.internal` when a service address exists, and the same records on the application network (`dns.domain`, `user.apelora.dns`), merged into the config already there. MOCK VERIFIED. Not answered inside the application, and not REAL INCUS TESTED. OVN's DNS still answers instance names only.
