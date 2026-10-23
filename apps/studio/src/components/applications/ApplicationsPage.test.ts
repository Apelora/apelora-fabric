import { expect, test } from "vitest";

import type { AppView } from "../../features/applications/model";
import { matches, sortApps } from "./ApplicationsPage";

const denied = { allowed: false };
const actions = { migrate: denied, restart: denied, shutdown: denied, start: denied, stop: denied };
const project = { id: "p", name: "Shop", organizationId: "o", organizationSlug: "o", slug: "shop" };

function app(name: string, patch: Partial<AppView> = {}): AppView {
  return {
    actions,
    archived: false,
    createdAt: "2026-01-01T00:00:00Z",
    description: `${name} web front`,
    icon: "generic-box",
    name,
    project,
    updatedAt: "2026-01-01T00:00:00Z",
    design: { hasUnpublishedChanges: false, status: "draft", validationErrors: 0 },
    deployment: {
      lastActivityAt: "2026-01-01T00:00:00Z",
      modules: 1,
      observedAt: "2026-01-01T00:00:00Z",
      replicas: { desired: 1, healthy: 1, running: 1, unhealthy: 0 },
      source: "none",
      stale: false,
      status: "not-deployed",
    },
    ...patch,
  };
}

test("matches looks at name, description and where the application lives", () => {
  const a = app("checkout", { environment: { id: "e", name: "Prod", slug: "prod", kind: "production" } });
  expect(matches(a, "")).toBe(true);
  expect(matches(a, "  ")).toBe(true);
  expect(matches(a, "CHECK")).toBe(true);
  expect(matches(a, "front")).toBe(true);
  expect(matches(a, "shop")).toBe(true);
  expect(matches(a, "prod")).toBe(true);
  expect(matches(a, "missing")).toBe(false);
});

test("sortApps lists failed before running, and newest first when sorting by update", () => {
  const failed = app("alpha", { deployment: { ...app("alpha").deployment, status: "failed" } });
  const running = app("beta", { deployment: { ...app("beta").deployment, status: "running" } });
  const older = app("old", { updatedAt: "2026-01-01T00:00:00Z", deployment: { ...app("old").deployment, lastActivityAt: "2026-01-01T00:00:00Z" } });
  const newer = app("new", { updatedAt: "2026-10-01T00:00:00Z", deployment: { ...app("new").deployment, lastActivityAt: "2026-10-01T00:00:00Z" } });
  expect(sortApps([running, failed], "status", false).map((x) => x.name)).toEqual(["alpha", "beta"]);
  expect(sortApps([older, newer], "updated", false).map((x) => x.name)).toEqual(["new", "old"]);
  expect(sortApps([newer, older], "name", false).map((x) => x.name)).toEqual(["new", "old"]);
  expect(sortApps([newer, older], "name", true).map((x) => x.name)).toEqual(["old", "new"]);
});
