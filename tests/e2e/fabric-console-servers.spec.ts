import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page, type Route } from "@playwright/test";

import { servers as healthServers, server as healthServer } from "./health-fixtures";
import { NOW, atRiskQuorum, groups, member, members, mixedBlocked, noRecovery, recovery, settings, system } from "./servers-fixtures";

// The Servers area against a stubbed control plane: database roles and majority, members with
// their releases and capacity, groups, settings with their defaults, certificates, upgrades and
// the old addresses that now lead here.

const health = { status: "ok", service: "control-plane", version: "0.1.0", apiVersion: "v1", provider: "incus", auth: "tokens", incus: { connected: true }, checkedAt: NOW };
const ALL = ["read-inventory", "read-audit", "manage-tokens", "manage-infrastructure", "design-applications", "deploy"];
const admin = { name: "alex", role: "fabric-admin", permissions: ALL, capabilities: [], platformPermissions: [], roles: [], deployProjects: ["apelora"] };
const viewer = { ...admin, role: null, permissions: ["read-inventory"], capabilities: ["cluster.view"], platformPermissions: ["cluster.view"] };
const designer = { ...admin, role: "studio-designer", permissions: ["design-applications"] };

type Reply = { status?: number; json?: unknown; delay?: number } | "abort";
async function stub(page: Page, routes: Record<string, Reply | (() => Reply)>) {
  await page.route("**/api/**", async (route: Route) => {
    const r0 = routes[new URL(route.request().url()).pathname];
    const r = typeof r0 === "function" ? r0() : r0;
    if (!r || r === "abort") return route.abort();
    // Real servers take a moment (LESSONS-LEARNED: stubs that answer at once hide races).
    await new Promise((res) => setTimeout(res, r.delay ?? 60));
    return route.fulfill({ status: r.status ?? 200, json: r.json });
  });
}

const base = (who: unknown = admin) => ({
  "/api/v1/health": { json: health },
  "/api/v1/whoami": { json: who },
  "/api/v1/cluster/members": { json: members() },
  "/api/v1/cluster/groups": { json: groups() },
  "/api/v1/cluster/recovery": { json: noRecovery() },
  "/api/v1/health/servers": { json: healthServers("incus", [healthServer("node-1"), healthServer("node-2"), healthServer("node-3"), healthServer("node-4")]) },
});

