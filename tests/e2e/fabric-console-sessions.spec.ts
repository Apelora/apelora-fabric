// SPDX-License-Identifier: AGPL-3.0-only
import AxeBuilder from "@axe-core/playwright";
import { expect as baseExpect, test, type Page, type WebSocketRoute } from "@playwright/test";

import { ME, auditItem, base, error, items, stub, uid, whoami, type Call, type Handler, type Result } from "./identity-fixtures";

// Terminal sessions from the Fabric Console (a second door into the Studio's brokered sessions)
// and the Sessions page, against a stubbed control plane and a stubbed websocket. These prove the
// interface: what is offered, the approval step, how a session is opened, typed into and ended,
// and how sessions are listed and ended. The broker itself is covered by the control plane's tests
// (services/control-plane/src/api/sessions_tests.rs). Evidence: MOCK VERIFIED.

// The terminal code loads on first use; on a cold dev server that compile can take a while.
const expect = baseExpect.configure({ timeout: 15_000 });

const T = "2026-10-21T10:00:00Z";
const SESSIONS = whoami(["application.view", "instance.console", "approval.request", "project.view"], { platform: [] });
const OPERATOR = whoami(["application.view", "application.operate", "audit.read", "project.view", "user.view"], { platform: [] });

const inst = (name: string, extra: Record<string, unknown> = {}) => ({ name, project: "apelora", kind: "container", status: "Running", location: "node-1", architecture: "x86_64", description: "", managed: true, owner: { application: "shop", module: "web-01" }, ...extra });
const INSTANCES = { source: { provider: "mock", observedAt: T }, items: [
  inst("shop-web-01"),
  inst("other-vm", { managed: false, owner: null }),
] };
type KindState = { allowed: boolean; reason?: string; approvalRequired?: boolean };
const access = (k: KindState, observed = true) => ({
  observed, provider: "mock", idleSecs: 900, maxSecs: 3600,
  items: observed ? [{ instance: "shop-web-01", module: "web-01", guestOs: "linux", status: "Running", kinds: [{ kind: "terminal", allowed: k.allowed, reason: k.allowed ? undefined : k.reason, approvalRequired: !!k.approvalRequired }] }] : [],
});
const TICKET = { id: uid(900), ticket: "t".repeat(64), expiresAt: T, connectPath: "/api/v1/sessions/connect", protocol: "apelora.session.v1", kind: "terminal", instance: "shop-web-01", idleSecs: 900, maxSecs: 3600, provider: "mock" };

/** A shell-like websocket: echoes what is typed, runs a line on Enter, `exit` ends it. */
function shell(sockets: { url: string; protocols: string }[]) {
  return (ws: WebSocketRoute) => {
    sockets.push({ url: ws.url(), protocols: "" });
    ws.send(Buffer.from("MOCK session for shop\r\n$ "));
    let line = "";
    ws.onMessage((m) => {
      if (typeof m === "string") return;
      for (const ch of Buffer.from(m).toString()) {
        if (ch !== "\r") { line += ch; ws.send(Buffer.from(ch)); continue; }
        ws.send(Buffer.from(`\r\nran:${line}\r\n$ `));
        line = "";
      }
    });
  };
}

async function rig(page: Page, me: unknown, extra: Record<string, Handler | Result> = {}) {
  await page.addInitScript(() => sessionStorage.setItem("apelora.apiToken", "eyJhbGciOiJFUzI1NiJ9.e30.stub"));
  const sockets: { url: string; protocols: string }[] = [];
  const headers: Record<string, string>[] = [];
  await page.routeWebSocket(/\/api\/v1\/sessions\/connect/, shell(sockets));
  const calls = await stub(page, {
    ...base(me),
    "GET /api/v1/instances": { json: INSTANCES },
    "GET /api/v1/cluster": { json: { source: { provider: "mock", observedAt: T }, server: { clustered: true, serverVersion: null }, members: [] } },
    "GET /api/v1/applications/shop/access": { json: access({ allowed: true }) },
    "POST /api/v1/applications/shop/instances/shop-web-01/sessions": { status: 201, json: TICKET },
    ...extra,
  });
  page.on("request", (r) => { if (r.url().includes("/instances/shop-web-01/sessions")) headers.push(r.headers()); });
  return { calls, sockets, headers };
}

