import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page, type Route } from "@playwright/test";

import { degraded, healthy, servers, troubledServers } from "./health-fixtures";

// The Health page against a stubbed control plane: states in words, reasons and next steps
// from the server, resource bars with their totals, honest loading, error and stale states.

const now = "2026-10-21T10:00:00Z";
const health = { status: "ok", service: "control-plane", version: "0.1.0", apiVersion: "v1", provider: "incus", auth: "tokens", incus: { connected: true, apiVersion: "1.0", serverVersion: "7.0.1", clustered: true } };
const admin = { name: "alex", role: "fabric-admin", permissions: ["read-inventory", "read-audit", "manage-tokens", "manage-infrastructure", "design-applications", "deploy"], deployProjects: ["apelora"] };
const designer = { ...admin, role: "studio-designer", permissions: ["design-applications"] };
const cluster = { source: { provider: "incus", observedAt: now }, server: { clustered: true, serverVersion: "7.0.1" }, members: [] };

type Reply = { status?: number; json?: unknown; delay?: number } | "abort";
async function stub(page: Page, routes: Record<string, Reply | (() => Reply)>) {
  await page.route("**/api/**", async (route: Route) => {
    const r0 = routes[new URL(route.request().url()).pathname];
    const r = typeof r0 === "function" ? r0() : r0;
    if (!r || r === "abort") return route.abort();
    // Real servers take a moment (LESSONS-LEARNED: stubs that answer at once hide races).
    await new Promise((res) => setTimeout(res, r.delay ?? 60));
    return route.fulfill({ status: r.status ?? 200, json: r.json });
  });
}

const base = (who: unknown = admin) => ({
  "/api/v1/health": { json: health },
  "/api/v1/whoami": { json: who },
  "/api/v1/cluster": { json: cluster },
});

test.beforeEach(async ({ page }) => {
  await page.route("**/api/**", (route) => route.abort());
  await page.emulateMedia({ reducedMotion: "reduce" });
});

test("healthy: every component in words, servers with used-of-total bars, instances on demand", { tag: "@smoke" }, async ({ page }) => {
  await stub(page, { ...base(), "/api/v1/health/components": { json: healthy() }, "/api/v1/health/servers": { json: servers() } });
  await page.goto("/#/health");
  const nav = page.getByRole("navigation", { name: "Console" });
  await expect(nav.getByRole("link", { name: "Health" })).toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Health");
  await expect(page.getByTestId("health-overall")).toHaveText("Healthy");
  await expect(page.getByTestId("health-age")).toContainText(/Checked \d+ s ago; reads again every 20 s/);

  const rows = page.getByTestId("health-components").locator(":scope > li");
  await expect(rows.locator("h3")).toHaveText(["Control plane", "Database", "Deployment workers", "Job queue", "Incus", "Networks", "Storage pools", "Identity providers"]);
  await expect(page.getByTestId("component-control-plane")).toContainText("Version 0.1.0, up 1 d 2 h, sign-in required, inventory from Incus");
  await expect(page.getByTestId("component-database")).toContainText("query 0.8 ms, schema 16 of 16");
  await expect(page.getByTestId("worker-row")).toContainText("worker-1 on cp-1, version 0.1.0");
  await expect(page.getByTestId("component-incus")).toContainText("Incus 7.0.1, clustered, 3 of 3 servers online");
  for (const row of await rows.all()) await expect(row).toHaveAttribute("data-state", "healthy");

  const cards = page.getByTestId("server-card");
  await expect(cards).toHaveCount(3);
  await expect(page.getByTestId("source-note")).toContainText("From Incus");
  const first = cards.first();
  await expect(first.getByTestId("server-memory")).toContainText("3.0 GiB of 8.0 GiB used");
  await expect(first.getByTestId("server-cpu")).toContainText("load 0.42 on 4 threads");
  await expect(first.getByTestId("server-pool")).toContainText("Storage pool local");
  await expect(first.getByRole("progressbar", { name: /Memory/ })).toHaveAttribute("max", String(8 * 1024 ** 3));
  await expect(cards.nth(2)).toContainText("No instances");

  await first.getByRole("button", { name: "Instances on node-1" }).click();
  const dialog = page.locator("dialog[open]");
  await expect(dialog.getByRole("heading")).toHaveText("Instances on node-1");
  await expect(dialog.getByTestId("server-instance-row")).toHaveCount(2);
  await expect(dialog.getByTestId("server-instance-row").nth(1)).toContainText("Virtual machine");
  await page.keyboard.press("Escape");
  await expect(page.locator("dialog[open]")).toHaveCount(0);
  await expect(page.locator(".btn-primary")).toHaveCount(0);
});

