// SPDX-License-Identifier: AGPL-3.0-only
import { execFileSync } from "node:child_process";
import { existsSync, mkdtempSync, readFileSync, rmSync, statSync } from "node:fs";
import { createServer, type Server } from "node:http";
import type { AddressInfo } from "node:net";
import { tmpdir } from "node:os";
import { extname, join, normalize, resolve, sep } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test, type Page } from "@playwright/test";

import { base, stub, whoami } from "./identity-fixtures";

// The production builds of both applications, served with exactly the response headers of
// infra/serve/web-security-headers.conf. Proves two things in a real browser:
// 1. the content security policy is enough: no violation while the applications are used
//    (canvas, dialogs, every Console page, the terminal and its websocket);
// 2. the applications load nothing from outside their origin (installations without internet).
// The API is stubbed (page.route); it is not what is under test here.

const ROOT = resolve(fileURLToPath(import.meta.url), "../../..");
const HEADERS_FILE = join(ROOT, "infra/serve/web-security-headers.conf");

const CONF_FILE = join(ROOT, "infra/serve/apelora.conf");

/** The `set $name "value";` lines of one site (`server_name`) in the example configuration. */
function siteVariables(site: string): Record<string, string> {
  const conf = readFileSync(CONF_FILE, "utf8");
  const block = conf.split(/^server \{/m).find((b) => b.includes(`server_name ${site};`));
  if (!block) throw new Error(`no server block for ${site} in ${CONF_FILE}`);
  return Object.fromEntries([...block.matchAll(/^\s*set\s+\$(\w+)\s+"([^"]*)";/gm)].map((m) => [m[1]!, m[2]!]));
}

/** The headers the example configuration sends for a site, its variables filled in as that site sets them. */
function configuredHeaders(site: string): Record<string, string> {
  const vars = siteVariables(site);
  const out: Record<string, string> = {};
  for (const m of readFileSync(HEADERS_FILE, "utf8").matchAll(/^add_header\s+([\w-]+)\s+"([^"]*)"\s+always;/gm)) {
    out[m[1]!] = m[2]!.replace(/\$(\w+)/g, (_, v: string) => {
      if (!(v in vars)) throw new Error(`${site} does not set $${v}`);
      return vars[v]!;
    }).replace(/ +;/g, ";");
  }
  return out;
}

const TYPES: Record<string, string> = { ".html": "text/html; charset=utf-8", ".js": "text/javascript", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".json": "application/json", ".woff2": "font/woff2", ".ico": "image/x-icon", ".txt": "text/plain; charset=utf-8" };

/** A static file server like the example configuration (`try_files $uri $uri.html /index.html`): files, else the same name with .html (the /licenses page), else index.html (client-side routes). */
async function serve(dir: string, headers: Record<string, string>): Promise<{ origin: string; server: Server }> {
  const server = createServer((req, res) => {
    const path = decodeURIComponent(new URL(req.url ?? "/", "http://x").pathname);
    const usable = (f: string) => f.startsWith(dir + sep) && existsSync(f) && !statSync(f).isDirectory();
    let file = normalize(join(dir, path));
    if (!usable(file)) file = usable(`${file}.html`) ? `${file}.html` : join(dir, "index.html");
    res.writeHead(200, { ...headers, "content-type": TYPES[extname(file)] ?? "application/octet-stream" });
    res.end(readFileSync(file));
  });
  await new Promise<void>((r) => server.listen(0, "127.0.0.1", r));
  return { origin: `http://127.0.0.1:${(server.address() as AddressInfo).port}`, server };
}

const built: Record<"studio" | "console", { dir: string; origin: string; server: Server }> = {} as never;
let work = "";

test.beforeAll(async () => {
  test.setTimeout(240_000);
  work = mkdtempSync(join(tmpdir(), "apelora-web-serving-"));
  for (const [name, app, site] of [["studio", "apps/studio", "studio.example.internal"], ["console", "apps/fabric-console", "console.example.internal"]] as const) {
    const headers = configuredHeaders(site);
    expect(headers["Content-Security-Policy"]).toContain("default-src 'none'; script-src 'self';");
    // The sign-in variable is empty in the example, as for an installation using API tokens only.
    expect(headers["Content-Security-Policy"]).toContain("connect-src 'self';");
    // Both host the terminal emulator, which writes <style> elements; scripts stay 'self' only.
    expect(headers["Content-Security-Policy"]).toContain("style-src-elem 'self' 'unsafe-inline';");
    const dir = join(work, name);
    // A fresh production build, so the policy is checked against what would be shipped.
    execFileSync(join(ROOT, "node_modules/.bin/vite"), ["build", "--outDir", dir, "--emptyOutDir", "--logLevel", "error"], { cwd: join(ROOT, app), stdio: "inherit" });
    built[name] = { dir, ...(await serve(dir, headers)) };
  }
});

test.afterAll(async () => {
  for (const b of Object.values(built)) await new Promise((r) => b.server.close(r));
  if (work) rmSync(work, { recursive: true, force: true });
});

/** Records policy violations and every request that leaves the page's origin. */
async function watch(page: Page, origin: string) {
  const violations: string[] = [];
  const offOrigin: string[] = [];
  await page.addInitScript(() => {
    document.addEventListener("securitypolicyviolation", (e) => {
      (window as unknown as { __csp: string[] }).__csp.push(`${e.violatedDirective} ${e.blockedURI} ${e.sourceFile}:${e.lineNumber}`);
    });
    (window as unknown as { __csp: string[] }).__csp = [];
  });
  page.on("console", (m) => { if (/Content Security Policy/i.test(m.text())) violations.push(m.text()); });
  page.on("request", (r) => {
    const u = new URL(r.url());
    if (u.protocol !== "data:" && u.protocol !== "blob:" && u.origin !== origin && u.origin !== origin.replace("http:", "ws:")) offOrigin.push(r.url());
  });
  return {
    async check() {
      const seen = await page.evaluate(() => (window as unknown as { __csp: string[] }).__csp);
      expect([...violations, ...seen], "content security policy violations").toEqual([]);
      expect(offOrigin, "requests that left the origin").toEqual([]);
    },
  };
}

test.describe("built applications under the serving policy", () => {
  test("both applications serve the licence notices at /licenses without a violation or an outside request", async ({ page, request }) => {
    for (const app of ["console", "studio"] as const) {
      const { origin } = built[app];
      const w = await watch(page, origin);
      await page.goto(origin + "/licenses");
      await expect(page.getByRole("heading", { level: 1, name: "Licences" })).toBeVisible();
      // Apelora's licence and the source offer its network clause requires.
      await expect(page.getByText("GNU Affero General Public License, version 3 only (AGPL-3.0-only)")).toBeVisible();
      await expect(page.getByRole("link", { name: "https://github.com/Apelora/apelora-fabric" })).toBeVisible();
      // Third-party packages from both ecosystems, with their licences.
      await expect(page.getByRole("row").filter({ hasText: "axum" }).first()).toContainText(/MIT/);
      await expect(page.getByRole("row").filter({ hasText: "react-dom" }).first()).toContainText("MIT");
      await w.check();
      const data = await (await request.get(origin + "/licenses.json")).json() as { licence: string; problems: string[]; components: { ecosystem: string }[] };
      expect(data.licence).toBe("AGPL-3.0-only");
      expect(data.problems).toEqual([]);
      expect(new Set(data.components.map((c) => c.ecosystem))).toEqual(new Set(["cargo", "npm"]));
      const texts = await request.get(origin + "/third-party-licenses.txt");
      expect(texts.headers()["content-type"]).toContain("text/plain");
      expect(await texts.text()).toContain("Permission is hereby granted");
    }
  });

  test("the Console works on every page without a violation or an outside request", async ({ page }) => {
    const { origin } = built.console;
    const w = await watch(page, origin);
    await page.addInitScript(() => sessionStorage.setItem("apelora.apiToken", "eyJhbGciOiJFUzI1NiJ9.e30.stub"));
    await stub(page, base());
    await page.goto(origin + "/");
    await expect(page.getByTestId("whoami")).toBeVisible();
    const links = page.getByRole("navigation", { name: "Console" }).getByRole("link");
    const count = await links.count();
    expect(count).toBeGreaterThan(8);
    for (let i = 0; i < count; i++) {
      await links.nth(i).click();
      await page.waitForLoadState("networkidle");
    }
    await w.check();
  });

  test("a terminal session opened from the Console works without a violation or an outside request", async ({ page }) => {
    const { origin } = built.console;
    const w = await watch(page, origin);
    const T = "2026-10-20T00:00:00Z";
    await page.addInitScript(() => sessionStorage.setItem("apelora.apiToken", "eyJhbGciOiJFUzI1NiJ9.e30.stub"));
    const instance = "shop-web-01";
    // A person with instance.console who may also read the instance list.
    await stub(page, {
      ...base(whoami(["cluster.view", "application.view", "instance.console"], { platform: [] })),
      "GET /api/v1/instances": { json: { source: { provider: "mock", observedAt: T }, items: [{ name: instance, project: "apelora", kind: "container", status: "Running", location: "node-1", architecture: "x86_64", description: "", managed: true, owner: { application: "shop", module: "web-01" } }] } },
      "GET /api/v1/applications/shop/access": { json: { observed: true, provider: "mock", idleSecs: 900, maxSecs: 3600, items: [{ instance, module: "web-01", guestOs: "linux", status: "Running", kinds: [{ kind: "terminal", allowed: true, approvalRequired: false }] }] } },
      [`POST /api/v1/applications/shop/instances/${instance}/sessions`]: { status: 201, json: { id: "11111111-2222-4333-8444-555555555555", ticket: "t".repeat(64), expiresAt: T, connectPath: "/api/v1/sessions/connect", protocol: "apelora.session.v1", kind: "terminal", instance, idleSecs: 900, maxSecs: 3600, provider: "mock" } },
    });
    await page.routeWebSocket(/\/api\/v1\/sessions\/connect/, (ws) => {
      ws.send(Buffer.from("\x1b[1;32mMOCK\x1b[0m session for shop\r\n$ "));
      ws.onMessage((m) => { if (typeof m !== "string") ws.send(Buffer.from(m)); });
    });
    await page.goto(origin + "/#/instances");
    await page.getByTestId("instance-row").first().getByTestId("terminal-open").click();
    const dialog = page.getByTestId("session-dialog");
    await dialog.getByTestId("session-open").click();
    await expect(dialog.getByTestId("session-terminal")).toContainText("MOCK session for shop", { timeout: 15_000 });
    await page.keyboard.type("ls");
    await expect(dialog.getByTestId("session-terminal")).toContainText("$ ls");
    await w.check();
  });

  test("the Console sign-in page works without a violation or an outside request", async ({ page }) => {
    const { origin } = built.console;
    const w = await watch(page, origin);
    await stub(page, {
      "GET /api/v1/health": base()["GET /api/v1/health"]!,
      "GET /api/v1/whoami": { status: 401, json: { error: { code: "unauthenticated", message: "a bearer token is required" } } },
      "GET /api/v1/sign-in": { json: { providers: [], apiTokens: true } },
    });
    await page.goto(origin + "/");
    await expect(page.getByTestId("sign-in-status")).toHaveText(/^Apelora \d+\.\d+\.\d+, /);
    await w.check();
  });

  test("the Studio canvas, its dialogs and the terminal websocket work without a violation or an outside request", async ({ page }) => {
    const { origin } = built.studio;
    const w = await watch(page, origin);
    await page.addInitScript(() => { if (!sessionStorage.getItem("__seeded")) { localStorage.clear(); localStorage.setItem("apelora.studio.tour.v1", "1"); sessionStorage.setItem("__seeded", "1"); } });
    await page.route("**/api/**", (r) => r.abort());
    await page.goto(origin + "/");
    await expect(page.getByTestId("module-node").first()).toBeVisible();
    // Logos (bundled files and inlined data: images) render.
    await expect(page.locator("[data-testid=module-node] img").first()).toBeVisible();
    await page.keyboard.press("?");
    await page.keyboard.press("Escape");
    // The terminal websocket goes to the same origin, as in production.
    const opened = await page.evaluate(async () => {
      const ws = new WebSocket(`${location.protocol === "https:" ? "wss" : "ws"}://${location.host}/api/v1/sessions/connect`, ["apelora.session.v1", "ticket.probe"]);
      return await new Promise<string>((done) => { ws.onopen = () => done("open"); ws.onerror = () => done("error"); setTimeout(() => done("timeout"), 3000); });
    });
    expect(["open", "error"]).toContain(opened); // refused by the static server, but not by the policy
    await w.check();
  });

  test("a terminal session (terminal emulator and websocket) works without a violation or an outside request", async ({ page }) => {
    const { origin } = built.studio;
    const w = await watch(page, origin);
    const T = "2026-10-20T00:00:00Z";
    await page.addInitScript(() => {
      localStorage.setItem("apelora.studio.tour.v1", "1");
      sessionStorage.setItem("apelora.apiToken", "eyJhbGciOiJFUzI1NiJ9.e30.stub");
    });
    const app = {
      name: "shop", description: "", icon: "nginx", archived: false, createdAt: T, updatedAt: T, latestRevision: 1,
      project: { id: "p1", name: "Shop", organizationId: "o1", organizationSlug: "acme", slug: "shop" },
      environment: { id: "e1", name: "production", slug: "production", kind: "production", incusProject: "shop-prod" },
      actions: { start: { allowed: false, reason: "running" }, restart: { allowed: false, reason: "-" }, stop: { allowed: true, reason: null } },
      design: { status: "published", draftVersion: 1, publishedVersion: 1, hasUnpublishedChanges: false, validationErrors: 0, updatedAt: T },
      deployment: { status: "running", modules: 1, replicas: { desired: 1, healthy: 1, running: 1, unhealthy: 0 }, observedAt: T, stale: false, source: "incus", lastActivityAt: T, deployedVersion: 1, operation: null },
    };
    const instance = "shop-web-01";
    await page.route("**/api/**", (route) => {
      const req = route.request();
      const p = new URL(req.url()).pathname.replace("/api/v1", "");
      const json = (body: unknown, status = 200) => route.fulfill({ status, json: body });
      if (p === "/health") return json({ status: "ok", service: "control-plane", apiVersion: "v1", provider: "incus", auth: "tokens", incus: { connected: true }, checkedAt: T });
      if (p === "/whoami") return json({ name: "bob", kind: "user", role: null, permissions: [], deployProjects: [], capabilities: [], platformPermissions: [] });
      if (p === "/applications") return json({ items: [app], observedAt: T, stale: false });
      if (p === "/applications/shop") return json({ application: app, modules: [{ module: "web-01", availability: "stateless", replicas: 1, instances: [{ instance, location: "node-1", status: "Running", unhealthy: false }] }], operations: [], versions: [] });
      if (p === "/applications/shop/events") return json({ items: [] });
      if (p === "/applications/shop/access") return json({ observed: true, provider: "incus", idleSecs: 900, maxSecs: 3600, items: [{ instance, module: "web-01", guestOs: "linux", status: "Running", kinds: [{ kind: "terminal", allowed: true, approvalRequired: false }] }] });
      if (p === `/applications/shop/instances/${instance}/sessions` && req.method() === "POST") {
        return json({ id: "11111111-2222-4333-8444-555555555555", ticket: "t".repeat(64), expiresAt: T, connectPath: "/api/v1/sessions/connect", protocol: "apelora.session.v1", kind: "terminal", instance, idleSecs: 900, maxSecs: 3600, provider: "mock" }, 201);
      }
      return json({ error: { code: "not_found", message: `unstubbed ${req.method()} ${p}` } }, 404);
    });
    await page.routeWebSocket(/\/api\/v1\/sessions\/connect/, (ws) => {
      ws.send(Buffer.from("\x1b[1;32mMOCK\x1b[0m session for shop\r\n$ "));
      ws.onMessage((m) => { if (typeof m !== "string") ws.send(Buffer.from(m)); });
    });
    await page.goto(origin + "/studio/applications/shop/monitoring");
    await page.getByTestId("replica-row").first().getByTestId("open-terminal").click();
    const dialog = page.getByTestId("session-dialog");
    await dialog.getByTestId("session-open").click();
    await expect(dialog.getByTestId("session-terminal")).toContainText("MOCK session for shop", { timeout: 15_000 });
    await page.keyboard.type("ls");
    await expect(dialog.getByTestId("session-terminal")).toContainText("$ ls");
    await w.check();
  });
});
