# Maintenance on a server

Evacuate the server, do the work, then restore it. Its instances move to other servers, or stop, until you restore it. A container on local storage typically moves in about ten seconds and keeps its addresses.

The names in the pictures are examples.

## Evacuate

In Fabric Console, open **Servers → Members** and choose **Evacuate…** on an Online member.

<p align="center">
  <img src="../screenshots/47-console-evacuate-1440x900.png" alt="Evacuate node-2 dialog. shop-web-01-r2 moves and the shop service stays up on its other replica. shop-db-01 stops on this server until restore. A warning says the shop replicas will share node-1 until restore, and that shop-db-01 is down during the evacuation. The name node-2 is typed and Evacuate member is available." width="720">
</p>

Read the dry run before you type the name.

- **What Incus does** comes from the instance's `cluster.evacuate` mode. `auto` moves it while it is stopped. A volume that cannot move makes Incus stop the instance on this server until restore.
- **Effect.** Replicas elsewhere keep the service up. A single instance, or a stateful module, is down while it moves, or until restore if it only stops.
- **Blocked.** No other member is Online, or none has the instance's architecture. Fix that first. Evacuation is refused while anything blocks it.
- **Warnings.** Replicas will share the remaining servers until you restore this one.

The same dry run from the API. `API` is the control plane, and `TOKEN` is an API token:

```sh
curl -sS -H "Authorization: Bearer $TOKEN" \
  "$API/api/v1/cluster/members/node-2/evacuation"
```

Type the member's name and choose **Evacuate member**.

```sh
curl -sS -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"confirm":"node-2"}' \
  -X POST "$API/api/v1/cluster/members/node-2/evacuate"
```

The member shows Evacuating, then Evacuated. The replica supervisor does not start a replacement for an instance on a member that is not Online.

## Do the work

Updates, a reboot, or hardware. The instances stay where the evacuation put them.

## Restore

On the evacuated member, choose **Restore…**, type its name, and confirm. Incus moves the evacuated instances back and starts them. New instances may be placed on it again.

<p align="center">
  <img src="../screenshots/48-console-restore-1440x900.png" alt="Restore node-2 dialog. The text says Incus moves the evacuated instances back and starts them, and that new instances may be placed on it again. The name node-2 is typed and Restore member is available." width="720">
</p>

```sh
curl -sS -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"confirm":"node-2"}' \
  -X POST "$API/api/v1/cluster/members/node-2/restore"
```

## Check

The member is Online. Each application's instances are on the servers you expect. Its service addresses answer.

Both actions need `cluster.evacuate` (the Fabric Administrator role; for an API token, `manage-infrastructure`). They are audited as `cluster.member.evacuate` and `cluster.member.restore`.

## Move one module first

A virtual machine with **Keep running when moved** (Studio, Availability) moves to another server while it runs. In Fabric Console open **Instances → Move…**, or let evacuation move it. A container, or a virtual machine whose volumes are on its own server, stops for the move: **Move…** with **Allow a short stop** copies the volumes and starts it on the new server.

See [Moving modules and surviving failures](../design/mobility-and-ha.md).
