import { test, type Page, type Request } from "@playwright/test";
import { expect, open, openProps } from "./helpers";

// Volumes in the Studio: a singleton module keeps data in volumes set in its Properties
// window; every change is checked before it is kept, the deploy review lists them and the plan
// request carries them. Stateless modules cannot have volumes.
const health = { status: "ok", service: "control-plane", apiVersion: "v1", provider: "incus", auth: "tokens", incus: { connected: true }, checkedAt: "2026-10-20T00:00:00Z" };
const operator = { name: "alex", role: "operator", permissions: ["read-inventory", "design-applications", "deploy"], deployProjects: ["apelora"] };

async function stub(page: Page, routes: Record<string, (req: Request) => { status?: number; json?: unknown }>) {
  await page.route("**/api/**", async (route) => {
    const req = route.request();
    const handler = routes[`${req.method()} ${new URL(req.url()).pathname}`];
    if (!handler) return route.abort();
    const r = handler(req);
    return route.fulfill({ status: r.status ?? 200, json: r.json });
  });
}

test("a singleton module keeps volumes that are checked, reviewed and sent with the plan", { tag: "@smoke" }, async ({ page }) => {
  let planBody: { modules: { module: string; spec: { volumes?: unknown } }[] } | null = null;
  await stub(page, {
    "GET /api/v1/health": () => ({ json: health }),
    "GET /api/v1/whoami": () => ({ json: operator }),
    "POST /api/v1/applications/commerce-stack/plan": (req) => { planBody = req.postDataJSON(); return { json: { items: [], warnings: [] } }; },
  });
  await open(page);
  await openProps(page, "nginx-01");
  await expect(page.getByTestId("volumes-stateless")).toContainText("keep no data");
  await page.keyboard.press("Escape");

  await page.locator("[data-testid=catalog-tile][data-class-id=debian]").dragTo(page.getByTestId("canvas"), { targetPosition: { x: 300, y: 560 } });
  const debian = page.locator("[data-testid=module-node][data-node-id^=debian]");
  await expect(debian).toHaveCount(1);
  const id = (await debian.getAttribute("data-node-id"))!;
  await openProps(page, id);
  const volumes = page.getByTestId("volumes");
  await expect(volumes).toContainText("None.");
  await page.getByTestId("add-volume").click();
  const row = page.getByTestId("volume-row");
  await expect(row).toHaveCount(1);
  await expect(row.getByTestId("volume-name")).toHaveValue("data");
  await expect(row.getByTestId("volume-path")).toHaveValue("/srv/data");

  // An unsafe path is refused with the reason and the field goes back.
  await row.getByTestId("volume-path").fill("/srv/../etc");
  await row.getByTestId("volume-path").press("Enter");
  await expect(page.getByTestId("volume-error")).toContainText("without . or ..");
  await expect(row.getByTestId("volume-path")).toHaveValue("/srv/data");
  await row.getByTestId("volume-path").fill("/var/lib/app");
  await row.getByTestId("volume-path").press("Enter");
  await expect(page.getByTestId("volume-error")).toHaveCount(0);
  await row.getByTestId("volume-size").fill("2");
  await row.getByTestId("volume-size").press("Enter");
  // A second volume cannot reuse the mount path.
  await page.getByTestId("add-volume").click();
  await expect(row).toHaveCount(2);
  await row.nth(1).getByTestId("volume-path").fill("/var/lib/app/");
  await row.nth(1).getByTestId("volume-path").press("Enter");
  await expect(page.getByTestId("volume-error")).toContainText("same mount path");
  await row.nth(1).getByRole("button", { name: "Remove volume data-2" }).click();
  await expect(row).toHaveCount(1);
  await page.keyboard.press("Escape");

  await page.getByTestId("deploy-open").click();
  const dialog = page.getByTestId("deploy-dialog");
  await expect(dialog.getByTestId("deploy-volumes")).toHaveText("Keeps data (2 GiB) at /var/lib/app");
  await dialog.getByTestId("deploy-preview").click();
  await expect.poll(() => planBody).not.toBeNull();
  const sent = planBody!.modules.find((a) => a.module === id);
  expect(sent?.spec.volumes).toEqual([{ name: "data", sizeMib: 2048, path: "/var/lib/app" }]);
  expect(planBody!.modules.find((a) => a.module === "nginx-01")?.spec.volumes, "stateless: none").toBeUndefined();
});

test("a VM module can keep running when moved; the plan asks Incus for it", async ({ page }) => {
  let planBody: { modules: { module: string; spec: { kind: string; config?: Record<string, string> } }[] } | null = null;
  await stub(page, {
    "GET /api/v1/health": () => ({ json: health }),
    "GET /api/v1/whoami": () => ({ json: operator }),
    "POST /api/v1/applications/commerce-stack/plan": (req) => { planBody = req.postDataJSON(); return { json: { items: [], warnings: [] } }; },
  });
  await open(page);
  await page.locator("[data-testid=catalog-tile][data-class-id=linux-base]").dragTo(page.getByTestId("canvas"), { targetPosition: { x: 300, y: 560 } });
  const vm = page.locator("[data-testid=module-node][data-node-id^=linux-base]");
  const id = (await vm.getAttribute("data-node-id"))!;
  await openProps(page, id);
  await page.getByTestId("live-migration-toggle").check();
  await expect(page.getByTestId("live-migration")).toContainText("pause of under a second");
  await page.keyboard.press("Escape");
  await page.getByTestId("deploy-open").click();
  const dialog = page.getByTestId("deploy-dialog");
  await expect(dialog.getByTestId("deploy-live-migration")).toHaveText("Keeps running when moved");
  await dialog.getByTestId("deploy-preview").click();
  await expect.poll(() => planBody).not.toBeNull();
  const sent = planBody!.modules.find((m) => m.module === id)!;
  expect(sent.spec.kind).toBe("virtual-machine");
  expect(sent.spec.config).toMatchObject({ "migration.stateful": "true", "cluster.evacuate": "live-migrate" });
});
