# Install Incus and form the cluster

Apelora Fabric runs your modules on [Incus](https://linuxcontainers.org/incus/), the open-source system that runs containers and virtual machines on a cluster of Linux servers. Apelora does not install or upgrade Incus for you. This page shows how to install Incus on each server, form the cluster, and give Apelora a project to deploy into and the certificates to reach it. Read [servers and network](servers-and-network.md) first.

Run the commands as root (or with `sudo`) on the server named in each step. Values you choose are written as variables, for example `$SERVER_IP`; set them in your shell first.

## Where to get Incus

| What | Where |
|---|---|
| **Incus installation guide** (every supported distribution) | <https://linuxcontainers.org/incus/docs/main/installing/> |
| **Release downloads and notes** | <https://github.com/lxc/incus/releases> |
| **Packages for Debian and Ubuntu** (the project's own repository, which carries the LTS series) | <https://github.com/zabbly/incus>, repository at <https://pkgs.zabbly.com/incus/> |
| **Forming a cluster** | <https://linuxcontainers.org/incus/docs/main/howto/cluster_form/> |
| **Setting up OVN** | <https://linuxcontainers.org/incus/docs/main/howto/network_ovn_setup/> |
| **The series Apelora is tested with** | `series` and `pinned_tag` in [`upstream.lock.yaml`](../../upstream.lock.yaml) |

Use the long-term support (LTS) series that `upstream.lock.yaml` pins, and the same release on every server. Newer feature releases are watched but not tested. Moving to another LTS series is a deliberate decision with its own record ([upstream Incus](../development/UPSTREAM-INCUS.md)).

## 1. Install Incus on every server

There are two ways. Both end with `incus --version` printing the pinned series.

### From the project's package repository

This route gives the pinned LTS series on Debian and Ubuntu.

1. Set the series from `upstream.lock.yaml`.
2. Fetch the repository's signing key, and **compare its fingerprint with the one the publisher shows**. Stop if they differ.
3. Add the repository and install.

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

The `incus` package includes virtual machine support. If you will run virtual machines, check that `ls /dev/kvm` finds the device.

### From your distribution

Some distributions package Incus themselves. Use that package only if `apt policy incus` (or your package manager's equivalent) offers the pinned series: distributions often carry an older LTS. The [installation guide](https://linuxcontainers.org/incus/docs/main/installing/) lists the commands for each distribution.

You can install only the client on a workstation to look around. The Incus daemon runs only on Linux.

## 2. Prepare each server

- **Address and clock:** a static IP address and a host name. Clocks must be synchronised: `timedatectl` should say the clock is synchronised.
- **Firewall:** between the servers, allow TCP 8443 (Incus), UDP 6081 (OVN tunnels) and, if OVN's databases are on them, TCP 6641 and 6642. From the machine that will run Apelora, allow TCP 8443 to the servers. [Servers and network](servers-and-network.md#network-layouts) has the full list.
- **Storage:** a disk or partition for the storage pool. A file-backed pool also works, for a trial only.

## 3. Form the cluster

Set up the first server, then add the others. Give the cluster a **specific** address: a cluster does not accept the wildcard address as its own.

### The first server

Write a preseed file with your values, then apply it:

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

`incus cluster list` shows this one server, online.

`local` is the pool name that the examples and Apelora's settings use. Choose the storage driver your servers should have. Incus supports several; Apelora asks Incus to create and grow volumes in the pools it is told to use, and does not manage the driver ([storage](../architecture/STORAGE.md)).

You can also run this step interactively with `incus admin init`: answer yes to clustering, give the address, and say you are not joining an existing cluster.

Then shorten how long join tokens live (the default is three hours):

```sh
incus config set cluster.join_token_expiry=30M
```

### Every other server

On a **member already in the cluster**, create a single-use token for each new server:

```sh
NEW_MEMBER=...          # the name the new server will have
incus cluster add "$NEW_MEMBER"      # prints the token
```

On the **new server**, install Incus first. It must be a fresh installation, because joining deletes what is on it. Then join with the token:

```sh
NEW_MEMBER=...          # the same name the token was created for
SERVER_IP=...           # the new server's own address
LEADER=...              # a member that is already in the cluster
TOKEN=...               # from the step above; paste it, never keep it in a file or script

cat >join.yaml <<EOF
cluster:
  enabled: true
  server_name: $NEW_MEMBER
  server_address: $SERVER_IP:8443
  cluster_address: $LEADER:8443
  cluster_token: $TOKEN
  member_config:
  - entity: storage-pool
    name: local
    key: size
    value: 20GiB
  - entity: storage-pool
    name: local
    key: source
    value: ""
EOF
incus admin init --preseed <join.yaml
rm -f join.yaml
```

A sized loop pool asks the new member for `size` and `source`. An empty `source` tells Incus to create the loop file. A pool on a real disk sets `source` to that device and does not set `size`. Delete `join.yaml` after the join: it held the token.

The same loop-pool answers from the enrolment script, which also holds the member out of placement. The token stays on standard input:

```sh
printf '%s\n' "$TOKEN" | sudo bash enrol-member.sh --name "$NEW_MEMBER" --address "$SERVER_IP" --pool-size 20GiB
```

`incus cluster list` on any member now shows every server online, and three of them with the database role.

Servers you add **later** are better enrolled through Apelora. It registers and approves a new server, and holds it until it passes its checks: [add a server to the cluster](../runbooks/member-join.md).

## 4. Networking for applications (OVN)

OVN (Open Virtual Network) is the virtual network layer Incus uses. Apelora gives every application its own OVN network, and turns the cables you draw into access rules on it. This needs OVN on the servers and an **uplink**: the network that application routers attach to.

Without them, Apelora still deploys, but it cannot enforce the connections you draw, and the deploy dialog says so. You can skip this step to try deployment first. Add it before you rely on the connections.

Follow Incus's own guide for OVN: [setting up OVN](https://linuxcontainers.org/incus/docs/main/howto/network_ovn_setup/). It covers a highly available set of OVN databases on three servers, which is what a real installation should use. Also put **TLS on the database connections**: Incus's outline uses plaintext `tcp:` listeners (`--db-nb-create-insecure-remote=yes`), and a real installation should not. In outline:

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

On Debian, `ovn-central.service` does nothing by itself. To stop or restart the databases, use `ovn-ovsdb-server-nb` and `ovn-ovsdb-server-sb`.

Then create the **uplink** network that application routers attach to. With a physical network, create it once per member with `--target`, then once for the cluster, as Incus's guide shows. These are the settings that matter to Apelora:

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

[Servers and network](servers-and-network.md#ip-addresses) explains what each range is and how to choose it.

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

`restricted.cluster.target=allow` lets Apelora place and move instances on named servers. `restricted.snapshots=allow` allows volume snapshots. [Tenancy isolation](../design/tenancy-isolation.md) gives the reasons for each setting. Apelora can also prepare an environment's project itself, with the admin identity below.

Next, create **two client certificates** and trust them. They are ordinary X.509 client certificates, each with an ECDSA P-384 key, for client authentication only. [ADR 0003](../adr/0003-incus-identities-and-cluster-join.md) explains why there are two.

```sh
DIR=...                          # a directory outside any repository, mode 0700

# make a key and certificate for each identity
for NAME in deploy admin; do
  openssl ecparam -name secp384r1 -genkey -noout | openssl pkcs8 -topk8 -nocrypt -out "$DIR/$NAME.key"
  openssl req -new -x509 -key "$DIR/$NAME.key" -subj "/CN=apelora-$NAME" -days 90 -out "$DIR/$NAME.crt" \
    -addext "basicConstraints=critical,CA:FALSE" -addext "keyUsage=critical,digitalSignature" -addext "extendedKeyUsage=clientAuth"
done

# on a cluster member, with the certificates copied there
incus config trust add-certificate "$DIR/deploy.crt" --name apelora-deploy --restricted --projects "$PROJECT"
incus config trust add-certificate "$DIR/admin.crt"  --name apelora-admin
```

- The **deploy** identity is restricted to the project. The control plane and the worker use it.
- The **admin** identity is unrestricted. Only the control plane gets it, for join tokens and cluster administration.

These certificates last 90 days. Rotating them has its own runbook: [rotation](../runbooks/certificates.md).

Finally, collect what Apelora needs to reach Incus. They become settings on the next page.

1. A server's address: `https://$SERVER_IP:8443`.
2. The **cluster certificate**, `/var/lib/incus/cluster.crt` on a member. Apelora pins it.
3. The **name inside that certificate**, to verify it against: `openssl x509 -in cluster.crt -noout -text | grep DNS:`.

Next: [install Apelora Fabric](installing-apelora.md).

## Before you rely on it

- These steps have been followed on virtual machines with one network card each. Physical servers and a physical uplink have not been tried yet; those parts follow Incus's own documentation. Plan a trial on your own servers before production.
