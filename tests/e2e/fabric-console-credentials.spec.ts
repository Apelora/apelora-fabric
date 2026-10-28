// SPDX-License-Identifier: AGPL-3.0-only
import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { base, error, stub, uid, whoami, type Call, type Handler, type Result } from "./identity-fixtures";

// Your tokens, Your sign-ins, Identity & Access → Sign-ins & tokens, and Administration →
// Settings, Support bundle and Licences, against a stubbed control plane. The rules themselves
// (live permissions, refusals, redaction, the bundle's contents) are proven by the control plane's
// tests (services/control-plane/src/api/account_tests.rs). Evidence: MOCK VERIFIED.

const T = "2026-10-21T10:00:00Z";
const LATER = "2026-11-20T10:00:00Z";
const SECRET = `apt_${"ab".repeat(32)}`;

const token = (n: number, extra: Record<string, unknown> = {}) => ({
  id: uid(800 + n), name: `script ${n}`, userId: uid(1), owner: "alex", state: "active", createdAt: T, expiresAt: LATER,
  lastUsedAt: n === 1 ? T : null, revokedAt: null, revokedBy: null, rotatedFrom: null, successor: null, current: false, ...extra,
});
const tokens = (list: unknown[]) => ({ json: { items: list, limit: 20 } });
const signIn = (n: number, extra: Record<string, unknown> = {}) => ({
  id: uid(900 + n), userId: uid(1), person: "rita", method: "password", createdAt: T, lastUsedAt: T, expiresAt: "2026-10-21T18:00:00Z", endsAt: "2026-10-21T11:00:00Z", current: false, ...extra,
});
const signIns = (list: unknown[], credential = "session") => ({ json: { items: list, credential, idleSecs: 3600, absoluteSecs: 28800 } });

const setting = (name: string, extra: Record<string, unknown> = {}) => ({ name, value: null, set: false, isDefault: true, default: null, source: "not set", hidden: null, readBy: ["control-plane"], description: `What ${name} does`, known: true, ...extra });
const SETTINGS = {
  items: [
    setting("APELORA_API_ADDR", { value: "0.0.0.0:8080", set: true, isDefault: false, default: "127.0.0.1:8080", source: "environment" }),
    setting("APELORA_CONSOLE_IDLE_SECS", { value: "900", default: "900", source: "default" }),
    setting("APELORA_TLS_KEY", { set: true, isDefault: false, source: "environment", hidden: "secret" }),
    setting("APELORA_VOLUME_POOL"),
    setting("APELORA_TYPO", { set: true, isDefault: false, source: "environment", hidden: "unknown setting", known: false, readBy: [] }),
  ],
  saved: [setting("auth.local_accounts", { value: "on", set: true, isDefault: false, default: "off", source: "console" })],
  workers: [{ id: "worker-1", host: "node-1", version: "0.0.0", lastSeen: T }],
  readAt: T,
};
const NOTICES = {
  product: "Apelora", licence: "AGPL-3.0-only", version: "0.0.0", commit: "0123456789abcdef", sourceUrl: "https://github.com/Apelora/apelora-fabric",
  licenceUrl: "/apelora-license.txt", agplUrl: "https://www.gnu.org/licenses/agpl-3.0.html", noticesUrl: "/third-party-licenses.txt", generated: T, problems: [],
  components: [{ ecosystem: "cargo", name: "axum", version: "0.8.9", license: "MIT" }, { ecosystem: "npm", name: "react", version: "19.0.0", license: "MIT" }],
};
const ADMIN_PAGE: Record<string, Result> = {
  "GET /api/v1/system": { json: { version: "0.0.0", apiVersion: "v1", incusPinned: "v7.0.3", incusSeries: "7.0", incusTestedRelease: "", certificates: [] } },
  "GET /api/v1/image-mirrors": { json: { mode: "any", mirrors: [], overall: "ok", checks: [], servers: [] } },
  "GET /api/v1/monitoring": { json: { region: "default", site: "default", sector: "default", provider: "mock", mayScrape: false, permission: "fabric.metrics.read", role: "metrics-reader", endpoints: [], formats: [], incusMetricsPath: "/1.0/metrics" } },
  "GET /api/v1/tokens": { json: { items: [] } },
  "GET /api/v1/settings": { json: SETTINGS },
};

