import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { ALEX, NOW, SAM, base, error, items, stub, uid, whoami, type Call } from "./identity-fixtures";

// Local accounts in the Fabric Console against a stubbed control plane (nothing reaches a real
// server): password sign-in, the set-password page, creating and resetting accounts, the user
// menu (sign out, change password) and the local-accounts setting. Stubs answer after a short
// delay, as a real control plane hashing a password does.

test.beforeEach(async ({ page }) => { await page.route("**/api/**", (route) => route.abort()); });

const SESSION = `apu_${"a".repeat(64)}`;
const SETUP = `apw_${"b".repeat(64)}`;
const LOCAL = { id: uid(5), username: "rita", email: null, displayName: "Rita Local", issuer: "apelora:local", createdAt: NOW, lastSeenAt: null, disabled: false };
const options = (o: { providers?: unknown[]; localAccounts?: boolean; apiTokens?: boolean } = {}) => ({ json: { providers: o.providers ?? [], apiTokens: o.apiTokens ?? true, localAccounts: o.localAccounts ?? true, directories: [] } });
const countOf = (calls: Call[], line: string) => calls.filter((c) => `${c.method} ${c.path}` === line).length;
const bodyOf = (calls: Call[], line: string) => calls.filter((c) => `${c.method} ${c.path}` === line).map((c) => c.body);
const noSidewaysScroll = (page: Page) => page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth);
const serious = async (page: Page) => (await page.addStyleTag({ content: "*, *::before, *::after { transition: none !important; animation: none !important; }" }), await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze()).violations
  .filter((v) => v.impact === "serious" || v.impact === "critical")
  .map((v) => `${v.id}: ${v.nodes.slice(0, 3).map((n) => `${n.target.join(" ")} ${(n.any[0]?.message ?? "").slice(0, 160)}`).join(" | ")}`);

/** A signed-out console whose password sign-in accepts `rita` / `right password here`. */
async function signedOut(page: Page, opts: { providers?: unknown[] } = {}) {
  let signedIn = false;
  const calls = await stub(page, {
    ...base(),
    "GET /api/v1/sign-in": options({ providers: opts.providers }),
    "GET /api/v1/whoami": () => (signedIn ? { json: { ...whoami(["user.view"], { name: "rita" }), localAccount: true } } : error(401, "unauthenticated", "a bearer token is required")),
    "GET /api/v1/cluster": { json: { source: { provider: "mock", observedAt: NOW }, server: { clustered: false, serverVersion: null }, members: [] } },
    "POST /api/v1/sign-in/password": (c) => {
      const b = c.body as { username: string; password: string };
      if (b.username === "rita" && b.password === "right password here") { signedIn = true; return { json: { token: SESSION, expiresAt: NOW, name: "Rita Local" }, delayMs: 300 }; }
      return { ...error(401, "invalid_credentials", "The username or password is incorrect."), delayMs: 300 };
    },
  });
  return calls;
}

test.describe("password sign-in", () => {
  test("with no provider the username and password come first; a wrong password says so; the right one signs in", { tag: "@smoke" }, async ({ page }) => {
    const calls = await signedOut(page);
    await page.goto("/");
    const card = page.getByTestId("sign-in-page");
    const user = card.getByTestId("username-input");
    await expect(user).toBeFocused();
    await expect(user).toHaveAttribute("autocomplete", "username");
    await expect(card.getByTestId("password-input")).toHaveAttribute("autocomplete", "current-password");
    await expect(card.getByTestId("password-submit")).toHaveClass(/btn-primary/);
    await expect(card.getByTestId("sign-in-use-token")).toBeVisible();
    await expect(card.getByTestId("sign-in")).toHaveCount(0);

    // Nothing typed: explained next to the field, nothing sent.
    await card.getByTestId("password-submit").click();
    await expect(card.getByText("Enter your username.")).toBeVisible();
    expect(countOf(calls, "POST /api/v1/sign-in/password")).toBe(0);

    await user.fill("rita");
    await card.getByTestId("password-input").fill("not the password");
    await card.getByTestId("password-input").press("Enter");
    await expect(card.getByTestId("password-submit")).toBeDisabled();
    await expect(card.getByTestId("password-sign-in-error")).toHaveText("The username or password is incorrect.");
    await expect(card.getByTestId("password-input")).toHaveValue("");
    await expect(card.getByTestId("password-input")).toBeFocused();

    await card.getByTestId("password-input").fill("right password here");
    await card.getByTestId("password-submit").click();
    await expect(page.getByTestId("sign-in-page")).toHaveCount(0);
    await expect(page.getByTestId("whoami")).toHaveText("rita");
    expect(await page.evaluate(() => sessionStorage.getItem("apelora.apiToken"))).toBe(SESSION);
    expect(bodyOf(calls, "POST /api/v1/sign-in/password").at(-1)).toEqual({ username: "rita", password: "right password here" });
  });

  test("next to an identity provider, the form sits below the provider button after an 'or'", async ({ page }) => {
    await signedOut(page, { providers: [{ issuer: "https://id.example.test", clientId: "c", scopes: ["openid"], label: "Example Corp" }] });
    await page.goto("/");
    const card = page.getByTestId("sign-in-page");
    const button = await card.getByTestId("sign-in-oidc").boundingBox();
    const form = await card.getByTestId("password-sign-in").boundingBox();
    expect(button!.y).toBeLessThan(form!.y);
    await expect(card.getByText("or", { exact: true })).toBeVisible();
    await expect(card.getByTestId("password-submit")).toHaveClass(/btn-outline/);
    await expect(card.getByTestId("username-input")).not.toBeFocused();
  });

  test("on a phone the sign-in page fits without sideways scrolling and has no serious axe violations", async ({ browser }) => {
    const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
    const page = await ctx.newPage();
    await page.route("**/api/**", (route) => route.abort());
    await signedOut(page);
    await page.goto("/");
    await expect(page.getByTestId("password-sign-in")).toBeVisible();
    expect(await noSidewaysScroll(page)).toBe(true);
    expect(await serious(page)).toEqual([]);
    await ctx.close();
  });
});

