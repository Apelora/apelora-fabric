import { expect, type Locator, type Page } from "@playwright/test";

export async function open(page: Page, opts: { fresh?: boolean } = { fresh: true }) {
  if (opts.fresh) await page.addInitScript(() => { if (!sessionStorage.getItem("__seeded")) { localStorage.clear(); localStorage.setItem("apelora.studio.tour.v1", "1"); sessionStorage.setItem("__seeded", "1"); } });
  await page.goto("/");
  await page.waitForSelector("[data-testid=module-node]");
  await page.waitForTimeout(350);
}

export const node = (page: Page, id: string): Locator => page.locator(`[data-testid=module-node][data-node-id="${id}"]`);
export const handle = (page: Page, id: string, portId: string): Locator => page.locator(`.react-flow__node[data-id="${id}"] .react-flow__handle[data-port-id="${portId}"]`);

export async function viewport(page: Page) {
  const t = await page.locator(".react-flow__viewport").evaluate((el) => (el as HTMLElement).style.transform);
  const m = /translate\(([-\d.]+)px, ([-\d.]+)px\) scale\(([-\d.]+)\)/.exec(t);
  if (!m) throw new Error(`bad viewport transform: ${t}`);
  return { x: Number(m[1]), y: Number(m[2]), zoom: Number(m[3]) };
}

export async function center(l: Locator) {
  const b = await l.boundingBox();
  if (!b) throw new Error("no box");
  return { x: b.x + b.width / 2, y: b.y + b.height / 2, width: b.width, height: b.height, left: b.x, top: b.y };
}

/** Flow-space size of a node, independent of viewport zoom. */
export async function flowSize(page: Page, id: string) {
  const { zoom } = await viewport(page);
  const b = await node(page, id).boundingBox();
  if (!b) throw new Error("no box");
  return { width: b.width / zoom, height: b.height / zoom };
}

export async function assemblyState(page: Page) {
  return page.evaluate(() => {
    const a = (window as unknown as { __studio: { getState: () => { assembly: { spec: { modules: { id: string; ports: unknown[] }[]; connections: { id: string; source: { moduleId: string; portId: string }; target: { moduleId: string; portId: string } }[] }; editor: { nodes: { id: string; x: number; y: number; width: number; height: number; custom: boolean }[]; density: string } } } } }).__studio.getState().assembly;
    return JSON.parse(JSON.stringify(a)) as typeof a;
  });
}

/** Every cable must start/end 6px beyond the outer edge of its jacks, under the plug boot (±tol flow px). */
export async function expectEdgesAnchored(page: Page, tol = 1.5) {
  const vp = await viewport(page);
  const rf = await page.locator(".react-flow").first().boundingBox();
  if (!rf) throw new Error("no canvas");
  const a = await assemblyState(page);
  expect(await edgeCount(page)).toBe(a.spec.connections.length);
  for (const c of a.spec.connections) {
    const d = await page.locator(`.react-flow__edge[data-id="${c.id}"] path.react-flow__edge-path`).getAttribute("d");
    const nums = d!.match(/-?\d+(\.\d+)?/g)!.map(Number);
    const [sx, sy, tx, ty] = [nums[0]!, nums[1]!, nums[nums.length - 2]!, nums[nums.length - 1]!];
    const sh = await handle(page, c.source.moduleId, c.source.portId).boundingBox();
    const th = await handle(page, c.target.moduleId, c.target.portId).boundingBox();
    if (!sh || !th) throw new Error(`handle missing for ${c.id}`);
    const f = (px: number, py: number) => ({ x: (px - vp.x - rf.x) / vp.zoom, y: (py - vp.y - rf.y) / vp.zoom });
    const s = f(sh.x + sh.width, sh.y + sh.height / 2);
    const t = f(th.x, th.y + th.height / 2);
    expect(Math.abs(s.x + 6 - sx), `${c.id} source x`).toBeLessThanOrEqual(tol);
    expect(Math.abs(s.y - sy), `${c.id} source y`).toBeLessThanOrEqual(tol);
    expect(Math.abs(t.x - 6 - tx), `${c.id} target x`).toBeLessThanOrEqual(tol);
    expect(Math.abs(t.y - ty), `${c.id} target y`).toBeLessThanOrEqual(tol);
  }
}

export async function addPorts(page: Page, side: "input" | "output", n: number) {
  for (let i = 0; i < n; i++) await page.getByTestId(`add-${side}`).click();
}

export async function dragHandle(page: Page, from: Locator, to: Locator) {
  const a = await center(from);
  const b = await center(to);
  await page.mouse.move(a.x, a.y);
  await page.mouse.down();
  await page.mouse.move((a.x + b.x) / 2, (a.y + b.y) / 2, { steps: 6 });
  await page.mouse.move(b.x, b.y, { steps: 6 });
  await page.mouse.up();
}

