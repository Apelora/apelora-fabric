import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page, type Request, type Route } from "@playwright/test";

// Alerts against a stubbed control plane: what needs attention first, acknowledging and
// silencing as the server allows, history, rules with their thresholds, webhook channels with
// a test send, the Overview row, the phone layout and axe. The server decides every state.

const now = "2026-10-21T10:00:00Z";
const health = { status: "ok", service: "control-plane", version: "0.1.0", apiVersion: "v1", provider: "mock", auth: "tokens", incus: { connected: false } };
const viewer = { name: "vic", role: "viewer", permissions: ["read-inventory"], deployProjects: [] };
const cluster = { source: { provider: "mock", observedAt: now }, server: { clustered: true, serverVersion: null }, members: [] };

const yes = { allowed: true, reason: null };
const no = (reason: string) => ({ allowed: false, reason });
const RESPOND_NO = no("Acknowledging and silencing alerts needs the alert.respond permission.");
const CONFIGURE_NO = no("Changing rules and channels needs the alert.configure permission.");
const SIL = "44444444-4444-4444-8444-444444444444";
const CH = "55555555-5555-4555-8555-555555555555";

const alert = (id: number, rule: string, title: string, subject: string, severity: string, extra: Record<string, unknown> = {}) => ({
  id, rule, title, subject, severity, state: "firing", summary: `${subject} is not right`, value: 1, threshold: 1, comparison: "at-least", unit: "while it lasts",
  startedAt: now, firedAt: now, lastSeenAt: now, resolvedAt: null, clearingSince: null, acknowledged: null, silencedBy: null, runbook: `alerts.md#${rule}`, ...extra,
});
const evaluator = { lastPassAt: now, everySecs: 30, worker: "worker-1", durationSecs: 0.2, stale: false, message: "Evaluated every 30 s by worker-1." };
const list = (alerts: unknown[], actions = { respond: yes, configure: yes }, silences: unknown[] = []) => ({
  alerts, silences, evaluator, actions,
  counts: { critical: alerts.filter((a) => (a as { severity: string }).severity === "critical").length, warning: alerts.filter((a) => (a as { severity: string }).severity === "warning").length, info: 0,
    needsAttention: alerts.filter((a) => !(a as { acknowledged: unknown }).acknowledged && !(a as { silencedBy: unknown }).silencedBy).length },
});
const params = (threshold: number, forSecs: number, severity: string) => ({ enabled: true, threshold, forSecs, severity });
const rules = (actions = { respond: yes, configure: yes }, wait = params(300, 60, "warning")) => ({
  actions,
  items: [
    { id: "job-queue-wait", title: "Jobs are waiting too long", description: "How long the oldest job that could run has waited.", unit: "seconds", comparison: "at-least", thresholdEditable: true, thresholdMin: 10, thresholdMax: 86400,
      params: wait, defaults: params(300, 60, "warning"), source: "Job queue", sourceState: "ok", firing: 0, runbook: "alerts.md#jobs-are-waiting-too-long", updatedAt: null, updatedBy: null },
    { id: "restore-drill-age", title: "Apelora's database restore is not proven", description: "Days since a restore was verified.", unit: "days", comparison: "at-least", thresholdEditable: true, thresholdMin: 1, thresholdMax: 365,
      params: params(35, 0, "warning"), defaults: params(35, 0, "warning"), source: "Restore drill record", sourceState: "not-configured", firing: 0, runbook: "alerts.md#x", updatedAt: null, updatedBy: null },
  ],
});
const channel = (extra: Record<string, unknown> = {}) => ({
  id: CH, name: "ops", kind: "webhook", host: "hooks.example", url: "https://hooks.example/apelora", secretRef: "APELORA_ALERT_SECRET_OPS", enabled: true, minSeverity: "warning",
  delivery: { pending: 0, failing: 0, lastError: null, lastDeliveredAt: null }, createdAt: now, createdBy: "user:1", updatedAt: now, ...extra,
});

type Reply = { status?: number; json?: unknown; delay?: number };
type Handler = Reply | ((req: Request) => Reply);
async function stub(page: Page, routes: Record<string, Handler>) {
  await page.route("**/api/v1/**", async (route: Route) => {
    const req = route.request();
    const h = routes[`${req.method()} ${new URL(req.url()).pathname}`];
    if (!h) return route.fulfill({ status: 404, json: { error: { code: "not_found", message: `no stub for ${req.method()} ${new URL(req.url()).pathname}` } } });
    const r = typeof h === "function" ? h(req) : h;
    await new Promise((res) => setTimeout(res, r.delay ?? 60));
    return route.fulfill({ status: r.status ?? (r.json === undefined ? 204 : 200), json: r.json });
  });
}

