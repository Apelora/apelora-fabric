import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { ACME, GLOBEX, NOW, SHOP, base, error, stub, uid, whoami, type Call } from "./identity-fixtures";

// Tenancy & quotas (ADR 0012) against a stubbed control plane: each environment's Incus project
// with its state, restrictions and limit use; quotas with their use; setting a quota with the
// server's checks; preparing an Incus project; permissions, errors, the phone layout and axe.

const GIB = 1024 ** 3;
const PROD_ID = uid(30);
const DEV_ID = uid(31);

const restrictions = {
  restricted: "true", "restricted.containers.nesting": "block", "restricted.containers.privilege": "unprivileged",
  "restricted.devices.disk": "managed", "restricted.devices.gpu": "block", "features.networks": "true",
};
const prod = (extra: Record<string, unknown> = {}) => ({
  id: PROD_ID, slug: "prod", name: "Production", kind: "production", incusProject: "acme-shop-prod", incusState: "created",
  incusDetail: "Incus project acme-shop-prod created by Apelora with the baseline restrictions", checkedAt: NOW,
  quota: null, effective: { instances: 10, cpu: 8, memoryGib: 32, diskGib: 500 }, apeloraLimits: { "limits.cpu": "8" },
  incus: {
    restrictions, limits: { "limits.cpu": "8", "limits.instances": "10" },
    usage: { instances: { used: 3, limit: 10 }, cpu: { used: 6, limit: 8 }, memory: { used: 6 * GIB, limit: 32 * GIB }, disk: { used: 30 * GIB, limit: null } },
  },
  incusError: null, findings: [], canSetQuota: true, canPrepare: true, ...extra,
});
const dev = (extra: Record<string, unknown> = {}) => ({
  id: DEV_ID, slug: "dev", name: "Development", kind: "development", incusProject: "acme-shop-dev", incusState: "pending",
  incusDetail: "Incus: connection refused", checkedAt: NOW, quota: { instances: 2 }, effective: { instances: 2, cpu: 8, memoryGib: 32, diskGib: 500 },
  apeloraLimits: {}, incus: null, incusError: "the Incus project does not exist yet",
  findings: [{ severity: "warning", key: "restricted.devices.gpu", message: "restricted.devices.gpu is allow: looser than the block of projects Apelora creates" }],
  canSetQuota: true, canPrepare: true, ...extra,
});
const overview = (o: { admin?: boolean; envs?: unknown[]; orgCan?: boolean; projectCan?: boolean; orgUsage?: unknown } = {}) => ({
  source: { provider: "mock", observedAt: NOW }, adminConfigured: o.admin ?? true,
  baseline: restrictions,
  organizations: [
    {
      id: ACME.id, slug: "acme", name: "Acme Corp", quota: { instances: 10, cpu: 16, memoryGib: 64, diskGib: 500 },
      usage: o.orgUsage === undefined ? { instances: 3, cpu: 6, memoryBytes: 6 * GIB, diskBytes: 30 * GIB } : o.orgUsage, canSetQuota: o.orgCan ?? true,
      projects: [{ id: SHOP.id, slug: "shop", name: "Shop", quota: { cpu: 8 }, usage: { instances: 3, cpu: 6, memoryBytes: 6 * GIB, diskBytes: 30 * GIB }, canSetQuota: o.projectCan ?? true, environments: o.envs ?? [prod(), dev()] }],
    },
    { id: GLOBEX.id, slug: "globex", name: "Globex", quota: null, usage: { instances: 0, cpu: 0, memoryBytes: 0, diskBytes: 0 }, canSetQuota: o.orgCan ?? true, projects: [] },
  ],
});
const quotaView = (extra: Record<string, unknown> = {}) => ({
  kind: "organization", id: ACME.id, name: "Acme Corp", quota: { instances: 10, cpu: 16, memoryGib: 64, diskGib: 500 }, bound: {},
  usage: { instances: 3, cpu: 6, memoryBytes: 6 * GIB, diskBytes: 30 * GIB }, usageError: null, updatedBy: "user:x", updatedAt: NOW, canSet: true, ...extra,
});

