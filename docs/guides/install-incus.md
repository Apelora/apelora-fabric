# Install Incus and form the cluster

Apelora Fabric runs your modules on [Incus](https://linuxcontainers.org/incus/), the open-source system that runs containers and virtual machines on a cluster of Linux servers. It does not install or upgrade Incus for you: you install it on each server, form the cluster, and give Apelora a project to deploy into and a certificate to reach it. This page shows how.

**Evidence:** these are the steps the four-server lab was built with (`scripts/test-cluster/`), written with variables where your values differ. REAL INCUS TESTED on that lab, on virtual machines with one network card each. Physical servers and a physical uplink have not been tried; those parts follow Incus's own documentation and say so. A highly available OVN database with TLS is scripted for the lab (`ovn-ha.sh`) and has not been applied. Read [servers and network](servers-and-network.md) first.

Commands run as root (or with `sudo`) on the server named in each step. Values you choose are written as variables, for example `$SERVER_IP`; set them in your shell first.

## Where to get Incus

| What | Where |
|---|---|
| **Incus installation guide** (every supported distribution) | <https://linuxcontainers.org/incus/docs/main/installing/> |
| **Release downloads and notes** | <https://github.com/lxc/incus/releases> |
| **Packages for Debian and Ubuntu** (the project's own repository, which carries the LTS series) | <https://github.com/zabbly/incus>, repository at <https://pkgs.zabbly.com/incus/> |
| **Forming a cluster** | <https://linuxcontainers.org/incus/docs/main/howto/cluster_form/> |
| **Setting up OVN** | <https://linuxcontainers.org/incus/docs/main/howto/network_ovn_setup/> |
| **The series Apelora is tested with** | `series` and `pinned_tag` in [`upstream.lock.yaml`](../../upstream.lock.yaml) |

Use the **LTS series that `upstream.lock.yaml` pins**, the same release on every server. Newer feature releases are watched but not tested; moving to another LTS series is a deliberate decision with its own record ([upstream Incus](../development/UPSTREAM-INCUS.md)).

## 1. Install Incus on every server

Two ways. Both end with `incus --version` printing the pinned series.

### From the project's package repository (what the lab does)

This is the route that gives the pinned LTS series on Debian and Ubuntu. Set the series from `upstream.lock.yaml`, fetch the repository's signing key, **compare its fingerprint with the one the publisher shows** (the lab script refuses a key that differs), then add the repository and install:

```sh
SERIES=7.0        # `series` in upstream.lock.yaml

apt-get update && apt-get install -y curl gpg
install -d -m 0755 /etc/apt/keyrings
curl -fsSL https://pkgs.zabbly.com/key.asc -o /etc/apt/keyrings/zabbly.asc
gpg --show-keys --with-colons /etc/apt/keyrings/zabbly.asc | awk -F: '/^fpr/ {print $10; exit}'   # compare with the publisher's page

cat >/etc/apt/sources.list.d/zabbly-incus-lts-$SERIES.sources <<EOF
Enabled: yes
Types: deb
URIs: https://pkgs.zabbly.com/incus/lts-$SERIES
Suites: $(. /etc/os-release && echo "$VERSION_CODENAME")
Components: main
Architectures: $(dpkg --print-architecture)
Signed-By: /etc/apt/keyrings/zabbly.asc
EOF

apt-get update && apt-get install -y incus
incus --version
```

The `incus` package includes virtual machine support. Check `ls /dev/kvm` if you will run virtual machines. The lab's version of these steps, with the key check in code, is in `scripts/test-cluster/incus-vm.yaml`.

### From your distribution

Some distributions package Incus themselves. Use that only if `apt policy incus` (or your package manager's equivalent) offers the pinned series; distributions often carry an older LTS. The [installation guide](https://linuxcontainers.org/incus/docs/main/installing/) lists the commands for each distribution. Installing only the client on a workstation is fine for looking around; the daemon runs only on Linux.

## 2. Prepare each server

- A **static IP address** and a host name; clocks synchronised (`timedatectl` should say the clock is synchronised).
- **Firewall:** between the servers, allow TCP 8443 (Incus), UDP 6081 (OVN tunnels) and, if OVN's databases are on them, TCP 6641 and 6642. From the machine that will run Apelora, allow TCP 8443 to the servers. See [servers and network](servers-and-network.md#network-layouts) for the full list.
- A **disk or partition** for the storage pool, if you do not use a file-backed pool (the lab does, for convenience only).

## 3. Form the cluster

Do the first server, then add the others. Use a **specific** address for the cluster: a cluster does not accept the wildcard address as its own.

### The first server

Write a preseed file with your values (it is the lab's, with variables) and apply it:

```sh
SERVER_IP=...           # this server's address, reachable by the other servers
MEMBER=...              # the name this server has in the cluster
POOL_SOURCE=...         # a disk such as /dev/sdb; for a trial, delete the `source:` line below to get a file-backed pool

cat >preseed.yaml <<EOF
config:
  core.https_address: "$SERVER_IP:8443"
  cluster.https_address: "$SERVER_IP:8443"
cluster:
  server_name: $MEMBER
  enabled: true
storage_pools:
  - name: local
    driver: btrfs
    config:
      source: $POOL_SOURCE
EOF
incus admin init --preseed <preseed.yaml
incus cluster list
```

`local` is the pool name the examples and Apelora's settings use. Choose the storage driver your servers should have: Incus supports several, and Apelora asks Incus to create and grow volumes in the pools it is told to use without managing the driver ([storage](../architecture/STORAGE.md)). The interactive form of the same step is `incus admin init`: answer yes to clustering, give the address, and say you are not joining an existing cluster.

Also shorten how long join tokens live (the default is three hours):

```sh
incus config set cluster.join_token_expiry=30M
```

### Every other server

On a **member already in the cluster**, create a single-use token for each new server:

```sh
NEW_MEMBER=...          # the name the new server will have
incus cluster add "$NEW_MEMBER"      # prints the token
```

On the **new server** (install Incus first; it must be a fresh install, because joining deletes what is on it), join with the token:

```sh
SERVER_IP=...           # the new server's own address
TOKEN=...               # from the step above; paste it, never keep it in a file or script

cat >join.yaml <<EOF
cluster:
  enabled: true
  server_address: $SERVER_IP:8443
  cluster_token: $TOKEN
EOF
incus admin init --preseed <join.yaml
```

`incus cluster list` on any member should now show every server online, and three of them with the database role. Servers added **later** are better enrolled through Apelora, which registers, approves and holds a new server until it passes its checks: [add a server to the cluster](../runbooks/member-join.md).

## 4. Networking for applications (OVN)

Apelora gives every application its own OVN network and turns the cables you draw into access rules on it. This needs OVN on the servers and an **uplink**. Without them Apelora still deploys, but it cannot enforce the connections you drew (the deploy dialog says so). Skip this step to try deployment first, and add it before you rely on the connections.

Follow Incus's own guide for OVN ([setting up OVN](https://linuxcontainers.org/incus/docs/main/howto/network_ovn_setup/)): it covers a highly available set of OVN databases on three servers, which is what a real installation should use. Put **TLS on the database remotes** as well: Incus's outline uses plaintext `tcp:` listeners (`--db-nb-create-insecure-remote=yes`); a real installation should not. In outline:

```sh
# every server
apt-get install -y ovn-host
# the servers that hold the OVN databases (three, an odd number)
apt-get install -y ovn-central

# /etc/default/ovn-central on each database server: cluster local address, join the first
# member on the other two, ssl: remotes, create-insecure-remote=no, and the PEM files for
# the northbound and southbound databases and for northd. ovn-host gets the controller's
# matching files. Ports 6641/6642 are the client remotes; 6643/6644 are the raft ports.

# every server: point the local OVN controller at every database, and set the tunnel address
OVN_DB=ssl:$DB1:6642,ssl:$DB2:6642,ssl:$DB3:6642
ovs-vsctl set open_vswitch . \
  external_ids:ovn-remote=$OVN_DB \
  external_ids:ovn-encap-type=geneve \
  external_ids:ovn-encap-ip=$SERVER_IP

# once, on any member: Incus talks to the northbound cluster with its own client certificate
incus config set network.ovn.northbound_connection=ssl:$DB1:6641,ssl:$DB2:6641,ssl:$DB3:6641 \
  network.ovn.ca_cert="$(cat ca.crt)" \
  network.ovn.client_cert="$(cat incus.crt)" \
  network.ovn.client_key="$(cat incus.key)"
```

The lab's `scripts/test-cluster/ovn.sh` still starts one database without TLS (a shortcut so application-network tests can run). `scripts/test-cluster/ovn-ha.sh` converts that layout to three clustered databases with TLS; it is a dry-run until `APPLY=1`. Failover of the database leader while applications keep their rules is **not REAL INCUS TESTED**.

Then create the **uplink** network that application routers attach to. With a physical network, create it once per member with `--target`, then once for the cluster, as Incus's guide shows; the settings that matter to Apelora:

```sh
UPLINK=...                       # the uplink's name
GATEWAY=...                      # the uplink network's gateway
ROUTER_RANGE=...                 # free addresses inside the uplink's subnet, for application routers
SERVICE_RANGE=...                # a range your network routes to the uplink, for service addresses

incus network set "$UPLINK" ipv4.ovn.ranges="$ROUTER_RANGE"
incus network set "$UPLINK" ipv4.routes="$SERVICE_RANGE"
# Optional: an IPv6 service range (/112 to /126) also needs ipv6.routes on the uplink
# and the same range in the project's restricted.networks.subnets.
```

What each range is, and how to choose it, is in [servers and network](servers-and-network.md#ip-addresses). The lab does this on a managed NAT bridge for convenience: `scripts/test-cluster/ovn.sh` is the worked example for a single database; `ovn-ha.sh` is the highly available TLS variant (dry-run until applied).

## 5. A project for Apelora, and two identities

Apelora deploys into a **restricted Incus project**, so that the identity it uses can see and change that project and nothing else ([tenancy](../design/tenancy-isolation.md)).

```sh
PROJECT=apelora                  # the project name; `APELORA_DEPLOY_PROJECTS` lists the ones Apelora may use

incus project create "$PROJECT" \
  -c restricted=true -c features.images=false -c features.profiles=true \
  -c features.networks=true -c restricted.networks.uplinks="$UPLINK"     # the last two only with OVN
incus project set "$PROJECT" restricted.snapshots=allow
incus project set "$PROJECT" restricted.cluster.target=allow
incus project set "$PROJECT" restricted.networks.subnets="$UPLINK:$SERVICE_RANGE"                 # with OVN
incus profile device add default root disk path=/ pool=local --project "$PROJECT"
```

`restricted.cluster.target=allow` lets Apelora place and move instances on named servers; `restricted.snapshots=allow` allows volume snapshots. The reasons for each setting are in [tenancy isolation](../design/tenancy-isolation.md). Apelora can also prepare an environment's project itself, with the admin identity below.

Create **two client certificates** and trust them. They are ordinary X.509 client certificates; the lab script `scripts/test-cluster/up.sh` makes both (an ECDSA P-384 key, client authentication only). Why two: [ADR 0003](../adr/0003-incus-identities-and-cluster-join.md).

```sh
DIR=...                          # a directory outside any repository, mode 0700

# make a key and certificate for each identity (the lab script has the exact openssl options)
for NAME in deploy admin; do
  openssl ecparam -name secp384r1 -genkey -noout | openssl pkcs8 -topk8 -nocrypt -out "$DIR/$NAME.key"
  openssl req -new -x509 -key "$DIR/$NAME.key" -subj "/CN=apelora-$NAME" -days 90 -out "$DIR/$NAME.crt" \
    -addext "basicConstraints=critical,CA:FALSE" -addext "keyUsage=critical,digitalSignature" -addext "extendedKeyUsage=clientAuth"
done

# on a cluster member, with the certificates copied there
incus config trust add-certificate "$DIR/deploy.crt" --name apelora-deploy --restricted --projects "$PROJECT"
incus config trust add-certificate "$DIR/admin.crt"  --name apelora-admin
```

The **deploy** identity is restricted to the project and is the one the control plane and the worker use. The **admin** identity is unrestricted and only the control plane gets it, for join tokens and cluster administration. Certificates here last 90 days; [rotation](../runbooks/certificates.md) is a runbook.

Finally, collect what Apelora needs to reach Incus: a server's address (`https://$SERVER_IP:8443`), the **cluster certificate** (`/var/lib/incus/cluster.crt` on a member), which Apelora pins, and the **name inside that certificate** to verify it against (`openssl x509 -in cluster.crt -noout -text | grep DNS:`). They become settings in the next page.

Next: [install Apelora Fabric](installing-apelora.md).
