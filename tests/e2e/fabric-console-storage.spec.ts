import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page, type Request, type Route } from "@playwright/test";

// Storage and Networks against a stubbed control plane: pool use, application volumes with
// their schedules and backups, back up now, restore from a backup (the server's dry run, the
// volume's name typed, the job followed), schedules, orphans, and per-application networks with
// drift. What a caller is offered follows their permissions; the server decides.

const now = "2026-10-21T10:00:00Z";
const GIB = 1024 ** 3;
const src = { provider: "incus", observedAt: now };
const health = { status: "ok", service: "control-plane", version: "0.1.0", apiVersion: "v1", provider: "incus", auth: "tokens", incus: { connected: true } };
const cluster = { source: src, server: { clustered: true, serverVersion: "7.0.1" }, members: [] };

const person = (capabilities: string[]) => ({
  name: "alex", id: "0b9e1a3c-2f4d-4c6e-8a1b-3d5f7e9a1c2b", kind: "user", role: null, permissions: [], deployProjects: [],
  capabilities, platformPermissions: capabilities, roles: [],
});
const ADMIN = person(["cluster.view", "application.view", "application.operate", "application.deploy"]);
const VIEWER = person(["application.view"]);

const schedule = (everyMinutes: number, keep: number, error: string | null = null) => ({ everyMinutes, keep, lastRunAt: now, error });
const volume = (extra: Record<string, unknown> = {}) => ({
  application: "shop", module: "db-01", instance: "shop-db-01", name: "data", incusName: "shop-db-01-data", pool: "local", member: "node-1",
  sizeMib: 10240, class: "data-local", state: "retained", usedBy: [], schedules: { snapshots: schedule(60, 24), backups: null },
  backups: 2, lastBackupAt: now, ...extra,
});
const backup = (name: string, extra: Record<string, unknown> = {}) => ({
  name, pool: "backup", member: "node-2", sizeMib: 10240, takenAt: now, scheduled: false, beforeRestore: null, present: true, ...extra,
});
const BACKUP = "shop-db-01-data-bk-20261021100000";
const plan = (extra: Record<string, unknown> = {}) => ({
  volume: "shop-db-01-data", pool: "local", member: "node-1", sizeMib: 10240, backup: BACKUP, backupPool: "backup", backupMember: "node-2",
  backupTakenAt: now, snapshotsDeleted: ["snap-20261020-090000"], usedBy: ["shop-db-01"], running: [], safetyBackupPool: "backup", blocked: [],
  steps: [
    "Copy the volume as it is now into pool backup (a safety backup, listed with the others)",
    `Copy backup ${BACKUP} over volume shop-db-01-data on node-1: its data becomes the backup's from ${now}`,
    "Incus deletes the volume's 1 snapshot while copying",
    "Leave shop-db-01 stopped: start it again once the data is checked",
  ],
  ...extra,
});
const job = (id: string, kind: string, status: string) => ({
  id, kind, status, attempts: 1, maxAttempts: 5, lastError: null, incusOperation: null, incusOperations: [], requestId: null, request: {},
  createdAt: now, updatedAt: now, queuedBy: "user:x", retryOf: null, retriedBy: null, cancelRequestedAt: null, cancelRequestedBy: null,
  actions: { retry: { allowed: false, reason: "x" }, cancel: { allowed: false, reason: "x" } },
});
const rule = (source: string, destination: string, port: string, state: string) => ({ action: "allow", source, destination, protocol: "tcp", port, description: "", state });

type Reply = { status?: number; json?: unknown; delay?: number };
type Handler = Reply | ((req: Request) => Reply);
async function stub(page: Page, routes: Record<string, Handler>) {
  await page.route("**/api/v1/**", async (route: Route) => {
    const req = route.request();
    const path = new URL(req.url()).pathname;
    const h = routes[`${req.method()} ${path}`];
    if (!h) return route.fulfill({ status: 404, json: { error: { code: "not_found", message: `no stub for ${req.method()} ${path}` } } });
    const r = typeof h === "function" ? h(req) : h;
    await new Promise((res) => setTimeout(res, r.delay ?? 60));
    return r.json === undefined ? route.fulfill({ status: r.status ?? 204, body: "" }) : route.fulfill({ status: r.status ?? 200, json: r.json });
  });
}

