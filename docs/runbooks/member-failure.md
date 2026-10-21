# A server fails

**Evidence:** REAL INCUS TESTED, 2026-10-20 — a member's VM (a Lima VM standing in for a physical
server) was force-stopped three times on the test cluster with two members and three times with
three members (`demo.sh`, recovery delay 30 s): stateless replicas were substituted and later retired automatically; the service was whole
again 84–206 s after the failure. REAL INCUS TESTED, 2026-10-21 (four members, Incus 7.0.1,
clustered LVM pool `shared` on the lab SAN, `scripts/test-cluster/ha-drill.sh`): a stateful module
on the shared pool restarted on another member 144–155 s after its member's VM was force-stopped (three runs),
only once that member's storage lease was dead, with its data intact (§ Modules on shared
storage). Removing a member that never returns is NOT YET VERIFIED.

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
4. Singletons and stateful modules whose disks are all on the shared pool (volume class
   `data-replicated`) restart on another member once the failed one is **fenced** (next section).
   Everything else on a member-local pool is **not** recovered automatically: it waits for its
   member, or you restore it elsewhere (step 2 below). Servers → Members lists every instance of
   the failed server with what happens to it and why.

## What to do

1. Confirm the failure: Servers → Members shows the member Offline; check the server's power, network
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

## Modules on shared storage

Design: [moving modules](../design/mobility-and-ha.md) §3b, decision: [ADR 0013](../adr/0013-fencing-before-restart.md).
Needs the fence probe on the worker (`APELORA_FENCE_PROBE=ssh`, [settings](../architecture/CONNECTIONS-AND-SCALE.md) connection 22);
without it, only step 4 below fences a member.

What happens, with the times measured by `ha-drill.sh` (t = the VM was force-stopped; three runs
on 2026-10-21, a container module with a 256 MiB volume; run 1 was before the job waited for the
lock in step 6, a fourth attempt was stopped before the failure because of a stale SSH host key):

| Step | What | Run 1 | Run 2 | Run 3 |
|---|---|---|---|---|
| 1 | Incus marks the member Offline (no heartbeat for 20 s) | t+22 s | t+23 s | t+23 s |
| 2 | The supervisor opens the failure (`cluster.member.failed`); Servers → Members shows *Waiting for fencing*: "its storage lease is still being renewed (LIVE)" | t+29 s | t+30 s | t+30 s |
| 3 | sanlock on member 1 sees the lease's renewals stop (FAIL) | t+85 s | t+92 s | t+74 s |
| 4 | A healthy member's sanlock reports the lease DEAD (140 s after its last renewal; its watchdog would have reset it if it were still running). The supervisor records the evidence (`cluster.member.fenced`) and the decision, and queues one `instance.recover` job | t+139 s | t+148 s | t+144 s |
| 5 | The job relinks the instance to another member (`POST /1.0/instances/<name>?target=`; Incus moves the records, nothing is copied) | t+139 s | t+148 s | t+144 s |
| 6 | The start there is refused while *that* member's sanlock still sees the lease held ("LV locked by other host"): each member judges the lease from its own readings. The job waits without using up attempts | 7 s, 3 attempts | waited 5 s | no wait |
| 7 | Running on the other member, the data written before the failure intact (`application.instance.recovered`) | t+146 s | t+155 s | t+144 s |

After the VM was started again (run 3): the member was Online after 15 s, its own lease LIVE with
the next generation after 29 s, the worker had learned it after 76 s (it reads Online members'
leases once a minute), and the failure was closed.

In run 1 Incus also reported the three members that were **not** stopped as Offline for about 10 s
(the heartbeat round was held up by the dead member). Apelora opened failures for them, saw their
leases LIVE, fenced nothing and closed them again: this is why nothing restarts on Incus's word alone.

What to do:

