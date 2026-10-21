import AxeBuilder from "@axe-core/playwright";
import { readFileSync } from "node:fs";

import { expect, test, type Page, type Route } from "@playwright/test";

// Application Management in the Studio. Two backends are exercised: the browser-only one (no control
// plane reachable; runtime state is unknown and nothing can be started) and the control-plane one,
// against a stubbed API that answers the way the real one is documented to.

const seed = async (page: Page) => page.addInitScript(() => {
  if (!sessionStorage.getItem("__seeded")) { localStorage.clear(); localStorage.setItem("apelora.studio.tour.v1", "1"); sessionStorage.setItem("__seeded", "1"); }
});

async function offline(page: Page) {
  await seed(page);
  await page.route("**/api/**", (r) => r.abort());
}

const create = async (page: Page, name: string, opts: { icon?: string; from?: "template" } = {}) => {
  await page.getByTestId("new-application").click();
  await page.getByTestId("new-app-name").fill(name);
  if (opts.icon) await page.getByTestId(`new-icon-${opts.icon}`).click();
  if (opts.from === "template") await page.getByTestId("from-template").check();
  await page.getByTestId("new-app-create").click();
  await page.waitForURL(`**/studio/applications/${name}/designer`);
  await page.waitForSelector("[data-testid=canvas]");
};

