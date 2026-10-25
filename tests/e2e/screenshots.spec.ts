import { expect, test, type Page } from "@playwright/test";
import { addPorts, catalogTile, node, open, openProps } from "./helpers";
import { ME, base, health, stub, uid } from "./identity-fixtures";
import { capacity, degraded, healthy, server as healthServer, servers, troubledServers } from "./health-fixtures";
import { atRiskQuorum, groups as serverGroups, members as serverMembers, mixedBlocked, settings as serverSettings } from "./servers-fixtures";

// Run with: SCREENSHOTS=1 npx playwright test tests/e2e/screenshots.spec.ts
test.skip(!process.env.SCREENSHOTS, "screenshots are generated on demand");

for (const [w, h] of [[1440, 900], [1280, 800]] as const) {
  test(`screenshots ${w}x${h}`, async ({ page }) => {
    await page.setViewportSize({ width: w, height: h });
    // Windows and menus animate in; reduced motion shows them finished.
    await page.emulateMedia({ reducedMotion: "reduce" });
    await open(page);
    const out = (n: string) => `docs/screenshots/${n}-${w}x${h}.png`;
    await page.screenshot({ path: out("01-commerce-stack") });

    // catalog: Custom (empty state), Security & identity and Microsoft Windows open; their first subcategories show tiles
    await page.getByTestId("catalog-collapse-all").click();
    for (const cat of ["Custom", "Security & identity", "Microsoft Windows"]) await page.locator(`[data-testid=catalog-section][data-category='${cat}'] [data-testid=catalog-category]`).click();
    for (const cat of ["Security & identity", "Microsoft Windows"]) await page.locator(`[data-testid=catalog-section][data-category='${cat}'] [data-testid=catalog-subcategory]`).first().click();
    await page.screenshot({ path: out("21-catalog-custom-identity-windows"), clip: { x: 0, y: 0, width: 720, height: h } });
    await page.getByTestId("catalog-expand-all").click();

    await openProps(page, "haproxy-01");
    await page.screenshot({ path: out("02-properties-window") });
    await page.getByTestId("props-close").click();

    await node(page, "nginx-01").click({ button: "right" });
    await page.screenshot({ path: out("10-context-menu-module") });
    await page.keyboard.press("Escape");
    await page.locator(".react-flow__node[data-id=postgresql-01] .react-flow__handle[data-port-id=postgres]").click({ button: "right", force: true });
    await page.screenshot({ path: out("11-context-menu-port") });
    await page.keyboard.press("Escape");
    const cv = (await page.getByTestId("canvas").boundingBox())!;
    await page.mouse.click(cv.x + 120, cv.y + 120, { button: "right" });
    await page.getByTestId("menu-add").hover();
    await page.screenshot({ path: out("12-context-menu-canvas") });
    await page.keyboard.press("Escape");
    await openProps(page, "postgresql-01", "config");
    await page.locator("[data-testid=cfg-group][data-group=Limits] summary").click();
    await page.getByTestId("cfg-max-connections").fill("5");
    await page.getByTestId("cfg-max-connections").press("Enter");
    await page.screenshot({ path: out("13-configuration-form") });
    await page.getByTestId("cfg-max-connections").fill("250");
    await page.getByTestId("cfg-max-connections").press("Enter");
    await page.getByTestId("props-close").click();
    await openProps(page, "grafana-01", "ports");
    await addPorts(page, "input", 5);
    await page.waitForTimeout(200);
    await page.screenshot({ path: out("03-ports-grow-node") });

    await (await catalogTile(page, "nginx")).click();
    await page.getByTestId("validation-toggle").click();
    await page.screenshot({ path: out("04-validation") });

    await page.getByTestId("new-module").click();
    await page.getByTestId("designer-name").fill("Session cache");
    await page.getByTestId("designer-add-output").click();
    await page.getByTestId("designer-port").first().getByLabel("Port type").selectOption("redis");
    await page.screenshot({ path: out("06-module-designer") });
    await page.getByTestId("designer-tab-config").click();
    await page.getByTestId("designer-add-prop").click();
    await page.getByTestId("designer-prop").first().getByLabel("Field label").fill("Max connections");
    await page.getByTestId("designer-prop").first().getByLabel("Field type").selectOption("int");
    await page.screenshot({ path: out("14-designer-configuration") });
    await page.getByTestId("designer-save").click();
    // port placement showcase: new modules side by side
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-empty").click();
    const cv2 = (await page.getByTestId("canvas").boundingBox())!;
    const showcase = ["kafka", "haproxy", "apache", "couchdb", "redis", "mongodb", "mysql", "cassandra", "traefik", "elasticsearch", "keycloak", "mosquitto"];
    for (const [idx, cls] of showcase.entries()) {
      await page.locator(`[data-testid=catalog-search]`).fill(cls === "apache" ? "httpd" : cls);
      await (await catalogTile(page, cls)).dragTo(page.getByTestId("canvas"), { targetPosition: { x: 90 + (idx % 4) * 215, y: 80 + Math.floor(idx / 4) * 205 } });
    }
    await page.getByTestId("catalog-search").fill("");
    await page.getByRole("button", { name: "Fit to view" }).click();
    await page.waitForTimeout(500);
    await page.screenshot({ path: out("18-new-modules-port-placement") });
    void cv2;
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-sample-network").click();
    await page.waitForTimeout(500);
    await page.screenshot({ path: out("15-networked-stack-link-tags") });
    await node(page, "haproxy-01").click();
    await node(page, "nginx-01").click({ modifiers: ["ControlOrMeta"] });
    await node(page, "nginx-02").click({ modifiers: ["ControlOrMeta"] });
    await page.keyboard.press("ControlOrMeta+g");
    await page.getByTestId("component-name").fill("Web tier");
    await page.screenshot({ path: out("16-group-into-component") });
    await page.getByTestId("component-create").click();
    await page.waitForTimeout(500);
    await page.screenshot({ path: out("17-component") });
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-sample").click();
    await page.waitForTimeout(400);
    // zones: wrap the web tier, add a circle and a triangle sector, label and colour them
    await node(page, "haproxy-01").click();
    await node(page, "nginx-01").click({ modifiers: ["ControlOrMeta"] });
    await node(page, "nginx-02").click({ modifiers: ["ControlOrMeta"] });
    await node(page, "nginx-02").click({ button: "right" });
    await page.getByTestId("menu-wrap").click();
    await page.getByTestId("menu-wrap-rounded").click();
    await page.getByTestId("shape-label-input").fill("Web tier");
    await page.getByTestId("props-tab-style").click();
    await page.locator("[data-testid=shape-fill] [data-color='#3b82f6']").click();
    await page.screenshot({ path: out("19-zone-style-editor") });
    await page.getByTestId("props-close").click();
    await page.getByTestId("canvas").click({ button: "right", position: { x: 60, y: 60 } });
    await page.getByTestId("menu-add-shape").hover();
    await page.screenshot({ path: out("20-canvas-menu-add-zone") });
    await page.keyboard.press("Escape");
    await page.waitForTimeout(150);
    await page.getByTestId("density-expanded").click();
    await page.waitForTimeout(400);
    await page.screenshot({ path: out("05-expanded-density") });
  });
}

