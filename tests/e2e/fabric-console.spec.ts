import { expect, test, type Page } from "@playwright/test";

import { member as serverMember, members as serverMembers } from "./servers-fixtures";

// The console must report the control plane, Incus and the data's origin honestly: never
// "connected" or unlabelled data unless the API says it came from Incus.
type Provider = "mock" | "incus";
const now = "2026-10-20T00:00:00Z";
// The public health answer is coarse: no versions, no Incus error text (those need signing in).
const health = (provider: Provider, connected: boolean) => ({
  status: "ok", service: "control-plane", apiVersion: "v1", provider, auth: "disabled-development", incus: { connected }, checkedAt: now,
});
const member = (name: string, status = "Online") => ({ name, url: `https://${name}:8443`, roles: ["database"], status, message: "", architecture: "x86_64", failureDomain: "rack-a", database: true, groups: [] });
const cluster = (provider: Provider, names: string[]) => ({ source: { provider, observedAt: now }, server: { clustered: true, serverVersion: provider === "incus" ? "7.0.2" : null }, members: names.map((n) => member(n)) });
const instances = (provider: Provider) => ({ source: { provider, observedAt: now }, items: [
  { name: `${provider}-web-01`, project: "default", kind: "container", status: "Running", location: "m1", architecture: "x86_64", description: "" },
  { name: `${provider}-vm-01`, project: "default", kind: "virtual-machine", status: "Stopped", location: "m2", architecture: "x86_64", description: "" },
] });

async function api(page: Page, routes: Record<string, { status?: number; json: unknown } | "abort">) {
  for (const [path, r] of Object.entries(routes)) {
    await page.route(`**${path}`, (route) => (r === "abort" ? route.abort() : route.fulfill({ status: r.status ?? 200, json: r.json })));
  }
}

