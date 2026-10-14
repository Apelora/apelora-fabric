---
name: system-design-framework
description: Run a structured design process before building any new Apelora Fabric subsystem or major feature — clarify scope, sketch the high-level design, deep-dive the risky parts, then wrap up with trade-offs. Use at the start of a new component, API, or cross-cutting change, and when reviewing a design proposal.
---

# System design framework

Use before writing code for anything that spans components (Studio, Controller, Node, Catalog, Fabric networking) or that is hard to reverse.

## Step 1 — Understand the problem and bound the scope

Do not propose a solution yet. Write down, and get the user to confirm:
- Which user-visible features are in scope, and which are explicitly out
- Who the users are (single operator, team, multi-tenant) and expected scale now, in 6 months, in a year (hosts, appliances per assembly, assemblies, concurrent users)
- Existing components and constraints to reuse (the Hypervisor trait, schemas, the licensing boundary in `docs/XEN-LICENSING-STRATEGY.md`)
- Non-functional needs: availability, latency, consistency, security
- Open assumptions, each labeled as an assumption

## Step 2 — High-level design and buy-in

- Box diagram of components, data stores, queues, and trust boundaries
- Walk through two or three concrete use cases end to end (e.g. "import an assembly, validate, deploy to two nodes")
- Rough capacity estimate (see `capacity-estimation`) to confirm the shape fits
- Define the main API calls and the core data model, only at the level the problem needs
- Present it and ask for feedback before going deeper

## Step 3 — Deep dive

Pick the two or three riskiest components and design them in detail. Typical Apelora candidates: the reconcile loop, network plan compilation, schema migration, node enrollment/auth. For each, cover data structures, failure modes, concurrency, and how it will be tested.

## Step 4 — Wrap up

- Name the bottlenecks and single points of failure
- List trade-offs made and alternatives rejected, with reasons
- Failure handling: what happens when a node, the Controller, or a store goes down
- Operations: logging, metrics, alerts, rollout and rollback
- Next steps and what to measure first

## Guardrails

- Prefer the simplest design that meets the stated scale; over-engineering is a defect. Note what would trigger a more complex design later.
- Each decision gets a one-line reason. Record significant ones as short ADRs in `docs/adr/`.
- Ask clarifying questions instead of guessing when scope is ambiguous.

## Output

A design note in `docs/design/<topic>.md` with the four sections above, ending in the open questions and the recommended first implementation step.
