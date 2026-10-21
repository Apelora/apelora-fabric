import AxeBuilder from "@axe-core/playwright";
import { test, type Page, type Route } from "@playwright/test";

import { capacity } from "./health-fixtures";
import { expect, open } from "./helpers";

// The Studio's capacity chip: what the design asks for next to what the cluster has free,
// only when connected to a control plane; otherwise it says capacity is unknown here.

const health = { status: "ok", service: "control-plane", version: "0.1.0", apiVersion: "v1", provider: "incus", auth: "tokens", incus: { connected: true, apiVersion: "1.0", serverVersion: "7.0.1", clustered: true } };
const designer = { name: "dana", role: null, kind: "user", permissions: ["design-applications"], capabilities: ["application.design", "capacity.view"], platformPermissions: [], roles: [], deployProjects: ["apelora"] };
const GIB = 1024 ** 3;

type Reply = { status?: number; json?: unknown };
async function stub(page: Page, routes: Record<string, Reply | (() => Reply)>, seen: string[] = []) {
  await page.route("**/api/**", async (route: Route) => {
    const path = new URL(route.request().url()).pathname;
    seen.push(path);
    const r0 = routes[path];
    const r = typeof r0 === "function" ? r0() : r0;
    if (!r) return route.abort();
    await new Promise((res) => setTimeout(res, 60));
    return route.fulfill({ status: r.status ?? 200, json: r.json });
  });
}
const signedIn = (page: Page) => page.addInitScript(() => sessionStorage.setItem("apelora.apiToken", "apl_test_token_for_stub"));
const chip = (page: Page) => page.getByTestId("capacity-chip");
const panel = (page: Page) => page.getByTestId("capacity-panel");

test("not connected: capacity is unknown in this browser and nothing is asked of the cluster", async ({ page }) => {
  const seen: string[] = [];
  await stub(page, {}, seen);
  await open(page);
  await expect(chip(page)).toHaveText("Capacity: unknown");
  await chip(page).click();
  await expect(panel(page).getByTestId("capacity-local")).toHaveText("Capacity unknown: this browser is not connected to a control plane. Connect to compare the design with the cluster.");
  expect(seen).not.toContain("/api/v1/capacity");
  await page.keyboard.press("Escape");
  await expect(panel(page)).toHaveCount(0);
  await expect(chip(page)).toBeFocused();
});

test("connected: the design fits on one server; servers are numbered, numbers come from the API", { tag: "@smoke" }, async ({ page }) => {
  await signedIn(page);
  await stub(page, { "/api/v1/health": { json: health }, "/api/v1/whoami": { json: designer }, "/api/v1/capacity": { json: capacity() } });
  await open(page);
  await chip(page).click();
  await expect(chip(page)).toHaveText("Capacity: fits on one server");
  await expect(chip(page)).toHaveAttribute("aria-expanded", "true");
  const p = panel(page);
  await expect(p.getByTestId("capacity-verdict")).toHaveText("The design fits on one server.");
  await expect(p).toContainText("From Incus");
  await expect(p.getByRole("row", { name: /Memory/ })).toContainText("13 GiB of 16 GiB");
  await expect(p.getByRole("row", { name: /vCPU/ })).toContainText("8 threads");
  await expect(p.getByTestId("capacity-servers").locator("li")).toHaveText([/Server 1\s*6 GiB free, 4 threads/, /Server 2\s*7 GiB free, 4 threads/, /Server 3\s*It is Offline/]);
  await expect(p).toContainText("root disks. Nothing is reserved until you deploy.");
  const r = await new AxeBuilder({ page }).include("[data-testid=capacity-panel]").include("[data-testid=capacity-chip]").withTags(["wcag2a", "wcag2aa"]).analyze();
  expect(r.violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id)).toEqual([]);
});

test("connected: too little free memory does not fit and says by how much", async ({ page }) => {
  await signedIn(page);
  const tiny = capacity({
    servers: [{ label: "Server 1", acceptsModules: true, reason: null, architecture: "aarch64", cpuThreads: 2, load: 0.1, memory: { usedBytes: 0.9 * GIB, totalBytes: 1 * GIB }, volumePool: null }],
    totals: { serversAccepting: 1, cpuThreads: 2, memoryFreeBytes: 0.1 * GIB, memoryTotalBytes: 1 * GIB, volumePoolFreeBytes: null },
    volumePool: null,
  });
  await stub(page, { "/api/v1/health": { json: health }, "/api/v1/whoami": { json: designer }, "/api/v1/capacity": { json: tiny } });
  await open(page);
  await chip(page).click();
  await expect(chip(page)).toHaveText("Capacity: does not fit");
  await expect(panel(page)).toContainText(/Not enough free memory: the design asks for .+, 102 MiB is free\./);
});

test("mock data is labelled; a refused permission is explained", async ({ page }) => {
  await signedIn(page);
  let refuse = false;
  await stub(page, {
    "/api/v1/health": { json: health },
    "/api/v1/whoami": { json: designer },
    "/api/v1/capacity": () => (refuse ? { status: 403, json: { error: { code: "forbidden", message: "you may not see the cluster's capacity (capacity.view)" } } } : { json: capacity({ source: { provider: "mock", observedAt: "2026-10-21T10:00:00Z" } }) }),
  });
  await open(page);
  await chip(page).click();
  await expect(chip(page)).toHaveText("Mock · Capacity: fits on one server");
  await expect(panel(page)).toContainText("Mock data");
  await chip(page).click();
  refuse = true;
  await chip(page).click();
  await expect(panel(page).getByTestId("capacity-error")).toHaveText("Your role does not include seeing the cluster's capacity.");
  await expect(chip(page)).toHaveText("Capacity: unknown");
});

test.describe("narrow screen", () => {
  test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  test("the panel stays inside the screen", async ({ page }) => {
    await signedIn(page);
    await stub(page, { "/api/v1/health": { json: health }, "/api/v1/whoami": { json: designer }, "/api/v1/capacity": { json: capacity() } });
    // The canvas itself is not built for phones; the header and this panel are.
    await page.goto("/");
    await chip(page).click();
    await expect(panel(page).getByTestId("capacity-verdict")).toBeVisible();
    const box = (await panel(page).boundingBox())!;
    expect(box.x).toBeGreaterThanOrEqual(0);
    expect(box.x + box.width).toBeLessThanOrEqual(390);
  });
});