test.describe("set-password page", () => {
  test("checks length and that both match, shows the server's rule, then leads to sign-in", async ({ page }) => {
    let tries = 0;
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/whoami": error(401, "unauthenticated", "a bearer token is required"),
      "GET /api/v1/sign-in": options(),
      "POST /api/v1/sign-in/setup": () => (++tries === 1 ? { ...error(400, "weak_password", "Use 12 to 128 characters, not your username. A few unrelated words work well."), delayMs: 200 } : { status: 204, delayMs: 200 }),
    });
    await page.goto(`/#/set-password?token=${SETUP}`);
    const card = page.getByTestId("set-password-page");
    await expect(card.getByRole("heading", { level: 1 })).toHaveText("Choose your password");
    await expect(card.getByTestId("new-password")).toHaveAttribute("autocomplete", "new-password");
    await expect(card.getByTestId("new-password-help")).toContainText("Use 12 to 128 characters");

    await card.getByTestId("new-password").fill("short one");
    await card.getByTestId("set-password-submit").click();
    await expect(card.getByTestId("new-password-help")).toHaveText("This has 9 characters. Use at least 12.");
    await expect(card.getByTestId("new-password")).toBeFocused();

    await card.getByTestId("new-password").fill("a long enough phrase");
    await card.getByTestId("new-password-again").fill("a long enough phrasE");
    await card.getByTestId("set-password-submit").click();
    await expect(card.getByTestId("new-password-again-error")).toContainText("The two passwords are different");
    expect(countOf(calls, "POST /api/v1/sign-in/setup")).toBe(0);

    await card.getByTestId("new-password-again").fill("a long enough phrase");
    await card.getByTestId("set-password-submit").click();
    await expect(card.getByTestId("new-password-help")).toContainText("not your username");
    await card.getByTestId("set-password-submit").click();
    await expect(card.getByRole("heading", { level: 1 })).toHaveText("Your password is set");
    expect(bodyOf(calls, "POST /api/v1/sign-in/setup").at(-1)).toEqual({ token: SETUP, password: "a long enough phrase" });

    await card.getByTestId("set-password-sign-in").click();
    await expect(page.getByTestId("sign-in-page")).toBeVisible();
    expect(page.url()).not.toContain("token=");
  });

  test("a spent or expired link says so and offers sign-in; the page fits a phone and passes axe", async ({ browser }) => {
    const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
    const page = await ctx.newPage();
    await page.route("**/api/**", (route) => route.abort());
    await stub(page, {
      ...base(),
      "GET /api/v1/whoami": error(401, "unauthenticated", "a bearer token is required"),
      "GET /api/v1/sign-in": options(),
      "POST /api/v1/sign-in/setup": error(400, "setup_invalid", "This setup link has been used, has expired or is not known. Ask an administrator for a new one."),
    });
    await page.goto(`/#/set-password?token=${SETUP}`);
    await expect(page.getByTestId("set-password-form")).toBeVisible();
    expect(await noSidewaysScroll(page)).toBe(true);
    expect(await serious(page)).toEqual([]);
    await page.getByTestId("new-password").fill("a long enough phrase");
    await page.getByTestId("new-password-again").fill("a long enough phrase");
    await page.getByTestId("set-password-submit").click();
    await expect(page.getByTestId("set-password-link-error")).toContainText("Ask an administrator for a new one.");
    await ctx.close();
  });
});