test.describe("fabric console", () => {
  // Never reach a real control plane: anything a test does not stub fails like a network error.
  // (Routes registered later take precedence over this one.)
  test.beforeEach(async ({ page }) => { await page.route("**/api/**", (route) => route.abort()); });

  test("control plane down: says so, shows how to start it, retries into mock data that is labelled", { tag: "@smoke" }, async ({ page }) => {
    let up = false;
    await page.route("**/api/v1/health", (route) => (up ? route.fulfill({ json: health("mock", false) }) : route.abort()));
    await api(page, { "/api/v1/cluster": { json: cluster("mock", ["mock-member-01", "mock-member-02", "mock-member-03"]) }, "/api/v1/instances": { json: instances("mock") } });
    await page.goto("/");
    const status = page.getByTestId("status");
    await expect(status).toContainText("Not reachable");
    await expect(status).toContainText("Unknown");
    await expect(page.getByTestId("empty-state")).toContainText("cargo run -p apelora-control-plane");
    await expect(page.getByTestId("members")).toHaveCount(0);
    up = true;
    await page.getByRole("button", { name: "Check again" }).click();
    await expect(status).toContainText("Running · mock data");
    await expect(page.getByTestId("mock-badge")).toHaveText("Mock data");
    await expect(page.getByTestId("empty-state")).toContainText("APELORA_INCUS_URL");
    await expect(page.getByTestId("member-row")).toHaveCount(3);
    for (const note of await page.getByTestId("source-note").all()) await expect(note).toHaveText("Mock data · not a real cluster");
    await page.getByRole("navigation", { name: "Console" }).getByRole("link", { name: "Instances" }).click();
    await expect(page).toHaveURL(/#\/instances$/);
    await expect(page.getByTestId("instance-row")).toHaveCount(2);
    await expect(page.getByTestId("instance-row").nth(1)).toContainText("Virtual machine");
    await expect(page.getByTestId("source-note")).toHaveText("Mock data · not a real cluster");
  });

  test("Incus connected: real data is attributed to Incus, no mock badge or empty state", { tag: "@smoke" }, async ({ page }) => {
    await api(page, { "/api/v1/health": { json: health("incus", true) }, "/api/v1/cluster": { json: cluster("incus", ["node-1", "node-2"]) }, "/api/v1/instances": { json: instances("incus") } });
    await page.goto("/");
    await expect(page.getByTestId("status")).toContainText("Connected · Incus 7.0.2");
    await expect(page.getByTestId("mock-badge")).toHaveText("Connected");
    await expect(page.getByTestId("empty-state")).toHaveCount(0);
    await expect(page.getByTestId("member-row")).toHaveCount(2);
    await expect(page.getByTestId("source-note").first()).toContainText("From Incus · observed");
  });

  test("Incus configured but unreachable: the API's error is shown, not stale or invented data", async ({ page }) => {
    const message = "cannot reach the Incus server: connection refused";
    await api(page, {
      "/api/v1/health": { json: health("incus", false) },
      "/api/v1/cluster": { status: 502, json: { error: { code: "incus_unreachable", message } } },
      "/api/v1/instances": { status: 502, json: { error: { code: "incus_unreachable", message } } },
    });
    await page.goto("/");
    await expect(page.getByTestId("status")).toContainText("Unreachable");
    await expect(page.getByTestId("mock-badge")).toHaveText("Not connected");
    await expect(page.getByTestId("empty-state")).toContainText(message);
    await expect(page.getByTestId("members-error")).toHaveText(message);
    await expect(page.getByTestId("member-row")).toHaveCount(0);
    await page.goto("/#/instances");
    await expect(page.getByTestId("instances-error")).toHaveText(message);
  });

  test("token mode: a sign-in page asks for a token, sends it as a Bearer header, keeps it only for the tab", async ({ page }) => {
    const good = "apl_" + "a".repeat(64);
    const guard = (body: unknown) => (route: import("@playwright/test").Route) =>
      route.request().headers()["authorization"] === `Bearer ${good}`
        ? route.fulfill({ json: body })
        : route.fulfill({ status: 401, json: { error: { code: "unauthenticated", message: "an API token is required (Authorization: Bearer <token>)" } } });
    await page.route("**/api/v1/health", (route) => route.fulfill({ json: { ...health("mock", false), auth: "tokens" } }));
    await page.route("**/api/v1/sign-in", (route) => route.fulfill({ json: { providers: [], apiTokens: true } }));
    await page.route("**/api/v1/whoami", guard(whoami("fabric-admin", ADMIN)));
    await page.route("**/api/v1/cluster", guard(cluster("mock", ["mock-member-01"])));
    await page.route("**/api/v1/instances", guard(instances("mock")));
    await page.goto("/");
    const signIn = page.getByTestId("sign-in-page");
    await expect(signIn.getByRole("heading", { level: 1 })).toHaveText("Sign in to the Fabric Console");
    await expect(signIn.getByTestId("sign-in-error")).toHaveCount(0);
    await expect(page.getByRole("navigation", { name: "Console" })).toHaveCount(0);
    await expect(page.locator(".btn-primary")).toHaveCount(1);
    await expect(page.getByTestId("sign-in-status")).toContainText("showing mock data");

    // A wrong token: said plainly, the field is marked and emptied (a refused secret is not kept on screen).
    await page.getByTestId("token-input").fill("apl_wrong");
    await page.getByTestId("sign-in").getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByTestId("sign-in-error")).toHaveText("That token was not accepted. It may have expired or been revoked.");
    await expect(page.getByTestId("token-input")).toHaveAttribute("aria-invalid", "true");
    await expect(page.getByTestId("token-input")).toHaveValue("");

    await page.getByTestId("token-input").fill(good);
    await page.getByTestId("sign-in").getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByTestId("member-row")).toHaveCount(1);
    await expect(signIn).toHaveCount(0);
    expect(await page.evaluate(() => [sessionStorage.getItem("apelora.apiToken"), localStorage.getItem("apelora.apiToken")])).toEqual([good, null]);
  });

  test("organisation sign-in: the identity provider signs the person in and the token stays in the tab", async ({ page }) => {
    const issuer = "https://id.example.test/realms/apelora";
    const at = "eyJhbGciOiJSUzI1NiJ9.access.token";
    let tokenRequest: URLSearchParams | null = null;
    await page.route("**/api/v1/health", (route) => route.fulfill({ json: { ...health("incus", true), auth: "tokens" } }));
    await page.route("**/api/v1/sign-in", (route) => route.fulfill({ json: { providers: [{ issuer, clientId: "apelora-console", scopes: ["openid", "profile"], label: "Example Corp" }], apiTokens: true } }));
    await page.route("**/api/v1/whoami", (route) => route.request().headers()["authorization"] === `Bearer ${at}`
      ? route.fulfill({ json: whoami("operator", OPERATOR) })
      : route.fulfill({ status: 401, json: { error: { code: "unauthenticated", message: "sign in" } } }));
    await page.route("**/api/v1/cluster", (route) => route.fulfill({ json: cluster("incus", ["node-1"]) }));
    await page.route("**/api/v1/instances", (route) => route.fulfill({ json: instances("incus") }));
    // A provider allows its registered browser client to call it (CORS), as real ones do.
    const cors = { "access-control-allow-origin": "*" };
    await page.route(`${issuer}/.well-known/openid-configuration`, (route) => route.fulfill({ headers: cors, json: { issuer, authorization_endpoint: `${issuer}/auth`, token_endpoint: `${issuer}/token` } }));
    // The provider "signs in" at once and sends the browser back with a code.
    await page.route(`${issuer}/auth?**`, (route) => {
      const u = new URL(route.request().url());
      return route.fulfill({ status: 302, headers: { location: `${u.searchParams.get("redirect_uri")}?code=the-code&state=${u.searchParams.get("state")}` } });
    });
    let tokenRequests = 0;
    await page.route(`${issuer}/token`, async (route) => {
      tokenRequests += 1;
      tokenRequest = new URLSearchParams(route.request().postData() ?? "");
      const pending = tokenRequest; // nonce is checked only when an id_token is returned
      await new Promise((r) => setTimeout(r, 400)); // a real provider takes a moment: the app must wait, once
      return route.fulfill({ headers: cors, json: { access_token: at, token_type: "Bearer", scope: pending.get("scope") ?? "" } });
    });
    await page.goto("/#/instances");
    await expect(page.getByTestId("sign-in-oidc")).toHaveText("Continue with Example Corp");
    await expect(page.getByTestId("sign-in")).toHaveCount(0);
    await page.getByTestId("sign-in-use-token").click();
    await expect(page.getByTestId("sign-in")).toBeVisible();
    await expect(page.locator(".btn-primary")).toHaveCount(1);
    await page.getByTestId("sign-in-oidc").click();
    await expect(page.getByTestId("instance-row")).toHaveCount(2);
    expect(page.url()).not.toContain("code=");
    expect(page.url()).toContain("#/instances");
    await expect(page.getByTestId("sign-in-error")).toHaveCount(0);
    expect(tokenRequests).toBe(1);
    expect(tokenRequest!.get("code")).toBe("the-code");
    expect(tokenRequest!.get("code_verifier")).toMatch(/^[A-Za-z0-9_-]{43}$/);
    expect(await page.evaluate(() => sessionStorage.getItem("apelora.apiToken"))).toBe(at);
  });

  test("sign-in page: several identity providers are offered by name in one row; the illustration is decorative and still for reduced motion", async ({ page }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.emulateMedia({ reducedMotion: "reduce" });
    await page.route("**/api/v1/health", (route) => route.fulfill({ json: { ...health("incus", true), auth: "tokens" } }));
    await page.route("**/api/v1/sign-in", (route) => route.fulfill({ json: { providers: [
      { issuer: "https://id.example.test", clientId: "c", scopes: ["openid"], label: "Example Corp" },
      { issuer: "https://partners.example.test", clientId: "c", scopes: ["openid"], label: "Partner accounts" },
    ], apiTokens: true } }));
    await page.route("**/api/v1/whoami", (route) => route.fulfill({ status: 401, json: { error: { code: "unauthenticated", message: "sign in" } } }));
    await page.goto("/");
    const group = page.getByRole("group", { name: "Sign in with your organisation" });
    await expect(group.getByRole("button")).toHaveText(["Example Corp", "Partner accounts"]);
    await expect(page.getByRole("button", { name: "Sign in with Partner accounts" })).toBeVisible();
    const [a, b] = await Promise.all(["Example Corp", "Partner accounts"].map((n) => group.getByRole("button", { name: `Sign in with ${n}` }).boundingBox()));
    expect(Math.abs(a!.y - b!.y)).toBeLessThan(1);
    expect(a!.width).toBeLessThan(200);
    await expect(page.locator(".btn-primary")).toHaveCount(1);
    await expect(page.locator("aside svg")).toHaveAttribute("aria-hidden", "true");
    await expect(page.locator("aside svg animateMotion")).toHaveCount(0);
  });

  test("sign-in page: an empty token is explained, not sent; no way to sign in says what an administrator does", async ({ page }) => {
    let whoamiCalls = 0;
    let apiTokens = true;
    await page.route("**/api/v1/health", (route) => route.fulfill({ json: { ...health("incus", true), auth: "tokens" } }));
    await page.route("**/api/v1/sign-in", (route) => route.fulfill({ json: { providers: [], apiTokens } }));
    await page.route("**/api/v1/whoami", (route) => { whoamiCalls += 1; return route.fulfill({ status: 401, json: { error: { code: "unauthenticated", message: "sign in" } } }); });
    await page.goto("/");
    await expect(page.getByTestId("sign-in")).toBeVisible();
    const calls = whoamiCalls;
    await page.getByTestId("sign-in").getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByTestId("token-input")).toHaveAttribute("aria-invalid", "true");
    await expect(page.getByText("Paste the API token an administrator gave you.")).toBeVisible();
    await expect(page.getByTestId("token-input")).toBeFocused();
    expect(whoamiCalls).toBe(calls);
    apiTokens = false;
    await page.reload();
    await expect(page.getByTestId("sign-in-none")).toContainText("Sign-in is not set up yet");
    await expect(page.getByTestId("sign-in")).toHaveCount(0);
  });

  test("a token from the identity provider that Apelora refuses says why, not that the session ended", async ({ page }) => {
    const issuer = "https://id.example.test";
    const cors = { "access-control-allow-origin": "*" };
    await page.route("**/api/v1/health", (route) => route.fulfill({ json: { ...health("incus", true), auth: "tokens" } }));
    await page.route("**/api/v1/sign-in", (route) => route.fulfill({ json: { providers: [{ issuer, clientId: "c", scopes: ["openid"], label: "Example Corp" }], apiTokens: false } }));
    await page.route("**/api/v1/whoami", (route) => route.fulfill({ status: 401, json: { error: { code: "invalid_token", message: "the token lives longer than Apelora accepts" } } }));
    await page.route(`${issuer}/.well-known/openid-configuration`, (route) => route.fulfill({ headers: cors, json: { issuer, authorization_endpoint: `${issuer}/auth`, token_endpoint: `${issuer}/token` } }));
    await page.route(`${issuer}/auth?**`, (route) => {
      const u = new URL(route.request().url());
      return route.fulfill({ status: 302, headers: { location: `${u.searchParams.get("redirect_uri")}?code=c&state=${u.searchParams.get("state")}` } });
    });
    await page.route(`${issuer}/token`, (route) => route.fulfill({ headers: cors, json: { access_token: "a.b.c", token_type: "Bearer" } }));
    await page.goto("/");
    await page.getByTestId("sign-in-oidc").click();
    await expect(page.getByTestId("sign-in-error")).toHaveText("You signed in, but Apelora did not accept the identity provider's token: the token lives longer than Apelora accepts");
    expect(await page.evaluate(() => sessionStorage.getItem("apelora.apiToken"))).toBeNull();
  });

  test("organisation sign-in that the person cancels comes back to the sign-in page with the reason", async ({ page }) => {
    await page.route("**/api/v1/health", (route) => route.fulfill({ json: { ...health("incus", true), auth: "tokens" } }));
    await page.route("**/api/v1/sign-in", (route) => route.fulfill({ json: { providers: [{ issuer: "https://id.example.test", clientId: "c", scopes: ["openid"], label: "Example Corp" }], apiTokens: false } }));
    await page.route("**/api/v1/whoami", (route) => route.fulfill({ status: 401, json: { error: { code: "unauthenticated", message: "sign in" } } }));
    await page.goto("/?error=access_denied&error_description=The%20sign-in%20was%20cancelled&state=x");
    await expect(page.getByTestId("sign-in-error")).toContainText("The sign-in was cancelled");
    await expect(page.getByTestId("sign-in")).toHaveCount(0);
    expect(page.url()).not.toContain("error=");
  });

  test("development mode says authentication is disabled", async ({ page }) => {
    await api(page, { "/api/v1/health": { json: { ...health("mock", false), auth: "disabled-development" } }, "/api/v1/cluster": { json: cluster("mock", ["mock-member-01"]) }, "/api/v1/instances": { json: instances("mock") } });
    await page.goto("/");
    await expect(page.getByTestId("status")).toContainText("Disabled (development only)");
    await expect(page.getByTestId("sign-in")).toHaveCount(0);
  });

  test("shell follows the Studio visual contract", async ({ page }) => {
    await page.route("**/api/v1/health", (route) => route.abort());
    await page.goto("/");
    const header = page.locator("header");
    expect((await header.boundingBox())!.height).toBeCloseTo(48, 0);
    expect(await page.locator("body > #root > div").evaluate((el) => getComputedStyle(el).backgroundColor)).toBe("oklch(1 0 0)");
    await expect(page.locator(".btn-primary")).toHaveCount(1);
  });
});

// ---- Views and actions (Phase 05) ----------------------------------------------------------

type Perm = "read-inventory" | "read-audit" | "manage-tokens" | "manage-infrastructure" | "design-applications" | "deploy";
const OPERATOR: Perm[] = ["read-inventory", "design-applications", "deploy"];
const ADMIN: Perm[] = ["read-inventory", "read-audit", "manage-tokens", "manage-infrastructure", "design-applications", "deploy"];
const whoami = (role: string, permissions: Perm[]) => ({ name: "alex", role, permissions, deployProjects: ["apelora"] });
const inst = (name: string, project: string, status: string, managed: boolean) =>
  ({ name, project, kind: "container", status, location: "node-1", architecture: "x86_64", description: "", managed });

