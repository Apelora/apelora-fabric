import type { Runtime } from "@apelora/assembly-model";

/**
 * How catalog classes run on Incus. Only images that have run on the disposable test cluster
 * are listed (docs/development/TEST-CLUSTER.md); every other class stays design-only until its
 * image has been tried. OCI images are pinned by digest; system images by alias, because
 * public image servers drop old builds (pin a fingerprint from an image server you control).
 */
export const RUNTIMES: Readonly<Record<string, Runtime>> = {
  // Official nginx images, multi-architecture index digests resolved on 2026-10-30 (stable 1.30.6 is still the newest stable).
  // Serves files and proxies requests; keeps no data, so replicas are interchangeable.
  nginx: {
    kind: "incus-oci-container",
    availability: "stateless",
    imageBy: {
      property: "channel",
      images: {
        stable: { registry: "https://docker.io", reference: "library/nginx@sha256:eb1c896deef834f4e056829fd6dadac8305501f16cc71ba65fb252c8d0aa168e", version: "1.30.6" },
        mainline: { registry: "https://docker.io", reference: "library/nginx@sha256:c4d1df1ca40ef8cf932f19853f00eb0f81ffe5becda1256365bbf0a057060ffb", version: "1.31.8" },
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
  // Multi-architecture index digest resolved on 2026-10-30 (tag 9). Its settings become flags. The
  // image listens on every address with protected mode off and no password (checked on the test
  // cluster): the application network admits only the modules drawn to it. Snapshots and the
  // append-only file go to /data, kept on a volume.
  valkey: {
    kind: "incus-oci-container",
    image: { registry: "https://docker.io", reference: "valkey/valkey@sha256:3add1a336a920ea54770a001ab6feb3da5fceea96a40373a3a2cd469b815f0b7", version: "9.1.3" },
    command: ["docker-entrypoint.sh", "valkey-server", "--maxmemory", "{maxmemory}mb", "--maxmemory-policy", "{eviction}", "--appendonly", "{append-only:yes|no}"],
    volumes: [{ name: "data", sizeMiB: 1024, path: "/data" }],
  },
  // Multi-architecture index digest resolved on 2026-10-30 (tag 2). Monitors, status pages and
  // its database live in /app/data; they are set up in its own web UI.
  "uptime-kuma": {
    kind: "incus-oci-container",
    availability: "stateful",
    image: { registry: "https://docker.io", reference: "louislam/uptime-kuma@sha256:20abea8c48a163cf0a7a5d5936ab418e4b742d908704e1f58f6c06e56a9c6b67", version: "2.5.8" },
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
