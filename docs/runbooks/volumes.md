# An application's volumes: kept, missing or to be deleted

Use this page to delete, grow, snapshot, back up and restore an application's volumes, and to
deal with volumes that are kept or missing.

What Apelora does on its own:

1. Each volume a module asks for is created for its instance on the member Incus placed the
   instance on (`data-local` volumes live in that member's pool, `APELORA_VOLUME_POOL`), attached
   before the instance starts, and recorded.
2. Removing the module, or the whole application, deletes the instance and **keeps** the
   volume: Fabric Console → Applications → Show details lists it as *Kept, not in use*.
3. Nothing deletes a volume automatically, and a volume without Apelora's labels is never used
   or changed.

Delete a kept volume (its data and snapshots are gone for good):

1. Make sure nobody needs the data. Back it up first if unsure: on the volume's member,
   `incus storage volume export <pool> <volume> <file> --project <project> --target <member>`.
2. Fabric Console → Applications → Show details → *Delete…* next to the volume, type its name.
   Or: `DELETE /api/v1/applications/<name>/volumes/<volume>` with `{"confirm": "<volume>"}`.
   It needs `application.delete` (and an approval where the policy asks for one); it is refused
   while an instance uses the volume, and audited as `application.volume.delete`.

Grow a volume that is filling up (it can stay in use):

1. Fabric Console → Applications → Show details → *Grow…*, enter the new size in GiB. Or:
   `PATCH /api/v1/applications/<name>/volumes/<volume>` with `{"sizeMib": <size>}`. Needs
   `application.deploy`; audited as `application.volume.grow`.
2. Volumes never shrink through Apelora: Incus would accept it and could destroy data. To make
   one smaller, copy the data to a new, smaller volume and delete the old one.

Snapshots (protection against mistakes, not against losing the server: they live in the same pool):

1. Fabric Console → Applications → Show details → *Snapshots…* → *Take snapshot*. While the
   module runs, the snapshot is crash-consistent (what a power cut leaves); stop it first when
   that is not good enough. The Incus project must allow snapshots
   (`incus project set <project> restricted.snapshots=allow`, administrator).
2. To go back: stop the module (Instances → Stop), *Restore…*, type the snapshot's name. Every
   change since the snapshot is lost. Start the module again. Audited as
   `application.volume.restore`; needs `application.operate`.
3. Delete snapshots you no longer need; they use space in the pool.

## Back up a volume and set its schedules

1. Fabric Console → Storage → *Backups…* next to the volume → *Back up now*. A backup is a copy,
   through a short-lived snapshot, into the backup pool (`APELORA_BACKUP_POOL`, set for the
   control plane and the worker) on another member where there is one. Needs
   `application.operate`; the job is `volume.backup`. Backups made this way are kept until
   someone deletes them; scheduled ones are pruned to what the schedule keeps.
2. *Schedules…* sets snapshots and backups on a schedule (every 15 minutes to once a week, keep
   1–50). Needs `application.deploy`. A failed scheduled run shows on the Storage page with its
   reason and is retried within five minutes.
3. To delete one copy: *Delete…* on that row, then type the backup's name. The volume itself
   is unchanged, and the copy cannot be brought back. A copy taken just before a restore is
   the only way back to the data from before that restore; the dialog says so. A copy Incus
   no longer has can still be deleted, which clears Apelora's record of it. Needs
   `application.delete` (and an approval where a policy asks). Audited as
   `application.volume.backup.delete`.

## Restore a volume from a backup

1. **Stop the module** that uses the volume (Applications or Instances → Stop). Apelora never
   overwrites a volume under a running instance.
2. Fabric Console → Storage → *Backups…* next to the volume → *Restore…* on the backup to go back
   to. Read what it shows: the time of the backup (everything written since is replaced), the
   volume's snapshots that will be deleted, the steps. If it says it cannot run now, do what it
   says first.
3. Leave *Take a safety backup of the current data first* on unless the current data is
   worthless or the backup pool has no room: it is the only way back from a mistaken restore.
4. Type the volume's name and choose *Restore volume*. The dialog follows the `volume.restore`
   job; Operations shows it too, with its attempts. Needs `application.operate` (and an approval
   where a policy asks for one).
5. Check the data, then start the module again.

API: `GET /api/v1/applications/<name>/volumes/<volume>/backups/<backup>/restore` (dry run,
`?safetyBackup=false` to see it without the safety backup), then `POST` the same path with
`{"confirm": "<volume>", "safetyBackup": true}` and an `Idempotency-Key`.

If the job fails:

- *nothing was changed: stop … first* — an instance using the volume runs (or was started while
  the safety backup was taken). Stop it and restore again.
- *backup … is not in Incus* — the backup copy was deleted outside Apelora or its member is
  offline (`incus cluster list`).
- A failure while copying — the volume may hold part of the backup. Restore again (a new
  request; failed restores are not retried as they are), or restore the safety backup listed
  as *Safety copy before a restore*.
- *cancelled … before volume … was changed* — the job stopped at its safe point after the safety
  backup; the volume is as it was.

To undo a restore: restore again from the backup marked *Safety copy before a restore*.

A volume whose member is lost cannot be restored in place: the restore needs the volume to exist
on its member. Restoring onto another member is not built yet; copy the backup by hand with
`incus storage volume copy` (administrator) and redeploy.

## Volumes no application uses

Storage → *Volumes no application uses* lists custom volumes in Apelora's projects that no
application or backup record refers to (left behind outside Apelora, or created by Apelora for an
application that no longer records them), with their size. Apelora does not delete them. Check
the reason shown and the audit history, then delete them in Incus if nobody needs them.

A volume shows *Not found in Incus*:

1. Its member may be offline: `incus cluster list`. When the member returns, the volume returns.
2. Otherwise it was deleted outside Apelora. Check the audit history and the Incus logs; if the
   data is gone, deleting the record from the console only forgets it.

A stateful module on a member under maintenance: evacuating the member **stops** the instance
in place (Incus does not move a member-local volume) and restoring the member starts it again.
Plan the downtime; see [member-maintenance.md](member-maintenance.md).

## Before you rely on it

- A volume shown *Missing*, and a server outage with a volume on it, have not been tried yet.
- Deleting a backup copy, and the restore's dry run and dialog, have been tested with simulated
  data only; the restore itself has run on a lab cluster. Try a restore on a test volume first.
