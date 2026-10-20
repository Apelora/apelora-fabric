# An application's volumes: kept, missing or to be deleted

**Evidence:** REAL INCUS TESTED, 2026-10-20 (Incus 7.0.1, four members, btrfs pool `local`):
an application with two volumes deployed through the control plane and worker; one appliance
removed from the design (its volume kept, shown *Kept, not in use*); that volume deleted from the
Fabric Console with its name typed; the remaining volume deleted after the application was
removed. The *Missing* case and a member outage with a volume on it are NOT YET VERIFIED.

What Apelora does on its own:

1. Each volume an appliance asks for is created for its instance on the member Incus placed the
   instance on (`data-local` volumes live in that member's pool, `APELORA_VOLUME_POOL`), attached
   before the instance starts, and recorded.
2. Removing the appliance, or the whole application, deletes the instance and **keeps** the
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

A volume shows *Not found in Incus*:

1. Its member may be offline: `incus cluster list`. When the member returns, the volume returns.
2. Otherwise it was deleted outside Apelora. Check the audit history and the Incus logs; if the
   data is gone, deleting the record from the console only forgets it.

A stateful appliance on a member under maintenance: evacuating the member **stops** the instance
in place (Incus does not move a member-local volume) and restoring the member starts it again.
Plan the downtime; see [member-maintenance.md](member-maintenance.md).
