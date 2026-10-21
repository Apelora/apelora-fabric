// SPDX-License-Identifier: AGPL-3.0-only
import { readFileSync } from "node:fs";

import { describe, expect, it } from "vitest";

import { catalogImages, catalogImagesJson } from "./images";

describe("catalog images", () => {
  it("lists every image of every deployable class, by setting", () => {
    const images = catalogImages();
    expect(images.filter((i) => i.class === "nginx").map((i) => [i.setting, i.protocol, i.server])).toEqual([
      ["channel = stable", "oci", "https://docker.io"],
      ["channel = mainline", "oci", "https://docker.io"],
    ]);
    expect(images.find((i) => i.class === "linux-base")).toMatchObject({ protocol: "simplestreams", server: "https://images.linuxcontainers.org", alias: "debian/13" });
    expect(images.find((i) => i.class === "linux-base")?.setting).toBeUndefined();
  });

  it("the file the control plane reads is current (regenerate with npm run catalog:doc)", () => {
    const committed = readFileSync(new URL("../catalog-images.json", import.meta.url), "utf8");
    expect(committed).toBe(catalogImagesJson());
  });
});
