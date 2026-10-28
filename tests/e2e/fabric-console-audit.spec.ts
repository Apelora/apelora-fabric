import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page, type Route } from "@playwright/test";

import { auditItem, base, error, items, stub, whoami, type Call } from "./identity-fixtures";

// The Audit log (`#/audit`): filters kept in the address, paging, and the export. The API is
// stubbed; the control plane's own tests prove the filters, the export and its audit row.

// A platform `audit.read` holder; `whoami` also reports the coarse `read-audit` for it.
const reader = { ...whoami(["audit.read"], { platform: ["audit.read"] }), permissions: ["read-audit"] };
const rows = [
  auditItem(12, "role.assign", "denied", { actorName: "ada", detail: { reason: "you cannot grant more than you hold", requestId: "req-1" } }),
  auditItem(11, "group.create", "success", { actorName: "ada", target: "Operators" }),
  auditItem(10, "auth.oidc.signed_in", "success", { actorName: null }),
];

const listCalls = (calls: Call[]) => calls.filter((c) => c.path === "/api/v1/audit");

/** Routes the export (text, not JSON) and records its queries. */
async function stubExport(page: Page, answer: (q: URLSearchParams) => { status: number; body: string; type: string }) {
  const seen: URLSearchParams[] = [];
  await page.route("**/api/v1/audit/export**", async (route: Route) => {
    const q = new URL(route.request().url()).searchParams;
    seen.push(q);
    const a = answer(q);
    await route.fulfill({ status: a.status, body: a.body, headers: { "content-type": a.type } });
  });
  return seen;
}