const base = (who = viewer) => ({ "GET /api/v1/health": { json: health }, "GET /api/v1/whoami": { json: who }, "GET /api/v1/cluster": { json: cluster }, "GET /api/v1/alerts/history": { json: { items: [] } } });

const serious = async (page: Page) => (await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze()).violations
  .filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => `${v.id}: ${v.nodes.map((n) => n.target).join(" ")}`);

test.beforeEach(async ({ page }) => {
  await page.route("**/api/**", (route) => route.abort());
  await page.emulateMedia({ reducedMotion: "reduce" });
  await page.addInitScript(() => sessionStorage.setItem("apelora.apiToken", "apl_test"));
});

test("alerts: what needs attention comes first; acknowledge and silence send what the person chose", { tag: "@smoke" }, async ({ page }) => {
  const sent: { what: string; body: unknown }[] = [];
  let acked = false;
  let silenced = false;
  const member = () => alert(2, "member-offline", "A cluster member is not online", "node-2", "critical", silenced ? { silencedBy: { id: SIL, until: "2026-10-21T11:00:00Z" } } : {});
  const jobs = () => alert(1, "job-failures", "Jobs are failing", "network.sync", "warning", acked ? { acknowledged: { by: "user:1", byName: "ada", at: now, note: "on it" } } : {});
  await stub(page, {
    ...base(),
    "GET /api/v1/alerts": () => ({ json: list([member(), jobs()], { respond: yes, configure: yes }, silenced ? [{ id: SIL, rule: "member-offline", ruleTitle: "A cluster member is not online", subject: "node-2", startsAt: now, endsAt: "2026-10-21T11:00:00Z", note: "rack work", createdBy: "user:1", createdByName: "ada" }] : []) }),
    "GET /api/v1/alerts/history": { json: { items: [alert(9, "outbox-stuck", "Messages are not being delivered", "outbox", "warning", { state: "resolved", resolvedAt: now })] } },
    "POST /api/v1/alerts/1/acknowledge": (req) => { acked = true; sent.push({ what: "ack", body: req.postDataJSON() }); return { json: jobs() }; },
    "POST /api/v1/alerts/silences": (req) => { silenced = true; sent.push({ what: "silence", body: req.postDataJSON() }); return { status: 201, json: { id: SIL } }; },
  });
  await page.goto("/#/alerts");
  await expect(page.getByRole("heading", { level: 1, name: "Alerts" })).toBeVisible();
  const rows = page.getByTestId("alert-row");
  await expect(rows).toHaveCount(2);
  await expect(rows.nth(0)).toContainText("Critical");
  await expect(rows.nth(0)).toContainText("node-2");
  await expect(page.getByTestId("alerts-evaluated")).toContainText("every 30 s");
  await expect(page.getByTestId("alert-history-row")).toContainText("Messages are not being delivered");
  // No primary button competes on this tab while no dialog is open.
  await expect(page.locator(".btn-primary:visible")).toHaveCount(0);

  await rows.nth(1).getByTestId("alert-ack").click();
  const ack = page.getByTestId("ack-dialog");
  await expect(page.getByTestId("ack-note")).toBeFocused();
  await page.getByTestId("ack-note").fill("on it");
  await ack.getByTestId("ack-submit").click();
  await expect(page.getByTestId("alerts-notice")).toContainText("You acknowledged “Jobs are failing” on network.sync");
  await expect(page.getByTestId("alert-handled-row")).toContainText("Acknowledged by ada");
  expect(sent[0]).toEqual({ what: "ack", body: { note: "on it" } });

  await page.getByTestId("alert-row").getByTestId("alert-silence").click();
  await page.getByTestId("silence-minutes").selectOption("240");
  await page.getByTestId("silence-note").fill("rack work");
  await page.getByTestId("silence-submit").click();
  await expect(page.getByTestId("alerts-none")).toContainText("Nothing needs attention");
  await expect(page.getByTestId("silence-row")).toContainText("rack work");
  expect(sent[1]).toEqual({ what: "silence", body: { rule: "member-offline", subject: "node-2", minutes: 240, note: "rack work" } });
  expect(await serious(page)).toEqual([]);
});