const count = (calls: Call[], line: string) => calls.filter((c) => `${c.method} ${c.path}` === line).length;

test.describe("terminal from the Console", () => {
  test("the Instances list offers a terminal on Apelora's instances of an application, to people who may open one", async ({ page }) => {
    await rig(page, whoami(["cluster.view", "instance.console", "application.view"], { platform: [] }));
    await page.goto("/#/instances");
    const rows = page.getByTestId("instance-row");
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0).getByTestId("terminal-open")).toBeVisible();
    await expect(rows.nth(0).getByTestId("terminal-open")).toHaveAccessibleName("Terminal in shop-web-01");
    await expect(rows.nth(1).getByTestId("terminal-open")).toHaveCount(0);
    await expect(rows.nth(1)).toContainText("Not managed by Apelora");
  });

  test("without instance.console there is no terminal button", async ({ page }) => {
    await rig(page, whoami(["cluster.view", "application.view"], { platform: [] }));
    await page.goto("/#/instances");
    await expect(page.getByTestId("instance-row")).toHaveCount(2);
    await expect(page.getByTestId("terminal-open")).toHaveCount(0);
  });

  test("opening a session: the facts first, then a live terminal; typing reaches it; ending it says so", { tag: "@smoke" }, async ({ page }) => {
    const r = await rig(page, whoami(["cluster.view", "instance.console", "application.view"], { platform: [] }));
    await page.goto("/#/instances");
    await page.getByTestId("terminal-open").click();
    const dialog = page.getByTestId("session-dialog");
    await expect(dialog.getByTestId("session-prepare")).toContainText("what you type or see is not");
    await expect(dialog.getByTestId("session-prepare")).toContainText("15 minutes without typing");
    await expect(dialog.getByTestId("session-mock")).toBeVisible();
    await expect(dialog.getByTestId("session-approval")).toHaveCount(0);
    await dialog.getByTestId("session-open").click();
    await expect(dialog.getByTestId("session-live")).toBeVisible();
    await expect(dialog.getByTestId("session-terminal")).toContainText("MOCK session for shop");
    const post = r.calls.find((c) => c.method === "POST")!;
    expect(post.body).toEqual({ kind: "terminal", cols: 100, rows: 30 });
    expect(r.headers[0]?.["apelora-approval"]).toBeUndefined();
    // The ticket is never in the address of the socket.
    expect(r.sockets).toHaveLength(1);
    expect(r.sockets[0]!.url).not.toContain("ticket");
    expect(r.sockets[0]!.url).not.toContain("tttttttt");
    expect(new URL(r.sockets[0]!.url).host).toBe(new URL(page.url()).host);
    await page.keyboard.type("uname -a");
    await page.keyboard.press("Enter");
    await expect(dialog.getByTestId("session-terminal")).toContainText("ran:uname -a");
    // Escape belongs to the program in the terminal, not to the dialog.
    await page.keyboard.press("Escape");
    await expect(dialog).toBeVisible();
    await dialog.getByTestId("session-end").click();
    await expect(dialog.getByTestId("session-ended")).toContainText("You ended the session");
    await dialog.getByTestId("session-close").click();
    await expect(dialog).toHaveCount(0);
  });

  test("the control plane's reason is shown when a session is not available, and nothing can be opened", async ({ page }) => {
    const r = await rig(page, whoami(["cluster.view", "instance.console", "application.view"], { platform: [] }), {
      "GET /api/v1/applications/shop/access": { json: access({ allowed: false, reason: "the instance is stopped; start the application first" }) },
    });
    await page.goto("/#/instances");
    await page.getByTestId("terminal-open").click();
    const dialog = page.getByTestId("session-dialog");
    await expect(dialog.getByTestId("session-unavailable")).toContainText("The instance is stopped; start the application first.");
    await expect(dialog.getByTestId("session-open")).toHaveCount(0);
    expect(count(r.calls, "POST /api/v1/applications/shop/instances/shop-web-01/sessions")).toBe(0);
  });

  test("when Incus cannot be read, the dialog says the state is unknown", async ({ page }) => {
    await rig(page, whoami(["cluster.view", "instance.console", "application.view"], { platform: [] }), {
      "GET /api/v1/applications/shop/access": { json: access({ allowed: true }, false) },
    });
    await page.goto("/#/instances");
    await page.getByTestId("terminal-open").click();
    await expect(page.getByTestId("session-unavailable")).toContainText("cannot read Incus");
    await expect(page.getByTestId("session-open")).toHaveCount(0);
  });

  test("a refused ticket shows the server's reason and stays on the first step", async ({ page }) => {
    const r = await rig(page, whoami(["cluster.view", "instance.console", "application.view"], { platform: [] }), {
      "POST /api/v1/applications/shop/instances/shop-web-01/sessions": error(429, "too_many_sessions", "you already have the most sessions allowed; close one first"),
    });
    await page.goto("/#/instances");
    await page.getByTestId("terminal-open").click();
    await page.getByTestId("session-open").click();
    await expect(page.getByTestId("session-error")).toContainText("you already have the most sessions allowed");
    await expect(page.getByTestId("session-prepare")).toBeVisible();
    expect(r.sockets).toHaveLength(0);
  });

  test("production: request approval, see it pending, then approved, and open with it", async ({ page }) => {
    let state: "none" | "pending" | "approved" = "none";
    const approval = (s: string) => ({ id: uid(800), action: "instance.console", target: "instance:shop/shop-web-01", requester: `user:${ME}`, approver: s === "approved" ? `user:${uid(2)}` : null, state: s, reason: "investigating a slow query", createdAt: T, expiresAt: "2099-01-01T00:00:00Z" });
    const r = await rig(page, SESSIONS, {
      "GET /api/v1/applications": { json: { items: [{ name: "shop", createdAt: T, latestRevision: 1 }], observedAt: T, stale: false } },
      "GET /api/v1/applications/shop/status": { json: { source: { provider: "mock", observedAt: T }, items: [{ module: "web-01", instance: "shop-web-01", project: "apelora", status: "Running", location: "node-1" }], services: [] } },
      "GET /api/v1/applications/shop/volumes": { json: { source: { provider: "mock", observedAt: T }, items: [] } },
      "GET /api/v1/applications/shop/access": { json: access({ allowed: true, approvalRequired: true }) },
      // A request for another instance, and someone else's request for this one, are not taken.
      "GET /api/v1/approvals": () => items([
        { ...approval("approved"), id: uid(801), target: "instance:shop/shop-db-01" },
        { ...approval("approved"), id: uid(803), requester: `user:${uid(2)}`, createdAt: "2026-10-21T11:00:00Z" },
        ...(state === "none" ? [] : [approval(state)]),
      ]),
      "POST /api/v1/approvals": () => { state = "pending"; return { status: 201, json: approval("pending") }; },
    });
    // People with instance.console but not cluster.view reach it from Applications.
    await page.goto("/#/applications");
    await page.getByTestId("application-toggle").click();
    await page.getByTestId("application-instance").getByTestId("terminal-open").click();
    const dialog = page.getByTestId("session-dialog");
    await expect(dialog.getByTestId("session-approval")).toContainText("runs in production");
    await expect(dialog.getByTestId("session-open")).toBeDisabled();
    await expect(dialog.getByTestId("approval-request")).toBeDisabled();
    await dialog.getByTestId("approval-reason").fill("investigating a slow query");
    await dialog.getByTestId("approval-request").click();
    await expect(dialog.getByTestId("approval-state")).toHaveAttribute("data-state", "pending");
    await expect(dialog.getByTestId("approval-state")).toContainText("Waiting for approval");
    expect(r.calls.find((c) => c.method === "POST" && c.path === "/api/v1/approvals")!.body).toEqual({ action: "instance.console", target: "instance:shop/shop-web-01", reason: "investigating a slow query" });
    await expect(dialog.getByTestId("session-open")).toBeDisabled();
    // Another person approves; the dialog notices on its own (or with Check now).
    state = "approved";
    await dialog.getByTestId("approval-check").click();
    await expect(dialog.getByTestId("approval-state")).toHaveAttribute("data-state", "approved");
    await expect(dialog.getByTestId("session-open")).toBeEnabled();
    await dialog.getByTestId("session-open").click();
    await expect(dialog.getByTestId("session-terminal")).toContainText("MOCK session for shop");
    expect(r.headers[0]?.["apelora-approval"]).toBe(uid(800));
  });

  test("production: an approval already granted is picked up when the dialog opens", async ({ page }) => {
    const r = await rig(page, whoami(["cluster.view", "instance.console", "application.view", "approval.request"], { platform: [] }), {
      "GET /api/v1/applications/shop/access": { json: access({ allowed: true, approvalRequired: true }) },
      "GET /api/v1/approvals": items([{ id: uid(802), action: "instance.console", target: "instance:shop/shop-web-01", requester: `user:${ME}`, approver: `user:${uid(2)}`, state: "approved", reason: "x", createdAt: T, expiresAt: "2099-01-01T00:00:00Z" }]),
    });
    await page.goto("/#/instances");
    await page.getByTestId("terminal-open").click();
    await expect(page.getByTestId("approval-state")).toContainText("Approved");
    await expect(page.getByTestId("approval-reason")).toHaveCount(0);
    await page.getByTestId("session-open").click();
    await expect(page.getByTestId("session-live")).toBeVisible();
    expect(r.headers[0]?.["apelora-approval"]).toBe(uid(802));
  });

  test("the session dialog has no serious accessibility violations", async ({ page }) => {
    await rig(page, SESSIONS, {
      "GET /api/v1/instances": { json: INSTANCES },
      "GET /api/v1/applications/shop/access": { json: access({ allowed: true, approvalRequired: true }) },
    });
    await page.goto("/#/instances");
    await page.getByTestId("terminal-open").click();
    await expect(page.getByTestId("session-approval")).toBeVisible();
    const results = await new AxeBuilder({ page }).include("[data-testid=session-dialog]").analyze();
    expect(results.violations.filter((v) => v.impact === "serious" || v.impact === "critical")).toEqual([]);
  });
});

