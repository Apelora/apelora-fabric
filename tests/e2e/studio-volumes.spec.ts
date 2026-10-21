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

test("a module's volumes can live on the shared pool, all of them or none, and the plan carries the class", async ({ page }) => {
  let planBody: { modules: { module: string; spec: { volumes?: { name: string; class?: string }[] } }[] } | null = null;
  await stub(page, {
    "GET /api/v1/health": () => ({ json: health }),
    "GET /api/v1/whoami": () => ({ json: operator }),
    "POST /api/v1/applications/commerce-stack/plan": (req) => { planBody = req.postDataJSON(); return { json: { items: [], warnings: [] } }; },
  });
  await open(page);
  await page.locator("[data-testid=catalog-tile][data-class-id=debian]").dragTo(page.getByTestId("canvas"), { targetPosition: { x: 300, y: 560 } });
  const debian = page.locator("[data-testid=module-node][data-node-id^=debian]");
  await expect(debian).toHaveCount(1);
  const id = (await debian.getAttribute("data-node-id"))!;
  await openProps(page, id);
  await expect(page.getByTestId("volume-class")).toHaveCount(0);
  await page.getByTestId("add-volume").click();
  await page.getByTestId("add-volume").click();
  await expect(page.getByTestId("volume-row")).toHaveCount(2);
  const choose = page.getByTestId("volume-class");
  await expect(choose).toHaveValue("data-local");
  await expect(page.getByTestId("volume-shared-note")).toHaveCount(0);

  // One choice for the whole module: both volumes move together, and a new one follows.
  await choose.selectOption("data-replicated");
  await expect(page.getByTestId("volume-shared-note")).toContainText("restarts on another server");
  await expect(page.getByTestId("volume-shared-note")).toContainText("APELORA_SHARED_POOL");
  await page.getByTestId("add-volume").click();
  await expect(page.getByTestId("volume-row")).toHaveCount(3);
  await page.keyboard.press("Escape");

  await page.getByTestId("deploy-open").click();
  const dialog = page.getByTestId("deploy-dialog");
  await expect(dialog.getByTestId("deploy-volumes")).toContainText("(shared, 1 GiB)");
  await expect(dialog.getByTestId("deploy-volumes")).toContainText("Restarts on another server after its server fails and is fenced");
  await dialog.getByTestId("deploy-preview").click();
  await expect.poll(() => planBody).not.toBeNull();
  const sent = planBody!.modules.find((a) => a.module === id)!.spec.volumes!;
  expect(sent.map((v) => v.class)).toEqual(["data-replicated", "data-replicated", "data-replicated"]);
  await page.keyboard.press("Escape");

  // Back to local: the class is dropped from every volume, so the saved design stays as it was.
  await openProps(page, id);
  await page.getByTestId("volume-class").selectOption("data-local");
  await expect(page.getByTestId("volume-shared-note")).toHaveCount(0);
  const saved = await page.evaluate((nodeId) => JSON.stringify((window as unknown as { __studio: { getState: () => { assembly: { spec: { modules: { id: string; volumes?: object[] }[] } } } } }).__studio.getState().assembly.spec.modules.find((m) => m.id === nodeId)?.volumes), id);
  expect(saved).not.toContain("class");
});

test("a module can declare an application check: it is validated, reviewed and sent with the plan", async ({ page }) => {
  let planBody: { modules: { module: string; readiness?: { command: string[]; timeoutSecs?: number } }[] } | null = null;
  await stub(page, {
    "GET /api/v1/health": () => ({ json: health }),
    "GET /api/v1/whoami": () => ({ json: operator }),
    "POST /api/v1/applications/commerce-stack/plan": (req) => { planBody = req.postDataJSON(); return { json: { items: [], warnings: [] } }; },
  });
  await open(page);
  await openProps(page, "nginx-01");
  const command = page.getByTestId("readiness-command");
  await expect(command).toHaveValue("");
  await expect(page.getByTestId("readiness-timeout")).toBeDisabled();

  // A shell and unsafe words are refused with the reason, and the field goes back.
  await command.fill("sh -c true");
  await command.press("Enter");
  await expect(page.getByTestId("readiness-error")).toContainText("shell");
  await expect(command).toHaveValue("");
  await command.fill("curl;reboot");
  await command.press("Enter");
  await expect(page.getByTestId("readiness-error")).toContainText("program name");
  await command.fill("curl -fsS http://127.0.0.1/health");
  await command.press("Enter");
  await expect(page.getByTestId("readiness-error")).toHaveCount(0);
  await expect(page.getByTestId("readiness-timeout")).toBeEnabled();
  await page.getByTestId("readiness-timeout").fill("3");
  await page.getByTestId("readiness-timeout").press("Enter");
  await page.getByTestId("readiness-timeout").fill("99");
  await page.getByTestId("readiness-timeout").press("Enter");
  await expect(page.getByTestId("readiness-timeout")).toHaveValue("3");
  await page.keyboard.press("Escape");

  await page.getByTestId("deploy-open").click();
  const dialog = page.getByTestId("deploy-dialog");
  await expect(dialog.getByTestId("deploy-readiness")).toContainText("curl -fsS http://127.0.0.1/health");
  await dialog.getByTestId("deploy-preview").click();
  await expect.poll(() => planBody).not.toBeNull();
  expect(planBody!.modules.find((m) => m.module === "nginx-01")?.readiness).toEqual({ command: ["curl", "-fsS", "http://127.0.0.1/health"], timeoutSecs: 3 });
  expect(planBody!.modules.find((m) => m.module === "haproxy-01")?.readiness, "other modules have none").toBeUndefined();
  await page.keyboard.press("Escape");

  // Clearing the field goes back to the class's (none): nothing stays in the design.
  await openProps(page, "nginx-01");
  await page.getByTestId("readiness-command").fill("");
  await page.getByTestId("readiness-command").press("Enter");
  await expect(page.getByTestId("readiness-timeout")).toBeDisabled();
});
