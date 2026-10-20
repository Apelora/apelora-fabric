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
npm run -s runtime-requests -w @apelora/module-catalog > /tmp/requests.json
APELORA_TEST_CATALOG_REQUESTS=/tmp/requests.json cargo test -p apelora-deployment-worker --test real_incus -- --ignored catalog_runtimes
```

Add a member (another VM), enrolled through Apelora (runbook: `docs/runbooks/member-join.md`): register and approve the machine, pipe the join token straight into the new VM (`add-member.sh` runs `scripts/enrol/enrol-member.sh` there, which also configures OVN when the cluster uses it and holds the member), then admit it:

```sh
api() { curl -s -H "Authorization: Bearer $APELORA_TOKEN" -H 'Content-Type: application/json' "$@"; }
api -d '{"name":"apelora-test-4"}' http://127.0.0.1:8080/api/v1/machines >/dev/null
api -X POST http://127.0.0.1:8080/api/v1/machines/apelora-test-4/approve | jq -r .token |
  bash scripts/test-cluster/add-member.sh 4
api -X POST http://127.0.0.1:8080/api/v1/machines/apelora-test-4/admit | jq '.checks[] | [.id, .status]'
```

Add OVN (optional, after `add-member.sh`) so network intents can be enforced:

```sh
bash scripts/test-cluster/ovn.sh
```

It installs the OVN database on the first member and the OVN controller on every member (tunnels over the members' private network), points Incus at it, gives `incusbr0` uplink ranges, and recreates the empty `apelora` project with `features.networks=true` and `restricted.networks.uplinks=incusbr0`, so the restricted deploy identity owns its OVN networks and ACLs. A shared `apelora-default` OVN network backs the project's default profile. Test-only shortcuts: one OVN database (OVN itself not highly available) without TLS on the private network.

With OVN in place, start the control plane with `APELORA_NETWORK_UPLINK=incusbr0` to enforce application connections. Add `APELORA_VOLUME_POOL=local` to deploy modules with volumes, and `APELORA_BACKUP_POOL=backup` (control plane and worker) for backups after `bash scripts/test-cluster/backup-pool.sh` has created that pool.

Install replica anti-affinity (Incus's placement scriptlet, cluster-wide; `--remove` restores Incus's own placement) and check it:

```sh
bash scripts/test-cluster/placement.sh
cargo test -p apelora-deployment-worker --test real_incus -- --ignored replicas_spread
```

Remove everything (the VMs and the credentials):

```sh
bash scripts/test-cluster/down.sh
```

## Things the real cluster taught us

- **The server certificate names the member's host name** (and loopback), not the address clients dial. `up.sh` reads the name from the certificate and sets `APELORA_INCUS_TLS_SERVER_NAME`; the adapter still pins the exact certificate.
- **Some `openssl req -x509` versions issue X.509 v1 certificates** when no extensions are given, and TLS libraries reject them. `up.sh` issues a v3 client-auth certificate and replaces a v1 one (and its trust entry); the adapter reports a v1 client certificate by name.
- **`"profiles": []` means no profiles**, and so no root disk; only an absent list gets the `default` profile. The adapter omits an empty list.
- **OCI image fingerprints are Incus's own**, not the registry's manifest digest. Pin an OCI image with a digest in the alias (`repository@sha256:…`); the `fingerprint` field is for simplestreams images.
- **OVN load balancers (Incus 7.0.1):** the restricted deploy identity can create them in its project once the uplink routes a range (`ipv4.routes`) and the project allows it (`restricted.networks.subnets=incusbr0:<range>`); `ovn.sh` sets both (198.18.100.0/28). Traffic to the listen address is translated to a backend *before* ACLs are evaluated, so the existing consumer→provider address-set rules cover it and no rule for the listen address is needed. Health-check probes (source: the subnet's last host address) are not subject to the ACLs. With `healthcheck=true`, a stopped backend stops receiving traffic within a few seconds and rejoins when it is back — REAL INCUS TESTED across two members. Health-check mappings are bound to the instance's logical port, so a backend recreated *with the same address* is not monitored (and gets no traffic) until the load balancer changes; rewriting it unchanged is not enough. Apelora therefore writes each backend's description as `<instance> <volatile.uuid>`, so a recreated replica always changes it.
- **Internet access from an application network:** an egress rule with destination `@external` (Incus's subject for traffic leaving through the uplink) let an instance resolve names and fetch from a public mirror, kept other internet ports blocked, and did *not* open other instances of the application — unlike an any-destination egress allow. REAL INCUS TESTED, three members, 2026-10-20.
- **Reaching a service address from outside (publishing):** the OVN router does not answer ARP for listen addresses outside the uplink's subnet, so the network outside must *route* them to the application network's router (`volatile.network.ipv4.address` of the network, e.g. `ip route add <address>/32 via <router> dev incusbr0`). On this lab every member has its own `incusbr0`, so that works only from the member that hosts the network's active gateway; on a real uplink (one shared network, or routes/BGP from the upstream router) it works from anywhere. With the route in place, a published port answered and an unpublished one stayed blocked — REAL INCUS TESTED, three members, 2026-10-20.
- **A member failure (VM force-stopped; Incus 7.0.1, OVN 25.03):** Incus marks the member Offline after about 20 s. Its instances stay listed with status `Error` and `config: null` (labels unknown); Apelora treats them as unreachable instead of failing the whole inventory. OVN health checks run on the backend's own chassis, so a dead member's load balancer backends stay "online" and keep receiving their share of requests until Apelora replaces them (with a 30 s recovery delay the service was whole again about 80 s after the failure). Incus may refuse load balancer updates while a member is going down (`peer node … is down`, HTTP 500); the supervisor retries failed syncs. Updates can also leave a load balancer without its OVN health check while Incus still reports `healthcheck: true` — its state endpoint then reports backends as `unknown`; switching `healthcheck` off and on reinstalls it, which `network.sync` does after every change and whenever it sees `unknown`.
- **Three end-to-end runs with a member force-stopped (`demo.sh`, three members):** all passed; the service was whole again after 93, 84 and 206 s. In one run Incus refused every load balancer update until the member returned, and the service still recovered after about 3.5 minutes (consistent with OVN eventually marking the dead backend offline — not confirmed). In another, Incus also reported a member that was *not* stopped as Offline for at least 30 s, so its replica got a needless substitute (retired later): keep `APELORA_RECOVER_AFTER_SECS` well above 30 s outside drills. The runs also exposed a supervisor bug — a slot could be retired twice (both instances deleted) when the returning member's instance was still starting — fixed: a slot is decided once, only when all its instances run.
- **Deleting a project removes it from restricted certificates**, but a running server may keep authorising them by project name until its identity cache is rebuilt — seen when a member went offline: the deploy identity then saw empty lists (HTTP 200) and got 403 on single objects. `ovn.sh` (which recreates the project) now re-grants it and warns if the deploy identity lacks it.
- **`incus launch` reads YAML from standard input**: in a script fed to `bash -s`, give it `</dev/null`, or it consumes the rest of the script.
- **Hot-plugging a NIC into a running OCI container leaves it without an address** (no DHCP client runs inside); set a NIC's ACLs when the instance is created.
- **Placement scriptlet:** `get_instances(location, project)` called with positional arguments returned no instances on Incus 7.0.1; keyword arguments (`location = …, project = …`) work. The scriptlet sees the new instance's `config`, so Apelora's labels drive the choice. `incus config set KEY VALUE` reads KEY as an instance name when VALUE contains `=`; use `KEY=VALUE`.
- **Custom volumes on the local pool** (details in `docs/architecture/STORAGE.md` §10): Incus does not place an instance next to the member-local volume it references (create fails with `Storage volume not found`); create the instance stopped, then the volume on its member, then attach. Quotas bind; shrinking is accepted by Incus; snapshots need `restricted.snapshots=allow` on the project (`up.sh` and `ovn.sh` set it); restore needs the instance stopped; evacuating a member stops such instances in place.
- **Copying volumes** (backups, moves; Incus 7.0.1, restricted identity): a copy into another pool or onto another member works with `source.type=copy` and `source.location`, but Incus carries neither the size nor the `user.*` labels over: Apelora sets both. Waiting on the copy's operation is refused (`User does not have permission for project "default"`, also with `?project=`), because that operation lives on the receiving member; the copy's other operation is listed in the project's operations and turns `Success` when the copy is done, which is what the adapter watches instead. A `refresh` copy brings a backup's data back into an existing volume but deletes the volume's own snapshots, and Incus accepts it while the instance runs: Apelora refuses that and takes a safety backup first. Moving an instance to another member (`?target=`) is refused for the restricted identity (`This project doesn't allow cluster member targeting`): moves use the admin identity.
- **OVN central database stopped** (member 1, application with a service address running): the service and names kept working from cached flows; a new replica's creation stalled until the database returned, then every queued job finished within 10 s.
- **Quorum loss** (two of three voters force-stopped): the surviving member's `incus cluster list` hung; the API answered 500 "context deadline exceeded" after 5 s (Apelora: 502 `incus_api_error`) or timed out (502 `incus_unreachable`); a running instance kept answering; with both members started again the database had quorum and all four members were Online within 10 s, roles unchanged.
- **Worker killed mid-create** (`drill-worker-restart.sh`): the job stayed `running` with its Incus operation recorded; the next worker claimed it 30 s later (lease expiry), waited on that operation and succeeded on its second attempt; no second instance was created.
- **A replica failing its port check (NGINX moved off port 80 while running; four members, `demo.sh`):** the load balancer's state reported the backend `offline` within seconds and stopped sending it requests; with a 10 s supervision interval and 20 s hysteresis the worker reported it unhealthy after 36 s and restarted it after 66 s. The change survived the restart (container root file system), so it kept failing, and the cooldown prevented a second restart; after the port reopened it was healthy again in 40 s. Right after a deployment a service address exists before its load balancer (Incus answers 404): no check yet, not an error.
- **Holding a member out of placement:** `scheduler.instance=manual` on a member kept six new instances off it while an instance targeted at it still landed there (REAL INCUS TESTED, 2026-10-20). Incus cannot preset it before the member joins, so `scripts/enrol/enrol-member.sh` sets it right after the join. Updating a member (`PATCH /1.0/cluster/members/<name>`) without `roles` is read as dropping them (`The "database" role cannot be dropped`): send the member's roles, groups, failure domain and description back with the config; an empty value removes a config key.
- **Locations:** unmanaged host interfaces are listed with `"locations": null`; OVN networks list only the members that existed when they were created (the fourth member was missing from `apelora-default` and its instances still worked on it), so per-member checks look at bridges and pools only. `GET /1.0?target=<member>` returns that member's own Incus and kernel version.
- A restricted client certificate can still read cluster members, managed networks and storage pools, so inventory and preflight work with least privilege; it sees only its own projects.

