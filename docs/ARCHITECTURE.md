# Apelora Fabric architecture (proposed)

Apelora Fabric targets the upstream **Xen Project hypervisor directly**, with no XCP-ng dependency.

- **Studio:** Vite + React + TypeScript + React Flow.
- **Controller:** Rust API, authentication, application lifecycle, desired-state reconciliation, scheduling.
- **Node agent:** Rust host-side integration via Xen toolstack interfaces on supported x86-64/ARM64 hosts.
- **Fabric networking:** typed application port graph compiled to virtual interfaces, switching, and access policies.
- **Volumes:** provider-neutral persistent disk abstractions.
- **Catalog:** appliance images, architectures, ports, constraints, resource envelopes, and local SVG logos.

The visual Studio prototype is not a hypervisor or deployment engine. Treat cross-architecture VM images, Xen toolstack licensing and host privilege isolation as design constraints.