test.describe("users", () => {
  test("creating a local account shows the setup link once; it is gone after closing", async ({ page }) => {
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/users": items([ALEX, SAM, LOCAL]),
      "POST /api/v1/identity/local-accounts": { json: { user: { ...LOCAL, id: uid(6), username: "nora", displayName: "Nora New" }, setupToken: SETUP, setupExpiresAt: "2026-10-22T09:00:00Z" }, delayMs: 200 },
    });
    await page.goto("/#/identity/users");
    await expect(page.getByTestId("user-row").filter({ hasText: "Rita Local" }).getByTestId("local-badge")).toHaveText("Local account");
    await expect(page.getByTestId("user-row").filter({ hasText: "Sam Support" }).getByTestId("local-badge")).toHaveCount(0);

    await page.getByTestId("local-account-open").click();
    const dialog = page.getByTestId("local-account-dialog");
    await dialog.getByTestId("local-username").fill("Not Valid");
    await dialog.getByTestId("local-account-create").click();
    await expect(dialog.getByTestId("local-username")).toHaveAttribute("aria-invalid", "true");
    expect(countOf(calls, "POST /api/v1/identity/local-accounts")).toBe(0);
    await dialog.getByTestId("local-username").fill(" Nora ");
    await dialog.getByTestId("local-display-name").fill("Nora New");
    await dialog.getByTestId("local-account-create").click();

    const secret = page.getByTestId("secret-dialog");
    await expect(secret.getByTestId("secret-value")).toHaveValue(`${new URL(page.url()).origin}/#/set-password?token=${SETUP}`);
    await expect(secret).toContainText("Send it to Nora New");
    expect(bodyOf(calls, "POST /api/v1/identity/local-accounts")).toEqual([{ username: "nora", displayName: "Nora New", email: null }]);
    await secret.getByTestId("secret-close").click();
    await expect(page.getByTestId("secret-value")).toHaveCount(0);
    expect(await page.content()).not.toContain(SETUP);
    await expect(page.getByTestId("users-notice")).toContainText("Nora New was created");
  });

  test("resetting a local account asks for the username, then shows a new link", async ({ page }) => {
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/users": items([ALEX, SAM, LOCAL]),
      [`POST /api/v1/identity/local-accounts/${LOCAL.id}/reset`]: { json: { setupToken: SETUP, setupExpiresAt: "2026-10-22T09:00:00Z" } },
    });
    await page.goto("/#/identity/users");
    await expect(page.getByTestId("user-row").filter({ hasText: "Sam Support" }).getByTestId("local-reset")).toHaveCount(0);
    await page.getByTestId("user-row").filter({ hasText: "Rita Local" }).getByTestId("local-reset").click();
    const confirm = page.locator("dialog[open]").getByTestId("confirm-dialog");
    await expect(confirm.getByTestId("confirm-action")).toBeDisabled();
    await confirm.getByTestId("confirm-input").fill("rita");
    await confirm.getByTestId("confirm-action").click();
    await expect(page.getByTestId("secret-value")).toHaveValue(new RegExp(`#/set-password\\?token=${SETUP}$`));
    expect(countOf(calls, `POST /api/v1/identity/local-accounts/${LOCAL.id}/reset`)).toBe(1);
    await page.getByTestId("secret-close").click();
    await expect(page.getByTestId("users-notice")).toContainText("signed out everywhere");
  });

  test("the Users page fits a phone with the new actions", async ({ browser }) => {
    const ctx = await browser.newContext({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
    const page = await ctx.newPage();
    await page.route("**/api/**", (route) => route.abort());
    await stub(page, { ...base(), "GET /api/v1/identity/users": items([ALEX, SAM, LOCAL]) });
    await page.goto("/#/identity/users");
    await expect(page.getByTestId("local-account-open")).toBeVisible();
    await expect(page.getByTestId("user-menu")).toBeVisible();
    expect(await noSidewaysScroll(page)).toBe(true);
    await ctx.close();
  });
});

