# Maintenance on a server (evacuate, then restore)

**Evidence:** REAL INCUS TESTED, 2026-10-20 — two-member cluster, through the control plane and the
Fabric Console; containers on local storage with OVN networking moved in about ten seconds and
kept their addresses.

1. Fabric Console → Membership → *Evacuate…* on the member (or
   `GET /api/v1/cluster/members/<name>/evacuation`). Read the dry run:
   - **What Incus does** per instance, from its `cluster.evacuate` mode (`auto` moves it while
     stopped, or only stops it if a device cannot move).
   - **Effect**: replicas elsewhere keep a service up; single instances and stateful modules
     are down while they move, or until restore if they only stop.
   - **Blocked**: no other online member, or none with the instance's architecture. Fix that
     first; evacuation is refused while anything blocks it.
   - **Warnings**: replicas that will share the remaining member(s) until restore.
2. Type the member's name and evacuate. The member shows *Evacuating*, then *Evacuated*. The
   replica supervisor leaves instances on a member that is not Online alone.
3. Do the maintenance (updates, reboots, hardware).
4. *Restore…* (type the name). Incus moves the evacuated instances back and starts them.
5. Check: the member is Online; the applications' status shows their instances where expected;
   service addresses answer.

Both actions need `manage-infrastructure` and are audited (`cluster.member.evacuate`,
`cluster.member.restore`).
