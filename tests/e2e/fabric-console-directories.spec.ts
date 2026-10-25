import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import { NOW, base, error, stub, uid, type Call } from "./identity-fixtures";

// Identity & Access → Sign-in → Directories against a fully stubbed control plane: nothing
// reaches a real server or a real directory.

test.beforeEach(async ({ page }) => { await page.route("**/api/**", (route) => route.abort()); });

const bodyOf = (calls: Call[], line: string) => calls.filter((c) => `${c.method} ${c.path}` === line).map((c) => c.body);
const countOf = (calls: Call[], line: string) => calls.filter((c) => `${c.method} ${c.path}` === line).length;
const openDialog = (page: Page) => page.locator("dialog[open]");

const LAB = {
  id: uid(300), name: "Staff directory", url: "ldaps://ldap.example.org", startTls: false, caPem: null,
  bindTemplate: "uid={username},ou=people,dc=example,dc=org", baseDn: null, userFilter: null, idAttribute: "entryUUID",
  usernameAttribute: "uid", emailAttribute: "mail", nameAttribute: "cn", groupAttribute: "memberOf", groupBaseDn: null, groupFilter: null,
  groupNameAttribute: "cn", enabled: true, reachable: true, problem: null, error: null, updatedBy: "user:x", updatedAt: NOW,
};
const DOWN = { ...LAB, id: uid(301), name: "Branch office", url: "ldap://ldap.branch.example.org:389", startTls: true, groupAttribute: null, groupBaseDn: "ou=groups,dc=branch", groupFilter: "(member={dn})", reachable: false, problem: "certificate-untrusted", error: "the directory's certificate is not trusted: add the certificate authority that signed it (PEM)" };
const OLD = { ...LAB, id: uid(302), name: "Old forest", url: "ldaps://old.example.org", enabled: false, reachable: false, groupAttribute: null };
const settings = (directories: unknown[]) => ({ json: { providers: [], apiTokens: { enabled: true, lockedByEnvironment: false }, localAccounts: { enabled: false, ready: 0, pending: 0 }, directories } });
const CHECK_OK = { json: { ok: true, problem: null, message: "signed in as ada and read the entry with 2 groups", entry: { dn: "uid=ada,ou=people,dc=example,dc=org", subject: "0b6a7c1e-1111-4c1a-9f00-000000000001", username: "ada", email: "ada@example.org", displayName: "Ada Admin", groups: ["cn=dev-admins,ou=groups,dc=example,dc=org", "cn=dev-operators,ou=groups,dc=example,dc=org"], mappedGroups: ["Platform admins"] } }, delayMs: 300 };

test("lists directories with their status in words", { tag: "@smoke" }, async ({ page }) => {
  await stub(page, { ...base(), "GET /api/v1/identity/sign-in": settings([LAB, DOWN, OLD]) });
  await page.goto("/#/identity/sign-in");
  const rows = page.getByTestId("directory-row");
  await expect(rows).toHaveCount(3);
  await expect(page.getByRole("heading", { name: /Directories/ })).toContainText("(3)");
  await expect(rows.nth(0)).toContainText("Answers");
  await expect(rows.nth(0)).toContainText("Groups from memberOf");
  await expect(rows.nth(1)).toContainText("ldap://ldap.branch.example.org:389 (StartTLS)");
  await expect(rows.nth(1)).toContainText("The certificate is not trusted");
  await expect(rows.nth(1)).toContainText("add the certificate authority that signed it");
  await expect(rows.nth(1)).toContainText("Groups by search under ou=groups,dc=branch");
  await expect(rows.nth(2)).toContainText("Off");
  await expect(rows.nth(2).getByRole("button", { name: "Turn on Old forest" })).toBeVisible();
});