test.describe("applications in this browser (no control plane)", () => {
  test.beforeEach(async ({ page }) => { await offline(page); });

  test("the existing commerce stack is migrated into an application with its nodes, connections and layout", { tag: "@smoke" }, async ({ page }) => {
    await page.goto("/studio/applications");
    const card = page.locator("[data-testid=app-card][data-app=commerce-stack]");
    await expect(card).toBeVisible();
    await expect(card.getByTestId("app-modules")).not.toHaveText("0");
    await card.getByTestId("app-open").click();
    await page.getByTestId("tab-designer").click();
    await page.waitForSelector("[data-testid=module-node]");
    const nodes = await page.getByTestId("module-node").count();
    expect(nodes).toBeGreaterThanOrEqual(5);
    expect(await page.locator(".react-flow__edge").count()).toBeGreaterThanOrEqual(4);
  });

  test("the Assembly Library at / is still the editor", async ({ page }) => {
    await page.goto("/");
    await page.waitForSelector("[data-testid=module-node]");
    await expect(page.getByTestId("deploy-open")).toHaveCount(0);
    await expect(page.getByTestId("library-exit")).toBeVisible();
    await expect(page.getByTestId("publish-open")).toHaveCount(0);
  });

  test("runtime status is unknown and no lifecycle action is offered, with the reason", async ({ page }) => {
    await page.goto("/studio/applications");
    const card = page.locator("[data-testid=app-card][data-app=commerce-stack]");
    await expect(card.getByTestId("runtime-status")).toHaveAttribute("data-status", "unknown");
    for (const k of ["start", "restart", "stop"]) {
      const b = card.getByTestId(`action-${k}`);
      await expect(b).toBeDisabled();
      await expect(b.locator("xpath=..")).toHaveAttribute("title", /control plane/i);
    }
    await expect(page.getByTestId("stale-banner")).toContainText("kept in this browser");
  });

  test("cards and list show the same data, and the layout choice survives a reload", async ({ page }) => {
    await page.goto("/studio/applications");
    await page.getByTestId("view-list").click();
    await expect(page.getByTestId("list-view")).toBeVisible();
    const row = page.locator("[data-testid=app-row][data-app=commerce-stack]");
    const fromList = { health: await row.getByTestId("app-health").innerText(), project: await row.getByTestId("app-project").innerText(), status: await row.getByTestId("runtime-status").getAttribute("data-status") };
    await page.reload();
    await expect(page.getByTestId("list-view")).toBeVisible();
    await page.getByTestId("view-cards").click();
    const card = page.locator("[data-testid=app-card][data-app=commerce-stack]");
    expect({ health: await card.getByTestId("app-health").innerText(), project: await card.getByTestId("app-project").innerText(), status: await card.getByTestId("runtime-status").getAttribute("data-status") }).toEqual(fromList);
    await page.reload();
    await expect(page.getByTestId("cards-view")).toBeVisible();
  });

  test("each application has its own canvas and editing B never changes A", async ({ page }) => {
    await page.goto("/studio/applications");
        await page.locator("[data-testid=app-card][data-app=commerce-stack] [data-testid=app-open]").click();
    await page.getByTestId("tab-designer").click();
    await page.waitForSelector("[data-testid=module-node]");
    const before = await page.getByTestId("module-node").count();

    await page.goto("/studio/applications");
    await create(page, "shop-b");
    expect(await page.getByTestId("module-node").count()).toBe(0);
    // Add a module to B from the catalog, then let the draft save.
    await page.locator("[data-testid=catalog-tile][data-class-id=nginx]").click();
    await expect(page.getByTestId("module-node")).toHaveCount(1);
    await expect(page.getByTestId("save-state")).toHaveAttribute("data-sync", "saved", { timeout: 8000 });

    await page.goto("/studio/applications/commerce-stack/designer");
    await page.waitForSelector("[data-testid=module-node]");
    expect(await page.getByTestId("module-node").count()).toBe(before);
    await page.goto("/studio/applications/shop-b/designer");
    await page.waitForSelector("[data-testid=module-node]");
    await expect(page.getByTestId("module-node")).toHaveCount(1);
  });

  test("search and filters work in both layouts", async ({ page }) => {
    await page.goto("/studio/applications");
    await create(page, "payments-api");
    await page.goto("/studio/applications");
    await expect(page.getByTestId("result-count")).toContainText("of 2");
    for (const view of ["view-cards", "view-list"]) {
      await page.getByTestId(view).click();
      await page.getByTestId("app-search").fill("payments");
      await expect(page.getByTestId("app-name")).toHaveCount(1);
      await expect(page.getByTestId("app-name")).toHaveText("payments-api");
      await page.getByTestId("app-search").fill("zzz");
      await expect(page.getByTestId("empty-state")).toContainText("No applications match");
      await page.getByTestId("clear-filters").click();
      await expect(page.getByTestId("app-name")).toHaveCount(2);
    }
    await page.getByTestId("filter-status").selectOption("running");
    await expect(page.getByTestId("empty-state")).toBeVisible();
    await page.getByTestId("filter-status").selectOption("unknown");
    await expect(page.getByTestId("app-name")).toHaveCount(2);
    await page.getByTestId("sort-direction").click();
    await expect(page.getByTestId("app-name").first()).toHaveText("payments-api");
  });

  test("the icon is chosen with the editor's own logos, shown everywhere, and persists", async ({ page }) => {
    await page.goto("/studio/applications");
    await create(page, "icon-app", { icon: "postgresql" });
    await page.goto("/studio/applications");
    await expect(page.locator("[data-testid=app-card][data-app=icon-app] [data-testid=app-icon]")).toHaveAttribute("data-icon", "postgresql");
    await page.goto("/studio/applications/icon-app/settings");
    await page.getByTestId("settings-icon-nginx").click();
    await page.getByTestId("settings-save").click();
    await expect(page.getByTestId("toast").first()).toContainText("Saved");
    await page.reload();
    await expect(page.locator("[data-testid=app-icon]").first()).toHaveAttribute("data-icon", "nginx");
    await page.goto("/studio/applications");
    await page.getByTestId("view-list").click();
    await expect(page.locator("[data-testid=app-row][data-app=icon-app] [data-testid=app-icon]")).toHaveAttribute("data-icon", "nginx");
  });

  test("publishing makes an immutable version; deploying needs a control plane", async ({ page }) => {
    await page.goto("/studio/applications/commerce-stack");
    await page.getByTestId("overview-publish").click();
    await page.getByTestId("publish-label").fill("first");
    await page.getByTestId("publish-confirm").click();
    await expect(page.getByTestId("publish-done")).toContainText("version 1");
    await page.getByTestId("publish-close").click();
    await page.getByTestId("tab-versions").click();
    await expect(page.getByTestId("version-row")).toHaveCount(1);
    // Publishing the same draft again is refused with a reason.
    await page.getByTestId("versions-publish").click();
    await page.getByTestId("publish-confirm").click();
    await expect(page.getByTestId("publish-error")).toContainText("already published");
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("versions-deploy-1")).toBeDisabled();
  });

  test("a design saves itself while you work, and the header never names the runtime", async ({ page }) => {
    await page.goto("/");
    await page.waitForSelector("[data-testid=module-node]");
    const before = await page.getByTestId("module-node").count();
    await page.locator("[data-testid=catalog-tile][data-class-id=nginx]").click();
    await expect(page.getByTestId("module-node")).toHaveCount(before + 1);
    await expect(page.getByTestId("save-state")).toHaveText("All changes saved");
    await page.reload();
    await page.waitForSelector("[data-testid=module-node]");
    await expect(page.getByTestId("module-node")).toHaveCount(before + 1);
    await expect(page.getByTestId("header")).not.toContainText("Incus");
    // Visible on a narrow window too.
    await page.setViewportSize({ width: 900, height: 700 });
    await expect(page.getByTestId("save-state")).toBeVisible();
  });

  test("double-click renames the design; Escape keeps the old name; an application's name is fixed", async ({ page }) => {
    // In an application the name cannot change, and double-click says why.
    await page.goto("/studio/applications");
    await page.locator("[data-testid=app-card][data-app=commerce-stack] [data-testid=app-open]").click();
    await page.getByTestId("tab-designer").click();
    await page.waitForSelector("[data-testid=module-node]");
    await page.getByTestId("assembly-name").dblclick();
    await expect(page.getByTestId("assembly-name-input")).toHaveCount(0);
    await expect(page.getByTestId("toast").first()).toContainText("name cannot be changed");

    await page.goto("/");
    await page.waitForSelector("[data-testid=module-node]");
    const label = page.getByTestId("assembly-name");
    await expect(label).toHaveText("commerce-stack");
    await label.dblclick();
    const input = page.getByTestId("assembly-name-input");
    await expect(input).toBeFocused();
    await input.fill("not-saved");
    await input.press("Escape");
    await expect(page.getByTestId("assembly-name")).toHaveText("commerce-stack");
    await page.getByTestId("assembly-name").dblclick();
    await page.getByTestId("assembly-name-input").fill("shop-front");
    await page.getByTestId("assembly-name-input").press("Enter");
    await expect(page.getByTestId("assembly-name")).toHaveText("shop-front");
    await expect(page.getByTestId("breadcrumb-name")).toHaveText("shop-front");
    // The keyboard reaches it too.
    await page.getByTestId("assembly-name").focus();
    await page.keyboard.press("F2");
    await expect(page.getByTestId("assembly-name-input")).toBeFocused();
    await page.keyboard.press("Escape");
  });

  test("applications are exported and imported from the list", async ({ page }) => {
    await page.goto("/studio/applications");
    const card = page.locator("[data-testid=app-card][data-app=commerce-stack]");
    await card.getByTestId("more-actions").click();
    const download = page.waitForEvent("download");
    await card.getByTestId("app-export").click();
    const file = await download;
    expect(file.suggestedFilename()).toBe("commerce-stack.assembly.json");
    const text = readFileSync((await file.path())!, "utf8");
    expect(JSON.parse(text).spec.modules.length).toBeGreaterThanOrEqual(5);

    // A file that is not an assembly is refused with the reason; nothing is created.
    await page.getByTestId("import-application-input").setInputFiles({ name: "bad.json", mimeType: "application/json", buffer: Buffer.from('{"hello":1}') });
    await expect(page.getByTestId("toast").first()).toContainText("bad.json was not imported");
    await expect(page.getByTestId("new-application-dialog")).toHaveCount(0);

    // A good one opens the dialog with its name filled in; the name can be changed before it is created.
    await page.getByTestId("import-application-input").setInputFiles({ name: "commerce-stack.assembly.json", mimeType: "application/json", buffer: Buffer.from(text) });
    const dialog = page.getByTestId("new-application-dialog");
    await expect(dialog).toContainText("Import an application");
    await expect(page.getByTestId("new-app-name")).toHaveValue("commerce-stack");
    await expect(page.getByTestId("from-file")).toBeChecked();
    await page.getByTestId("new-app-name").fill("imported-shop");
    await page.getByTestId("new-app-create").click();
    await page.waitForURL("**/studio/applications/imported-shop/designer");
    await page.waitForSelector("[data-testid=module-node]");
    expect(await page.getByTestId("module-node").count()).toBe(JSON.parse(text).spec.modules.length);
  });

  test("an unknown application and a missing canvas say so", async ({ page }) => {
    await page.goto("/studio/applications/no-such-app");
    await expect(page.getByTestId("empty-state")).toContainText("Application not found");
  });

  test("the workspace pages have no serious accessibility violations", async ({ page }) => {
    for (const path of ["/studio/applications", "/studio/applications/commerce-stack", "/studio/applications/commerce-stack/versions", "/studio/applications/commerce-stack/settings", "/studio/applications/commerce-stack/monitoring", "/studio/applications/commerce-stack/deployments"]) {
      await page.goto(path);
      await page.waitForSelector("main");
      await page.waitForTimeout(300);
      const r = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
      expect(r.violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => `${path}: ${v.id}: ${v.nodes[0]?.target.join(" ")}`)).toEqual([]);
    }
  });
});

