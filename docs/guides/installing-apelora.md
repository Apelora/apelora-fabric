# Install Apelora Fabric

Install the control plane, the worker, PostgreSQL and the two screens on top of an Incus cluster. Do [servers and network](servers-and-network.md) and [install Incus](install-incus.md) first: this page needs the cluster's address, its certificate, the two client certificates and the project name they produce.

**Evidence:** the services have run together against the four-server lab (REAL INCUS TESTED; see [current state](../architecture/current-state.md)). The example systemd units and NGINX configuration are parsed in CI on ubuntu-24.04 (`scripts/ci/verify-install-examples.sh`). They have also been **started** on one lab member from the draft release `v0.1.0-alpha.1` (`scripts/test-cluster/install-units.sh`, 2026-10-23: `User=apelora`, `ProtectHome=true`, both units active, `/api/v1/health` connected to the cluster, Studio and Console 200 through the example proxy). That is not a published release and not a production host (issue #17). Treat a first installation as an evaluation, and say what you find.

Values you choose are written as variables, for example `$DB_HOST`. Nothing here needs a fixed address, name or path.

## What runs where

| Part | What it is | Where it runs |
|---|---|---|
| **Control plane** (`apelora-control-plane`) | The API: plans, roles, audit | One process, near the database |
| **Worker** (`apelora-deployment-worker`) | Carries out jobs, enforces the network, replaces lost replicas | One or more processes |
| **PostgreSQL** | Apelora's own records, not your applications' data | Anywhere both can reach |
| **Studio and Console** | Two sets of static files | Any web server, over HTTPS |

Apelora is not in the data path: running instances keep running if the control plane, the worker or the database is down ([the control plane or the worker is down](../runbooks/api-outage.md)).

## 1. PostgreSQL

Any PostgreSQL the control plane and the worker can reach (development uses the version in `infra/dev/compose.yaml`). Create a role and a database for Apelora; the schema is created and upgraded automatically whenever the control plane or the worker connects, so the role needs the right to create tables in its own database.

```sh
DB_USER=...  DB_NAME=...  DB_HOST=...
sudo -u postgres createuser --pwprompt "$DB_USER"
sudo -u postgres createdb --owner "$DB_USER" "$DB_NAME"
# DATABASE_URL=postgres://$DB_USER:<password>@$DB_HOST:5432/$DB_NAME   (use TLS between hosts)
```

Back the database up: [back up and restore Apelora's own database](../runbooks/apelora-database-backup.md).

## 2. Get the programs

### From a release

Every version tag (`v*`) triggers the release workflow, which builds each service for **x86-64 and ARM64** and each screen as its own archive, with SHA-256 checksums, and attaches them to a draft GitHub release that a maintainer reviews and publishes. When a release is published, it is at <https://github.com/Apelora/apelora-fabric/releases>:

```sh
TAG=...          # the release's tag
ARCH=...         # x86_64 or aarch64
BASE=https://github.com/Apelora/apelora-fabric/releases/download/$TAG

for part in apelora-control-plane apelora-deployment-worker; do
  curl -fLO "$BASE/$part-$TAG-linux-$ARCH.tar.gz"
done
curl -fLO "$BASE/SHA256SUMS-services-$ARCH"
curl -fLO "$BASE/apelora-studio-$TAG.tar.gz"
curl -fLO "$BASE/apelora-fabric-console-$TAG.tar.gz"
curl -fLO "$BASE/SHA256SUMS-apps"
sha256sum --check --ignore-missing SHA256SUMS-services-$ARCH SHA256SUMS-apps
```

The service archives hold the program, the `LICENSE` and the `README.md`; the screen archives hold the files to serve.

### From source

You need the Rust toolchain the repository pins (`rust-toolchain.toml`; `rustup` installs it) and Node.js as `engines` in `package.json` says:

```sh
git clone https://github.com/Apelora/apelora-fabric && cd apelora-fabric
git checkout "$TAG"                      # a release tag, or main for the newest
cargo build --release -p apelora-control-plane -p apelora-deployment-worker   # target/release/
npm ci && npm run build                  # apps/studio/dist and apps/fabric-console/dist
```

## 3. Configure

Both services read **environment variables**. Put them in a file only root can read and hand it to your service manager; secrets come from there or from your secret store, never from the repository. The complete list, with defaults, is the [settings reference](../architecture/SETTINGS.md), generated from the code; what is needed to start is:

```sh
# /etc/apelora/apelora.env   (root:root, mode 0600)   -- both services
DATABASE_URL=postgres://...                       # without it the control plane runs without sign-in, on loopback only
APELORA_INCUS_URL=https://$SERVER_IP:8443         # any cluster member
APELORA_INCUS_CLIENT_CERT=$DIR/deploy.crt         # the restricted deploy identity
APELORA_INCUS_CLIENT_KEY=$DIR/deploy.key
APELORA_INCUS_SERVER_CERT=$DIR/cluster.crt        # the cluster's certificate: Apelora pins it
APELORA_INCUS_TLS_SERVER_NAME=$CERT_NAME          # the name inside that certificate, if not the URL's host
APELORA_DEPLOY_PROJECTS=$PROJECT                  # the Incus projects Apelora may change

# the control plane only
APELORA_INCUS_ADMIN_CLIENT_CERT=$DIR/admin.crt    # the unrestricted identity: never give it to the worker
APELORA_INCUS_ADMIN_CLIENT_KEY=$DIR/admin.key
APELORA_API_ADDR=...                              # where the API listens; loopback by default
APELORA_TLS_CERT=... APELORA_TLS_KEY=...          # TLS of its own, or APELORA_BEHIND_PROXY=1 behind a proxy that terminates it

# with OVN (see install Incus, step 4)
APELORA_NETWORK_UPLINK=$UPLINK
APELORA_SERVICE_ADDRESSES=$SERVICE_RANGE
```

The control plane refuses to listen on an address other than loopback without TLS of its own or a declared proxy, and without a database. Without `APELORA_NETWORK_UPLINK` it deploys but does not enforce the connections you draw. Optional features have their own settings: volumes and backups (`APELORA_VOLUME_POOL`, `APELORA_BACKUP_POOL`), alerts, fencing, sign-in; each is described on its page.

## 4. Run the services

Run both under a service manager so they restart. Example systemd units live in [`infra/systemd/`](../../infra/systemd/): `apelora-control-plane.service` and `apelora-deployment-worker.service`, same environment file, a dedicated `apelora` account that can read `/etc/apelora/apelora.env` and the Incus certificates (put those under `/etc/apelora`, not a home directory — the units set `ProtectHome=true`). Copy them to `/etc/systemd/system/` and `systemctl enable --now` each. CI parses the units (`systemd-analyze verify`) on ubuntu-24.04. The units have been started on a lab member from the draft release (`install-units.sh`, 2026-10-23); they have **not** been run on a production host from a published release (issue #17).

Both write one line per event to standard output (`APELORA_LOG_FORMAT=json` for structured logs). Metrics, logs, traces and an example collector (Prometheus, Loki, Tempo, Pyroscope, Grafana): [monitoring](../architecture/MONITORING.md). Several workers can run at once; they share the job queue safely. Do not give the worker the administrator Incus identity.

## 5. Serve Studio and Console

Serve the two directories as static files over HTTPS, each on its own host name, and pass `/api/` to the control plane, so each browser talks to one origin only. The repository's example, [`infra/serve/apelora.conf`](../../infra/serve/apelora.conf) (NGINX syntax), also sets the security headers and the content security policy, which a browser test checks. CI runs `nginx -t` on that file with dummy certificates. The same example has been serving Studio and Console on a lab member (200 over TLS, `/api/v1/health` through the proxy, 2026-10-23); it has **not** been run as a production proxy (issue #17).

## 6. Create the first administrator

```sh
DATABASE_URL=... apelora-control-plane token create --name "$ADMIN_NAME" --role fabric-admin
```

The token is shown once. Sign in to the Console with it ([signing in](signing-in.md) covers your identity provider afterwards), then open **Servers → Preflight** and **Health**: they say what is not yet right (a project the deploy identity cannot see, a member that is unreachable).

## 7. Add servers later

In the Console, **Servers → Machines and join tokens** registers a machine, approves it and shows a single-use join token; `scripts/enrol/enrol-member.sh` on the new server joins it to the cluster, and it is held out of placement until you admit it after its checks: [add a server to the cluster](../runbooks/member-join.md).

## Keep it healthy

[Rotate Apelora's Incus certificates](../runbooks/certificates.md) before they expire (they last as long as you made them), upgrade Incus one server at a time ([rolling upgrade](../runbooks/incus-rolling-upgrade.md)), and keep the [runbooks](../runbooks/README.md) at hand. Without internet access: [running without internet access](../architecture/DISCONNECTED.md).
