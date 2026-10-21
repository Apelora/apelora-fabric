import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page, type Route } from "@playwright/test";

import { servers as healthServers, server as healthServer } from "./health-fixtures";
import { NOW, groups, members, noRecovery, settings } from "./servers-fixtures";

// Changing a cluster setting from Servers → Cluster settings, against a stubbed control plane:
// who sees the button, the review (a dry run), the approval, the apply and the errors.

const health = { status: "ok", service: "control-plane", version: "0.1.0", apiVersion: "v1", provider: "incus", auth: "tokens", incus: { connected: true }, checkedAt: NOW };
const person = { name: "alex", kind: "user", id: "u1", credential: "identity-provider", role: null, permissions: ["read-inventory"], capabilities: ["cluster.view"], platformPermissions: ["cluster.view", "cluster.sensitive_change"], roles: [], deployProjects: ["apelora"], localAccount: false };
const viewer = { ...person, platformPermissions: ["cluster.view"] };
const legacy = { ...person, kind: "legacy-token", id: null, credential: "api-token", role: "fabric-admin", permissions: ["read-inventory", "manage-infrastructure"], platformPermissions: [] };
const TARGET = "cluster-settings:cluster.max_voters";
const APPROVAL_ID = "99999999-2222-4333-8444-555555555555";

interface Rig {
  patches: { body: { changes: Record<string, string | null>; dryRun: boolean }; approval: string | null }[];
  approvals: Record<string, unknown>[];
  requested: Record<string, unknown>[];
  /** What the approvals list says; the test changes it when "another person approves". */
  state: "pending" | "approved";
  patchError?: { status: number; code: string; message: string };
}

async function install(page: Page, who: unknown, rig: Rig) {
  await page.route("**/api/**", async (route: Route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, json: body });
    await new Promise((r) => setTimeout(r, 40));
    if (p === "/api/v1/health") return json(health);
    if (p === "/api/v1/whoami") return json(who);
    if (p === "/api/v1/cluster/members") return json(members());
    if (p === "/api/v1/cluster/groups") return json(groups());
    if (p === "/api/v1/cluster/recovery") return json(noRecovery());
    if (p === "/api/v1/health/servers") return json(healthServers("incus", [healthServer("node-1")]));
    if (p === "/api/v1/cluster/settings" && req.method() === "GET") return json(settings());
    if (p === "/api/v1/cluster/settings" && req.method() === "PATCH") {
      const body = req.postDataJSON() as { changes: Record<string, string | null>; dryRun: boolean };
      rig.patches.push({ body, approval: req.headers()["apelora-approval"] ?? null });
      if (rig.patchError) return json({ error: { code: rig.patchError.code, message: rig.patchError.message } }, rig.patchError.status);
      const [key, to] = Object.entries(body.changes)[0]!;
      return json({ applied: !body.dryRun, approvalTarget: `cluster-settings:${key}`, changes: [{ key, label: "Database voters", from: null, to, warnings: key === "cluster.max_voters" ? ["Changing database roles moves the cluster database between members. Do it with every member online."] : [] }] });
    }
    if (p === "/api/v1/approvals" && req.method() === "POST") {
      rig.requested.push(req.postDataJSON());
      return json({ id: APPROVAL_ID, action: "cluster.sensitive_change", target: TARGET, requester: "user:u1", approver: null, state: "pending", reason: "x", createdAt: NOW, expiresAt: "2099-01-01T00:00:00Z" }, 201);
    }
    if (p === "/api/v1/approvals") {
      return json({ items: rig.requested.length ? [{ id: APPROVAL_ID, action: "cluster.sensitive_change", target: TARGET, requester: "user:u1", approver: rig.state === "approved" ? "user:u2" : null, state: rig.state, reason: "x", createdAt: NOW, expiresAt: "2099-01-01T00:00:00Z" }] : [] });
    }
    return json({ error: { code: "not_found", message: `unstubbed ${req.method()} ${p}` } }, 404);
  });
}

const rig = (over: Partial<Rig> = {}): Rig => ({ patches: [], approvals: [], requested: [], state: "pending", ...over });
const row = (page: Page, label: string) => page.getByTestId("cluster-setting").filter({ hasText: label });

test.beforeEach(async ({ page }) => {
  await page.route("**/api/**", (route) => route.abort());
  await page.emulateMedia({ reducedMotion: "reduce" });
});

test("only people who may change settings see the button; everyone else reads", async ({ page }) => {
  await install(page, viewer, rig());
  await page.goto("/#/servers/settings");
  await expect(page.getByTestId("cluster-setting")).toHaveCount(4);
  await expect(page.getByTestId("change-setting-open")).toHaveCount(0);
  await expect(page.getByTestId("cluster-settings")).toContainText("Read-only here");

  const r = rig();
  await page.unroute("**/api/**");
  await install(page, person, r);
  await page.reload();
  await expect(page.getByTestId("change-setting-open")).toHaveCount(4);
  await expect(page.getByTestId("cluster-settings")).toContainText("another person's approval");
});

