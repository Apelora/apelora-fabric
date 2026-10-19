import { expect, test, type Page } from "@playwright/test";

// The console must report the control plane, Incus and the data's origin honestly: never
// "connected" or unlabelled data unless the API says it came from Incus.
type Provider = "mock" | "incus";
const now = "2026-10-20T00:00:00Z";
const health = (provider: Provider, connected: boolean, error?: string) => ({
  status: "ok", service: "control-plane", version: "0.0.0", apiVersion: "v1", provider,
  incus: connected ? { connected, apiVersion: "1.0", serverVersion: "7.0.2", clustered: true } : { connected, ...(error ? { error } : { apiVersion: "1.0", serverVersion: null }) },
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
  test("control plane down: says so, shows how to start it, retries into mock data that is labelled", async ({ page }) => {
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
    await expect(status).toContainText("Running · v0.0.0 · mock data");
    await expect(page.getByTestId("mock-badge")).toHaveText("Mock data");
    await expect(page.getByTestId("empty-state")).toContainText("APELORA_INCUS_URL");
    await expect(page.getByTestId("member-row")).toHaveCount(3);
    await expect(page.getByTestId("instance-row")).toHaveCount(2);
    await expect(page.getByTestId("instance-row").nth(1)).toContainText("Virtual machine");
    for (const note of await page.getByTestId("source-note").all()) await expect(note).toHaveText("Mock data · not a real cluster");
  });

  test("Incus connected: real data is attributed to Incus, no mock badge or empty state", async ({ page }) => {
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
      "/api/v1/health": { json: health("incus", false, message) },
      "/api/v1/cluster": { status: 502, json: { error: { code: "incus_unreachable", message } } },
      "/api/v1/instances": { status: 502, json: { error: { code: "incus_unreachable", message } } },
    });
    await page.goto("/");
    await expect(page.getByTestId("status")).toContainText("Unreachable");
    await expect(page.getByTestId("mock-badge")).toHaveText("Not connected");
    await expect(page.getByTestId("empty-state")).toContainText(message);
    await expect(page.getByTestId("members-error")).toHaveText(message);
    await expect(page.getByTestId("instances-error")).toHaveText(message);
    await expect(page.getByTestId("member-row")).toHaveCount(0);
  });

  test("token mode: asks for a token, sends it as a Bearer header, keeps it only for the tab", async ({ page }) => {
    const good = "apl_" + "a".repeat(64);
    const guard = (body: unknown) => (route: import("@playwright/test").Route) =>
      route.request().headers()["authorization"] === `Bearer ${good}`
        ? route.fulfill({ json: body })
        : route.fulfill({ status: 401, json: { error: { code: "unauthenticated", message: "an API token is required (Authorization: Bearer <token>)" } } });
    await page.route("**/api/v1/health", (route) => route.fulfill({ json: { ...health("mock", false), auth: "tokens" } }));
    await page.route("**/api/v1/cluster", guard(cluster("mock", ["mock-member-01"])));
    await page.route("**/api/v1/instances", guard(instances("mock")));
    await page.goto("/");
    await expect(page.getByTestId("status")).toContainText("API tokens required");
    const form = page.getByTestId("sign-in");
    await expect(form).toContainText("an API token is required");
    await expect(page.getByTestId("member-row")).toHaveCount(0);
    await expect(page.locator(".btn-primary")).toHaveCount(1);
    await page.getByTestId("token-input").fill(good);
    await form.getByRole("button", { name: "Sign in" }).click();
    await expect(page.getByTestId("member-row")).toHaveCount(1);
    await expect(form).toHaveCount(0);
    expect(await page.evaluate(() => [sessionStorage.getItem("apelora.apiToken"), localStorage.getItem("apelora.apiToken")])).toEqual([good, null]);
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