test("an empty list says what a directory is for, and an old server without directories still shows the page", async ({ page }) => {
  await stub(page, { ...base(), "GET /api/v1/identity/sign-in": { json: { providers: [], apiTokens: { enabled: true, lockedByEnvironment: false } } } });
  await page.goto("/#/identity/sign-in");
  await expect(page.getByTestId("no-directories")).toContainText("existing LDAP username and password");
  await expect(page.getByTestId("directory-add")).toBeVisible();
});

test("adding refuses plain LDAP, checks a test sign-in without saving the password, then saves the settings", async ({ page }) => {
  let list: unknown[] = [];
  const calls = await stub(page, {
    ...base(),
    "GET /api/v1/identity/sign-in": () => settings(list),
    "POST /api/v1/identity/sign-in/directories/check": CHECK_OK,
    "POST /api/v1/identity/sign-in/directories": () => { list = [LAB]; return { status: 201, json: LAB, delayMs: 200 }; },
  });
  await page.goto("/#/identity/sign-in");
  await page.getByTestId("directory-add").click();
  const dialog = page.getByTestId("directory-dialog");
  await dialog.getByTestId("directory-name").fill("Staff directory");
  await dialog.getByTestId("directory-url").fill("ldap://ldap.example.org");
  await expect(dialog.getByTestId("directory-url-error")).toContainText("Plain LDAP sends passwords unencrypted");
  await expect(dialog.getByTestId("directory-save")).toBeDisabled();
  await dialog.getByTestId("directory-start-tls").check();
  await expect(dialog.getByTestId("directory-url-error")).toHaveCount(0);
  await dialog.getByTestId("directory-url").fill("ldaps://ldap.example.org");
  await expect(dialog.getByTestId("directory-start-tls")).toHaveCount(0);
  await dialog.getByTestId("directory-bind").fill("uid=fixed,ou=people,dc=example,dc=org");
  await expect(dialog.getByText("Put {username} where the person's username goes.")).toBeVisible();
  await expect(dialog.getByTestId("directory-save")).toBeDisabled();
  await dialog.getByTestId("directory-bind").fill("uid={username},ou=people,dc=example,dc=org");

  // The test sign-in: Enter in the password field runs the check, not the save.
  await expect(dialog.getByTestId("directory-check-run")).toBeDisabled();
  await dialog.getByTestId("directory-check-username").fill(" ada ");
  await dialog.getByTestId("directory-check-password").fill("test-password-9");
  await dialog.getByTestId("directory-check-password").press("Enter");
  await expect(dialog.getByTestId("directory-check-run")).toHaveText("Checking…");
  const ok = dialog.getByTestId("directory-check-ok");
  await expect(ok).toContainText("Signed in as Ada Admin");
  await expect(ok).toContainText("cn=dev-admins,ou=groups,dc=example,dc=org, cn=dev-operators,ou=groups,dc=example,dc=org");
  await expect(dialog.getByTestId("directory-check-mapped")).toHaveText("Platform admins");
  expect(countOf(calls, "POST /api/v1/identity/sign-in/directories")).toBe(0);

  await dialog.getByTestId("directory-advanced").locator("summary").click();
  await dialog.getByLabel("Search groups under").fill("ou=groups,dc=example,dc=org");
  await dialog.getByTestId("directory-save").click();
  await expect(dialog).toBeHidden();
  await expect(page.getByTestId("directory-row")).toHaveCount(1);

  const settingsSent = {
    name: "Staff directory", url: "ldaps://ldap.example.org", startTls: false, caPem: null, bindTemplate: "uid={username},ou=people,dc=example,dc=org",
    baseDn: null, userFilter: null, idAttribute: "entryUUID", usernameAttribute: "uid", emailAttribute: "mail", nameAttribute: "cn",
    groupAttribute: "memberOf", groupBaseDn: null, groupFilter: null, groupNameAttribute: "cn", enabled: true,
  };
  // Not saved yet: no directory, so no group mappings to show.
  expect(bodyOf(calls, "POST /api/v1/identity/sign-in/directories/check")).toEqual([{ settings: settingsSent, directory: null, username: "ada", password: "test-password-9" }]);
  const saved = bodyOf(calls, "POST /api/v1/identity/sign-in/directories");
  expect(saved).toEqual([{ ...settingsSent, groupBaseDn: "ou=groups,dc=example,dc=org" }]);
  expect(JSON.stringify(saved)).not.toContain("test-password-9");

  // Opening the dialog again starts with an empty test password.
  await page.getByTestId("directory-add").click();
  await expect(dialog.getByTestId("directory-check-password")).toHaveValue("");
  await expect(dialog.getByTestId("directory-check-ok")).toHaveCount(0);
});

