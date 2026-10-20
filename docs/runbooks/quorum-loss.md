# The cluster loses quorum

**Evidence:** temporary loss REAL INCUS TESTED, 2026-10-20 (Incus 7.0.1, four members: three
database voters and a standby) — two voters were force-stopped at once and started again.
Permanent loss (recovering from the surviving members) is NOT YET VERIFIED: it rewrites the
cluster's database membership and was not run on the test cluster.

Incus keeps its cluster state in a replicated database. Members with the `database` role vote;
a majority of them must be reachable. Losing the majority (two of three voters) stops every
change to the cluster; the instances keep running.

## What you see

- **Instances keep running** and serving: the test cluster's probe instance answered throughout.
  Service addresses (OVN load balancers) keep forwarding.
- **Apelora reports Incus as unavailable instead of guessing:** API calls that read Incus fail
  with HTTP 502 (`incus_api_error` "context deadline exceeded" after about 5 s, or
  `incus_unreachable` after about 20 s); the Fabric Console and the Studio show the error, not
  stale data. Deployments that are queued wait: the worker's jobs fail and are retried with
  back-off. The replica supervisor skips its passes (it never acts on an inventory it cannot
  read).
- `incus cluster list` on a surviving member hangs.

## Temporary loss (the members come back)

1. Find out why the voters are down (power, network between members, disk) and bring them
   back. Do **not** remove members or force recovery while they may return.
2. The database regains quorum as soon as a majority is back: on the test cluster the API
   answered again and all members were Online within 10 s of the two members booting, with the
   same roles as before.
3. Check: Fabric Console → Membership lists every member Online; Preflight passes; Operations
   shows queued jobs completing. The supervisor resumes on its next pass.

## Permanent loss (the members will not come back) — NOT YET VERIFIED

Only when a majority of voters is lost for good. This changes the cluster's database
membership; take a backup of `/var/lib/incus/database` on the surviving members first.

1. Stop the Apelora worker (so nothing is queued against a cluster being repaired).
2. On every surviving member, stop the Incus daemon.
3. On one surviving voter, run `incus admin cluster recover-from-quorum-loss` (see the Incus
   documentation for the version in `upstream.lock.yaml`), then start its daemon, then the others.
4. Remove the lost members (`incus cluster remove --force <name>`); their instances are gone with
   them. Stateless replicas are recreated by the supervisor once the worker runs again;
   singleton and stateful appliances need their data restored.
5. Add new members through Apelora's enrolment ([member-join.md](member-join.md)) to get back
   to three voters.

Prevention: keep three or more voters in separate failure domains, and keep their clocks and
network healthy — losing one voter is survivable, losing two at once is not.