export const edgeCount = (page: Page) => page.locator(".react-flow__edge").count();
export { expect };

/** Resolves any CSS colour (oklch, color-mix…) to "rgb(r, g, b)" through a canvas. */
export async function resolvedColor(page: Page, selector: string, prop = "backgroundColor"): Promise<string> {
  return page.locator(selector).first().evaluate((el, p) => {
    const c = document.createElement("canvas").getContext("2d")!;
    c.fillStyle = "#000";
    c.fillStyle = getComputedStyle(el)[p as "color"];
    c.fillRect(0, 0, 1, 1);
    const [r, g, b] = c.getImageData(0, 0, 1, 1).data;
    return `rgb(${r}, ${g}, ${b})`;
  }, prop);
}

/** Screen position of a point along a cable (fraction t of its path length). */
export async function cablePoint(page: Page, id: string, t = 0.5) {
  return page.evaluate(({ id, t }) => {
    const p = document.querySelector(`.react-flow__edge[data-id="${id}"] path.react-flow__edge-path`) as SVGPathElement;
    const m = p.getScreenCTM()!;
    const pt = p.getPointAtLength(p.getTotalLength() * t);
    return { x: pt.x * m.a + m.e, y: pt.y * m.d + m.f };
  }, { id, t });
}

/** Opens the floating properties window for a module (double-click) and optionally switches tab. */
export async function openProps(page: Page, id: string, tab?: "general" | "config" | "ports" | "issues") {
  await node(page, id).dblclick();
  await expect(page.getByTestId("inspector")).toBeVisible();
  await expect(page.getByTestId("props-title")).toBeVisible();
  if (tab && tab !== "general") await page.getByTestId(`props-tab-${tab}`).click();
}

/**
 * Deploying starts from the Applications list. This publishes the design held in the Assembly Library
 * as version 1 of an application on the (already stubbed) control plane, goes to the list and opens
 * that version's deploy review. Routes the test stubbed itself keep working; this adds the
 * application, its detail, its version and its draft.
 */
export async function openDeploy(page: Page, opts: { name?: string; stopAt?: "list" | "review" } = {}) {
  const name = opts.name ?? "commerce-stack";
  const now = "2026-10-20T00:00:00Z";
  // The application's version is exactly what is on the canvas once it is saved.
  await expect(page.getByTestId("save-state")).toHaveText("All changes saved");
  const doc = await assemblyState(page) as unknown as Record<string, unknown>;
  const ok = { allowed: true, reason: null };
  const info = {
    name, description: "", icon: "generic-box", archived: false, createdAt: now, updatedAt: now, latestRevision: 1,
    project: { id: "p1", name: "Shop", organizationId: "o1", organizationSlug: "acme", slug: "shop" },
    environment: { id: "e1", name: "production", slug: "production", kind: "production", incusProject: "apelora" },
    actions: { start: ok, restart: ok, stop: ok },
    design: { status: "published", draftVersion: 1, publishedVersion: 1, hasUnpublishedChanges: false, validationErrors: 0, updatedAt: now },
    deployment: { status: "not-deployed", modules: 6, replicas: { desired: 0, healthy: 0, running: 0, unhealthy: 0 }, observedAt: now, stale: false, source: "incus", lastActivityAt: now, deployedVersion: null, operation: null },
  };
  await page.route("**/api/v1/applications**", async (route) => {
    const req = route.request();
    const path = new URL(req.url()).pathname.replace("/api/v1", "");
    if (req.method() !== "GET") return route.fallback();
    if (path === "/applications") return route.fulfill({ json: { items: [info], observedAt: now, stale: false } });
    if (path === `/applications/${name}`) return route.fulfill({ json: { application: info, modules: [], operations: [], versions: [{ revision: 1, published: true, label: null, draftVersion: 1, createdBy: "alex", createdAt: now }] } });
    if (path === `/applications/${name}/versions/1`) return route.fulfill({ json: { document: doc, published: true } });
    if (path === `/applications/${name}/draft`) return route.fulfill({ json: { document: doc, version: 1, updatedAt: now, updatedBy: "alex", validationErrors: [] } });
    return route.fallback();
  });
  await page.evaluate(() => sessionStorage.setItem("apelora.apiToken", "apl_test_token_for_stub"));
  await page.getByTestId("library-exit").click();
  const card = page.locator(`[data-testid=app-card][data-app=${name}]`);
  await expect(card).toBeVisible();
  if (opts.stopAt === "list") return;
  await card.getByTestId("action-deploy").click();
  await page.getByTestId("deploy-version-continue").click();
  await expect(page.getByTestId("deploy-dialog")).toBeVisible();
}

/** Back from the Applications list to the Assembly Library canvas, whose design is kept in the browser. */
export async function backToLibrary(page: Page) {
  await page.getByTestId("nav-assembly-library").click();
  await page.waitForSelector("[data-testid=module-node]");
}
