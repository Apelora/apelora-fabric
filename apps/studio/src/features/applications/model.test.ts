import { expect, test } from "vitest";

import { DESIGN_LABEL, NEEDS_ATTENTION, RUNTIME_LABEL, healthLine, type AppView } from "./model";

const denied = { allowed: false };
const actions = { migrate: denied, restart: denied, shutdown: denied, start: denied, stop: denied };
const project = { id: "p", name: "Shop", organizationId: "o", organizationSlug: "o", slug: "shop" };

function app(status: AppView["deployment"]["status"], replicas: { desired: number; healthy: number }): AppView {
  return {
    actions,
    archived: false,
    createdAt: "2026-01-01T00:00:00Z",
    description: "",
    icon: "generic-box",
    name: "shop",
    project,
    updatedAt: "2026-01-01T00:00:00Z",
    design: { hasUnpublishedChanges: false, status: "draft", validationErrors: 0 },
    deployment: {
      modules: 1,
      observedAt: "2026-01-01T00:00:00Z",
      replicas: { desired: replicas.desired, healthy: replicas.healthy, running: replicas.healthy, unhealthy: 0 },
      source: "none",
      stale: false,
      status,
    },
  };
}

test("healthLine names the statuses people read on a card", () => {
  expect(healthLine(app("not-deployed", { desired: 2, healthy: 0 }))).toBe("Not deployed");
  expect(healthLine(app("unknown", { desired: 2, healthy: 0 }))).toBe("Unknown");
  expect(healthLine(app("stopped", { desired: 2, healthy: 0 }))).toBe("Not running");
  expect(healthLine(app("running", { desired: 0, healthy: 0 }))).toBe("—");
  expect(healthLine(app("running", { desired: 2, healthy: 2 }))).toBe("2/2 healthy");
  expect(healthLine(app("degraded", { desired: 2, healthy: 1 }))).toBe("1/2 healthy");
});

test("needs-attention and labels keep the words people read", () => {
  expect([...NEEDS_ATTENTION].sort()).toEqual(["degraded", "failed"]);
  expect(RUNTIME_LABEL.running).toBe("Running");
  expect(RUNTIME_LABEL.degraded).toBe("Degraded");
  expect(RUNTIME_LABEL.failed).toBe("Failed");
  expect(RUNTIME_LABEL["not-deployed"]).toBe("Not deployed");
  expect(DESIGN_LABEL.draft).toBe("Draft");
  expect(DESIGN_LABEL.validated).toBe("Validated");
  expect(DESIGN_LABEL.published).toBe("Published");
});
