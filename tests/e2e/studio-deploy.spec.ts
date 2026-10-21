import { test, type Page, type Request } from "@playwright/test";
import { expect, open, openDeploy, openProps } from "./helpers";

// Deploying from the Studio, against a stubbed control plane: the Studio must only deploy
// modules that have a runtime, show the server's plan before changing anything, follow the
// jobs, and overlay live state without touching the design.
const now = "2026-10-20T00:00:00Z";
const health = { status: "ok", service: "control-plane", apiVersion: "v1", provider: "incus", auth: "tokens", incus: { connected: true }, checkedAt: "2026-10-20T00:00:00Z" };
const operator = { name: "alex", role: "operator", permissions: ["read-inventory", "design-applications", "deploy"], deployProjects: ["apelora"] };
const item = (module: string, action: string, reason = "not deployed yet") => ({ module, instance: `commerce-stack-${module}`, action, reason });
const job = (id: string, status: string) => ({ id, kind: "instance.create", status, attempts: 1, maxAttempts: 5, lastError: null, incusOperation: null, request: {}, createdAt: now, updatedAt: now });

async function stub(page: Page, routes: Record<string, (req: Request) => { status?: number; json?: unknown }>) {
  await page.route("**/api/**", async (route) => {
    const req = route.request();
    const handler = routes[`${req.method()} ${new URL(req.url()).pathname}`];
    if (!handler) return route.abort();
    const r = handler(req);
    return route.fulfill({ status: r.status ?? 200, json: r.json });
  });
}

