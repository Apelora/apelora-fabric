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
| Client identity | ECDSA P-384, X.509 v3, client-auth only, 90 days, **restricted to the `apelora` project** | Least privilege: Apelora cannot see or touch anything else |
| Nested virtualisation | On (Apple M3 or later, macOS 15 or later) | Lets the test cluster run QEMU virtual machines |

No host folders are shared with the VM and no guest ports are forwarded; the Incus API is reached on the VM's own address on the host-only NAT network.

## Use

```sh
brew install lima                       # once
bash scripts/test-cluster/up.sh         # create or restart; safe to repeat
source ~/.config/apelora/test-cluster/env
```

The `env` file sets `APELORA_INCUS_URL`, the three certificate paths, `APELORA_INCUS_TLS_SERVER_NAME` and `APELORA_DEPLOY_PROJECTS`. The certificates and the key live in `~/.config/apelora/test-cluster` (mode 0700, files 0600), never in the repository.

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

Remove everything (the VM and the credentials):

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

- One member: no placement, failover or cross-member networking evidence. Those claims wait for a multi-member test cluster.
- The packaging repository may ship a different 7.0.x than `pinned_tag`; `up.sh` prints the running version, and evidence names the version it was produced with.
- Clock drift and member latency are not measurable through the Incus API; preflight reports them as not checked.
- Public image servers keep only recent builds: a fingerprint pin of an image they have dropped fails to download, and the job retries until it gives up. Pin images from a server you control (or mirror them) for anything that must keep working.
- A spec that lists `architectures` is refused in a cluster that also runs other architectures unless the image is pinned by fingerprint, because Incus may place a multi-architecture alias on any of them.