/** Stubs the API: each entry answers one method + path; functions may change answers between calls. */
async function stub(page: Page, routes: Record<string, (req: import("@playwright/test").Request) => { status?: number; json?: unknown }>) {
  await page.route("**/api/v1/**", async (route) => {
    const req = route.request();
    const path = new URL(req.url()).pathname;
    const handler = routes[`${req.method()} ${path}`];
    if (!handler) return route.fulfill({ status: 404, json: { error: { code: "not_found", message: `no stub for ${req.method()} ${path}` } } });
    const r = handler(req);
    return r.json === undefined ? route.fulfill({ status: r.status ?? 204, body: "" }) : route.fulfill({ status: r.status ?? 200, json: r.json });
  });
}

test.describe("fabric console views", () => {
  test("only Apelora's instances in deploy projects get actions; stopping follows the job", async ({ page }) => {
    let stopped = false;
    let polls = 0;
    const sent: { body: unknown; key: string | undefined }[] = [];
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("operator", OPERATOR) }),
      "GET /api/v1/instances": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [inst("web-01", "apelora", stopped ? "Stopped" : "Running", true), inst("legacy-01", "default", "Running", false)] } }),
      "POST /api/v1/instances/apelora/web-01/state": (req) => { sent.push({ body: req.postDataJSON(), key: req.headers()["idempotency-key"] }); return { status: 202, json: { jobId: "j1", created: true } }; },
      "GET /api/v1/jobs/j1": () => { polls += 1; if (polls > 1) stopped = true; return { json: { id: "j1", kind: "instance.stop", status: polls > 1 ? "succeeded" : "running", attempts: 1, maxAttempts: 5, lastError: null, incusOperation: null, request: {}, createdAt: now, updatedAt: now } }; },
    });
    await page.goto("/#/instances");
    const rows = page.getByTestId("instance-row");
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(1)).toContainText("Not managed by Apelora");
    await expect(rows.nth(1).getByRole("button")).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Deploy module" })).toBeVisible();
    await rows.nth(0).getByRole("button", { name: "Stop web-01" }).click();
    await expect(rows.nth(0)).toContainText("Stopped");
    await expect(rows.nth(0).getByRole("button", { name: "Start web-01" })).toBeVisible();
    expect(sent).toHaveLength(1);
    expect(sent[0]!.body).toEqual({ action: "stop" });
    expect(sent[0]!.key).toBeTruthy();
  });

  test("deleting needs the instance name typed before it can be confirmed", async ({ page }) => {
    let deleted = 0;
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("operator", OPERATOR) }),
      "GET /api/v1/instances": () => ({ json: { source: { provider: "incus", observedAt: now }, items: deleted ? [] : [inst("web-01", "apelora", "Running", true)] } }),
      "DELETE /api/v1/instances/apelora/web-01": () => { deleted += 1; return { status: 202, json: { jobId: "j2", created: true } }; },
      "GET /api/v1/jobs/j2": () => ({ json: { id: "j2", kind: "instance.delete", status: "succeeded", attempts: 1, maxAttempts: 5, lastError: null, incusOperation: null, request: {}, createdAt: now, updatedAt: now } }),
    });
    await page.goto("/#/instances");
    await page.getByRole("button", { name: "Delete web-01" }).click();
    const dialog = page.getByTestId("confirm-dialog");
    await expect(dialog).toContainText("cannot be undone");
    await expect(page.getByTestId("confirm-input")).toBeFocused();
    const confirm = page.getByTestId("confirm-action");
    await expect(confirm).toBeDisabled();
    await page.getByTestId("confirm-input").fill("web-1");
    await expect(confirm).toBeDisabled();
    await page.getByTestId("confirm-input").fill("web-01");
    await confirm.click();
    await expect(page.getByText("No instances yet.")).toBeVisible();
    expect(deleted).toBe(1);
  });

  test("the deploy form builds the request from the catalog runtime", async ({ page }) => {
    let body: unknown = null;
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("operator", OPERATOR) }),
      "GET /api/v1/instances": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [] } }),
      "POST /api/v1/instances": (req) => { body = req.postDataJSON(); return { status: 202, json: { jobId: "j3", created: true } }; },
      "GET /api/v1/jobs/j3": () => ({ json: { id: "j3", kind: "instance.create", status: "pending", attempts: 2, maxAttempts: 5, lastError: "Incus operation failed: image download timed out", incusOperation: null, request: {}, createdAt: now, updatedAt: now } }),
    });
    await page.goto("/#/instances");
    await page.getByRole("button", { name: "Deploy module" }).click();
    const dialog = page.getByTestId("deploy-dialog");
    await dialog.getByTestId("deploy-class").selectOption({ label: "NGINX" });
    await dialog.getByTestId("deploy-setting").selectOption("mainline");
    await expect(dialog.getByTestId("deploy-setting")).toContainText("mainline (1.31.7)");
    await dialog.getByTestId("deploy-name").fill("2web");
    await expect(dialog).toContainText("starting with a letter");
    await expect(dialog.getByTestId("deploy-submit")).toBeDisabled();
    await dialog.getByTestId("deploy-name").fill("web-02");
    await dialog.getByTestId("deploy-submit").click();
    await expect(page.getByTestId("pending-deploy")).toContainText("apelora/web-02");
    await expect(page.getByTestId("pending-deploy")).toContainText("attempt 2");
    expect(body).toEqual({
      project: "apelora", name: "web-02", kind: "container", start: true, architectures: ["x86_64", "aarch64"],
      config: { "limits.cpu": "1", "limits.memory": "512MiB" },
      source: { protocol: "oci", server: "https://docker.io", alias: "library/nginx@sha256:bc4cad49969aaf95eb4eecec5572846f86e59f1e8332aba36ed64b91aff8b724" },
    });
  });

  test("an administrator adds a machine, sees its join token once, and admits it after the checks", async ({ page }) => {
    type Row = { name: string; description: string; state: string; nextStep: string; warnings: string[] };
    let machines: Row[] = [];
    let pending: { memberName: string; expiresAt: string }[] = [];
    let revoked = 0;
    let admitted = 0;
    let admissible = false;
    const token = "eyJzZXJ2ZXJfbmFtZSI6InNlcnZlci0wNCJ9";
    const row = (name: string, description: string, state: string, nextStep: string): Row => ({ name, description, state, nextStep, warnings: [] });
    const check = (id: string, title: string, status: string, detail: string) => ({ id, title, status, detail });
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("fabric-admin", ADMIN) }),
      "GET /api/v1/cluster/join-tokens": () => ({ json: { source: { provider: "incus", observedAt: now }, items: pending } }),
      "DELETE /api/v1/cluster/join-tokens/server-04": () => { revoked += 1; pending = []; machines = machines.map((m) => ({ ...m, state: "approval-expired" })); return { status: 204 }; },
      "GET /api/v1/machines": () => ({ json: { source: { provider: "incus", observedAt: now }, items: machines } }),
      "POST /api/v1/machines": (req) => {
        const { name, description } = req.postDataJSON() as { name: string; description: string };
        machines = [row(name, description, "registered", "Install Incus on the server, then approve it.")];
        return { status: 201, json: machines[0] };
      },
      "POST /api/v1/machines/server-04/approve": () => {
        pending = [{ memberName: "server-04", expiresAt: "2026-10-20T03:00:00Z" }];
        machines = [{ ...machines[0], state: "approved", nextStep: "Run the enrolment script on the server and paste the join token." }];
        return { status: 201, json: { name: "server-04", expiresAt: "2026-10-20T03:00:00Z", token, enrolCommand: "sudo bash scripts/enrol/enrol-member.sh --name server-04 --address <this server's cluster address>", warnings: ["this token stays valid until 2026-10-20T03:00:00Z; set the Incus option cluster.join_token_expiry to 1H or less"] } };
      },
      "GET /api/v1/machines/server-04/admission": () => ({ json: { source: { provider: "incus", observedAt: now }, name: "server-04", admissible, checks: [
        check("member-online", "Member online", "pass", "online"),
        check("storage-pools", "Storage pools", admissible ? "pass" : "fail", admissible ? "1 created on the member" : "not created on the member: local"),
        check("clock", "Clock", "not-checked", "the Incus API cannot measure clock drift"),
      ] } }),
      "POST /api/v1/machines/server-04/admit": () => { admitted += 1; machines = [{ ...machines[0], state: "eligible", nextStep: "None." }]; return { json: { source: { provider: "incus", observedAt: now }, name: "server-04", admissible: true, checks: [] } }; },
    });
    await page.goto("/#/membership");
    await expect(page.getByText("No machines yet.")).toBeVisible();
    await page.getByTestId("machine-name").fill("2server");
    await expect(page.getByTestId("register-machine")).toBeDisabled();
    await page.getByTestId("machine-name").fill("server-04");
    await page.getByTestId("machine-description").fill("Rack 2, slot 7");
    await page.getByTestId("register-machine").click();
    await expect(page.getByTestId("machine-state")).toHaveText("Not approved");
    await expect(page.getByTestId("machine-row")).toContainText("Rack 2, slot 7");

    // Approval: the token and the command, once.
    await page.getByTestId("approve-machine").click();
    await expect(page.getByTestId("issued-token-value")).toHaveValue(token);
    await expect(page.getByTestId("enrol-command")).toContainText("--name server-04");
    await expect(page.getByTestId("issued-token")).toContainText("Shown only once");
    await expect(page.getByTestId("issued-token")).toContainText("cluster.join_token_expiry");
    await expect(page.getByTestId("machine-state")).toHaveText("Waiting to join");
    await expect(page.getByTestId("join-token-row")).toHaveCount(1);
    await page.getByRole("button", { name: "Hide token" }).click();
    await expect(page.getByTestId("issued-token")).toHaveCount(0);
    await expect(page.getByText(token)).toHaveCount(0);

    // The token can still be revoked from the list of unused tokens.
    await page.getByRole("button", { name: "Revoke the join token for server-04" }).click();
    const open = page.locator("dialog[open]");
    await expect(open.getByTestId("confirm-dialog").getByRole("button", { name: "Cancel" })).toBeFocused();
    await open.getByTestId("confirm-action").click();
    await expect(page.getByText("No unused join tokens.")).toBeVisible();
    expect(revoked).toBe(1);

    // The server joined (held): a failed check blocks admission until it is fixed.
    machines = [{ ...machines[0], state: "held", nextStep: "Check and admit it." }];
    await page.reload();
    await expect(page.getByTestId("machine-state")).toHaveText("Joined, not admitted");
    await page.getByTestId("admit-machine").click();
    const dialog = page.getByTestId("admit-dialog");
    await expect(dialog.getByTestId("admission-check")).toHaveCount(3);
    await expect(dialog.getByTestId("admission-blocked")).toBeVisible();
    await expect(dialog.getByTestId("admit-start")).toHaveCount(0);
    admissible = true;
    await dialog.getByRole("button", { name: "Check again" }).click();
    await expect(dialog.getByTestId("admission-check").nth(1)).toContainText("Passed");
    await dialog.getByTestId("admit-start").click();
    await expect(page.getByTestId("machines-notice")).toContainText("server-04 is in service");
    await expect(page.getByTestId("machine-state")).toHaveText("In service");
    expect(admitted).toBe(1);
  });

  test("an evacuated member is removed only when the check passes and its name is typed", async ({ page }) => {
    let gone = false;
    let blocked = ["1 instance is still on it (apelora/shop-db-01); an evacuation leaves instances whose volumes are on the member stopped in place"];
    const sent: unknown[] = [];
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("fabric-admin", ADMIN) }),
      "GET /api/v1/cluster/join-tokens": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [] } }),
      "GET /api/v1/machines": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [] } }),
      "GET /api/v1/cluster/members": () => ({ json: serverMembers({ members: [serverMember("node-1"), ...(gone ? [] : [serverMember("node-4", { status: "Evacuated" })])] }) }),
      "GET /api/v1/cluster/members/node-4/removal": () => ({ json: { source: { provider: "incus", observedAt: now }, plan: {
        member: "node-4", status: "Evacuated", instances: blocked.length ? ["apelora/shop-db-01"] : [], volumes: [], blocked,
        warnings: ["to use the server again, reinstall it and enrol it as a new machine"] } } }),
      "POST /api/v1/cluster/members/node-4/remove": (req) => { sent.push(req.postDataJSON()); gone = true; return { status: 204 }; },
    });
    await page.goto("/#/servers/members");
    await page.getByRole("button", { name: "Remove node-4 from the cluster" }).click();
    const dialog = page.getByTestId("remove-member-dialog");
    await expect(dialog.getByTestId("removal-blocked")).toContainText("Cannot remove: 1 instance is still on it");
    await expect(dialog.getByTestId("remove-member-confirm")).toHaveCount(0);
    await expect(dialog.getByTestId("remove-member-start")).toBeDisabled();
    await dialog.getByRole("button", { name: "Cancel" }).click();

    blocked = [];
    await page.getByRole("button", { name: "Remove node-4 from the cluster" }).click();
    await expect(dialog.getByTestId("removal-warning")).toContainText("To use the server again, reinstall it and enrol it as a new machine");
    await dialog.getByTestId("remove-member-confirm").fill("node-");
    await expect(dialog.getByTestId("remove-member-start")).toBeDisabled();
    await dialog.getByTestId("remove-member-confirm").fill("node-4");
    await dialog.getByTestId("remove-member-start").click();
    await expect(page.getByTestId("servers-notice")).toContainText("node-4 was removed from the cluster");
    await expect(page.getByTestId("server-member")).toHaveCount(1);
    expect(sent).toEqual([{ confirm: "node-4" }]);
  });

  test("an administrator evacuates a member only after the dry run and typing its name, then restores it", async ({ page }) => {
    let status = "Online";
    const sent: { path: string; body: unknown }[] = [];
    const plan = (blocked: string[]) => ({ source: { provider: "incus", observedAt: now }, plan: {
      member: "node-2", targets: ["node-1"], blocked,
      warnings: ["replicas of shop/web-01 share node-1 until restore: one failure takes them all", "down during the evacuation: shop-db-01"],
      items: [
        { project: "apelora", instance: "shop-web-01-r2", status: "Running", mode: "auto", outcome: "moves if it can; stopped during the move", application: "shop", module: "web-01", impact: "stays up (1 other replica)" },
        { project: "apelora", instance: "shop-db-01", status: "Running", mode: "stop", outcome: "stops here until restore", application: "shop", module: "db-01", impact: "down until restore (stateful)" },
      ] } });
    let blocked: string[] = [];
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("fabric-admin", ADMIN) }),
      "GET /api/v1/cluster/join-tokens": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [] } }),
      "GET /api/v1/machines": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [] } }),
      "GET /api/v1/cluster/members": () => ({ json: serverMembers({ members: [serverMember("node-1"), serverMember("node-2", { status })] }) }),
      "GET /api/v1/cluster/members/node-2/evacuation": () => ({ json: plan(blocked) }),
      "POST /api/v1/cluster/members/node-2/evacuate": (req) => { sent.push({ path: "evacuate", body: req.postDataJSON() }); status = "Evacuated"; return { status: 202, json: { member: "node-2", operation: "op-1" } }; },
      "POST /api/v1/cluster/members/node-2/restore": (req) => { sent.push({ path: "restore", body: req.postDataJSON() }); status = "Online"; return { status: 202, json: { member: "node-2", operation: "op-2" } }; },
    });
    await page.goto("/#/servers/members");
    await expect(page.getByTestId("server-member")).toHaveCount(2);

    // Blocked: explained, and nothing can be confirmed.
    blocked = ["no other online member can take its instances"];
    await page.getByTestId("evacuate-member").nth(1).click();
    const dialog = page.getByTestId("evacuate-dialog");
    await expect(dialog.getByTestId("evacuation-blocked")).toHaveText("Cannot evacuate: no other online member can take its instances");
    await expect(dialog.getByTestId("evacuate-confirm")).toHaveCount(0);
    await expect(dialog.getByTestId("evacuate-start")).toBeDisabled();
    await dialog.getByRole("button", { name: "Cancel" }).click();

    // Not blocked: every instance's fate is shown; the name must be typed.
    blocked = [];
    await page.getByTestId("evacuate-member").nth(1).click();
    await expect(dialog.getByTestId("evacuation-item")).toHaveCount(2);
    await expect(dialog.getByTestId("evacuation-item").nth(1)).toContainText("Down until restore (stateful)");
    await expect(dialog.getByTestId("evacuation-warning").first()).toContainText("share node-1 until restore");
    await expect(dialog.getByTestId("evacuate-start")).toBeDisabled();
    await dialog.getByTestId("evacuate-confirm").fill("node-1");
    await expect(dialog.getByTestId("evacuate-start")).toBeDisabled();
    await dialog.getByTestId("evacuate-confirm").fill("node-2");
    await dialog.getByTestId("evacuate-start").click();
    await expect(page.getByTestId("servers-notice")).toContainText("Evacuating node-2");
    await expect(page.getByTestId("restore-member")).toHaveCount(1);

    await page.getByTestId("restore-member").click();
    const open = page.locator("dialog[open]");
    await open.getByTestId("confirm-input").fill("node-2");
    await open.getByTestId("confirm-action").click();
    await expect(page.getByTestId("servers-notice")).toContainText("Restoring node-2");
    expect(sent).toEqual([{ path: "evacuate", body: { confirm: "node-2" } }, { path: "restore", body: { confirm: "node-2" } }]);
  });

  test("operators are not offered machine enrolment, the audit log or token administration", async ({ page }) => {
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("operator", OPERATOR) }),
      "GET /api/v1/cluster": () => ({ json: cluster("incus", ["node-1"]) }),
    });
    await page.goto("/#/membership");
    const nav = page.getByRole("navigation", { name: "Console" });
    await expect(nav.getByRole("link")).toHaveText(["Overview", "Health", "Servers", "Applications", "Instances", "Operations", "Networks", "Storage", "Alerts", "Administration"]);
    // The old enrolment address leads to Servers, without the machines tab.
    await expect(page.getByRole("heading", { level: 1 })).toHaveText("Servers");
    await expect(page.getByRole("navigation", { name: "Servers" }).getByRole("link", { name: "Machines and join tokens" })).toHaveCount(0);
    await expect(page.getByTestId("whoami")).toHaveText("alex · operator");
  });

  test("preflight, networks, storage and the audit log show what the API returns", async ({ page }) => {
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("fabric-admin", ADMIN) }),
      "GET /api/v1/preflight": () => ({ json: { source: { provider: "incus", observedAt: now }, overall: "warn", checks: [
        { id: "incus-version", title: "Incus version", status: "pass", detail: "Incus 7.0.1" },
        { id: "project-apelora", title: "Project apelora", status: "warn", detail: "exists but is not restricted" },
        { id: "clock", title: "Clock agreement between members", status: "not-checked", detail: "not measurable through the Incus API" },
      ] } }),
      "GET /api/v1/networks/applications": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [] } }),
      "GET /api/v1/networks/settings": () => ({ json: { source: { provider: "incus", observedAt: now }, uplink: null, serviceRange: null, serviceAddressesUsed: 0, serviceAddressesTotal: null,
        projects: [{ project: "apelora", found: true, restricted: true, ownNetworks: true, uplinks: ["incusbr0"], subnets: [] }], uplinkObserved: null, uplinkNote: null,
        findings: [{ severity: "info", message: "No uplink is set (APELORA_NETWORK_UPLINK): connections are not enforced and instances share their project's network" }] } }),
      "GET /api/v1/storage/pools": () => ({ json: { source: { provider: "incus", observedAt: now }, volumePool: "local", backupPool: null, items: [
        { name: "local", driver: "btrfs", status: "Created", description: "", shared: false, role: "volumes", members: [{ member: "node-1", usedBytes: 2 * 1024 ** 3, totalBytes: 20 * 1024 ** 3 }], total: { usedBytes: 2 * 1024 ** 3, totalBytes: 20 * 1024 ** 3 }, unreadable: [] }] } }),
      "GET /api/v1/storage/volumes": () => ({ json: { source: { provider: "incus", observedAt: now }, backupPool: null, incusError: null, items: [] } }),
      "GET /api/v1/storage/orphans": () => ({ json: { source: { provider: "incus", observedAt: now }, projects: ["apelora"], items: [] } }),
      "GET /api/v1/networks": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [
        { name: "incusbr0", project: "default", kind: "bridge", managed: true, status: "Created", description: "" },
        { name: "eth0", project: "default", kind: "physical", managed: false, status: "", description: "" },
      ] } }),
      "GET /api/v1/storage-pools": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [{ name: "local", driver: "btrfs", status: "Created", description: "" }] } }),
      "GET /api/v1/audit": () => ({ json: { items: [{ id: 1, at: now, actor: "alex", action: "cluster.join-token.created", target: "server-04", detail: {} }] } }),
    });
    await page.goto("/#/preflight");
    await expect(page.getByTestId("preflight-check")).toHaveCount(3);
    await expect(page.getByTestId("preflight-overall")).toContainText("need attention");
    await expect(page.getByTestId("preflight-check").nth(2)).toContainText("Not checked");
    // The former "Networks & storage" address opens Networks.
    await page.goto("/#/infrastructure");
    await expect(page).toHaveURL(/#\/networks$/);
    await expect(page.getByRole("heading", { level: 1 })).toHaveText("Networks");
    await expect(page.getByTestId("network-project-row")).toContainText("Yes");
    await expect(page.getByTestId("network-row")).toHaveCount(1);
    await expect(page.getByText("1 host interfaces not managed by Incus are not shown.")).toBeVisible();
    await page.goto("/#/storage");
    await expect(page.getByTestId("storage-pool-row")).toContainText("btrfs");
    await expect(page.getByTestId("storage-pool-row")).toContainText("2.0 GiB of 20.0 GiB");
    await page.goto("/#/audit");
    await expect(page.getByTestId("audit-row")).toContainText("Cluster join token created");
  });

  test("applications show how their instances run and which fail their check", async ({ page }) => {
    const inst = (instance: string, module: string, status: string, location: string, health?: string) => ({
      module, instance, project: "apelora", status, location,
      ...(health ? { health: { health, since: now, detail: health === "healthy" ? "port check passing" : "port check failing" } } : {}),
    });
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("fabric-admin", ADMIN) }),
      "GET /api/v1/applications": () => ({ json: { items: [{ name: "shop", createdAt: now, latestRevision: 3 }, { name: "blog", createdAt: now, latestRevision: 1 }] } }),
      "GET /api/v1/applications/shop/status": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [
        inst("shop-client-01", "client-01", "Running", "node-1"),
        inst("shop-web-01", "web-01", "Running", "node-2", "healthy"),
        inst("shop-web-01-r2", "web-01", "Running", "node-3", "unhealthy"),
      ], services: [{ module: "web-01", project: "apelora", address: "198.18.100.1" }] } }),
      "GET /api/v1/applications/blog/status": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [inst("blog-web-01", "web-01", "Stopped", "node-1")], services: [] } }),
    });
    await page.goto("/#/applications");
    await expect(page.getByTestId("application-row")).toHaveCount(2);
    const shop = page.getByTestId("application-row").first();
    await expect(shop).toContainText("Revision 3");
    await expect(shop.getByTestId("application-summary")).toContainText("1 failing its check");
    await expect(shop.getByTestId("application-summary")).toContainText("3 of 3 running");
    await expect(shop).toContainText("198.18.100.1");
    await expect(page.getByTestId("application-row").nth(1).getByTestId("application-summary")).toContainText("1 not running");
    await shop.getByTestId("application-toggle").click();
    await expect(shop.getByTestId("application-toggle")).toHaveAttribute("aria-expanded", "true");
    await expect(page.getByTestId("application-instance")).toHaveCount(3);
    await expect(page.getByTestId("application-instance").nth(0)).toContainText("Not checked");
    await expect(page.getByTestId("application-instance").nth(1)).toContainText("Passing");
    await expect(page.getByTestId("application-instance").nth(2)).toContainText("Failing since");
    await shop.getByTestId("application-toggle").click();
    await expect(page.getByTestId("application-instance")).toHaveCount(0);
  });

  test("kept volumes are listed with their state and deleted only with their name typed", async ({ page }) => {
    let deleted = false;
    const sent: unknown[] = [];
    const vol = (incusName: string, state: string, usedBy: string[]) => ({
      module: "db-01", instance: "shop-db-01", name: incusName.endsWith("logs") ? "logs" : "data", incusName, pool: "local",
      member: "node-2", sizeMib: incusName.endsWith("logs") ? 512 : 2048, content: "filesystem", class: "data-local", state, usedBy, createdAt: now,
    });
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("operator", OPERATOR) }),
      "GET /api/v1/applications": () => ({ json: { items: [{ name: "shop", createdAt: now, latestRevision: 4 }] } }),
      "GET /api/v1/applications/shop/status": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [
        { module: "web-01", instance: "shop-web-01", project: "apelora", status: "Running", location: "node-1" },
      ], services: [] } }),
      "GET /api/v1/applications/shop/volumes": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [
        vol("shop-web-01-logs", "attached", ["shop-web-01"]),
        ...(deleted ? [] : [vol("shop-db-01-data", "retained", [])]),
      ] } }),
      "DELETE /api/v1/applications/shop/volumes/shop-db-01-data": (req) => { sent.push(req.postDataJSON()); deleted = true; return { status: 204 }; },
      "PATCH /api/v1/applications/shop/volumes/shop-web-01-logs": (req) => { sent.push(req.postDataJSON()); return { status: 204 }; },
    });
    await page.goto("/#/applications");
    await page.getByTestId("application-toggle").click();
    const volumes = page.getByTestId("application-volume");
    await expect(volumes).toHaveCount(2);
    await expect(volumes.nth(0)).toContainText("In use by shop-web-01");
    await expect(volumes.nth(0).getByTestId("delete-volume")).toHaveCount(0);
    await expect(volumes.nth(1)).toContainText("Kept, not in use");
    await expect(volumes.nth(1)).toContainText("2 GiB");
    await volumes.nth(1).getByRole("button", { name: "Delete shop-db-01-data" }).click();
    const dialog = page.getByTestId("delete-volume-dialog");
    await expect(dialog).toContainText("2 GiB of data and its snapshots are deleted from pool local. This cannot be undone.");
    await expect(dialog.getByTestId("delete-volume-start")).toBeDisabled();
    await dialog.getByTestId("delete-volume-confirm").fill("shop-db-01");
    await expect(dialog.getByTestId("delete-volume-start")).toBeDisabled();
    await dialog.getByTestId("delete-volume-confirm").fill("shop-db-01-data");
    await dialog.getByTestId("delete-volume-start").click();
    await expect(page.getByTestId("application-volumes").getByRole("status")).toHaveText("Volume shop-db-01-data deleted.");
    await expect(volumes).toHaveCount(1);
    expect(sent).toEqual([{ confirm: "shop-db-01-data" }]);

    // A volume in use can still grow; the new size must be larger.
    await volumes.nth(0).getByRole("button", { name: "Grow shop-web-01-logs" }).click();
    const grow = page.getByTestId("grow-volume-dialog");
    await expect(grow).toContainText("It is 512 MiB now.");
    await grow.getByTestId("grow-volume-size").fill("0.25");
    await expect(grow.getByTestId("grow-volume-start")).toBeDisabled();
    await grow.getByTestId("grow-volume-size").fill("4");
    await grow.getByTestId("grow-volume-start").click();
    await expect(page.getByTestId("application-volumes").getByRole("status")).toHaveText("Volume shop-web-01-logs is now 4 GiB.");
    expect(sent.at(-1)).toEqual({ sizeMib: 4096 });
  });

  test("a volume's snapshots are taken, restored only with the name typed, and deleted", async ({ page }) => {
    let snaps: { name: string; createdAt: string }[] = [];
    const sent: { what: string; body: unknown }[] = [];
    const vol = { module: "db-01", instance: "shop-db-01", name: "data", incusName: "shop-db-01-data", pool: "local", member: "node-2", sizeMib: 2048, content: "filesystem", class: "data-local", state: "attached", usedBy: ["shop-db-01"], createdAt: now };
    const base = "/api/v1/applications/shop/volumes/shop-db-01-data/snapshots";
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("operator", OPERATOR) }),
      "GET /api/v1/applications": () => ({ json: { items: [{ name: "shop", createdAt: now, latestRevision: 1 }] } }),
      "GET /api/v1/applications/shop/status": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [], services: [] } }),
      "GET /api/v1/applications/shop/volumes": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [vol] } }),
      [`GET ${base}`]: () => ({ json: { source: { provider: "incus", observedAt: now }, items: snaps } }),
      [`POST ${base}`]: () => { snaps = [...snaps, { name: "snap-20261021-101500", createdAt: now }]; sent.push({ what: "take", body: null }); return { status: 201, json: { name: "snap-20261021-101500", consistency: "crash-consistent" } }; },
      [`POST ${base}/snap-20261021-101500/restore`]: (req) => { sent.push({ what: "restore", body: req.postDataJSON() }); return { status: 204 }; },
      [`DELETE ${base}/snap-20261021-101500`]: () => { snaps = []; sent.push({ what: "delete", body: null }); return { status: 204 }; },
    });
    await page.goto("/#/applications");
    await page.getByTestId("application-toggle").click();
    await page.getByRole("button", { name: "Snapshots of shop-db-01-data" }).click();
    const dialog = page.getByTestId("snapshots-dialog");
    await expect(dialog).toContainText("No snapshots yet.");
    await dialog.getByTestId("take-snapshot").click();
    await expect(dialog.getByTestId("snapshots-notice")).toContainText("taken while the instance runs");
    await expect(dialog.getByTestId("snapshot-row")).toHaveCount(1);
    await dialog.getByRole("button", { name: "Restore snap-20261021-101500" }).click();
    await expect(dialog.getByTestId("restore-start")).toBeDisabled();
    await dialog.getByTestId("restore-confirm").fill("snap-20261021-101500");
    await dialog.getByTestId("restore-start").click();
    await expect(dialog.getByTestId("snapshots-notice")).toContainText("is back to snap-20261021-101500");
    await dialog.getByRole("button", { name: "Delete snap-20261021-101500" }).click();
    await expect(dialog).toContainText("No snapshots yet.");
    expect(sent).toEqual([{ what: "take", body: null }, { what: "restore", body: { confirm: "snap-20261021-101500" } }, { what: "delete", body: null }]);
  });

  test("an instance moves after a dry run: a stop only with consent, the name typed, the job followed", async ({ page }) => {
    const sent: unknown[] = [];
    let moved = false;
    const plan = (allowStop: boolean) => ({ source: { provider: "incus", observedAt: now }, plan: {
      instance: "web-01", from: "node-1", to: "node-2", live: false, stops: allowStop, volumes: ["web-01-data"], copyMib: 1024,
      blocked: allowStop ? [] : ["it would have to stop: a container cannot move while running; allow a short stop to move it"],
      notes: [...(allowStop ? ["it stops for the move (a container cannot move while running) and starts again on node-2"] : []), "1 volume (1024 MiB) copied to node-2; the old copies are deleted once it runs there"] } });
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("operator", OPERATOR) }),
      "GET /api/v1/instances": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [inst("web-01", "apelora", "Running", true)].map((i) => ({ ...i, location: moved ? "node-2" : "node-1" })) } }),
      "GET /api/v1/cluster": () => ({ json: { ...cluster("incus", ["node-1"]), members: [member("node-1"), member("node-2"), member("node-3", "Offline")] } }),
      "GET /api/v1/instances/apelora/web-01/move": (req) => ({ json: plan(new URL(req.url()).searchParams.get("allowStop") === "true") }),
      "POST /api/v1/instances/apelora/web-01/move": (req) => { sent.push(req.postDataJSON()); return { status: 202, json: { jobId: "jm", created: true } }; },
      "GET /api/v1/jobs/jm": () => { moved = true; return { json: { id: "jm", kind: "instance.move", status: "succeeded", attempts: 1, maxAttempts: 5, lastError: null, incusOperation: null, request: {}, createdAt: now, updatedAt: now } }; },
    });
    await page.goto("/#/instances");
    await page.getByRole("button", { name: "Move web-01" }).click();
    const dialog = page.getByTestId("move-dialog");
    await expect(dialog.getByTestId("move-target").locator("option")).toHaveText(["node-2"]);
    await expect(dialog.getByTestId("move-blocked")).toContainText("allow a short stop");
    await expect(dialog.getByTestId("move-start")).toBeDisabled();
    await dialog.getByTestId("move-allow-stop").check();
    await expect(dialog.getByTestId("move-note").first()).toContainText("It stops for the move");
    await dialog.getByTestId("move-confirm").fill("web-01");
    await dialog.getByTestId("move-start").click();
    await expect(page.getByTestId("instance-row")).toContainText("node-2");
    expect(sent).toEqual([{ target: "node-2", confirm: "web-01", allowStop: true }]);
  });

  test("each view keeps one primary action", async ({ page }) => {
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("fabric-admin", ADMIN) }),
      "GET /api/v1/instances": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [] } }),
      "GET /api/v1/cluster/join-tokens": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [] } }),
      "GET /api/v1/machines": () => ({ json: { source: { provider: "incus", observedAt: now }, items: [] } }),
      "GET /api/v1/cluster": () => ({ json: cluster("incus", ["node-1", "node-2"]) }),
    });
    for (const view of ["instances", "servers/machines"]) {
      await page.goto(`/#/${view}`);
      await expect(page.getByRole("heading", { level: 1 })).toHaveCount(1);
      await expect(page.locator(".btn-primary:visible")).toHaveCount(1);
    }
  });
});