// ---- Fabric Console: sign-in, against a stubbed control plane ----------------------------------
// Nothing here reaches a real server or shows a real token: every /api call is answered by a stub
// (or aborted), and the identity providers' addresses are example domains.

const CONSOLE = process.env.APELORA_CONSOLE_URL ?? "http://127.0.0.1:5174";
const consoleHealth = { ...health, auth: "tokens" };
const EXAMPLE_CORP = { issuer: "https://login.example.org/realms/staff", clientId: "apelora-console", scopes: ["openid", "profile", "email"], label: "Example Corp" };

/**
 * Waits until no time-based transition is running (menu and button colours still fade under
 * reduced motion). Endless and scroll-driven animations are not waited for.
 */
const settled = (page: Page) => page.waitForFunction(() =>
  document.getAnimations().every((a) => a.playState !== "running" || !(a.timeline instanceof DocumentTimeline) || a.effect?.getTiming().iterations === Infinity));

/** The sign-in page as a person sees it before signing in: health answers, `whoami` refuses. */
async function signInStub(page: Page, providers: unknown[], apiTokens = true) {
  await page.route("**/api/**", (route) => route.abort());
  await page.route("**/api/v1/health", (route) => route.fulfill({ json: consoleHealth }));
  await page.route("**/api/v1/sign-in", (route) => route.fulfill({ json: { providers, apiTokens } }));
  await page.route("**/api/v1/whoami", (route) => route.fulfill({ status: 401, json: { error: { code: "unauthenticated", message: "a bearer token is required" } } }));
}

