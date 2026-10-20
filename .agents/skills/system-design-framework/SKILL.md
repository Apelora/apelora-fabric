---
name: system-design-framework
description: Run a structured, written design process before building any new Apelora Fabric subsystem or major feature — clarify scope, sketch the high-level design, deep-dive the risky parts, then wrap up with trade-offs, failure handling, and rollout. Use at the start of a new component, API, data model, or cross-cutting change, and when reviewing someone else's design proposal.
---

# System design framework

Design is cheaper than rework. Use this before writing code for anything that spans components (Studio, Controller, Node, Catalog, Fabric networking) or is hard to reverse (schemas, wire protocols, on-disk formats, licensing boundaries).

**Inputs required:** a one-paragraph problem statement from the user. **Output:** `docs/design/<topic>.md` plus any ADRs in `docs/adr/`.

## When to skip

A bug fix, a copy change, or a change inside one module with an unchanged interface does not need this. If unsure, do Step 1 only and ask.

## Step 1 — Understand the problem and bound the scope

Do not propose a solution yet. Resist the urge to answer fast; a confident answer to the wrong question is the costliest mistake.

Ask, and record the answers (mark anything the user did not confirm as an *assumption*):

1. **Features:** what exactly must it do? What is explicitly out of scope?
2. **Users and scale:** single operator, team, or multi-tenant? Hosts, modules per assembly, assemblies, concurrent users, now / 6 months / 1 year.
3. **Constraints and reuse:** existing schemas, the Incus authority boundary in `docs/adr/0001-incus-first.md`, upstream licences (`license-compliance`), supported architectures (x86-64, ARM64).
4. **Non-functional needs:** availability target, latency budget, consistency expectations, security and audit requirements, offline or air-gapped operation.
5. **Failure expectations:** what must still work when a node, the Controller, or the store is down?
6. **Success criteria:** how will we know it works (a test, a metric, a demo)?

Stop and wait for answers when any of 1–4 is unknown and changes the design.

## Step 2 — High-level design and buy-in

1. Draw a box diagram (Mermaid in the markdown is fine): clients, services, stores, queues, trust boundaries, and the direction of every arrow.
2. Walk through 2–3 concrete use cases end to end, e.g. "import an assembly → validate → plan → deploy to two nodes → one node fails". Use cases expose missing components and edge cases.
3. Do a rough capacity estimate (`capacity-estimation`) to confirm the shape fits.
4. Define the **contracts**: main API calls (method, path, request, response, errors), the core data model, and events/messages. Only to the depth the problem needs.
5. Present this to the user and ask for feedback **before** the deep dive. Treat them as a teammate: invite objections.

## Step 3 — Deep dive

Choose the two or three riskiest or least understood components and design them in detail. For Apelora the usual candidates:

| Candidate | Questions to settle |
|---|---|
| Reconcile loop | Trigger model, idempotency, backoff, conflict handling, crash recovery |
| Network plan compiler | Allocation determinism, isolation guarantees, apply ordering |
| Schema evolution | Versioning, migration, compatibility window |
| Node enrollment and auth | Identity, rotation, revocation, blast radius |
| Scheduler | Placement constraints, fairness, rebalancing |
| Storage lifecycle | Attach/detach safety, snapshot consistency, retention |

For each: data structures, algorithms, concurrency model, failure modes, observability, and how it will be tested (fake providers, property tests, fault injection).

Time-box. Prioritize depth where being wrong is expensive.

## Step 4 — Wrap up

- **Bottlenecks and single points of failure:** name them, and say whether each is accepted or mitigated.
- **Failure handling:** table of component → failure → detection → behavior → recovery.
- **Operations:** logging, metrics, alerts (`observability`); rollout, canary, and rollback; data migration.
- **Security:** threats and mitigations (`security-review`); licensing impact (`license-compliance`).
- **Scale-up path:** what you would change at 10× and what signal triggers it.
- **Recap and next steps:** the recommended first implementation slice, small enough to review.

## Guardrails

- **Simplicity first.** Over-engineering is a defect: every extra component needs a stated reason and a measured trigger. Prefer a boring solution that meets today's scale with a documented path to tomorrow's.
- **Every decision gets a reason.** Record significant ones as ADRs (Context, Decision, Consequences, Alternatives).
- **Separate facts from assumptions.** Do not present guesses as measurements.
- **No hidden scope.** Anything not listed in Step 1 is out of scope until agreed.
- **Ask, don't guess,** when requirements are ambiguous; avoid narrow-mindedness: compare at least two viable options for each major choice.

## Design note template

```markdown
# <Topic>
Status: draft | agreed | superseded   Owner: <name>   Date: <YYYY-MM-DD>

## 1. Problem and scope
Goals / Non-goals / Users and scale / Constraints / Assumptions / Open questions

## 2. High-level design
Diagram / Use cases / Contracts (API, data model, events) / Capacity estimate

## 3. Deep dive
<component 1> / <component 2> / <component 3>

## 4. Wrap-up
Bottlenecks and SPOFs / Failure table / Operations / Security and licensing / Scale-up path

## 5. Decisions and alternatives
ADR links

## 6. Next steps
First slice, tests, owners
```

## Acceptance criteria

- Scope, non-goals, and assumptions are written and confirmed by the user
- At least two alternatives were considered for each major decision, with reasons for the choice
- Failure table and rollback plan exist
- Capacity estimate present and consistent with the design
- Security and licensing reviewed, or listed as explicit open items
- A concrete, small first slice is proposed