test("a failed check and a refused save are explained inside the dialog", async ({ page }) => {
  await stub(page, {
    ...base(),
    "GET /api/v1/identity/sign-in": settings([]),
    "POST /api/v1/identity/sign-in/directories/check": { json: { ok: false, problem: "certificate-name-mismatch", message: "the directory's certificate does not name the host in the URL: use the host name the certificate was issued for", entry: null }, delayMs: 300 },
    "POST /api/v1/identity/sign-in/directories": error(409, "directory_exists", "another directory has this name or address"),
  });
  await page.goto("/#/identity/sign-in");
  await page.getByTestId("directory-add").click();
  const dialog = page.getByTestId("directory-dialog");
  await dialog.getByTestId("directory-name").fill("Staff directory");
  await dialog.getByTestId("directory-url").fill("ldaps://10.0.0.5");
  await dialog.getByTestId("directory-bind").fill("{username}@example.org");
  await dialog.getByTestId("directory-check-username").fill("ada");
  await dialog.getByTestId("directory-check-password").fill("x");
  await dialog.getByTestId("directory-check-run").click();
  await expect(dialog.getByTestId("directory-check-failed")).toContainText("The certificate is for another host name");
  await expect(dialog.getByTestId("directory-check-failed")).toContainText("use the host name the certificate was issued for");
  await dialog.getByTestId("directory-save").click();
  await expect(dialog.getByTestId("directory-save-error")).toHaveText("another directory has this name or address");
  await expect(dialog).toBeVisible();
});

test("row actions: check from the edit dialog, edit, turn off, and remove with the name typed", async ({ page }) => {
  const calls = await stub(page, {
    ...base(),
    "GET /api/v1/identity/sign-in": settings([LAB]),
    "POST /api/v1/identity/sign-in/directories/check": CHECK_OK,
    [`PUT /api/v1/identity/sign-in/directories/${LAB.id}`]: { json: LAB, delayMs: 200 },
    [`DELETE /api/v1/identity/sign-in/directories/${LAB.id}`]: { status: 204 },
  });
  await page.goto("/#/identity/sign-in");

  // Checking a saved directory from its edit dialog, then cancelling, changes nothing.
  await page.getByRole("button", { name: "Edit Staff directory" }).click();
  const check = page.getByTestId("directory-dialog");
  await check.getByTestId("directory-check-username").fill("ada");
  await check.getByTestId("directory-check-password").fill("pw-1");
  await check.getByTestId("directory-check-run").click();
  await expect(check.getByTestId("directory-check-ok")).toContainText("Signed in as Ada Admin");
  const checked = bodyOf(calls, "POST /api/v1/identity/sign-in/directories/check")[0] as { settings: { url: string }; directory: string | null };
  expect(checked.settings.url).toBe(LAB.url);
  expect(checked.directory, "a saved directory's mappings are the ones shown").toBe(LAB.id);
  await check.getByRole("button", { name: "Cancel" }).click();
  await expect(check).toBeHidden();
  expect(countOf(calls, `PUT /api/v1/identity/sign-in/directories/${LAB.id}`)).toBe(0);

  await page.getByRole("button", { name: "Edit Staff directory" }).click();
  const dialog = page.getByTestId("directory-dialog");
  await expect(dialog.getByTestId("directory-url")).toHaveValue(LAB.url);
  await expect(dialog.getByTestId("directory-bind")).toHaveValue(LAB.bindTemplate);
  await dialog.getByTestId("directory-ca").fill("-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----");
  await dialog.getByTestId("directory-save").click();
  await expect(dialog).toBeHidden();

  await page.getByRole("button", { name: "Turn off Staff directory" }).click();
  const puts = bodyOf(calls, `PUT /api/v1/identity/sign-in/directories/${LAB.id}`) as Array<Record<string, unknown>>;
  await expect.poll(() => puts.length).toBe(2);
  expect(puts[0]).toMatchObject({ name: LAB.name, url: LAB.url, caPem: "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----", enabled: true });
  expect(puts[1]).toMatchObject({ name: LAB.name, enabled: false, groupAttribute: "memberOf" });

  await page.getByRole("button", { name: "Remove Staff directory" }).click();
  await expect(openDialog(page).getByTestId("confirm-dialog")).toContainText("Their accounts and roles in Apelora stay");
  await expect(openDialog(page).getByTestId("confirm-action")).toBeDisabled();
  await openDialog(page).getByRole("textbox").fill("Staff directory");
  await openDialog(page).getByTestId("confirm-action").click();
  await expect.poll(() => countOf(calls, `DELETE /api/v1/identity/sign-in/directories/${LAB.id}`)).toBe(1);
  expect(bodyOf(calls, `DELETE /api/v1/identity/sign-in/directories/${LAB.id}`)).toEqual([{ confirm: "Staff directory" }]);
});

