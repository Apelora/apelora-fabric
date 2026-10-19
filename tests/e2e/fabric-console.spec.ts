import { expect, test } from "@playwright/test";

// The console must report the control plane and Incus honestly: never "connected" unless the API says so.
const health = (incusConnected: boolean) => ({
  status: "ok",
  service: "control-plane",
  version: "0.0.0",
  apiVersion: "v1",
  provider: incusConnected ? "incus" : "mock",
  incus: { connected: incusConnected, apiVersion: "1.0", serverVersion: null },
});

test.describe("fabric console", () => {
  test("control plane down: says so, shows how to start it, and retries", async ({ page }) => {
    let up = false;
    await page.route("**/api/v1/health", (route) => (up ? route.fulfill({ json: health(false) }) : route.abort()));
    await page.goto("/");
    const status = page.getByTestId("status");
    await expect(status).toContainText("Control plane");
    await expect(status).toContainText("Not reachable");
    await expect(status).toContainText("Unknown");
    await expect(page.getByTestId("mock-badge")).toHaveText("Mock · not connected");
    await expect(page.getByTestId("empty-state")).toContainText("cargo run -p apelora-control-plane");
    await expect(page.getByRole("button", { name: "Check again" })).toHaveCount(1);
    up = true;
    await page.getByRole("button", { name: "Check again" }).click();
    await expect(status).toContainText("Running · v0.0.0 · mock data");
    await expect(status).toContainText("Not connected");
    await expect(page.getByTestId("empty-state")).toContainText("No cluster connected yet");
  });

  test("Incus connected: no mock badge, no empty state", async ({ page }) => {
    await page.route("**/api/v1/health", (route) => route.fulfill({ json: health(true) }));
    await page.goto("/");
    await expect(page.getByTestId("status")).toContainText("Connected");
    await expect(page.getByTestId("mock-badge")).toHaveText("Connected");
    await expect(page.getByTestId("empty-state")).toHaveCount(0);
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
