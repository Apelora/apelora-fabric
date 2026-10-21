import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page, type Request, type Route } from "@playwright/test";

// Start, restart and stop of an application from the Applications page, against a stubbed control
// plane: only what the server allows is offered, a stop asks first and again when the server says
// clients lose service, the row follows the operation, and an older server without `actions`
// still shows the page.

const now = "2026-10-21T10:00:00Z";
const health = { status: "ok", service: "control-plane", version: "0.1.0", apiVersion: "v1", provider: "mock", auth: "tokens", incus: { connected: false } };
const operator = { name: "alex", role: "operator", permissions: ["read-inventory", "design-applications", "deploy"], deployProjects: ["apelora"] };
const cluster = { source: { provider: "mock", observedAt: now }, server: { clustered: true, serverVersion: null }, members: [] };
const yes = { allowed: true, reason: null };
const no = (reason: string) => ({ allowed: false, reason });
const OP = "44444444-4444-4444-8444-444444444444";

const app = (name: string, actions?: Record<string, unknown>) => ({ name, createdAt: now, latestRevision: 2, ...(actions ? { actions } : {}) });
const status = (state: string) => ({ source: { provider: "incus", observedAt: now }, items: [
  { project: "apelora", instance: "shop-web-01", module: "web", status: state, location: "node-1", owner: true, health: null },
], services: [] });
const operation = (state: string, done: number, extra: Record<string, unknown> = {}) => ({ id: OP, kind: "stop", state, phases: [], stepsDone: done, stepsTotal: 2, createdAt: now, requestedBy: "user:alex", error: null, ...extra });