const SESSION = (n: number, extra: Record<string, unknown> = {}) => ({
  id: uid(700 + n), application: "shop", instance: "shop-web-01", kind: "terminal", person: "sam",
  startedAt: new Date(Date.now() - 5 * 60_000).toISOString(), endsBy: new Date(Date.now() + 55 * 60_000).toISOString(),
  lastActivityAt: new Date(Date.now() - 3 * 60_000).toISOString(), idleEndsAt: new Date(Date.now() + 12 * 60_000).toISOString(), mine: false, ...extra,
});

test.describe("sessions page", () => {
  test("is offered under Identity & Access to people who open or operate sessions, at #/sessions", async ({ page }) => {
    await rig(page, SESSIONS);
    await page.goto("/");
    const group = page.getByRole("navigation", { name: "Console" }).getByRole("group", { name: "Identity & Access" });
    await expect(group.getByRole("link", { name: "Sessions" })).toBeVisible();
    await group.getByRole("link", { name: "Sessions" }).click();
    await expect(page).toHaveURL(/#\/sessions$/);
    await expect(page.getByRole("heading", { level: 1, name: "Sessions" })).toBeVisible();
    await expect(page.getByTestId("sessions-empty")).toBeVisible();
    // Without audit.read the history is not offered, and says where it lives.
    await expect(page.getByTestId("sessions-history-off")).toContainText("audit.read");
  });

  test("lists open sessions with who, where, when and time left, and ends one after confirming", async ({ page }) => {
    let ended = false;
    const r = await rig(page, OPERATOR, {
      "GET /api/v1/sessions": () => items(ended ? [SESSION(2, { mine: true, person: "alex", instance: "shop-db-01" })] : [SESSION(1), SESSION(2, { mine: true, person: "alex", instance: "shop-db-01", idleEndsAt: new Date(Date.now() + 90 * 60_000).toISOString() })]),
      [`DELETE /api/v1/sessions/${uid(701)}`]: () => { ended = true; return { status: 204 }; },
      "GET /api/v1/identity/audit": (c) => {
        expect(c.query.get("action")).toBe("session.");
        return items([
          auditItem(3, "session.close", "success", { actor: `user:${uid(2)}`, target: "instance:shop/shop-web-01", detail: { reason: "idle", seconds: 905 } }),
          auditItem(2, "session.open", "success", { actor: `user:${uid(2)}`, target: "instance:shop/shop-web-01", detail: { approval: uid(800), source: "198.51.100.7" } }),
          auditItem(1, "session.denied", "denied", { actor: `user:${uid(2)}`, target: "instance:shop/shop-web-01", detail: { reason: "approval" } }),
        ]);
      },
    });
    await page.goto("/#/sessions");
    const rows = page.getByTestId("session-row");
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0)).toContainText("sam");
    await expect(rows.nth(0)).toContainText("shop-web-01");
    await expect(rows.nth(0)).toContainText("Terminal · shop");
    await expect(rows.nth(0).getByTestId("session-left")).toContainText(/1[12] min/);
    await expect(rows.nth(0).getByTestId("session-left")).toContainText("unless they type");
    await expect(rows.nth(1)).toContainText("You");
    await expect(rows.nth(1).getByTestId("session-left")).toContainText("time limit");
    // History, from the audit trail.
    const history = page.getByTestId("session-history-row");
    await expect(history).toHaveCount(3);
    await expect(history.nth(0)).toContainText("Session ended");
    await expect(history.nth(0)).toContainText("reason: idle");
    await expect(history.nth(0)).toContainText("lasted 15 min");
    await expect(history.nth(0)).toContainText("Sam Support");
    await expect(history.nth(2)).toContainText("Denied");
    // Ending one: Cancel first, nothing happens.
    await rows.nth(0).getByTestId("session-terminate").click();
    const confirm = page.locator("dialog[open]").getByTestId("confirm-dialog");
    await expect(confirm).toContainText("End sam's session on shop-web-01?");
    await confirm.getByRole("button", { name: "Cancel" }).click();
    expect(count(r.calls, `DELETE /api/v1/sessions/${uid(701)}`)).toBe(0);
    await rows.nth(0).getByTestId("session-terminate").click();
    await page.locator("dialog[open]").getByTestId("confirm-action").click();
    await expect(page.getByTestId("sessions-notice")).toContainText("sam's session on shop-web-01 was ended");
    await expect(rows).toHaveCount(1);
    expect(count(r.calls, `DELETE /api/v1/sessions/${uid(701)}`)).toBe(1);
  });

  test("a refusal to end a session shows the server's reason", async ({ page }) => {
    await rig(page, OPERATOR, {
      "GET /api/v1/sessions": items([SESSION(1)]),
      [`DELETE /api/v1/sessions/${uid(701)}`]: error(404, "not_found", "not found: session"),
    });
    await page.goto("/#/sessions");
    await page.getByTestId("session-terminate").click();
    await page.locator("dialog[open]").getByTestId("confirm-action").click();
    await expect(page.getByTestId("sessions-error")).toContainText("not found: session");
  });

  test("has no serious accessibility violations and no sideways scroll on a phone", async ({ browser }) => {
    const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
    const page = await ctx.newPage();
    await rig(page, OPERATOR, {
      "GET /api/v1/sessions": items([SESSION(1), SESSION(2, { mine: true, person: "alex" })]),
      "GET /api/v1/identity/audit": items([auditItem(3, "session.close", "success", { target: "instance:shop/shop-web-01", detail: { reason: "idle", seconds: 905 } })]),
    });
    await page.goto("/#/sessions");
    await expect(page.getByTestId("session-row")).toHaveCount(2);
    await expect(page.getByTestId("session-history-row")).toHaveCount(1);
    await expect(page.getByTestId("session-terminate").first()).toBeVisible();
    const box = await page.getByTestId("session-terminate").first().boundingBox();
    expect(box!.x + box!.width).toBeLessThanOrEqual(390);
    expect(box!.height).toBeGreaterThanOrEqual(44);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow).toBeLessThanOrEqual(0);
    const results = await new AxeBuilder({ page }).include("main").analyze();
    expect(results.violations.filter((v) => v.impact === "serious" || v.impact === "critical")).toEqual([]);
    await ctx.close();
  });

  test("the terminal dialog fits a phone", async ({ browser }) => {
    const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
    const page = await ctx.newPage();
    await rig(page, whoami(["cluster.view", "instance.console", "application.view"], { platform: [] }));
    await page.goto("/#/instances");
    await page.getByTestId("terminal-open").click();
    await page.getByTestId("session-open").click();
    await expect(page.getByTestId("session-terminal")).toContainText("MOCK session");
    const box = await page.getByTestId("session-dialog").locator(".modal-box").boundingBox();
    expect(box!.x).toBeGreaterThanOrEqual(0);
    expect(box!.x + box!.width).toBeLessThanOrEqual(390);
    await expect(page.getByTestId("session-end")).toBeInViewport();
    await ctx.close();
  });
});
