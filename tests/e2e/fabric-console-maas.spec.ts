import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page, type Request, type Route } from "@playwright/test";

// The MAAS provisioner in the Console against a stubbed control plane: what MAAS reports, power
// states in words, registering a machine with its MAAS id, an unconfigured or older control plane,
// and the phone layout.

const NOW = "2026-10-21T10:00:00Z";
const health = { status: "ok", service: "control-plane", version: "0.1.0", apiVersion: "v1", provider: "incus", auth: "tokens", incus: { connected: true }, checkedAt: NOW };
const admin = { name: "alex", role: "fabric-admin", permissions: ["read-inventory", "read-audit", "manage-tokens", "manage-infrastructure", "design-applications", "deploy"], capabilities: [], platformPermissions: [], roles: [], deployProjects: ["apelora"] };
const src = { provider: "incus", observedAt: NOW };

const maasMachine = (over: Record<string, unknown>) => ({
  systemId: "abc123", hostname: "rack-a-1.maas", status: "Deployed", powerState: "on", powerType: "redfish", architecture: "arm64/generic",
  cpuCount: 8, memoryMib: 16384, os: "ubuntu noble", addresses: ["10.0.0.11"], hardwareUuid: "4c4c4544-0042-4c10-8051-b7c04f435331",
  zone: "rack-a", pool: "default", registeredAs: null, memberStatus: null, ...over,
});