test("alerts: a viewer sees everything and changes nothing; a stale evaluator is said plainly", async ({ page }) => {
  await stub(page, {
    ...base(),
    "GET /api/v1/alerts": { json: { ...list([alert(1, "job-failures", "Jobs are failing", "network.sync", "warning")], { respond: RESPOND_NO, configure: CONFIGURE_NO }),
      evaluator: { lastPassAt: null, everySecs: null, worker: null, durationSecs: null, stale: true, message: "Alerts have not been evaluated yet: no deployment worker has run the alert evaluator." } } },
    "GET /api/v1/alerts/rules": { json: rules({ respond: RESPOND_NO, configure: CONFIGURE_NO }) },
    "GET /api/v1/alerts/channels": { json: { items: [channel({ url: null })], secretPrefix: "APELORA_ALERT_SECRET_", actions: { respond: RESPOND_NO, configure: CONFIGURE_NO } } },
  });
  await page.goto("/#/alerts");
  await expect(page.getByTestId("alerts-stale")).toContainText("no deployment worker has run the alert evaluator");
  await expect(page.getByTestId("alert-row")).toContainText("View only");
  await expect(page.getByTestId("alert-ack")).toHaveCount(0);
  await page.getByTestId("tab-rules").click();
  await expect(page.getByTestId("rule-row")).toHaveCount(2);
  await expect(page.getByTestId("rule-edit")).toHaveCount(0);
  await expect(page.getByTestId("rules-readonly")).toContainText("alert.configure");
  await expect(page.locator('[data-rule="restore-drill-age"]').getByTestId("rule-state")).toHaveText("Not set up");
  // Arrow keys move between tabs.
  await page.getByTestId("tab-rules").press("ArrowRight");
  await expect(page.getByTestId("tab-channels")).toBeFocused();
  await expect(page.getByTestId("channel-row")).toContainText("hooks.example");
  await expect(page.getByTestId("channel-add")).toHaveCount(0);
  expect(await serious(page)).toEqual([]);
});

test("alerts: a rule's threshold is edited and the server's refusal is shown", async ({ page }) => {
  let saved: unknown = null;
  let wait = params(300, 60, "warning");
  await stub(page, {
    ...base(),
    "GET /api/v1/alerts": { json: list([]) },
    "GET /api/v1/alerts/rules": () => ({ json: rules({ respond: yes, configure: yes }, wait) }),
    "PUT /api/v1/alerts/rules/job-queue-wait": (req) => {
      const body = req.postDataJSON() as { threshold: number; forSecs: number; severity: string; enabled: boolean };
      if (body.threshold < 60) return { status: 400, json: { error: { code: "invalid_request", message: "the threshold of job-queue-wait must be at least 60 seconds here" } } };
      saved = body;
      wait = body;
      return { json: rules().items[0] };
    },
  });
  await page.goto("/#/alerts");
  await expect(page.getByTestId("alerts-none")).toBeVisible();
  await page.getByTestId("tab-rules").click();
  const row = page.locator('[data-rule="job-queue-wait"]');
  await expect(row.getByTestId("rule-condition")).toHaveText("≥ 300 seconds");
  await row.getByTestId("rule-edit").click();
  await expect(page.getByTestId("rule-enabled")).toBeFocused();
  // Below the rule's minimum the browser stops it; what passes it, the server still decides.
  await page.getByTestId("rule-threshold").fill("5");
  await page.getByTestId("rule-save").click();
  await expect(page.getByTestId("rule-error")).toHaveCount(0);
  expect(saved).toBeNull();
  await page.getByTestId("rule-threshold").fill("15");
  await page.getByTestId("rule-save").click();
  await expect(page.getByTestId("rule-error")).toContainText("must be at least 60 seconds here");
  await page.getByTestId("rule-threshold").fill("900");
  await page.getByTestId("rule-severity").selectOption("critical");
  await page.getByTestId("rule-save").click();
  await expect(page.getByTestId("rules-notice")).toContainText("saved");
  expect(saved).toEqual({ enabled: true, threshold: 900, forSecs: 60, severity: "critical" });
  await expect(row.getByTestId("rule-condition")).toHaveText("≥ 900 seconds");
});

