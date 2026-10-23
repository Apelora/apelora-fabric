import { expect, test } from "vitest";

import type { AppView } from "../../features/applications/model";
import { deployReason } from "./DeployAction";

const denied = { allowed: false };
const actions = { migrate: denied, restart: denied, shutdown: denied, start: denied, stop: denied };
const project = { id: "p", name: "Shop", organizationId: "o", organizationSlug: "o", slug: "shop" };

function app(patch: Partial<AppView> = {}): AppView {
  return {
    actions,
    archived: false,
    createdAt: "2026-01-01T00:00:00Z",
    description: "",
    icon: "generic-box",
    latestRevision: 1,
    name: "shop",
    project,
    updatedAt: "2026-01-01T00:00:00Z",
    design: { hasUnpublishedChanges: false, status: "published", validationErrors: 0 },
    deployment: {
      modules: 1,
      observedAt: "2026-01-01T00:00:00Z",
      replicas: { desired: 1, healthy: 1, running: 1, unhealthy: 0 },
      source: "incus",
      stale: false,
      status: "running",
    },
    ...patch,
  };
}

test("deployReason refuses a local backend, an archive, and a never-published design", () => {
  expect(deployReason(app(), "local")).toMatch(/control plane/);
  expect(deployReason(app({ archived: true }), "api")).toMatch(/archived/);
  expect(deployReason(app({ latestRevision: null }), "api")).toMatch(/published/);
  expect(deployReason(app({ latestRevision: undefined }), "api")).toMatch(/published/);
  expect(deployReason(app(), "api")).toBeNull();
});