// ---------------------------------------------------------------- control plane (stubbed)

type Status = "not-deployed" | "starting" | "running" | "degraded" | "stopping" | "stopped" | "failed";
const T = "2026-10-20T00:00:00Z";
const project = { id: "p1", name: "Shop", organizationId: "o1", organizationSlug: "acme", slug: "shop" };
const environment = { id: "e1", name: "production", slug: "production", kind: "production", incusProject: "shop-prod" };
const ok = { allowed: true, reason: null };
const no = (reason: string) => ({ allowed: false, reason });

function actionsFor(status: Status, busy: boolean, denied = false) {
  const all = (a: { allowed: boolean; reason: string | null }) => ({ start: a, restart: a, stop: a, shutdown: a, migrate: a });
  if (denied) return all(no("You may view this application but not operate it"));
  if (busy || status === "starting" || status === "stopping") return all(no("An operation is in progress"));
  switch (status) {
    case "running": return { start: no("Already running"), restart: no("Nothing is failing; shut it down to stop it"), stop: ok, shutdown: ok, migrate: ok };
    case "degraded": return { start: no("Already running"), restart: ok, stop: ok, shutdown: ok, migrate: ok };
    case "stopped": return { start: ok, restart: no("Nothing is running"), stop: no("Already stopped"), shutdown: no("Already stopped"), migrate: ok };
    case "failed": return { start: ok, restart: ok, stop: ok, shutdown: ok, migrate: ok };
    default: return all(no("Deploy a published version first"));
  }
}

