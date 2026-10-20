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
- OVN load balancer health checks run on the backend's own chassis: a dead member's backends stay "online". Read `GET …/load-balancers/<addr>/state` (`backend_health`); `unknown` means no check is installed — re-apply by toggling `healthcheck` (Apelora does this after every change).
- OVN load balancers (verified on 7.0.1): listen addresses must be in the uplink's `ipv4.routes` and, for a restricted project, `restricted.networks.subnets=<uplink>:<range>`; backends must be in the network's subnet; health-check mappings bind to the instance's logical port, so change the backend (Apelora: description with `volatile.uuid`) when an instance is recreated at the same address — an unchanged PUT does not rebind.
- Record source/destination/port/transport/security semantics of each connection.

## Evidence required before calling a feature complete

Explain what changed, actual tests and commands run, source/API reference, remaining safety constraints, and whether tested against real Incus or only mocks.

## Primary references

- https://linuxcontainers.org/incus/docs/main/
- https://github.com/lxc/incus
- https://doc.rust-lang.org/cargo/reference/workspaces.html

## Facts from the test cluster (REAL INCUS TESTED, Incus 7.0.1, OVN 25.03, 2026-10-20)

- `features.networks=true` gives a project its own OVN networks and ACLs (needs OVN; can only change on an empty project); `restricted.networks.uplinks` limits which uplink they use. A managed bridge (`incusbr0` with `ipv4.ovn.ranges`) works as the uplink for east-west testing.
- OVN networks resolve instances by name (`<name>.incus`) across members; Lima's `user-v2` network carries the Geneve tunnels.
- **Use IP addresses or address sets in ACL rules, not ACL subject selectors, when instances can be on different members:** selector rules matched only same-member traffic (the remote member answered SYN with RST; `ovn-trace` claimed allow).
- A NIC with ACLs rejects unmatched traffic both ways; egress and ingress allows combine with OR (either side's allow passes the flow), and default egress `allow` on a consumer outranks the provider's ingress rejection. Write matching rules on both sides; never relax a default.
- Test listeners carefully: a name resolving to IPv6 first while the service listens on IPv4 only looks like a block.
