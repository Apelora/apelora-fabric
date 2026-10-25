# A server fails

Use this page when a cluster member goes Offline: what Apelora does on its own, and what you do.

## What happens on its own

1. Incus marks the member *Offline* after about 20 s. Its instances stay listed (status `Error`,
   configuration unknown). Incus does not move them (`cluster.healing_threshold` stays off).
2. Service addresses keep sending a share of requests to the dead member's replicas. The load
   balancer's health checks run on the member that hosts the replica, so they cannot notice that
   it is gone. Expect failed requests until step 3.
3. After `APELORA_RECOVER_AFTER_SECS` (default 120 s), the replica supervisor creates a substitute
   (`<replica>-s<n>`) on another member for every stateless replica stranded there. The next
   network sync replaces the dead backend in the load balancer. This is audited as
   `application.replica.substitute`.

   With Incus 7.0, the load balancer cannot change while the member is offline: Incus refuses
   every load balancer update in the cluster (`peer node … is down`). The substitute then runs but
   gets no traffic, and the service address can keep sending a share of requests to the dead
   replica until the member returns or you remove it (what to do, step 2).
4. Singletons and stateful modules whose disks are all on the shared pool (volume class
   `data-replicated`) restart on another member once the failed one is **fenced** (next section).
   Everything else on a member-local pool is **not** recovered automatically: it waits for its
   member, or you restore it elsewhere (step 2 below). Servers → Members lists every instance of
   the failed server with what happens to it and why.

## What to do

1. Confirm the failure: Servers → Members shows the member Offline. Check the server's power,
   network and Incus service.
2. Decide whether the member will be back soon. If it will not, and service addresses must
   stop sending requests to its replicas, remove it from the cluster with Incus on a remaining
   member: `incus cluster remove --force <member>`. Apelora does not force a removal for you.
   A forced removal deletes the member and the records of every instance on it, so do it only
   after step 3 for anything stateful. Afterwards load balancer updates go through again and the
   substitutes receive the traffic.
3. For single-instance and stateful modules on the member: wait for the member if it will be
   back soon. Otherwise restore them elsewhere from your backups (Apelora never recreates
   stateful data from scratch).
4. When the member returns, Incus starts its instances again. Each replica slot then holds two
   instances; the supervisor keeps the one that puts replicas back on different members and
   deletes the other (`application.replica.retire`).
5. If a load balancer still shows backends as `unknown` in
   `GET /1.0/networks/<network>/load-balancers/<address>/state`, the next network sync re-applies
   its health check; a failed sync is retried after a minute.

## Modules on shared storage

Design: [moving modules](../design/mobility-and-ha.md) §3b, decision: [ADR 0013](../adr/0013-fencing-before-restart.md).
This needs the fence probe on the worker (`APELORA_FENCE_PROBE=ssh`, [settings](../architecture/CONNECTIONS-AND-SCALE.md) connection 22).
Without it, only step 4 below fences a member.

What happens, with typical times after the server stopped:

| Step | What | About |
|---|---|---|
| 1 | Incus marks the member Offline (no heartbeat for 20 s) | 22 s |
| 2 | The supervisor opens the failure (`cluster.member.failed`). Servers → Members shows *Waiting for fencing*: "its storage lease is still being renewed (LIVE)" | 30 s |
| 3 | sanlock on another member sees the lease's renewals stop (FAIL) | 75–90 s |
| 4 | A healthy member's sanlock reports the lease DEAD, 140 s after its last renewal (its watchdog would have reset the server if it were still running). The supervisor records the evidence (`cluster.member.fenced`) and the decision, and queues one `instance.recover` job | 140–150 s |
| 5 | The job relinks the instance to another member (`POST /1.0/instances/<name>?target=`). Incus moves the records; nothing is copied | 140–150 s |
| 6 | The start there may be refused for a few seconds while *that* member's sanlock still sees the lease held ("LV locked by other host"): each member judges the lease from its own readings. The job waits without using up attempts | 0–7 s more |
| 7 | The module runs on the other member, with the data written before the failure intact (`application.instance.recovered`) | 145–155 s |

A virtual machine module restarts the same way, as a cold start: Incus reports it Running about
140–145 s after the failure, and its guest answers some 15–20 s later.

