---
name: fabric-networking
description: Compile Studio connections into network intents and apply them through Incus — stable service names, endpoints, deny-by-default ACLs, ingress, and the VLAN (MVP) vs OVN (full platform) choice. Use when changing how edges in the port graph become real connectivity, or when working on crates/network-fabric or crates/service-discovery.
---

# Fabric networking (Apelora virtual network fabric)

A line drawn in Studio is a **service dependency plus an authorisation**, not a cable or a fixed IP binding. The network fabric is a Rust library used by the control plane and deployment worker; Incus (and OVN, when enabled) does the actual packet handling. See also `incus-networking`.

## From graph to intent

| Studio element | Meaning |
|---|---|
| Appliance | One or more Incus instances (replicas) |
| Input terminal | A service endpoint (protocol, port) |
| Output terminal | A dependency on another service |
| Connection | Allow that traffic and resolve the destination by **service name** |
| Public terminal | Ingress: an externally reachable endpoint |

Each connection compiles to an intent such as: source service `web`, target service `backend`, protocol TCP, port 8080, access allow, discovery name `backend.<app>.internal` (an Apelora-managed name, not something Incus provides).

## Rules

1. **Stable service identity.** Consumers address a service name that maps to its *healthy* replicas. Replacing or relocating an instance must not change what consumers connect to.
2. **Deterministic compilation.** The same graph revision always yields the same intents and the same diff; compile, show the diff (dry run), then apply.
3. **Deny by default.** Instances talk only over declared connections, only on the declared protocol/port and direction. Scope everything by Incus project; no cross-tenant paths.
4. **Two network profiles, chosen explicitly:**
   - *MVP:* an existing physical VLAN/L2 that genuinely spans the members, with Incus NICs attached to it. A local managed bridge on one member does **not** reach other members.
   - *Full platform:* Incus-managed OVN networks with ACLs, DNS zones, forwards and health-checked load balancers. OVN brings its own services and needs a suitable uplink; treat it as a deployment profile with its own runbook.
5. **Ingress is part of availability.** Several replicas behind one unreplicated proxy or address is not HA.
6. Never bind unauthorised host ports; never let an instance reach the management network or the Incus API.
7. Mark a path ready only after a connectivity/readiness check passes.

## Checklist

- Snapshot tests of compiled intents for representative graphs; re-compiling an unchanged graph yields no diff
- Isolation tests: unconnected services have no path; other projects unreachable
- Mock-provider tests labelled MOCK VERIFIED; real connectivity only claimed after a test-cluster run (`testing-operations`)

## Current implementation

- Compiler: `packages/assembly-model/src/network.ts` — `compileNetwork(assembly, application)` → `{ services, intents, segments, warnings }`; `allows(plan, from, to, port)`. Default ports per protocol in `DEFAULT_PORTS` (generic `tcp` has none: the rule allows any TCP port and the plan warns). Tests: `packages/appliance-catalog/src/network.test.ts` (snapshot of the commerce stack, order independence, isolation, tags, generic TCP).
- Links are cables plus every pair of terminals sharing a link tag; the output terminal is the consumer, the input the provider; `network` terminals only form segments.
- Not enforced yet: the Studio shows the rules in the deploy review with that statement. Enforcement (OVN networks, ACLs, DNS) comes next and must be REAL INCUS TESTED across members before any claim.