test("degraded: reasons and next steps come from the server; warnings per server; mock data is labelled", async ({ page }) => {
  await stub(page, { ...base(), "/api/v1/health/components": { json: degraded() }, "/api/v1/health/servers": { json: troubledServers() } });
  await page.goto("/#/health");
  await expect(page.getByTestId("health-overall")).toHaveText("Down");
  await expect(page.getByTestId("component-workers")).toHaveAttribute("data-state", "down");
  await expect(page.getByTestId("component-workers-reason")).toHaveText("No deployment worker is running: deployments, moves and recovery wait until one starts.");
  await expect(page.getByTestId("component-workers")).toContainText("Next step: Start the deployment worker");
  await expect(page.getByTestId("worker-row")).toContainText("lost, last heartbeat");
  await expect(page.getByTestId("component-job-queue")).toContainText("3 waiting, 1 waiting to retry, 0 running, 1 stalled, 2 failed in the last hour, oldest waiting 5 min");
  await expect(page.getByTestId("component-incus-reason")).toHaveText("1 of 3 servers not online: node-3 is Offline.");
  await expect(page.getByTestId("component-identity")).toContainText("Example Corp: not reachable");

  const cards = page.getByTestId("server-card");
  await expect(cards.nth(0)).toHaveAttribute("data-state", "degraded");
  await expect(cards.nth(0).getByTestId("server-warnings").locator("li")).toHaveText([
    "Memory 95 % used (7.6 GiB of 8.0 GiB)", "Storage pool local 90 % full (2.0 GiB free)", "CPU busy: load 5.1 on 4 threads",
  ]);
  await expect(cards.nth(1)).toContainText("held out of placement");
  await expect(cards.nth(2)).toContainText("Offline");
  await expect(cards.nth(2).getByTestId("server-resources-unavailable")).toHaveCount(0);
  await expect(cards.nth(2).getByTestId("server-warnings")).toContainText("Resources unavailable: GET /1.0/resources?target=node-3: cannot reach the member");
  await expect(cards.nth(2).getByRole("progressbar")).toHaveCount(0);
});

test("mock data says so", async ({ page }) => {
  await stub(page, { ...base(), "/api/v1/health/components": { json: healthy("mock") }, "/api/v1/health/servers": { json: servers("mock") } });
  await page.goto("/#/health");
  await expect(page.getByTestId("source-note")).toHaveText("Mock data · not a real cluster");
  await expect(page.getByTestId("component-incus")).toHaveAttribute("data-state", "not-set-up");
  await expect(page.getByTestId("component-incus-reason")).toHaveText("Mock data: no Incus server is configured.");
  await expect(page.getByTestId("component-control-plane")).toContainText("mock inventory");
});

