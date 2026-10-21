import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page, type Request, type Route } from "@playwright/test";

// Operations core against a stubbed control plane: job detail with its attempts and links,
// retry and cancel as the server allows them, the instance page with restart and logs, and
// the phone layout. The server decides what is allowed; the page shows its reasons.

const now = "2026-10-21T10:00:00Z";
const later = "2026-10-21T10:00:42Z";
const health = { status: "ok", service: "control-plane", version: "0.1.0", apiVersion: "v1", provider: "mock", auth: "tokens", incus: { connected: false } };
const operator = { name: "alex", role: "operator", permissions: ["read-inventory", "design-applications", "deploy"], deployProjects: ["apelora"] };
const cluster = { source: { provider: "mock", observedAt: now }, server: { clustered: true, serverVersion: null }, members: [] };

const yes = { allowed: true, reason: null };
const no = (reason: string) => ({ allowed: false, reason });
const J1 = "11111111-1111-4111-8111-111111111111";
const J2 = "22222222-2222-4222-8222-222222222222";
const J3 = "33333333-3333-4333-8333-333333333333";
const REQ = "3f2b7c1e-5d4a-4e8f-9a61-0c2d8e7b4a15";

const job = (id: string, kind: string, status: string, extra: Record<string, unknown> = {}) => ({
  id, kind, status, attempts: 1, maxAttempts: 5, lastError: null, incusOperation: null, incusOperations: [], requestId: null,
  request: { project: "apelora", name: "web-01" }, createdAt: now, updatedAt: later, queuedBy: "user:alice", retryOf: null, retriedBy: null,
  cancelRequestedAt: null, cancelRequestedBy: null,
  actions: { retry: status === "failed" ? yes : no("Only failed jobs can be retried."), cancel: status === "pending" || status === "running" ? yes : no("Only waiting or running jobs can be cancelled.") },
  ...extra,
});

