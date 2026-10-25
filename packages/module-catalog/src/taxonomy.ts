import { SUBCATEGORIES, type Category } from "@apelora/assembly-model";

type BuiltIn = Exclude<Category, "Custom">;

/**
 * Where every built-in class sits: category → subcategory → class ids, in display order. The
 * Studio's left menu, the generated catalog reference (docs/architecture/CATALOG.md) and the
 * website read this through each class's `category` and `subcategory`. Subcategory names come
 * from `SUBCATEGORIES` in the assembly model; a test keeps the two lists identical and checks
 * that every class appears exactly once. Keep each subcategory to a handful of tiles.
 */
export const CATALOG_TREE: Readonly<Record<BuiltIn, Readonly<Record<string, readonly string[]>>>> = {
  Networking: {
    "Application network": ["gateway", "network", "resolver"],
    Routing: ["frr", "bird", "keepalived"],
    "Load balancing": ["haproxy", "traefik", "envoy", "varnish"],
    "DNS & discovery": ["coredns", "powerdns", "unbound", "bind9", "consul"],
    "DHCP & IPAM": ["kea", "netbox"],
    VPN: ["wireguard", "openvpn", "strongswan", "headscale"],
    Firewalls: ["opnsense", "pfsense", "openwrt"],
  },
  "Runtimes & OS": {
    "Web servers": ["nginx", "apache", "caddy", "tomcat"],
    "Language runtimes": ["nodejs", "python", "openjdk", "php", "go"],
    "Linux systems": ["linux-base", "debian", "ubuntu", "alpine", "rocky", "almalinux", "fedora", "opensuse"],
  },
  Data: {
    Relational: ["postgresql", "mariadb", "mysql", "cockroachdb", "yugabytedb", "pgbouncer"],
    "Key-value & cache": ["redis", "valkey", "memcached", "etcd"],
    Document: ["mongodb", "ferretdb", "couchdb", "arangodb"],
    "Wide-column & graph": ["cassandra", "scylladb", "neo4j"],
    Search: ["elasticsearch", "opensearch", "solr", "meilisearch"],
    Analytics: ["clickhouse", "influxdb", "trino", "spark", "airflow", "superset"],
    "Object & file storage": ["minio", "garage", "seaweedfs", "ceph", "nfs", "samba"],
  },
  Messaging: {
    "Brokers & queues": ["rabbitmq", "nats", "activemq", "mosquitto"],
    Streaming: ["kafka", "pulsar", "debezium", "flink"],
  },
  Observability: {
    Metrics: ["prometheus", "victoriametrics", "node-exporter", "blackbox-exporter", "zabbix"],
    Logs: ["loki", "vector", "fluent-bit", "logstash"],
    Traces: ["otel", "jaeger", "tempo"],
    Dashboards: ["grafana", "kibana", "opensearch-dashboards"],
    "Alerting & uptime": ["alertmanager", "uptime-kuma"],
  },
  "Security & identity": {
    "Identity providers": ["keycloak", "authentik", "authelia", "zitadel", "pocketid"],
    Directories: ["openldap", "freeipa"],
    "Secrets & certificates": ["vault", "openbao", "step-ca", "vaultwarden"],
    "Threat detection": ["crowdsec", "suricata", "wazuh"],
  },
  "Developer tools": {
    "Source control": ["gitea", "forgejo", "gitlab"],
    "Build & release": ["jenkins", "woodpecker", "harbor", "registry"],
  },
  Applications: {
    Collaboration: ["nextcloud", "mattermost", "synapse", "jitsi"],
    "Websites & wikis": ["wordpress", "ghost", "drupal", "mediawiki"],
    Mail: ["postfix", "dovecot", "roundcube"],
    Business: ["odoo", "matomo"],
  },
  "Microsoft Windows": {
    "Windows systems": ["windows-server", "windows-11", "windows-10"],
    "Directory & access": ["active-directory", "rd-gateway"],
    "Servers & apps": ["iis", "sql-server", "exchange", "smb-files", "dotnet"],
  },
};

export interface Place { category: BuiltIn; subcategory: string; order: number }

/** Each class id's place and its position in display order. */
export const PLACES: ReadonlyMap<string, Place> = (() => {
  const out = new Map<string, Place>();
  for (const [category, subs] of Object.entries(CATALOG_TREE) as [BuiltIn, Record<string, readonly string[]>][]) {
    for (const [subcategory, ids] of Object.entries(subs)) {
      if (!SUBCATEGORIES[category].includes(subcategory)) throw new Error(`taxonomy: ${category} has no subcategory ${subcategory}`);
      for (const id of ids) {
        if (out.has(id)) throw new Error(`taxonomy: ${id} is placed twice`);
        out.set(id, { category, subcategory, order: out.size });
      }
    }
  }
  return out;
})();

export function placeOf(id: string): Place {
  const place = PLACES.get(id);
  if (!place) throw new Error(`taxonomy: built-in class ${id} has no place in CATALOG_TREE (packages/module-catalog/src/taxonomy.ts)`);
  return place;
}
