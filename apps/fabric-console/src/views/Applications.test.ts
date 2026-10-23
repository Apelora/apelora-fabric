import { expect, test } from "vitest";

import type { ModuleStatus } from "@apelora/api-client";

import { size, summary } from "./Applications";

const item = (status: string, health?: ModuleStatus["health"]): ModuleStatus => ({
  instance: "shop-web-01",
  location: "n1",
  module: "web",
  project: "apelora",
  status,
  health: health ? { detail: health.health, ...health } : health,
});

test("summary names an empty, healthy, mixed and fully failed set", () => {
  expect(summary([])).toEqual({ label: "No instances", dot: "status-neutral" });
  expect(summary([item("Running", { health: "healthy", since: "2026-01-01T00:00:00Z" })])).toEqual({
    label: "All running",
    dot: "status-success",
  });
  const mixed = summary([
    item("Running", { health: "unhealthy", since: "2026-01-01T00:00:00Z" }),
    item("Stopped"),
  ]);
  expect(mixed.dot).toBe("status-warning");
  expect(mixed.label.toLowerCase()).toContain("failing");
  expect(mixed.label.toLowerCase()).toContain("not running");
  expect(summary([item("Stopped"), item("Stopped")])).toEqual({
    label: "2 not running",
    dot: "status-error",
  });
});

test("size uses the largest whole binary unit", () => {
  expect(size(512)).toBe("512 MiB");
  expect(size(1024)).toBe("1 GiB");
  expect(size(1536)).toBe("1536 MiB");
  expect(size(1024 * 1024)).toBe("1 TiB");
});