test.describe("fabric console on a phone", () => {
  test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });

  test("every view is reachable without sideways scrolling and controls are touch-sized", async ({ page }) => {
    await page.route("**/api/**", (route) => route.abort());
    await page.route("**/api/v1/health", (route) => route.fulfill({ json: health("incus", true) }));
    await page.route("**/api/v1/whoami", (route) => route.fulfill({ json: { name: "alex", role: "fabric-admin", permissions: ["read-inventory", "read-audit", "manage-tokens", "manage-infrastructure", "design-applications", "deploy"], deployProjects: ["apelora"] } }));
    await page.route("**/api/v1/instances", (route) => route.fulfill({ json: { source: { provider: "incus", observedAt: now }, items: [{ name: "web-01", project: "apelora", kind: "container", status: "Running", location: "node-1", architecture: "aarch64", description: "", managed: true }] } }));
    await page.goto("/#/instances");
    await expect(page.getByTestId("instance-row")).toHaveCount(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    for (const link of await page.getByRole("navigation", { name: "Console" }).getByRole("link").all()) {
      const box = (await link.boundingBox())!;
      expect(box.x + box.width, await link.innerText()).toBeLessThanOrEqual(390);
      expect(box.height).toBeGreaterThanOrEqual(44);
    }
    for (const name of ["Deploy module", "Stop web-01", "Delete web-01"]) {
      const box = (await page.getByRole("button", { name }).boundingBox())!;
      expect(box.height, name).toBeGreaterThanOrEqual(44);
      expect(box.x + box.width, name).toBeLessThanOrEqual(390);
    }
  });

  test("machines show their next step and actions under the state, touch-sized and without sideways scrolling", async ({ page }) => {
    await page.route("**/api/**", (route) => route.abort());
    await page.route("**/api/v1/health", (route) => route.fulfill({ json: health("incus", true) }));
    await page.route("**/api/v1/whoami", (route) => route.fulfill({ json: { name: "alex", role: "fabric-admin", permissions: ["read-inventory", "read-audit", "manage-tokens", "manage-infrastructure", "design-applications", "deploy"], deployProjects: ["apelora"] } }));
    await page.route("**/api/v1/machines", (route) => route.fulfill({ json: { source: { provider: "incus", observedAt: now }, items: [
      { name: "server-04", description: "fourth member, enrolled by hand", state: "eligible", nextStep: "None.", warnings: [] },
      { name: "server-05", description: "rack 1, slot 3", state: "registered", nextStep: "Install Incus on the server, then approve it.", warnings: [] },
    ] } }));
    await page.goto("/#/membership");
    await expect(page.getByTestId("machine-row")).toHaveCount(2);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    const frame = page.getByTestId("machines").locator("div.overflow-x-auto");
    expect(await frame.evaluate((d) => d.scrollWidth - d.clientWidth)).toBeLessThanOrEqual(0);
    await expect(page.getByTestId("machine-row").nth(1)).toContainText("Install Incus on the server, then approve it.");
    for (const name of ["Add machine", "Approve", "Remove server-05"]) {
      const box = (await page.getByRole("button", { name, exact: true }).boundingBox())!;
      expect(box.height, name).toBeGreaterThanOrEqual(44);
      expect(box.x + box.width, name).toBeLessThanOrEqual(390);
    }
    const input = (await page.getByTestId("machine-name").boundingBox())!;
    expect(input.height).toBeGreaterThanOrEqual(44);
  });

  test("applications fit a phone and their toggle is touch-sized", async ({ page }) => {
    await page.route("**/api/**", (route) => route.abort());
    await page.route("**/api/v1/health", (route) => route.fulfill({ json: health("incus", true) }));
    await page.route("**/api/v1/whoami", (route) => route.fulfill({ json: { name: "alex", role: "fabric-admin", permissions: ["read-inventory"], deployProjects: ["apelora"] } }));
    await page.route("**/api/v1/applications", (route) => route.fulfill({ json: { items: [{ name: "commerce-stack", createdAt: now, latestRevision: 2 }] } }));
    await page.route("**/api/v1/applications/commerce-stack/status", (route) => route.fulfill({ json: { source: { provider: "incus", observedAt: now }, items: [
      { module: "web-01", instance: "commerce-stack-web-01-r2", project: "apelora", status: "Running", location: "apelora-test-3", health: { health: "unhealthy", since: now, detail: "port check failing" } },
    ], services: [{ module: "web-01", project: "apelora", address: "198.18.100.1" }] } }));
    await page.goto("/#/applications");
    await page.getByTestId("application-toggle").click();
    await expect(page.getByTestId("application-instance")).toHaveCount(1);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    const box = (await page.getByTestId("application-toggle").boundingBox())!;
    expect(box.height).toBeGreaterThanOrEqual(44);
  });

  test("image mirrors fit a phone: long addresses wrap and the button is touch-sized", async ({ page }) => {
    await page.route("**/api/**", (route) => route.abort());
    await page.route("**/api/v1/health", (route) => route.fulfill({ json: health("incus", true) }));
    await page.route("**/api/v1/whoami", (route) => route.fulfill({ json: { name: "alex", role: "auditor", permissions: ["read-inventory"], deployProjects: ["apelora"] } }));
    await page.route("**/api/v1/system", (route) => route.fulfill({ json: { version: "0.0.0", apiVersion: "v1", incusPinned: "v7.0.3", incusSeries: "7.0", incusTestedWith: "", certificates: [] } }));
    await page.route("**/api/v1/image-mirrors", (route) => route.fulfill({ json: imageMirrors }));
    await page.goto("/#/administration");
    await expect(page.getByTestId("image-server-row")).toHaveCount(2);
    expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    const box = (await page.getByTestId("mirrors-recheck").boundingBox())!;
    expect(box.height).toBeGreaterThanOrEqual(44);
    expect(box.x + box.width).toBeLessThanOrEqual(390);
  });
});

