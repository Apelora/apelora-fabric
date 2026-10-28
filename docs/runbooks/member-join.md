# Add a server to the cluster

Use this page to add a server to a running cluster through Apelora: register it, approve it,
enrol it, then admit it once its checks pass.

Before you start:

- The new server runs the same Incus release as the cluster (see `upstream.lock.yaml`), reaches
  the members' cluster addresses on port 8443, and has not been initialised.
- You have an API token with the `fabric-admin` role.
- The control plane has the unrestricted admin identity (`APELORA_INCUS_ADMIN_CLIENT_CERT/KEY`,
  ADR 0003).

Design: [node enrolment](../design/node-enrolment.md).

1. **Add the machine:** Fabric Console → Servers → *Machines and join tokens* → name (the member name it will
   have) and an optional note, or `POST /api/v1/machines {"name": "<name>"}`. State: *Not
   approved*.
2. **Approve it:** *Approve* (or `POST /api/v1/machines/<name>/approve`). The join token and the
   command to run are shown once; the token is single-use and expires
   (`cluster.join_token_expiry`, keep it at 1 hour or less). State: *Waiting to join*.
3. **Enrol the server:** copy `scripts/enrol/enrol-member.sh` to it and run, as root,
   `bash enrol-member.sh --name <name> --address <its cluster address> --pool-size <size>`.
   `--pool-size 20GiB` is the sized loop pool named `local`: it answers `size` and an empty
   `source`, so Incus creates the loop file. A pool on a disk uses `--member-config <file>`
   instead (one YAML item per key; do not pass both). With OVN, add
   `--ovn-remote tcp:<ovn-db>:6642 --ovn-encap-ip <its address>`. Paste the token when asked
   (it is not echoed; pass it on standard input in automation — never on the command line). The
   script checks Incus ≥ 7.0 and kernel ≥ 6.12, configures the OVN controller, joins, and holds
   the member out of automatic placement (`scheduler.instance=manual`). State: *Joined, not
   admitted*.
4. **Admit it:** *Check and admit…* runs the admission checks (online, Incus and kernel versions,
   architecture, storage pools and networks created on it; OVN and clock reported as not
   checked). Fix any failed check on the server and *Check again*; *Admit server* then lets Incus
   place instances on it. State: *In service*.
5. **With OVN**, confirm that an instance on the new member reaches one on another member. For
   example: `incus launch … --target <name>` on the application network, then `ping` the other
   instance and resolve `<instance>.incus`.

If the server never joins, *Withdraw approval* (the token stops working) or *Remove…* the
machine. Every step is audited (`machine.register`, `machine.approve`,
`machine.approval.withdraw`, `machine.admit`, `machine.remove`); the token itself is never stored,
logged or audited. A member that joined without being held is flagged *joined without being
held*: Incus may already place instances on it, so run the admission checks at once.
