# Enrolling a server

A new server goes from known to an Incus member that Apelora can place modules on. An administrator approves each step that changes something, and each step is audited. The operating system can be installed by hand, or by [MAAS](provisioner-maas.md).

Apelora keeps a machine record, issues a single-use join token, and runs an enrolment script on the host. A new member is held out of placement until it has been checked, and then admitted. Fabric Console is where this is done. Removing a member from the cluster is a separate action.

Apelora does not provide PXE, DHCP, or its own power controller, and it does not store controller credentials. Incus places instances. This covers tens of machines, with one administrator acting at a time.

## What is recorded

```mermaid
flowchart LR
  admin[Administrator<br/>Fabric Console] -->|register, approve, admit| api[Control plane]
  api -->|join token, member config| incus[(Incus cluster<br/>admin identity)]
  api --> db[(PostgreSQL<br/>machines, audit)]
  api --> maas[MAAS]
  host[New server] -->|enrol-member.sh, token on stdin| incus
```

**Lifecycle** (derived, never stored as a status string: Incus is the truth for membership):

| State | Meaning | Derived from |
|---|---|---|
| `registered` | known to Apelora, not approved | record without approval |
| `approved` | a join token is waiting for the host | approval recorded and Incus lists the pending token |
| `approval-expired` | the token was used up, revoked or expired without a join | approval recorded, no token, no member |
| `held` | joined; Incus does not place instances on it automatically | member with `scheduler.instance=manual` |
| `eligible` | joined and placed on by Incus | member with any other scheduler setting |
| `left` | was admitted, no longer a member | admission recorded, no member |

**Placement gate.** Incus's member setting `scheduler.instance=manual` excludes a member from
automatic placement while still allowing explicitly targeted instances (REAL INCUS TESTED: six
new instances avoided the held member; a targeted one landed on it). Incus cannot preset it
before a member joins, so the enrolment script sets it right after the join; a member that joins
without it is reported (warning "joined without being held") rather than silently trusted.

**Admission** re-reads Incus and checks the member: online; Incus major version and same
version as the cluster; kernel ≥ 6.12; every storage pool and located network created on it;
architecture (a warning for the first member of a new architecture). OVN chassis and clock are
reported as not checked (the Incus API does not show them). Any failure blocks admission;
admission sets `scheduler.instance` back to the default (key removed). The member update sends
the member's roles, groups and failure domain with the config: Incus treats a missing role list
as dropping roles.

**Provisioners.** `ProvisionerKind` (`manual`, `maas`) with `Capabilities` (discovers,
installs, powers). The manual provider can do none of these: the administrator installs the
OS and runs the enrolment script. The MAAS adapter will implement them and deliver the same
script and token through cloud-init; nothing in the lifecycle above changes.

**Contracts:**

| Call | Effect | Audit |
|---|---|---|
| `GET /api/v1/machines` | machines with derived state, member status, next step, warnings | — |
| `POST /api/v1/machines` `{name, provider, description}` | register (a name that is already a member is adopted) | `machine.register` |
| `POST /api/v1/machines/{name}/approve` | join token, shown once (`Cache-Control: no-store`) with the enrolment command | `machine.approve` (no token) |
| `DELETE /api/v1/machines/{name}/approval` | revoke the pending token | `machine.approval.withdraw` |
| `GET /api/v1/machines/{name}/admission` | admission checks (dry run) | — |
| `POST /api/v1/machines/{name}/admit` | checks, then make eligible; 409 when a check fails | `machine.admit` |
| `DELETE /api/v1/machines/{name}` | forget a machine that is not a member | `machine.remove` |
| `GET /api/v1/cluster/members/{member}/removal` | removal check (dry run): what is left on it, blockers, warnings | — |
| `POST /api/v1/cluster/members/{member}/remove` `{confirm}` | remove an evacuated, empty member for good (the machine becomes `left`) | `cluster.member.remove` |

## When it fails

| Failure | Behaviour |
|---|---|
| Token expires before the host joins | `approval-expired`; approve again |
| Host joins but the script could not hold it | `eligible` with a warning; the administrator can still run the admission checks |
| A check fails | admission refused, nothing changed, the failing checks returned |
| Incus unreachable | 502 like every other Incus call; records unchanged |
| Two administrators approve at once | the second gets 409 (`approval_pending`) |

## Security

A join token is not stored, logged, or audited. The approve response is `no-store`. The enrolment script reads the token from standard input. Every call that changes something needs `manage-infrastructure`, the admin identity, and the database. Controller credentials stay in MAAS. The only credential Apelora handles itself is in the worker's environment, for fencing.

## See also

[Installing a server with MAAS](provisioner-maas.md) · [Bare-metal provisioning](../adr/0002-bare-metal-provisioning-maas.md) · [Join tokens](../adr/0003-incus-identities-and-cluster-join.md) · [Fencing by powering off](../adr/0014-bmc-power-off-fencing.md)