test.describe("fabric console operations and administration", () => {
  test.beforeEach(async ({ page }) => { await page.route("**/api/**", (route) => route.abort()); });

  test("operations lists jobs with their outcome and refreshes while some are in progress", async ({ page }) => {
    let finished = false;
    const job = (id: string, kind: string, status: string, lastError: string | null, requestId: string | null = null) =>
      ({ id, kind, status, attempts: 1, maxAttempts: 5, lastError, incusOperation: null, incusOperations: [], requestId, request: { project: "apelora", name: "web-01" }, createdAt: now, updatedAt: now });
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("operator", OPERATOR) }),
      "GET /api/v1/jobs": () => ({ json: { items: [
        job("j2", "instance.stop", finished ? "succeeded" : "running", null),
        job("j1", "instance.create", "failed", "no cluster member has a supported architecture (x86_64)", "3f2b7c1e-5d4a-4e8f-9a61-0c2d8e7b4a15"),
      ] } }),
    });
    await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
    await page.goto("/#/operations");
    const rows = page.getByTestId("job-row");
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(1)).toContainText("Create instance");
    await expect(rows.nth(1)).toContainText("apelora/web-01");
    await expect(rows.nth(1).getByTestId("job-error")).toContainText("supported architecture");
    // The request that queued the job, to follow it into the audit log and the logs.
    await expect(rows.nth(1).getByTestId("job-request")).toContainText("3f2b7c1e-5d4a-4e8f-9a61-0c2d8e7b4a15");
    await expect(rows.nth(0).getByTestId("job-request")).toHaveCount(0);
    await rows.nth(1).getByRole("button", { name: "Copy request id 3f2b7c1e-5d4a-4e8f-9a61-0c2d8e7b4a15" }).click();
    await expect(rows.nth(1).getByTestId("job-request")).toContainText("Copied");
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe("3f2b7c1e-5d4a-4e8f-9a61-0c2d8e7b4a15");
    await expect(rows.nth(0)).toContainText("Running");
    finished = true; // the page refreshes on its own while a job is in progress
    await expect(rows.nth(0)).toContainText("Done", { timeout: 8000 });
  });

  test("an administrator creates a token shown once, sees running versus pinned Incus, and revokes", async ({ page }) => {
    let items = [{ id: "t1", name: "admin", role: "fabric-admin", createdAt: now, expiresAt: null, revokedAt: null, lastUsedAt: now }];
    let body: unknown = null;
    let revoked = "";
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("fabric-admin", ADMIN) }),
      "GET /api/v1/system": () => ({ json: { version: "0.0.0", apiVersion: "v1", incusPinned: "v7.0.3", incusSeries: "7.0", incusTestedWith: "REAL INCUS TESTED with 7.0.2", incus: { connected: true, apiVersion: "1.0", serverVersion: "7.0.2", clustered: true }, certificates: [
        { certificate: "deploy", expiresAt: "2026-11-02T00:00:00Z", daysLeft: 12, state: "expiring" },
        { certificate: "server", expiresAt: "2036-10-18T00:00:00Z", daysLeft: 3650, state: "ok" },
      ] } }),
      "GET /api/v1/tokens": () => ({ json: { items } }),
      "POST /api/v1/tokens": (req) => {
        body = req.postDataJSON();
        const info = { id: "t2", name: "ci", role: "auditor", createdAt: now, expiresAt: "2027-01-18T00:00:00Z", revokedAt: null, lastUsedAt: null };
        items = [...items, info];
        return { status: 201, json: { token: "apl_" + "b".repeat(64), info } };
      },
      "DELETE /api/v1/tokens/t2": () => { revoked = "t2"; items = items.map((t) => (t.id === "t2" ? { ...t, revokedAt: now } : t)); return { status: 204 }; },
    });
    await page.goto("/#/administration");
    await expect(page.getByTestId("system")).toContainText("7.0.3 (7.0 LTS series) · running 7.0.2 differs");
    await expect(page.getByTestId("certificate-row")).toHaveCount(0);
    await page.getByTestId("token-open").click();
    await page.getByTestId("token-name").fill("ci");
    await page.getByTestId("token-role").selectOption("auditor");
    await page.getByTestId("token-days").fill("30");
    await page.getByTestId("token-submit").click();
    expect(body).toEqual({ name: "ci", role: "auditor", expiresInDays: 30 });
    await expect(page.getByTestId("new-token-value")).toHaveValue("apl_" + "b".repeat(64));
    await page.getByRole("button", { name: "Hide token" }).click();
    await expect(page.getByTestId("new-token")).toHaveCount(0);
    await expect(page.getByTestId("token-row")).toHaveCount(2);
    await page.getByRole("button", { name: "Revoke token ci" }).click();
    await expect(page.getByTestId("confirm-dialog").getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.getByTestId("confirm-action").click();
    await expect(page.getByTestId("token-row").nth(1)).toContainText("Revoked");
    expect(revoked).toBe("t2");
  });

  test("image mirrors show the mode, whether each mirror answers, and which catalog servers are covered", async ({ page }) => {
    let loads = 0;
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("auditor", ["read-inventory", "read-audit"]) }),
      "GET /api/v1/system": () => ({ json: { version: "0.0.0", apiVersion: "v1", incusPinned: "v7.0.3", incusSeries: "7.0", incusTestedWith: "", certificates: [] } }),
      "GET /api/v1/image-mirrors": () => { loads += 1; return { json: imageMirrors }; },
    });
    await page.goto("/#/administration");
    const section = page.getByTestId("image-mirrors");
    await expect(section.getByRole("heading", { name: "Image mirrors" })).toBeVisible();
    await expect(page.getByTestId("mirrors-mode")).toHaveText("Only mirrors may be used.");
    const mirrors = page.getByTestId("mirror-row");
    await expect(mirrors).toHaveCount(2);
    await expect(mirrors.nth(0)).toContainText("https://images.mirror.internal");
    await expect(mirrors.nth(0)).toContainText("for https://images.linuxcontainers.org");
    await expect(mirrors.nth(0)).toContainText("Answers");
    await expect(mirrors.nth(0)).toContainText(`checked ${new Date(now).toLocaleTimeString()}`);
    await expect(mirrors.nth(1)).toContainText("Does not answer");
    await expect(mirrors.nth(1)).toContainText("no answer within 3 s");
    const servers = page.getByTestId("image-server-row");
    await expect(servers).toHaveCount(2);
    await expect(servers.nth(0)).toContainText("OCI images for NGINX (channel stable, mainline)");
    await expect(servers.nth(0)).toContainText("Refused");
    await expect(servers.nth(1)).toContainText("system and VM images for Linux base, Debian (version 12, 13)");
    await expect(servers.nth(1)).toContainText("Mirrored");
    await expect(servers.nth(1)).toContainText("from https://images.mirror.internal");
    // Read-only: nothing on the section changes configuration, and no primary action is added.
    await expect(section.locator(".btn-primary")).toHaveCount(0);
    await expect(section.getByRole("button")).toHaveText(["Check again"]);
    const before = loads; // development builds mount twice (StrictMode), so count from here
    await section.getByRole("button", { name: "Check again" }).click();
    await expect.poll(() => loads).toBe(before + 1);
  });

  test("image mirrors not configured: says that images come from upstream, no mirror list", async ({ page }) => {
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("mock", false) }),
      "GET /api/v1/whoami": () => ({ json: whoami("auditor", ["read-inventory"]) }),
      "GET /api/v1/system": () => ({ json: { version: "0.0.0", apiVersion: "v1", incusPinned: "v7.0.3", incusSeries: "7.0", incusTestedWith: "", certificates: [] } }),
      "GET /api/v1/image-mirrors": () => ({ json: { mode: "any", mirrors: [], overall: "warn", checks: [], servers: imageMirrors.servers.map((s) => ({ ...s, coverage: "upstream", mirror: null })) } }),
    });
    await page.goto("/#/administration");
    await expect(page.getByTestId("mirrors-mode")).toContainText("No mirrors: images come from their upstream servers.");
    await expect(page.getByTestId("mirror-row")).toHaveCount(0);
    await expect(page.getByTestId("image-server-row")).toHaveCount(2);
    await expect(page.getByTestId("image-server-row").first()).toContainText("From upstream");
  });

  test("an image-mirror error stays in its section and can be retried", async ({ page }) => {
    let fail = true;
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("auditor", ["read-inventory"]) }),
      "GET /api/v1/system": () => ({ json: { version: "0.0.0", apiVersion: "v1", incusPinned: "v7.0.3", incusSeries: "7.0", incusTestedWith: "", certificates: [] } }),
      "GET /api/v1/image-mirrors": () => (fail ? { status: 403, json: { error: { code: "forbidden", message: "not allowed: cluster.view" } } } : { json: imageMirrors }),
    });
    await page.goto("/#/administration");
    await expect(page.getByTestId("image-mirrors-error")).toContainText("cluster.view");
    await expect(page.getByTestId("system")).toContainText("7.0.3");
    fail = false;
    await page.getByTestId("image-mirrors").getByRole("button", { name: "Try again" }).click();
    await expect(page.getByTestId("mirror-row")).toHaveCount(2);
  });
});

