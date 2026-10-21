import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page, type Route } from "@playwright/test";

// Administration → Apelora's database backups, against a stubbed control plane: not set up, fine,
// stale, failing, and who sees the section.

const NOW = "2026-10-21T10:00:00Z";
const health = { status: "ok", service: "control-plane", version: "0.1.0", apiVersion: "v1", provider: "incus", auth: "tokens", incus: { connected: true }, checkedAt: NOW };
const admin = { name: "alex", kind: "user", id: "u1", credential: "identity-provider", role: null, permissions: ["read-inventory"], capabilities: ["cluster.view"], platformPermissions: ["cluster.view"], roles: [], deployProjects: ["apelora"], localAccount: false };
const noView = { ...admin, capabilities: [], platformPermissions: [] };
const system = { version: "0.1.0", apiVersion: "v1", incusPinned: "v7.0.2", incusSeries: "7.0", incusTestedWith: "REAL INCUS TESTED with 7.0.1", incus: { connected: true, apiVersion: "1.0", serverVersion: "7.0.1", clustered: true }, certificates: [] };

const backup = { file: "apelora-db-20261021T020000Z.dump", createdAt: "2026-10-21T02:00:00Z", sizeBytes: 5_242_880, encryption: "age", pgDumpVersion: "pg_dump 18", durationSeconds: 4, migrations: 28 };
const passed = { drilledAt: "2026-10-20T04:00:00Z", backup: backup.file, passed: true, reason: "", durationSeconds: 12 };

async function install(page: Page, who: unknown, report: unknown) {
  await page.route("**/api/**", async (route: Route) => {
    const p = new URL(route.request().url()).pathname;
    const json = (body: unknown, status = 200) => route.fulfill({ status, json: body });
    await new Promise((r) => setTimeout(r, 40));
    if (p === "/api/v1/health") return json(health);
    if (p === "/api/v1/whoami") return json(who);
    if (p === "/api/v1/system") return json(system);
    if (p === "/api/v1/database-backups") return json(report);
    return json({ error: { code: "not_found", message: `unstubbed ${p}` } }, 404);
  });
}

const finding = (level: string, text: string) => ({ level, text });
const ok = { configured: true, directory: "/srv/apelora-backups", backups: 14, newestBackup: backup, lastDrill: passed, lastVerified: passed, assessment: { state: "ok", backupAgeHours: 8, drillAgeDays: 1, findings: [finding("ok", "Backed up recently, and a restore was verified recently.")] } };

test.beforeEach(async ({ page }) => {
  await page.route("**/api/**", (route) => route.abort());
  await page.emulateMedia({ reducedMotion: "reduce" });
});

test("not set up: says how, and what is at stake", async ({ page }) => {
  await install(page, admin, { configured: false, backups: 0, newestBackup: null, lastDrill: null, lastVerified: null, assessment: null });
  await page.goto("/#/administration");
  const s = page.getByTestId("database-backups");
  await expect(s.getByTestId("backups-not-configured")).toContainText("APELORA_DB_BACKUP_DIR");
});

test("a recent encrypted backup and a verified restore", async ({ page }) => {
  await install(page, admin, ok);
  await page.goto("/#/administration");
  const s = page.getByTestId("database-backups");
  await expect(s.getByTestId("backups-state")).toHaveAttribute("data-state", "ok");
  await expect(s).toContainText("14 backups");
  await expect(s).toContainText("8 h ago");
  await expect(s).toContainText("5.0 MiB");
  await expect(s).toContainText("encrypted (age)");
  await expect(s).toContainText("28 migrations");
  await expect(s).toContainText("1 days ago");
});

test("stale, unencrypted and failed-drill findings are listed worst first", async ({ page }) => {
  const bad = { ...ok, newestBackup: { ...backup, encryption: "none" }, lastDrill: { ...passed, passed: false, reason: "the audit trigger is missing", drilledAt: "2026-10-21T04:00:00Z" },
    assessment: { state: "failing", backupAgeHours: 80, drillAgeDays: 1, findings: [finding("failing", "The latest restore drill failed: the audit trigger is missing"), finding("failing", "The newest backup is 80 hours old; daily backups have stopped."), finding("warning", "The newest backup is not encrypted. It holds the audit trail and every stored secret reference.")] } };
  await install(page, admin, bad);
  await page.goto("/#/administration");
  const s = page.getByTestId("database-backups");
  await expect(s.getByTestId("backups-state")).toHaveAttribute("data-state", "failing");
  await expect(s.getByTestId("backups-findings").getByRole("listitem")).toHaveCount(3);
  await expect(s.getByTestId("backups-findings").getByRole("listitem").first()).toContainText("audit trigger is missing");
  await expect(s).toContainText("not encrypted");
  await expect(s).toContainText("failed");
});

test("people without cluster.view do not see the section", async ({ page }) => {
  await install(page, noView, ok);
  await page.goto("/#/administration");
  await expect(page.getByTestId("system")).toBeVisible();
  await expect(page.getByTestId("database-backups")).toHaveCount(0);
});

test("the section has no serious accessibility violations", async ({ page }) => {
  await install(page, admin, ok);
  await page.goto("/#/administration");
  await expect(page.getByTestId("database-backups").getByTestId("backups-state")).toBeVisible();
  const r = await new AxeBuilder({ page }).include("[data-testid=database-backups]").withTags(["wcag2a", "wcag2aa"]).analyze();
  expect(r.violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => v.id)).toEqual([]);
});
