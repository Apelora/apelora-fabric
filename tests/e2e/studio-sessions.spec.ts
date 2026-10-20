import AxeBuilder from "@axe-core/playwright";
import { expect as baseExpect, test, type Page, type Route, type WebSocketRoute } from "@playwright/test";

// The session dialog loads the terminal code on first use; on a cold dev server that compile can
// take longer than the default five seconds, whichever test happens to open it first.
const expect = baseExpect.configure({ timeout: 15_000 });

// Terminal sessions in the Studio, against a stubbed control plane and a stubbed websocket. These
// prove the interface: what is offered and why not, the approval step, how a session is opened,
// typed into and ended. The real broker is covered by the control plane's own tests.

const T = "2026-10-20T00:00:00Z";
// The sample design is read from the Studio itself (the catalog is bundled by Vite, not importable here).
let doc: { spec: { appliances: { id: string }[] } };
let APPLIANCE = "";
let INSTANCES: string[] = [];

test.beforeAll(async ({ browser }) => {
  const page = await browser.newPage();
  await page.addInitScript(() => { localStorage.clear(); localStorage.setItem("apelora.studio.tour.v1", "1"); });
  await page.route("**/api/**", (r) => r.abort());
  await page.goto("/");
  await page.waitForSelector("[data-testid=appliance-node]");
  doc = await page.evaluate(() => JSON.parse(JSON.stringify((window as unknown as { __studio: { getState: () => { assembly: unknown } } }).__studio.getState().assembly)));
  await page.close();
  APPLIANCE = doc.spec.appliances[0]!.id;
  INSTANCES = [`shop-${APPLIANCE}`, `shop-${APPLIANCE}-r2`];
});
const project = { id: "p1", name: "Shop", organizationId: "o1", organizationSlug: "acme", slug: "shop" };
const environment = { id: "e1", name: "production", slug: "production", kind: "production", incusProject: "shop-prod" };
const ok = { allowed: true, reason: null };
const no = (reason: string) => ({ allowed: false, reason });

interface Rig {
  /** What the person may do with each instance's terminal. */
  access: Record<string, { allowed: boolean; reason?: string; approvalRequired?: boolean }>;
  posted: { path: string; body: { kind: string; cols: number; rows: number }; approval: string | null }[];
  approvals: Record<string, unknown>[];
  sockets: { url: string }[];
  /** Replies the stub websocket gives. */
  onSocket?: (ws: WebSocketRoute) => void;
}