test("alerts: a webhook is added with a secret reference, tested, shows its failures and is deleted", async ({ page }) => {
  const sent: { what: string; body?: unknown }[] = [];
  let channels: ReturnType<typeof channel>[] = [];
  await stub(page, {
    ...base(),
    "GET /api/v1/alerts": { json: list([]) },
    "GET /api/v1/alerts/channels": () => ({ json: { items: channels, secretPrefix: "APELORA_ALERT_SECRET_", actions: { respond: yes, configure: yes } } }),
    "POST /api/v1/alerts/channels": (req) => {
      const body = req.postDataJSON() as { secretRef: string };
      sent.push({ what: "create", body });
      if (!body.secretRef.startsWith("APELORA_ALERT_SECRET_") || body.secretRef === "APELORA_ALERT_SECRET_") return { status: 400, json: { error: { code: "invalid_request", message: "the secret reference must name an environment variable on the worker that starts with APELORA_ALERT_SECRET_" } } };
      channels = [channel()];
      return { status: 201, json: channel() };
    },
    "POST /api/v1/alerts/channels/55555555-5555-4555-8555-555555555555/test": () => {
      sent.push({ what: "test" });
      channels = [channel({ delivery: { pending: 1, failing: 1, lastError: "secret APELORA_ALERT_SECRET_OPS is not set on the worker", lastDeliveredAt: null } })];
      return { status: 202, json: { messageId: 42 } };
    },
    "DELETE /api/v1/alerts/channels/55555555-5555-4555-8555-555555555555": () => { sent.push({ what: "delete" }); channels = []; return {}; },
  });
  await page.goto("/#/alerts");
  await page.getByTestId("tab-channels").click();
  await expect(page.getByTestId("channels-empty")).toBeVisible();
  await page.getByTestId("channel-add").click();
  await expect(page.getByTestId("channel-name")).toBeFocused();
  await page.getByTestId("channel-name").fill("ops");
  await page.getByTestId("channel-url").fill("https://hooks.example/apelora");
  // The field starts with the prefix; the server refuses a bare prefix.
  await expect(page.getByTestId("channel-secret")).toHaveValue("APELORA_ALERT_SECRET_");
  await page.getByTestId("channel-save").click();
  await expect(page.getByTestId("channel-error")).toContainText("starts with APELORA_ALERT_SECRET_");
  await page.getByTestId("channel-secret").fill("apelora_alert_secret_ops");
  await expect(page.getByTestId("channel-secret")).toHaveValue("APELORA_ALERT_SECRET_OPS");
  await page.getByTestId("channel-save").click();
  await expect(page.getByTestId("channels-notice")).toContainText("Send a test to check it");
  expect(sent[1]).toEqual({ what: "create", body: { name: "ops", url: "https://hooks.example/apelora", secretRef: "APELORA_ALERT_SECRET_OPS", minSeverity: "warning", enabled: true } });

  await page.getByTestId("channel-test").click();
  await expect(page.getByTestId("channels-notice")).toContainText("message 42");
  await expect(page.getByTestId("channel-delivery")).toContainText("Failing");
  await expect(page.getByTestId("channel-delivery")).toContainText("Secret APELORA_ALERT_SECRET_OPS is not set on the worker");

  await page.getByTestId("channel-edit").click();
  await page.getByTestId("channel-delete").click();
  const dialog = page.locator("dialog[open]");
  await expect(dialog.getByRole("button", { name: "Cancel" })).toBeFocused();
  await dialog.getByRole("button", { name: "Delete webhook" }).click();
  await expect(page.getByTestId("channels-empty")).toBeVisible();
  expect(sent.map((s) => s.what)).toEqual(["create", "create", "test", "delete"]);
  expect(await serious(page)).toEqual([]);
});

test("alerts: the Overview counts what needs attention and links to it", async ({ page }) => {
  await stub(page, {
    ...base(),
    "GET /api/v1/alerts": { json: list([alert(2, "member-offline", "A cluster member is not online", "node-2", "critical"), alert(1, "job-failures", "Jobs are failing", "network.sync", "warning", { acknowledged: { by: "user:1", byName: "ada", at: now, note: null } })]) },
  });
  await page.goto("/#/overview");
  await expect(page.getByTestId("overview-alerts")).toContainText("1 alert needs attention · 1 critical");
  await page.getByTestId("overview-alerts-link").click();
  await expect(page).toHaveURL(/#\/alerts$/);
  await expect(page.getByTestId("alert-row")).toHaveCount(1);
});

test("alerts: a person without inventory access is not offered Alerts", async ({ page }) => {
  await stub(page, { ...base({ name: "dee", role: "designer", permissions: ["design-applications"], deployProjects: [] }) });
  await page.goto("/#/overview");
  await expect(page.getByRole("navigation", { name: "Console" })).toBeVisible();
  await expect(page.getByRole("link", { name: "Alerts" })).toHaveCount(0);
  await expect(page.getByTestId("overview-alerts")).toHaveCount(0);
});

test.describe("phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
  test("alerts: rows keep their actions on screen with no sideways scroll", async ({ page }) => {
    await stub(page, {
      ...base(),
      "GET /api/v1/alerts": { json: list([alert(2, "member-offline", "A cluster member is not online", "node-2", "critical"), alert(1, "application-status", "An application is failed or degraded", "a-very-long-application-name-for-phones", "warning")]) },
    });
    await page.goto("/#/alerts");
    await expect(page.getByTestId("alert-row")).toHaveCount(2);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow).toBeLessThanOrEqual(0);
    const box = await page.getByTestId("alert-ack").first().boundingBox();
    expect(box).not.toBeNull();
    expect(box!.x + box!.width).toBeLessThanOrEqual(390);
    expect(box!.height).toBeGreaterThanOrEqual(44);
    expect(await serious(page)).toEqual([]);
  });
});
