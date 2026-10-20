import { test, expect } from "@playwright/test";
import { node, open, openProps } from "./helpers";

// Pixel baselines depend on fonts and the OS, so they are opt-in:
//   VISUAL=1 npx playwright test tests/e2e/visual.spec.ts --update-snapshots   (create or refresh)
//   VISUAL=1 npx playwright test tests/e2e/visual.spec.ts                      (compare)
test.skip(!process.env.VISUAL, "visual baselines are opt-in");
const opts = { maxDiffPixelRatio: 0.01, animations: "disabled" as const };

test("commerce stack canvas", async ({ page }) => {
  await open(page);
  await expect(page.getByTestId("canvas")).toHaveScreenshot("canvas-commerce.png", opts);
});
test("context menu on an appliance", async ({ page }) => {
  await open(page);
  await node(page, "nginx-01").click({ button: "right" });
  await expect(page.getByTestId("context-menu")).toHaveScreenshot("menu-appliance.png", opts);
});
test("properties window", async ({ page }) => {
  await open(page);
  await openProps(page, "haproxy-01");
  await expect(page.getByTestId("inspector")).toHaveScreenshot("properties.png", opts);
});
test("templates gallery", async ({ page }) => {
  await open(page);
  await page.getByTestId("new-menu").click();
  await page.getByTestId("new-template").click();
  await expect(page.getByTestId("templates-dialog")).toHaveScreenshot("templates.png", opts);
});
test("night theme canvas", async ({ page }) => {
  await open(page);
  await page.getByLabel("Theme").selectOption("night");
  await expect(page.getByTestId("canvas")).toHaveScreenshot("canvas-night.png", opts);
});
