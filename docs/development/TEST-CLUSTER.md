# Disposable Incus test cluster

The **only** Incus that Apelora's tests may change. It runs in local VMs on a developer workstation, holds nothing of value, and is deleted and recreated at will. Results produced against it are labelled **REAL INCUS TESTED**.

New here, or wondering what a Mac can and cannot test (BMC, PXE, the physical network)? Start with [testing without hardware](../guides/testing-without-hardware.md).

Four Lima VMs (`apelora-incus`, `apelora-incus-2`, `-3`, `-4`; Incus members `apelora-test-1` … `-4`) stand in for the physical servers a production cluster has, and a fifth Lima VM (`apelora-san`) stands in for a SAN. Where a result depends on real hardware (disk speed, network latency, fencing through a BMC), the lab says nothing about it.

## What it is

| Part | Choice | Why |
|---|---|---|
| Member VMs | [Lima](https://lima-vm.io) (`brew install lima`), Apple Virtualization (`vz`), 4 CPU, 8 GiB, 40 GiB each (`incus-vm.yaml`) | Local, scriptable, deleted with one command; each stands in for a physical server |
| SAN VM | `apelora-san` (`san-vm.yaml`): 2 CPU, 2 GiB, 40 GiB, on the members' `user-v2` network, exporting one iSCSI LUN without authentication | Stands in for a SAN, so the clustered LVM pool can be tried; throwaway only |
| Guest | Debian 13 cloud image, digest-pinned by Lima's template | Kernel 6.12, the Incus 7.0 minimum |
| Incus | 7.0 LTS from the packaging repository named in Incus's installation guide; the signing key's fingerprint is checked before use | Our pinned series (`upstream.lock.yaml`) |
| Topology | Four members: three database voters and a standby (the fourth enrolled through Apelora) | Quorum survives the loss of one member; enough members to keep replicas apart |
| Storage pools | `local` (btrfs, 20 GiB loop file per member: root disks and `data-local` volumes), `backup` (dir per member, `backup-pool.sh`: backup copies), `shared` (`lvmcluster` on the SAN VM's LUN, `shared-storage.sh`, created 2026-10-21) | Local pools need no host setup; `shared` holds `data-replicated` volumes and their modules' root disks, which restart on another member after fencing (ADR 0013) |
| Network | `incusbr0` (NAT bridge per member) as the uplink only; module traffic runs on one OVN network per application (`ovn.sh`) | A bridge does not span members; OVN does |
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

A real MAAS (optional, 6 GiB, for the provisioner adapter; it manages no hardware, its machines are records with manual power; credentials in `~/.config/apelora/test-cluster/maas.env`, TLS with the certificate in `maas-ca.pem`):

```sh
bash scripts/test-cluster/maas.sh up        # VM, MAAS 3.7 (snap), TLS, admin and API key
bash scripts/test-cluster/maas.sh machines  # waits for the boot images, then creates two test machines
bash scripts/test-cluster/maas.sh test      # the adapter's ignored real-API test
source ~/.config/apelora/test-cluster/maas.env   # APELORA_MAAS_URL, _API_KEY, _CA for the control plane
bash scripts/test-cluster/maas.sh down      # delete it
```

Add OVN (optional, after `add-member.sh`) so network intents can be enforced:

```sh
bash scripts/test-cluster/ovn.sh
```

Measured sizing (issue #16; lab VMs, not hardware). Needs OVN. Removes what it deployed:

```sh
APPS=4 MODULES=10 WORKERS=1 bash scripts/test-cluster/measure.sh
```

Figures: [lab sizing](../design/capacity-lab-sizing.md).

It installs the OVN database on the first member and the OVN controller on every member (tunnels over the members' private network), points Incus at it, gives `incusbr0` uplink ranges, and recreates the empty `apelora` project with `features.networks=true` and `restricted.networks.uplinks=incusbr0`, so the restricted deploy identity owns its OVN networks and ACLs. A shared `apelora-default` OVN network backs the project's default profile. Test-only shortcuts: one OVN database (OVN itself not highly available) without TLS on the private network. `bash scripts/test-cluster/ovn-ha.sh` (dry-run; `APPLY=1` to change the lab) converts that to three clustered databases with TLS on the first three members (issue #13). Certificates stay in `~/.config/apelora/test-cluster/ovn-pki`. REAL INCUS TESTED 2026-10-22: after `APPLY=1`, stopping the leader's `ovn-ovsdb-server-nb/sb` elected another member; existing OVN ACLs were unchanged; a container on `apelora-default` kept pinging the OVN router. Debian's `ovn-central.service` is a dummy oneshot — stop the ovsdb units, not that name alone.

With OVN in place, start the control plane with `APELORA_NETWORK_UPLINK=incusbr0` to enforce application connections. `bash scripts/test-cluster/ipv6.sh` (about a minute after a cold compile, needs port 8080 free) checks enforcement over IPv4 and over the IPv6 subnet Incus gives each application network, and an IPv6 service address from `fd42:198:100::/112` (`ovn.sh` sets `ipv6.routes`; the uplink bridge stays `ipv6.address=none`), then removes what it deployed. REAL INCUS TESTED 2026-10-22. `bash scripts/test-cluster/dns.sh` (same port, IPv4 service range) checks that consumers receive `APELORA_SERVICE_NAME_*`, that `network.sync` wrote `dns.domain` and `user.apelora.dns`, and that a client restarted after the optional resolver is up resolves the service name and still resolves the instance name. REAL INCUS TESTED 2026-10-23, including the in-application answer: the restarted client resolved the service name to the service address and the instance name still resolved. The script needs `APELORA_API_ADDR` when 8080 is already in use. `bash scripts/test-cluster/metrics.sh` (dry-run; `APPLY=1` to change the lab) trusts a metrics-only certificate and scrapes `/1.0/metrics` on every member. REAL INCUS TESTED 2026-10-23. Add `APELORA_VOLUME_POOL=local` to deploy modules with volumes, and `APELORA_BACKUP_POOL=backup` (control plane and worker) for backups after `bash scripts/test-cluster/backup-pool.sh` has created that pool.

`bash scripts/test-cluster/install-units.sh` (dry-run; `APPLY=1` to change one member) installs `v0.1.0-alpha.2` (control plane, worker, Studio and Console from that tag), the example systemd units and the NGINX example on `apelora-incus` under the `apelora` account. The units were first started from draft `v0.1.0-alpha.1` on 2026-10-23; do not install that tag. Issue #17 stays open until a published release on a production host. `APPLY=1 STOP=1` disables the units. While they run, the member's control plane listens on its own `127.0.0.1:8080` and PostgreSQL on `127.0.0.1:5432`: on VMs created before 2026-10-25 Lima forwards both to the same ports on your workstation, so a control plane you start there fails with "address in use" and `npm run dev:console` talks to the member's. Stop the units first, or apply the fix below.

`bash scripts/test-cluster/observe.sh` (dry-run; `APPLY=1` to start on this workstation) runs the example collector on loopback: Prometheus, Loki, Tempo, Pyroscope, Grafana and Alloy (`infra/observe`). Point the control plane at `APELORA_METRICS_ADDR=127.0.0.1:9470`, `APELORA_LOG_FORMAT=json` and `APELORA_OTLP_ENDPOINT=http://127.0.0.1:4318`. Grafana is `http://127.0.0.1:3000`. Incus `/1.0/metrics` stays on the members (`metrics.sh`). REAL INCUS TESTED 2026-10-23: Prometheus scraped both processes (`apelora_incus_up=1`, four members Online), Loki ingested JSON logs, Tempo held request traces.

Install replica anti-affinity (Incus's placement scriptlet, cluster-wide; `--remove` restores Incus's own placement) and check it:

```sh
bash scripts/test-cluster/placement.sh
cargo test -p apelora-deployment-worker --test real_incus -- --ignored replicas_spread
```

Shared storage (clustered LVM on the lab SAN; HA slice B in `docs/design/mobility-and-ha.md`). Start the SAN VM (first command below) and give it an iSCSI target that exports one LUN as `iqn.2026-10.org.apelora:lab-shared` with no authentication (done by hand on the VM so far, not scripted); then:

```sh
limactl start --name apelora-san scripts/test-cluster/san-vm.yaml
bash scripts/test-cluster/shared-storage.sh plan     # default: print every step, change nothing
bash scripts/test-cluster/shared-storage.sh apply    # safe to repeat; finished steps are skipped
bash scripts/test-cluster/shared-storage.sh status   # iSCSI sessions, locks, volume group, pools
```

On every member it installs the iSCSI initiator, the LVM lock daemon and sanlock, loads a software watchdog, logs in to the target (also at boot) and sets a unique `host_id`; then it creates the shared volume group `apelora-shared` on the LUN and the Incus pool `shared` (`lvmcluster`) on all members. `APELORA_SAN_IP` (default `192.168.104.6`, the SAN VM's `user-v2` address), `APELORA_SAN_IQN`, `APELORA_SHARED_VG` and `APELORA_SHARED_POOL` override the defaults. REAL INCUS TESTED (2026-10-21): the pool exists on all four members, and `shared_pool_instances_keep_their_disks_on_it_and_move_without_copying` (`real_incus`) puts a module's root disk and volume on it and moves it cold to another member in 2.5 s without copying. Start the control plane with `APELORA_SHARED_POOL=shared` for `data-replicated` volumes. `down.sh` leaves the SAN VM alone: remove it with `limactl delete --force apelora-san`.

Fence probe (ADR 0013): an account `apelora-fence` on every member whose only authorised key runs `sanlock client gets -h 1`; the key and the pinned host keys stay in `~/.config/apelora/test-cluster/fence-probe` (0700):

```sh
bash scripts/test-cluster/fence-probe.sh plan     # default: print every step, change nothing
bash scripts/test-cluster/fence-probe.sh apply    # safe to repeat; writes ~/.config/apelora/test-cluster/fence-probe.env (worker only)
bash scripts/test-cluster/fence-probe.sh status   # each member's lockspace as the worker reads it; checks the account runs nothing else
bash scripts/test-cluster/fence-probe.sh remove
```

Lima gives a VM new SSH host keys at every boot: after restarting a member VM, run `apply` again (the probe refuses changed host keys, as it should).

HA drill (slice B): a stateful module on `shared`, its member force-stopped, restarted elsewhere after fencing, the member started again:

```sh
bash scripts/test-cluster/ha-drill.sh plan              # read-only: checks the lab, prints every step, writes a plan file
bash scripts/test-cluster/ha-drill.sh run <planfile>    # refuses without a fresh plan for the same member list
```

It never stops `apelora-san`, `apelora-test-1` (the API address and the OVN database) or the database leader, and only a member whose loss keeps the quorum; it runs its own database, control plane (:8095) and worker (:9495) and removes everything on exit. `HA_DRILL_VM=1` (set for both `plan` and `run`) deploys a virtual machine with `migration.stateful=true` instead of a container, and waits for the guest agent before it reads the marker back. Timings: [member failure](../runbooks/member-failure.md#modules-on-shared-storage).

Blocked egress (ADR 0010: an installation without internet access). Outbound traffic from the members, and from the instances they run, is dropped except to the lab's own networks (the members' directly connected subnets, which include the host and `incusbr0`), the members' DNS resolvers, and the mirror you declare:

```sh
APELORA_LAB_MIRROR=192.168.104.20:443 scripts/test-cluster/egress.sh plan     # default: print and nft --check the rules, change nothing
APELORA_LAB_MIRROR=192.168.104.20:443 scripts/test-cluster/egress.sh block    # load them on every member
scripts/test-cluster/egress.sh status    # table loaded or not, drop counters, whether an upstream image server answers
scripts/test-cluster/egress.sh unblock   # remove them
```

- The rules are one nftables table, `inet apelora_egress`, per member: an output chain (the member itself) with policy drop, and a forward chain that drops what instances send out through the member's default-route interfaces (`eth0`, `lima0`). The lab subnets, the resolvers and the default-route interfaces are read from each member at run time; `plan` prints the result and has nft validate it without loading it.
- `APELORA_LAB_MIRROR` is `IPv4[/prefix][:port]`, comma separated, or `none`; `block` refuses to run without it. `APELORA_LAB_ALLOW` adds destinations (for example an NTP server: outside NTP is blocked too). `APELORA_LAB_PROBE` changes the URL `status` tries.
- Names still resolve through the host's resolver, so a lookup can leave the lab; only connections cannot. A VM restart clears the rules (they are not saved). The SAN VM is left alone.
- Then point both services at the mirror (`APELORA_IMAGE_MIRRORS`, `APELORA_IMAGE_SERVERS=mirrors-only`) and run the real-Incus tests; ADR 0010 asks for them to pass like this before a release is called ready for installations without internet access.
- Evidence so far (2026-10-21): `plan` produced rules that `nft --check` accepted on all four members, and `status` showed egress open (the upstream simplestreams index answered HTTP 200). `block` and `unblock` have **not** been run, no mirror runs in the lab yet, and the real-Incus tests have not been run with egress blocked.

Remove everything (the VMs and the credentials):

```sh
bash scripts/test-cluster/down.sh
```

## Things the real cluster taught us

- **The server certificate names the member's host name** (and loopback), not the address clients dial. `up.sh` reads the name from the certificate and sets `APELORA_INCUS_TLS_SERVER_NAME`; the adapter still pins the exact certificate.
- **Some `openssl req -x509` versions issue X.509 v1 certificates** when no extensions are given, and TLS libraries reject them. `up.sh` issues a v3 client-auth certificate and replaces a v1 one (and its trust entry); the adapter reports a v1 client certificate by name.
- **`"profiles": []` means no profiles**, and so no root disk; only an absent list gets the `default` profile. The adapter omits an empty list.
- **OCI image fingerprints are Incus's own**, not the registry's manifest digest. Pin an OCI image with a digest in the alias (`repository@sha256:…`); the `fingerprint` field is for simplestreams images.
- **OVN load balancers (Incus 7.0.1):** the restricted deploy identity can create them in its project once the uplink routes a range (`ipv4.routes`, and `ipv6.routes` for IPv6 listen addresses) and the project allows it (`restricted.networks.subnets=incusbr0:<range>`); `ovn.sh` sets the IPv4 pair (198.18.100.0/28) and `fd42:198:100::/112` (`APELORA_TEST_SERVICES_V6=` skips IPv6). IPv6 listen addresses REAL INCUS TESTED 2026-10-22 (`ipv6.sh`). Traffic to the listen address is translated to a backend *before* ACLs are evaluated, so the existing consumer→provider address-set rules cover it and no rule for the listen address is needed. Health-check probes (source: the subnet's last host address) are not subject to the ACLs. With `healthcheck=true`, a stopped backend stops receiving traffic within a few seconds and rejoins when it is back — REAL INCUS TESTED across two members. Health-check mappings are bound to the instance's logical port, so a backend recreated *with the same address* is not monitored (and gets no traffic) until the load balancer changes; rewriting it unchanged is not enough. Apelora therefore writes each backend's description as `<instance> <volatile.uuid>`, so a recreated replica always changes it.
- **Internet access from an application network:** an egress rule with destination `@external` (Incus's subject for traffic leaving through the uplink) let an instance resolve names and fetch from a public mirror, kept other internet ports blocked, and did *not* open other instances of the application — unlike an any-destination egress allow. REAL INCUS TESTED, three members, 2026-10-20.
- **Reaching a service address from outside (publishing):** the OVN router does not answer ARP for listen addresses outside the uplink's subnet, so the network outside must *route* them to the application network's router (`volatile.network.ipv4.address` of the network, e.g. `ip route add <address>/32 via <router> dev incusbr0`). On this lab every member has its own `incusbr0`, so that works only from the member that hosts the network's active gateway; on a real uplink (one shared network, or routes/BGP from the upstream router) it works from anywhere. With the route in place, a published port answered and an unpublished one stayed blocked — REAL INCUS TESTED, three members, 2026-10-20.
- **A member failure with shared storage (VM force-stopped; Incus 7.0.1, lvmcluster, sanlock 3.9.5, 2026-10-21):** the other members' sanlock reported the dead member's lease FAIL after 74–92 s and DEAD after 135–148 s; each member judges this from its own readings (up to about 10 s apart), so the member that restarted the instance sometimes still refused the LV lock ("LV locked by other host") for 5–7 s after another member reported DEAD. Relinking the instance to a healthy member took under a second, the start a few seconds. In one run Incus reported every member, not only the stopped one, Offline for about 10 s. The returning member rejoined the lockspace at boot with the next lease generation.
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
- **Lima forwards ports bound to the guest's loopback even when told to ignore `0.0.0.0`** (Lima 2.2): a rule for `0.0.0.0` matches only sockets bound to all interfaces, and Lima's built-in last rule forwards the rest to the workstation. Found when the install test's control plane on a member took the workstation's `127.0.0.1:8080` (and its PostgreSQL `5432`; the MAAS VM's port 954 even on all of the workstation's interfaces). The VM files now ignore both `0.0.0.0` and `127.0.0.1`. For VMs created earlier, copy the `portForwards` block from `incus-vm.yaml` into `~/.lima/<vm>/lima.yaml` and `limactl restart <vm>`; `lsof -nP -iTCP -sTCP:LISTEN | grep limactl` should then show only each VM's SSH port.
- A restricted client certificate can still read cluster members, managed networks and storage pools, so inventory and preflight work with least privilege; it sees only its own projects.

## Limits

- Four members (three database voters and a standby since the fourth was enrolled through Apelora, 2026-10-20), all aarch64: no mixed-architecture evidence yet. With two members the second was only a database standby; with three, the cluster keeps quorum when one member is lost.
- The packaging repository may ship a different 7.0.x than `pinned_tag`; `up.sh` prints the running version, and evidence names the version it was produced with.
- Clock drift and member latency are not measurable through the Incus API; preflight reports them as not checked.
- Public image servers keep only recent builds: a fingerprint pin of an image they have dropped fails to download, and the job retries until it gives up. Pin images from a server you control (or mirror them) for anything that must keep working.
- **OVN findings (Incus 7.0.1, OVN 25.03, two members):** instances on different members reach each other by name (`<instance>.incus`) on an OVN network. ACL rules whose source or destination is *another ACL* (subject selectors) work only between instances on the same member: across members the destination member rejects the connection although `ovn-trace` allows it (`ovn-monitor-all` did not change that). Rules on IP addresses (and so address sets) work across members. Once an ACL is on a NIC, unmatched traffic is rejected in both directions, so consumers need egress rules; a consumer's egress allow and a provider's ingress allow combine with OR, so Apelora writes both sides; setting a NIC's default egress to allow overrides the provider's ingress rejection and must not be used.
- A spec that lists `architectures` is refused in a cluster that also runs other architectures unless the image is pinned by fingerprint, because Incus may place a multi-architecture alias on any of them.
