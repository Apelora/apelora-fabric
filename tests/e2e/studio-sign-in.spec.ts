import { expect, test, type Page } from "@playwright/test";

// The Studio's sign-in with the organisation's identity provider, against a stubbed control plane
// and a stubbed provider (nothing reaches a real server).

const issuer = "https://id.example.test/realms/apelora";
const at = "eyJhbGciOiJSUzI1NiJ9.studio.token";
const cors = { "access-control-allow-origin": "*" };

async function controlPlane(page: Page, opts: { apiTokens: boolean; accept: boolean }) {
  let tokenRequests = 0;
  await page.route("**/api/**", (route) => {
    const p = new URL(route.request().url()).pathname;
    if (p === "/api/v1/health") return route.fulfill({ json: { status: "ok", service: "control-plane", version: "0", apiVersion: "v1", provider: "incus", auth: "tokens", incus: { connected: true, apiVersion: "1", serverVersion: "7" } } });
    if (p === "/api/v1/sign-in") return route.fulfill({ json: { providers: [{ issuer, clientId: "apelora-console", scopes: ["openid"], label: "Example Corp" }], apiTokens: opts.apiTokens } });
    if (p === "/api/v1/whoami") {
      return opts.accept && route.request().headers()["authorization"] === `Bearer ${at}`
        ? route.fulfill({ json: { name: "sam", role: null, permissions: [], deployProjects: [], capabilities: [], platformPermissions: [] } })
        : route.fulfill({ status: 401, json: { error: { code: "invalid_token", message: "the token lives longer than Apelora accepts" } } });
    }
    if (p === "/api/v1/applications") return route.fulfill({ json: { items: [], observedAt: "2026-10-21T00:00:00Z", stale: false } });
    return route.fulfill({ status: 404, json: { error: { code: "not_found", message: `no stub for ${p}` } } });
  });
  await page.route(`${issuer}/.well-known/openid-configuration`, (route) => route.fulfill({ headers: cors, json: { issuer, authorization_endpoint: `${issuer}/auth`, token_endpoint: `${issuer}/token` } }));
  await page.route(`${issuer}/auth?**`, (route) => {
    const u = new URL(route.request().url());
    return route.fulfill({ status: 302, headers: { location: `${u.searchParams.get("redirect_uri")}?code=the-code&state=${u.searchParams.get("state")}` } });
  });
  await page.route(`${issuer}/token`, async (route) => {
    tokenRequests += 1;
    await new Promise((r) => setTimeout(r, 400)); // a real provider takes a moment: exchanged once
    return route.fulfill({ headers: cors, json: { access_token: at, token_type: "Bearer" } });
  });
  return () => tokenRequests;
}

test("the Studio signs in with the identity provider and comes back to the page it started from", async ({ page }) => {
  const exchanges = await controlPlane(page, { apiTokens: true, accept: true });
  await page.goto("/studio/applications");
  await expect(page.getByTestId("source-chip")).toHaveAttribute("data-source", "local");
  await page.getByTestId("source-chip").click();
  const dialog = page.getByTestId("connect-dialog");
  await expect(dialog.getByTestId("connect-oidc")).toHaveText("Continue with Example Corp");
  await expect(dialog.getByTestId("connect-token")).toHaveCount(0);
  await dialog.getByTestId("connect-oidc").click();
  await expect(page.getByTestId("source-chip")).toHaveAttribute("data-source", "api");
  await expect(page.getByTestId("source-chip")).toContainText("Control plane · sam");
  expect(new URL(page.url()).pathname).toBe("/studio/applications");
  expect(page.url()).not.toContain("code=");
  expect(exchanges()).toBe(1);
  expect(await page.evaluate(() => sessionStorage.getItem("apelora.apiToken"))).toBe(at);
});

test("a provider token the control plane refuses says why; a token can still be pasted", async ({ page }) => {
  await controlPlane(page, { apiTokens: true, accept: false });
  await page.goto("/studio/applications");
  await page.getByTestId("source-chip").click();
  await page.getByTestId("connect-dialog").getByTestId("connect-oidc").click();
  await expect(page.getByTestId("source-chip")).toHaveAttribute("data-source", "local");
  await expect(page.getByTestId("source-chip")).toHaveAttribute("title", "You signed in, but Apelora did not accept the identity provider's token: the token lives longer than Apelora accepts");
  expect(await page.evaluate(() => sessionStorage.getItem("apelora.apiToken"))).toBeNull();
  await page.getByTestId("source-chip").click();
  const dialog = page.getByTestId("connect-dialog");
  await dialog.getByTestId("connect-use-token").click();
  await expect(dialog.getByTestId("connect-token")).toBeFocused();
  await dialog.getByTestId("connect-submit").click();
  await expect(dialog.getByTestId("connect-error")).toHaveText("Paste an access token first.");
});