async function install(page: Page, rig: Rig) {
  await page.addInitScript(() => {
    if (!sessionStorage.getItem("__seeded")) { localStorage.clear(); localStorage.setItem("apelora.studio.tour.v1", "1"); sessionStorage.setItem("__seeded", "1"); }
    sessionStorage.setItem("apelora.apiToken", "eyJhbGciOiJFUzI1NiJ9.e30.stub");
  });
  const app = {
    name: "shop", description: "The shop", icon: "nginx", archived: false, createdAt: T, updatedAt: T, latestRevision: 1, project, environment,
    actions: { start: no("Already running"), restart: no("Nothing is failing"), stop: ok },
    design: { status: "published", draftVersion: 1, publishedVersion: 1, hasUnpublishedChanges: false, validationErrors: 0, updatedAt: T },
    deployment: { status: "running", appliances: 1, replicas: { desired: 2, healthy: 2, running: 2, unhealthy: 0 }, observedAt: T, stale: false, source: "incus", lastActivityAt: T, deployedVersion: 1, operation: null },
  };
  await page.route("**/api/**", async (route: Route) => {
    const req = route.request();
    const p = new URL(req.url()).pathname.replace("/api/v1", "");
    const json = (body: unknown, status = 200) => route.fulfill({ status, json: body });
    const err = (status: number, code: string, message: string) => json({ error: { code, message } }, status);
    if (p === "/health") return json({ status: "ok", service: "control-plane", version: "0", apiVersion: "v1", provider: "incus", auth: "tokens", incus: { connected: true, apiVersion: "1", serverVersion: "7", clustered: false } });
    if (p === "/whoami") return json({ name: "bob", kind: "user", role: null, permissions: [], deployProjects: [], capabilities: [], platformPermissions: [] });
    if (p === "/applications") return json({ items: [app], observedAt: T, stale: false });
    if (p === "/applications/shop") return json({ application: app, appliances: [{ appliance: APPLIANCE, availability: "stateless", replicas: 2, instances: INSTANCES.map((i, n) => ({ instance: i, location: `node-${n + 1}`, status: "Running", unhealthy: false })) }], operations: [], versions: [{ revision: 1, published: true, label: "first", draftVersion: 1, createdBy: "bob", createdAt: T }] });
    if (p === "/applications/shop/draft") return json({ document: doc, version: 1, updatedAt: T, updatedBy: "bob", validationErrors: [] });
    if (p === "/applications/shop/events") return json({ items: [] });
    if (p === "/applications/shop/access") {
      return json({ observed: true, provider: "incus", idleSecs: 900, maxSecs: 3600, items: INSTANCES.map((i) => {
        const a = rig.access[i] ?? { allowed: true };
        return { instance: i, appliance: APPLIANCE, guestOs: "linux", status: "Running", kinds: [{ kind: "terminal", allowed: a.allowed, reason: a.allowed ? undefined : (a.reason ?? "you do not have instance.console on this application"), approvalRequired: !!a.approvalRequired }] };
      }) });
    }
    const open = /^\/applications\/shop\/instances\/([^/]+)\/sessions$/.exec(p);
    if (open && req.method() === "POST") {
      const approval = req.headers()["apelora-approval"] ?? null;
      rig.posted.push({ path: p, body: req.postDataJSON(), approval });
      const a = rig.access[open[1]!] ?? { allowed: true };
      if (a.approvalRequired && !approval) return err(409, "approval_required", "instance.console needs another person's approval");
      return json({ id: "11111111-2222-4333-8444-555555555555", ticket: "t".repeat(64), expiresAt: T, connectPath: "/api/v1/sessions/connect", protocol: "apelora.session.v1", kind: "terminal", instance: open[1], idleSecs: 900, maxSecs: 3600, provider: "mock" }, 201);
    }
    if (p === "/approvals" && req.method() === "POST") {
      rig.approvals.push(req.postDataJSON());
      return json({ id: "99999999-2222-4333-8444-555555555555", action: "instance.console", target: "x", requester: "user:bob", approver: null, state: "pending", reason: "x", createdAt: T, expiresAt: T }, 201);
    }
    return err(404, "not_found", `unstubbed ${req.method()} ${p}`);
  });
  await page.routeWebSocket(/\/api\/v1\/sessions\/connect/, (ws) => {
    rig.sockets.push({ url: ws.url() });
    if (rig.onSocket) return rig.onSocket(ws);
    ws.send(Buffer.from("MOCK session for shop\r\n$ "));
    let line = "";
    ws.onMessage((m) => {
      if (typeof m === "string") return;
      // Like a shell: show what is typed, run the line on Enter.
      for (const ch of Buffer.from(m).toString()) {
        if (ch !== "\r") { line += ch; ws.send(Buffer.from(ch)); continue; }
        if (line === "exit") { ws.send(JSON.stringify({ type: "closed", reason: "exited" })); ws.close(); return; }
        ws.send(Buffer.from(`\r\nran:${line}\r\n$ `));
        line = "";
      }
    });
  });
}

const rig = (over: Partial<Rig> = {}): Rig => ({ access: {}, posted: [], approvals: [], sockets: [], ...over });