interface Sim { apps: Record<string, { status: Status; denied?: boolean; versions: number; error?: string; needsConfirm?: boolean }>; ops: Record<string, { name: string; kind: string; polls: number; fail?: boolean; error?: string }>; posted: unknown[]; previews?: string[]; staleAll?: boolean; listFails?: boolean }

function appInfo(sim: Sim, name: string) {
  const a = sim.apps[name]!;
  const busy = Object.values(sim.ops).some((o) => o.name === name && o.polls < 2);
  const status: Status = sim.staleAll ? ("unknown" as Status) : a.status;
  return {
    name, description: `${name} description`, icon: name === "web-shop" ? "nginx" : "generic-box", archived: false, createdAt: T, updatedAt: T, latestRevision: a.versions || null,
    project, environment, actions: sim.staleAll ? { start: no("Status unknown"), restart: no("Status unknown"), stop: no("Status unknown"), shutdown: no("Status unknown"), migrate: no("Status unknown") } : actionsFor(a.status, busy, a.denied),
    design: { status: a.versions ? "published" : "draft", draftVersion: 1, publishedVersion: a.versions || null, hasUnpublishedChanges: !a.versions, validationErrors: 0, updatedAt: T },
    deployment: { status, modules: 2, replicas: { desired: 2, healthy: status === "running" ? 2 : status === "degraded" ? 1 : 0, running: status === "running" ? 2 : status === "degraded" ? 1 : 0, unhealthy: status === "degraded" ? 1 : 0 }, observedAt: T, stale: !!sim.staleAll, source: "incus", lastActivityAt: T, deployedVersion: a.status === "not-deployed" ? null : 1, operation: null },
  };
}

