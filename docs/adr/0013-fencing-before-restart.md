# ADR 0013 — Fencing before a module restarts on another server

Status: **accepted; built (HA slice B)** · Date: 2026-10-21 · Design: [moving modules and surviving failures](../design/mobility-and-ha.md) §3b · Runbook: [a server fails](../runbooks/member-failure.md)

## Context

A module whose disks are on shared storage (the `lvmcluster` pool on a SAN, or distributed storage) can run on any server. When its server fails, Incus can start it on another one without copying anything. Doing that while the failed server is still writing to the same disk destroys the data: two kernels mount one file system, or two virtual machines write one block device.

Three facts, read in the pinned Incus source (v7.0.2) and on the lab, shape the decision:

1. **Incus's own healing** (`cluster.healing_threshold`, default 0 = off) runs on the cluster leader once a minute. It takes every member whose last heartbeat is older than the threshold, pings its address once (an answer skips it), and then moves *every* instance on that member whose pool is remote, in every project, and starts the ones that were running. The member is then marked Evacuated. It knows nothing about the shared storage's locks (`cmd/incusd/api_cluster_evacuation.go`, `autoHealClusterTask`, `healClusterMember`).
2. **The primitive healing uses is open to Apelora:** `POST /1.0/instances/<name>?target=<member>` with `{"migration": true}` on an instance whose member is offline and whose pool is remote relinks the instance's records to the target and imports its volumes there (`instance_post.go`, `migrateInstance`, "migration of an instance away from an offline server"). Incus refuses it for a member-local pool, and refuses a cluster *group* as the target while the source is offline (the request is forwarded to the target, which must be a member). The placement scriptlet still validates the chosen member.
3. **The clustered LVM driver activates volumes in two modes** (`driver_lvm_utils.go`, `activateVolume`): container file systems exclusively (`lvchange -aey`), and **virtual-machine disks and block volumes shared** (`-asy`, so they can move live). An exclusive activation fails while a live host holds the lock; a shared one does not. So the lock layer stops a second writer of a container's disk, but **not** of a virtual machine's.

What *does* stop a failed host from writing is sanlock's host lease. Every member renews its delta lease in the shared volume group's lockspace every 20 s; a host that cannot renew is reset by its watchdog (wdmd) before other hosts treat its lease as expired. sanlock reports the remote host as `DEAD` once its lease has not been renewed for `host_dead_seconds = id_renewal_fail_seconds + watchdog_fire_timeout` — 8 × `io_timeout` + 60 s = **140 s** with the defaults (`io_timeout` 10 s). Sources: sanlock(8) "the direct link between delta lease renewals and watchdog renewals … sanlock knows the time the watchdog on another host has fired based on the delta lease time"; lvmlockd(8) "sanlock lease storage failure"; sanlock `src/timeouts.h` for the formula. A host that is partitioned from the other servers but still reaches the SAN keeps renewing, so its lease stays `LIVE`.

## Decision

1. **Apelora restarts an instance on another server only when all of these hold:**
   - Incus reports its member `Offline`;
   - the member is **fenced** (2.);
   - Apelora deployed it (its deployment record), it is not a stateless replica (those get substitutes, already built), and its root disk **and** every volume are on a shared pool (driver `lvmcluster` or `ceph`) — declared by the module's volumes having the class `data-replicated`, which also puts its root disk on the shared pool. Anything on a member-local pool waits for its server or a restore.
2. **Fenced means one of:**
   - **`storage-lease`** (automatic, clustered LVM): sanlock, read on a healthy member, reports the failed member's host lease `DEAD` in the shared volume group's lockspace, with the same lease generation Apelora last saw `LIVE` while that member was Online. The read is at most 60 s old.
   - **`confirmed`**: a person confirms the server is powered off (its name typed, permission `cluster.fence_confirm`, which needs another person's approval). Valid until the member is Online again.
   - (Later) **`power-off`** through a fence provider that switches the server off through its BMC.
   Time alone never counts, and neither does "Incus cannot reach it".
3. **The lockspace is read through a narrow probe, not an agent:** an SSH forced command. Each member has an unprivileged account (`apelora-fence`, group `sanlock`, no password, no shell access) whose only authorised key runs exactly `sanlock client gets -h 1` (`restrict,command=…`). The worker holds the private key (`APELORA_FENCE_PROBE_KEY`, outside the repository) and pins every member's host key. Without the probe, only confirmation fences. The probe is read-only: the command lists lockspaces and host states.
4. **Incus's healing stays off.** It acts on every remote-pool instance of every project with no fencing input but one ping; Apelora could neither apply its policy nor record evidence. Apelora calls the same Incus primitive per instance instead, as a worker job, after fencing.
5. **Apelora names the target member** (Incus requires a member while the source is offline): an Online member, not held out of placement, with the failed member's architecture, with the fewest instances of the same module, then the fewest instances. Incus's placement scriptlet can still refuse it.
6. **One decision per failure:** the supervisor leader records the failure, the evidence and the decision in PostgreSQL (`member_failures`, migration 0028), audits them, and queues one `instance.recover` job per instance with an idempotency key bound to the failure.

## Consequences

- A failed server's stateful modules on the SAN run again about **140 s + one supervision pass** after its last lease renewal, without anyone acting; measured on the lab in [the runbook](../runbooks/member-failure.md#modules-on-shared-storage).
- Each member's sanlock judges a lease dead from its own readings, so the member that restarts an instance may still hold the lease for live a few seconds after another member reported it DEAD (5–7 s measured). The storage lock refuses the start meanwhile; the job waits for it without using up attempts.
- A partitioned server that still reaches the SAN keeps its modules; nothing restarts elsewhere, which is the safe outcome. People see why on Servers → Members.
- Each member gets one more account and one more authorised key; the worker gets one more secret and needs to reach each member's SSH port. Hosts without the probe still get confirmation-based recovery.
- Ceph pools have no sanlock lease: they recover only after confirmation (or a fence provider), until a Ceph-specific check is added.
- A mapping mistake cannot fence the wrong server: the host id is learned from each member's own lockspace line while it is Online, and a lease generation that changed is not accepted.

## Alternatives considered

- **Turn on `cluster.healing_threshold`** with a threshold above 140 s. Simple, and works without Apelora. Rejected for the reasons in decision 4; it also marks the member Evacuated, so its return needs a restore that moves instances back.
- **Time only** (offline for longer than 140 s + margin). Wrong for virtual machines on clustered LVM (shared activation) and for a server partitioned from the cluster but not from the SAN.
- **A host agent** on each member reporting lock state. More moving parts and a privileged daemon (AGENTS.md needs an ADR for that); the forced command gives the same reading with less.
- **Ask a person every time.** Kept as the fallback, but slow at night.
