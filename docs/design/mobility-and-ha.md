# Moving modules and surviving failures

Status: **agreed; slice A done (REAL INCUS TESTED), slices B and C next** · Date: 2026-10-21 · Related: [platform architecture §7](../architecture/PLATFORM-ARCHITECTURE.md), [storage](../architecture/STORAGE.md), [storage options](../assets/diagrams/storage-options.svg), [member failure](../runbooks/member-failure.md)

## 1. What Incus does, measured on the test cluster (Incus 7.0.1, four members, btrfs local pools)

| Case | Result | Evidence |
|---|---|---|
| Move a running VM (`migration.stateful=true`) to another member | Moves running; the disk on the local pool is copied, then memory. Boot time unchanged; longest pause of a 0.1 s ticker inside: **0.80 s** | REAL INCUS TESTED, 2026-10-21 |
| Evacuate the VM's member with `cluster.evacuate=live-migrate`; restore | Moved running to another member and back; longest pause **0.29 s**; no reboot | REAL INCUS TESTED, 2026-10-21 |
| Container | No live move (needs CRIU): it stops, moves and starts | Incus documentation |
| Instance with an attached custom volume on a member-local pool | The volume does not follow (unless marked `dependent`, which ties its life to the instance); evacuation stops the instance in place | REAL INCUS TESTED (containers), Incus documentation (VMs) |
| A member fails | Its instances stop. Incus can restart those on **shared** storage elsewhere (`cluster.healing_threshold`), and warns that it must be sure the member is really off (fencing), or two copies may write the same disk | Incus documentation |
| Load balancing | Incus can live-migrate VMs to the least loaded member (`cluster.rebalance.*`) | Incus documentation |

The restricted deploy identity may not choose a member for an instance unless the project allows it (`restricted.cluster.target=allow`).

## 2. What each workload gets

| Workload | Planned move (maintenance, rebalancing) | Unplanned failure |
|---|---|---|
| Stateless replicas behind a service address | Replace, then retire (built) | Other replicas keep serving; the lost one is replaced (built, REAL INCUS TESTED) |
| VM without local data volumes, live migration on | **Moves running, under a second** (slice A) | Restarts elsewhere only on shared storage, after fencing (slice B) |
| Container, or VM with `data-local` volumes | Stop, copy the volumes to the new member, move, start: a short stop (slice A) | Waits for its member, or is restored from a backup on another member |
| Anything on a shared pool (SAN with clustered LVM, distributed storage) | Moves without copying | Restarts on another member once the failed one is fenced (slice B) |

No interruption at all through a failure needs more than one copy running: replicas for stateless modules, the application's own replication for databases.

## 3. Slices

**A. Moves.** A module can ask for live migration (VMs only; refused together with `data-local` volumes, which cannot follow it). Apelora then sets `migration.stateful=true` and `cluster.evacuate=live-migrate`, so evacuations move it running. A *Move…* action (dry run first) moves one instance to a chosen member: live when it can, otherwise only when the person allows a short stop, copying its volumes to the new member and deleting the old copies once it runs there. The move is a worker job (`instance.move`) so a crash resumes it; it needs `restricted.cluster.target=allow` on the project.

**B. Failure recovery on shared storage, with fencing.** Shared pools (clustered LVM on a SAN, or distributed storage) become the `data-replicated` class. Apelora fences a failed member before anything restarts elsewhere: a *fence provider* powers it off through its BMC (later: through MAAS) or, without one, an administrator confirms it is off (typed name, approval). Only then are its instances on shared storage started elsewhere. Incus's own healing stays off unless an automatic fence provider is configured.

**C. Rebalancing.** Expose Incus's rebalancing settings for live-migratable VMs and include replica spread in the decision.

## 3a. Slice A as built

- `apelora_domain::mobility::plan_move` decides live or stopped, what is copied and what blocks it (not Apelora's, the same member, not Online, another architecture, a stop not allowed).
- `GET|POST /api/v1/instances/{project}/{name}/move` (dry run; request with the name typed and `allowStop`), needs `instance.manage`; queues `instance.move`, audited.
- The worker's `instance.move` job re-reads the instance at every step: live move, or stop → copy volumes to the target member → move → start → delete the old copies and record the volumes' new member. A failed copy or move starts it again where it was.
- Studio: *Keep running when moved* for VM modules (refused with `data-local` volumes); the request sets `migration.stateful=true` and `cluster.evacuate=live-migrate`. Fabric Console: *Move…* on Apelora's instances.
- REAL INCUS TESTED (2026-10-21): `instances_move_with_their_volumes_and_vms_move_live`; from the console in a real browser, a VM module moved running from member 1 to member 2 with a longest pause of 0.26 s and no reboot.

## 4. Failure handling of a move

| Failure during a move | Behaviour |
|---|---|
| The copy of a volume fails | The instance is started again where it was; the partial copy is deleted |
| The instance move fails | The old volumes are still there; the instance is started where it was |
| The worker stops half way | The job is resumed: it reads where the instance and the volumes are, and continues |
| The instance does not start on the new member | The old volumes are kept; the job fails and says so; the runbook moves it back |

Old volumes are deleted only after the instance runs on the new member.