## Limits

- Four members (three database voters and a standby since the fourth was enrolled through Apelora, 2026-10-20), all aarch64: no mixed-architecture evidence yet. With two members the second was only a database standby; with three, the cluster keeps quorum when one member is lost.
- The packaging repository may ship a different 7.0.x than `pinned_tag`; `up.sh` prints the running version, and evidence names the version it was produced with.
- Clock drift and member latency are not measurable through the Incus API; preflight reports them as not checked.
- Public image servers keep only recent builds: a fingerprint pin of an image they have dropped fails to download, and the job retries until it gives up. Pin images from a server you control (or mirror them) for anything that must keep working.
- **OVN findings (Incus 7.0.1, OVN 25.03, two members):** instances on different members reach each other by name (`<instance>.incus`) on an OVN network. ACL rules whose source or destination is *another ACL* (subject selectors) work only between instances on the same member: across members the destination member rejects the connection although `ovn-trace` allows it (`ovn-monitor-all` did not change that). Rules on IP addresses (and so address sets) work across members. Once an ACL is on a NIC, unmatched traffic is rejected in both directions, so consumers need egress rules; a consumer's egress allow and a provider's ingress allow combine with OR, so Apelora writes both sides; setting a NIC's default egress to allow overrides the provider's ingress rejection and must not be used.
- A spec that lists `architectures` is refused in a cluster that also runs other architectures unless the image is pinned by fingerprint, because Incus may place a multi-architecture alias on any of them.