const providerView = (n: number, name: string, issuer: string, extra: Record<string, unknown> = {}) => ({
  id: uid(n), source: "console", name, issuer, audiences: ["apelora-api"], clientId: "apelora-console", scopes: ["openid", "profile", "email"],
  groupsClaim: "groups", usernameClaim: "preferred_username", mfaAmr: ["mfa", "otp", "hwk"], mfaAcr: [], authorizedParties: [],
  enabled: true, reachable: true, error: null, updatedBy: `user:${ME}`, ...extra,
});
const PROVIDERS = [
  providerView(70, "Example Corp", EXAMPLE_CORP.issuer),
  providerView(71, "Contractors", "https://id.contractors.example/oidc", { reachable: false, error: "discovery failed: connection refused" }),
  providerView(72, "Old directory", "https://sso.example.net/oidc", { enabled: false, clientId: null }),
];

for (const [w, h] of [[1440, 900], [1280, 800]] as const) {
  const out = (n: string) => `docs/screenshots/${n}-${w}x${h}.png`;

  test(`console sign-in screenshots ${w}x${h}`, async ({ page }) => {
    await page.setViewportSize({ width: w, height: h });
    await page.emulateMedia({ reducedMotion: "reduce" });
    await signInStub(page, [EXAMPLE_CORP]);
    await page.goto(`${CONSOLE}/`);
    await expect(page.getByTestId("sign-in-oidc")).toHaveText("Continue with Example Corp");
    await expect(page.getByTestId("sign-in-status")).toContainText("Apelora");
    await settled(page);
    await page.screenshot({ path: out("22-console-sign-in") });
  });

  test(`console sign-in with an API token ${w}x${h}`, async ({ page }) => {
    await page.setViewportSize({ width: w, height: h });
    await page.emulateMedia({ reducedMotion: "reduce" });
    await signInStub(page, []);
    await page.goto(`${CONSOLE}/`);
    // An obviously fake value; the field masks it anyway.
    await page.getByTestId("token-input").fill("not-a-real-token");
    await page.getByTestId("sign-in").getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByTestId("sign-in-error")).toHaveText("That token was not accepted. It may have expired or been revoked.");
    await settled(page);
    await page.screenshot({ path: out("23-console-sign-in-token-not-accepted") });
  });

  test(`console sign-in settings ${w}x${h}`, async ({ page }) => {
    await page.setViewportSize({ width: w, height: h });
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.route("**/api/**", (route) => route.abort());
    await stub(page, {
      ...base(),
      "GET /api/v1/health": { json: consoleHealth },
      "GET /api/v1/identity/sign-in": { json: { providers: PROVIDERS, apiTokens: { enabled: true, lockedByEnvironment: false }, localAccounts: { enabled: false, ready: 0, pending: 0 } } },
      "POST /api/v1/identity/sign-in/check": { json: { authorizationEndpoint: `${EXAMPLE_CORP.issuer}/auth`, tokenEndpoint: `${EXAMPLE_CORP.issuer}/token`, jwksUri: `${EXAMPLE_CORP.issuer}/keys`, keys: 2 } },
    });
    await page.goto(`${CONSOLE}/#/identity/sign-in`);
    await expect(page.getByTestId("provider-row")).toHaveCount(3);
    await expect(page.getByTestId("api-tokens-setting")).toBeVisible();
    await settled(page);
    await page.screenshot({ path: out("24-console-sign-in-settings") });

    await page.getByTestId("provider-add").click();
    const dialog = page.getByTestId("provider-dialog");
    await dialog.getByTestId("provider-name").fill("Example Corp");
    await dialog.getByTestId("provider-issuer").fill(EXAMPLE_CORP.issuer);
    await dialog.getByTestId("provider-check").click();
    await expect(dialog.getByTestId("provider-check-ok")).toContainText("2 signing keys");
    await dialog.getByTestId("provider-audiences").fill("apelora-api");
    await dialog.getByTestId("provider-client").fill("apelora-console");
    await dialog.getByTestId("provider-advanced").locator("summary").click();
    await settled(page);
    await page.screenshot({ path: out("25-console-identity-provider-dialog") });
  });
}