const imageMirrors = {
  mode: "mirrors-only",
  overall: "fail",
  checks: [],
  mirrors: [
    { upstream: "https://images.linuxcontainers.org", mirror: "https://images.mirror.internal", probe: { state: "answers", detail: "HTTP 200 from /streams/v1/index.json" }, checkedAt: now },
    { upstream: "https://registry.example.org", mirror: "https://registry.mirror.internal", probe: { state: "fails", detail: "no answer within 3 s" }, checkedAt: now },
  ],
  servers: [
    { server: "https://docker.io", protocol: "oci", coverage: "refused", mirror: null, images: [
      { class: "nginx", displayName: "NGINX", setting: "channel = stable", alias: "library/nginx@sha256:aaaa" },
      { class: "nginx", displayName: "NGINX", setting: "channel = mainline", alias: "library/nginx@sha256:bbbb" },
    ] },
    { server: "https://images.linuxcontainers.org", protocol: "simplestreams", coverage: "mirrored", mirror: "https://images.mirror.internal", images: [
      { class: "linux-base", displayName: "Linux base", setting: null, alias: "debian/13" },
      { class: "debian", displayName: "Debian", setting: "version = 12", alias: "debian/12" },
      { class: "debian", displayName: "Debian", setting: "version = 13", alias: "debian/13" },
    ] },
  ],
};

