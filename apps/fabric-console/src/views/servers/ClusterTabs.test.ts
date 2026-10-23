import { expect, test } from "vitest";

import { expiry } from "./ClusterTabs";

test("expiry names remaining days and points at the runbook once it is close", () => {
  const expiresAt = "2026-12-01T00:00:00Z";
  expect(expiry({ certificate: "deploy", daysLeft: 90, expiresAt, state: "ok" })).toMatch(/90 days left/);
  expect(expiry({ certificate: "deploy", daysLeft: 1, expiresAt, state: "ok" })).toMatch(/1 day left/);
  expect(expiry({ certificate: "deploy", daysLeft: 12, expiresAt, state: "expiring" })).toMatch(/rotate it soon/);
  expect(expiry({ certificate: "deploy", daysLeft: -3, expiresAt, state: "expired" })).toMatch(/^Expired /);
  expect(expiry({ certificate: "deploy", daysLeft: -3, expiresAt, state: "expired" })).toContain("docs/runbooks/certificates.md");
});
