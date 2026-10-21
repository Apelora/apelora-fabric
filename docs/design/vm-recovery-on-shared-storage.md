# Restarting VM modules after a server dies (planned; waits for the environment)

Status: **planned, nothing built.** The work below is written down now and **not started until the
environment can prove it** (see §6). Related: [ADR 0013](../adr/0013-fencing-before-restart.md),
[mobility and HA](mobility-and-ha.md), [lab shared storage](../development/TEST-CLUSTER.md),
[implementation phases, 08](../development/IMPLEMENTATION-PHASES.md).

Items marked *assumption* were not confirmed with the owner. Nothing here is claimed until it has
been drilled; every result will be labelled MOCK VERIFIED, CONTRACT VERIFIED or REAL INCUS TESTED.

## 1. Where things stand

| | State |
|---|---|
| Live migration for planned moves (a VM moves running, 0.26 s longest pause; containers and data-local VMs stop, copy and start) | Built, REAL INCUS TESTED |
| Stateless replicas replaced on other servers, failing replicas restarted | Built, REAL INCUS TESTED |
| Restart of a failed server's stateful or single-instance **container** modules whose disks are all on the shared pool, after fencing (sanlock lease DEAD, or a person confirms the server is off) | Built; three drills, running again 144–155 s after the server stopped, data intact. REAL INCUS TESTED |
| Same for **VM** modules on the shared pool | **Never drilled** |
| `data-replicated` chosen in the Studio | **Built (2026-10-21)**: one choice per module in Properties → Volumes (all its volumes on the shared pool or all on its server; mixing is refused by the schema and by the API), carried in the plan request and named in the deploy review. MOCK VERIFIED in the browser; whether the pool exists is the control plane's answer at plan time |
| A BMC power-off as fencing evidence | Not built |
| Partition drills (member cut off from the other servers but not from the SAN, and the reverse) | Not run |

A dead server cannot hand over its memory, so this is restart-based recovery: the VM comes back
cold from shared storage, after about 2.5 minutes today. Live migration helps only for planned
moves. As far as we know Xen's HA also restarts VMs after fencing, so parity is realistic
(*assumption*): the difference today is speed and proof, not capability.

## 2. Goal and what is out of scope

**Goal:** when a server dies, VM modules on shared storage restart on another server without
anyone acting, quickly, with evidence, and with no chance of two writers on one disk.

**Out of scope:** seamless failover with no restart, Ceph-specific fencing, failback automation,
and rebalancing (slice C).

## 3. Work packages

Each package says what it needs from the environment (§6) before it can start.

**1. VM drill on the shared pool.** First, because it can overturn the design. ADR 0013 fact 3:
lvmcluster activates VM disks shared (`-asy`), so the lock layer does not stop a second writer of
a VM's disk; the sanlock lease plus the watchdog is the only guard. Add a VM case to
`scripts/test-cluster/ha-drill.sh` using the *Linux base* VM class with its root disk on the
`shared` pool, and check:
- `plan_recovery` and `blocked_by_storage` handle a VM, whose configuration comes back `null`
  from an offline member;
- the relink succeeds with `migration.stateful=true` and its state volume;
- the VM boots cold and its data is intact;
- the start is not refused by a lock held a few seconds longer.
Any gap goes through the usual bug flow: a failing test first, then the fix. Evidence: three runs
like the container drill. *Needs:* the shared-pool lab with VM support (nested virtualisation or
real servers).

**2. Partition and confirmation drills.** The proof that Apelora never writes twice:
- cut a member off from the other servers but not from the SAN: the lease stays LIVE, nothing
  restarts, the `member-unfenced` alert fires;
- cut a member off from the SAN: the watchdog resets it and the lease goes DEAD;
- run *Confirm powered off…* end to end against the lab.
*Needs:* a way to cut network paths per member in the lab (packet filter rules on the lab VMs).

**3. The Studio can choose `data-replicated`. Done (2026-10-21).** The smallest package, and independent UI work.
Add a storage-class choice to the volume editor (`setVolumes`/`volumesOf` in `assembly-model` and
the inspector), refuse mixing with `data-local` the way the API does, and add a Playwright test
and a real-browser check. *Needs:* nothing from the lab; this can be built and verified now.

**4. Faster fencing: a BMC power-off provider.** The real speed lever. Recovery takes about 140 s
today because sanlock's `host_dead_seconds` is the floor. With a power-off fence: Incus marks the
member Offline at 20 s, Apelora powers it off through its BMC, the evidence is "powered off", and
recovery lands around 40–60 s (*estimate, not measured*).
- A `FenceProvider` trait next to the SSH probe in `services/deployment-worker/src/fencing.rs`, a
  new `power-off` evidence kind in `apelora_domain::fencing::decide`, and an ADR 0014 amending 0013.
- BMC credentials stay in the backend's secret store, never in the database, logs or the browser
  (non-negotiable 7).
- The power-off is confirmed by reading the power state back; a "sent" reply is not enough.
- Redfish first, IPMI later. The lab has no BMC, so build a Lima-VM provider that force-stops the
  VM and exercises the same code path; the Redfish provider is CONTRACT VERIFIED until it runs on
  real hardware.
- The most dangerous code in the project: a wrong target powers off a healthy server. It needs the
  same mapping safeguards as the sanlock path: identity learned while Online, a typed or approved
  allowlist, and a refusal if the member is Online.
*Needs:* the answers in §5, and ideally real servers with a BMC.

**5. Optional: fence on the first healthy observer's DEAD reading** (the open item in the design
document). Saves about 9 s; low priority.

## 4. Order and risks

Do 1 and 2 first and together; do 3 in parallel (it needs no lab); do 4 last, because it needs the
hardware answer and an ADR first.

- The VM drill may find that start-after-relink needs a code change. That is why it comes first.
- A BMC fence can power off a healthy server if its mapping is wrong (see 4).
- Not claimed until drilled: anything about VMs.

## 5. Questions that change the design (open)

1. Are there real servers with BMCs (Redfish or IPMI) to test on, or only the Lima lab?
   Recommendation: build against the lab, label Redfish CONTRACT VERIFIED until real hardware exists.
2. What recovery time is wanted? About 2.5 minutes is acceptable for a first release if packages 1–3
   are enough; about one minute needs package 4.
3. Should the BMC fence run automatically or only with a person's approval? Recommendation:
   automatic once an allowlist is configured; anything else makes the night-time case no better
   than today's confirmation path.

## 6. What the environment must provide before this starts

| Package | Needed |
|---|---|
| 1 | The four-member lab with the `shared` lvmcluster pool, VMs able to run on it (nested virtualisation on the lab VMs, or physical servers), the *Linux base* VM class |
| 2 | Per-member network and storage-path cuts in the lab (packet filter rules), the sanlock watchdog working in the lab VMs |
| 3 | Nothing beyond the repository |
| 4 | An agreed answer to §5, a BMC or the Lima provider, a secret store for BMC credentials, ADR 0014 |

The lab (Lima) is not on every developer machine; until it is, packages 1, 2 and 4 stay planned and
the documents keep saying NOT YET VERIFIED. Package 3 and the writing of the design for 4 (ADR 0014,
the failure table, the first slice) do not wait.