const serious = async (page: Page) => {
  const r = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
  return r.violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(" ")}`);
};

test.beforeEach(async ({ page }) => {
  await page.route("**/api/**", (route) => route.abort());
  await page.emulateMedia({ reducedMotion: "reduce" });
});

test("the navigation is grouped and old addresses lead to their new place", async ({ page }) => {
  await stub(page, { ...base(), "/api/v1/machines": { json: { source: { provider: "incus", observedAt: NOW }, items: [] } }, "/api/v1/cluster/join-tokens": { json: { source: { provider: "incus", observedAt: NOW }, items: [] } } });
  await page.goto("/#/membership");
  await expect(page).toHaveURL(/#\/servers\/machines$/);
  const nav = page.getByRole("navigation", { name: "Console" });
  await expect(nav.getByRole("link")).toHaveText(["Overview", "Health", "Servers", "Applications", "Instances", "Operations", "Networks", "Storage", "Alerts", "Audit", "Administration"]);
  await expect(nav.getByRole("group", { name: "Workloads" }).getByRole("link")).toHaveText(["Applications", "Instances", "Operations"]);
  await expect(nav.getByRole("link", { name: "Servers" })).toHaveAttribute("aria-current", "page");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Servers");
  const tabs = page.getByRole("navigation", { name: "Servers" });
  await expect(tabs.getByRole("link", { name: "Machines and join tokens" })).toHaveAttribute("aria-current", "page");
  await expect(page.getByText("No machines yet.")).toBeVisible();

  // Keyboard: the tabs are links in reading order.
  await tabs.getByRole("link", { name: "Members" }).focus();
  await page.keyboard.press("Enter");
  await expect(page).toHaveURL(/#\/servers\/members$/);
  await expect(page.getByTestId("server-member")).toHaveCount(4);
});

test("members: database roles, the majority, releases, capacity and groups", async ({ page }) => {
  await stub(page, base());
  await page.goto("/#/servers");
  const quorum = page.getByTestId("quorum");
  await expect(quorum.getByTestId("quorum-state")).toHaveText("Majority safe");
  await expect(quorum).toContainText("Voters answering3 of 3");
  await expect(quorum).toContainText("Voters it can lose1");
  await expect(quorum).toContainText("Leadernode-1");
  // When all is well the figures carry it; the sentence appears when something is wrong.
  await expect(quorum.getByTestId("quorum-summary")).toHaveCount(0);

  const rows = page.getByTestId("server-member");
  await expect(rows).toHaveCount(4);
  await expect(rows.nth(0).getByTestId("member-leader")).toHaveText("Leader");
  await expect(rows.nth(0)).toContainText("Voter");
  await expect(rows.nth(0)).toContainText("7.0.1");
  await expect(rows.nth(0)).toContainText("Memory 3.0 GiB of 8.0 GiB");
  await expect(rows.nth(2)).toContainText("rack-b");
  await expect(rows.nth(3)).toContainText("Stand-by");
  await expect(rows.nth(3)).toContainText("event hub");
  await expect(rows.nth(3)).toContainText("Held out of placement");
  await expect(page.getByTestId("source-note").first()).toContainText("From Incus · observed");
  await expect(page.getByTestId("cluster-group-row")).toHaveCount(2);
  await expect(page.getByTestId("cluster-group-row").nth(1)).toContainText("None");
  // Administrators get the maintenance actions; one primary action at most on the view.
  await expect(page.getByTestId("evacuate-member")).toHaveCount(4);
  await expect(page.locator(".btn-primary:visible")).toHaveCount(0);
  expect(await serious(page)).toEqual([]);
});

test("members: one voter down puts the majority at risk, with the server's reason and next step", async ({ page }) => {
  const report = members({ quorum: atRiskQuorum });
  report.members[2] = member("node-3", { status: "Offline", message: "No heartbeat for 40s", upForDatabase: false, incusVersion: null, versionError: "no answer within 5 s" });
  await stub(page, { ...base(), "/api/v1/cluster/members": { json: report } });
  await page.goto("/#/servers/members");
  await expect(page.getByTestId("quorum-state")).toHaveText("One loss from stopping");
  await expect(page.getByTestId("quorum-summary")).toContainText("Losing one more voter stops every change");
  await expect(page.getByTestId("quorum-next-step")).toContainText("Next step: Incus promotes a stand-by");
  await expect(page.getByTestId("quorum-warning")).toContainText("Every voter is in failure domain rack-a");
  const offline = page.getByTestId("server-member").nth(2);
  await expect(offline.getByTestId("member-status")).toHaveText("Offline");
  await expect(offline).toContainText("Unknown");
  await expect(offline.getByTestId("evacuate-member")).toBeDisabled();
});

test("members: loading, then an error with its retry; a standalone server says so", async ({ page }) => {
  let fail = true;
  await stub(page, {
    ...base(),
    "/api/v1/cluster/members": () => (fail
      ? { status: 502, json: { error: { code: "incus_unreachable", message: "cannot reach the Incus server: connection refused" } }, delay: 400 }
      : { json: members({ clustered: false, members: [], quorum: { ...atRiskQuorum, state: "not-clustered", voters: 0, votersUp: 0, needed: 0, standbys: 0, standbysUp: 0, canLose: 0, leader: null, summary: "A single Incus server: there is no cluster database majority to keep.", nextStep: null, warnings: [] } }) }),
    "/api/v1/cluster/groups": { json: { source: { provider: "incus", observedAt: NOW }, items: [] } },
  });
  await page.goto("/#/servers/members");
  await expect(page.getByTestId("server-members-loading")).toBeVisible();
  await expect(page.getByTestId("server-members-error")).toHaveText("cannot reach the Incus server: connection refused");
  fail = false;
  await page.getByTestId("server-members").getByRole("button", { name: "Try again" }).click();
  await expect(page.getByTestId("quorum-state")).toHaveText("Single server");
  await expect(page.getByTestId("quorum-summary")).toContainText("A single Incus server");
  await expect(page.getByText("This Incus server is not clustered.")).toBeVisible();
  await expect(page.getByText("No cluster groups.")).toBeVisible();
});

test("a viewer sees the cluster but no machines tab and no maintenance actions", async ({ page }) => {
  await stub(page, base(viewer));
  await page.goto("/#/servers/machines");
  const tabs = page.getByRole("navigation", { name: "Servers" });
  await expect(tabs.getByRole("link")).toHaveText(["Members", "Cluster settings", "Certificates", "Upgrades", "Preflight"]);
  await expect(page.getByTestId("server-member")).toHaveCount(4);
  await expect(page.getByTestId("evacuate-member")).toHaveCount(0);
  await expect(page.getByRole("columnheader", { name: "Actions" })).toHaveCount(0);
});

test("callers without cluster.view are not offered Servers", async ({ page }) => {
  await stub(page, { ...base(designer), "/api/v1/cluster": { json: { source: { provider: "incus", observedAt: NOW }, server: { clustered: true, serverVersion: "7.0.1" }, members: [] } } });
  await page.goto("/#/servers/members");
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Overview");
  await expect(page.getByRole("navigation", { name: "Console" }).getByRole("link", { name: "Servers" })).toHaveCount(0);
});

test("cluster settings: set values stand out, defaults are muted, and a missing admin identity is explained", async ({ page }) => {
  let configured = true;
  await stub(page, {
    ...base(),
    "/api/v1/cluster/settings": () => (configured ? { json: settings() } : { status: 503, json: { error: { code: "cluster_admin_not_configured", message: "cluster administration needs an unrestricted Incus identity (APELORA_INCUS_ADMIN_CLIENT_CERT and APELORA_INCUS_ADMIN_CLIENT_KEY)" } } }),
  });
  await page.goto("/#/servers/settings");
  const rows = page.getByTestId("cluster-setting");
  await expect(rows).toHaveCount(4);
  await expect(rows.nth(0)).toContainText("Default · 20");
  await expect(rows.nth(0)).toHaveAttribute("data-default", "true");
  await expect(rows.nth(2)).toContainText("30M");
  await expect(rows.nth(2)).toContainText("default 3H");
  await expect(rows.nth(2)).toHaveAttribute("data-default", "false");
  await expect(rows.nth(3)).toContainText("Default (not reported)");
  await expect(page.getByTestId("cluster-settings")).toContainText("Read-only here");
  expect(await serious(page)).toEqual([]);
  configured = false;
  await page.reload();
  await expect(page.getByTestId("cluster-settings-error")).toContainText("needs an unrestricted Incus identity");
});

test("certificates: days left, an expiring one pointing to the runbook, and none with mock data", async ({ page }) => {
  let certs = [
    { certificate: "deploy", expiresAt: "2026-11-02T00:00:00Z", daysLeft: 12, state: "expiring" },
    { certificate: "admin", expiresAt: "2027-01-18T00:00:00Z", daysLeft: 89, state: "ok" },
    { certificate: "server", expiresAt: "2036-10-18T00:00:00Z", daysLeft: 3650, state: "ok" },
  ];
  await stub(page, { ...base(), "/api/v1/system": () => ({ json: system(certs) }) });
  await page.goto("/#/servers/certificates");
  const rows = page.getByTestId("certificate-row");
  await expect(rows).toHaveCount(3);
  await expect(rows.nth(0)).toContainText("Deploy identity");
  await expect(rows.nth(0)).toContainText("12 days left: rotate it soon (docs/runbooks/certificates.md)");
  await expect(rows.nth(2)).toContainText("3650 days left");
  await expect(page.getByTestId("certificates-due")).toBeVisible();
  certs = [];
  await page.reload();
  await expect(page.getByText("No certificates: the control plane uses mock data")).toBeVisible();
});

test("upgrades: skew against the pin, what blocks the next member, and the rolling procedure", async ({ page }) => {
  let versions = members().versions;
  await stub(page, { ...base(), "/api/v1/cluster/members": () => ({ json: members({ versions }) }) });
  await page.goto("/#/servers/upgrades");
  await expect(page.getByTestId("upgrade-state")).toHaveText("Upgrade due");
  await expect(page.getByTestId("upgrade-summary")).toContainText("Apelora is pinned to 7.0.2");
  await expect(page.getByTestId("version-group")).toHaveCount(1);
  await expect(page.getByTestId("version-group")).toContainText("Older than the pin");
  await expect(page.getByTestId("upgrade-ready")).toBeVisible();
  await expect(page.getByTestId("rolling-procedure").getByRole("listitem")).toHaveCount(5);
  await expect(page.getByTestId("rolling-procedure")).toContainText("docs/runbooks/incus-rolling-upgrade.md");
  expect(await serious(page)).toEqual([]);

  versions = mixedBlocked;
  await page.reload();
  await expect(page.getByTestId("upgrade-state")).toHaveText("Mixed releases");
  await expect(page.getByTestId("version-group")).toHaveCount(2);
  await expect(page.getByTestId("upgrade-blocker")).toContainText("Offline: node-3.");
  await expect(page.getByTestId("upgrade-note")).toContainText("In maintenance: node-1");
  await expect(page.getByTestId("upgrade-ready")).toHaveCount(0);
});

test("a failed server: fencing state, what restarts where, and confirming it is off", async ({ page }) => {
  let fenced = false;
  let confirmed: unknown = null;
  const offline = members({ members: [member("node-1", { leader: true }), member("node-2"), member("node-3", { status: "Offline", message: "No heartbeat for 2m10s" })], quorum: atRiskQuorum });
  await page.route("**/api/**", async (route: Route) => {
    const req = route.request();
    const path = new URL(req.url()).pathname;
    await new Promise((res) => setTimeout(res, 60));
    if (path === "/api/v1/cluster/members/node-3/fencing" && req.method() === "POST") {
      confirmed = req.postDataJSON();
      fenced = true;
      return route.fulfill({ json: recovery(true).items[0] });
    }
    const replies: Record<string, unknown> = {
      "/api/v1/health": health, "/api/v1/whoami": admin, "/api/v1/cluster/members": offline, "/api/v1/cluster/groups": groups(),
      "/api/v1/cluster/recovery": recovery(fenced),
      "/api/v1/health/servers": healthServers("incus", [healthServer("node-1"), healthServer("node-2")]),
    };
    return path in replies ? route.fulfill({ json: replies[path] }) : route.abort();
  });
  await page.goto("/#/servers/members");
  const failed = page.getByTestId("failed-server");
  await expect(failed).toHaveCount(1);
  await expect(failed.getByRole("heading", { name: "node-3 is offline" })).toBeVisible();
  await expect(failed.getByTestId("fencing-state")).toHaveText("Waiting for fencing");
  await expect(failed.getByTestId("fencing-reason")).toContainText("Its storage lease stopped being renewed (FAIL)");
  await expect(failed.getByTestId("fencing-next-step")).toContainText("Apelora restarts its modules once the lease is dead");
  const items = failed.getByTestId("recovery-item");
  await expect(items).toHaveCount(2);
  await expect(items.nth(0)).toContainText("Restarts on node-2 once fenced");
  await expect(items.nth(1)).toContainText("Waits for its server");
  await expect(items.nth(1)).toContainText("Volume data is on member-local pool local");
  expect(await serious(page)).toEqual([]);

  // Confirming needs the name typed; Cancel leaves everything as it was.
  await failed.getByTestId("confirm-powered-off").click();
  const dialog = page.locator("dialog[open]");
  await expect(dialog).toContainText("two copies would write the same disks");
  const go = dialog.getByRole("button", { name: "Confirm powered off" });
  await expect(go).toBeDisabled();
  await dialog.getByRole("textbox").fill("node-3");
  await go.click();
  await expect.poll(() => confirmed).toEqual({ confirm: "node-3", poweredOff: true });
  await expect(page.getByTestId("recovery-notice")).toContainText("node-3 is recorded as powered off");
  await expect(failed.getByTestId("fencing-state")).toHaveText("Fenced");
  await expect(failed.getByTestId("fencing-reason")).toContainText("Sanlock on node-1 reported host 3");
  await expect(items.nth(0)).toContainText("Restarts on node-2");
  await expect(failed.getByTestId("confirm-powered-off")).toHaveCount(0);
});

test("a viewer sees failed servers without the confirm action", async ({ page }) => {
  const offline = members({ members: [member("node-1", { leader: true }), member("node-3", { status: "Offline" })] });
  await stub(page, { ...base(viewer), "/api/v1/cluster/members": { json: offline }, "/api/v1/cluster/recovery": { json: recovery(false) } });
  await page.goto("/#/servers/members");
  await expect(page.getByTestId("failed-server")).toHaveCount(1);
  await expect(page.getByTestId("confirm-powered-off")).toHaveCount(0);
});

test("preflight lives under Servers and its old address still works", async ({ page }) => {
  await stub(page, { ...base(), "/api/v1/preflight": { json: { source: { provider: "incus", observedAt: NOW }, overall: "pass", checks: [{ id: "incus-version", title: "Incus version", status: "pass", detail: "Incus 7.0.1" }] } } });
  await page.goto("/#/preflight");
  await expect(page).toHaveURL(/#\/servers\/preflight$/);
  await expect(page.getByRole("heading", { level: 1 })).toHaveText("Servers");
  await expect(page.getByRole("heading", { level: 2, name: "Preflight" })).toBeVisible();
  await expect(page.getByTestId("preflight-check")).toHaveCount(1);
});

test.describe("phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });

  test("members, tabs and navigation fit without sideways scrolling and are touch-sized", async ({ page }) => {
    await stub(page, base());
    await page.goto("/#/servers/members");
    await expect(page.getByTestId("server-member")).toHaveCount(4);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    const frame = page.getByTestId("server-members").locator("div.overflow-x-auto");
    expect(await frame.evaluate((d) => d.scrollWidth - d.clientWidth)).toBeLessThanOrEqual(0);
    for (const link of await page.getByRole("navigation", { name: "Servers" }).getByRole("link").all()) {
      const box = (await link.boundingBox())!;
      expect(box.x + box.width, await link.innerText()).toBeLessThanOrEqual(390);
      expect(box.height, await link.innerText()).toBeGreaterThanOrEqual(44);
    }
    for (const link of await page.getByRole("navigation", { name: "Console" }).getByRole("link").all()) {
      const box = (await link.boundingBox())!;
      expect(box.x + box.width, await link.innerText()).toBeLessThanOrEqual(390);
      expect(box.height, await link.innerText()).toBeGreaterThanOrEqual(44);
    }
    const evacuate = (await page.getByTestId("evacuate-member").first().boundingBox())!;
    expect(evacuate.height).toBeGreaterThanOrEqual(44);
    expect(evacuate.x + evacuate.width).toBeLessThanOrEqual(390);
    expect(await serious(page)).toEqual([]);
  });

  test("a failed server's panel fits a phone", async ({ page }) => {
    const offline = members({ members: [member("node-1", { leader: true }), member("node-3", { status: "Offline" })] });
    await stub(page, { ...base(), "/api/v1/cluster/members": { json: offline }, "/api/v1/cluster/recovery": { json: recovery(false) } });
    await page.goto("/#/servers/members");
    await expect(page.getByTestId("recovery-item")).toHaveCount(2);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    const confirm = (await page.getByTestId("confirm-powered-off").boundingBox())!;
    expect(confirm.height).toBeGreaterThanOrEqual(44);
    expect(confirm.x + confirm.width).toBeLessThanOrEqual(390);
    expect(await serious(page)).toEqual([]);
  });

  test("upgrades and settings fit a phone", async ({ page }) => {
    await stub(page, { ...base(), "/api/v1/cluster/settings": { json: settings() } });
    await page.goto("/#/servers/upgrades");
    await expect(page.getByTestId("version-group")).toHaveCount(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    await page.goto("/#/servers/settings");
    await expect(page.getByTestId("cluster-setting")).toHaveCount(4);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    expect(await serious(page)).toEqual([]);
  });
});