test.describe("sessions on the Monitoring page", () => {
  test("each instance offers what the person may open, and says why not", async ({ page }) => {
    const r = rig({ access: { [INSTANCES[0]!]: { allowed: true }, [INSTANCES[1]!]: { allowed: false, reason: "the instance is stopped; start the application first" } } });
    await install(page, r);
    await page.goto("/studio/applications/shop/monitoring");
    const rows = page.getByTestId("replica-row");
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0).getByTestId("open-terminal")).toBeEnabled();
    await expect(rows.nth(1).getByTestId("open-terminal")).toBeDisabled();
    await expect(rows.nth(1).getByTestId("why-terminal")).toContainText("stopped; start the application first");
  });

  test("an instance in production says approval is needed", async ({ page }) => {
    await install(page, rig({ access: { [INSTANCES[0]!]: { allowed: true, approvalRequired: true } } }));
    await page.goto("/studio/applications/shop/monitoring");
    await expect(page.getByTestId("replica-row").first().getByTestId("open-terminal")).toContainText("needs approval");
  });

  test("opening a session in production asks for approval first, then opens the terminal", async ({ page }) => {
    const r = rig({ access: { [INSTANCES[0]!]: { allowed: true, approvalRequired: true } } });
    await install(page, r);
    await page.goto("/studio/applications/shop/monitoring");
    await page.getByTestId("replica-row").first().getByTestId("open-terminal").click();
    const dialog = page.getByTestId("session-dialog");
    await expect(dialog).toBeVisible();
    await expect(dialog.getByTestId("session-approval")).toBeVisible();
    await expect(dialog.getByTestId("session-open")).toBeDisabled();
    await dialog.getByTestId("approval-reason").fill("investigating a slow query");
    await dialog.getByTestId("approval-request").click();
    await expect(dialog.getByTestId("approval-asked")).toContainText("99999999");
    expect(r.approvals).toEqual([{ action: "instance.console", target: `instance:shop/${INSTANCES[0]}`, reason: "investigating a slow query" }]);
    await dialog.getByTestId("session-open").click();
    await expect(dialog.getByTestId("session-live")).toBeVisible();
    await expect(dialog.getByTestId("session-mock")).toBeVisible();
    expect(r.posted).toHaveLength(1);
    expect(r.posted[0]!.approval).toBe("99999999-2222-4333-8444-555555555555");
    expect(r.posted[0]!.body.kind).toBe("terminal");
    // The ticket is not in the address of the socket.
    expect(r.sockets).toHaveLength(1);
    expect(r.sockets[0]!.url).not.toContain("ticket");
    expect(r.sockets[0]!.url).not.toContain("tttttttt");
    await expect(dialog.getByTestId("session-terminal")).toContainText("MOCK session for shop");
  });

  test("typed input reaches the session and its output is shown; ending it says so", async ({ page }) => {
    const r = rig();
    await install(page, r);
    await page.goto("/studio/applications/shop/monitoring");
    await page.getByTestId("replica-row").first().getByTestId("open-terminal").click();
    const dialog = page.getByTestId("session-dialog");
    await expect(dialog.getByTestId("session-prepare")).toContainText("does not record what you type");
    await dialog.getByTestId("session-open").click();
    await expect(dialog.getByTestId("session-terminal")).toContainText("$");
    await page.keyboard.type("hostname");
    await page.keyboard.press("Enter");
    await expect(dialog.getByTestId("session-terminal")).toContainText("ran:hostname");
    // Escape belongs to the program in the terminal, not to the dialog.
    await page.keyboard.press("Escape");
    await expect(dialog).toBeVisible();
    await dialog.getByTestId("session-end").click();
    await expect(dialog.getByTestId("session-ended")).toContainText("You ended the session");
    await dialog.getByTestId("session-close").click();
    await expect(dialog).toHaveCount(0);
  });

  test("when the control plane ends the session, the reason is given in plain words", async ({ page }) => {
    const r = rig({ onSocket: (ws) => { ws.send(Buffer.from("$ ")); setTimeout(() => { ws.send(JSON.stringify({ type: "closed", reason: "idle" })); ws.close(); }, 200); } });
    await install(page, r);
    await page.goto("/studio/applications/shop/monitoring");
    await page.getByTestId("replica-row").first().getByTestId("open-terminal").click();
    await page.getByTestId("session-open").click();
    await expect(page.getByTestId("session-ended")).toContainText("Ended after 15 minutes without typing");
  });

  test("a refused session shows the server's reason and stays on the first step", async ({ page }) => {
    const r = rig({ access: { [INSTANCES[0]!]: { allowed: true, approvalRequired: true } } });
    await install(page, r);
    await page.goto("/studio/applications/shop/monitoring");
    await page.getByTestId("replica-row").first().getByTestId("open-terminal").click();
    await page.getByTestId("approval-id").fill("00000000-0000-4000-8000-000000000000");
    await page.route("**/api/v1/applications/shop/instances/*/sessions", (route) => route.fulfill({ status: 409, json: { error: { code: "approval_invalid", message: "the request is not approved" } } }));
    await page.getByTestId("session-open").click();
    await expect(page.getByTestId("session-error")).toContainText("the request is not approved");
    await expect(page.getByTestId("session-prepare")).toBeVisible();
    expect(r.sockets).toHaveLength(0);
  });

  test("the session dialogs have no serious accessibility violations", async ({ page }) => {
    await install(page, rig({ access: { [INSTANCES[0]!]: { allowed: true, approvalRequired: true } } }));
    await page.goto("/studio/applications/shop/monitoring");
    await page.getByTestId("replica-row").first().getByTestId("open-terminal").click();
    await expect(page.getByTestId("session-dialog")).toBeVisible();
    const results = await new AxeBuilder({ page }).include("[data-testid=session-dialog]").analyze();
    expect(results.violations.filter((v) => v.impact === "serious" || v.impact === "critical")).toEqual([]);
  });
});

