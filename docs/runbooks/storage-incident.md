# A storage problem: a pool filling up, a pool that is down, or backups that stop

Use this page when a storage pool fills up or becomes unavailable, when the shared pool
misbehaves, or when backups stop.

Apelora does not manage Incus's storage drivers. It asks Incus to create, grow, snapshot and
delete volumes in the pools it is told to use (`APELORA_VOLUME_POOL`, `APELORA_SHARED_POOL`, the
backup pool) and shows what Incus reports. When a pool is unhealthy the repair happens in Incus or
on the server; Apelora's part is to show it, refuse to make it worse, and recover the workloads.

## Find out what is wrong

1. **Fabric Console → Health → Servers.** Each server shows pool usage and warnings. A pool near
   its limit is a warning; an unusable pool shows as such with Incus's reason.
2. **Fabric Console → Storage.** Pools with use, the volumes each application holds (*In use*,
   *Kept, not in use*, *Missing*), schedules and backups, and custom volumes no application uses.
3. **Fabric Console → Alerts** for *failed backups*, *old backups* and *an old restore drill*.
4. **Fabric Console → Operations** for failed jobs that mention a volume, a pool or a quota. The
   job page names the Incus operation and its error ([troubleshooting.md](troubleshooting.md)).
5. **Preflight** fails while the pool for volumes is missing or not ready, and Apelora then refuses
   deployments that need it instead of guessing.

## A pool is filling up

Do the cheap, reversible things first, and never delete a volume you have not backed up.

1. Look at *Storage → Volumes no application uses* and the *Kept, not in use* volumes: these are
   volumes left behind by removed modules. Back one up if unsure, then delete it with its name
   typed ([volumes.md](volumes.md), *Delete a kept volume*).
2. Check snapshots. Scheduled snapshots are pruned to what the schedule keeps; manual ones never
   are. Delete manual snapshots that are no longer needed (Snapshots dialog).
3. Move a workload to a server with room: *Instance → Move* (dry run first). A module with a
   `data-local` volume stops while its volumes are copied; check the dry run says so
   ([member-maintenance.md](member-maintenance.md)).
4. Add capacity in Incus (a bigger disk, another pool member). Apelora does not do this.
5. Do not grow volumes to "fix" a full pool: growing consumes more of it.

A full pool shows up first as failed jobs (a volume or instance that cannot be created) and
applications that stay *Degraded* because a replica cannot be recreated. Once there is room the
supervisor retries on its own; check the job list for anything left failed and retry it from the
job page.

## A pool or its disks are unavailable on one server

1. Treat it as a failing server for the workloads on it: [member-failure.md](member-failure.md).
   Stateless replicas are substituted on other servers on their own. Single-instance and
   stateful modules are recovered automatically **only** when all their disks are on the shared
   pool and the failed server has been fenced (*Modules on shared storage* in that runbook);
   otherwise they wait for you.
2. Do not delete the volumes of a module whose server is down. A volume shown *Missing* may only
   mean the server cannot be read right now.
3. Repair the pool in Incus (`incus storage show <pool> --target <member>`, the driver's own tools).
   When the server is back, *Storage* should show the volumes again; anything still *Missing*
   afterwards is a real loss: restore it from a backup.

## The shared pool (lvmcluster with sanlock) misbehaves

This is the pool that lets a failed server's modules restart elsewhere. Apelora restarts a module
only after the server is fenced: Incus reports it offline **and** its storage lease reads DEAD
through the read-only fence probe, or another person confirms the server is powered off. If the
probe cannot be read, nothing restarts, by design.

1. Health → Servers shows *Failed servers* and says what restarts where or why it waits.
2. If it waits because the lease is not DEAD, the server may still be writing: do not confirm
   "powered off" until it really is (power it off at the BMC or the wall, then confirm).
3. Check the storage network and the SAN or iSCSI target from the other servers. Losing the
   storage network on every server is a cluster-wide storage outage: stop deploying, leave
   running workloads alone, and restore the network before anything else.

## Backups stop or are old

1. *Alerts* names which volume and whether the last attempt failed or is simply old.
2. A failed backup job (Operations) shows Incus's error. Common causes: the backup pool is full,
   the backup pool's server is down (backups are copied to **another** member), or the volume was
   busy beyond the copy-wait.
3. Fix the cause, then *Storage → Back up now* for that volume. Do not rely on snapshots for
   this: they live in the same pool as the data.
4. Apelora's own database is separate: [apelora-database-backup.md](apelora-database-backup.md).

## Restore a volume

1. Read [volumes.md](volumes.md) *Restore a volume from a backup* first. It is a worker job with a
   dry run, the volume's name typed, a safety backup first, and it is refused while an instance
   using the volume runs.
2. Restore into the same module when the data is only damaged; restore from a backup copy on the
   other member when the original pool is gone.
3. Start the module and check the application's own data, not only that it is running.

## After the incident

- Note what filled or failed and why, and add an alert threshold or schedule that would have
  caught it earlier (Alerts → rules have editable thresholds).
- Run a restore drill for the affected volume class; the *old restore drill* alert exists so this
  does not go stale.
- If the procedure above was wrong or missing a step, correct it here.

## Before you rely on it

- The operations this page points to (grow, snapshot, backup, restore, delete) work on their own,
  as [volumes.md](volumes.md) describes. The incident procedure as a whole has not been rehearsed
  yet; it follows Apelora's features and the Incus documentation. Follow it with care.