When the server is started again, it is Online within about 20 s. The worker learns its new lease
within a minute or two (it reads Online members' leases once a minute), and the failure is closed.

While one server is failing, Incus can briefly report healthy servers as Offline too. Apelora then
opens failures for them, sees their leases LIVE, fences nothing and closes them again. This is why
nothing restarts on Incus's word alone.

What to do:

1. Nothing, if Servers → Members shows *Fenced* and the instances as *Restarts on …*. Check the
   application afterwards; the restart is a cold start (a crash from the application's point of view).
2. If it stays *Waiting for fencing* (alert `member-unfenced` after 5 minutes), read the reason:
   - **lease still LIVE**: the server may still be running, cut off from the other servers but not
     from the SAN. Its modules keep running there. Find out; do not confirm unless it is off.
   - **no healthy server answered the probe**: check the probe. The worker logs
     `recovery: the fence probe failed`. The probe pins each server's SSH host key, so a server
     whose host key changed is refused until the probe is set up again for it.
   - **never read this server's lease / generation changed**: Apelora cannot prove it; go to step 4.
3. If its lease is FAIL, wait: it becomes DEAD within 140 s of the last renewal.
4. Only when you have checked that the server is switched off (or cut off from the SAN), use
   *Confirm powered off…* on Servers → Members (name typed; `cluster.fence_confirm`, which needs
   another person's approval). The next supervision pass restarts its modules elsewhere.
5. When the server returns: Incus marks it Online. Its instances that were restarted elsewhere are
   no longer recorded on it, so Incus does not start them there. Its sanlock rejoins the lockspace at
   boot (`lvmlocks.service`) with a new lease generation, which the worker learns again, and Apelora
   closes the failure. Nothing was evacuated, so there is nothing to restore in Incus. Move instances
   back with *Move…* if you want them there.

### A server cut off but still running: fencing through its BMC

[ADR 0014](../adr/0014-bmc-power-off-fencing.md). This is the case the lease cannot settle: a server
cut off from the other servers that still reaches the SAN keeps renewing its lease (LIVE), and would
never be fenced.

When the worker has a mapping to the server's baseboard management controller (BMC)
(`APELORA_BMC_MAP`, see the [settings reference](../architecture/SETTINGS.md)), a member Offline for
`APELORA_BMC_FENCE_AFTER_SECS` (default 30) is switched off through its BMC. Its power state is read
back, and *Off* is the fence.

What happens when a server is cut off but keeps running (times after the cut):

| t | What |
|---|---|
| +22 s | Incus reports the member Offline |
| +29 s | The supervisor opens the failure: *its storage lease is still being renewed (LIVE)* |
| +61 s | After the 30 s grace: power state read (On), power-off sent, **Off read back**: fenced (`cluster.member.power_off`, `cluster.member.fenced`), decision recorded, restart queued |
| +152 s / +215 s | sanlock sees the powered-off server's lease FAIL, then DEAD (about 140 s after its last renewal, which stopped at the power-off) |
| +220 s | The instance runs on another member, with the data written before the failure intact |

So the BMC fences earlier, and in a case nothing else can, but **it does not shorten the restart** on
clustered LVM: Incus refuses the start until sanlock expires the dead server's lease. The restart
job waits without using up attempts.

What to know:

- Apelora never switches a server off when: it was Offline for less than the grace; half or more of
  the members are Offline (more likely the control plane's own network); another server was already
  switched off by Apelora and is still down (confirm the second one yourself); or it switched this
  one off in the last 10 minutes. Each refusal is audited once as `cluster.member.power_off` with
  outcome `denied` and the reason.
- A failed attempt is audited with outcome `failed` and its reason
  (`apelora_bmc_power_offs_total{outcome}` counts success, failed, refused, unreadable). A person's
  *Confirm powered off…* still works and wins.
- Before enabling a mapping on real servers, read each BMC's power state with the check described
  in the ADR (the BMC must report the mapped hardware UUID), and test on one server you can afford
  to lose.
- **A BMC without Redfish (IPMI).** Map it as `{ "provider": "ipmi", "host": "<BMC address>", "user": "<operator user>", "secretRef": "APELORA_BMC_SECRET_<NAME>", "uuid": "<system GUID>" }` (`port` defaults to 623). Give the BMC user the operator role, not administrator. The BMC must offer cipher suite 17 or 3; one that offers only weaker suites is refused (`the BMC offers neither cipher suite 17 nor 3`), so use Redfish or MAAS for it. The GUID is the server's system UUID as the operating system reports it (`dmidecode -s system-uuid`); byte-order differences between BMCs are handled. Read it once without switching anything: `APELORA_IPMI_TEST_HOST=… APELORA_IPMI_TEST_USER=… APELORA_IPMI_TEST_UUID=… APELORA_BMC_SECRET_IPMI_TEST=… cargo test -p apelora-deployment-worker --test real_ipmi -- --ignored --nocapture` prints the power state and checks that a wrong GUID is refused.

## A member that will not return

Apelora does not remove an unreachable member (for an evacuated, reachable one, see
[member-removal.md](member-removal.md)).

1. On a remaining member, remove it from the cluster with `incus cluster remove --force <member>`.
   This deletes the records of its instances.
2. Let the supervisor recreate the missing stateless replicas.
3. Keep at least three database members, so the cluster keeps quorum when one is lost.

## Before you rely on it

- The times on this page come from a lab of virtual machines with clustered LVM on a test SAN.
  Your servers and storage will differ; measure them once on your own hardware.
- The Redfish and IPMI providers have been checked against test servers only, not real BMCs.
- Removing a member that will not return, including to unblock its service addresses, has not been tried on a cluster yet.
