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
