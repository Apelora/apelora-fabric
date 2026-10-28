// SPDX-License-Identifier: AGPL-3.0-only
import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page, type Request, type Route } from "@playwright/test";

// Servers → Sectors against a stubbed control plane (MOCK VERIFIED): the home sector and the
// registered ones with their state, registering one (only names are sent), and forgetting one
// after typing its name. The API's own rules are tested in sectors_tests.rs.

const NOW = "2026-10-26T12:00:00Z";
const health = { status: "ok", service: "control-plane", version: "0.1.0", apiVersion: "v1", provider: "incus", auth: "tokens", incus: { connected: true }, checkedAt: NOW };
const who = (platform: string[]) => ({ name: "alex", kind: "user", role: null, permissions: ["read-inventory"], capabilities: platform, platformPermissions: platform, roles: [], deployProjects: [] });
const home = { name: "default", site: "default", home: true, url: null, state: "connected", members: 4, incusVersion: "7.0.2", credentialVars: [] };
const rackB = { name: "rack-b1", site: "site-b", home: false, url: "https://incus.b.example:8443", state: "not-configured", members: null, incusVersion: null, detail: "APELORA_SECTOR_SITE_B_CLIENT_CERT is not set on the control plane", credentialVars: ["APELORA_SECTOR_SITE_B_CLIENT_CERT", "APELORA_SECTOR_SITE_B_CLIENT_KEY", "APELORA_SECTOR_SITE_B_SERVER_CERT"], addedBy: "user:alex", addedAt: NOW };
const note = "Applications deploy to the home sector only. Registered sectors are read for their state and count towards the sites a second deployment could use; deploying to them is a later step.";

async function rig(page: Page, platform: string[]) {
  await page.addInitScript(() => sessionStorage.setItem("apelora.apiToken", "eyJhbGciOiJFUzI1NiJ9.e30.stub"));
  let items: unknown[] = [home];
  const sent: { method: string; body: unknown }[] = [];
  await page.route("**/api/**", async (route: Route) => {
    const req: Request = route.request();
    const path = new URL(req.url()).pathname;
    await new Promise((r) => setTimeout(r, 60));
    if (path === "/api/v1/health") return route.fulfill({ json: health });
    if (path === "/api/v1/whoami") return route.fulfill({ json: who(platform) });
    if (path === "/api/v1/sectors" && req.method() === "GET") return route.fulfill({ json: { source: { provider: "incus", observedAt: NOW }, items, note } });
    if (path === "/api/v1/sectors" && req.method() === "POST") {
      sent.push({ method: "POST", body: req.postDataJSON() });
      items = [home, rackB];
      return route.fulfill({ status: 201, json: rackB });
    }
    if (path === "/api/v1/sectors/rack-b1" && req.method() === "DELETE") {
      sent.push({ method: "DELETE", confirm: new URL(req.url()).searchParams.get("confirm"), body: req.postData() });
      items = [home];
      return route.fulfill({ status: 204 });
    }
    return route.abort();
  });
  return sent;
}

test.describe("sectors", () => {
  test("an administrator registers a sector by name and forgets it after typing the name", async ({ page }) => {
    const sent = await rig(page, ["cluster.view", "cluster.manage"]);
    await page.goto("/#/servers/sectors");
    const rows = page.getByTestId("sector-row");
    await expect(rows).toHaveCount(1);
    await expect(rows.first()).toContainText("home");
    await expect(rows.first()).toHaveAttribute("data-state", "connected");
    await expect(page.getByTestId("sectors-note")).toContainText("home sector only");

    await page.getByTestId("sector-add").click();
    const dialog = page.getByTestId("sector-dialog");
    await expect(dialog.getByTestId("sector-save")).toBeDisabled();
    await dialog.getByTestId("sector-name").fill("rack-b1");
    await dialog.getByTestId("sector-site").fill("site-b");
    await dialog.getByTestId("sector-url").fill("https://incus.b.example:8443");
    await dialog.getByTestId("sector-credentials").fill("SITE_B");
    await expect(dialog.getByTestId("sector-vars")).toContainText("APELORA_SECTOR_SITE_B_CLIENT_KEY");
    await dialog.getByTestId("sector-save").click();
    await expect(rows).toHaveCount(2);
    expect(sent[0]).toEqual({ method: "POST", body: { name: "rack-b1", site: "site-b", url: "https://incus.b.example:8443", credentials: "SITE_B" } });
    const b = rows.nth(1);
    await expect(b).toHaveAttribute("data-state", "not-configured");
    await expect(b).toContainText("Identity missing");
    await expect(b).toContainText("Set APELORA_SECTOR_SITE_B_CLIENT_CERT");
    // Checked once the dialog has closed: mid-fade its text measures as low contrast.
    await expect(dialog).toBeHidden();
    expect((await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze()).violations.filter((v) => v.impact === "serious" || v.impact === "critical")).toEqual([]);

    await b.getByTestId("sector-forget").click();
    const confirm = page.getByTestId("confirm-dialog");
    await expect(confirm.getByTestId("confirm-action")).toBeDisabled();
    await confirm.getByTestId("confirm-input").fill("rack-b1");
    await confirm.getByTestId("confirm-action").click();
    await expect(rows).toHaveCount(1);
    expect(sent[1]).toEqual({ method: "DELETE", confirm: "rack-b1", body: null });
  });

  test("people who may only look see the sectors without the actions", async ({ page }) => {
    await rig(page, ["cluster.view"]);
    await page.goto("/#/servers/sectors");
    await expect(page.getByTestId("sector-row")).toHaveCount(1);
    await expect(page.getByTestId("sector-add")).toHaveCount(0);
  });
});