test("loading, then an error with a retry; a later failure keeps the data and says how old it is", async ({ page }) => {
  let fail = true;
  await stub(page, {
    ...base(),
    "/api/v1/health/components": () => (fail ? { status: 503, json: { error: { code: "store_unavailable", message: "the database is not reachable; check DATABASE_URL" } }, delay: 400 } : { json: healthy() }),
    "/api/v1/health/servers": () => (fail ? { status: 502, json: { error: { code: "incus_unreachable", message: "cannot reach the Incus server: connection refused" } }, delay: 400 } : { json: servers() }),
  });
  await page.goto("/#/health");
  await expect(page.getByTestId("health-loading")).toHaveText("Checking each component…");
  await expect(page.getByTestId("health-servers-loading")).toBeVisible();
  await expect(page.getByTestId("health-error")).toHaveText("the database is not reachable; check DATABASE_URL");
  await expect(page.getByTestId("health-servers-error")).toHaveText("cannot reach the Incus server: connection refused");
  fail = false;
  await page.getByTestId("health-error").locator("..").getByRole("button", { name: "Try again" }).click();
  await page.getByTestId("health-servers-error").locator("..").getByRole("button", { name: "Try again" }).click();
  await expect(page.getByTestId("health-overall")).toHaveText("Healthy");
  await expect(page.getByTestId("server-card")).toHaveCount(3);

  fail = true;
  await page.getByTestId("health-refresh").click();
  const stale = page.getByTestId("health-stale");
  await expect(stale).toContainText("Could not read the latest state");
  await expect(stale).toContainText("The database is not reachable; check DATABASE_URL. Showing what was read");
  // The last data stays on screen.
  await expect(page.getByTestId("health-overall")).toHaveText("Healthy");
  await expect(page.getByTestId("server-card")).toHaveCount(3);
});

test("reads again on its own while visible", async ({ page }) => {
  await page.clock.install();
  let reads = 0;
  await stub(page, { ...base(), "/api/v1/health/components": () => { reads += 1; return { json: healthy(), delay: 0 }; }, "/api/v1/health/servers": { json: servers(), delay: 0 } });
  await page.goto("/#/health");
  await expect(page.getByTestId("health-overall")).toHaveText("Healthy");
  const before = reads;
  await page.clock.runFor(20_500);
  await expect.poll(() => reads).toBeGreaterThan(before);
});

test("not offered without read access", async ({ page }) => {
  await stub(page, base(designer));
  await page.goto("/");
  await expect(page.getByTestId("whoami")).toContainText("studio-designer");
  await expect(page.getByRole("navigation", { name: "Console" }).getByRole("link", { name: "Health" })).toHaveCount(0);
});

test("an empty cluster says so", async ({ page }) => {
  await stub(page, { ...base(), "/api/v1/health/components": { json: healthy() }, "/api/v1/health/servers": { json: servers("incus", []) } });
  await page.goto("/#/health");
  await expect(page.getByTestId("health-servers")).toContainText("Incus reports no servers.");
});

test.describe("phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  test("no sideways scrolling, touch-sized controls, no serious accessibility problems", async ({ page }) => {
    await stub(page, { ...base(), "/api/v1/health/components": { json: degraded() }, "/api/v1/health/servers": { json: troubledServers() } });
    await page.goto("/#/health");
    await expect(page.getByTestId("server-card")).toHaveCount(3);
    const overflow = await page.evaluate(() => [document.documentElement.scrollWidth, document.documentElement.clientWidth, (document.querySelector("main") as HTMLElement).scrollWidth, (document.querySelector("main") as HTMLElement).clientWidth]);
    expect(overflow[0]).toBeLessThanOrEqual(overflow[1]!);
    expect(overflow[2]).toBeLessThanOrEqual(overflow[3]!);
    const box = await page.getByTestId("health-refresh").boundingBox();
    expect(box!.height).toBeGreaterThanOrEqual(44);
    const r = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
    expect(r.violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(" ")}`)).toEqual([]);
  });
});

test("desktop: no serious accessibility problems, healthy and degraded", async ({ page }) => {
  await stub(page, { ...base(), "/api/v1/health/components": { json: degraded() }, "/api/v1/health/servers": { json: troubledServers() } });
  await page.goto("/#/health");
  await expect(page.getByTestId("server-card")).toHaveCount(3);
  const r = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  expect(r.violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(" ")}`)).toEqual([]);
});
