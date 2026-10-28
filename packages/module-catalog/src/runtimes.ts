import type { Runtime } from "@apelora/assembly-model";

/**
 * How catalog classes run on Incus. Only images that have run on the disposable test cluster
 * are listed (docs/development/TEST-CLUSTER.md); every other class stays design-only until its
 * image has been tried. OCI images are pinned by digest; system images by alias, because
 * public image servers drop old builds (pin a fingerprint from an image server you control).
 */
export const RUNTIMES: Readonly<Record<string, Runtime>> = {
  // Official nginx images, multi-architecture index digests resolved on 2026-10-20.
  // Serves files and proxies requests; keeps no data, so replicas are interchangeable.
  nginx: {
    kind: "incus-oci-container",
    availability: "stateless",
    imageBy: {
      property: "channel",
      images: {
        stable: { registry: "https://docker.io", reference: "library/nginx@sha256:eb1c896deef834f4e056829fd6dadac8305501f16cc71ba65fb252c8d0aa168e", version: "1.30.6" },
        mainline: { registry: "https://docker.io", reference: "library/nginx@sha256:bc4cad49969aaf95eb4eecec5572846f86e59f1e8332aba36ed64b91aff8b724", version: "1.31.7" },
      },
    },
  },
  // A Debian 13 virtual machine: moved running between members on the test cluster
  // (docs/design/mobility-and-ha.md).
  "linux-base": {
    kind: "incus-vm",
    image: { server: "https://images.linuxcontainers.org", alias: "debian/13" },
  },
  // Optional (ADR 0004): not added to an application unless the design includes it.
  // The worker installs apelora-dns and points the network's nameservers at this instance.
  resolver: {
    kind: "incus-system-container",
    image: { server: "https://images.linuxcontainers.org", alias: "debian/13" },
  },
  // Multi-architecture index digest resolved on 2026-10-25 (tag 9). Its settings become flags. The
  // image listens on every address with protected mode off and no password (checked on the test
  // cluster): the application network admits only the modules drawn to it. Snapshots and the
  // append-only file go to /data, kept on a volume.
  valkey: {
    kind: "incus-oci-container",
    image: { registry: "https://docker.io", reference: "valkey/valkey@sha256:418652cfb58ef879d4978c33553735d7147016032d5aefaa14c828e611eb9dfd", version: "9.1.2" },
    command: ["docker-entrypoint.sh", "valkey-server", "--maxmemory", "{maxmemory}mb", "--maxmemory-policy", "{eviction}", "--appendonly", "{append-only:yes|no}"],
    volumes: [{ name: "data", sizeMiB: 1024, path: "/data" }],
  },
  // Multi-architecture index digest resolved on 2026-10-25 (tag 2). Monitors, status pages and
  // its database live in /app/data; they are set up in its own web UI.
  "uptime-kuma": {
    kind: "incus-oci-container",
    availability: "stateful",
    image: { registry: "https://docker.io", reference: "louislam/uptime-kuma@sha256:4b95dbe7ac5343b768f8e04e3035ccd59997872b1a35ab9b0f082d2cc279070e", version: "2.5.7" },
    volumes: [{ name: "data", sizeMiB: 1024, path: "/app/data" }],
  },
  // Multi-architecture index digest resolved on 2026-10-25. Keeps nothing; the check and its
  // target are chosen per request by whatever asks it (`/probe?module=…&target=…`).
  "blackbox-exporter": {
    kind: "incus-oci-container",
    availability: "stateless",
    image: { registry: "https://docker.io", reference: "prom/blackbox-exporter@sha256:ea1c7aa3ef3f307cd03e85f34abb604478ccb90b567fc690da0cd9776da76b13", version: "0.29.1" },
  },
  debian: {
    kind: "incus-system-container",
    imageBy: {
      property: "version",
      images: {
        "12": { server: "https://images.linuxcontainers.org", alias: "debian/12" },
        "13": { server: "https://images.linuxcontainers.org", alias: "debian/13" },
      },
    },
  },
};