test.describe("phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  test("console sign-in screenshot 390x844", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await signInStub(page, [EXAMPLE_CORP]);
    await page.goto(`${CONSOLE}/`);
    await expect(page.getByTestId("sign-in-oidc")).toHaveText("Continue with Example Corp");
    await page.getByTestId("sign-in-use-token").click();
    await expect(page.getByTestId("token-input")).toBeVisible();
    await settled(page);
    await page.screenshot({ path: "docs/screenshots/26-console-sign-in-phone-390x844.png" });
  });
});

// Health page and the Studio's capacity chip, with stubbed data (made-up numbers, see health-fixtures.ts).
const healthAdmin = { name: "alex", role: "fabric-admin", permissions: ["read-inventory", "read-audit", "manage-tokens", "manage-infrastructure", "design-applications", "deploy"], deployProjects: ["apelora"] };
const incusHealth = { ...consoleHealth, provider: "incus", incus: { connected: true, apiVersion: "1.0", serverVersion: "7.0.1", clustered: true } };
/** The console scrolls inside <main>: grow the window to the page's height for a whole-page image. */
const fullHeight = async (page: Page, w: number, h: number) => {
  const height = await page.evaluate(() => document.querySelector("main")!.scrollHeight + 48);
  await page.setViewportSize({ width: w, height: Math.max(h, height) });
};
const healthStub = async (page: Page, report: unknown, list: unknown) => {
  await page.route("**/api/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = ({
      "/api/v1/health": incusHealth, "/api/v1/whoami": healthAdmin, "/api/v1/health/components": report, "/api/v1/health/servers": list,
      "/api/v1/capacity": capacity(), "/api/v1/cluster": { source: { provider: "incus", observedAt: "2026-10-21T10:00:00Z" }, server: { clustered: true }, members: [] },
    } as Record<string, unknown>)[path];
    return json ? route.fulfill({ json }) : route.abort();
  });
};

for (const [w, h] of [[1440, 900], [1280, 800]] as const) {
  test(`console health screenshots ${w}x${h}`, async ({ page }) => {
    await page.setViewportSize({ width: w, height: h });
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.clock.install({ time: new Date("2026-10-21T10:00:05Z") });
    await healthStub(page, healthy(), servers());
    await page.goto(`${CONSOLE}/#/health`);
    await expect(page.getByTestId("server-card")).toHaveCount(3);
    await fullHeight(page, w, h);
    await settled(page);
    await page.screenshot({ path: `docs/screenshots/27-console-health-${w}x${h}.png` });
  });
}

test("console health degraded screenshot 1440x900", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.clock.install({ time: new Date("2026-10-21T10:00:05Z") });
  await healthStub(page, degraded(), troubledServers());
  await page.goto(`${CONSOLE}/#/health`);
  await expect(page.getByTestId("server-card")).toHaveCount(3);
  await fullHeight(page, 1440, 900);
  await settled(page);
  await page.screenshot({ path: "docs/screenshots/28-console-health-degraded-1440x900.png" });
});

