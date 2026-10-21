import { expect, test, type Page, type Request, type Route } from "@playwright/test";
import { open } from "./helpers";
import { auditItem, base as identityBase, items, stub as identityStub, whoami } from "./identity-fixtures";

// Run with: SCREENSHOTS=1 npx playwright test tests/e2e/screenshots-console-docs.spec.ts
// Console pages for the documentation, with a stubbed control plane and made-up names and numbers.
test.skip(!process.env.SCREENSHOTS, "screenshots are generated on demand");

const CONSOLE = process.env.APELORA_CONSOLE_URL ?? "http://127.0.0.1:5174";
const NOW = "2026-10-21T10:00:00Z";
const LATER = "2026-10-21T10:00:42Z";
const health = { status: "ok", service: "control-plane", version: "0.1.0", apiVersion: "v1", provider: "incus", auth: "tokens", incus: { connected: true }, checkedAt: NOW };
const admin = { name: "alex", role: "fabric-admin", permissions: ["read-inventory", "read-audit", "manage-tokens", "manage-infrastructure", "design-applications", "deploy"], capabilities: [], platformPermissions: [], roles: [], deployProjects: ["apelora"] };
const src = { provider: "incus", observedAt: NOW };
const yes = { allowed: true, reason: null };
const no = (reason: string) => ({ allowed: false, reason });

type Reply = { status?: number; json?: unknown };
type Handler = Reply | ((req: Request) => Reply);
async function stub(page: Page, routes: Record<string, Handler>) {
  await page.route("**/api/**", async (route: Route) => {
    const req = route.request();
    const h = routes[`${req.method()} ${new URL(req.url()).pathname}`];
    if (!h) return route.fulfill({ status: 404, json: { error: { code: "not_found", message: "no stub" } } });
    const r = typeof h === "function" ? h(req) : h;
    return route.fulfill({ status: r.status ?? 200, json: r.json });
  });
}
const common = { "GET /api/v1/health": { json: health }, "GET /api/v1/whoami": { json: admin }, "GET /api/v1/cluster": { json: { source: src, server: { clustered: true }, members: [] } } };
const settled = (page: Page) => page.evaluate(() => document.fonts.ready.then(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))));
const fullHeight = async (page: Page, w: number, h: number) => {
  const height = await page.evaluate(() => document.querySelector("main")!.scrollHeight + 48);
  await page.setViewportSize({ width: w, height: Math.max(h, height) });
};

test.beforeEach(async ({ page }) => {
  await page.route("**/api/**", (route) => route.abort());
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.addInitScript(() => sessionStorage.setItem("apelora.apiToken", "apl_test"));
});

const job = (n: number, kind: string, status: string, project: string, name: string, extra: Record<string, unknown> = {}) => ({
  id: `00000000-0000-4000-8000-0000000000${String(n).padStart(2, "0")}`, kind, status, attempts: status === "failed" ? 3 : 1, maxAttempts: 5, lastError: null, incusOperation: null, incusOperations: [], requestId: null,
  request: { project, name }, createdAt: NOW, updatedAt: LATER, queuedBy: "user:alice", retryOf: null, retriedBy: null, cancelRequestedAt: null, cancelRequestedBy: null,
  actions: { retry: status === "failed" ? yes : no("Only failed jobs can be retried."), cancel: status === "pending" || status === "running" ? yes : no("Only waiting or running jobs can be cancelled.") }, ...extra,
});

test("console operations screenshot 1440x900", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await stub(page, { ...common, "GET /api/v1/jobs": { json: { items: [
    job(6, "instance.stop", "running", "shop", "shop-web-02"),
    job(5, "volume.backup", "succeeded", "shop", "shop-db-data"),
    job(4, "instance.create", "failed", "shop", "shop-web-03", { lastError: "Not enough memory on any server in the cluster group" }),
    job(3, "instance.start", "succeeded", "blog", "blog-web-01"),
    job(2, "instance.create", "succeeded", "blog", "blog-web-01"),
    job(1, "instance.create", "succeeded", "shop", "shop-web-01"),
  ] } } });
  await page.goto(`${CONSOLE}/#/operations`);
  await expect(page.getByTestId("job-row")).toHaveCount(6);
  await fullHeight(page, 1440, 900);
  await settled(page);
  await page.screenshot({ path: "docs/screenshots/35-console-operations-1440x900.png" });
});