async function installApi(page: Page, sim: Sim) {
  await seed(page);
  await page.addInitScript(() => sessionStorage.setItem("apelora.apiToken", "apl_test_token_for_stub"));
  await page.route("**/api/**", async (route: Route) => {
    const req = route.request();
    const url = new URL(req.url());
    const p = url.pathname.replace("/api/v1", "");
    const json = (body: unknown, status = 200) => route.fulfill({ status, json: body });
    const err = (status: number, code: string, message: string) => json({ error: { code, message } }, status);
    if (p === "/health") return json({ status: "ok", service: "control-plane", apiVersion: "v1", provider: "incus", auth: "tokens", incus: { connected: true }, checkedAt: "2026-10-20T00:00:00Z" });
    if (p === "/whoami") return json({ name: "alex", role: "operator", permissions: ["read-inventory", "design-applications", "deploy"], deployProjects: ["shop-prod"], capabilities: [], platformPermissions: [] });
    if (p === "/cluster/members") return json({ clustered: true, members: ["node-a", "node-b", "node-c"].map((n) => ({ name: n, status: n === "node-c" ? "Offline" : "Online", architecture: "x86_64", roles: [], groups: [], failureDomain: "default", database: true, message: "", url: "" })), quorum: {}, source: { provider: "incus", observedAt: T }, versions: {} });
    if (p === "/applications" && req.method() === "GET") return sim.listFails ? err(500, "internal", "database unavailable") : json({ items: Object.keys(sim.apps).map((n) => appInfo(sim, n)), observedAt: T, stale: !!sim.staleAll });
    const m = /^\/applications\/([^/]+)(?:\/(.*))?$/.exec(p);
    if (!m) return err(404, "not_found", `unstubbed ${req.method()} ${p}`);
    const [, name, rest] = m as unknown as [string, string, string | undefined];
    if (!sim.apps[name]) return err(404, "not_found", `Application ${name} does not exist`);
    const a = sim.apps[name]!;
    if (!rest) return json({ application: appInfo(sim, name), modules: a.status === "not-deployed" ? [] : [{ module: "web", availability: "stateless", replicas: 2, instances: [{ instance: `${name}-web-1`, location: "node-a", status: "Running", unhealthy: false }, { instance: `${name}-web-2`, location: "node-b", status: a.status === "degraded" ? "Stopped" : "Running", unhealthy: a.status === "degraded" }] }], operations: Object.entries(sim.ops).filter(([, o]) => o.name === name).map(([id, o]) => opDto(id, o)), versions: a.versions ? [{ revision: 1, published: true, label: "first", draftVersion: 1, createdBy: "alex", createdAt: T }] : [] });
    if (rest === "migration") {
      const target = url.searchParams.get("target") ?? "";
      const blocked = target === "node-a" ? ["node-a runs aarch64, the instance needs x86_64"] : [];
      const item = (instance: string, from: string, stops: boolean) => ({ module: "web", instance, from, alreadyThere: from === target, live: false, stops, copyMib: 0, blocked: from === target ? [] : blocked, notes: stops ? ["it stops for the move (a container cannot move while running) and starts again on " + target] : [] });
      const items = [item("web-1", "node-a", true), item("web-2", "node-b", true)];
      sim.previews = [...(sim.previews ?? []), target];
      return json({ source: { provider: "incus", observedAt: T }, target, items, possible: !blocked.length && items.some((i) => !i.alreadyThere), stops: items.filter((i) => i.stops && !i.alreadyThere).map((i) => i.instance) });
    }
    if (rest === "draft") return json({ document: { apiVersion: "fabric.apelora/v1alpha1", kind: "ApplicationAssembly", metadata: { name, uid: "assembly-x" }, spec: { modules: [], connections: [] }, editor: { layoutVersion: 1, density: "compact", nodes: [] } }, version: 1, updatedAt: T, updatedBy: "alex", validationErrors: [] });
    if (rest === "events") return json({ items: [] });
    if (rest === "operations" && req.method() === "POST") {
      const body = req.postDataJSON() as { action: string; confirmDisruption: boolean };
      sim.posted.push({ name, ...body });
      if (a.needsConfirm && !body.confirmDisruption) return err(409, "confirmation_required", "Clients of this application will lose service while it restarts. Confirm to continue.");
      const id = `op${Object.keys(sim.ops).length + 1}`;
      sim.ops[id] = { name, kind: body.action, polls: 0, fail: !!a.error, error: a.error };
      return json({ operation: opDto(id, sim.ops[id]!) }, 202);
    }
    const op = /^operations\/(.+)$/.exec(rest ?? "");
    if (op) {
      const o = sim.ops[op[1]!]!;
      o.polls++;
      if (o.polls >= 2) {
        if (o.fail) a.status = "failed";
        else a.status = o.kind === "stop" || o.kind === "shutdown" ? "stopped" : "running";
      } else a.status = o.kind === "stop" || o.kind === "shutdown" ? "stopping" : "starting";
      return json(opDto(op[1]!, o));
    }
    return err(404, "not_found", `unstubbed ${req.method()} ${p}`);
  });
}