const storageRoutes = (me: unknown, extra: Record<string, Handler> = {}): Record<string, Handler> => ({
  "GET /api/v1/health": { json: health },
  "GET /api/v1/whoami": { json: me },
  "GET /api/v1/cluster": { json: cluster },
  "GET /api/v1/storage/pools": { json: { source: src, volumePool: "local", backupPool: "backup", items: [
    { name: "local", driver: "btrfs", status: "Created", description: "", shared: false, role: "volumes", unreadable: [],
      members: [{ member: "node-1", usedBytes: 4 * GIB, totalBytes: 20 * GIB }, { member: "node-2", usedBytes: 2 * GIB, totalBytes: 20 * GIB }],
      total: { usedBytes: 6 * GIB, totalBytes: 40 * GIB } },
    { name: "shared", driver: "lvmcluster", status: "Created", description: "", shared: true, role: null, unreadable: [],
      members: [{ member: "node-1", usedBytes: 1 * GIB, totalBytes: 100 * GIB }], total: { usedBytes: 1 * GIB, totalBytes: 100 * GIB } },
  ] } },
  "GET /api/v1/storage/volumes": { json: { source: src, backupPool: "backup", incusError: null, items: [
    volume(), volume({ module: "web-01", instance: "shop-web-01", incusName: "shop-web-01-cache", name: "cache", state: "attached", usedBy: ["shop-web-01"], backups: 0, lastBackupAt: null,
      schedules: { snapshots: null, backups: schedule(1440, 7, "no backup pool is configured") } }),
  ] } },
  "GET /api/v1/storage/orphans": { json: { source: src, projects: ["apelora"], items: [
    { project: "apelora", pool: "local", name: "old-db-data", member: "node-3", sizeMib: 4096, content: "filesystem", usedBy: [], application: "legacy", backupOf: null, createdAt: now,
      reason: "Created by Apelora for legacy; no application records it now" },
  ] } },
  "GET /api/v1/applications/shop/volumes/shop-db-01-data/backups": { json: { source: src, backupPool: "backup",
    schedules: { snapshots: schedule(60, 24), backups: schedule(1440, 7) },
    items: [backup("shop-db-01-data-bk-20261020100000", { scheduled: true, takenAt: "2026-10-20T10:00:00Z" }), backup(BACKUP)] } },
  ...extra,
});

