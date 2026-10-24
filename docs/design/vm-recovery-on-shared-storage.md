# Restarting a virtual machine after its server stops

When a server stops, a virtual machine whose disks are all on the shared pool is started on another server. It starts cold. The stopped server cannot hand over its memory. A planned move is a different operation: the machine keeps running and is live-migrated.

## What happens

Apelora restarts the machine only after the server is fenced. Fencing is the shared-storage lease read as dead from a healthy server, a person confirming the server is off, or the machine's controller reporting that it is powered off. Time alone never fences it.

Incus then starts the instance on another member. Incus reports the virtual machine as Running when its process is up, which is before the guest agent answers. Anything that needs a command inside the guest waits for that agent.

In the Studio, Properties → Volumes puts every volume of a module on the shared pool, or every volume on the module's own server. Mixing the two is refused. The plan names the choice. Whether the pool exists is answered when the application is planned.

## How long it took in the lab

Three runs on 2026-10-23, Incus 7.0.2, nested virtual machines, not a physical server. Each run force-stopped `apelora-test-2`. The guest was Debian 13, with its root disk and a 256 MiB volume on the shared pool, and with migration while running enabled.

| | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| Server reported offline | 24 s | 25 s | 24 s |
| Lease dead, machine running on the other server | 144 s | 141 s | 144 s |
| File written beforehand read back from the guest | 163 s | 158 s | 164 s |

The same drill for a container, three earlier runs, had the module running again 144–155 seconds after its server stopped. The file was intact.

Powering the server off through its controller does not make this faster on the lab's storage. Clustered LVM still waits for the lease to expire. On 2026-10-21 a cut-off member was fenced at 61 seconds and the instance was running again at 220 seconds. That path does cover a server that can still reach the disks while it is cut off from the other servers, which the lease alone cannot see. It has not been drilled as a network partition.

## What is not covered

These have not been run:

- A server cut off from the others but still able to reach the shared disks, and the reverse.
- Confirming "powered off" from the console against the lab.
- A physical server.

Recovery is a restart. It does not keep the machine running, and it does not move it back when the old server returns.

## See also

[Moving modules and surviving a failure](mobility-and-ha.md) · [When a member fails](../runbooks/member-failure.md) · [Fencing before a restart](../adr/0013-fencing-before-restart.md) · [The lab](../development/TEST-CLUSTER.md)
