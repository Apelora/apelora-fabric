// SPDX-License-Identifier: AGPL-3.0-only
import { instanceRequestFor, type ModuleClass } from "@apelora/assembly-model";

import { MODULE_CLASSES } from "./classes";

/** One image a deployable class can use: the image server it names, by setting. */
export interface CatalogImage {
  class: string;
  displayName: string;
  /** The image-selecting setting, e.g. `channel = stable`; absent when the class has one image. */
  setting?: string;
  protocol: "simplestreams" | "oci";
  server: string;
  alias: string;
  fingerprint?: string;
}

/**
 * Every image the catalog's deployable classes use, built the same way the Studio and the
 * Console build their deploy requests (`instanceRequestFor`). The control plane reads this list
 * from `catalog-images.json` (written by `npm run catalog:doc`) to check image mirrors (ADR 0010).
 */
export function catalogImages(classes: readonly ModuleClass[] = MODULE_CLASSES): CatalogImage[] {
  const out: CatalogImage[] = [];
  for (const c of classes) {
    const rt = c.runtime;
    if (!rt) continue;
    const settings = rt.imageBy ? Object.keys(rt.imageBy.images).map((v) => ({ [rt.imageBy!.property]: v })) : [{}];
    for (const config of settings) {
      const r = instanceRequestFor(c, { config }, { project: "apelora", name: "image-check" });
      if (!r.ok) throw new Error(r.error);
      const s = r.request.source;
      const [setting] = Object.entries(config).map(([k, v]) => `${k} = ${v}`);
      out.push({
        class: c.id,
        displayName: c.displayName,
        ...(setting ? { setting } : {}),
        protocol: s.protocol,
        server: s.server,
        alias: s.alias,
        ...(s.fingerprint ? { fingerprint: s.fingerprint } : {}),
      });
    }
  }
  return out;
}

/** The generated file's content (stable formatting, trailing newline). */
export const catalogImagesJson = (classes?: readonly ModuleClass[]) =>
  JSON.stringify({ generatedBy: "npm run catalog:doc (packages/module-catalog/src/images.ts); do not edit by hand", images: catalogImages(classes) }, null, 2) + "\n";
