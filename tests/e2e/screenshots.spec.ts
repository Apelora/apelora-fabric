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

    await node(page, "nginx-01").click({ button: "right" });
    await page.screenshot({ path: out("10-context-menu-appliance") });
    await page.keyboard.press("Escape");
    await page.locator(".react-flow__node[data-id=postgresql-01] .react-flow__handle[data-port-id=postgres]").click({ button: "right", force: true });
    await page.screenshot({ path: out("11-context-menu-port") });
    await page.keyboard.press("Escape");
    const cv = (await page.getByTestId("canvas").boundingBox())!;
    await page.mouse.click(cv.x + 120, cv.y + 120, { button: "right" });
    await page.getByTestId("menu-add").hover();
    await page.screenshot({ path: out("12-context-menu-canvas") });
    await page.keyboard.press("Escape");
    await node(page, "postgresql-01").click();
    await page.locator("[data-testid=cfg-group][data-group=Limits] summary").click();
    await page.getByTestId("cfg-max-connections").fill("5");
    await page.getByTestId("cfg-max-connections").press("Enter");
    await page.screenshot({ path: out("13-configuration-form") });
    await page.getByTestId("cfg-max-connections").fill("250");
    await page.getByTestId("cfg-max-connections").press("Enter");
    await node(page, "grafana-01").click();
    await addPorts(page, "input", 5);
    await page.waitForTimeout(200);
    await page.screenshot({ path: out("03-ports-grow-node") });

    await page.locator("[data-testid=catalog-tile][data-class-id=nginx]").click();
    await page.getByTestId("validation-toggle").click();
    await page.screenshot({ path: out("04-validation") });

    await page.getByTestId("new-appliance").click();
    await page.getByTestId("designer-name").fill("Session cache");
    await page.getByTestId("designer-add-output").click();
    await page.getByTestId("designer-port").first().getByLabel("Port type").selectOption("redis");
    await page.screenshot({ path: out("06-appliance-designer") });
    await page.getByTestId("designer-tab-config").click();
    await page.getByTestId("designer-add-prop").click();
    await page.getByTestId("designer-prop").first().getByLabel("Field label").fill("Max connections");
    await page.getByTestId("designer-prop").first().getByLabel("Field type").selectOption("int");
    await page.screenshot({ path: out("14-designer-configuration") });
    await page.getByTestId("designer-save").click();
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-sample").click();
    await page.getByTestId("density-expanded").click();
    await page.waitForTimeout(400);
    await page.screenshot({ path: out("05-expanded-density") });
  });
}
