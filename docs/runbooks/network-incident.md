# Network incidents (OVN)

Use this page when OVN (Open Virtual Network), a member's OVN controller or the uplink fails.

Apelora's application networks, access rules (ACLs), address sets and service addresses are OVN
objects that Incus manages. Each member runs an OVN controller that keeps the flows it has installed.

## The OVN central database is down

What happens:

- **Running applications keep working:** requests to service addresses and connections between
  modules keep succeeding, and names keep resolving (`<instance>.incus`), for the whole outage.
- **Changes wait:** a deployment during the outage is accepted, but creating its instances stalls
  (Incus waits on OVN for their network interfaces), and the network sync stays queued. Nothing
  fails for good.
- **Recovery is automatic:** once the database is back, the queued jobs finish within seconds, and
  new replicas run and join their service.

Steps:

1. Restore the database: on the member(s) running it, `systemctl status ovn-central`, check disk
   space and logs, start it again. On Debian, `ovn-central` does nothing by itself: check and
   restart `ovn-ovsdb-server-nb` and `ovn-ovsdb-server-sb`.
2. Watch Fabric Console → Operations: queued instance and network jobs finish by themselves.
3. If a job ran out of attempts while OVN was down, deploy the application again (same design):
   the plan only creates what is missing.

Prevention: in production, run the OVN central database clustered on three members, with TLS
([install Incus, step 4](../guides/install-incus.md)).

## One member's OVN controller or tunnel is down

Its instances lose connectivity to the other members, while the member stays Online in Incus.
The port checks of its service backends may still pass (they run on the same member), so
requests to its replicas can fail.

1. On the member, check `ovs-vsctl show` and `systemctl status ovn-host`.
2. Until it is fixed, evacuate the member ([member maintenance](member-maintenance.md)) so its
   replicas move.

## The uplink is unreachable

Traffic between modules (inside OVN) continues. Internet access, publishing, and anything
routed through the uplink stop. Restore the uplink network; no Apelora action is needed.

## Before you rely on it

- Only the central database outage has been tried. The controller, tunnel and uplink cases are
  written from how OVN works.