// Monitoring (Administration): only what GET /api/v1/monitoring returns; read-only, no primary action.
const monitoring = (over: Record<string, unknown> = {}) => ({
  region: "eu-west", site: "fra1", sector: "s01", provider: "incus", mayScrape: true,
  permission: "fabric.metrics.read", role: "metrics-reader", incusMetricsPath: "/1.0/metrics",
  formats: ["text/plain; version=0.0.4; charset=utf-8", "application/openmetrics-text; version=1.0.0; charset=utf-8"],
  endpoints: [
    { component: "control-plane", address: null, path: "/metrics", authentication: "bearer" },
    { component: "worker", address: "127.0.0.1:9471", path: "/metrics", authentication: "none" },
  ],
  ...over,
});
const system = { version: "0.0.0", apiVersion: "v1", incusPinned: "v7.0.3", incusSeries: "7.0", incusTestedWith: "", certificates: [] };

test.describe("fabric console monitoring", () => {
  test.beforeEach(async ({ page }) => { await page.route("**/api/**", (route) => route.abort()); });

  test("shows the endpoints, the labels on every metric and how to connect a collector", async ({ page }) => {
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("fabric-admin", ADMIN) }),
      "GET /api/v1/system": () => ({ json: system }),
      "GET /api/v1/tokens": () => ({ json: { items: [] } }),
      "GET /api/v1/monitoring": () => ({ json: monitoring() }),
    });
    await page.goto("/#/administration");
    const panel = page.getByTestId("monitoring");
    await expect(panel.getByRole("heading", { name: "Monitoring" })).toBeVisible();
    await expect(page.getByTestId("monitoring-labels")).toHaveText("region=eu-west · site=fra1 · sector=s01");
    await expect(page.getByTestId("monitoring-access")).toHaveText("Can read metrics");
    const rows = page.getByTestId("metrics-endpoint");
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0)).toContainText("Control plane");
    await expect(rows.nth(0)).toContainText("/metrics on this API's address");
    await expect(rows.nth(0)).toContainText("Token with fabric.metrics.read");
    await expect(rows.nth(1)).toContainText("127.0.0.1:9471/metrics");
    await expect(rows.nth(1)).toContainText("None (loopback or management network only)");
    const steps = page.getByTestId("monitoring-steps").getByRole("listitem");
    await expect(steps).toHaveCount(4);
    await expect(steps.nth(0)).toContainText("Metrics Reader");
    await expect(steps.nth(3)).toContainText("incus config trust add-certificate metrics.crt --type=metrics");
    await expect(steps.nth(3)).toContainText("/1.0/metrics");
    // Read-only: the panel offers no action of its own.
    await expect(panel.getByRole("button")).toHaveCount(0);
  });

  test("says when the sign-in cannot read metrics, labels mock data and lists a separate listener", async ({ page }) => {
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("mock", false) }),
      "GET /api/v1/whoami": () => ({ json: whoami("auditor", ["read-inventory", "read-audit"]) }),
      "GET /api/v1/system": () => ({ json: system }),
      "GET /api/v1/monitoring": () => ({ json: monitoring({ provider: "mock", mayScrape: false, region: "default", site: "default", sector: "default", endpoints: [
        { component: "control-plane", address: null, path: "/metrics", authentication: "bearer" },
        { component: "control-plane-listener", address: "10.0.0.5:9470", path: "/metrics", authentication: "none" },
        { component: "worker", address: "127.0.0.1:9471", path: "/metrics", authentication: "none" },
      ] }) }),
    });
    await page.goto("/#/administration");
    await expect(page.getByTestId("monitoring-access")).toContainText("Cannot read metrics. A collector needs fabric.metrics.read.");
    await expect(page.getByTestId("monitoring")).toContainText("Mock · not a real cluster");
    await expect(page.getByTestId("monitoring-labels")).toHaveText("region=default · site=default · sector=default");
    await expect(page.getByTestId("metrics-endpoint")).toHaveCount(3);
    await expect(page.getByTestId("metrics-endpoint").nth(1)).toContainText("Control plane (separate listener)");
    await expect(page.getByTestId("metrics-endpoint").nth(1)).toContainText("10.0.0.5:9470/metrics");
  });

  test("an error from the API is shown with a retry, never an invented set-up", async ({ page }) => {
    await stub(page, {
      "GET /api/v1/health": () => ({ json: health("incus", true) }),
      "GET /api/v1/whoami": () => ({ json: whoami("operator", OPERATOR) }),
      "GET /api/v1/system": () => ({ json: system }),
      "GET /api/v1/monitoring": () => ({ status: 403, json: { error: { code: "forbidden", message: "you may not do this (fabric.metrics.read)" } } }),
    });
    await page.goto("/#/administration");
    await expect(page.getByTestId("monitoring")).toContainText("you may not do this (fabric.metrics.read)");
    await expect(page.getByTestId("metrics-endpoint")).toHaveCount(0);
  });

  test.describe("on a phone", () => {
    test.use({ viewport: { width: 390, height: 844 }, isMobile: true, hasTouch: true });
    test("the panel fits without sideways scrolling", async ({ page }) => {
      await stub(page, {
        "GET /api/v1/health": () => ({ json: health("incus", true) }),
        "GET /api/v1/whoami": () => ({ json: whoami("fabric-admin", ADMIN) }),
        "GET /api/v1/system": () => ({ json: system }),
        "GET /api/v1/tokens": () => ({ json: { items: [] } }),
        "GET /api/v1/monitoring": () => ({ json: monitoring() }),
      });
      await page.goto("/#/administration");
      await expect(page.getByTestId("metrics-endpoint")).toHaveCount(2);
      expect(await page.evaluate(() => document.documentElement.scrollWidth)).toBeLessThanOrEqual(390);
    });
  });
});
