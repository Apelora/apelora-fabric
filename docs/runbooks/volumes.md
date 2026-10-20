# An application's volumes: kept, missing or to be deleted

**Evidence:** REAL INCUS TESTED, 2026-10-20 (Incus 7.0.1, four members, btrfs pool `local`):
an application with two volumes deployed through the control plane and worker; one module
removed from the design (its volume kept, shown *Kept, not in use*); that volume deleted from the
Fabric Console with its name typed; the remaining volume deleted after the application was
removed. The *Missing* case and a member outage with a volume on it are NOT YET VERIFIED.

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
   `application.deploy`; audited as `application.volume.grow`. REAL INCUS TESTED on a running
   container (filesystem volume, btrfs).
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
3. Delete snapshots you no longer need; they use space in the pool. REAL INCUS TESTED
   (2026-10-21, through the console against the cluster).

A volume shows *Not found in Incus*:

1. Its member may be offline: `incus cluster list`. When the member returns, the volume returns.
2. Otherwise it was deleted outside Apelora. Check the audit history and the Incus logs; if the
   data is gone, deleting the record from the console only forgets it.

A stateful module on a member under maintenance: evacuating the member **stops** the instance
in place (Incus does not move a member-local volume) and restoring the member starts it again.
Plan the downtime; see [member-maintenance.md](member-maintenance.md).
