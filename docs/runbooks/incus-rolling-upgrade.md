# Reading quorum, and a rolling Incus upgrade

**Evidence:** the readings (Servers → Members and Upgrades, `GET /api/v1/cluster/members`) are
REAL INCUS TESTED read-only, 2026-10-21 (Incus 7.0.1, four members: three voters and a stand-by).
The upgrade procedure itself is **NOT YET VERIFIED**: it is written from the Incus documentation
for the pinned release and our design, and has not been run on the test cluster.

Apelora does not install or upgrade Incus. It shows where the cluster stands, previews each
evacuation and audits it; the package upgrade happens on each server.

## Reading quorum (Servers → Members → Cluster database)

Incus keeps its cluster state in a replicated database. Members with the `database` role are
**voters**; `database-standby` members keep a copy and are promoted when a voter is lost; the rest
are **spares**. Changes need a majority of voters (two of three, three of five).

| Field | Meaning |
|---|---|
| Voters answering | Voters whose daemon answers: Online, Evacuating, Evacuated or Restoring. Offline and Blocked members are not counted, so the figure errs on the cautious side. |
| Needed | The majority: half the voters, rounded down, plus one. |
| Voters it can lose | How many voters can go down at once, now, before every change stops. |
| Stand-bys answering | Stand-bys Incus can promote. A promotion raises the figure above again after a loss. |
| Leader | The member leading the database. Losing it costs a short election, not data. |

States: **Majority safe** (at least one voter to spare), **One loss from stopping** (exactly a
majority answers: do no maintenance on voters), **No majority** (see [quorum-loss.md](quorum-loss.md)),
**Single server**, **Unknown** (no voters reported). Warnings name voters that share one failure
domain, a missing leader, or fewer than three voters with three or more members.

## Rolling upgrade (Servers → Upgrades)

The Upgrades tab compares every member's release (`GET /1.0?target=<member>`) with the pin in
`upstream.lock.yaml`, and lists what must be fixed first.

Before you start:
1. No blockers on the Upgrades tab: every member answers (Incus cannot upgrade a cluster with
   offline members), and Members shows *Majority safe*.
2. Read the release notes of the target release, and update the pin through
   `scripts/upstream/prepare-update.sh` ([UPSTREAM-INCUS.md](../development/UPSTREAM-INCUS.md)).
3. Back up Apelora's database ([apelora-database-backup.md](apelora-database-backup.md)).

For each member, spares and stand-bys first, the database leader last:
1. **Evacuate** it from Servers → Members after reading the dry run
   ([member-maintenance.md](member-maintenance.md)).
2. **Upgrade** the Incus package on that server and restart the Incus daemon.
3. **Wait** until the member is Online again and Upgrades shows the new release for it.
   - **Blocked** on the upgraded member (or on the others) means the new release changes the
     cluster database or API. Blocked members stop answering the API until every member runs the
     new release; their instances keep running. Upgrade the remaining members without waiting,
     evacuating each first only while the cluster still accepts changes.
4. **Restore** the member from Servers → Members, and check that its instances are back.
5. Take the next member.

Afterwards: Upgrades shows *Up to date*, Preflight passes, and Operations shows queued jobs
completing.

## When a step fails

- **The member does not come back Online:** leave it evacuated, read its Incus log, and do not
  start on another voter while Members says *One loss from stopping*.
- **Two voters down at once:** the cluster stops accepting changes; follow
  [quorum-loss.md](quorum-loss.md).
- **A member stays Blocked after all are upgraded:** compare the releases on Upgrades; a member
  that still runs the old release is the one to upgrade.
