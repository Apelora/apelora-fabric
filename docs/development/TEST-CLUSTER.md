# Disposable Incus test cluster

The **only** Incus that Apelora's tests may change. It runs in a local VM on a developer workstation, holds nothing of value, and is deleted and recreated at will. Results produced against it are labelled **REAL INCUS TESTED**.

## What it is

| Part | Choice | Why |
|---|---|---|
| VM | [Lima](https://lima-vm.io) (`brew install lima`), Apple Virtualization (`vz`), 4 CPU, 8 GiB, 40 GiB | Local, scriptable, deleted with one command |
| Guest | Debian 13 cloud image, digest-pinned by Lima's template | Kernel 6.12, the Incus 7.0 minimum |
| Incus | 7.0 LTS from the packaging repository named in Incus's installation guide; the signing key's fingerprint is checked before use | Our pinned series (`upstream.lock.yaml`) |
| Topology | One member with clustering enabled | Exercises the cluster API paths; more members come with Phase 08 |
| Storage / network | `local` (btrfs, 20 GiB loop file), `incusbr0` (NAT bridge) | Defaults that need no host setup |
| Project | `apelora`, `restricted=true`, own `default` profile (root disk + `eth0`) | The deploy project Apelora is allowed to change |
| Client identities | ECDSA P-384, X.509 v3, client-auth only, 90 days. `client`: **restricted to the `apelora` project** (control plane and worker). `admin`: unrestricted, control plane only, for join tokens ([ADR 0003](../adr/0003-incus-identities-and-cluster-join.md)) | Least privilege: deployments cannot see or touch anything else |
| Join tokens | `cluster.join_token_expiry=30M` | Short-lived approvals |
| Nested virtualisation | On (Apple M3 or later, macOS 15 or later) | Lets the test cluster run QEMU virtual machines |

No host folders are shared with the VM and no guest ports are forwarded. Each VM has two networks: the host reaches the Incus API on the VM's NAT address (`lima0`), and members talk to each other over Lima's `user-v2` network (`eth0`), because the NAT network isolates VMs from each other (found when the second member could not reach the first).

## Use

```sh
brew install lima                       # once
bash scripts/test-cluster/up.sh         # create or restart; safe to repeat
source ~/.config/apelora/test-cluster/env
```

The `env` file sets `APELORA_INCUS_URL`, the certificate paths (deploy and admin identities), `APELORA_INCUS_TLS_SERVER_NAME` and `APELORA_DEPLOY_PROJECTS`. Give the worker only the deploy identity: unset `APELORA_INCUS_ADMIN_CLIENT_CERT`/`KEY` in its environment. The certificates and the key live in `~/.config/apelora/test-cluster` (mode 0700, files 0600), never in the repository.

Then any of:

```sh
cargo run -p apelora-control-plane            # API against the test cluster
DATABASE_URL=… cargo run -p apelora-deployment-worker
DATABASE_URL=… cargo test -p apelora-deployment-worker --test real_incus -- --ignored --test-threads 1
```

The `real_incus` tests are ignored by default and refuse to run against the mock. They create uniquely named instances (`apl-it-…`) in the deploy project and delete them again. `unowned_instances_are_refused` needs an instance that Apelora did not create:

```sh
limactl shell apelora-incus sudo incus launch images:debian/13 not-apeloras --project apelora
APELORA_TEST_UNOWNED_INSTANCE=not-apeloras cargo test … unowned -- --ignored
limactl shell apelora-incus sudo incus delete --force not-apeloras --project apelora
```

Every catalog runtime (requests generated from the catalog itself):

```sh
npm run -s runtime-requests -w @apelora/appliance-catalog > /tmp/requests.json
APELORA_TEST_CATALOG_REQUESTS=/tmp/requests.json cargo test -p apelora-deployment-worker --test real_incus -- --ignored catalog_runtimes
```

Add a member (another VM), approved through Apelora: a fabric administrator asks for a join token and the token goes straight to the new VM over standard input:

```sh
curl -s -X POST -H "Authorization: Bearer $APELORA_TOKEN" -H 'Content-Type: application/json' \
  -d '{"memberName":"apelora-test-2"}' http://127.0.0.1:8080/api/v1/cluster/join-tokens |
  jq -r .token | bash scripts/test-cluster/add-member.sh 2
```

Add OVN (optional, after `add-member.sh`) so network intents can be enforced:

```sh
bash scripts/test-cluster/ovn.sh
```

It installs the OVN database on the first member and the OVN controller on every member (tunnels over the members' private network), points Incus at it, gives `incusbr0` uplink ranges, and recreates the empty `apelora` project with `features.networks=true` and `restricted.networks.uplinks=incusbr0`, so the restricted deploy identity owns its OVN networks and ACLs. A shared `apelora-default` OVN network backs the project's default profile. Test-only shortcuts: one OVN database (OVN itself not highly available) without TLS on the private network.

With OVN in place, start the control plane with `APELORA_NETWORK_UPLINK=incusbr0` to enforce application connections.

Remove everything (the VMs and the credentials):

```sh
bash scripts/test-cluster/down.sh
```

## Things the real cluster taught us

- **The server certificate names the member's host name** (and loopback), not the address clients dial. `up.sh` reads the name from the certificate and sets `APELORA_INCUS_TLS_SERVER_NAME`; the adapter still pins the exact certificate.
- **Some `openssl req -x509` versions issue X.509 v1 certificates** when no extensions are given, and TLS libraries reject them. `up.sh` issues a v3 client-auth certificate and replaces a v1 one (and its trust entry); the adapter reports a v1 client certificate by name.
- **`"profiles": []` means no profiles**, and so no root disk; only an absent list gets the `default` profile. The adapter omits an empty list.
- **OCI image fingerprints are Incus's own**, not the registry's manifest digest. Pin an OCI image with a digest in the alias (`repository@sha256:…`); the `fingerprint` field is for simplestreams images.
- A restricted client certificate can still read cluster members, managed networks and storage pools, so inventory and preflight work with least privilege; it sees only its own projects.

## Limits

- Two members at most so far, both aarch64: no failover, mixed-architecture or cross-member networking evidence yet.
- The packaging repository may ship a different 7.0.x than `pinned_tag`; `up.sh` prints the running version, and evidence names the version it was produced with.
- Clock drift and member latency are not measurable through the Incus API; preflight reports them as not checked.
- Public image servers keep only recent builds: a fingerprint pin of an image they have dropped fails to download, and the job retries until it gives up. Pin images from a server you control (or mirror them) for anything that must keep working.
- **OVN findings (Incus 7.0.1, OVN 25.03, two members):** instances on different members reach each other by name (`<instance>.incus`) on an OVN network. ACL rules whose source or destination is *another ACL* (subject selectors) work only between instances on the same member: across members the destination member rejects the connection although `ovn-trace` allows it (`ovn-monitor-all` did not change that). Rules on IP addresses (and so address sets) work across members. Once an ACL is on a NIC, unmatched traffic is rejected in both directions, so consumers need egress rules; a consumer's egress allow and a provider's ingress allow combine with OR, so Apelora writes both sides; setting a NIC's default egress to allow overrides the provider's ingress rejection and must not be used.
- A spec that lists `architectures` is refused in a cluster that also runs other architectures unless the image is pinned by fingerprint, because Incus may place a multi-architecture alias on any of them.