test.describe("deploying from the Studio", () => {
  test("without a control plane the list explains why nothing can be deployed", async ({ page }) => {
    await page.route("**/api/**", (route) => route.abort());
    await page.addInitScript(() => { if (!sessionStorage.getItem("__seeded")) { localStorage.clear(); localStorage.setItem("apelora.studio.tour.v1", "1"); sessionStorage.setItem("__seeded", "1"); } });
    await page.goto("/studio/applications");
    const deploy = page.locator("[data-testid=app-card][data-app=commerce-stack]").getByTestId("action-deploy");
    await expect(deploy).toBeDisabled();
    await expect(deploy.locator("xpath=..")).toHaveAttribute("title", /needs a control plane/i);
    // The canvas offers no way to deploy: deploying is done from the list.
    await page.goto("/");
    await page.waitForSelector("[data-testid=module-node]");
    await expect(page.getByTestId("deploy-open")).toHaveCount(0);
  });

  test("plan, approve, follow the jobs and see live state per module", { tag: "@smoke" }, async ({ page }) => {
    let deployed: { body: Record<string, unknown>; key: string | undefined } | null = null;
    let planBody: unknown = null;
    let done = false;
    let drafts = 0;
    await page.route("**/api/v1/applications/commerce-stack/draft", (route) => { if (route.request().method() !== "GET") drafts += 1; return route.fallback(); });
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health }),
      "GET /api/v1/whoami": () => ({ json: operator }),
      "POST /api/v1/applications/commerce-stack/plan": (req) => {
        planBody = req.postDataJSON();
        return { json: { items: [item("nginx-01", "create"), { ...item("nginx-01", "create"), instance: "commerce-stack-nginx-01-r2", replica: 2 }, item("nginx-02", "create")], warnings: ["single points of failure: nginx-02 runs as one instance"] } };
      },
      "POST /api/v1/applications/commerce-stack/deployments": (req) => {
        deployed = { body: req.postDataJSON() as Record<string, unknown>, key: req.headers()["idempotency-key"] };
        return { status: 202, json: { application: "commerce-stack", revision: 1, plan: { items: [item("nginx-01", "create"), item("nginx-02", "create")], warnings: [] },
          jobs: [{ module: "nginx-01", instance: "commerce-stack-nginx-01", action: "create", jobId: "j1" }, { module: "nginx-02", instance: "commerce-stack-nginx-02", action: "create", jobId: "j2" }] } };
      },
      "GET /api/v1/jobs/j1": () => { done = true; return { json: job("j1", "succeeded") }; },
      "GET /api/v1/jobs/j2": () => ({ json: job("j2", "succeeded") }),
      "GET /api/v1/applications/commerce-stack/status": () => ({ json: { source: { provider: "incus", observedAt: now }, items: done ? [
        { module: "nginx-01", instance: "commerce-stack-nginx-01", project: "apelora", status: "Running", location: "node-1" },
        { module: "nginx-01", instance: "commerce-stack-nginx-01-r2", project: "apelora", status: "Stopped", location: "node-2" },
        { module: "nginx-02", instance: "commerce-stack-nginx-02", project: "apelora", status: "Running", location: "node-2", health: { health: "unhealthy", since: now, detail: "port check failing" } },
      ] : [], services: done ? [{ module: "nginx-01", project: "apelora", address: "198.18.100.1" }] : [] } }),
    });
    await open(page);
    // Two replicas of nginx-01: nginx is stateless, so the Properties window offers replicas.
    await openProps(page, "nginx-01");
    await expect(page.getByTestId("availability")).toContainText("recreated automatically");
    await page.getByTestId("replicas").fill("2");
    await page.getByTestId("replicas").press("Enter");
    await page.getByTestId("internet-access").selectOption("web");
    await page.keyboard.press("Escape");
    // Saving is debounced: take the reference copy once the edit is stored.
    await expect.poll(() => page.evaluate(() => localStorage.getItem("apelora.studio.v1") ?? "")).toContain("replicas");
    await openDeploy(page);
    const dialog = page.getByTestId("deploy-dialog");
    await expect(dialog.getByTestId("deploy-app")).toHaveValue("commerce-stack");
    await expect(dialog.getByTestId("deploy-deployable")).toHaveCount(2);
    await expect(dialog.getByTestId("deploy-design-only")).toHaveCount(4);
    await expect(dialog.getByTestId("deploy-design-only").first()).toContainText("Design only");
    await expect(dialog.getByTestId("deploy-replicas")).toHaveText("× 2 replicas");
    await expect(dialog.getByTestId("deploy-resources").first()).toHaveText("1 CPU · 512 MiB each");

    await dialog.getByTestId("deploy-preview").click();
    expect((planBody as { modules: { module: string; replicas: number; availability: string; internet: string }[] }).modules.map((a) => [a.module, a.replicas, a.availability, a.internet]))
      .toEqual([["nginx-01", 2, "stateless", "web"], ["nginx-02", 1, "stateless", "none"]]);
    expect((planBody as { connections: unknown[] }).connections, "only cables between deployed modules").toEqual([]);
    expect((planBody as { modules: { spec: { config: Record<string, string> } }[] }).modules[0]!.spec.config, "resources as Incus limits")
      .toEqual({ "limits.cpu": "1", "limits.memory": "512MiB" });
    await expect(dialog.getByTestId("plan-row")).toHaveCount(3);
    await expect(dialog).toContainText("single points of failure: nginx-02");
    await expect(dialog.getByTestId("deploy-apply")).toHaveText("Deploy 3 changes");
    await dialog.getByTestId("network-rules").locator("summary").click();
    await expect(dialog.getByTestId("network-rule")).toHaveCount(6);
    await expect(dialog.getByTestId("network-rules")).toContainText("nginx-01 → postgresql-01.commerce-stack.internal · postgres 5432/tcp");
    await expect(dialog.getByTestId("network-rules")).toContainText("$APELORA_SERVICE_POSTGRESQL_01");
    await expect(dialog.getByTestId("network-rules")).toContainText("everything else between the application's instances is rejected");

    await dialog.getByTestId("deploy-apply").click();
    await expect(dialog.getByTestId("deploy-job")).toHaveCount(2);
    await expect(dialog.getByTestId("deploy-job").first()).toContainText("succeeded");
    expect(deployed!.key).toBeTruthy();
    expect(deployed!.body).toMatchObject({ project: "apelora", removeMissing: false, document: { apiVersion: "fabric.apelora/v1alpha1", metadata: { name: "commerce-stack" } } });
    await dialog.getByTestId("deploy-done").click();
    // The application's canvas overlays what runs, without changing the design.
    await page.locator("[data-testid=app-card][data-app=commerce-stack]").getByTestId("app-open").click();
    await page.getByTestId("tab-designer").click();
    await page.waitForSelector("[data-testid=module-node]");

    const liveOf = (id: string) => page.locator(`[data-node-id="${id}"] [data-testid=live-status]`);
    // One of nginx-01's two replicas is down: degraded, with every replica and the service address listed.
    await expect(liveOf("nginx-01")).toHaveAttribute("data-state", "Degraded");
    await expect(liveOf("nginx-01")).toHaveAttribute("title", /commerce-stack-nginx-01-r2: Stopped on node-2[\s\S]*Service address 198\.18\.100\.1/);
    // nginx-02 runs but fails its port check: not serving.
    await expect(liveOf("nginx-02")).toHaveAttribute("data-state", "Unhealthy");
    await expect(liveOf("nginx-02")).toHaveAttribute("title", /commerce-stack-nginx-02: Running, failing its port check on node-2/);
    await expect(liveOf("postgresql-01")).toHaveAttribute("data-state", "absent");
    await expect(page.getByTestId("mock-badge")).toHaveText("Live · 2 running");
    await expect(page.getByTestId("mock-badge")).not.toContainText("Incus");
    await page.waitForTimeout(1500);
    expect(drafts, "the overlay never changes the design: no draft was saved").toBe(0);
  });

  test("a conflict blocks deploying and nothing is sent", async ({ page }) => {
    let deployCalls = 0;
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health }),
      "GET /api/v1/whoami": () => ({ json: operator }),
      "POST /api/v1/applications/commerce-stack/plan": () => ({ json: { items: [item("nginx-01", "conflict", "an instance with this name exists and was not created by Apelora"), item("nginx-02", "create")], warnings: [] } }),
      "POST /api/v1/applications/commerce-stack/deployments": () => { deployCalls += 1; return { status: 409, json: { error: { code: "plan_conflict", message: "nothing was deployed" } } }; },
    });
    await open(page);
    await openDeploy(page);
    await page.getByTestId("deploy-preview").click();
    await expect(page.getByTestId("deploy-conflicts")).toContainText("Resolve the conflicts first");
    await expect(page.getByTestId("plan-row").first()).toContainText("not created by Apelora");
    await expect(page.getByTestId("deploy-apply")).toBeDisabled();
    expect(deployCalls).toBe(0);
  });

  test("a changed CPU or memory size is shown as a resize in place and counts as a change", async ({ page }) => {
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health }),
      "GET /api/v1/whoami": () => ({ json: operator }),
      "POST /api/v1/applications/commerce-stack/plan": () => ({ json: { items: [
        { ...item("nginx-01", "resize", "limits.cpu 1 → 2, limits.memory 512MiB → 1GiB"), changes: [{ key: "limits.cpu", from: "1", to: "2" }, { key: "limits.memory", from: "512MiB", to: "1GiB" }] },
        item("nginx-02", "keep", "already running"),
      ], warnings: [] } }),
    });
    await open(page);
    await openDeploy(page);
    await page.getByTestId("deploy-preview").click();
    const rows = page.getByTestId("plan-row");
    await expect(rows).toHaveCount(2);
    await expect(rows.first()).toHaveAttribute("data-action", "resize");
    await expect(rows.first()).toContainText("Resize");
    await expect(rows.first()).toContainText("limits.cpu 1 → 2");
    await expect(page.getByTestId("deploy-apply")).toHaveText("Deploy 1 change");
  });

  test("the review shows quota use, and a deployment over quota cannot be applied", async ({ page }) => {
    let deployCalls = 0;
    const GIB = 1024 ** 3;
    const quota = { allowed: false, adding: { instances: 2, cpu: 2, memoryBytes: GIB, diskBytes: 0 }, scopes: [
      { kind: "project", id: "p", name: "Shop", dimensions: [
        { dimension: "instances", used: 9, adding: 2, limit: 10, exceeded: true },
        { dimension: "memory", used: 4 * GIB, adding: GIB, limit: 8 * GIB, exceeded: false },
      ] },
    ] };
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health }),
      "GET /api/v1/whoami": () => ({ json: operator }),
      "POST /api/v1/applications/commerce-stack/plan": () => ({ json: { items: [item("nginx-01", "create"), item("nginx-02", "create")], warnings: ["over quota, applying would be refused: project Shop: 9 instances in use, this adds 2, quota 10"], quota } }),
      "POST /api/v1/applications/commerce-stack/deployments": () => { deployCalls += 1; return { status: 409, json: { error: { code: "quota_exceeded", message: "nothing was deployed" } } }; },
    });
    await open(page);
    await openDeploy(page);
    await page.getByTestId("deploy-preview").click();
    const rows = page.getByTestId("deploy-quota-row");
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0)).toContainText("Instances of project Shop");
    await expect(rows.nth(0)).toContainText("11 of 10 (over quota)");
    await expect(rows.nth(1)).toContainText("5 GiB of 8 GiB");
    await expect(page.getByTestId("deploy-over-quota")).toContainText("Over quota: nothing is deployed.");
    await expect(page.getByTestId("deploy-apply")).toBeDisabled();
    expect(deployCalls).toBe(0);
  });

  test("asks to sign in again when the control plane stops accepting the token, and keeps it for the tab only", async ({ page }) => {
    const good = "apl_" + "c".repeat(64);
    let expired = false;
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health }),
      // The list signs in with the saved token; the deploy review then finds it expired.
      "GET /api/v1/whoami": (req) => (!expired || req.headers()["authorization"] === `Bearer ${good}` ? { json: operator } : { status: 401, json: { error: { code: "unauthenticated", message: "an API token is required" } } }),
    });
    await open(page);
    await openDeploy(page, { stopAt: "list" });
    expired = true;
    await page.locator("[data-testid=app-card][data-app=commerce-stack]").getByTestId("action-deploy").click();
    await page.getByTestId("deploy-version-continue").click();
    await expect(page.getByTestId("deploy-sign-in")).toContainText("an API token is required");
    await page.getByTestId("deploy-token").fill(good);
    await page.getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByTestId("deploy-preview")).toBeVisible();
    expect(await page.evaluate(() => [sessionStorage.getItem("apelora.apiToken"), localStorage.getItem("apelora.apiToken")])).toEqual([good, null]);
  });
});
