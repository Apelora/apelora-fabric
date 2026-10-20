# Add a server to the cluster

**Evidence:** REAL INCUS TESTED, 2026-10-20 — second and third members joined the test cluster
this way (Incus 7.0.1).

Prerequisites: the new server runs the same Incus release as the cluster (see
`upstream.lock.yaml`), reaches the members' cluster addresses on port 8443, and has not been
initialised. You need an API token with the `fabric-admin` role, and the control plane needs the
unrestricted admin identity (`APELORA_INCUS_ADMIN_CLIENT_CERT/KEY`, ADR 0003).

1. Choose the member name the server will use (letters, digits, hyphens).
2. Approve it: Fabric Console → Membership → *Approve a new member*, or
   `POST /api/v1/cluster/join-tokens {"memberName": "<name>"}`. The token is shown once; it is
   single-use and expires (`cluster.join_token_expiry`, keep it at 1 hour or less).
3. On the new server, give the token to `incus admin init` (cluster join). Pass it on standard
   input or through a preseed file with restrictive permissions — never on the command line.
4. Check: Membership lists the member as Online; *Waiting to join* no longer lists the token;
   Preflight passes the member checks.
5. If OVN is used, install the OVN controller on the server and point it at the OVN database
   (the test cluster's `scripts/test-cluster/ovn.sh` shows the settings), then confirm the
   server appears as an OVN chassis.

If the token was not used, revoke it (Membership → *Revoke*). Every approval and revocation is in
the audit log (`cluster.join-token.*`); the token itself is never stored or logged.
