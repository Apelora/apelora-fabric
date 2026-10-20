# Network incidents (OVN)

**Evidence:** the OVN central database outage is REAL INCUS TESTED (2026-10-20, Incus 7.0.1,
OVN 25.03, four members; the test cluster runs one central database on member 1). The other
cases are NOT YET VERIFIED.

Apelora's application networks, ACLs, address sets and service addresses are OVN objects that
Incus manages. Each member runs an OVN controller that keeps the flows it has installed.

## The OVN central database is down

What the drill showed (`ovn-central` stopped on the member that runs it, with an application
of a client and two web replicas behind a service address):

- **Running applications keep working:** requests to the service address and connections
  between modules kept succeeding, and names kept resolving (`<instance>.incus`), for the
  whole outage.
- **Changes wait:** a deployment that added a replica during the outage was accepted, but
  creating its instance stalled (Incus waits on OVN for its network interface) and the network
  sync stayed queued. Nothing failed for good.
- **Recovery is automatic:** after `ovn-central` started again, the queued jobs finished within
  10 s, the new replica ran and joined the service.

Steps:

1. Restore the database: on the member(s) running it, `systemctl status ovn-central`, check disk
   space and logs, start it again.
2. Watch Fabric Console → Operations: queued instance and network jobs finish by themselves.
3. If a job ran out of attempts while OVN was down, deploy the application again (same design):
   the plan only creates what is missing.

Prevention: in production run the OVN central database clustered on three members (the test
cluster's single database is a documented shortcut, `scripts/test-cluster/ovn.sh`).

## One member's OVN controller or tunnel is down — NOT YET VERIFIED

Its instances lose connectivity to the other members while the member stays Online in Incus.
The port checks of its service backends may still pass (they run on the same member), so
requests to its replicas can fail. Check `ovs-vsctl show` and `systemctl status ovn-host` on the
member; until fixed, evacuate it (member maintenance runbook) so its replicas move.

## The uplink is unreachable — NOT YET VERIFIED

Traffic between modules (inside OVN) continues; internet access, publishing and anything
routed through the uplink stop. Restore the uplink network; no Apelora action is needed.