test("console applications stop dialog screenshot 1440x900", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const app = (name: string, actions: Record<string, unknown>) => ({ name, createdAt: NOW, latestRevision: 2, actions });
  const status = (state: string, inst: string) => ({ source: src, items: [{ project: "apelora", instance: inst, module: "web", status: state, location: "node-1", owner: true, health: null }], services: [] });
  await stub(page, { ...common,
    "GET /api/v1/applications": { json: { items: [
      app("shop", { start: no("it is already running"), stop: yes, restart: yes }),
      app("blog", { start: yes, stop: no("nothing is running"), restart: no("nothing is running") }),
    ] } },
    "GET /api/v1/applications/shop/status": { json: status("Running", "shop-web-01") },
    "GET /api/v1/applications/blog/status": { json: status("Stopped", "blog-web-01") },
  });
  await page.goto(`${CONSOLE}/#/applications`);
  await expect(page.getByTestId("application-row")).toHaveCount(2);
  await page.getByTestId("application-row").first().getByTestId("application-stop").click();
  await expect(page.locator("dialog[open]")).toContainText("Stop shop?");
  await settled(page);
  await page.screenshot({ path: "docs/screenshots/36-console-application-stop-1440x900.png" });
});

test("console audit screenshot 1440x900", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const reader = { ...whoami(["audit.read"], { platform: ["audit.read"] }), permissions: ["read-audit"] };
  await identityStub(page, { ...identityBase(reader), "GET /api/v1/audit": items([
    auditItem(14, "application.stop", "success", { actorName: "alex", target: "shop" }),
    auditItem(13, "role.assign", "denied", { actorName: "ada", detail: { reason: "you cannot grant more than you hold", requestId: "req-1" } }),
    auditItem(12, "bmc.power_off", "success", { actorName: null, target: "node-3" }),
    auditItem(11, "group.create", "success", { actorName: "ada", target: "Operators" }),
    auditItem(10, "auth.oidc.signed_in", "success", { actorName: "sam" }),
  ]) });
  await page.goto(`${CONSOLE}/#/audit`);
  await expect(page.getByTestId("audit-row")).toHaveCount(5);
  await fullHeight(page, 1440, 900);
  await settled(page);
  await page.screenshot({ path: "docs/screenshots/37-console-audit-1440x900.png" });
});

test("console MAAS machines screenshot 1440x900", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  const m = (over: Record<string, unknown>) => ({
    systemId: "abc123", hostname: "rack-a-1.maas", status: "Deployed", powerState: "on", powerType: "redfish", architecture: "amd64/generic", cpuCount: 8, memoryMib: 16384, os: "ubuntu noble",
    addresses: ["10.0.0.11"], hardwareUuid: "4c4c4544-0042-4c10-8051-b7c04f435331", zone: "rack-a", pool: "default", registeredAs: null, memberStatus: null, ...over,
  });
  const empty = { json: { source: src, items: [] } };
  await stub(page, { ...common,
    "GET /api/v1/cluster/members": empty, "GET /api/v1/cluster/groups": empty, "GET /api/v1/cluster/recovery": empty, "GET /api/v1/cluster/join-tokens": empty, "GET /api/v1/machines": empty,
    "GET /api/v1/provisioners/maas/machines": { json: { source: src, configured: true, items: [
      m({ registeredAs: "rack-a-1", memberStatus: "Online" }),
      m({ systemId: "def456", hostname: "rack-a-2.maas", status: "Ready", powerState: "off", hardwareUuid: "4c4c4544-0042-4c10-8051-b7c04f435332", addresses: ["10.0.0.12"] }),
      m({ systemId: "ghi789", hostname: "rack-b-1.maas", status: "Failed commissioning", powerState: "error", zone: "rack-b", addresses: [] }),
    ] } },
  });
  await page.goto(`${CONSOLE}/#/servers/machines`);
  await expect(page.getByText("rack-a-2.maas").first()).toBeVisible();
  await fullHeight(page, 1440, 900);
  await settled(page);
  await page.screenshot({ path: "docs/screenshots/38-console-machines-1440x900.png" });
});