1. Nothing, if Servers → Members shows *Fenced* and the instances as *Restarts on …*. Check the
   application afterwards; the restart is a cold start (a crash from the application's point of view).
2. If it stays *Waiting for fencing* (alert `member-unfenced` after 5 minutes), read the reason:
   - **lease still LIVE**: the server may still be running, cut off from the other servers but not
     from the SAN. Its modules keep running there. Find out; do not confirm unless it is off.
   - **no healthy server answered the probe**: check the probe (`fence-probe.sh status` on the lab;
     the worker log `recovery: the fence probe failed`). On the lab, a VM restarted since the probe
     was set up has new SSH host keys: run `fence-probe.sh apply` again.
   - **never read this server's lease / generation changed**: Apelora cannot prove it; go to step 4.
3. If its lease is FAIL, wait: it becomes DEAD within 140 s of the last renewal.
4. Only when you have checked that the server is switched off (or cut off from the SAN), use
   *Confirm powered off…* on Servers → Members (name typed; `cluster.fence_confirm`, which needs
   another person's approval). The next supervision pass restarts its modules elsewhere.
5. When the server returns: Incus marks it Online; its instances that were restarted elsewhere are
   no longer recorded on it, so Incus does not start them there. Its sanlock rejoins the lockspace at
   boot (`lvmlocks.service`) with a new lease generation, which the worker learns again; Apelora closes
   the failure. Nothing was evacuated, so there is nothing to restore in Incus. Move instances back
   with *Move…* if you want them there.

### A server cut off but still running: fencing through its BMC

[ADR 0014](../adr/0014-bmc-power-off-fencing.md). When the worker has a BMC mapping for the server (`APELORA_BMC_MAP`, [testing without hardware](../guides/testing-without-hardware.md) for the lab's version), a member Offline for `APELORA_BMC_FENCE_AFTER_SECS` (default 30) is switched off through its BMC, its power state is read back, and *Off* is the fence. This is the case the lease cannot settle: a server cut off from the other servers that still reaches the SAN keeps renewing its lease (LIVE) and would never be fenced.

Measured by `HA_DRILL_BMC=1 ha-drill.sh` on 2026-10-21 (REAL INCUS TESTED, the lab's `lima` provider standing in for a BMC; the Redfish provider is CONTRACT VERIFIED only): the member's Incus port was cut with nftables and its VM kept running.

| t | What |
|---|---|
| +22 s | Incus reports the member Offline |
| +29 s | The supervisor opens the failure: *its storage lease is still being renewed (LIVE)* |
| +61 s | After the 30 s grace: power state read (On), power-off sent, **Off read back**: fenced (`cluster.member.power_off`, `cluster.member.fenced`), decision recorded, restart queued |
| +152 s / +215 s | sanlock sees the powered-off host's lease FAIL, then DEAD (about 140 s after its last renewal, which stopped at the power-off) |
| +220 s | The instance runs on another member, the marker file written before the failure intact |

So the BMC fences earlier and in a case nothing else can, but **it does not shorten the restart** on clustered LVM: Incus refuses the start until sanlock expires the dead host's lease. The restart job waits without using up attempts (5 attempts, 26 Incus operations in the drill).

What to know:

- Apelora never switches a server off when: it was Offline for less than the grace, half or more of the members are Offline (more likely the control plane's own network), another server was already switched off by Apelora and is still down (confirm the second one yourself), or it switched this one off in the last 10 minutes. Each refusal is audited once as `cluster.member.power_off` with outcome `denied` and the reason.
- A failed attempt is audited with outcome `failed` and its reason (`apelora_bmc_power_offs_total{outcome}` counts success, failed, refused, unreadable); a person's *Confirm powered off…* still works and wins.
- Before enabling a mapping on real servers, read each BMC's power state with the check described in the ADR (the BMC must report the mapped hardware UUID), and test on one server you can afford to lose.

## A member that will not return (NOT YET VERIFIED)

Apelora does not remove an unreachable member (an evacuated, reachable one: [member-removal.md](member-removal.md)).
Remove it from the cluster with `incus cluster remove --force <member>` on a remaining member
(this deletes the records of its instances), then let the supervisor recreate missing stateless
replicas. Keep at least three database members so the cluster keeps quorum when one is lost.