const serious = async (page: Page) => (await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze()).violations
  .filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(" ")}`);

test.beforeEach(async ({ page }) => {
  await page.route("**/api/**", (route) => route.abort());
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.addInitScript(() => sessionStorage.setItem("apelora.apiToken", "apl_test"));
});

test("storage shows pool use per server, volumes with their protection, and volumes no application uses", async ({ page }) => {
  await stub(page, storageRoutes(ADMIN));
  await page.goto("/#/storage");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Storage");
  const pools = page.getByTestId("storage-pool-row");
  await expect(pools).toHaveCount(2);
  await expect(pools.nth(0)).toContainText("Application volumes");
  await expect(pools.nth(0).getByTestId("storage-pool-member")).toHaveCount(2);
  await expect(pools.nth(0).getByTestId("storage-pool-total")).toContainText("6.0 GiB of 40.0 GiB");
  await expect(pools.nth(1)).toContainText("Shared by every server");
  await expect(pools.nth(1).getByTestId("storage-pool-member")).toHaveCount(0);

  const vols = page.getByTestId("storage-volume-row");
  await expect(vols).toHaveCount(2);
  await expect(vols.nth(0)).toContainText("Kept, not in use");
  await expect(vols.nth(0).getByTestId("storage-volume-protection")).toContainText("Snapshots every hour, keeping 24");
  await expect(vols.nth(0).getByTestId("storage-volume-protection")).toContainText("2 backups");
  await expect(vols.nth(1)).toContainText("In use by shop-web-01");
  await expect(vols.nth(1)).toContainText("Last scheduled run failed: no backup pool is configured");

  await expect(page.getByTestId("storage-orphan-row")).toHaveCount(1);
  await expect(page.getByTestId("storage-orphan-row")).toContainText("4 GiB");
  await expect(page.getByTestId("storage-orphan-row")).toContainText("no application records it now");
  await expect(page.locator(".btn-primary")).toHaveCount(0);
  expect(await serious(page)).toEqual([]);
});

test("a restore shows the server's dry run, refuses while blocked, needs the volume's name and follows its job", async ({ page }) => {
  let blocked = true;
  let polls = 0;
  const previews: string[] = [];
  const sent: { body: unknown; key?: string }[] = [];
  const J = "44444444-4444-4444-8444-444444444444";
  await stub(page, storageRoutes(ADMIN, {
    [`GET /api/v1/applications/shop/volumes/shop-db-01-data/backups/${BACKUP}/restore`]: (req) => {
      previews.push(new URL(req.url()).search);
      const safety = new URL(req.url()).searchParams.get("safetyBackup") !== "false";
      return { json: { source: src, plan: plan(blocked
        ? { running: ["shop-db-01"], blocked: ["stop shop-db-01 first: a volume is never overwritten under a running instance"] }
        : safety ? {} : { safetyBackupPool: null, steps: plan().steps.slice(1) }) } };
    },
    [`POST /api/v1/applications/shop/volumes/shop-db-01-data/backups/${BACKUP}/restore`]: (req) => {
      sent.push({ body: req.postDataJSON(), key: req.headers()["idempotency-key"] });
      return { status: 202, json: { jobId: J, created: true } };
    },
    [`GET /api/v1/jobs/${J}`]: () => { polls += 1; return { json: job(J, "volume.restore", polls > 1 ? "succeeded" : "running") }; },
  }));
  await page.goto("/#/storage");
  await page.getByRole("button", { name: "Backups of shop-db-01-data" }).click();
  const dialog = page.getByTestId("backups-dialog");
  await expect(dialog.getByTestId("backup-row")).toHaveCount(2);
  await expect(dialog.getByTestId("backup-row").nth(1)).toContainText("Scheduled");
  await expect(dialog.getByTestId("backups-schedule")).toContainText("Backups every day, keeping 7");
  await dialog.getByTestId("backup-row").nth(0).getByTestId("backup-restore").click();

  // Blocked: the server's reason, and no way to start it.
  await expect(dialog.getByTestId("restore-blocked")).toContainText("Stop shop-db-01 first");
  await dialog.getByTestId("restore-confirm-input").fill("shop-db-01-data");
  await expect(dialog.getByTestId("restore-start")).toBeDisabled();

  // Once stopped: the steps, the snapshots it deletes; the name must match exactly.
  blocked = false;
  await dialog.getByRole("button", { name: "Back to backups" }).click();
  await dialog.getByTestId("backup-row").nth(0).getByTestId("backup-restore").click();
  await expect(dialog.getByTestId("restore-steps").locator("li")).toHaveCount(4);
  await expect(dialog.getByTestId("restore-snapshots")).toContainText("snap-20261020-090000");
  await dialog.getByTestId("restore-safety").uncheck();
  await expect(dialog.getByTestId("restore-steps").locator("li")).toHaveCount(3);
  await dialog.getByTestId("restore-safety").check();
  await expect(dialog.getByTestId("restore-steps").locator("li")).toHaveCount(4);
  expect(previews.some((q) => q.includes("safetyBackup=false"))).toBe(true);
  await dialog.getByTestId("restore-confirm-input").fill("shop-db-01");
  await expect(dialog.getByTestId("restore-start")).toBeDisabled();
  await dialog.getByTestId("restore-confirm-input").fill("shop-db-01-data");
  await expect(dialog.getByTestId("restore-start")).toHaveClass(/text-error/);
  await dialog.getByTestId("restore-start").click();
  await expect(dialog.getByTestId("backups-notice")).toContainText("now holds the data from");
  expect(sent).toHaveLength(1);
  expect(sent[0]!.body).toEqual({ confirm: "shop-db-01-data", safetyBackup: true });
  expect(sent[0]!.key).toBeTruthy();
});

test("back up now follows its job, and schedules are saved as set", async ({ page }) => {
  let schedules: unknown = null;
  const J = "55555555-5555-4555-8555-555555555555";
  await stub(page, storageRoutes(ADMIN, {
    "POST /api/v1/applications/shop/volumes/shop-db-01-data/backups": { status: 202, json: { jobId: J, created: true } },
    [`GET /api/v1/jobs/${J}`]: { json: job(J, "volume.backup", "succeeded") },
    "PUT /api/v1/applications/shop/volumes/shop-db-01-data/schedules": (req) => { schedules = req.postDataJSON(); return { status: 204 }; },
  }));
  await page.goto("/#/storage");
  await page.getByRole("button", { name: "Backups of shop-db-01-data" }).click();
  const dialog = page.getByTestId("backups-dialog");
  await dialog.getByTestId("backup-now").click();
  await expect(dialog.getByTestId("backups-notice")).toHaveText("Backup made.");
  await dialog.getByRole("button", { name: "Close" }).click();

  await page.getByRole("button", { name: "Schedules of shop-db-01-data" }).click();
  const form = page.getByTestId("schedules-dialog");
  await expect(form.getByTestId("schedule-snapshots").locator("select")).toHaveValue("60");
  await form.getByTestId("schedule-backups").getByRole("checkbox").check();
  await form.getByTestId("schedule-backups").locator("select").selectOption("1440");
  await form.getByTestId("schedule-backups").getByRole("spinbutton").fill("14");
  await expect(page.locator("dialog[open] .btn-primary")).toHaveCount(1);
  await form.getByTestId("schedules-save").click();
  await expect(page.getByTestId("storage-volumes-notice")).toContainText("Schedules of shop-db-01-data saved");
  expect(schedules).toEqual({ snapshots: { everyMinutes: 60, keep: 24 }, backups: { everyMinutes: 1440, keep: 14 } });
});

test("a viewer of an application sees its volumes and backups, but no restore, backup, schedules, pools or orphans", async ({ page }) => {
  await stub(page, storageRoutes(VIEWER));
  await page.goto("/#/storage");
  await expect(page.getByTestId("storage-volume-row")).toHaveCount(2);
  await expect(page.getByTestId("storage-pools")).toHaveCount(0);
  await expect(page.getByTestId("storage-orphans")).toHaveCount(0);
  await expect(page.getByTestId("storage-volume-schedules")).toHaveCount(0);
  await page.getByRole("button", { name: "Backups of shop-db-01-data" }).click();
  const dialog = page.getByTestId("backups-dialog");
  await expect(dialog.getByTestId("backup-row")).toHaveCount(2);
  await expect(dialog.getByTestId("backup-restore")).toHaveCount(0);
  await expect(dialog.getByTestId("backup-now")).toHaveCount(0);
});

test("a section that fails keeps its error and retry; the rest of the page works", async ({ page }) => {
  let fail = true;
  await stub(page, storageRoutes(ADMIN, {
    "GET /api/v1/storage/orphans": () => (fail ? { status: 502, json: { error: { code: "incus_unreachable", message: "cannot reach the Incus server: timed out" } } }
      : { json: { source: src, projects: ["apelora"], items: [] } }),
  }));
  await page.goto("/#/storage");
  await expect(page.getByTestId("storage-orphans-error")).toContainText("cannot reach the Incus server");
  await expect(page.getByTestId("storage-volume-row")).toHaveCount(2);
  fail = false;
  await page.getByTestId("storage-orphans").getByRole("button", { name: "Try again" }).click();
  await expect(page.getByTestId("storage-orphans-empty")).toBeVisible();
});

const netApp = (drift: boolean) => ({
  application: "shop", project: "apelora", revision: 3, enforced: true, network: "shop", uplink: "incusbr0", serviceRange: "198.18.100.0/28",
  networkObserved: { kind: "ovn", status: "Created", ipv4Address: "10.99.0.1/24", routerAddress: "10.8.171.100" },
  connections: [{ from: "web-01", to: "db-01", transport: "tcp", port: 5432 }],
  published: [{ module: "web-01", transport: "tcp", port: 443 }],
  modules: [
    { module: "web-01", acl: "shop-web-01", internet: "web", aclPresent: true, addresses: ["10.99.0.11", "10.99.0.12"], drift: [],
      ingress: [rule("", "", "443", "applied")], egress: [rule("", "$shop-db-01", "5432", "applied"), rule("", "@external", "80,443", "applied")] },
    { module: "db-01", acl: "shop-db-01", internet: "none", aclPresent: true, addresses: ["10.99.0.20"],
      drift: drift ? ["1 ingress rule not in Incus", "1 ingress rule in Incus that the design does not have"] : [],
      ingress: drift ? [rule("$shop-web-01", "", "5432", "missing"), rule("", "", "22", "unexpected")] : [rule("$shop-web-01", "", "5432", "applied")], egress: [] },
  ],
  services: [{ module: "web-01", address: "198.18.100.1", ports: [{ transport: "tcp", port: 443 }], published: true, drift: [],
    backends: [{ name: "r1", address: "10.99.0.11", instance: "shop-web-01" }, { name: "r2", address: "10.99.0.12", instance: "shop-web-01-r2" }] }],
  drift: [],
});

const networkRoutes = (me: unknown, drift: boolean): Record<string, Handler> => ({
  "GET /api/v1/health": { json: health },
  "GET /api/v1/whoami": { json: me },
  "GET /api/v1/cluster": { json: cluster },
  "GET /api/v1/networks/applications": { json: { source: src, incusError: null, items: [netApp(drift)] } },
  "GET /api/v1/networks/settings": { json: { source: src, uplink: "incusbr0", serviceRange: "198.18.100.0/28", serviceAddressesUsed: 1, serviceAddressesTotal: 14,
    projects: [{ project: "apelora", found: true, restricted: true, ownNetworks: true, uplinks: ["incusbr0"], subnets: ["incusbr0:198.18.100.0/28"] }],
    uplinkObserved: { name: "incusbr0", kind: "bridge", status: "Created", ipv4Address: "10.8.171.1/24", ipv4Routes: ["198.18.100.0/28"], ovnRanges: ["10.8.171.100-10.8.171.150"] },
    uplinkNote: null, findings: [{ severity: "warn", message: "Service range 198.18.100.0/28 is not routed to uplink incusbr0 (ipv4.routes): outside clients cannot reach service addresses" }] } },
  "GET /api/v1/networks": { json: { source: src, items: [{ name: "shop", project: "apelora", kind: "ovn", managed: true, status: "Created", description: "" }] } },
});

test("networks show each application's rules next to Incus, with differences, and the settings in effect", async ({ page }) => {
  await stub(page, networkRoutes(ADMIN, true));
  await page.goto("/#/networks");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Networks");
  const row = page.getByTestId("network-application-row");
  await expect(row).toHaveCount(1);
  await expect(row.getByTestId("network-application-state")).toHaveText("2 differences");
  await row.getByTestId("network-application-toggle").click();
  const details = page.getByTestId("network-details");
  await expect(details.getByTestId("network-connections")).toContainText("web-01 may reach db-01 on tcp 5432");
  const db = details.getByTestId("network-module").nth(1);
  await expect(db.getByTestId("network-rule").nth(0)).toContainText("From web-01 on tcp 5432");
  await expect(db.getByTestId("network-rule").nth(0)).toContainText("Missing in Incus");
  await expect(db.getByTestId("network-rule").nth(1)).toContainText("Not in the design");
  await expect(db.getByTestId("network-module-drift")).toContainText("1 ingress rule not in Incus");
  await expect(details.getByTestId("network-module").nth(0).getByTestId("network-rule").nth(2)).toContainText("To Outside the application on tcp 80,443");
  await expect(details.getByTestId("network-service")).toContainText("198.18.100.1");
  await expect(details.getByTestId("network-service")).toContainText("shop-web-01-r2");
  await expect(details).toContainText("must be routed to 10.8.171.100");
  await expect(page.getByTestId("network-finding")).toContainText("not routed to uplink incusbr0");
  await expect(page.getByTestId("network-settings-list")).toContainText("1 of 14 in use");
  await expect(page.getByTestId("network-project-row")).toContainText("incusbr0:198.18.100.0/28");
  expect(await serious(page)).toEqual([]);
});

test("a viewer sees only application networks; when Incus cannot be read the design's side stays", async ({ page }) => {
  await stub(page, { ...networkRoutes(VIEWER, false),
    "GET /api/v1/networks/applications": { json: { source: src, incusError: "cannot reach the Incus server: timed out", items: [{ ...netApp(false), networkObserved: null,
      modules: netApp(false).modules.map((m) => ({ ...m, aclPresent: null, addresses: null, ingress: m.ingress.map((r) => ({ ...r, state: "unknown" })), egress: m.egress.map((r) => ({ ...r, state: "unknown" })) })) }] } } });
  await page.goto("/#/networks");
  await expect(page.getByTestId("network-settings")).toHaveCount(0);
  await expect(page.getByTestId("networks")).toHaveCount(0);
  await expect(page.getByTestId("network-incus-error")).toContainText("only the design's side is shown");
  await expect(page.getByTestId("network-application-state")).toHaveText("Not checked");
  await page.getByTestId("network-application-toggle").click();
  await expect(page.getByTestId("network-rule").first()).toContainText("Not checked");
});

test.describe("phones", () => {
  test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });

  test("storage and networks fit a phone without sideways scrolling, and pass axe", async ({ page }) => {
    await stub(page, { ...storageRoutes(ADMIN), ...networkRoutes(ADMIN, true),
      [`GET /api/v1/applications/shop/volumes/shop-db-01-data/backups/${BACKUP}/restore`]: { json: { source: src, plan: plan() } } });
    await page.goto("/#/storage");
    await expect(page.getByTestId("storage-volume-row")).toHaveCount(2);
    const fits = () => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth + 1);
    expect(await fits()).toBe(true);
    await expect(page.getByRole("button", { name: "Backups of shop-db-01-data" })).toBeInViewport();
    expect(await serious(page)).toEqual([]);
    await page.getByRole("button", { name: "Backups of shop-db-01-data" }).click();
    await page.getByTestId("backup-restore").first().click();
    await expect(page.getByTestId("restore-steps")).toBeVisible();
    expect(await fits()).toBe(true);
    await page.goto("/#/networks");
    await page.reload();
    await page.getByTestId("network-application-toggle").click();
    await expect(page.getByTestId("network-details")).toBeVisible();
    expect(await fits()).toBe(true);
    expect(await serious(page)).toEqual([]);
  });
});