function opDto(id: string, o: { kind: string; polls: number; fail?: boolean; error?: string }) {
  const done = o.polls >= 2;
  return { id, kind: o.kind, state: !done ? "running" : o.fail ? "failed" : "succeeded", requestedBy: "alex", createdAt: T, startedAt: T, finishedAt: done ? T : null, stepsDone: done && !o.fail ? 2 : done ? 1 : 0, stepsTotal: 2, error: done && o.fail ? o.error : null, phases: [{ action: o.kind === "stop" ? "stop" : "start", state: done ? (o.fail ? "failed" : "succeeded") : "running", steps: [{ module: "web", instance: "web-1", state: done ? "succeeded" : "running", error: null }, { module: "web", instance: "web-2", state: done ? (o.fail ? "failed" : "succeeded") : "pending", error: done && o.fail ? o.error : null }] }] };
}

const act = (page: Page, app: string, kind: string) => page.locator(`[data-testid=app-card][data-app=${app}]`).getByTestId(`action-${kind}`);

test.describe("applications on the control plane (stubbed API)", () => {
  test("which buttons are available follows the application's state", async ({ page }) => {
    const sim: Sim = { ops: {}, posted: [], apps: { "web-run": { status: "running", versions: 1 }, "web-stop": { status: "stopped", versions: 1 }, "web-deg": { status: "degraded", versions: 1 }, "web-new": { status: "not-deployed", versions: 0 }, "web-ro": { status: "stopped", versions: 1, denied: true } } };
    await installApi(page, sim);
    await page.goto("/studio/applications");
    await expect(page.getByTestId("app-card")).toHaveCount(5);
    const allowed = async (app: string) => Object.fromEntries(await Promise.all(["start", "restart", "stop"].map(async (k) => [k, await act(page, app, k).isEnabled()])));
    expect(await allowed("web-run")).toEqual({ start: false, restart: false, stop: true });
    expect(await allowed("web-stop")).toEqual({ start: true, restart: false, stop: false });
    expect(await allowed("web-deg")).toEqual({ start: false, restart: true, stop: true });
    expect(await allowed("web-new")).toEqual({ start: false, restart: false, stop: false });
    expect(await allowed("web-ro")).toEqual({ start: false, restart: false, stop: false });
    await expect(act(page, "web-ro", "start").locator("xpath=..")).toHaveAttribute("title", /not operate/);
    await expect(act(page, "web-new", "start").locator("xpath=..")).toHaveAttribute("title", /published version/);
  });

  test("Start stays pending until the control plane confirms, then Stop becomes the only action", async ({ page }) => {
    const sim: Sim = { ops: {}, posted: [], apps: { "web-stop": { status: "stopped", versions: 1 } } };
    await installApi(page, sim);
    await page.goto("/studio/applications");
    await act(page, "web-stop", "start").click();
    await expect(page.getByTestId("lifecycle").first()).toHaveAttribute("data-busy", "true");
    for (const k of ["start", "restart", "stop"]) await expect(act(page, "web-stop", k)).toBeDisabled();
    expect(sim.posted).toEqual([{ name: "web-stop", action: "start", confirmDisruption: false }]);
    await expect(page.locator("[data-testid=app-card] [data-testid=runtime-status]")).toHaveAttribute("data-status", "running", { timeout: 15000 });
    await expect(act(page, "web-stop", "stop")).toBeEnabled();
    await expect(act(page, "web-stop", "start")).toBeDisabled();
    await expect(act(page, "web-stop", "restart")).toBeDisabled();
    await expect(page.getByTestId("toast").first()).toContainText("finished");
  });

  test("Stop asks first, and a failure shows the real error and the real state", async ({ page }) => {
    const sim: Sim = { ops: {}, posted: [], apps: { "web-run": { status: "running", versions: 1, error: "instance web-2 did not stop within 30s" } } };
    await installApi(page, sim);
    await page.goto("/studio/applications");
    await act(page, "web-run", "stop").click();
    await expect(page.getByTestId("lifecycle-dialog")).toBeVisible();
    expect(sim.posted).toEqual([]);
    await page.getByTestId("lifecycle-confirm").click();
    await expect(page.getByTestId("toast").first()).toContainText("instance web-2 did not stop within 30s", { timeout: 15000 });
    await expect(page.locator("[data-testid=app-card] [data-testid=runtime-status]")).toHaveAttribute("data-status", "failed");
    await page.goto("/studio/applications/web-run/deployments");
    await expect(page.getByTestId("operation-error")).toContainText("did not stop within 30s");
  });

  test("a restart that would interrupt clients asks again before it is sent with confirmation", async ({ page }) => {
    const sim: Sim = { ops: {}, posted: [], apps: { "web-deg": { status: "degraded", versions: 1, needsConfirm: true } } };
    await installApi(page, sim);
    await page.goto("/studio/applications");
    await act(page, "web-deg", "restart").click();
    await page.getByTestId("lifecycle-confirm").click();
    await expect(page.getByTestId("lifecycle-warning")).toContainText("lose service");
    await page.getByTestId("lifecycle-confirm-disruption").click();
    await expect(page.getByTestId("lifecycle-dialog")).toHaveCount(0);
    expect(sim.posted).toEqual([{ name: "web-deg", action: "restart", confirmDisruption: false }, { name: "web-deg", action: "restart", confirmDisruption: true }]);
  });

  test("Shut down asks each guest to stop and Stop powers off at once; each asks first and sends its own action", async ({ page }) => {
    const sim: Sim = { ops: {}, posted: [], apps: { "web-run": { status: "running", versions: 1 } } };
    await installApi(page, sim);
    await page.goto("/studio/applications");
    await expect(act(page, "web-run", "shutdown")).toBeEnabled();
    await act(page, "web-run", "shutdown").click();
    const dialog = page.getByTestId("lifecycle-dialog");
    await expect(dialog).toContainText("Shut down web-run?");
    await expect(dialog).toContainText("asks this application's instances to shut down");
    await page.getByTestId("lifecycle-confirm").click();
    await expect(page.locator("[data-testid=app-card] [data-testid=runtime-status]")).toHaveAttribute("data-status", "stopped", { timeout: 15000 });
    expect(sim.posted).toEqual([{ name: "web-run", action: "shutdown", confirmDisruption: false }]);
    await act(page, "web-run", "start").click();
    await expect(page.locator("[data-testid=app-card] [data-testid=runtime-status]")).toHaveAttribute("data-status", "running", { timeout: 15000 });
    await act(page, "web-run", "stop").click();
    await expect(dialog).toContainText("Stop web-run now?");
    await expect(dialog).toContainText("powered off at once");
    await page.getByTestId("lifecycle-confirm").click();
    await expect(page.locator("[data-testid=app-card] [data-testid=runtime-status]")).toHaveAttribute("data-status", "stopped", { timeout: 15000 });
    expect(sim.posted.at(-1)).toEqual({ name: "web-run", action: "stop", confirmDisruption: false });
  });

  test("Migrate shows what would happen to each instance before anything is asked, and what stops needs a tick", async ({ page }) => {
    const sim: Sim = { ops: {}, posted: [], apps: { "web-run": { status: "running", versions: 1 } } };
    await installApi(page, sim);
    await page.goto("/studio/applications");
    await act(page, "web-run", "migrate").click();
    const dialog = page.getByTestId("migrate-dialog");
    await expect(dialog).toContainText("Migrate web-run");
    await expect(dialog.getByTestId("migrate-confirm")).toBeDisabled();
    // An offline server cannot be chosen.
    await expect(dialog.getByTestId("migrate-target").locator("option", { hasText: "node-c (Offline)" })).toHaveJSProperty("disabled", true);
    // A target that blocks it says why and cannot be confirmed.
    await dialog.getByTestId("migrate-target").selectOption("node-a");
    await expect(dialog.getByTestId("migrate-row").nth(1)).toContainText("needs x86_64");
    await expect(dialog.getByTestId("migrate-confirm")).toBeDisabled();
    // A good one: the plan, the replicas warning and the tick.
    await dialog.getByTestId("migrate-target").selectOption("node-b");
    await expect(dialog.getByTestId("migrate-row")).toHaveCount(2);
    await expect(dialog.getByTestId("migrate-row").nth(0)).toHaveAttribute("data-how", "Stops while it moves");
    await expect(dialog.getByTestId("migrate-row").nth(1)).toHaveAttribute("data-how", "Already there");
    await expect(dialog).toContainText("Replicas of one module then share that server");
    await expect(dialog.getByTestId("migrate-confirm")).toBeDisabled();
    await dialog.getByTestId("migrate-agree").check();
    await expect(dialog.getByTestId("migrate-confirm")).toBeEnabled();
    const axe = await new AxeBuilder({ page }).include("[data-testid=migrate-dialog]").withTags(["wcag2a", "wcag2aa"]).analyze();
    expect(axe.violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => `${v.id}: ${v.nodes[0]?.target.join(" ")}`)).toEqual([]);
    expect(sim.posted, "the dry runs sent nothing").toEqual([]);
    await dialog.getByTestId("migrate-confirm").click();
    await expect(page.getByTestId("migrate-dialog")).toHaveCount(0);
    expect(sim.posted).toEqual([{ name: "web-run", action: "migrate", confirmDisruption: true, target: "node-b" }]);
  });

  test("when Incus cannot be observed the status is Unknown and nothing can be operated", async ({ page }) => {
    const sim: Sim = { ops: {}, posted: [], staleAll: true, apps: { "web-run": { status: "running", versions: 1 } } };
    await installApi(page, sim);
    await page.goto("/studio/applications");
    await expect(page.getByTestId("runtime-status")).toHaveAttribute("data-status", "unknown");
    await expect(page.getByTestId("stale-banner")).toContainText("could not observe");
    for (const k of ["start", "restart", "stop"]) await expect(act(page, "web-run", k)).toBeDisabled();
  });

  test("the list and the workspace agree with the card", async ({ page }) => {
    const sim: Sim = { ops: {}, posted: [], apps: { "web-deg": { status: "degraded", versions: 1 } } };
    await installApi(page, sim);
    await page.goto("/studio/applications");
    const card = page.locator("[data-testid=app-card][data-app=web-deg]");
    const text = { status: await card.getByTestId("runtime-status").innerText(), health: await card.getByTestId("app-health").innerText() };
    await page.getByTestId("view-list").click();
    const row = page.locator("[data-testid=app-row][data-app=web-deg]");
    expect({ status: await row.getByTestId("runtime-status").innerText(), health: await row.getByTestId("app-health").innerText() }).toEqual(text);
    await row.getByTestId("app-open").click();
    await expect(page.getByTestId("workspace-name")).toHaveText("web-deg");
    await expect(page.getByTestId("card-status").getByTestId("runtime-status")).toHaveText(text.status);
    await expect(page.getByTestId("action-restart")).toBeEnabled();
    await page.getByTestId("tab-monitoring").click();
    await expect(page.locator("[data-testid=replica-row][data-unhealthy=true]")).toHaveCount(1);
    await expect(page.getByTestId("no-metrics")).toContainText("not collected");
  });

  test("a failing list shows the error with a retry, not an empty page", async ({ page }) => {
    const sim: Sim = { ops: {}, posted: [], listFails: true, apps: {} };
    await installApi(page, sim);
    await page.goto("/studio/applications");
    await expect(page.getByTestId("error-state")).toContainText("database unavailable");
    sim.listFails = false;
    sim.apps["a-1"] = { status: "stopped", versions: 0 };
    await page.getByTestId("error-state").getByRole("button", { name: "Retry" }).click();
    await expect(page.getByTestId("app-card")).toHaveCount(1);
  });

  test("a draft cannot be deployed; the deployments page says to publish first", async ({ page }) => {
    const sim: Sim = { ops: {}, posted: [], apps: { "web-new": { status: "not-deployed", versions: 0 } } };
    await installApi(page, sim);
    await page.goto("/studio/applications/web-new/deployments");
    await expect(page.getByTestId("deploy-nothing-published")).toContainText("Drafts cannot be started or deployed");
    await expect(page.getByTestId("deploy-version")).toBeDisabled();
  });

  test("pagination works in both layouts", async ({ page }) => {
    const sim: Sim = { ops: {}, posted: [], apps: Object.fromEntries(Array.from({ length: 14 }, (_, i) => [`app-${String(i + 1).padStart(2, "0")}`, { status: "stopped" as Status, versions: 1 }])) };
    await installApi(page, sim);
    await page.goto("/studio/applications");
    await expect(page.getByTestId("app-card")).toHaveCount(12);
    await page.getByTestId("page-next").click();
    await expect(page.getByTestId("app-card")).toHaveCount(2);
    await page.getByTestId("view-list").click();
    await expect(page.getByTestId("app-row")).toHaveCount(2);
  });
});