type Reply = { status?: number; json?: unknown } | "abort";
type Handler = Reply | ((req: Request) => Reply);
async function stub(page: Page, routes: Record<string, Handler>) {
  await page.route("**/api/**", async (route: Route) => {
    const req = route.request();
    const h = routes[`${req.method()} ${new URL(req.url()).pathname}`];
    if (!h) return route.fulfill({ status: 404, json: { error: { code: "not_found", message: "no stub" } } });
    const r = typeof h === "function" ? h(req) : h;
    if (r === "abort") return route.abort();
    await new Promise((res) => setTimeout(res, 50));
    return route.fulfill({ status: r.status ?? 200, json: r.json });
  });
}
const base = {
  "GET /api/v1/health": { json: health }, "GET /api/v1/whoami": { json: admin },
  "GET /api/v1/cluster/members": { json: { source: src, items: [] } },
  "GET /api/v1/cluster/groups": { json: { source: src, items: [] } },
  "GET /api/v1/cluster/recovery": { json: { source: src, items: [] } },
  "GET /api/v1/cluster/join-tokens": { json: { source: src, items: [] } },
  "GET /api/v1/machines": { json: { source: src, items: [] } },
};
const serious = async (page: Page) => (await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze()).violations
  .filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(" ")}`);

test.beforeEach(async ({ page }) => {
  await page.route("**/api/**", (route) => route.abort());
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.addInitScript(() => sessionStorage.setItem("apelora.apiToken", "apl_test"));
});

test("MAAS machines are listed in words and one is registered with its MAAS id", { tag: "@smoke" }, async ({ page }) => {
  let registered: Record<string, unknown> | null = null;
  await stub(page, {
    ...base,
    "GET /api/v1/provisioners/maas/machines": () => ({ json: { source: src, configured: true, items: [
      maasMachine({ registeredAs: registered ? "rack-a-1" : null, memberStatus: registered ? "Online" : null }),
      maasMachine({ systemId: "def456", hostname: "rack-a-2.maas", status: "New", powerState: "unknown", powerType: "manual", hardwareUuid: null, addresses: [], os: "" }),
      maasMachine({ systemId: "ghi789", hostname: "rack-b-1.maas", status: "Failed commissioning", powerState: "error" }),
    ] } }),
    "POST /api/v1/machines": (req) => { registered = req.postDataJSON() as Record<string, unknown>; return { status: 201, json: { name: "rack-a-1", provider: "maas", providerRef: "abc123", state: "registered", capabilities: {}, description: "", nextStep: "Approve it.", warnings: [], registeredBy: "x", registeredAt: NOW } }; },
  });
  await page.goto("/#/servers/machines");
  const rows = page.getByTestId("maas-row");
  await expect(rows).toHaveCount(3);
  await expect(rows.nth(0)).toContainText("rack-a-1.maas");
  await expect(rows.nth(0)).toContainText("Powered on");
  await expect(rows.nth(0)).toContainText("via redfish");
  await expect(rows.nth(0)).toContainText("8 CPUs · 16 GiB");
  await expect(rows.nth(1)).toContainText("Power unknown");
  await expect(rows.nth(1)).toContainText("via manual");
  await expect(rows.nth(2)).toContainText("Power check failed");
  await expect(rows.nth(2)).toContainText("Failed commissioning");

  await rows.nth(0).getByTestId("maas-register").click();
  const dialog = page.getByTestId("maas-register-dialog");
  await expect(dialog).toContainText("abc123");
  await expect(page.getByTestId("maas-name")).toHaveValue("rack-a-1");
  await page.getByTestId("maas-name").fill("1bad");
  await expect(page.getByTestId("maas-register-confirm")).toBeDisabled();
  await page.getByTestId("maas-name").fill("rack-a-1");
  await page.getByTestId("maas-register-confirm").click();
  await expect(page.getByTestId("maas-notice")).toContainText("rack-a-1 is registered");
  expect(registered).toEqual({ name: "rack-a-1", provider: "maas", systemId: "abc123", description: "" });
  await expect(rows.nth(0).getByTestId("maas-registered")).toContainText("Registered as rack-a-1 · Online");
  await expect(rows.nth(0).getByTestId("maas-register")).toHaveCount(0);
  expect(await serious(page)).toEqual([]);
});

test("a refused registration keeps the dialog open with the reason", async ({ page }) => {
  await stub(page, {
    ...base,
    "GET /api/v1/provisioners/maas/machines": { json: { source: src, configured: true, items: [maasMachine({})] } },
    "POST /api/v1/machines": { status: 400, json: { error: { code: "unknown_machine", message: "MAAS does not know a machine abc123" } } },
  });
  await page.goto("/#/servers/machines");
  await page.getByTestId("maas-register").click();
  await page.getByTestId("maas-register-confirm").click();
  await expect(page.getByTestId("maas-register-error")).toContainText("MAAS does not know a machine abc123");
  await expect(page.getByTestId("maas-register-dialog")).toBeVisible();
});

test("without MAAS the section says how to connect it", async ({ page }) => {
  await stub(page, { ...base, "GET /api/v1/provisioners/maas/machines": { json: { source: src, configured: false, items: [] } } });
  await page.goto("/#/servers/machines");
  await expect(page.getByTestId("maas-not-configured")).toContainText("APELORA_MAAS_URL");
  await expect(page.getByTestId("maas-not-configured")).not.toContainText("secret");
  await expect(page.getByTestId("maas-row")).toHaveCount(0);
});

test("an older control plane without the endpoint shows no MAAS section and no error", async ({ page }) => {
  await stub(page, base); // no stub for the MAAS path: a 404, as an older control plane answers
  await page.goto("/#/servers/machines");
  await expect(page.getByTestId("machines")).toBeVisible();
  await expect(page.getByTestId("maas")).toBeAttached();
  await expect(page.getByTestId("maas-not-configured")).toHaveCount(0);
  await expect(page.getByTestId("maas-error")).toHaveCount(0);
  await expect(page.getByText("Machines MAAS manages")).toHaveCount(0);
});

test("a MAAS that cannot be reached is said plainly with a retry", async ({ page }) => {
  await stub(page, { ...base, "GET /api/v1/provisioners/maas/machines": { status: 502, json: { error: { code: "provisioner_unavailable", message: "MAAS is unreachable: cannot connect" } } } });
  await page.goto("/#/servers/machines");
  await expect(page.getByTestId("maas-error")).toContainText("MAAS is unreachable");
  await expect(page.getByTestId("maas").getByRole("button", { name: "Try again" })).toBeVisible();
});

test("commissioning and installing an operating system type the hostname and do not keep the cloud-init", async ({ page }) => {
  const sent: { path: string; body: Record<string, unknown> }[] = [];
  await stub(page, {
    ...base,
    "GET /api/v1/provisioners/maas/machines": { json: { source: src, configured: true, items: [maasMachine({ status: "Ready", powerState: "off" })] } },
    "POST /api/v1/provisioners/maas/machines/abc123/commission": (req) => { sent.push({ path: "commission", body: req.postDataJSON() as Record<string, unknown> }); return { status: 202, json: { systemId: "abc123", hostname: "rack-a-1.maas", action: "commission" } }; },
    "POST /api/v1/provisioners/maas/machines/abc123/deploy": (req) => { sent.push({ path: "deploy", body: req.postDataJSON() as Record<string, unknown> }); return { status: 202, json: { systemId: "abc123", hostname: "rack-a-1.maas", action: "deploy" } }; },
    "POST /api/v1/provisioners/maas/machines/abc123/release": (req) => { sent.push({ path: "release", body: req.postDataJSON() as Record<string, unknown> }); return { status: 202, json: { systemId: "abc123", hostname: "rack-a-1.maas", action: "release" } }; },
  });
  await page.goto("/#/servers/machines");
  await page.getByTestId("maas-commission").click();
  const dialog = page.getByTestId("maas-action-dialog");
  await expect(dialog.getByTestId("maas-action-go")).toBeDisabled();
  await dialog.getByTestId("maas-action-confirm").fill("wrong");
  await expect(dialog.getByTestId("maas-action-go")).toBeDisabled();
  await dialog.getByTestId("maas-action-confirm").fill("rack-a-1.maas");
  await dialog.getByTestId("maas-action-go").click();
  await expect(page.getByTestId("maas-notice")).toContainText("being commissioned");

  await page.getByTestId("maas-deploy").click();
  await dialog.getByTestId("maas-distro").fill("noble");
  await dialog.getByTestId("maas-userdata").fill("#cloud-config\nhostname: lab\n");
  await dialog.getByTestId("maas-action-confirm").fill("rack-a-1.maas");
  await dialog.getByTestId("maas-action-go").click();
  await expect(page.getByTestId("maas-notice")).toContainText("not stored");
  await page.getByTestId("maas-release").click();
  await expect(dialog.getByTestId("maas-erase")).not.toBeChecked();
  await dialog.getByTestId("maas-action-confirm").fill("rack-a-1.maas");
  await dialog.getByTestId("maas-action-go").click();
  await expect(page.getByTestId("maas-notice")).toContainText("disks were not erased");
  expect(sent.map((s) => s.path)).toEqual(["commission", "deploy", "release"]);
  expect(sent[1]?.body).toMatchObject({ confirm: "rack-a-1.maas", distroSeries: "noble", userData: "#cloud-config\nhostname: lab\n" });
  expect(sent[2]?.body).toMatchObject({ confirm: "rack-a-1.maas", eraseDisks: false });
  await expect(page.getByText("#cloud-config")).toHaveCount(0);
  expect(await serious(page)).toEqual([]);
});

test.describe("phone", () => {
  test.use({ viewport: { width: 390, height: 800 } });
  test("the list fits at 390 px without sideways scrolling", async ({ page }) => {
    await stub(page, { ...base, "GET /api/v1/provisioners/maas/machines": { json: { source: src, configured: true, items: [maasMachine({}), maasMachine({ systemId: "def456", hostname: "rack-a-2.maas" })] } } });
    await page.goto("/#/servers/machines");
    await expect(page.getByTestId("maas-row")).toHaveCount(2);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow).toBeLessThanOrEqual(0);
    const small = await page.getByTestId("maas-register").first().boundingBox();
    expect((small?.height ?? 0) >= 24).toBe(true);
    await page.getByTestId("maas-deploy").first().click();
    await expect(page.getByTestId("maas-action-dialog")).toBeVisible();
    const overflowOpen = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflowOpen).toBeLessThanOrEqual(0);
    expect(await serious(page)).toEqual([]);
  });
});