test("review, ask for approval, wait for it, then apply the change", async ({ page }) => {
  const r = rig();
  await install(page, person, r);
  await page.goto("/#/servers/settings");
  await row(page, "Database voters").getByTestId("change-setting-open").click();
  const dialog = page.getByTestId("change-setting");
  await expect(dialog).toBeVisible();
  await dialog.getByTestId("setting-value").fill("5");
  await dialog.getByTestId("setting-review-btn").click();

  // The review is a dry run: nothing is changed yet, and the warning is shown.
  await expect(dialog.getByTestId("setting-review")).toBeVisible();
  expect(r.patches).toEqual([{ body: { changes: { "cluster.max_voters": "5" }, dryRun: true }, approval: null }]);
  await expect(dialog.getByTestId("setting-warning")).toContainText("every member online");
  await expect(dialog.getByTestId("setting-apply")).toBeDisabled();

  await dialog.getByTestId("approval-reason").fill("more copies of the database");
  await dialog.getByTestId("approval-request").click();
  await expect(dialog.getByTestId("approval-state")).toHaveAttribute("data-state", "pending");
  expect(r.requested).toEqual([{ action: "cluster.sensitive_change", target: TARGET, reason: "more copies of the database" }]);
  await expect(dialog.getByTestId("setting-apply")).toBeDisabled();

  // Another person approves; the dialog notices when asked, and the change can go ahead.
  r.state = "approved";
  await dialog.getByTestId("approval-check").click();
  await expect(dialog.getByTestId("approval-state")).toHaveAttribute("data-state", "approved");
  await expect(dialog.getByTestId("setting-apply")).toBeEnabled();
  await dialog.getByTestId("setting-apply").click();
  await expect(dialog.getByTestId("setting-done")).toContainText("is now 5");
  expect(r.patches[1]).toEqual({ body: { changes: { "cluster.max_voters": "5" }, dryRun: false }, approval: APPROVAL_ID });
});

test("leaving the value empty goes back to Incus's default", async ({ page }) => {
  const r = rig();
  await install(page, person, r);
  await page.goto("/#/servers/settings");
  // A setting that is set: its row offers a value to clear.
  const target = row(page, "Join tokens expire after");
  await target.getByTestId("change-setting-open").click();
  await page.getByTestId("setting-value").fill("");
  await page.getByTestId("setting-review-btn").click();
  await expect(page.getByTestId("setting-review")).toBeVisible();
  expect(Object.values(r.patches[0]!.body.changes)[0]).toBeNull();
});

test("a legacy administrator token applies without an approval step", async ({ page }) => {
  const r = rig();
  await install(page, legacy, r);
  await page.goto("/#/servers/settings");
  await row(page, "Database voters").getByTestId("change-setting-open").click();
  const dialog = page.getByTestId("change-setting");
  await dialog.getByTestId("setting-value").fill("5");
  await dialog.getByTestId("setting-review-btn").click();
  await expect(dialog.getByTestId("setting-approval")).toHaveCount(0);
  await dialog.getByTestId("setting-apply").click();
  await expect(dialog.getByTestId("setting-done")).toBeVisible();
  expect(r.patches[1]).toEqual({ body: { changes: { "cluster.max_voters": "5" }, dryRun: false }, approval: null });
});

test("a refused value shows the server's reason and nothing moves on", async ({ page }) => {
  const r = rig({ patchError: { status: 400, code: "invalid_setting", message: "cluster.max_voters is 0 to 5" } });
  await install(page, person, r);
  await page.goto("/#/servers/settings");
  await row(page, "Database voters").getByTestId("change-setting-open").click();
  await page.getByTestId("setting-value").fill("9");
  await page.getByTestId("setting-review-btn").click();
  await expect(page.getByTestId("setting-error")).toContainText("cluster.max_voters is 0 to 5");
  await expect(page.getByTestId("setting-review")).toHaveCount(0);
});

test("the dialog closes with Escape and has no serious accessibility violations", async ({ page }) => {
  await install(page, person, rig());
  await page.goto("/#/servers/settings");
  await row(page, "Database voters").getByTestId("change-setting-open").click();
  await page.getByTestId("setting-value").fill("5");
  await page.getByTestId("setting-review-btn").click();
  await expect(page.getByTestId("setting-review")).toBeVisible();
  const results = await new AxeBuilder({ page }).include("[data-testid=change-setting]").withTags(["wcag2a", "wcag2aa"]).analyze();
  expect(results.violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id)).toEqual([]);
  await page.keyboard.press("Escape");
  await expect(page.getByTestId("change-setting")).toHaveCount(0);
});
