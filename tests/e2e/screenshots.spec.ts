import { test } from "@playwright/test";
import { addPorts, node, open } from "./helpers";

// Run with: SCREENSHOTS=1 npx playwright test tests/e2e/screenshots.spec.ts
test.skip(!process.env.SCREENSHOTS, "screenshots are generated on demand");

for (const [w, h] of [[1440, 900], [1280, 800]] as const) {
  test(`screenshots ${w}x${h}`, async ({ page }) => {
    await page.setViewportSize({ width: w, height: h });
    await open(page);
    const out = (n: string) => `docs/screenshots/${n}-${w}x${h}.png`;
    await page.screenshot({ path: out("01-commerce-stack") });

    await node(page, "haproxy-01").click();
    await page.getByTestId("catalog-search").hover();
    await page.screenshot({ path: out("02-selected-inspector") });

    await node(page, "grafana-01").click();
    await addPorts(page, "input", 5);
    await page.waitForTimeout(200);
    await page.screenshot({ path: out("03-ports-grow-node") });

    await page.locator("[data-testid=catalog-tile][data-class-id=nginx]").click();
    await page.getByTestId("validation-toggle").click();
    await page.screenshot({ path: out("04-validation") });

    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-sample").click();
    await page.getByTestId("density-expanded").click();
    await page.waitForTimeout(400);
    await page.screenshot({ path: out("05-expanded-density") });
  });
}