test("studio deploy screenshots 1440x900", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.route("**/api/**", (route) => route.abort());
  let done = false;
  const row = (module: string, action: string, reason = "not deployed yet") => ({ module, instance: `commerce-stack-${module}`, action, reason });
  const j = (id: string) => ({ id, kind: "instance.create", status: "succeeded", attempts: 1, maxAttempts: 5, lastError: null, incusOperation: null, request: {}, createdAt: NOW, updatedAt: NOW });
  await stub(page, {
    "GET /api/v1/health": { json: health }, "GET /api/v1/whoami": { json: { name: "alex", role: "operator", permissions: ["read-inventory", "design-applications", "deploy"], deployProjects: ["apelora"] } },
    "POST /api/v1/applications/commerce-stack/plan": { json: { items: [row("nginx-01", "create"), row("nginx-02", "create")], warnings: [] } },
    "POST /api/v1/applications/commerce-stack/deployments": () => ({ status: 202, json: { application: "commerce-stack", revision: 1, plan: { items: [row("nginx-01", "create"), row("nginx-02", "create")], warnings: [] },
      jobs: [{ module: "nginx-01", instance: "commerce-stack-nginx-01", action: "create", jobId: "j1" }, { module: "nginx-02", instance: "commerce-stack-nginx-02", action: "create", jobId: "j2" }] } }),
    "GET /api/v1/jobs/j1": () => { done = true; return { json: j("j1") }; },
    "GET /api/v1/jobs/j2": { json: j("j2") },
    "GET /api/v1/applications/commerce-stack/status": () => ({ json: { source: src, items: done ? [
      { module: "nginx-01", instance: "commerce-stack-nginx-01", project: "apelora", status: "Running", location: "node-1" },
      { module: "nginx-02", instance: "commerce-stack-nginx-02", project: "apelora", status: "Running", location: "node-2" },
    ] : [], services: [] } }),
  });
  await open(page);
  await page.getByTestId("deploy-open").click();
  const dialog = page.getByTestId("deploy-dialog");
  await expect(dialog.getByTestId("deploy-deployable")).toHaveCount(2);
  await settled(page);
  await page.screenshot({ path: "docs/screenshots/39-studio-deploy-dialog-1440x900.png" });
  await dialog.getByTestId("deploy-preview").click();
  await expect(dialog.getByTestId("plan-row")).toHaveCount(2);
  await settled(page);
  await page.screenshot({ path: "docs/screenshots/40-studio-deploy-plan-1440x900.png" });
  await dialog.getByTestId("deploy-apply").click();
  await expect(dialog.getByTestId("deploy-job")).toHaveCount(2);
  await dialog.getByTestId("deploy-done").click();
  await expect(page.locator("[data-node-id=nginx-01] [data-testid=live-status]")).toHaveAttribute("data-state", /Running|Healthy|Up/i, { timeout: 10_000 }).catch(() => undefined);
  await page.mouse.move(700, 8);
  await page.waitForTimeout(600);
  await settled(page);
  await page.screenshot({ path: "docs/screenshots/41-studio-live-status-1440x900.png" });
});

test("studio applications list screenshots 1440x900", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.addInitScript(() => { localStorage.setItem("apelora.studio.tour.v1", "1"); sessionStorage.setItem("apelora.apiToken", "apl_test_token_for_stub"); });
  const proj = { id: "p1", name: "Shop", organizationId: "o1", organizationSlug: "acme", slug: "shop" };
  const env = { id: "e1", name: "production", slug: "production", kind: "production", incusProject: "shop-prod" };
  const a = (allowed: boolean, reason: string | null = null) => ({ allowed, reason });
  const app = (name: string, status: string, actions: Record<string, { allowed: boolean; reason: string | null }>, extra: Record<string, unknown> = {}) => ({
    name, description: `${name} for customers`, icon: "nginx", archived: false, createdAt: NOW, updatedAt: NOW, latestRevision: 2, project: proj, environment: env, actions,
    design: { status: "published", draftVersion: 2, publishedVersion: 2, hasUnpublishedChanges: false, validationErrors: 0, updatedAt: NOW },
    deployment: { status, modules: 4, replicas: { desired: 4, healthy: status === "running" ? 4 : 0, running: status === "running" ? 4 : 0, unhealthy: 0 }, observedAt: NOW, stale: false, source: "incus", lastActivityAt: NOW, deployedVersion: 2, operation: null }, ...extra,
  });
  const running = { start: a(false, "already running"), restart: a(false, "running normally"), shutdown: a(true), stop: a(true), migrate: a(true) };
  const stopped = { start: a(true), restart: a(false, "nothing is running"), shutdown: a(false, "nothing is running"), stop: a(false, "nothing is running"), migrate: a(true) };
  await stub(page, { ...common,
    "GET /api/v1/whoami": { json: { name: "alex", role: "operator", permissions: ["read-inventory", "design-applications", "deploy"], deployProjects: ["shop-prod"], capabilities: [], platformPermissions: [] } },
    "GET /api/v1/applications": { json: { items: [app("shop", "running", running), app("blog", "stopped", stopped)], observedAt: NOW, stale: false } },
  });
  await page.goto("http://127.0.0.1:5173/studio/applications");
  await expect(page.getByTestId("app-card")).toHaveCount(2);
  await settled(page);
  await page.screenshot({ path: "docs/screenshots/42-studio-applications-1440x900.png" });
  await page.getByTestId("view-list").click();
  await expect(page.getByTestId("app-row")).toHaveCount(2);
  await settled(page);
  await page.screenshot({ path: "docs/screenshots/43-studio-applications-list-1440x900.png" });
});