test("studio capacity screenshot 1440x900", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.addInitScript(() => sessionStorage.setItem("apelora.apiToken", "apl_test_token_for_stub"));
  await healthStub(page, healthy(), servers());
  await open(page);
  await page.getByTestId("capacity-chip").click();
  await expect(page.getByTestId("capacity-verdict")).toBeVisible();
  await settled(page);
  await page.screenshot({ path: "docs/screenshots/30-studio-capacity-1440x900.png", clip: { x: 0, y: 0, width: 900, height: 560 } });
});

test.describe("phone health", () => {
  test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  test("console health screenshot 390x844", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.clock.install({ time: new Date("2026-10-21T10:00:05Z") });
    await healthStub(page, degraded(), troubledServers());
    await page.goto(`${CONSOLE}/#/health`);
    await expect(page.getByTestId("server-card")).toHaveCount(3);
    await settled(page);
    await page.screenshot({ path: "docs/screenshots/29-console-health-phone-390x844.png" });
  });
});

// Servers area with stubbed data (servers-fixtures.ts: the test cluster's layout, made-up numbers).
const serversStub = async (page: Page, report = serverMembers()) => {
  await page.route("**/api/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    const json = ({
      "/api/v1/health": incusHealth, "/api/v1/whoami": healthAdmin, "/api/v1/cluster/members": report, "/api/v1/cluster/groups": serverGroups(),
      "/api/v1/cluster/settings": serverSettings(),
      "/api/v1/health/servers": servers("incus", ["node-1", "node-2", "node-3", "node-4"].map((n) => healthServer(n))),
    } as Record<string, unknown>)[path];
    return json ? route.fulfill({ json }) : route.abort();
  });
};

for (const [w, h] of [[1440, 900], [1280, 800]] as const) {
  test(`console servers screenshots ${w}x${h}`, async ({ page }) => {
    await page.setViewportSize({ width: w, height: h });
    await page.emulateMedia({ reducedMotion: "reduce" });
    await serversStub(page);
    await page.goto(`${CONSOLE}/#/servers/members`);
    await expect(page.getByTestId("server-member")).toHaveCount(4);
    await expect(page.getByTestId("server-member").first()).toContainText("Memory");
    await fullHeight(page, w, h);
    await settled(page);
    await page.screenshot({ path: `docs/screenshots/31-console-servers-members-${w}x${h}.png` });
  });
}

test("console servers upgrades and settings screenshots 1440x900", async ({ page }) => {
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.emulateMedia({ reducedMotion: "reduce" });
  await serversStub(page, serverMembers({ quorum: atRiskQuorum, versions: mixedBlocked }));
  await page.goto(`${CONSOLE}/#/servers/upgrades`);
  await expect(page.getByTestId("version-group")).toHaveCount(2);
  await fullHeight(page, 1440, 900);
  await settled(page);
  await page.screenshot({ path: "docs/screenshots/32-console-servers-upgrades-1440x900.png" });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.goto(`${CONSOLE}/#/servers/settings`);
  await expect(page.getByTestId("cluster-setting")).toHaveCount(4);
  await settled(page);
  await page.screenshot({ path: "docs/screenshots/33-console-servers-settings-1440x900.png" });
});

test.describe("phone servers", () => {
  test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  test("console servers screenshot 390x844", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await serversStub(page, serverMembers({ quorum: atRiskQuorum }));
    await page.goto(`${CONSOLE}/#/servers/members`);
    await expect(page.getByTestId("server-member")).toHaveCount(4);
    await page.evaluate(() => document.querySelector("[data-testid=server-tabs]")?.scrollIntoView());
    await settled(page);
    await page.screenshot({ path: "docs/screenshots/34-console-servers-phone-390x844.png" });
  });
});
