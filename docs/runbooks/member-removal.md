# Remove a server from the cluster for good

Use this when a server is retired, replaced, or reinstalled, and it can still be reached. Empty it, remove what is left on it, then remove the member.

Run on 2026-10-20, Incus 7.0.1, four members. Member 4 was evacuated, checked, and removed through the API. The cluster kept three database members. The machine was approved again, joined with the enrolment script, and admitted. Every step was audited. Removing a database voter from the console, on a real cluster, has not been run.

A server that is down and will not come back is a different case. Apelora does not remove an unreachable member, because Incus can only force that. See [A server fails](member-failure.md).

The picture is sample data from `tests/e2e/screenshots-console-docs.spec.ts`. The names are examples.

## Empty it

Evacuate the member first. See [Maintenance on a server](member-maintenance.md). Incus moves instances away. An instance whose volumes are on this member stops in place and stays there.

## Move or delete what is left

The removal check lists instances still on the member, and application volumes recorded on it. Copy a volume you need to keep:

```sh
incus storage volume export <pool> <volume> <file>
```

Then remove the module from its design and delete the kept volume. See [An application's volumes](volumes.md).

## Check and remove

In Fabric Console, open **Servers → Members** and choose **Remove…** on the evacuated member. The check runs again. Removal is refused while anything is left on the member, while the member is not evacuated or not reachable, or when it is the only member.

Read the warnings. They name a cluster that would keep fewer than three database members, and replicas that would have fewer servers to spread over.

<p align="center">
  <img src="../screenshots/49-console-remove-member-1440x900.png" alt="Remove node-4 from the cluster dialog. Warnings say the cluster would keep 2 database members, 2 members would remain for replicas, and the server must be reinstalled and enrolled to be used again. The name node-4 is typed and Remove member is available." width="720">
</p>

Type the member's name and choose **Remove member**.

The same check and removal from the API. `API` is the control plane, and `TOKEN` is an API token:

```sh
curl -sS -H "Authorization: Bearer $TOKEN" \
  "$API/api/v1/cluster/members/node-4/removal"
```

```sh
curl -sS -H "Authorization: Bearer $TOKEN" -H "Content-Type: application/json" \
  -d '{"confirm":"node-4"}' \
  -X POST "$API/api/v1/cluster/members/node-4/remove"
```

This needs `cluster.manage` and the admin identity. It is audited as `cluster.member.remove`.

## Afterwards

The machine record shows left. Incus resets the removed server to a standalone server. To use it again, approve the machine and enrol it. See [Add a server to the cluster](member-join.md). Reinstall it first when the hardware was replaced or repaired.