test.describe("sessions from the canvas menu", () => {
  const rightClickAppliance = async (page: Page) => {
    await page.goto("/studio/applications/shop/designer");
    await page.waitForSelector("[data-testid=appliance-node]");
    await page.locator(`[data-testid=appliance-node][data-node-id="${APPLIANCE}"]`).click({ button: "right" });
    await expect(page.getByTestId("context-menu")).toBeVisible();
  };

  test("a deployed appliance offers a terminal on each of its instances", async ({ page }) => {
    const r = rig();
    await install(page, r);
    await rightClickAppliance(page);
    const menu = page.getByTestId("context-menu");
    await expect(menu).toContainText("Running instance");
    await menu.getByTestId("menu-session-terminal").hover();
    await page.getByTestId(`menu-session-terminal-${INSTANCES[1]}`).click();
    await expect(page.getByTestId("session-dialog")).toBeVisible();
    await expect(page.getByTestId("session-dialog")).toContainText(INSTANCES[1]!);
  });

  test("an entry the person may not use is shown disabled with the reason", async ({ page }) => {
    await install(page, rig({ access: { [INSTANCES[0]!]: { allowed: false, reason: "you do not have instance.console on this application" }, [INSTANCES[1]!]: { allowed: false, reason: "you do not have instance.console on this application" } } }));
    await rightClickAppliance(page);
    await page.getByTestId("menu-session-terminal").hover();
    const entry = page.getByTestId(`menu-session-terminal-${INSTANCES[0]}`);
    await expect(entry).toBeDisabled();
    await expect(page.getByTestId(`menu-session-terminal-${INSTANCES[0]}-hint`)).toContainText("instance.console");
  });

  test("an appliance with no running instance, and the library canvas, offer no session", async ({ page }) => {
    await page.addInitScript(() => { localStorage.clear(); localStorage.setItem("apelora.studio.tour.v1", "1"); });
    await page.route("**/api/**", (r) => r.abort());
    await page.goto("/");
    await page.waitForSelector("[data-testid=appliance-node]");
    await page.locator("[data-testid=appliance-node]").first().click({ button: "right" });
    await expect(page.getByTestId("context-menu")).toBeVisible();
    await expect(page.getByTestId("context-menu")).not.toContainText("terminal");
    await expect(page.getByTestId("context-menu")).not.toContainText("Running instance");
  });
});