async function rig(page: Page, me: unknown, extra: Record<string, Handler | Result> = {}): Promise<Call[]> {
  await page.addInitScript(() => sessionStorage.setItem("apelora.apiToken", "eyJhbGciOiJFUzI1NiJ9.e30.stub"));
  await page.route("**/licenses.json", (r) => r.fulfill({ json: NOTICES }));
  return stub(page, { ...base(me), ...extra });
}

const ME = whoami(["cluster.view"], { platform: ["cluster.view"] });

test.describe("Your tokens", () => {
  test("is in the user menu; a token is created, shown once, replaced and revoked", { tag: "@smoke" }, async ({ page }) => {
    let list: unknown[] = [];
    const calls = await rig(page, ME, {
      "GET /api/v1/me/tokens": () => tokens(list),
      "POST /api/v1/me/tokens": (c) => {
        list = [token(1, { name: (c.body as { name: string }).name })];
        return { status: 201, json: { token: SECRET, info: list[0] } };
      },
      [`POST /api/v1/me/tokens/${uid(801)}/rotate`]: () => {
        list = [token(2, { name: "Backup script", rotatedFrom: uid(801) }), token(1, { name: "Backup script", state: "rotating", successor: uid(802), expiresAt: "2026-10-21T10:10:00Z" })];
        return { status: 201, json: { token: SECRET.replace("ab", "cd"), info: list[0], previous: list[1] } };
      },
      [`DELETE /api/v1/me/tokens/${uid(802)}`]: () => { list = [token(2, { name: "Backup script", state: "revoked", revokedAt: T, revokedBy: `user:${uid(1)}` })]; return {}; },
    });
    await page.goto("/");
    await page.getByTestId("user-menu").click();
    await page.getByTestId("menu-tokens").click();
    await expect(page).toHaveURL(/#\/account\/tokens$/);
    await expect(page.getByRole("heading", { level: 1, name: "Your tokens" })).toBeVisible();
    await expect(page.getByTestId("personal-tokens-empty")).toBeVisible();

    await page.getByTestId("personal-token-open").click();
    const dialog = page.getByTestId("personal-token-dialog");
    await expect(dialog.getByTestId("personal-token-submit")).toBeDisabled();
    await dialog.getByTestId("personal-token-name").fill("Backup script");
    await dialog.getByTestId("personal-token-days").selectOption("90");
    await dialog.getByTestId("personal-token-submit").click();
    await expect(page.getByTestId("secret-value")).toHaveValue(SECRET);
    expect(calls.find((c) => c.method === "POST" && c.path === "/api/v1/me/tokens")?.body).toEqual({ name: "Backup script", expiresInDays: 90 });
    await page.getByTestId("secret-close").click();
    await expect(page.locator(`input[value="${SECRET}"]`)).toHaveCount(0);
    await expect(page.getByTestId("personal-token-row")).toHaveCount(1);
    await expect(page.getByTestId("personal-token-row")).toContainText("Active");

    await page.getByTestId("personal-token-rotate").click();
    await page.getByTestId("rotate-overlap").selectOption("10");
    await page.getByTestId("rotate-submit").click();
    expect(calls.find((c) => c.path.endsWith("/rotate"))?.body).toEqual({ overlapMinutes: 10 });
    await expect(page.getByTestId("secret-dialog")).toContainText("The old token works until");
    await page.getByTestId("secret-close").click();
    await expect(page.locator("[data-testid=personal-token-row][data-state=rotating]")).toContainText("Replaced");

    await page.locator("[data-testid=personal-token-row][data-state=active]").getByTestId("personal-token-revoke").click();
    const confirm = page.locator("dialog[open]");
    await expect(confirm.getByRole("button", { name: "Cancel" })).toBeFocused();
    await confirm.getByTestId("confirm-action").click();
    await expect(page.getByTestId("personal-token-notice")).toContainText("no longer works");
    await expect(page.locator("[data-testid=personal-token-row][data-state=revoked]")).toHaveCount(1);
  });

  test("shows the server's refusal and says why a personal token cannot make another", async ({ page }) => {
    await rig(page, whoami(["cluster.view"], { credential: "personal-token" }), {
      "GET /api/v1/me/tokens": tokens([token(1, { current: true })]),
    });
    await page.goto("/#/account/tokens");
    await expect(page.getByTestId("personal-token-open")).toBeDisabled();
    await expect(page.getByTestId("personal-token-why")).toContainText("a personal token cannot make another");
    await expect(page.getByTestId("personal-token-row")).toContainText("Used for this page");
    await expect(page.getByTestId("personal-token-rotate")).toBeDisabled();
  });

  test("a create refused by the server keeps the dialog open with its reason", async ({ page }) => {
    await rig(page, ME, {
      "GET /api/v1/me/tokens": tokens([]),
      "POST /api/v1/me/tokens": error(409, "too_many_tokens", "you already have 20 personal tokens; revoke one you no longer use"),
    });
    await page.goto("/#/account/tokens");
    await page.getByTestId("personal-token-open").click();
    await page.getByTestId("personal-token-name").fill("one more");
    await page.getByTestId("personal-token-submit").click();
    await expect(page.getByTestId("personal-token-error")).toContainText("revoke one you no longer use");
    await expect(page.getByTestId("secret-dialog").locator("input")).toHaveCount(0);
  });

  test("an automation identity is told tokens belong to people", async ({ page }) => {
    await rig(page, whoami(["cluster.view"], { kind: "service", name: "ci" }));
    await page.goto("/#/account/tokens");
    await expect(page.getByTestId("personal-tokens-people-only")).toContainText("belong to people");
  });
});

test.describe("Your sign-ins", () => {
  test("lists this person's sign-ins and signs another browser out", async ({ page }) => {
    let list = [signIn(1, { current: true }), signIn(2)];
    const calls = await rig(page, { ...whoami(["user.view"], { name: "rita", credential: "session" }), localAccount: true }, {
      "GET /api/v1/me/sign-ins": () => signIns(list),
      [`DELETE /api/v1/me/sign-ins/${uid(902)}`]: () => { list = [list[0]!]; return {}; },
    });
    await page.goto("/");
    await page.getByTestId("user-menu").click();
    await page.getByTestId("menu-sign-ins").click();
    await expect(page.getByRole("heading", { level: 1, name: "Your sign-ins" })).toBeVisible();
    await expect(page.getByTestId("sign-in-row")).toHaveCount(2);
    await expect(page.getByTestId("sign-in-row").first()).toContainText("This browser");
    await expect(page.getByTestId("sign-in-row").first().getByTestId("sign-in-end")).toHaveCount(0);
    await page.getByTestId("sign-in-end").click();
    await page.locator("dialog[open]").getByTestId("confirm-action").click();
    await expect(page.getByTestId("sign-ins-notice")).toHaveText("That browser is signed out.");
    await expect(page.getByTestId("sign-in-row")).toHaveCount(1);
    expect(calls.some((c) => c.method === "DELETE" && c.path === `/api/v1/me/sign-ins/${uid(902)}`)).toBe(true);
  });

  test("says that an identity provider sign-in is not a session Apelora keeps", async ({ page }) => {
    await rig(page, ME, { "GET /api/v1/me/sign-ins": signIns([], "identity-provider") });
    await page.goto("/#/account/sign-ins");
    await expect(page.getByTestId("sign-ins-provider")).toContainText("Apelora does not keep that sign-in");
    await expect(page.getByTestId("my-sign-ins-empty")).toBeVisible();
  });
});

test.describe("Identity & Access → Sign-ins & tokens", () => {
  test("administrators see and end everyone's sign-ins and tokens", async ({ page }) => {
    const calls = await rig(page, whoami(["user.manage", "token.manage", "user.view"]), {
      "GET /api/v1/identity/sign-ins": signIns([signIn(1, { person: "rita" }), signIn(2, { person: "sam", method: "directory" })], "identity-provider"),
      "GET /api/v1/identity/personal-tokens": tokens([token(1, { owner: "sam" }), token(3, { owner: "rita", state: "expired" })]),
      [`DELETE /api/v1/identity/sign-ins/${uid(902)}`]: {},
      [`DELETE /api/v1/identity/personal-tokens/${uid(801)}`]: {},
    });
    await page.goto("/");
    await page.getByTestId("nav-identity").getByRole("link", { name: "Sign-ins & tokens" }).click();
    await expect(page.getByRole("heading", { level: 1, name: "Sign-ins & tokens" })).toBeVisible();
    await expect(page.getByTestId("people-sign-ins").getByTestId("sign-in-row")).toHaveCount(2);
    await page.getByRole("button", { name: /^Sign out sam,/ }).click();
    await page.locator("dialog[open]").getByTestId("confirm-action").click();
    await expect(page.getByTestId("people-sign-ins-notice")).toContainText("sam was signed out");
    await expect(page.getByTestId("people-token-revoke")).toHaveCount(1);
    await page.getByTestId("people-token-revoke").click();
    await expect(page.locator("dialog[open]")).toContainText("Revoke sam’s token");
    await page.locator("dialog[open]").getByTestId("confirm-action").click();
    await expect(page.getByTestId("people-tokens-notice")).toContainText("no longer works");
    expect(calls.filter((c) => c.method === "DELETE").map((c) => c.path)).toEqual([`/api/v1/identity/sign-ins/${uid(902)}`, `/api/v1/identity/personal-tokens/${uid(801)}`]);
  });

  test("is not offered without user.manage or token.manage", async ({ page }) => {
    await rig(page, whoami(["user.view", "audit.read"]));
    await page.goto("/");
    await expect(page.getByTestId("nav-identity")).toBeVisible();
    await expect(page.getByTestId("nav-identity").getByRole("link", { name: "Sign-ins & tokens" })).toHaveCount(0);
  });
});

test.describe("Administration: settings, support bundle, licences", () => {
  test("settings: defaults muted, secrets never shown, filter and changed-only", async ({ page }) => {
    await rig(page, ME, ADMIN_PAGE);
    await page.goto("/#/administration");
    const section = page.getByTestId("settings");
    await expect(section.getByTestId("setting-row")).toHaveCount(5 + 1);
    const addr = section.locator("[data-name=APELORA_API_ADDR]");
    await expect(addr).toContainText("0.0.0.0:8080");
    await expect(addr.locator("code")).toHaveClass(/font-semibold/);
    await expect(section.locator("[data-name=APELORA_CONSOLE_IDLE_SECS] code")).toHaveClass(/text-base-content\/60/);
    await expect(section.locator("[data-name=APELORA_TLS_KEY]")).toContainText("not shown: a secret");
    await expect(section.locator("[data-name=APELORA_TYPO]")).toContainText("Not a setting this version reads");
    await expect(section.locator("[data-name=APELORA_VOLUME_POOL]")).toContainText("Off");
    await section.getByTestId("settings-changed").check();
    await expect(section.locator("table").first().getByTestId("setting-row")).toHaveCount(3);
    await section.getByTestId("settings-filter").fill("tls");
    await expect(section.locator("table").first().getByTestId("setting-row")).toHaveCount(1);
    await section.getByTestId("settings-filter").fill("nothing like this");
    await expect(section.getByTestId("settings-none")).toBeVisible();
    await expect(section.getByTestId("settings-workers")).toContainText("worker-1 on node-1");
  });

  test("settings and the support bundle need their permissions", async ({ page }) => {
    await rig(page, whoami([], { platform: [] }), ADMIN_PAGE);
    await page.goto("/#/administration");
    await expect(page.getByTestId("licences")).toBeVisible();
    await expect(page.getByTestId("settings")).toHaveCount(0);
    await expect(page.getByTestId("support-bundle")).toHaveCount(0);
  });

  test("the support bundle downloads as a file, and a refusal is shown", async ({ page }) => {
    let refuse = false;
    const calls = await rig(page, whoami(["cluster.view", "support.bundle"]), {
      ...ADMIN_PAGE,
      "GET /api/v1/support-bundle": () => refuse ? error(503, "store_unavailable", "the database is not reachable; check DATABASE_URL and that PostgreSQL is running") : { json: undefined },
    });
    // A real tar body (the stub helper answers JSON only).
    await page.route("**/api/v1/support-bundle", (r) => refuse ? r.fallback() : r.fulfill({ status: 200, headers: { "content-type": "application/x-tar" }, body: Buffer.alloc(2048) }));
    await page.goto("/#/administration");
    const section = page.getByTestId("support-bundle");
    const download = page.waitForEvent("download");
    await section.getByTestId("support-bundle-download").click();
    expect((await download).suggestedFilename()).toMatch(/^apelora-support-\d{8}-\d{6}\.tar$/);
    await expect(section.getByTestId("support-bundle-saved")).toContainText("2 KiB");
    refuse = true;
    await section.getByTestId("support-bundle-download").click();
    await expect(section.getByTestId("support-bundle-error")).toContainText("the database is not reachable");
    expect(calls.filter((c) => c.path === "/api/v1/support-bundle")).toHaveLength(1);
  });

  test("licences: Apelora's licence, its source and the third-party counts", async ({ page }) => {
    await rig(page, ME, ADMIN_PAGE);
    await page.goto("/#/administration");
    const section = page.getByTestId("licences");
    await expect(section.getByTestId("licence-own")).toHaveText("AGPL-3.0-only");
    await expect(section.getByTestId("licence-build")).toHaveText("Version 0.0.0, commit 0123456789ab");
    await expect(section.getByTestId("licence-source")).toHaveAttribute("href", "https://github.com/Apelora/apelora-fabric");
    await expect(section.getByTestId("licence-counts")).toHaveText("1 Rust crate and 1 npm package");
    await expect(section.getByTestId("licence-page")).toHaveAttribute("href", "/licenses");
    // No primary action here: downloading and reading are secondary.
    await expect(page.locator("main .btn-primary:visible")).toHaveCount(0);
  });

  test("the development server serves the real licence page, linked from sign-in", async ({ page }) => {
    test.setTimeout(60_000);
    await page.goto("/licenses");
    await expect(page.getByRole("heading", { level: 1, name: "Licences" })).toBeVisible();
    await expect(page.getByText("(AGPL-3.0-only)")).toBeVisible();
    await expect(page.getByRole("row").filter({ hasText: "react-dom" }).first()).toBeVisible();
    await stub(page, {
      "GET /api/v1/health": { json: { status: "ok", service: "control-plane", version: "0.0.0", apiVersion: "v1", provider: "mock", auth: "tokens", incus: { connected: false }, checkedAt: T } },
      "GET /api/v1/whoami": error(401, "unauthenticated", "a bearer token is required"),
      "GET /api/v1/sign-in": { json: { providers: [], apiTokens: true } },
    });
    await page.goto("/");
    await expect(page.getByTestId("sign-in-licences")).toHaveAttribute("href", "/licenses");
  });
});

test.describe("phones and accessibility", () => {
  for (const [path, me, extra] of [
    ["/#/account/tokens", ME, { "GET /api/v1/me/tokens": tokens([token(1, { name: "a rather long token name for the laptop backup", current: true }), token(2, { state: "rotating", successor: uid(803) }), token(3, { state: "revoked", revokedAt: T, revokedBy: "disabled" })]) }],
    ["/#/account/sign-ins", whoami([], { credential: "session" }), { "GET /api/v1/me/sign-ins": signIns([signIn(1, { current: true }), signIn(2, { method: "directory" })]) }],
    ["/#/identity/credentials", whoami(["user.manage", "token.manage"]), { "GET /api/v1/identity/sign-ins": signIns([signIn(1)]), "GET /api/v1/identity/personal-tokens": tokens([token(1)]) }],
    ["/#/administration", whoami(["cluster.view", "support.bundle"]), ADMIN_PAGE],
  ] as const) {
    test(`${path} fits a phone without sideways scroll and has no serious accessibility violations`, async ({ browser }) => {
      const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
      const page = await ctx.newPage();
      await page.emulateMedia({ reducedMotion: "reduce" });
      await rig(page, me, extra as Record<string, Result>);
      await page.goto(path);
      await expect(page.locator("main h1")).toBeVisible();
      await expect(page.locator("main [role=status]:has-text('Loading')")).toHaveCount(0);
      const overflow = await page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
      expect(overflow).toBeLessThanOrEqual(0);
      for (const b of await page.locator("main table button:visible").all()) {
        const box = await b.boundingBox();
        expect(box!.x + box!.width).toBeLessThanOrEqual(390);
      }
      const results = await new AxeBuilder({ page }).include("main").analyze();
      expect(results.violations.filter((v) => v.impact === "serious" || v.impact === "critical")).toEqual([]);
      await ctx.close();
    });
  }
});
