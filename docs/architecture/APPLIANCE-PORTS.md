# Appliance port placement

How terminals are arranged on every appliance, so a diagram reads the same way regardless of which appliances are in it. The full list of appliances and their ports is generated in [CATALOG.md](CATALOG.md); the geometry (128×96 node, 18 px port pitch) is in [STUDIO-MVP.md](STUDIO-MVP.md). A unit test enforces these rules for every catalog class.

## Placement rules

| Side | Order, top to bottom | Why |
|---|---|---|
| **Left (inputs)** | 1. primary service input (`in`, `http`, `sql`, `kv`…) · 2. secondary service inputs · **`net` last** | Traffic enters on the left; the network card is a fixed, predictable place at the bottom of the inputs |
| **Right (outputs)** | 1. primary downstream (`out`, `db`, `lan`…) · 2. peer / replication links (`repl`, `peer`) · 3. caches, stores, brokers · **`mon` last** | Data leaves on the right; metrics are always the lowest output so one scrape cable pattern fits every appliance |

- **At most 3 inputs and 4 outputs.** A compact node holds two ports per side in 96 px; a third row grows the node by 18 px (114 px), a fourth by another 18 px. Appliances meant to stay compact have one service input plus `net`, and one or two outputs.
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
| `in` / `out` | http, or the broker's protocol | in / out | Primary traffic through proxies and brokers |
| `http`, `ui`, `api` | http | in | Web UI or API of an application |
| `db` | postgres, mysql… | out | The application's database (single cable) |
| `cache` | redis, memcached | out | Cache or session store |
| `store` | s3, http | out | Object storage or search backend |
| `mq` | kafka, amqp, redis | out | Message stream or queue |
| `repl` / `peer` | database protocol | out | Replication or cluster gossip with sibling nodes |
| `mon` | metrics | out | Metrics endpoint; feeds Prometheus `scrape` |
| `audit` | syslog | out | Audit log stream; feeds log pipelines such as Logstash |

## Examples

```
Web tier            HAProxy            PostgreSQL           Kafka
in   → ● ┌────┐ ● → out   sql → ● ┌────┐ ● → repl   in → ● ┌────┐ ● → out
net  → ● │    │ ● → mon   net → ● │    │ ● → mon    net → ● │    │ ● → mon
```

(Inputs on the left edge, outputs on the right edge; `net` is the last left terminal and `mon` the last right terminal.)

## Adding appliances

Add a class in `packages/appliance-catalog/src/classes-extra.ts` using the shorthand helpers (`i` for inputs, `o` for outputs, `mon()` for metrics), put the icon in `assets/logos/` (record origin and licence in `ATTRIBUTION.md`, or use a `generic-*` icon), run `npm test` (the catalog-quality tests check placement, icons, settings and schema) and `npm run catalog:doc`.
