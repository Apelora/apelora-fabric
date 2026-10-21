# Remove a server from the cluster for good

**Evidence:** REAL INCUS TESTED, 2026-10-20 (Incus 7.0.1, four members): member 4 evacuated,
checked, removed through the API (the cluster kept three database members), then approved again,
joined with the enrolment script and admitted; every step audited. Removing a member with a
database role and the console flow on the real cluster are NOT YET VERIFIED.

Use this when a server leaves for good (retired, replaced, reinstalled). For a server that is
down and will not come back, see [member-failure.md](member-failure.md): Apelora does not remove
an unreachable member, because Incus can only force that.

1. **Empty it.** Fabric Console → Servers → Members → *Evacuate…* (see
   [member-maintenance.md](member-maintenance.md)). Incus moves instances away; instances whose
   volumes are on this member are stopped in place and stay there.
2. **Move or delete what is left.** The removal check lists instances still on the member and
   application volumes recorded on it. Copy the data of a volume you need
   (`incus storage volume export`), then remove the module from its design and delete the
   kept volume ([volumes.md](volumes.md)).
3. **Check and remove.** Servers → Members → *Remove…* on the evacuated member. The check runs again;
   it is refused while anything is left, while the member is not evacuated or reachable, or when
   it is the only member. Read the warnings (fewer than three database members, fewer places for
   replicas), type the member's name and confirm. Or:
   `GET /api/v1/cluster/members/<member>/removal`, then
   `POST /api/v1/cluster/members/<member>/remove` with `{"confirm": "<member>"}`. Needs
   `cluster.manage` and the admin identity; audited as `cluster.member.remove`.
4. **Afterwards.** The machine record shows *left*. Incus resets the removed server to a
   standalone server. To use it again, approve the machine again and enrol it
   ([member-join.md](member-join.md)); reinstalling it first is the safer choice for hardware
   that was replaced or repaired.
