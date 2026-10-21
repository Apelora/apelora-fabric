# ADR 0015 — Application lifecycle verbs: Shut down, Stop, and Migrate

Status: **accepted; built (API, worker, Studio)** · Date: 2026-10-21 · Extends [application lifecycle](../design/applications-view.md) · Mobility: [moving modules](../design/mobility-and-ha.md)

## Context

An application could be started, restarted and *stopped*. "Stop" always meant a graceful stop: every instance is asked to shut down, in dependency order, and given time to. People who run virtual machines expect two different things from a stop control: asking the guest to shut down (safe, slow) and cutting the power (immediate, can lose what was only in memory). They also expect to move a running workload to another server. The Studio's Applications list, which is where an application is operated, offered neither.

## Decision

1. **Five verbs per application:** Start, Restart, **Shut down**, **Stop**, **Migrate**. Their words follow the convention of the virtualisation tools people already know.
   - **Shut down** (`shutdown`) is the old behaviour of `stop`: graceful, dependency order, each guest given time.
   - **Stop** (`stop`) is an immediate power-off of every instance, in the same dependency order, with no wait for the guests (the instance job carries `force`). It exists for the application that will not shut down. It is destructive in the sense that unsaved memory is lost; data on disk and volumes are kept.
   - **Migrate** (`migrate`, with a `target` server) moves every instance of the application to that server through the existing `instance.move` machinery ([mobility](../design/mobility-and-ha.md)): live where a virtual machine can, otherwise stopped, moved and started again. Instances already on the target stay.
2. **`stop` changes meaning on the wire.** The name `stop` now means the immediate one and `shutdown` the graceful one. Operations recorded before this ADR keep saying `stop`; they were graceful, and the history shows them as recorded. Clients that send `stop` and want the old behaviour send `shutdown` (the Console does).
3. **The same rules for all of them.** Which verbs are offered, and why not, comes from the server per application (`actions.start|restart|shutdown|stop|migrate`); the interface never decides. Shut down, Stop and Restart need `application.operate` and the same confirmation (`confirmDisruption`) when clients of a single-instance or stateful module lose service. One operation at a time per application. Success is declared only when the cluster confirms each instance in its target state.
4. **Migrate is planned before it is asked for.** `GET /api/v1/applications/{name}/migration?target=` is a dry run: for each instance whether it keeps running, what is copied, and what blocks it (an unknown or offline server, another architecture, an instance Apelora did not create). Starting the operation runs the same check again. Moving something that has to stop needs the same confirmation as a shutdown. A blocked instance blocks the whole migration: nothing is moved.
5. **Replicas land together.** A migration moves everything to the one server named, so a module's replicas then share a server and lose their spread. The dialog says so; rebalancing is not built (see [current state](../architecture/current-state.md)).
6. **Constraints that come from moving an instance.** The project must allow member targeting (`restricted.cluster.target=allow`), as for moving one instance; the control plane refuses otherwise. Evidence: MOCK VERIFIED through the API and the mock provider's move; **REAL INCUS TESTED** at the worker (2026-10-21, the four-member lab, `real_incus.rs`: two containers were started, restarted, shut down gracefully, stopped at once with forced stop jobs, and migrated to another member, each confirmed running on the new member). The control plane's dry run and the Studio dialog have not been run against the real cluster, and live migration of virtual machines through an application is not exercised (the single-instance move is, see [mobility](../design/mobility-and-ha.md)).

## Consequences

- A new status, `migrating`, while a migration runs; every action is greyed out meanwhile.
- API: `OperateRequest.target`, `Actions.shutdown|migrate`, `OperationKind` gains `shutdown` and `migrate`, a new `migrating` deployment status, and the migration preview. Database migration 0030 widens the operation kinds.
- The worker's coordinator learns two phase actions: `poweroff` and `migrate` (the latter carries the checked `instance.move` request per instance and confirms the instance on its new member).
- Alternatives considered: keeping `stop` graceful and adding `force-stop` (no breaking change, but "Stop" then means the opposite of what the virtualisation tools use, and the list would show two near-identical buttons); migrating one module at a time in dependency order (safer for chatty applications, more steps to get wrong; can be added behind the same verb).
