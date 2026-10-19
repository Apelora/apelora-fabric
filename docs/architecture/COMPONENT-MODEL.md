# Component model (concepts and clean-room practice)

This describes the ideas Apelora's appliance and assembly model is built on, in our own words, and how we keep the implementation original.

## Concepts we use

These are general, long-established ideas in component-based and infrastructure-as-code design:

| Concept | In Apelora | Status |
|---|---|---|
| **Class vs instance** | An *appliance class* (manifest: ports, resources, defaults, image placeholder) is instantiated many times; each instance carries its own copy of ports and settings. | Implemented |
| **Typed terminals** | Ports are directional (input left, output right), named, typed (`http`, `postgres`, …) and carry multiplicity and a required/optional flag. Short names are shown on the canvas; full text is in the tooltip. | Implemented |
| **Point-to-point logical links** | An assembly connects one output terminal to one input terminal. Links are logical (protocol-level), not IP addresses or hosts; the platform later decides how to realise them on the network. | Model implemented; realisation is a later phase |
| **Hardware independence** | Assemblies contain no hosts, IP addresses or storage identifiers, only resource *requests*. | Implemented (schema) |
| **Custom components** | Users define their own appliance classes (name, icon, ports, resources) in the Appliance Designer, or turn a configured instance into a class. Custom classes are embedded in exported assemblies so files are self-contained. | Implemented |
| **Hierarchical composition** | A selection of appliances is packaged as a class (`composition`) whose exposed ports map to inner terminals; its `net` maps to every inner NIC. Components can contain components. | Implemented (grouping, ungrouping, export/import); drill-in view planned |
| **Network interface** | Every appliance has a `net` terminal (a NIC) that plugs into a network or a network appliance; unplugged it uses the default network. | Implemented |
| **Link tags** | Terminals sharing a name are linked without a drawn cable. | Implemented |
| **Setup parameters** | Per-instance values (names, public addresses, credentials by reference) supplied when an application is instantiated. | Planned |
| **Volumes and resource budgets** | Persistent volumes attached to appliances, and a minimum/maximum resource budget per application. | Planned |
| **Catalog sharing** | Exporting/importing classes and assemblies as portable, versioned, diff-friendly text. | Partly implemented (JSON) |

## What "original" means here

- Our names, schema (`fabric.apelora/v1alpha1`), manifests, icons, layout and visual language are our own. Terminals are drawn as RJ45-style jacks and plugs designed for this project.
- We do not copy another product's source, artwork, screenshots, documentation text, file formats, class names or manifests, and we do not store third-party documents in this repository.
- Behaviour that is common knowledge (drag-and-drop canvas, typed ports, undo/redo, import/export) is implemented from first principles.

## Clean-room practice

1. Write requirements in our own words from general concepts (this document). Do not paste or summarise third-party documents into the repo.
2. Design the data model and UI ourselves; record significant decisions in `docs/decisions/`.
3. Keep a short provenance note for any asset or snippet from elsewhere (origin, licence). Logos follow `packages/appliance-catalog/assets/logos/ATTRIBUTION.md`.
4. Before the first public release, obtain a **freedom-to-operate review by qualified counsel** covering patents and trademarks. This repository's engineering notes are not legal advice, and patent status must be verified by counsel, not assumed.
5. Keep the architecture modular so any one mechanism can be replaced if a concern is raised.
