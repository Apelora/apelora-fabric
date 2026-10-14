---
name: fabric-networking
description: Compile logical appliance connections into virtual networks — address and VLAN/VNI allocation, bridges and virtual interfaces, isolation and access policies. Use when changing how edges in the port graph become real network configuration.
---

# Fabric networking

The user draws logical connections; the compiler turns them into a **network plan** the Node can apply.

## Compilation

1. Each connection (or group of compatible connections) maps to an isolated L2 segment or routed link.
2. Allocate deterministically and collision-free: segment ids (VLAN/VXLAN VNI), subnets from a configured pool, and per-NIC MAC addresses (locally administered range, stable per instance+port).
3. Produce per-node plans: bridges, virtual interfaces (vif), IPs, and policies. The plan is data; applying it is the Node's job.
4. Default deny: instances talk only over declared connections. Policy rules derive from port protocol/type (allow only that protocol and direction).
5. Cross-node connections use an overlay (e.g. VXLAN); same-node use local bridges. Plans must be identical regardless of apply order.

## Rules

- Allocation state lives in the Controller store; re-compiling an unchanged assembly yields the same plan.
- Tenants/applications never share a segment unless explicitly connected.
- Validate that no plan lets a guest reach the management network or dom0.

## Checklist

- Property tests: no duplicate MACs/IPs/VNIs, no overlapping subnets
- Isolation tests: unconnected instances have no path
- Plan snapshot tests for representative topologies
