---
name: incus-networking
description: Use for Incus bridges, VLANs, OVN, service discovery, load balancing and Studio port mapping.
---

# Incus Networking and Typed Connectors

## When to use

Use for Incus bridges, VLANs, OVN, service discovery, load balancing and Studio port mapping.

## Required workflow and guardrails

- Model typed logical terminals (HTTP, TCP, Postgres) as desired service contracts, not direct physical wires.
- An Incus local bridge does not automatically form a cross-host overlay; validate LAN/VLAN reachability across members.
- For advanced multi-tenant overlays use documented Incus OVN/OVS, forwarding, ACLs, and load balancers only as an explicit architecture choice.
- A service replica is not HA if ingress remains a single failure point; inspect ingress/DNS availability and client routing.
- Enforce deny-by-default graph connection policy where appropriate; scope network resources by project and prevent cross-tenant access.
- Generate deterministic network changes and perform dry-run/diff before writes; never bind to unauthorized host ports.
- Verify health checks and failure drains before marking a network path ready.
- Record source/destination/port/transport/security semantics of each connection.

## Evidence required before calling a feature complete

Explain what changed, actual tests and commands run, source/API reference, remaining safety constraints, and whether tested against real Incus or only mocks.

## Primary references

- https://linuxcontainers.org/incus/docs/main/
- https://github.com/lxc/incus
- https://doc.rust-lang.org/cargo/reference/workspaces.html
