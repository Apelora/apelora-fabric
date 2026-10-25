# The cluster loses quorum

Use this page when a majority of the cluster's database members is down.

Incus keeps its cluster state in a replicated database. Members with the `database` role vote,
and a majority of them must be reachable. Losing the majority (two of three voters) stops every
change to the cluster; the instances keep running.

## What you see

- **Instances keep running** and serving. Service addresses (OVN load balancers) keep forwarding.
- **Apelora reports Incus as unavailable instead of guessing:** API calls that read Incus fail
  with HTTP 502 (`incus_api_error` "context deadline exceeded" after about 5 s, or
  `incus_unreachable` after about 20 s). The Fabric Console and the Studio show the error, not
  stale data. Queued deployments wait: the worker's jobs fail and are retried with back-off. The
  replica supervisor skips its passes (it never acts on an inventory it cannot read).
- `incus cluster list` on a surviving member hangs.

## Temporary loss (the members come back)

1. Find out why the voters are down (power, network between members, disk) and bring them
   back. Do **not** remove members or force recovery while they may return.
2. The database regains quorum as soon as a majority is back. Expect the API to answer again,
   and every member to be Online with the same roles as before, within seconds of the members
   booting.
3. Check: Fabric Console → Servers → Members lists every member Online and the cluster database as
   *Majority safe*; Preflight passes; Operations shows queued jobs completing. The supervisor resumes
   on its next pass.

## Permanent loss (the members will not come back)

Only when a majority of voters is lost for good. This changes the cluster's database
membership. First take a backup of `/var/lib/incus/database` on the surviving members.

1. Stop the Apelora worker, so nothing is queued against a cluster being repaired.
2. On every surviving member, stop the Incus daemon.
3. On one surviving voter, run `incus admin cluster recover-from-quorum-loss` (see the Incus
   documentation for the version in `upstream.lock.yaml`), then start its daemon, then the others.
4. Remove the lost members (`incus cluster remove --force <name>`); their instances are gone with
   them. The supervisor recreates stateless replicas once the worker runs again. Singleton and
   stateful modules need their data restored.
5. Add new members through Apelora's enrolment ([member-join.md](member-join.md)) to get back
   to three voters.

Prevention: keep three or more voters in separate failure domains, and keep their clocks and
network healthy. Losing one voter is survivable; losing two at once is not.

## Before you rely on it

- A temporary loss has been tried. Recovering from a permanent loss has not: it rewrites the
  cluster's database membership. Read the Incus documentation for your version before you start.
