# A server fails

**Evidence:** REAL INCUS TESTED, 2026-10-20 — a member's VM was force-stopped three times on the
two-member test cluster and three times on the three-member cluster (`demo.sh`, recovery delay
30 s): stateless replicas were substituted and later retired automatically; the service was whole
again 84–206 s after the failure. Removing a member that never returns is NOT YET VERIFIED.

## What happens on its own

1. Incus marks the member *Offline* after about 20 s. Its instances stay listed (status `Error`,
   configuration unknown). Incus does not move them (`cluster.healing_threshold` stays off).
2. Service addresses keep sending a share of requests to the dead member's replicas: the load
   balancer's health checks run on the member that hosts the replica, so they cannot notice it
   is gone. Expect failed requests until step 3.
3. After `APELORA_RECOVER_AFTER_SECS` (default 120 s) the replica supervisor creates a substitute
   (`<replica>-s<n>`) on another member for every stateless replica stranded there, and the next
   network sync replaces the dead backend in the load balancer. Audited as
   `application.replica.substitute`. Incus sometimes refuses load balancer updates for the
   whole outage (`peer node … is down`); the sync is retried, and in the one run where it never
   got through the service still became whole after about 3.5 minutes.
4. Singletons and stateful modules are **not** recovered automatically. The supervisor logs
   them; decide yourself (step 2 below).

## What to do

1. Confirm the failure: Membership shows the member Offline; check the server's power, network
   and Incus service.
2. For single-instance and stateful modules on the member: wait for the member if it will be
   back soon. Otherwise restore them elsewhere from your backups (stateful data is never
   recreated from scratch by Apelora).
3. When the member returns, Incus starts its instances again. Each replica slot then holds two
   instances; the supervisor keeps the one that puts replicas back on different members and
   deletes the other (`application.replica.retire`).
4. If a load balancer still shows backends as `unknown` in
   `GET /1.0/networks/<network>/load-balancers/<address>/state`, the next network sync re-applies
   its health check; a failed sync is retried after a minute.

## A member that will not return (NOT YET VERIFIED)

Apelora does not remove an unreachable member (an evacuated, reachable one: [member-removal.md](member-removal.md)).
Remove it from the cluster with `incus cluster remove --force <member>` on a remaining member
(this deletes the records of its instances), then let the supervisor recreate missing stateless
replicas. Keep at least three database members so the cluster keeps quorum when one is lost.