type Reply = { status?: number; json?: unknown; delay?: number };
type Handler = Reply | ((req: Request) => Reply);
/** Answers by method and path (query ignored); unstubbed calls fail. Real servers take a moment. */
async function stub(page: Page, routes: Record<string, Handler>) {
  await page.route("**/api/v1/**", async (route: Route) => {
    const req = route.request();
    const h = routes[`${req.method()} ${new URL(req.url()).pathname}`];
    if (!h) return route.fulfill({ status: 404, json: { error: { code: "not_found", message: `no stub for ${req.method()} ${new URL(req.url()).pathname}` } } });
    const r = typeof h === "function" ? h(req) : h;
    await new Promise((res) => setTimeout(res, r.delay ?? 60));
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

test("operations: retry and cancel where the server allows them, each row opens its job", async ({ page }) => {
  let retried = false;
  let cancelled = false;
  const sent: { path: string; key?: string }[] = [];
  await stub(page, {
    ...base,
    "GET /api/v1/jobs": () => ({ json: { items: [
      job(J3, "instance.stop", cancelled ? "cancelled" : "pending"),
      job(J2, "instance.start", "failed", { lastError: "Incus said no", retriedBy: retried ? J3 : null, actions: { retry: retried ? no(`Retried already by job ${J3}.`) : yes, cancel: no("Only waiting or running jobs can be cancelled.") } }),
      job(J1, "instance.create", "failed", { actions: { retry: no("You may not do this (instance.manage)."), cancel: no("Only waiting or running jobs can be cancelled.") } }),
    ] } }),
    "POST /api/v1/jobs/22222222-2222-4222-8222-222222222222/retry": (req) => { retried = true; sent.push({ path: "retry", key: req.headers()["idempotency-key"] }); return { status: 202, json: { jobId: J3, created: true } }; },
    "POST /api/v1/jobs/33333333-3333-4333-8333-333333333333/cancel": () => { cancelled = true; sent.push({ path: "cancel" }); return { json: { outcome: "cancelled", job: job(J3, "instance.stop", "cancelled") } }; },
  });
  await page.goto("/#/operations");
  const rows = page.getByTestId("job-row");
  await expect(rows).toHaveCount(3);
  // Only what the caller may do is offered.
  await expect(rows.nth(2).getByTestId("job-retry")).toHaveCount(0);
  await expect(rows.nth(0).getByTestId("job-retry")).toHaveCount(0);
  await rows.nth(1).getByTestId("job-retry").click();
  await expect.poll(() => sent.map((s) => s.path)).toEqual(["retry"]);
  expect(sent[0]!.key).toBeTruthy();
  await expect(rows.nth(1).getByTestId("job-retry")).toHaveCount(0);

  // Cancelling asks first; keeping the job is the default.
  await rows.nth(0).getByTestId("job-cancel").click();
  const dialog = page.locator("dialog[open]");
  await expect(dialog).toContainText("It has not started, so nothing in Incus changes.");
  await expect(dialog.getByRole("button", { name: "Keep it" })).toBeFocused();
  await dialog.getByRole("button", { name: "Cancel job" }).click();
  await expect(rows.nth(0)).toContainText("Cancelled");
  expect(sent.map((s) => s.path)).toEqual(["retry", "cancel"]);

  await rows.nth(1).getByTestId("job-link").click();
  await expect(page).toHaveURL(new RegExp(`#/operations/${J2}$`));
});

test("job detail: state in words, attempts with their Incus operations, links, and a retry that opens the new job", async ({ page }) => {
  let retried = false;
  const failed = job(J2, "instance.stop", "failed", {
    attempts: 2, maxAttempts: 2, requestId: REQ, lastError: "Incus operation 0b1e failed: stop timed out",
    incusOperations: ["0b1e7a2c-0000-4000-8000-000000000001", "0b1e7a2c-0000-4000-8000-000000000002"],
  });
  const detail = (j: unknown, extra: Record<string, unknown> = {}) => ({
    job: j, application: "shop", target: "apelora/web-01", startedAt: now, finishedAt: later, safePoints: "Before each change it asks Incus for.",
    attempts: [
      { attempt: 1, worker: "worker-1", startedAt: now, endedAt: "2026-10-21T10:00:20Z", outcome: "lost", error: null, incusOperations: ["0b1e7a2c-0000-4000-8000-000000000001"] },
      { attempt: 2, worker: "worker-2", startedAt: "2026-10-21T10:00:21Z", endedAt: later, outcome: "failed", error: "stop timed out", incusOperations: ["0b1e7a2c-0000-4000-8000-000000000002"] },
    ],
    linked: [{ id: J1, kind: "application.operation", status: "failed", relation: "same-request", target: "shop", createdAt: now }],
    ...extra,
  });
  await stub(page, {
    ...base,
    "GET /api/v1/jobs/22222222-2222-4222-8222-222222222222/detail": () => ({ json: detail({ ...failed, retriedBy: retried ? J3 : null, actions: { retry: retried ? no(`Retried already by job ${J3}.`) : yes, cancel: no("Only waiting or running jobs can be cancelled.") } }) }),
    "POST /api/v1/jobs/22222222-2222-4222-8222-222222222222/retry": () => { retried = true; return { status: 202, json: { jobId: J3, created: true } }; },
    "GET /api/v1/jobs/33333333-3333-4333-8333-333333333333/detail": () => ({ json: detail(job(J3, "instance.stop", "running", { retryOf: J2, cancelRequestedAt: later, cancelRequestedBy: "user:alice", actions: { retry: no("Only failed jobs can be retried."), cancel: no("Cancellation was requested by user:alice; it stops at the next safe point.") } }), { attempts: [], linked: [{ id: J2, kind: "instance.stop", status: "failed", relation: "retry-of", target: "apelora/web-01", createdAt: now }] }) }),
  });
  await page.goto(`/#/operations/${J2}`);
  await expect(page.getByRole("navigation", { name: "Console" }).getByRole("link", { name: "Operations" })).toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Stop instance apelora/web-01");
  await expect(page.getByTestId("job-status")).toHaveText("Failed");
  await expect(page.getByTestId("job-last-error")).toContainText("stop timed out");
  await expect(page.getByTestId("job-request")).toContainText(REQ);
  await expect(page.getByTestId("attempt-row")).toHaveCount(2);
  await expect(page.getByTestId("attempt-row").nth(0)).toContainText("Worker stopped; taken over");
  await expect(page.getByTestId("attempt-row").nth(1)).toContainText("0b1e7a2c-0000-4000-8000-000000000002");
  await expect(page.getByTestId("job-operations").locator("li")).toHaveCount(2);
  await expect(page.getByTestId("linked-row")).toContainText("Same request");
  await expect(page.locator(".btn-primary")).toHaveCount(1);

  await page.getByTestId("job-retry").click();
  await expect(page).toHaveURL(new RegExp(`#/operations/${J3}$`));
  await expect(page.getByTestId("job-status")).toHaveText("Running · cancelling");
  await expect(page.getByTestId("job-cancel-note")).toContainText("Asked by user:alice");
  await expect(page.getByTestId("job-cancel-note")).toContainText("Before each change it asks Incus for.");
  await expect(page.getByTestId("job-cancel")).toHaveCount(0);
  await expect(page.getByTestId("linked-row")).toContainText("Retries");
  expect(await serious(page)).toEqual([]);
});

test("job detail: a retry the caller may not make says why; an unknown job says it is not found", async ({ page }) => {
  await stub(page, {
    ...base,
    "GET /api/v1/jobs/22222222-2222-4222-8222-222222222222/detail": { json: { job: job(J2, "instance.create", "failed", { actions: { retry: no("You may not do this (instance.manage)."), cancel: no("Only waiting or running jobs can be cancelled.") } }), application: null, target: "apelora/web-01", startedAt: null, finishedAt: later, safePoints: "Only before the instance is created.", attempts: [], linked: [] } },
    "GET /api/v1/jobs/33333333-3333-4333-8333-333333333333/detail": { status: 404, json: { error: { code: "not_found", message: `not found: job ${J3}` } } },
  });
  await page.goto(`/#/operations/${J2}`);
  await expect(page.getByTestId("job-retry")).toBeDisabled();
  await expect(page.getByTestId("job-retry-reason")).toHaveText("You may not do this (instance.manage).");
  await page.goto(`/#/operations/${J3}`);
  await page.reload();
  await expect(page.getByTestId("job-detail-error")).toContainText("not found");
});

const view = (status: string, extra: Record<string, unknown> = {}) => ({
  name: "web-01", project: "apelora", kind: "container", status, location: "mock-member-01", architecture: "x86_64", description: "", managed: true,
  owner: { application: "shop", module: "web", replica: 1 }, createdAt: now, lastUsedAt: now, startedAt: status === "Running" ? now : null, profiles: ["default"],
  image: { description: "Mock image", os: "Debian", release: "13", fingerprint: "aa11bb22cc33dd44" },
  limits: { cpu: "2", memory: "2GiB", rootDisk: "10GiB" },
  usage: status === "Running" ? { cpuNs: 42e9, memoryBytes: 268435456, memoryPeakBytes: 301989888, memoryTotalBytes: 2147483648, processes: 23, rootDiskBytes: 1073741824 } : null,
  addresses: status === "Running" ? [{ interface: "eth0", family: "inet", address: "10.0.0.10", netmask: "24", scope: "global" }] : [],
  disks: [{ device: "root", path: "/", pool: "mock-local", volume: null, size: "10GiB" }],
  snapshots: [], ...extra,
});
const instancePage = (status: string, actions: Record<string, unknown> = {}) => ({
  source: { provider: "mock", observedAt: now }, instance: view(status), application: "shop",
  jobs: [job(J1, "instance.create", "succeeded")],
  actions: { start: status === "Running" ? no("It is running.") : yes, stop: status === "Running" ? yes : no("It is not running."), restart: status === "Running" ? yes : no("It is not running; start it instead."), move: yes, delete: yes, logs: yes, ...actions },
});
const logText = (file: string, kib: number) => ({ source: { provider: "mock", observedAt: now }, file, tailKib: kib, log: { content: `Mock · ${file} of apelora/web-01\nline two\n`, size: 40, truncated: false } });

test("instance page: facts from Incus, restart through a job, logs by file and size, labelled as mock", async ({ page }) => {
  let restarted = false;
  let polls = 0;
  const logReads: string[] = [];
  const sent: unknown[] = [];
  await stub(page, {
    ...base,
    "GET /api/v1/instances": { json: { source: { provider: "mock", observedAt: now }, items: [{ name: "web-01", project: "apelora", kind: "container", status: "Running", location: "mock-member-01", architecture: "x86_64", description: "", managed: true }] } },
    "GET /api/v1/instances/apelora/web-01": () => ({ json: instancePage("Running") }),
    "POST /api/v1/instances/apelora/web-01/state": (req) => { sent.push(req.postDataJSON()); return { status: 202, json: { jobId: J2, created: true } }; },
    "GET /api/v1/jobs/22222222-2222-4222-8222-222222222222": () => { polls += 1; if (polls > 1) restarted = true; return { json: job(J2, "instance.restart", restarted ? "succeeded" : "running") }; },
    "GET /api/v1/instances/apelora/web-01/logs": { json: { source: { provider: "mock", observedAt: now }, files: ["lxc.log"] } },
    "GET /api/v1/instances/apelora/web-01/console-log": (req) => { logReads.push(`console ${new URL(req.url()).searchParams.get("tailKib")}`); return { json: logText("console", 64) }; },
    "GET /api/v1/instances/apelora/web-01/logs/lxc.log": (req) => { const kib = Number(new URL(req.url()).searchParams.get("tailKib")); logReads.push(`lxc.log ${kib}`); return { json: logText("lxc.log", kib) }; },
  });
  await page.goto("/#/instances");
  await page.getByTestId("instance-link").click();
  await expect(page).toHaveURL(/#\/instances\/apelora\/web-01$/);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("web-01");
  await expect(page.getByTestId("instance-status")).toHaveText("Running");
  await expect(page.getByTestId("source-note")).toHaveText("Mock data · not a real cluster");
  await expect(page.locator("#instance-overview + dl")).toContainText("mock-member-01");
  await expect(page.locator("#instance-resources + dl")).toContainText("256.0 MiB used · limit 2GiB");
  await expect(page.getByTestId("instance-start")).toHaveCount(0);
  await expect(page.locator(".btn-primary")).toHaveCount(1);
  await expect(page.getByTestId("instance-restart")).toHaveClass(/btn-primary/);
  await expect(page.getByTestId("instance-job-row")).toContainText("Create instance");

  await page.getByTestId("instance-restart").click();
  const dialog = page.locator("dialog[open]");
  await expect(dialog.getByRole("button", { name: "Keep it running" })).toBeFocused();
  await dialog.getByRole("button", { name: "Restart instance" }).click();
  await expect.poll(() => sent).toEqual([{ action: "restart" }]);
  await expect.poll(() => restarted, { timeout: 8000 }).toBe(true);

  // Logs: the console first, then a file Incus lists, then more of it.
  await expect(page.getByTestId("log-text")).toContainText("Mock · console");
  await expect(page.getByTestId("log-mock")).toHaveText("Mock · not read from Incus");
  await page.getByTestId("log-file").selectOption("lxc.log");
  await expect(page.getByTestId("log-text")).toContainText("Mock · lxc.log");
  await page.getByTestId("log-tail").selectOption("256");
  await expect.poll(() => logReads.at(-1)).toBe("lxc.log 256");
  await expect(page.getByTestId("log-size")).toContainText("All of it");
  expect(logReads[0]).toBe("console 64");
});

test("instance page: the terminal is offered to people who hold instance.console, and only to them", async ({ page }) => {
  const instanceRoute = { "GET /api/v1/instances/apelora/web-01": () => ({ json: instancePage("Running") }) };
  await stub(page, { ...base, ...instanceRoute });
  await page.goto("/#/instances/apelora/web-01");
  await expect(page.getByTestId("instance-restart")).toBeVisible();
  await expect(page.getByTestId("terminal-open")).toHaveCount(0);
  await page.unrouteAll({ behavior: "ignoreErrors" });
  await stub(page, { ...base, ...instanceRoute, "GET /api/v1/whoami": { json: { ...operator, capabilities: ["instance.console"] } } });
  await page.reload();
  await expect(page.getByTestId("terminal-open")).toBeVisible();
  await expect(page.locator(".btn-primary")).toHaveCount(1);
});

test("instance page: someone who may not change it or read its logs is told why; a missing one is not found", async ({ page }) => {
  await stub(page, {
    ...base,
    "GET /api/v1/instances/apelora/web-01": { json: instancePage("Stopped", {
      start: no("You may not change instances in this project (instance.manage)."), stop: no("You may not change instances in this project (instance.manage)."),
      restart: no("You may not change instances in this project (instance.manage)."), move: no("You may not change instances in this project (instance.manage)."),
      delete: no("You may not change instances in this project (instance.manage)."),
      logs: no("Reading logs needs application.logs.read on its application or instance.manage on its environment."),
    }) },
    "GET /api/v1/instances/apelora/gone": { status: 404, json: { error: { code: "not_found", message: "not found: instance apelora/gone" } } },
  });
  await page.goto("/#/instances/apelora/web-01");
  await expect(page.getByTestId("instance-start")).toBeDisabled();
  await expect(page.getByTestId("instance-actions-reason")).toHaveText("You may not change instances in this project (instance.manage).");
  await expect(page.getByTestId("logs-not-allowed")).toContainText("application.logs.read");
  await expect(page.getByTestId("instance-detail")).toContainText("Incus reports use only while it runs.");
  expect(await serious(page)).toEqual([]);
  await page.goto("/#/instances/apelora/gone");
  await page.reload();
  await expect(page.getByTestId("instance-detail-error")).toContainText("Not found: instance apelora/gone");
});

test.describe("phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  test("instance and job pages fit at 390 px with touch-sized controls and no serious accessibility problems", async ({ page }) => {
    await stub(page, {
      ...base,
      "GET /api/v1/instances/apelora/web-01": { json: instancePage("Running") },
      "GET /api/v1/instances/apelora/web-01/logs": { json: { source: { provider: "mock", observedAt: now }, files: ["lxc.log"] } },
      "GET /api/v1/instances/apelora/web-01/console-log": { json: logText("console", 64) },
      "GET /api/v1/jobs/22222222-2222-4222-8222-222222222222/detail": { json: { job: job(J2, "instance.stop", "failed", { requestId: REQ, lastError: "stop timed out", incusOperations: ["0b1e7a2c-0000-4000-8000-000000000002"] }), application: "shop", target: "apelora/web-01", startedAt: now, finishedAt: later, safePoints: "Before each change it asks Incus for.", attempts: [{ attempt: 1, worker: "worker-1", startedAt: now, endedAt: later, outcome: "failed", error: "stop timed out", incusOperations: ["0b1e7a2c-0000-4000-8000-000000000002"] }], linked: [] } },
    });
    await page.goto("/#/instances/apelora/web-01");
    await expect(page.getByTestId("log-text")).toBeVisible();
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    const back = (await page.getByRole("navigation", { name: "Breadcrumb" }).getByRole("link", { name: "Instances" }).boundingBox())!;
    expect(back.height).toBeGreaterThanOrEqual(44);
    for (const id of ["instance-restart", "instance-stop", "instance-move", "instance-delete"]) {
      const box = (await page.getByTestId(id).boundingBox())!;
      expect(box.height, id).toBeGreaterThanOrEqual(44);
      expect(box.x + box.width, id).toBeLessThanOrEqual(390);
    }
    expect(await serious(page)).toEqual([]);

    await page.goto(`/#/operations/${J2}`);
    await page.reload();
    await expect(page.getByTestId("attempt-row")).toHaveCount(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    const retry = (await page.getByTestId("job-retry").boundingBox())!;
    expect(retry.height).toBeGreaterThanOrEqual(44);
    expect(await serious(page)).toEqual([]);
  });
});
