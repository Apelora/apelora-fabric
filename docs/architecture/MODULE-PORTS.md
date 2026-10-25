# Module port placement

How terminals are arranged on every module, so a diagram reads the same way regardless of which modules are in it. The full list of modules and their ports is generated in [CATALOG.md](CATALOG.md); the geometry (128×96 node, 18 px port pitch) is in [STUDIO-MVP.md](STUDIO-MVP.md). A unit test enforces these rules for every catalog class.

## Placement rules

| Side | Order, top to bottom | Why |
|---|---|---|
| **Left (inputs)** | 1. primary service input (`in`, `http`, `sql`, `kv`…) · 2. secondary service inputs · **`net` last** | Traffic enters on the left; the network card is a fixed, predictable place at the bottom of the inputs |
| **Right (outputs)** | 1. primary downstream (`out`, `db`, `lan`…) · 2. peer / replication links (`repl`, `peer`) · 3. caches, stores, brokers · **`mon` last** | Data leaves on the right; metrics are always the lowest output so one scrape cable pattern fits every module |

- **At most 3 inputs and 4 outputs.** A compact node holds two ports per side in 96 px; a third row grows the node by 18 px (114 px), a fourth by another 18 px. Modules meant to stay compact have one service input plus `net`, and one or two outputs.
- **Short lowercase names (≤ 8 characters)** so the label fits beside the 30 px logo: `in`, `out`, `db`, `cache`, `store`, `mq`, `repl`, `peer`, `mon`, `net`, `wan`, `lan`. Longer explanations live in the port description (tooltip).
- **One protocol per port**, shown as the cable colour. A client output feeds a server input of the same protocol; a port of type `tcp` accepts any layer-4 protocol.
- **Multiplicity:** server inputs accept many cables (many clients); client outputs that name *the* database, origin or store accept one. Replication/peer and metrics outputs accept many.
- **Required:** a server's service input is required (an unused server is flagged); outputs are optional; `net` is always optional (default network).
- **Operating-system images** (Debian, Ubuntu, Alpine, Rocky, AlmaLinux, Fedora, Linux base) have only `net`; add ports in the inspector or designer when you turn one into an application.

## Standard terminals

