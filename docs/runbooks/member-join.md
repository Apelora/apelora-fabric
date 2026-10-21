# Add a server to the cluster

**Evidence:** REAL INCUS TESTED, 2026-10-20 (Incus 7.0.1) — the fourth test member was
registered, approved, enrolled with `scripts/enrol/enrol-member.sh`, held, checked and admitted
this way; the second and third joined with a bare join token (steps 2–3 without the machine
record).

Prerequisites: the new server runs the same Incus release as the cluster (see
`upstream.lock.yaml`), reaches the members' cluster addresses on port 8443, and has not been
initialised. You need an API token with the `fabric-admin` role, and the control plane needs the
unrestricted admin identity (`APELORA_INCUS_ADMIN_CLIENT_CERT/KEY`, ADR 0003). Design:
[node enrolment](../design/node-enrolment.md).

1. **Add the machine:** Fabric Console → Servers → *Machines and join tokens* → name (the member name it will
   have) and an optional note, or `POST /api/v1/machines {"name": "<name>"}`. State: *Not
   approved*.
2. **Approve it:** *Approve* (or `POST /api/v1/machines/<name>/approve`). The join token and the
   command to run are shown once; the token is single-use and expires
   (`cluster.join_token_expiry`, keep it at 1 hour or less). State: *Waiting to join*.
3. **Enrol the server:** copy `scripts/enrol/enrol-member.sh` to it and run, as root,
   `bash enrol-member.sh --name <name> --address <its cluster address>` — add
   `--member-config <file>` for per-member storage settings and, with OVN,
   `--ovn-remote tcp:<ovn-db>:6642 --ovn-encap-ip <its address>`. Paste the token when asked
   (it is not echoed; pass it on standard input in automation — never on the command line). The
   script checks Incus ≥ 7.0 and kernel ≥ 6.12, configures the OVN controller, joins, and holds
   the member out of automatic placement (`scheduler.instance=manual`). State: *Joined, not
   admitted*.
4. **Admit it:** *Check and admit…* runs the admission checks (online, Incus and kernel versions,
   architecture, storage pools and networks created on it; OVN and clock reported as not
   checked). Fix any failed check on the server and *Check again*; *Admit server* then lets Incus
   place instances on it. State: *In service*.
5. **With OVN**, confirm an instance on the new member reaches one on another member (the lab
   check: `incus launch … --target <name>` on the application network, then `ping` the other
   instance and resolve `<instance>.incus`).

If the server never joins, *Withdraw approval* (the token stops working) or *Remove…* the
machine. Every step is audited (`machine.register`, `machine.approve`,
`machine.approval.withdraw`, `machine.admit`, `machine.remove`); the token itself is never stored,
logged or audited. A member that joined without being held is flagged *joined without being
held*: Incus may already place instances on it, so run the admission checks at once.