type Reply = { status?: number; json?: unknown };
type Handler = Reply | ((req: Request) => Reply);
async function stub(page: Page, routes: Record<string, Handler>) {
  await page.route("**/api/v1/**", async (route: Route) => {
    const req = route.request();
    const h = routes[`${req.method()} ${new URL(req.url()).pathname}`];
    if (!h) return route.fulfill({ status: 404, json: { error: { code: "not_found", message: "no stub" } } });
    const r = typeof h === "function" ? h(req) : h;
    await new Promise((res) => setTimeout(res, 40));
    return route.fulfill({ status: r.status ?? 200, json: r.json });
  });
}
const base = { "GET /api/v1/health": { json: health }, "GET /api/v1/whoami": { json: operator }, "GET /api/v1/cluster": { json: cluster } };
const serious = async (page: Page) => (await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze()).violations
  .filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(" ")}`);

test.beforeEach(async ({ page }) => {
  await page.route("**/api/**", (route) => route.abort());
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.addInitScript(() => sessionStorage.setItem("apelora.apiToken", "apl_test"));
});

test("a stop asks first, asks again when the server says clients lose service, and the row follows the operation", { tag: "@smoke" }, async ({ page }) => {
  const sent: unknown[] = [];
  let polls = 0;
  let listed = 0;
  await stub(page, {
    ...base,
    "GET /api/v1/applications": () => { listed++; return { json: { items: [app("shop", { start: no("it is already running"), stop: yes, restart: yes }), app("blog", { start: yes, stop: no("nothing is running"), restart: no("nothing is running") })] } }; },
    "GET /api/v1/applications/shop/status": () => ({ json: status("Running") }),
    "GET /api/v1/applications/blog/status": () => ({ json: status("Stopped") }),
    "POST /api/v1/applications/shop/operations": (req) => {
      const body = req.postDataJSON() as { action: string; confirmDisruption?: boolean };
      sent.push(body);
      if (!body.confirmDisruption) return { status: 409, json: { error: { code: "confirmation_required", message: "stop will interrupt service for clients of db (stateful). Send confirmDisruption to proceed; data is kept." } } };
      return { status: 202, json: { created: true, operation: operation("running", 0) } };
    },
    "GET /api/v1/applications/shop/operations/44444444-4444-4444-8444-444444444444": () => { polls++; return { json: polls < 2 ? operation("running", 1) : operation("succeeded", 2, { finishedAt: now }) }; },
  });
  await page.goto("/#/applications");
  const rows = page.getByTestId("application-row");
  await expect(rows).toHaveCount(2);
  // Only what the server allows is offered, per application.
  await expect(rows.nth(0).getByTestId("application-stop")).toBeVisible();
  await expect(rows.nth(0).getByTestId("application-restart")).toBeVisible();
  await expect(rows.nth(0).getByTestId("application-start")).toHaveCount(0);
  await expect(rows.nth(1).getByTestId("application-start")).toBeVisible();
  await expect(rows.nth(1).getByTestId("application-stop")).toHaveCount(0);

  await rows.nth(0).getByTestId("application-stop").click();
  const dialog = page.locator("dialog[open]");
  await expect(dialog).toContainText("Stop shop?");
  await expect(dialog).toContainText("dependency order");
  await expect(dialog.getByRole("button", { name: "Keep it as it is" })).toBeFocused();
  await dialog.getByRole("button", { name: "Stop", exact: true }).click();
  // The server says a stateful module's clients lose service: the same dialog says so and asks again.
  await expect(dialog).toContainText("stateful");
  await expect(dialog.getByRole("button", { name: "Stop and interrupt service" })).toBeVisible();
  expect(sent).toEqual([{ action: "stop", confirmDisruption: false }]);
  await dialog.getByRole("button", { name: "Stop and interrupt service" }).click();
  await expect(page.locator("dialog[open]")).toHaveCount(0);
  expect(sent).toEqual([{ action: "stop", confirmDisruption: false }, { action: "stop", confirmDisruption: true }]);
  const note = rows.nth(0).getByTestId("application-operation");
  await expect(note).toContainText("Stopping");
  await expect(note).toContainText("Stopped.");
  await expect(rows.nth(0).getByTestId("application-stop")).toBeEnabled();
  expect(listed).toBeGreaterThan(1);
  expect(await serious(page)).toEqual([]);
});

test("a start goes at once and a refusal is shown on its row; a viewer is told why nothing is offered", async ({ page }) => {
  await stub(page, {
    ...base,
    "GET /api/v1/applications": () => ({ json: { items: [app("blog", { start: yes, stop: no("nothing is running"), restart: no("nothing is running") }), app("docs", { start: no("you may not start or stop this application"), stop: no("you may not start or stop this application"), restart: no("you may not start or stop this application") })] } }),
    "GET /api/v1/applications/blog/status": () => ({ json: status("Stopped") }),
    "GET /api/v1/applications/docs/status": () => ({ json: status("Running") }),
    "POST /api/v1/applications/blog/operations": () => ({ status: 409, json: { error: { code: "action_not_available", message: "start is not available now: an operation is in progress" } } }),
  });
  await page.goto("/#/applications");
  const rows = page.getByTestId("application-row");
  await expect(rows).toHaveCount(2);
  await rows.nth(0).getByTestId("application-start").click();
  await expect(page.locator("dialog[open]")).toHaveCount(0);
  await expect(rows.nth(0).getByTestId("application-action-error")).toContainText("an operation is in progress");
  await expect(rows.nth(1).getByTestId("application-actions-why")).toContainText("you may not start or stop");
  await expect(rows.nth(1).locator("button[data-testid^=application-s], button[data-testid=application-restart]")).toHaveCount(0);
});

test("a current control plane offers Shut down (graceful) and Stop (immediate), each asking first and sending its own action", async ({ page }) => {
  const sent: unknown[] = [];
  const all = (over: Record<string, unknown> = {}) => ({ start: no("it is already running"), restart: yes, shutdown: yes, stop: yes, migrate: yes, ...over });
  await stub(page, {
    ...base,
    "GET /api/v1/applications": () => ({ json: { items: [app("shop", all())] } }),
    "GET /api/v1/applications/shop/status": () => ({ json: status("Running") }),
    "POST /api/v1/applications/shop/operations": (req) => { sent.push(req.postDataJSON()); return { status: 202, json: { created: true, operation: operation("running", 0) } }; },
    "GET /api/v1/applications/shop/operations/44444444-4444-4444-8444-444444444444": () => ({ json: operation("succeeded", 2, { finishedAt: now, kind: (sent.at(-1) as { action: string }).action }) }),
  });
  await page.goto("/#/applications");
  const row = page.getByTestId("application-row").first();
  await expect(row.getByTestId("application-shutdown")).toHaveText("Shut down…");
  await expect(row.getByTestId("application-stop")).toHaveText("Stop…");
  await row.getByTestId("application-shutdown").click();
  const dialog = page.locator("dialog[open]");
  await expect(dialog).toContainText("Shut down shop?");
  await expect(dialog).toContainText("asked to shut down");
  await dialog.getByRole("button", { name: "Shut down", exact: true }).click();
  await expect(page.locator("dialog[open]")).toHaveCount(0);
  await expect(row.getByTestId("application-operation")).toContainText("Shut down.");
  await row.getByTestId("application-stop").click();
  await expect(dialog).toContainText("Stop now shop?");
  await expect(dialog).toContainText("powered off at once");
  await dialog.getByRole("button", { name: "Stop", exact: true }).click();
  await expect(page.locator("dialog[open]")).toHaveCount(0);
  await expect(row.getByTestId("application-operation")).toContainText("Stopped.");
  expect(sent).toEqual([{ action: "shutdown", confirmDisruption: false }, { action: "stop", confirmDisruption: false }]);
  expect(await serious(page)).toEqual([]);
});

test("an older control plane without actions still shows the applications", async ({ page }) => {
  await stub(page, {
    ...base,
    "GET /api/v1/applications": () => ({ json: { items: [app("shop")] } }),
    "GET /api/v1/applications/shop/status": () => ({ json: status("Running") }),
  });
  await page.goto("/#/applications");
  await expect(page.getByTestId("application-row")).toHaveCount(1);
  await expect(page.getByTestId("application-summary")).toContainText("All running");
  await expect(page.getByTestId("application-stop")).toHaveCount(0);
});