test.describe("user menu", () => {
  test("sign out ends the session on the server, then forgets the token in the tab", async ({ page }) => {
    const calls = await stub(page, {
      ...base({ ...whoami(["user.view"], { name: "rita" }), localAccount: true }),
      "GET /api/v1/sign-in": options(),
      "GET /api/v1/cluster": { json: { source: { provider: "mock", observedAt: NOW }, server: { clustered: false, serverVersion: null }, members: [] } },
      "POST /api/v1/sign-out": { status: 204, delayMs: 200 },
    });
    await page.addInitScript((t) => sessionStorage.setItem("apelora.apiToken", t), SESSION);
    await page.goto("/");
    await page.getByTestId("user-menu").click();
    await expect(page.getByRole("menuitem", { name: "Change password" })).toBeVisible();
    await page.getByRole("menuitem", { name: "Sign out" }).click();
    await expect(page.getByTestId("sign-in-page")).toBeVisible();
    await expect(page.getByTestId("sign-in-error")).toHaveText("You signed out.");
    expect(countOf(calls, "POST /api/v1/sign-out")).toBe(1);
    expect(await page.evaluate(() => sessionStorage.getItem("apelora.apiToken"))).toBeNull();
  });

  test("change password: checked on the page, the server's answer next to the field, then done", async ({ page }) => {
    let attempt = 0;
    const calls = await stub(page, {
      ...base({ ...whoami(["user.view"], { name: "rita" }), localAccount: true }),
      "GET /api/v1/cluster": { json: { source: { provider: "mock", observedAt: NOW }, server: { clustered: false, serverVersion: null }, members: [] } },
      "PUT /api/v1/me/password": () => (++attempt === 1 ? { ...error(400, "wrong_password", "The current password is incorrect."), delayMs: 200 } : { status: 204, delayMs: 200 }),
    });
    await page.goto("/");
    await page.getByTestId("user-menu").click();
    await page.getByRole("menuitem", { name: "Change password" }).click();
    const dialog = page.getByTestId("change-password-dialog");
    await expect(dialog.getByTestId("password-current")).toBeFocused();
    expect(await serious(page)).toEqual([]);
    await dialog.getByTestId("password-current").fill("old password here");
    await dialog.getByTestId("password-next").fill("a brand new phrase");
    await dialog.getByTestId("password-again").fill("a different phrase");
    await dialog.getByTestId("change-password-submit").click();
    await expect(dialog.getByTestId("password-again-help")).toContainText("different");
    expect(countOf(calls, "PUT /api/v1/me/password")).toBe(0);
    await dialog.getByTestId("password-again").fill("a brand new phrase");
    await dialog.getByTestId("change-password-submit").click();
    await expect(dialog.getByTestId("password-current-help")).toHaveText("The current password is incorrect.");
    await expect(dialog.getByTestId("password-current")).toBeFocused();
    await dialog.getByTestId("change-password-submit").click();
    await expect(dialog.getByTestId("change-password-done")).toContainText("signed out everywhere else");
    expect(bodyOf(calls, "PUT /api/v1/me/password").at(-1)).toEqual({ current: "old password here", new: "a brand new phrase" });
  });

  test("people who sign in another way get their tokens, sign-ins and Sign out, not Change password", async ({ page }) => {
    await stub(page, { ...base(), "GET /api/v1/cluster": { json: { source: { provider: "mock", observedAt: NOW }, server: { clustered: false, serverVersion: null }, members: [] } } });
    await page.goto("/");
    await page.getByTestId("user-menu").click();
    await expect(page.getByRole("menuitem")).toHaveText(["Your tokens", "Your sign-ins", "Sign out"]);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("user-menu-list")).toHaveCount(0);
    await expect(page.getByTestId("user-menu")).toBeFocused();
  });
});

test.describe("sign-in settings", () => {
  test("local accounts are turned on, and turning them off shows the server's refusal", async ({ page }) => {
    let enabled = false;
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/sign-in": () => ({ json: { providers: [], apiTokens: { enabled: false, lockedByEnvironment: false }, localAccounts: { enabled, ready: enabled ? 2 : 0, pending: enabled ? 1 : 0 } } }),
      "PUT /api/v1/identity/sign-in/local-accounts": (c) => {
        if ((c.body as { enabled: boolean }).enabled) { enabled = true; return { json: { enabled: true, ready: 2, pending: 1 } }; }
        return error(409, "would_lock_out", "nothing was changed: API tokens are off and no identity provider answers, so without local accounts nobody could sign in");
      },
    });
    await page.goto("/#/identity/sign-in");
    const section = page.getByTestId("local-accounts-setting");
    await expect(section).toContainText("Off");
    await section.getByTestId("local-accounts-toggle").click();
    await expect(section.getByTestId("local-accounts-summary")).toHaveText("People sign in with a username and password kept by Apelora. 2 accounts can sign in; 1 is waiting for setup.");
    await section.getByTestId("local-accounts-toggle").click();
    await page.locator("dialog[open]").getByTestId("confirm-action").click();
    await expect(section.getByTestId("local-accounts-error")).toContainText("without local accounts nobody could sign in");
    expect(bodyOf(calls, "PUT /api/v1/identity/sign-in/local-accounts")).toEqual([{ enabled: true }, { enabled: false }]);
  });
});