| Name | Type | Direction | Meaning |
|---|---|---|---|
| `net` | network | in | Network interface (NIC) |
| `wan` / `lan` | network | in / out | Firewall, router and VPN uplink and protected side |
| `in` / `out` | http, or the broker's protocol | in / out | Primary traffic through proxies. A broker has only `in`: producers and consumers are both clients that connect to it |
| `http`, `ui`, `api` | http | in | Web UI or API of an application |
| `db` | postgres, mysql… | out | The application's database (single cable) |
| `cache` | redis, memcached | out | Cache or session store |
| `store` | s3, http | out | Object storage or search backend |
| `mq` | kafka, amqp, redis | out | Message stream or queue |
| `repl` / `peer` | database or broker protocol | out | Replication or cluster links with sibling nodes; always connectable to the service input of another module of the same class |
| `primary` / `bridge` | the service's protocol | out | The server this one copies from (a replica's primary, a DNS secondary's primary) or bridges to (MQTT) |
| `api` / `query` | http | in / out | A query API (Prometheus, Loki, Tempo) and the dashboard output that reads it: Grafana's `query` connects to Prometheus's `api` |
| `dns`, `fwd` | dns | in / out | DNS queries, and the upstream resolvers a DNS server forwards to |
| `peer` / `routes` | bgp | in / out | BGP sessions a routing daemon accepts and opens (TCP 179) |
| `mon` | metrics | out | Metrics endpoint a collector reads (its `scrape` input); only on classes whose metrics are readable as text on a port, see [Metrics](#metrics-the-mon-output) |
| `audit` | syslog | out | Audit log stream; feeds log pipelines such as Logstash |

## Examples

```
Web tier            HAProxy            PostgreSQL           Kafka
in   → ● ┌────┐ ● → out   sql → ● ┌────┐ ● → repl   in → ● ┌────┐ ● → peer
net  → ● │    │ ● → mon   net → ● │    │ ● → mon    net → ● │    │ ● → mon
```

(Inputs on the left edge, outputs on the right edge; `net` is the last left terminal and `mon` the last right terminal.)

## Metrics: the `mon` output

Every catalog class describes how its software really exposes metrics (`ModuleClass.metrics`, schema in `packages/assembly-model/src/schema.ts`, data in `packages/module-catalog/src/metrics.ts`, summarised per class in [CATALOG.md](CATALOG.md)):

| Field | Meaning |
|---|---|
| `source` | `native` (the software serves them, maybe after a setting or with a bundled plugin), `exporter` (an add-on: a separate process, an agent inside the runtime, or a plugin installed separately), `agent` (an optional operating-system agent in the guest), `platform` (the module runs no process; Incus reports its numbers), `none` |
| `port`, `path`, `transport` | Where a collector reads them. `portChosen`: the software has no default, so the class picks the port and its runtime must set it. `portBy`: the port depends on an enum setting (InfluxDB 2.7 → 8086, 3.0 → 8181) |
| `format` | `text` (metrics text a collector reads directly), `json`, `page` (the product's status page), `protocol` (SNMP, LDAP, FastCGI, MQTT, pushed metrics), `counters` (readable only inside the instance), `none` |
| `onByDefault`, `localOnly`, `enable` | Whether they are produced without any setting, whether the default listener answers only on localhost, and how to turn them on |
| `auth`, `separateListener`, `health` | Access (`none`, `basic`, `token`, `certificate`, `configurable`), whether they have a listener of their own, and a health endpoint |
| `signals`, `confidence`, `note` | The few numbers worth watching; `checked`, `partly-checked` or `unverified` against the project's documentation; findings that change how the module is wired |

Rules (enforced by `packages/module-catalog/src/metrics.test.ts`):

1. **A class has a `mon` output exactly when a collector can read its metrics as text on a port** (`readableMetrics`: source `native` or `exporter`, format `text`, a port). Metrics only readable inside the instance (`counters`), over the software's own protocol, as JSON or a status page, or not at all get no `mon` output: read them with an ordinary connection to the service port, or through an exporter module.
2. **A `mon` connection is an authorisation for the collector.** Metrics are read, not sent: the network compiler (`compileNetwork(assembly, app, lookupClass)`) turns a cable from a module's `mon` output to a collector's `scrape` input into a rule that lets the *collector* reach the *module* on the class's metrics port (and transport), never the reverse and never another port. A class without a metrics port (a custom class) falls back to 9100, the common default, and the plan warns.
3. **Endpoints without authentication stay on the application network.** A `mon` connection opens the port to one collector only; never publish a metrics port through the gateway. Listeners that answer only on localhost by default (`localOnly`) need the setting in `enable` changed before a collector can read them.
4. **Operating-system classes** have no `mon` output: Incus already reports each instance's CPU, memory, disk and network (for a VM through its agent inside the guest, otherwise QEMU-level numbers only). An agent can be installed and a `mon` output added in the inspector.
5. **Classes that run no process** (`gateway`, `network`) take their numbers from Incus and OVN (Incus network state and Incus's metrics endpoint), read by Apelora; a module never reaches the Incus API.
6. **No runtime turns metrics on yet.** Turning on, say, NGINX's status page needs a configuration file, and the current runtimes run their images unchanged. Next step: a way for a runtime to add a file or setting at creation, bound to the application network only.

## Adding modules

Add a class in `packages/module-catalog/src/classes-extra.ts` using the shorthand helpers (`i` for inputs, `o` for outputs, `mon()` for metrics), place it in its category and subcategory in `packages/module-catalog/src/taxonomy.ts` (the Studio menu, this reference and the website read that grouping), describe its metrics in `packages/module-catalog/src/metrics.ts`, put the icon in `assets/logos/` (record origin and licence in `ATTRIBUTION.md`, or use a `generic-*` icon), run `npm test` (the catalog-quality tests check placement, icons, settings and schema) and `npm run catalog:doc`.