test.describe("audit log", () => {
  test("filters go to the API, stay in the address and come back from it", { tag: "@smoke" }, async ({ page }) => {
    const calls = await stub(page, { ...base(reader), "GET /api/v1/audit": (c) => (c.query.get("target") === "Operators" ? items([rows[1]]) : items(rows)) });
    await page.goto("/#/audit?actor=ada&outcome=denied");
    await expect(page.getByTestId("audit-row")).toHaveCount(3);
    const first = listCalls(calls)[0]!;
    expect([first.query.get("actor"), first.query.get("outcome"), first.query.get("limit")]).toEqual(["ada", "denied", "50"]);
    await expect(page.getByTestId("audit-actor")).toHaveValue("ada");
    await expect(page.getByTestId("audit-outcome-filter")).toHaveValue("denied");

    // Typing narrows (debounced), and the address follows.
    await page.getByTestId("audit-target").fill("Operators");
    await expect(page.getByTestId("audit-row")).toHaveCount(1);
    expect(listCalls(calls).at(-1)!.query.get("target")).toBe("Operators");
    await expect(page).toHaveURL(/#\/audit\?actor=ada&target=Operators&outcome=denied$/);

    // A time range is sent as RFC 3339.
    await page.getByTestId("audit-from").fill("2026-10-20T08:00");
    await expect.poll(() => listCalls(calls).at(-1)!.query.get("from")).toMatch(/^2026-10-2\dT\d\d:00:00\.000Z$/);
    await expect(page).toHaveURL(/from=2026-10-2/);

    // Clearing removes every filter, from the request and the address.
    await page.getByTestId("audit-clear").click();
    await expect.poll(() => [...listCalls(calls).at(-1)!.query.keys()].sort().join(",")).toBe("limit");
    await expect(page).toHaveURL(/#\/audit$/);
    await expect(page.getByTestId("audit-clear")).toHaveCount(0);
  });

  test("older events follow the page's next cursor until there is none", async ({ page }) => {
    const calls = await stub(page, {
      ...base(reader),
      "GET /api/v1/audit": (c) => (c.query.get("after") === "11" ? { json: { items: [rows[2]] } } : { json: { items: rows.slice(0, 2), next: "11" } }),
    });
    await page.goto("/#/audit");
    await expect(page.getByTestId("audit-row")).toHaveCount(2);
    await page.getByTestId("audit-older").click();
    await expect(page.getByTestId("audit-row")).toHaveCount(3);
    // Only the cursor the first page gave is sent, once.
    expect(listCalls(calls).map((c) => c.query.get("after")).filter((a) => a !== null)).toEqual(["11"]);
    // The last page has no next: no button, and the reader is told.
    await expect(page.getByTestId("audit-older")).toHaveCount(0);
    await expect(page.getByTestId("audit-end")).toBeVisible();
  });

  test("a row says who, what, the outcome and why it was refused", async ({ page }) => {
    await stub(page, { ...base(reader), "GET /api/v1/audit": items(rows) });
    await page.goto("/#/audit");
    const denied = page.locator('[data-testid="audit-row"][data-outcome="denied"]');
    await expect(denied).toContainText("ada");
    await expect(denied).toContainText("Role assign");
    await expect(denied.getByTestId("audit-outcome")).toHaveText("Denied");
    await expect(denied.getByTestId("audit-reason")).toHaveText("Reason: you cannot grant more than you hold");
    await denied.getByText("Details").click();
    await expect(denied).toContainText("Request req-1");
    // An older row without a recorded name: the person by id, as far as the reader may see names.
    await expect(page.getByTestId("audit-row").last()).toContainText("Person 00000000");
  });

  test("export downloads the filtered events as CSV or JSON lines and says how many", async ({ page }) => {
    await stub(page, { ...base(reader), "GET /api/v1/audit": items(rows) });
    const seen = await stubExport(page, (q) =>
      q.get("format") === "csv"
        ? { status: 200, type: "text/csv; charset=utf-8", body: "id,at,actor\r\n12,2026,user:a\r\n11,2026,user:a\r\n" }
        : { status: 200, type: "application/x-ndjson", body: '{"id":12}\n{"id":11}\n{"id":10}\n' },
    );
    await page.goto("/#/audit?actor=ada");
    await expect(page.getByTestId("audit-row")).toHaveCount(3);
    // Export is not the primary action of the page.
    await expect(page.locator("main .btn-primary")).toHaveCount(0);

    await page.getByTestId("audit-export").click();
    const csv = page.waitForEvent("download");
    await page.getByTestId("audit-export-csv").click();
    expect((await csv).suggestedFilename()).toMatch(/^apelora-audit-\d{8}T\d{6}Z\.csv$/);
    await expect(page.getByTestId("audit-export-done")).toHaveText("Exported 2 events as CSV. Exports are audited too.");
    expect([seen[0]!.get("format"), seen[0]!.get("actor")]).toEqual(["csv", "ada"]);

    await page.getByTestId("audit-export").click();
    const jsonl = page.waitForEvent("download");
    await page.getByTestId("audit-export-jsonl").click();
    expect((await jsonl).suggestedFilename()).toMatch(/\.jsonl$/);
    await expect(page.getByTestId("audit-export-done")).toHaveText("Exported 3 events as JSON lines. Exports are audited too.");
  });

  test("a refused export shows the server's reason and the list stays", async ({ page }) => {
    await stub(page, { ...base(reader), "GET /api/v1/audit": items(rows) });
    await stubExport(page, () => ({ status: 403, type: "application/json", body: JSON.stringify(error(403, "forbidden", "you may not read the audit log (audit.read)").json) }));
    await page.goto("/#/audit");
    await page.getByTestId("audit-export").click();
    await page.getByTestId("audit-export-csv").click();
    await expect(page.getByTestId("audit-export-error")).toHaveText("you may not read the audit log (audit.read)");
    await expect(page.getByTestId("audit-row")).toHaveCount(3);
  });

  test("a filter that matches nothing says so", async ({ page }) => {
    await stub(page, { ...base(reader), "GET /api/v1/audit": (c) => (c.query.get("action") ? items([]) : items(rows)) });
    await page.goto("/#/audit?action=nothing.");
    await expect(page.getByTestId("audit-empty")).toHaveText("No events match these filters.");
  });

  test("the Identity & Access history uses the same filters and export", async ({ page }) => {
    const calls = await stub(page, { ...base(reader), "GET /api/v1/identity/audit": items(rows) });
    await page.goto("/#/identity/audit?outcome=denied");
    await expect(page.getByTestId("identity-audit-row")).toHaveCount(3);
    const c = calls.find((x) => x.path === "/api/v1/identity/audit")!;
    expect(c.query.get("outcome")).toBe("denied");
    await expect(page.getByTestId("audit-export")).toBeVisible();
  });

  test("on a phone the filters stack, nothing scrolls sideways and there are no accessibility violations", async ({ browser }) => {
    const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
    const page = await ctx.newPage();
    await page.emulateMedia({ reducedMotion: "reduce" });
    await stub(page, { ...base(reader), "GET /api/v1/audit": items(rows) });
    await page.goto("/#/audit");
    await expect(page.getByTestId("audit-row")).toHaveCount(3);
    const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(overflow).toBeLessThanOrEqual(0);
    // Nor does the table: the time wraps instead of squeezing what happened.
    const frame = page.getByTestId("audit-row").first().locator("xpath=ancestor::div[contains(@class,'overflow-x-auto')]");
    expect(await frame.evaluate((el) => el.scrollWidth - el.clientWidth)).toBeLessThanOrEqual(0);
    // The filters fold away so the events come first; the toggle opens them.
    await expect(page.getByTestId("audit-actor")).toBeHidden();
    const toggle = page.getByTestId("audit-filters-toggle");
    await expect(toggle).toHaveAttribute("aria-expanded", "false");
    await toggle.click();
    await expect(toggle).toHaveAttribute("aria-expanded", "true");
    const [actor, target] = await Promise.all([page.getByTestId("audit-actor").boundingBox(), page.getByTestId("audit-target").boundingBox()]);
    expect(target!.y).toBeGreaterThan(actor!.y + actor!.height - 1);
    await expect(page.getByTestId("audit-export")).toBeInViewport();
    // Set filters keep them open and counted.
    await page.goto("/#/audit?outcome=denied");
    await page.reload();
    await expect(page.getByTestId("audit-filters-toggle")).toHaveText("Hide filters (1 set)");
    await expect(page.getByTestId("audit-outcome-filter")).toBeVisible();
    const r = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
    expect(r.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`)).toEqual([]);
    await ctx.close();
  });

  test("on a desktop there are no accessibility violations, the export menu open included", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await stub(page, { ...base(reader), "GET /api/v1/audit": items(rows) });
    await page.goto("/#/audit?outcome=denied");
    await expect(page.getByTestId("audit-row")).toHaveCount(3);
    await page.getByTestId("audit-export").click();
    await expect(page.getByTestId("audit-export-csv")).toBeVisible();
    const r = await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze();
    expect(r.violations.map((v) => `${v.id}: ${v.nodes.map((n) => n.target.join(" ")).join(", ")}`)).toEqual([]);
  });
});
