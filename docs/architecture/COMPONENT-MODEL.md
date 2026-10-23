# Component model (concepts and clean-room practice)

This describes the ideas Apelora's module and assembly model is built on, in our own words, and how we keep the implementation original.

## Concepts we use

These are general, long-established ideas in component-based and infrastructure-as-code design:

| Concept | In Apelora | Status |
|---|---|---|
| **Class vs instance** | A *module class* (manifest: ports, resources, defaults, optional runtime with image and instance kind) is placed on the canvas many times; each placed module (`ModuleInstance`) carries its own copy of ports and settings. A placed module is not an Incus instance: deployed, it becomes one Incus instance per replica ([platform architecture §4.1](PLATFORM-ARCHITECTURE.md#41-from-an-application-to-running-instances)). | Implemented |
| **Typed terminals** | Ports are directional (input left, output right), named, typed (`http`, `postgres`, …) and carry multiplicity and a required/optional flag. Short names are shown on the canvas; full text is in the tooltip. | Implemented |
| **Point-to-point logical links** | An assembly connects one output terminal to one input terminal. Links are logical (protocol-level), not IP addresses or hosts; the platform decides how to realise them on the network. | Implemented: deployed, each becomes an access rule (and, where needed, a service address) on the application's OVN network |
| **Hardware independence** | Assemblies contain no hosts, IP addresses or storage identifiers, only resource *requests*. | Implemented (schema) |
| **Custom components** | Users define their own module classes (name, icon, ports, resources) in the Module Designer, or turn a configured instance into a class. Custom classes are embedded in exported assemblies so files are self-contained. | Implemented |
| **Hierarchical composition** | A selection of modules is packaged as a class (`composition`) whose exposed ports map to inner terminals; its `net` maps to every inner NIC. Components can contain components. | Implemented (grouping, ungrouping, export/import); drill-in view planned |
| **Network interface** | Every module has a `net` terminal (a NIC) that plugs into a network or a network module; unplugged it uses the default network. | Implemented |
| **Link tags** | Terminals sharing a name are linked without a drawn cable. | Implemented |
| **Setup parameters** | Per-module values supplied when an application is deployed. A class marks a field `supply: "deploy"`. The deploy step asks for it. A name or public address is sent as `environment.SETUP_*`. A credential is the reference name in `environment.SECRET_*`; the secret itself is not stored. Every secret reference in the catalog is asked this way, along with the gateway's public name, the web servers' names, and the Linux base hostname. | Implemented |
| **Volumes and resource budgets** | Persistent volumes per module (class defaults or set in the Studio; not for stateless classes) and resources per module are implemented. An application can set a minimum and maximum for vCPU and memory; the design is refused when the modules, counted once per replica, fall outside it. | Implemented |
| **Catalog sharing** | An application is a `.assembly.json` file. Custom module classes are a `.catalog.json` file (`ModuleCatalog`). Both carry `fabric.apelora/v1alpha1`, use sorted keys, and are rejected as a whole when they do not match the schema. Importing an application also brings the custom classes it uses. A catalog file cannot replace a built-in class. | Implemented |

## What "original" means here

- Our names, schema (`fabric.apelora/v1alpha1`), manifests, icons, layout and visual language are our own. Terminals are drawn as RJ45-style jacks and plugs designed for this project.
- We do not copy another product's source, artwork, screenshots, documentation text, file formats, class names or manifests, and we do not store third-party documents in this repository.
- Behaviour that is common knowledge (drag-and-drop canvas, typed ports, undo/redo, import/export) is implemented from first principles.

## Clean-room practice

1. Write requirements in our own words from general concepts (this document). Do not paste or summarise third-party documents into the repo.
2. Design the data model and UI ourselves; record significant decisions in `docs/decisions/`.
3. Keep a short provenance note for any asset or snippet from elsewhere (origin, licence). Logos follow `packages/module-catalog/assets/logos/ATTRIBUTION.md`.
4. Before the first public release, obtain a **freedom-to-operate review by qualified counsel** covering patents and trademarks. This repository's engineering notes are not legal advice, and patent status must be verified by counsel, not assumed.
5. Keep the architecture modular so any one mechanism can be replaced if a concern is raised.