test.describe("phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });

  test("fits without sideways scrolling and keeps every row action reachable", async ({ page }) => {
    await stub(page, { ...base(), "GET /api/v1/identity/sign-in": settings([LAB, DOWN, OLD]) });
    await page.goto("/#/identity/sign-in");
    await expect(page.getByTestId("directory-row")).toHaveCount(3);
    const overflow = () => page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    expect(await overflow()).toBeLessThanOrEqual(0);
    for (const name of ["Edit", "Turn off", "Remove"]) await expect(page.getByRole("button", { name: `${name} Staff directory` })).toBeInViewport();
    await page.getByRole("button", { name: "Edit Branch office" }).click();
    const dialog = page.getByTestId("directory-dialog");
    await dialog.getByTestId("directory-advanced").locator("summary").click();
    expect(await dialog.evaluate((el) => el.scrollWidth - el.clientWidth), "dialog").toBeLessThanOrEqual(0);
    expect(await overflow()).toBeLessThanOrEqual(0);
  });
});

test("the section and its dialogs have no serious or critical axe violations", async ({ page }) => {
  await stub(page, { ...base(), "GET /api/v1/identity/sign-in": settings([LAB, DOWN, OLD]), "POST /api/v1/identity/sign-in/directories/check": CHECK_OK });
  await page.emulateMedia({ reducedMotion: "reduce" });
  const serious = async () => (await page.addStyleTag({ content: "*, *::before, *::after { transition: none !important; animation: none !important; }" }), await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze()).violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id}: ${v.nodes.slice(0, 3).map((n) => n.target.join(" ")).join(" | ")}`);
  await page.goto("/#/identity/sign-in");
  await expect(page.getByTestId("directory-row")).toHaveCount(3);
  expect(await serious()).toEqual([]);
  await page.getByRole("button", { name: "Edit Staff directory" }).click();
  const dialog = page.getByTestId("directory-dialog");
  await dialog.getByTestId("directory-advanced").locator("summary").click();
  await dialog.getByTestId("directory-check-username").fill("ada");
  await dialog.getByTestId("directory-check-password").fill("pw");
  await dialog.getByTestId("directory-check-run").click();
  await expect(dialog.getByTestId("directory-check-ok")).toBeVisible();
  expect(await serious()).toEqual([]);
});