const bodyOf = (calls: Call[], line: string) => calls.filter((c) => `${c.method} ${c.path}` === line).map((c) => c.body);
const serious = async (page: Page) => (await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze()).violations
  .filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(" ")}`);

test.beforeEach(async ({ page }) => {
  await page.route("**/api/**", (route) => route.abort());
  await page.emulateMedia({ reducedMotion: "reduce" });
});

test("shows each environment's Incus project, its state, limit use and restrictions, and quota use per scope", { tag: "@smoke" }, async ({ page }) => {
  await stub(page, { ...base(), "GET /api/v1/identity/tenancy": { json: overview(), delayMs: 300 } });
  await page.goto("/#/identity/tenancy");
  await expect(page.getByTestId("tenancy-loading")).toBeVisible();
  await expect(page.getByRole("heading", { level: 1, name: "Tenancy & quotas" })).toBeVisible();
  await expect(page.getByTestId("source-note")).toHaveText("Mock data · not a real cluster");
  const org = page.getByTestId("tenancy-organization").first();
  await expect(org.getByTestId("org-quota-use-instances")).toContainText("3 of 10 instances");
  await expect(org.getByTestId("org-quota-use-memory")).toContainText("6 GiB of 64 GiB memory");
  await expect(page.getByTestId("tenancy-organization").nth(1).getByTestId("org-quota-use-none")).toHaveText("No quota.");
  const [p, d] = [page.getByTestId("tenancy-environment").nth(0), page.getByTestId("tenancy-environment").nth(1)];
  await expect(p.getByTestId("env-state")).toHaveText("Created by Apelora");
  await expect(p.getByTestId("env-incus-project")).toHaveText("acme-shop-prod");
  await expect(p.getByTestId("env-use-cpu")).toContainText("6 of 8 CPUs");
  await expect(p.getByTestId("env-use-disk")).toContainText("30 GiB in use, no limit");
  await expect(p.getByTestId("env-prepare")).toHaveCount(0);
  const r = p.getByTestId("env-restrictions");
  await expect(r.locator("dl")).toBeHidden();
  await r.locator("summary").click();
  await expect(r.locator("dl")).toContainText("restricted.containers.nesting");
  await expect(d.getByTestId("env-state")).toHaveText("Not prepared");
  await expect(d.getByTestId("env-detail")).toHaveText("Incus: connection refused.");
  await expect(d.getByTestId("env-incus-error")).toContainText("does not exist yet");
  await expect(d.getByTestId("env-findings")).toContainText("restricted.devices.gpu is allow");
  await expect(d.getByTestId("env-quota-own-limits")).toContainText("Quota: 2 instances.");
  expect(await serious(page)).toEqual([]);
});

test("setting a quota sends whole numbers, empty for no limit, and reports what Incus did", async ({ page }) => {
  let tenancy = overview();
  const calls = await stub(page, {
    ...base(),
    "GET /api/v1/identity/tenancy": () => ({ json: tenancy }),
    [`GET /api/v1/identity/quotas/organization/${ACME.id}`]: { json: quotaView(), delayMs: 200 },
    [`PUT /api/v1/identity/quotas/organization/${ACME.id}`]: () => {
      tenancy = { ...tenancy };
      return { json: { quota: { instances: 12, cpu: 16, memoryGib: 64, diskGib: null }, environments: [
        { environmentId: PROD_ID, environment: "Production", incusProject: "acme-shop-prod", result: "applied", limits: {}, detail: "Incus enforces these limits" },
        { environmentId: DEV_ID, environment: "Development", incusProject: "acme-shop-dev", result: "failed", limits: {}, detail: "Incus: \"limits.instances\" is too low: current total is 3" },
      ] } };
    },
  });
  await page.goto("/#/identity/tenancy");
  await page.getByRole("button", { name: "Set quota of organization Acme Corp" }).click();
  const dialog = page.getByRole("dialog", { name: "Quota of Acme Corp" });
  await expect(dialog.getByTestId("quota-instances")).toHaveValue("10");
  await expect(dialog.getByTestId("quota-instances")).toBeFocused();
  await expect(dialog.getByTestId("quota-in-use")).toContainText("In use now: 3 instances, 6 CPUs, 6 GiB memory, 30 GiB disk.");
  await dialog.getByTestId("quota-instances").fill("-1");
  await expect(dialog.getByTestId("quota-instances")).toHaveAttribute("aria-invalid", "true");
  await expect(dialog.getByTestId("quota-save")).toBeDisabled();
  await dialog.getByTestId("quota-instances").fill("12");
  await dialog.getByTestId("quota-disk").fill("");
  await dialog.getByTestId("quota-save").click();
  await expect(dialog).toBeHidden();
  expect(bodyOf(calls, `PUT /api/v1/identity/quotas/organization/${ACME.id}`)).toEqual([{ instances: 12, cpu: 16, memoryGib: 64, diskGib: null }]);
  const notice = page.getByTestId("tenancy-notice");
  await expect(notice).toContainText("Quota of Acme Corp saved. Incus limits set on 1 environment.");
  await expect(notice).toContainText("Incus refused Development");
  await expect(notice).toContainText("Apelora still enforces the quota.");
});

test("the server's refusal stays in the dialog, and the bound above is shown", async ({ page }) => {
  await stub(page, {
    ...base(),
    "GET /api/v1/identity/tenancy": { json: overview() },
    [`GET /api/v1/identity/quotas/project/${SHOP.id}`]: { json: quotaView({ kind: "project", id: SHOP.id, name: "Shop", quota: { cpu: 8 }, bound: { instances: 10, cpu: 16 } }) },
    [`PUT /api/v1/identity/quotas/project/${SHOP.id}`]: error(400, "quota_above_parent", "a quota may not allow more than the quota above it: CPUs"),
  });
  await page.goto("/#/identity/tenancy");
  await page.getByRole("button", { name: "Set quota of project Shop" }).click();
  const dialog = page.getByRole("dialog", { name: "Quota of Shop" });
  await expect(dialog).toContainText("Limit above: 16");
  await expect(dialog).toContainText("Limit above: 10");
  await dialog.getByTestId("quota-cpu").fill("32");
  await dialog.getByTestId("quota-save").click();
  await expect(dialog.getByTestId("quota-error")).toHaveText("a quota may not allow more than the quota above it: CPUs");
  await expect(dialog).toBeVisible();
  expect(await serious(page)).toEqual([]);
});

test("preparing a pending environment's Incus project shows the outcome and reloads", async ({ page }) => {
  let envs = [prod(), dev()];
  const calls = await stub(page, {
    ...base(),
    "GET /api/v1/identity/tenancy": () => ({ json: overview({ envs }) }),
    [`POST /api/v1/identity/environments/${DEV_ID}/incus-project`]: () => {
      envs = [prod(), dev({ incusState: "created", incusDetail: "Incus project acme-shop-dev created by Apelora with the baseline restrictions", incusError: null, findings: [] })];
      return { json: { environmentId: DEV_ID, incusProject: "acme-shop-dev", state: "created", detail: "Incus project acme-shop-dev created by Apelora with the baseline restrictions", findings: [], limits: null }, delayMs: 300 };
    },
  });
  await page.goto("/#/identity/tenancy");
  const d = page.getByTestId("tenancy-environment").nth(1);
  await d.getByTestId("env-prepare").click();
  await expect(d.getByTestId("env-prepare")).toHaveText("Preparing…");
  await expect(page.getByTestId("tenancy-notice")).toHaveText("Incus project acme-shop-dev created by Apelora with the baseline restrictions.");
  await expect(d.getByTestId("env-state")).toHaveText("Created by Apelora");
  expect(calls.filter((c) => c.method === "POST").length).toBe(1);
});

test("a caller sees only the actions the server allows, and says so without an admin identity", async ({ page }) => {
  await stub(page, {
    ...base(whoami(["project.view", "project.update"], { platform: [] })),
    "GET /api/v1/identity/tenancy": { json: overview({ admin: false, orgCan: false, projectCan: false, orgUsage: null, envs: [prod({ canPrepare: false }), dev({ canPrepare: false })] }) },
  });
  await page.goto("/#/identity/tenancy");
  await expect(page.getByTestId("tenancy-no-admin")).toContainText("cannot create projects or set limits");
  await expect(page.getByTestId("org-quota")).toHaveCount(0);
  await expect(page.getByTestId("project-quota")).toHaveCount(0);
  await expect(page.getByTestId("env-quota")).toHaveCount(2);
  await expect(page.getByTestId("env-prepare")).toHaveCount(0);
  await expect(page.getByTestId("tenancy-organization").first().getByTestId("org-quota-use-limits")).toContainText("Use hidden: you see only part of this organization.");
  const nav = page.getByRole("navigation", { name: "Console" });
  await expect(nav.getByRole("link", { name: "Tenancy & quotas" })).toHaveAttribute("aria-current", "page");
});

test("a failed load says why and offers to try again", async ({ page }) => {
  let fail = true;
  await stub(page, {
    ...base(),
    "GET /api/v1/identity/tenancy": () => (fail ? { status: 503, json: { error: { code: "store_unavailable", message: "the database is not reachable" } } } : { json: overview() }),
  });
  await page.goto("/#/identity/tenancy");
  await expect(page.getByTestId("tenancy-error")).toContainText("the database is not reachable");
  fail = false;
  await page.getByRole("button", { name: "Try again" }).click();
  await expect(page.getByTestId("tenancy-organization")).toHaveCount(2);
});

test.describe("phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  test("no sideways scroll, the actions stay on screen", async ({ page }) => {
    await stub(page, { ...base(), "GET /api/v1/identity/tenancy": { json: overview() } });
    await page.goto("/#/identity/tenancy");
    await expect(page.getByTestId("tenancy-environment")).toHaveCount(2);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow).toBeLessThanOrEqual(0);
    for (const b of await page.getByTestId("env-quota").all()) {
      const box = await b.boundingBox();
      expect(box && box.x >= 0 && box.x + box.width <= 390).toBeTruthy();
      expect(box!.height).toBeGreaterThanOrEqual(44);
    }
    expect(await serious(page)).toEqual([]);
  });
});
