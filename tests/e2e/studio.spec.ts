import { readFileSync } from "node:fs";
import { APPLIANCE_CLASSES } from "../../packages/appliance-catalog/src/classes";
import { test } from "@playwright/test";
import { openProps, addPorts, assemblyState, cablePoint, center, dragHandle, edgeCount, expect, expectEdgesAnchored, flowSize, handle, node, open, resolvedColor, viewport } from "./helpers";

const rgb = (page: import("@playwright/test").Page, sel: string, prop = "backgroundColor") => (prop === "backgroundColor" ? resolvedColor(page, sel) : page.locator(sel).first().evaluate((el, p) => getComputedStyle(el)[p as "color"], prop));

test.describe("visual contract", () => {
  test("white catalog and properties window around a distinct light-gray framed canvas; no always-on inspector", async ({ page }) => {
    await open(page);
    await expect(page.getByTestId("inspector")).toHaveCount(0); // properties are opened on demand
    expect(await rgb(page, "[data-testid=catalog]")).toBe("rgb(255, 255, 255)");
    await openProps(page, "nginx-01");
    expect(await rgb(page, "[data-testid=inspector]")).toBe("rgb(255, 255, 255)");
    expect(await rgb(page, "[data-testid=canvas-frame]")).toBe("rgb(247, 247, 248)");
    expect(await rgb(page, "[data-testid=canvas-frame]", "borderTopLeftRadius")).toBe("12px");
    const cat = await center(page.getByTestId("catalog"));
    expect(cat.width).toBeGreaterThanOrEqual(160);
    expect(cat.width).toBeLessThanOrEqual(200);
    const insp = await center(page.getByTestId("inspector"));
    expect(insp.width).toBeGreaterThanOrEqual(380);
    expect(insp.width).toBeLessThanOrEqual(440);
    await page.getByTestId("props-close").click();
    await expect(page.getByTestId("inspector")).toHaveCount(0);
    // the canvas now spans the full width right of the catalog
    const frame = await center(page.getByTestId("canvas-frame"));
    expect(frame.width).toBeGreaterThan(1100);
    await expect(page.getByTestId("mock-badge")).toContainText("Design only");
  });

  test("compact nodes are 128×96 with a 24px header, a centered 30px logo and soft shadow", async ({ page }) => {
    await open(page);
    const { zoom } = await viewport(page);
    const n = node(page, "nginx-01");
    const box = await n.boundingBox();
    expect(box!.width / zoom).toBeCloseTo(128, 0);
    expect(box!.height / zoom).toBeCloseTo(96, 0);
    const header = await n.locator(".apl-node-header").boundingBox();
    expect(header!.height / zoom).toBeCloseTo(24, 0);
    const logo = await n.getByTestId("node-logo").boundingBox();
    expect(logo!.width / zoom).toBeCloseTo(30, 0);
    const nodeC = await center(n);
    expect(Math.abs(logo!.x + logo!.width / 2 - nodeC.x)).toBeLessThan(1.5);
    expect(await n.evaluate((el) => getComputedStyle(el).boxShadow)).toContain("16px");
    expect(await n.evaluate((el) => getComputedStyle(el).borderRadius)).toBe("8px");
  });

  test("six-appliance commerce stack fits the framed canvas at the default view", async ({ page }) => {
    await open(page);
    const canvas = await page.getByTestId("canvas").boundingBox();
    const ids = ["gateway-01", "haproxy-01", "nginx-01", "nginx-02", "postgresql-01", "grafana-01"];
    for (const id of ids) {
      const b = (await node(page, id).boundingBox())!;
      expect(b.x).toBeGreaterThanOrEqual(canvas!.x);
      expect(b.y).toBeGreaterThanOrEqual(canvas!.y);
      expect(b.x + b.width).toBeLessThanOrEqual(canvas!.x + canvas!.width);
      expect(b.y + b.height).toBeLessThanOrEqual(canvas!.y + canvas!.height);
    }
    await expect(page.getByTestId("zoom-level")).not.toHaveText("0%");
    await expectEdgesAnchored(page);
  });

  test("ports: inputs on the left, outputs on the right, with accessible names", async ({ page }) => {
    await open(page);
    const n = (await center(node(page, "haproxy-01")));
    const inH = await center(handle(page, "haproxy-01", "frontend"));
    const outH = await center(handle(page, "haproxy-01", "backend"));
    expect(inH.x).toBeLessThan(n.x);
    expect(outH.x).toBeGreaterThan(n.x);
    expect(inH.width).toBeCloseTo(10 * (await viewport(page)).zoom, 0);
    await expect(handle(page, "haproxy-01", "backend")).toHaveAttribute("aria-label", /out \(http\)/);
  });
});

test.describe("context menus", () => {
  const menu = (page: import("@playwright/test").Page) => page.getByTestId("context-menu");
  const item = (page: import("@playwright/test").Page, id: string) => page.getByTestId(`menu-${id}`);

  test("appliance menu: lists every action; rename, duplicate, add port, reset size, unplug, delete", async ({ page }) => {
    await open(page);
    await node(page, "nginx-01").click({ button: "right" });
    await expect(menu(page)).toBeVisible();
    await expect(menu(page)).toContainText("nginx-01");
    for (const id of ["rename", "duplicate", "copy", "add-port", "reset-size", "disconnect", "zoom", "save-class", "delete"]) await expect(item(page, id)).toBeVisible();
    await expect(item(page, "reset-size")).toBeDisabled(); // not manually resized
    await expect(item(page, "edit-class")).toHaveCount(0); // built-in class
    // rename focuses the inspector field
    await item(page, "rename").click();
    await expect(page.getByTestId("instance-name")).toBeFocused();
    // add an input port via the submenu
    const portsBefore = (await assemblyState(page)).spec.appliances.find((a) => a.id === "nginx-01")!.ports.length;
    await node(page, "nginx-01").click({ button: "right" });
    await item(page, "add-port").click();
    await page.getByTestId("menu-add-input").click();
    expect((await assemblyState(page)).spec.appliances.find((a) => a.id === "nginx-01")!.ports.length).toBe(portsBefore + 1);
    // duplicate: new appliance, no cables copied for a single node
    const before = await assemblyState(page);
    await node(page, "nginx-01").click({ button: "right" });
    await item(page, "duplicate").click();
    const after = await assemblyState(page);
    expect(after.spec.appliances.length).toBe(before.spec.appliances.length + 1);
    expect(after.spec.connections.length).toBe(before.spec.connections.length);
    await expect(menu(page)).toHaveCount(0);
    // unplug all cables, then undo
    await node(page, "haproxy-01").click({ button: "right" });
    await item(page, "disconnect").click();
    expect((await assemblyState(page)).spec.connections.some((c) => c.source.applianceId === "haproxy-01" || c.target.applianceId === "haproxy-01")).toBe(false);
    await page.getByTestId("undo").click();
    // delete from the menu
    await node(page, "grafana-01").click({ button: "right" });
    await item(page, "delete").click();
    expect((await assemblyState(page)).spec.appliances.some((a) => a.id === "grafana-01")).toBe(false);
  });

  test("multi-selection menu: align and delete", async ({ page }) => {
    await open(page);
    await node(page, "nginx-01").click();
    await node(page, "grafana-01").click({ modifiers: ["Control"] });
    await node(page, "grafana-01").click({ button: "right" });
    await expect(menu(page)).toContainText("2 appliances selected");
    await item(page, "align").click();
    await page.getByTestId("menu-align-left").click();
    const st = await assemblyState(page);
    const xs = ["nginx-01", "grafana-01"].map((id) => st.editor.nodes.find((n) => n.id === id)!.x);
    expect(xs[0]).toBe(xs[1]);
    await node(page, "grafana-01").click({ button: "right" });
    await item(page, "delete").click();
    expect((await assemblyState(page)).spec.appliances.length).toBe(4);
  });

  test("port menu: type, multi-cable, required, unplug, reorder, delete", async ({ page }) => {
    await open(page);
    const pg = handle(page, "postgresql-01", "postgres");
    await pg.click({ button: "right", force: true });
    await expect(menu(page)).toContainText("sql (postgres)");
    await expect(item(page, "multi")).toHaveAttribute("aria-checked", "true");
    await item(page, "multi").click();
    await expect(pg).toHaveAttribute("aria-label", /single cable/);
    await pg.click({ button: "right", force: true });
    await item(page, "required").click();
    await expect(pg).toHaveAttribute("aria-label", /sql \(postgres\)(?!, required)/);
    await pg.click({ button: "right", force: true });
    await item(page, "type").click();
    await page.getByTestId("menu-type-tcp").click();
    expect((await assemblyState(page)).spec.appliances.find((a) => a.id === "postgresql-01")!.ports.find((p) => p.id === "postgres")!.type).toBe("tcp");
    await pg.click({ button: "right", force: true });
    await expect(item(page, "unplug")).toContainText("(3)");
    await item(page, "unplug").click();
    expect((await assemblyState(page)).spec.connections.length).toBe(3);
    await handle(page, "postgresql-01", "metrics").click({ button: "right", force: true });
    await item(page, "delete-port").click();
    expect((await assemblyState(page)).spec.appliances.find((a) => a.id === "postgresql-01")!.ports.length).toBe(2);
  });

  test("cable menu: select ends and unplug", async ({ page }) => {
    await open(page);
    const p = await cablePoint(page, "conn-01", 0.5);
    await page.mouse.click(p.x, p.y + 0, { button: "right" });
    await expect(menu(page)).toContainText("gateway-01.lan → haproxy-01.in");
    await item(page, "select-target").click();
    await expect(node(page, "haproxy-01")).toHaveAttribute("data-selected", "true");
    const q = await cablePoint(page, "conn-01", 0.5);
    await page.mouse.click(q.x, q.y, { button: "right" });
    await item(page, "delete").click();
    expect((await assemblyState(page)).spec.connections.some((c) => c.id === "conn-01")).toBe(false);
  });

  test("canvas menu: add appliance at the pointer, paste at the pointer, select all, view toggles", async ({ page }) => {
    await open(page);
    const canvas = (await page.getByTestId("canvas").boundingBox())!;
    const at = { x: canvas.x + canvas.width * 0.25, y: canvas.y + canvas.height * 0.8 };
    await page.mouse.click(at.x, at.y, { button: "right" });
    await expect(menu(page)).toContainText("Canvas");
    await expect(item(page, "paste")).toBeDisabled();
    await item(page, "add").click();
    await page.getByTestId("menu-add-redis").click();
    const st = await assemblyState(page);
    const redis = st.spec.appliances.find((a) => a.id.startsWith("redis"))!;
    const c = await center(node(page, redis.id));
    expect(Math.abs(c.x - at.x)).toBeLessThan(3);
    expect(Math.abs(c.y - at.y)).toBeLessThan(3);
    // copy it, paste elsewhere via the canvas menu
    await page.keyboard.press("Control+c");
    const at2 = { x: canvas.x + canvas.width * 0.55, y: canvas.y + canvas.height * 0.92 };
    await page.mouse.click(at2.x, at2.y, { button: "right" });
    await item(page, "paste").click();
    const st2 = await assemblyState(page);
    const pasted = st2.spec.appliances.find((a) => a.id.startsWith("redis") && a.id !== redis.id)!;
    const c2 = await center(node(page, pasted.id));
    expect(Math.abs(c2.x - at2.x)).toBeLessThan(80); // pasted block's top-left lands at the pointer
    expect(c2.x).toBeGreaterThan(at2.x - 10);
    // select all, view toggles
    await page.mouse.click(at.x, at.y - 60, { button: "right" });
    await item(page, "select-all").click();
    await expect(page.getByTestId("selection-info")).toContainText("selected");
    await page.mouse.click(at.x, at.y - 60, { button: "right" });
    await item(page, "labels").click();
    await expect(page.getByTestId("port-label").first()).toBeHidden();
  });

  test("keyboard: Context Menu key opens, arrows navigate, Escape closes; outside click closes; Ctrl+D/C/V/A work", async ({ page }) => {
    await open(page);
    await node(page, "nginx-01").click();
    await page.keyboard.press("ContextMenu");
    await expect(menu(page)).toBeVisible();
    await expect(item(page, "properties")).toBeFocused();
    await page.keyboard.press("ArrowDown");
    await expect(item(page, "rename")).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(menu(page)).toHaveCount(0);
    await page.keyboard.press("Control+d");
    expect((await assemblyState(page)).spec.appliances.length).toBe(7);
    await page.keyboard.press("Control+a");
    await expect(page.getByTestId("selection-info")).toContainText("7 selected");
    await node(page, "gateway-01").click({ button: "right" });
    await expect(menu(page)).toBeVisible();
    await page.mouse.click(10, 500); // outside
    await expect(menu(page)).toHaveCount(0);
  });
});

test.describe("configuration forms", () => {
  const field = (page: import("@playwright/test").Page, id: string) => page.locator(`[data-testid=cfg-field][data-prop-id="${id}"]`);
  const cfg = async (page: import("@playwright/test").Page, appId: string) => (await assemblyState(page)).spec.appliances.find((a) => a.id === appId)!.config;

  test("typed fields: int, enum, bool, string, secret with validation, reset and persistence", async ({ page }) => {
    await open(page);
    await openProps(page, "postgresql-01", "config");
    await expect(page.getByTestId("config-form")).toBeVisible();
    await expect(page.getByTestId("cfg-group")).toHaveCount(3);
    expect(await cfg(page, "postgresql-01")).toBeUndefined(); // defaults only, nothing stored
    // enum
    await page.getByTestId("cfg-version").selectOption("16");
    expect(await cfg(page, "postgresql-01")).toEqual({ version: "16" });
    await expect(field(page, "version")).toHaveAttribute("data-modified", "true");
    // picking the default again removes the override
    await page.getByTestId("cfg-version").selectOption("17");
    expect(await cfg(page, "postgresql-01")).toBeUndefined();
    // int with range validation (the Limits and Access groups start collapsed)
    await page.locator("[data-testid=cfg-group][data-group=Limits] summary").click();
    await page.locator("[data-testid=cfg-group][data-group=Access] summary").click();
    await page.getByTestId("cfg-max-connections").fill("5");
    await page.getByTestId("cfg-max-connections").press("Enter");
    await expect(field(page, "max-connections").getByTestId("cfg-error")).toContainText("at least 10");
    await expect(page.getByTestId("error-count")).toBeVisible();
    await expect(node(page, "postgresql-01")).toHaveAttribute("data-status", "error");
    await page.getByTestId("cfg-max-connections").fill("250");
    await page.getByTestId("cfg-max-connections").press("Enter");
    await expect(field(page, "max-connections").getByTestId("cfg-error")).toHaveCount(0);
    await expect(node(page, "postgresql-01")).toHaveAttribute("data-status", "ok");
    // required string cannot be emptied silently (falls back to default); secret accepts references only
    await page.getByTestId("cfg-password-secret").fill("My Real Password!");
    await page.getByTestId("cfg-password-secret").press("Enter");
    await expect(field(page, "password-secret").getByTestId("cfg-error")).toContainText("Never paste");
    await page.getByTestId("cfg-password-secret").fill("shop-db-password");
    await page.getByTestId("cfg-password-secret").press("Enter");
    expect(await cfg(page, "postgresql-01")).toEqual({ "max-connections": 250, "password-secret": "shop-db-password" });
    // survives reload; reset one, reset all
    await expect.poll(() => page.evaluate(() => localStorage.getItem("apelora.studio.v1") ?? "")).toContain("shop-db-password");
    await page.reload();
    await page.waitForSelector("[data-testid=appliance-node]");
    await openProps(page, "postgresql-01", "config");
    await expect(page.getByTestId("cfg-max-connections")).toHaveValue("250");
    await page.getByTestId("cfg-reset-max-connections").click();
    expect(await cfg(page, "postgresql-01")).toEqual({ "password-secret": "shop-db-password" });
    await page.getByTestId("cfg-reset-all").click();
    expect(await cfg(page, "postgresql-01")).toBeUndefined();
  });

  test("bool toggle and string field; configuration is exported and imported", async ({ page }) => {
    await open(page);
    await openProps(page, "nginx-01", "config");
    await page.getByTestId("cfg-gzip").uncheck();
    await page.getByTestId("cfg-server-name").fill("shop.example.test");
    await page.getByTestId("cfg-server-name").press("Enter");
    expect(await cfg(page, "nginx-01")).toEqual({ gzip: false, "server-name": "shop.example.test" });
    const dl = page.waitForEvent("download");
    await page.getByTestId("export").click();
    const text = readFileSync((await (await dl).path())!, "utf8");
    expect(JSON.parse(text).spec.appliances.find((a: { id: string }) => a.id === "nginx-01").config).toEqual({ gzip: false, "server-name": "shop.example.test" });
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-empty").click();
    await page.getByTestId("import-input").setInputFiles({ name: "c.json", mimeType: "application/json", buffer: Buffer.from(text) });
    await page.waitForSelector("[data-testid=appliance-node]");
    expect(await cfg(page, "nginx-01")).toEqual({ gzip: false, "server-name": "shop.example.test" });
  });

  test("context menu: Configure… focuses the form, Reset configuration restores defaults", async ({ page }) => {
    await open(page);
    await openProps(page, "haproxy-01", "config");
    await page.getByTestId("cfg-algorithm").selectOption("leastconn");
    await node(page, "haproxy-01").click({ button: "right" });
    await expect(page.getByTestId("menu-reset-config")).toContainText("1 changed");
    await page.getByTestId("menu-configure").click();
    await expect.poll(() => page.evaluate(() => document.activeElement?.closest("[data-testid=config-form]") !== null)).toBe(true);
    await node(page, "haproxy-01").click({ button: "right" });
    await page.getByTestId("menu-reset-config").click();
    expect(await cfg(page, "haproxy-01")).toBeUndefined();
    await node(page, "gateway-01").click({ button: "right" });
    await expect(page.getByTestId("menu-reset-config")).toBeDisabled();
  });

  test("designer: define configuration fields, reject bad defaults, create, configure an instance", async ({ page }) => {
    await open(page);
    await page.getByTestId("new-appliance").click();
    await page.getByTestId("designer-name").fill("Job runner");
    await page.getByTestId("designer-tab-config").click();
    // enum field with too few options is rejected
    await page.getByTestId("designer-add-prop").click();
    const p1 = page.getByTestId("designer-prop").nth(0);
    await p1.getByLabel("Field label").fill("Mode");
    await p1.getByLabel("Field type").selectOption("enum");
    await p1.getByLabel("Options").fill("fast");
    await page.getByTestId("designer-save").click();
    await expect(page.getByTestId("designer-errors")).toContainText("at least two options");
    await p1.getByLabel("Options").fill("fast, safe");
    await p1.getByLabel("Default value").fill("safe");
    await page.getByTestId("designer-add-prop").click();
    const p2 = page.getByTestId("designer-prop").nth(1);
    await p2.getByLabel("Field label").fill("Workers");
    await p2.getByLabel("Field type").selectOption("int");
    await p2.getByLabel("Minimum").fill("1");
    await p2.getByLabel("Maximum").fill("8");
    await p2.getByLabel("Default value").fill("20"); // outside the range
    await page.getByTestId("designer-save").click();
    await expect(page.getByTestId("designer-errors")).toContainText("at most 8");
    await p2.getByLabel("Default value").fill("4");
    await p2.locator("input[type=checkbox]").check();
    await page.getByTestId("designer-save").click();
    await expect(page.locator("[data-testid=catalog-tile][data-class-id=custom-job-runner]")).toBeVisible();
    await page.locator("[data-testid=catalog-tile][data-class-id=custom-job-runner]").click();
    await openProps(page, "custom-job-runner-01", "config");
    await expect(page.getByTestId("cfg-mode")).toHaveValue("safe");
    await expect(page.getByTestId("cfg-workers")).toHaveValue("4");
    await page.getByTestId("cfg-workers").fill("9");
    await page.getByTestId("cfg-workers").press("Enter");
    await expect(field(page, "workers").getByTestId("cfg-error")).toContainText("at most 8");
  });

  test("save as custom appliance carries the configured values as new defaults", async ({ page }) => {
    await open(page);
    await page.locator("[data-testid=catalog-tile][data-class-id=redis]").click();
    const id = (await assemblyState(page)).spec.appliances.find((a) => a.id.startsWith("redis"))!.id;
    await openProps(page, id, "config");
    await page.getByTestId("cfg-maxmemory").fill("2048");
    await page.getByTestId("cfg-maxmemory").press("Enter");
    await page.getByTestId("props-tab-general").click();
    await page.getByTestId("save-as-class").click();
    await page.getByTestId("designer-name").fill("Big cache");
    await page.getByTestId("designer-tab-config").click();
    await expect(page.getByTestId("designer-prop").first().getByLabel("Default value")).toHaveValue("2048");
    await page.getByTestId("designer-save").click();
    await page.locator("[data-testid=catalog-tile][data-class-id=custom-big-cache]").click();
    const id2 = (await assemblyState(page)).spec.appliances.find((a) => a.id.startsWith("custom-big-cache"))!.id;
    await openProps(page, id2, "config");
    await expect(page.getByTestId("cfg-maxmemory")).toHaveValue("2048");
    void id;
  });
});

test.describe("category colours and cable routing", () => {
  test("category accent on node headers, class footer, catalog; toggle and persistence", async ({ page }) => {
    await open(page);
    const hdr = (id: string) => node(page, id).locator(".apl-node-header");
    const shadow = (id: string) => hdr(id).evaluate((el) => getComputedStyle(el).boxShadow);
    expect(await shadow("nginx-01")).not.toBe(await shadow("postgresql-01")); // web vs database accent
    expect(await shadow("nginx-01")).toBe(await shadow("nginx-02"));
    await expect(node(page, "nginx-01").getByTestId("node-class")).toHaveText("NGINX");
    await expect(page.locator("[data-testid=catalog-tile][data-class-id=nginx]")).toBeVisible();
    // toggle off via the canvas menu; survives reload
    const canvas = (await page.getByTestId("canvas").boundingBox())!;
    await page.mouse.click(canvas.x + 80, canvas.y + 80, { button: "right" });
    await page.getByTestId("menu-category").click();
    await expect(node(page, "nginx-01").getByTestId("node-class")).toBeHidden();
    expect(await shadow("nginx-01")).toBe("none");
    await expect.poll(() => page.evaluate(() => localStorage.getItem("apelora.studio.view.v1") ?? "")).toContain('"showCategory":false');
    await page.reload();
    await page.waitForSelector("[data-testid=appliance-node]");
    await expect(node(page, "nginx-01").getByTestId("node-class")).toBeHidden();
  });

  test("right-angle routing: cables use straight segments, stay anchored, and the choice persists", async ({ page }) => {
    await open(page);
    const d = () => page.locator(".react-flow__edge[data-id=conn-02] path.react-flow__edge-path").getAttribute("d");
    expect(await d()).toContain("C"); // curved (bezier)
    await page.getByTestId("toggle-routing").click();
    const angled = (await d())!;
    expect(angled).not.toContain("C");
    expect(angled).toMatch(/L/);
    await expectEdgesAnchored(page);
    await expect.poll(() => page.evaluate(() => localStorage.getItem("apelora.studio.view.v1") ?? "")).toContain('"routing":"angled"');
    await page.reload();
    await page.waitForSelector("[data-testid=appliance-node]");
    expect((await d())!).not.toContain("C");
    await page.getByTestId("toggle-routing").click();
    expect(await d()).toContain("C");
  });
});

test.describe("network interface and link tags", () => {
  const stubs = (page: import("@playwright/test").Page, tag?: string) => page.locator(tag ? `[data-testid=tag-stub][data-tag="${tag}"]` : "[data-testid=tag-stub]");

  const dropAt = async (page: import("@playwright/test").Page, cls: string, fx: number, fy: number) => {
    const before = (await assemblyState(page)).spec.appliances.map((a) => a.id);
    const box = (await page.getByTestId("canvas").boundingBox())!;
    await page.locator(`[data-testid=catalog-tile][data-class-id=${cls}]`).dragTo(page.getByTestId("canvas"), { targetPosition: { x: box.width * fx, y: box.height * fy } });
    return (await assemblyState(page)).spec.appliances.map((a) => a.id).find((id) => !before.includes(id))!;
  };

  test("every appliance shows a net terminal with a link light; default network until plugged", async ({ page }) => {
    await open(page);
    for (const id of ["gateway-01", "haproxy-01", "nginx-01", "postgresql-01", "grafana-01"]) {
      await expect(handle(page, id, "net")).toHaveAttribute("aria-label", /net \(network\).*single cable/);
      await expect(node(page, id).getByTestId("nic-led")).toHaveAttribute("data-linked", "false");
    }
    await openProps(page, "nginx-01");
    await expect(page.getByTestId("nic-status")).toHaveText("default network");
    // plug it into a Network appliance by cable
    const net = await dropAt(page, "network", 0.12, 0.85);
    await dragHandle(page, handle(page, net, "net"), handle(page, "nginx-01", "net"));
    await expect(node(page, "nginx-01").getByTestId("nic-led")).toHaveAttribute("data-linked", "true");
    await openProps(page, "nginx-01");
    await expect(page.getByTestId("nic-status")).toContainText(`${net} (cable)`);
    // a NIC takes one network: a second cable is refused
    const net2 = await dropAt(page, "network", 0.3, 0.88);
    const before = await edgeCount(page);
    await dragHandle(page, handle(page, net2, "net"), handle(page, "nginx-01", "net"));
    expect(await edgeCount(page)).toBe(before);
    // a service port is not a NIC
    await dragHandle(page, handle(page, net2, "net"), handle(page, "haproxy-01", "frontend"));
    expect(await edgeCount(page)).toBe(before);
    // the net terminal cannot be deleted
    await openProps(page, "nginx-01", "ports");
    await expect(page.locator("[data-testid=port-row][data-port-id=net] [data-testid=delete-port]")).toBeDisabled();
  });

  test("link tags join terminals by name without a cable; hover highlights; convert to cable and back", async ({ page }) => {
    await open(page);
    const node1 = await dropAt(page, "nodejs", 0.1, 0.8);
    const redis = await dropAt(page, "redis", 0.62, 0.8);
    const edges = await edgeCount(page);
    await expect(handle(page, redis, "redis")).toBeVisible();
    // tag the redis input first: no source yet → warning
    await openProps(page, redis, "ports");
    await page.locator("[data-testid=port-row][data-port-id=redis] [data-testid=port-tag]").fill("cache");
    await page.locator("[data-testid=port-row][data-port-id=redis] [data-testid=port-tag]").press("Enter");
    await expect(stubs(page, "cache")).toHaveCount(1);
    await page.getByTestId("validation-toggle").click();
    await expect(page.getByTestId("diagnostics")).toContainText("nothing feeds it");
    await openProps(page, node1, "ports");
    await page.locator("[data-testid=port-row][data-port-id=cache] [data-testid=port-tag]").fill("cache");
    await page.locator("[data-testid=port-row][data-port-id=cache] [data-testid=port-tag]").press("Enter");
    await expect(stubs(page, "cache")).toHaveCount(2);
    expect(await edgeCount(page)).toBe(edges); // no cable drawn
    await expect(handle(page, node1, "cache").getByTestId("plug-head")).toHaveAttribute("data-type", "redis");
    await expect(page.getByTestId("diagnostics")).not.toContainText("nothing feeds it");
    // hovering one stub highlights the other (close the properties window first: it floats over the canvas)
    await page.getByTestId("props-close").click();
    await stubs(page, "cache").first().hover();
    await expect(stubs(page, "cache").nth(1)).toHaveAttribute("data-hot", "true");
    // selecting the tag shows its members; convert to a cable
    await stubs(page, "cache").first().click();
    await expect(page.getByTestId("inspector-tag")).toBeVisible();
    await expect(page.getByTestId("tag-members").locator("li")).toHaveCount(2);
    await page.getByTestId("tag-to-cable").click();
    await expect(stubs(page)).toHaveCount(0);
    expect(await edgeCount(page)).toBe(edges + 1);
    await expectEdgesAnchored(page);
    // cable → tag via the cable's context menu
    const id = (await assemblyState(page)).spec.connections.find((c) => c.source.applianceId === node1 && c.target.applianceId === redis)!.id;
    const pt = await cablePoint(page, id, 0.5);
    await page.mouse.click(pt.x, pt.y, { button: "right" });
    await page.getByTestId("menu-to-tag").click();
    await expect(stubs(page)).toHaveCount(2);
    expect(await edgeCount(page)).toBe(edges);
    // remove the tag
    await stubs(page).first().click();
    await page.getByTestId("tag-remove").click();
    await expect(stubs(page)).toHaveCount(0);
  });

  test("tag validation: protocol mismatch is rejected with a reason; port menu lists existing tags", async ({ page }) => {
    await open(page);
    const nodejs = await dropAt(page, "nodejs", 0.1, 0.8);
    const web = await dropAt(page, "nginx", 0.55, 0.8);
    await openProps(page, nodejs, "ports");
    await page.locator("[data-testid=port-row][data-port-id=cache] [data-testid=port-tag]").fill("link-a");
    await page.locator("[data-testid=port-row][data-port-id=cache] [data-testid=port-tag]").press("Enter");
    await expect(stubs(page, "link-a")).toHaveCount(1);
    await openProps(page, web, "ports");
    await page.locator("[data-testid=port-row][data-port-id=http] [data-testid=port-tag]").fill("link-a");
    await page.locator("[data-testid=port-row][data-port-id=http] [data-testid=port-tag]").press("Enter");
    await expect(page.getByTestId("toast").last()).toContainText(/cannot feed/);
    await expect(stubs(page, "link-a")).toHaveCount(1);
    await page.locator("[data-testid=port-row][data-port-id=http] [data-testid=port-tag]").fill("Bad Name");
    await page.locator("[data-testid=port-row][data-port-id=http] [data-testid=port-tag]").press("Enter");
    await expect(page.getByTestId("toast").last()).toContainText("not a valid tag name");
    // the port menu offers existing tags
    await handle(page, web, "db").click({ button: "right", force: true });
    await page.getByTestId("menu-link-tag").click();
    await expect(page.getByTestId("menu-tag-link-a")).toBeVisible();
    await expect(page.getByTestId("menu-tag-new")).toBeVisible();
  });

  test("sample with a Network appliance joins every NIC through the tag 'lan'; export/import keeps tags", async ({ page }) => {
    await open(page);
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-sample-network").click();
    await page.waitForSelector("[data-testid=appliance-node]");
    await page.waitForTimeout(400);
    await expect(stubs(page, "lan")).toHaveCount(7);
    await expect(page.getByTestId("nic-led").first()).toHaveAttribute("data-linked", "true");
    await expect(page.getByTestId("validation-panel")).toContainText("No issues");
    await openProps(page, "nginx-01");
    await expect(page.getByTestId("nic-status")).toContainText("(tag lan)");
    const dl = page.waitForEvent("download");
    await page.getByTestId("export").click();
    const text = readFileSync((await (await dl).path())!, "utf8");
    expect(JSON.parse(text).spec.links).toHaveLength(7);
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-empty").click();
    await page.getByTestId("import-input").setInputFiles({ name: "t.json", mimeType: "application/json", buffer: Buffer.from(text) });
    await page.waitForSelector("[data-testid=appliance-node]");
    await expect(stubs(page, "lan")).toHaveCount(7);
  });

  test("designer: a new appliance always includes a locked net interface", async ({ page }) => {
    await open(page);
    await page.getByTestId("new-appliance").click();
    await page.getByTestId("designer-name").fill("Probe");
    const ports = page.getByTestId("designer-port");
    await expect(ports).toHaveCount(3);
    const nic = ports.nth(1); // inputs are listed first: in, net
    await expect(nic.getByLabel("Port name")).toHaveValue("net");
    await expect(nic.getByLabel("Port name")).toBeDisabled();
    await expect(nic.getByLabel("Delete port")).toBeDisabled();
    await page.getByTestId("designer-save").click();
    await page.locator("[data-testid=catalog-tile][data-class-id=custom-probe]").click();
    const probe = (await assemblyState(page)).spec.appliances.find((a) => a.id.startsWith("custom-probe"))!;
    expect(probe.ports.some((p) => p.id === "net" && p.type === "network" && p.direction === "input")).toBe(true);
  });
});

test.describe("components", () => {
  const webTier = async (page: import("@playwright/test").Page) => {
    await node(page, "haproxy-01").click();
    await node(page, "nginx-01").click({ modifiers: ["Control"] });
    await node(page, "nginx-02").click({ modifiers: ["Control"] });
  };

  test("group three appliances into a component: dialog, re-plugged cables, badge, catalog tile; ungroup restores", async ({ page }) => {
    await open(page);
    await webTier(page);
    await node(page, "nginx-02").click({ button: "right" });
    await expect(page.getByTestId("menu-group")).toBeVisible();
    await page.getByTestId("menu-group").click();
    await expect(page.getByTestId("component-dialog")).toBeVisible();
    // linked terminals are pre-selected and locked
    const linked = page.locator("[data-testid=component-port][data-external=true]");
    await expect(linked).toHaveCount(3);
    for (const cb of await linked.locator("input[type=checkbox]").all()) { await expect(cb).toBeChecked(); await expect(cb).toBeDisabled(); }
    // validation: name is required
    await page.getByTestId("component-create").click();
    await expect(page.getByTestId("toast").last()).toContainText("name");
    await page.getByTestId("component-name").fill("Web tier");
    await page.getByTestId("component-create").click();
    await expect(page.getByTestId("component-dialog")).toHaveCount(0);

    const st = await assemblyState(page);
    expect(st.spec.appliances.map((a) => a.id).sort()).toEqual(["custom-web-tier-01", "gateway-01", "grafana-01", "postgresql-01"]);
    expect(st.spec.connections).toHaveLength(4);
    const comp = node(page, "custom-web-tier-01");
    await expect(comp.getByTestId("component-badge")).toContainText("3");
    await expect(comp).toHaveAttribute("data-component", "true");
    await openProps(page, "custom-web-tier-01");
    await expect(page.getByTestId("component-contents")).toContainText("haproxy-01");
    await expect(page.getByTestId("validation-panel")).toContainText("No issues");
    await expect(page.locator("[data-testid=catalog-tile][data-class-id=custom-web-tier]")).toBeVisible();
    await expectEdgesAnchored(page);
    // another instance from the catalog; the component is reusable
    await page.locator("[data-testid=catalog-tile][data-class-id=custom-web-tier]").click();
    expect((await assemblyState(page)).spec.appliances.some((a) => a.id === "custom-web-tier-02")).toBe(true);
    await page.getByTestId("undo").click();

    // ungroup via the menu
    await comp.click({ button: "right" });
    await page.getByTestId("menu-ungroup").click();
    const after = await assemblyState(page);
    expect(after.spec.appliances).toHaveLength(6);
    expect(after.spec.connections).toHaveLength(6);
    await expectEdgesAnchored(page);
    // undo brings the component back
    await page.getByTestId("undo").click();
    await expect(node(page, "custom-web-tier-01")).toBeVisible();
  });

  test("keyboard: Ctrl+G opens the dialog, Ctrl+Shift+G ungroups; unexposed cables are removed with a warning", async ({ page }) => {
    await open(page);
    await webTier(page);
    await page.keyboard.press("Control+g");
    await expect(page.getByTestId("component-dialog")).toBeVisible();
    await page.getByTestId("component-name").fill("Partial");
    // expose only the load balancer's input by unchecking nothing: linked ones are locked, so exposing all is the minimum
    await page.getByTestId("component-create").click();
    await expect(page.getByTestId("component-dialog")).toHaveCount(0);
    await expect(node(page, "custom-partial-01")).toBeVisible();
    await page.keyboard.press("Control+Shift+g");
    await expect(node(page, "custom-partial-01")).toHaveCount(0);
    expect((await assemblyState(page)).spec.appliances).toHaveLength(6);
  });

  test("a component joins the same network as its inner appliances", async ({ page }) => {
    await open(page);
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-sample-network").click();
    await page.waitForSelector("[data-testid=appliance-node]");
    await webTier(page);
    await page.keyboard.press("Control+g");
    await page.getByTestId("component-name").fill("Web tier");
    await page.getByTestId("component-create").click();
    await openProps(page, "custom-web-tier-01");
    await expect(page.getByTestId("nic-status")).toContainText("(tag lan)");
    await expect(page.getByTestId("validation-panel")).toContainText("No issues");
    await expect(page.locator("[data-testid=tag-stub][data-tag=lan]")).toHaveCount(5); // network, gateway, component, postgres, grafana
  });

  test("export embeds the component class and import restores it", async ({ page }) => {
    await open(page);
    await webTier(page);
    await page.keyboard.press("Control+g");
    await page.getByTestId("component-name").fill("Web tier");
    await page.getByTestId("component-create").click();
    const dl = page.waitForEvent("download");
    await page.getByTestId("export").click();
    const text = readFileSync((await (await dl).path())!, "utf8");
    const json = JSON.parse(text);
    expect(json.catalog.classes[0].composition.appliances).toHaveLength(3);
    await page.evaluate(() => localStorage.removeItem("apelora.studio.catalog.v1"));
    await page.reload();
    await page.waitForSelector("[data-testid=appliance-node]");
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-empty").click();
    await page.getByTestId("import-input").setInputFiles({ name: "c.json", mimeType: "application/json", buffer: Buffer.from(text) });
    await expect(node(page, "custom-web-tier-01").getByTestId("component-badge")).toContainText("3");
    await expect(page.locator("[data-testid=catalog-tile][data-class-id=custom-web-tier]")).toBeVisible();
    await node(page, "custom-web-tier-01").click({ button: "right" });
    await page.getByTestId("menu-ungroup").click();
    expect((await assemblyState(page)).spec.appliances).toHaveLength(6);
  });
});

test.describe("selection and pan tools", () => {
  test("select tool: drag a box to select several; Shift/Ctrl-click add; pan tool and Space pan the view", async ({ page }) => {
    await open(page);
    await expect(page.getByTestId("tool-select")).toHaveAttribute("aria-pressed", "true");
    const b1 = (await node(page, "nginx-01").boundingBox())!;
    const b2 = (await node(page, "nginx-02").boundingBox())!;
    const vp0 = await viewport(page);
    // rubber-band from empty canvas around both web servers
    await page.mouse.move(b1.x - 24, b1.y - 24);
    await page.mouse.down();
    await page.mouse.move(b2.x + b2.width + 24, b2.y + b2.height + 24, { steps: 8 });
    await page.mouse.up();
    await expect(page.getByTestId("selection-info")).toHaveText("2 selected");
    await expect(node(page, "nginx-01")).toHaveAttribute("data-selected", "true");
    await expect(node(page, "nginx-02")).toHaveAttribute("data-selected", "true");
    await expect(node(page, "haproxy-01")).toHaveAttribute("data-selected", "false");
    expect(await viewport(page)).toEqual(vp0); // selecting did not pan
    // Shift-click and Ctrl-click add to the selection
    await node(page, "haproxy-01").click({ modifiers: ["Shift"] });
    await expect(page.getByTestId("selection-info")).toHaveText("3 selected");
    await node(page, "grafana-01").click({ modifiers: ["Control"] });
    await expect(page.getByTestId("selection-info")).toHaveText("4 selected");
    await page.mouse.click(b1.x - 60, b1.y - 60); // empty canvas clears
    await expect(page.getByTestId("selection-info")).toHaveCount(0);

    // pan tool: dragging empty canvas moves the view
    await page.getByTestId("tool-pan").click();
    await expect(page.getByTestId("tool-pan")).toHaveAttribute("aria-pressed", "true");
    await page.mouse.move(b1.x - 60, b1.y - 60);
    await page.mouse.down();
    await page.mouse.move(b1.x + 20, b1.y + 10, { steps: 6 });
    await page.mouse.up();
    const panned = await viewport(page);
    expect(panned.x).toBeGreaterThan(vp0.x + 40);
    // keyboard: V selects, H pans, Space pans temporarily
    await page.keyboard.press("v");
    await expect(page.getByTestId("tool-select")).toHaveAttribute("aria-pressed", "true");
    await page.keyboard.press("h");
    await expect(page.getByTestId("tool-pan")).toHaveAttribute("aria-pressed", "true");
    await page.keyboard.press("v");
    await page.keyboard.down("Space");
    await page.mouse.move(b1.x - 60, b1.y - 60);
    await page.mouse.down();
    await page.mouse.move(b1.x - 20, b1.y - 40, { steps: 5 });
    await page.mouse.up();
    await page.keyboard.up("Space");
    expect((await viewport(page)).x).toBeGreaterThan(panned.x + 20);
    await expect(page.getByTestId("selection-info")).toHaveCount(0);
  });
});

test.describe("properties window", () => {
  test("opens on demand (double-click, Enter, context menu), not as a permanent sidebar; tabs; drag; Esc; follows deletion", async ({ page }) => {
    await open(page);
    await expect(page.getByTestId("inspector")).toHaveCount(0);
    await node(page, "nginx-01").click(); // selecting alone does not open it
    await expect(page.getByTestId("inspector")).toHaveCount(0);
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("props-title")).toHaveText("nginx-01");
    await page.getByTestId("props-close").click();
    await expect(page.getByTestId("inspector")).toHaveCount(0);

    await node(page, "postgresql-01").click({ button: "right" });
    await expect(page.getByTestId("menu-properties")).toBeVisible();
    await expect(page.getByTestId("context-menu").getByRole("menuitem").first()).toContainText("Properties");
    await page.getByTestId("menu-properties").click();
    await expect(page.getByTestId("props-title")).toHaveText("postgresql-01");
    await expect(page.getByTestId("inspector")).toContainText("PostgreSQL");

    // tabs: general, configuration (n), ports (n), issues
    await expect(page.getByTestId("props-tab-config")).toContainText("(4)");
    await page.getByTestId("props-tab-config").click();
    await expect(page.getByTestId("config-form")).toBeVisible();
    await page.getByTestId("props-tab-ports").click();
    await expect(page.getByTestId("port-row")).toHaveCount(3);
    await page.getByTestId("props-tab-issues").click();
    await expect(page.getByTestId("no-issues")).toBeVisible();
    await page.getByTestId("props-tab-general").click();
    await expect(page.getByTestId("instance-name")).toHaveValue("postgresql-01");

    // it stays within the viewport and is draggable; the new position is remembered
    const box = (await page.getByTestId("inspector").boundingBox())!;
    expect(box.y + box.height).toBeLessThanOrEqual(900 + 1);
    const hdr = (await page.getByTestId("props-header").boundingBox())!;
    await page.mouse.move(hdr.x + 60, hdr.y + 14);
    await page.mouse.down();
    await page.mouse.move(hdr.x + 60 - 300, hdr.y + 14 + 60, { steps: 6 });
    await page.mouse.up();
    const moved = (await page.getByTestId("inspector").boundingBox())!;
    expect(moved.x).toBeLessThan(box.x - 250);
    // the canvas stays usable while it is open: clicking another appliance selects it, the window stays on its target
    await node(page, "gateway-01").click();
    await expect(node(page, "gateway-01")).toHaveAttribute("data-selected", "true");
    await expect(page.getByTestId("props-title")).toHaveText("postgresql-01");
    // double-click retargets it
    await node(page, "grafana-01").dblclick();
    await expect(page.getByTestId("props-title")).toHaveText("grafana-01");
    const again = (await page.getByTestId("inspector").boundingBox())!;
    expect(Math.abs(again.x - moved.x)).toBeLessThan(3); // remembered position
    // Esc closes; the window also closes when its appliance is deleted
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("inspector")).toHaveCount(0);
    await openProps(page, "grafana-01");
    await page.getByTestId("delete-appliance").click();
    await expect(page.getByTestId("inspector")).toHaveCount(0);
  });

  test("cable, tag and assembly properties windows", async ({ page }) => {
    await open(page);
    const pt = await cablePoint(page, "conn-01", 0.5);
    await page.mouse.dblclick(pt.x, pt.y);
    await expect(page.getByTestId("inspector-connection")).toContainText("gateway-01");
    await page.getByTestId("props-close").click();
    const canvas = (await page.getByTestId("canvas").boundingBox())!;
    await page.mouse.click(canvas.x + 60, canvas.y + 60, { button: "right" });
    await page.getByTestId("menu-assembly-props").click();
    await expect(page.getByTestId("inspector-assembly")).toContainText("Appliances");
    await expect(page.getByTestId("inspector-assembly")).toContainText("6");
  });
});

test.describe("zones and shapes", () => {
  const shapeBox = async (page: import("@playwright/test").Page, id: string) => (await page.locator(`[data-testid=shape-node][data-shape-id="${id}"]`).boundingBox())!;
  const shapes = async (page: import("@playwright/test").Page) => (await assemblyState(page)).editor.shapes ?? [];
  const wrapWebTier = async (page: import("@playwright/test").Page, kind = "rect") => {
    await node(page, "haproxy-01").click();
    await node(page, "nginx-01").click({ modifiers: ["Control"] });
    await node(page, "nginx-02").click({ modifiers: ["Control"] });
    await node(page, "nginx-02").click({ button: "right" });
    await page.getByTestId("menu-wrap").click();
    await page.getByTestId(`menu-wrap-${kind}`).click();
  };

  test("add the four shapes from the Zones menu; each is selectable and editable", async ({ page }) => {
    await open(page);
    for (const kind of ["rect", "ellipse", "rounded", "triangle"]) {
      await page.getByTestId("shapes-button").click();
      await page.getByTestId(`add-shape-${kind}`).click();
      await expect(page.locator(`[data-testid=shape-node][data-kind=${kind}]`)).toHaveCount(1);
    }
    const sh = await shapes(page);
    expect(sh.map((x) => x.kind)).toEqual(["rect", "ellipse", "rounded", "triangle"]);
    expect(new Set(sh.map((x) => x.fill)).size).toBe(4); // modern palette cycles through hues
    expect(sh.find((x) => x.kind === "rounded")!.radius).toBeGreaterThan(0);
    // the label is a handle: double-clicking it opens the shape editor
    await page.locator("[data-testid=shape-node][data-kind=triangle] [data-testid=shape-label]").dblclick();
    await expect(page.getByTestId("inspector")).toHaveAttribute("data-kind", "shape");
    await expect(page.getByTestId("shape-kind").locator("[data-kind=triangle]")).toHaveAttribute("aria-checked", "true");
  });

  test("wrap a selection in a zone: sits behind appliances, wraps them, opens the label editor; cables stay clickable", async ({ page }) => {
    await open(page);
    await wrapWebTier(page, "rounded");
    await expect(page.getByTestId("shape-label-input")).toBeVisible();
    await page.getByTestId("shape-label-input").fill("Web tier");
    await expect(page.getByTestId("shape-label")).toHaveText("Web tier");
    const z = await shapeBox(page, "shape-01");
    for (const id of ["haproxy-01", "nginx-01", "nginx-02"]) {
      const b = (await node(page, id).boundingBox())!;
      expect(b.x).toBeGreaterThan(z.x); expect(b.y).toBeGreaterThan(z.y);
      expect(b.x + b.width).toBeLessThan(z.x + z.width); expect(b.y + b.height).toBeLessThan(z.y + z.height);
    }
    // behind: the topmost element over an appliance is the appliance, not the zone
    const top = await page.evaluate(() => { const n = document.querySelector("[data-testid=appliance-node][data-node-id=haproxy-01]")!.getBoundingClientRect(); return document.elementFromPoint(n.x + n.width / 2, n.y + n.height - 8)?.closest("[data-testid=appliance-node]")?.getAttribute("data-node-id") ?? "zone"; });
    expect(top).toBe("haproxy-01");
    // a cable inside the zone is still reachable (the interior does not capture clicks)
    const pt = await cablePoint(page, "conn-02", 0.5);
    await page.mouse.click(pt.x, pt.y, { button: "right" });
    await expect(page.getByTestId("menu-to-tag")).toBeVisible();
    await page.keyboard.press("Escape");
    await expectEdgesAnchored(page);
  });

  test("style editing: fill and border colours, opacity, width, style, corner radius, shape kind; one undo per control", async ({ page }) => {
    await open(page);
    await wrapWebTier(page, "rounded");
    await page.getByTestId("props-tab-style").click();
    await page.locator("[data-testid=shape-fill] [data-color='#14b8a6']").click();
    let sh = (await shapes(page))[0]!;
    expect(sh.fill).toBe("#14b8a6");
    await page.locator("[data-testid=shape-stroke] [data-color='#f43f5e']").click();
    await page.getByTestId("shape-stroke-width").fill("4");
    await page.locator("[data-testid=shape-stroke-style] [data-value=dotted]").click();
    await page.getByTestId("shape-fill-opacity").fill("40");
    await page.getByTestId("shape-radius").fill("40");
    sh = (await shapes(page))[0]!;
    expect(sh).toMatchObject({ stroke: "#f43f5e", strokeWidth: 4, strokeStyle: "dotted", fillOpacity: 0.4, radius: 40 });
    // the SVG reflects it
    const svg = page.locator("[data-testid=shape-node] rect").first();
    await expect(svg).toHaveAttribute("fill", "#14b8a6");
    await expect(svg).toHaveAttribute("stroke-width", "4");
    await expect(svg).toHaveAttribute("rx", "40");
    // slider drags merge into a single undo step
    const before = (await shapes(page))[0]!.fillOpacity;
    await page.getByTestId("shape-fill-opacity").fill("10");
    await page.getByTestId("shape-fill-opacity").fill("20");
    await page.getByTestId("shape-fill-opacity").fill("30");
    await page.getByTestId("undo").click();
    expect((await shapes(page))[0]!.fillOpacity).toBe(before);
    // switching shape kind keeps the style; the radius control only exists for rounded squares
    await page.locator("[data-testid=shape-kind] [data-kind=triangle]").click();
    await expect(page.locator("[data-testid=shape-node] polygon").first()).toBeVisible();
    await expect(page.getByTestId("shape-radius")).toHaveCount(0);
    await page.locator("[data-testid=shape-kind] [data-kind=ellipse]").click();
    await expect(page.locator("[data-testid=shape-node] ellipse").first()).toBeVisible();
    // custom colour input
    await page.locator("[data-testid=shape-fill] input[type=color]").fill("#112233");
    expect((await shapes(page))[0]!.fill).toBe("#112233");
    // label tab
    await page.getByTestId("props-tab-label").click();
    await page.locator("[data-testid=shape-label-pos] [data-value=bottom]").click();
    await page.getByTestId("shape-font-size").fill("20");
    expect((await shapes(page))[0]).toMatchObject({ labelPos: "bottom", fontSize: 20 });
    // layout tab
    await page.getByTestId("props-tab-layout").click();
    await expect(page.getByTestId("shape-contents")).toContainText("appliance");
  });

  test("dragging a zone carries the appliances inside; a locked zone stays put", async ({ page }) => {
    await open(page);
    await wrapWebTier(page, "rect");
    const before = await assemblyState(page);
    const pos = (id: string) => before.editor.nodes.find((n) => n.id === id)!;
    const lab = (await page.getByTestId("shape-label").boundingBox())!;
    await page.mouse.move(lab.x + 20, lab.y + lab.height / 2);
    await page.mouse.down();
    await page.mouse.move(lab.x + 20 + 90, lab.y + lab.height / 2 + 40, { steps: 8 });
    await page.mouse.up();
    const after = await assemblyState(page);
    const z0 = before.editor.shapes![0]!;
    const z1 = after.editor.shapes![0]!;
    const dx = z1.x - z0.x;
    const dy = z1.y - z0.y;
    expect(dx).toBeGreaterThan(40);
    for (const id of ["haproxy-01", "nginx-01", "nginx-02"]) {
      const n = after.editor.nodes.find((x) => x.id === id)!;
      expect(n.x - pos(id).x).toBe(dx);
      expect(n.y - pos(id).y).toBe(dy);
    }
    for (const id of ["gateway-01", "postgresql-01", "grafana-01"]) expect(after.editor.nodes.find((x) => x.id === id)).toMatchObject({ x: pos(id).x, y: pos(id).y });
    await expectEdgesAnchored(page);
    await page.getByTestId("undo").click(); // the whole drag is one undo step
    const undone = await assemblyState(page);
    expect(undone.editor.shapes![0]!.x).toBe(z0.x);
    expect(undone.editor.nodes.find((n) => n.id === "nginx-01")!.x).toBe(pos("nginx-01").x);
    // lock it: dragging no longer moves it
    await page.getByTestId("props-tab-layout").click();
    await page.getByTestId("shape-locked").check();
    const lab2 = (await page.getByTestId("shape-label").boundingBox())!;
    await page.mouse.move(lab2.x + 30, lab2.y + lab2.height / 2);
    await page.mouse.down();
    await page.mouse.move(lab2.x + 130, lab2.y + lab2.height / 2 + 50, { steps: 6 });
    await page.mouse.up();
    expect((await assemblyState(page)).editor.shapes![0]!.x).toBe(z0.x);
  });

  test("shape context menu: edit, label, shape kind, colour, border style, select contents, order, duplicate, lock, delete", async ({ page }) => {
    test.setTimeout(60_000);
    await open(page);
    await wrapWebTier(page, "rect");
    await page.getByTestId("props-close").click();
    const lab = page.locator("[data-testid=shape-label]");
    await lab.click({ button: "right" });
    await expect(page.getByTestId("context-menu")).toContainText("Zone");
    for (const id of ["properties", "rename", "shape-kind", "fill", "border-colour", "border-style", "select-contents", "front", "back", "duplicate", "lock", "delete"]) await expect(page.getByTestId(`menu-${id}`)).toBeVisible();
    await expect(page.getByTestId("menu-select-contents")).toContainText("(3)");
    await page.getByTestId("menu-fill").click();
    await page.getByTestId("menu-fill-violet").click();
    expect((await shapes(page))[0]).toMatchObject({ fill: "#8b5cf6", stroke: "#8b5cf6" });
    await lab.click({ button: "right" });
    await page.getByTestId("menu-border-style").click();
    await page.getByTestId("menu-style-dashed").click();
    expect((await shapes(page))[0]!.strokeStyle).toBe("dashed");
    await lab.click({ button: "right" });
    await page.getByTestId("menu-select-contents").click();
    await expect(page.getByTestId("selection-info")).toContainText("selected");
    await lab.click({ button: "right" });
    await page.getByTestId("menu-duplicate").click();
    expect(await shapes(page)).toHaveLength(2);
    await page.locator("[data-testid=shape-label]").last().click({ button: "right" });
    await page.getByTestId("menu-back").click();
    await page.locator("[data-testid=shape-label]").last().click({ button: "right" });
    await page.getByTestId("menu-lock").click();
    await expect(page.locator("[data-testid=shape-node][data-locked=true]")).toHaveCount(1);
    await page.locator("[data-testid=shape-label]").first().click({ button: "right" });
    await page.getByTestId("menu-rename").click();
    await expect(page.getByTestId("shape-label-input")).toBeFocused();
    await page.keyboard.press("Escape");
    await page.locator("[data-testid=shape-label]").first().click({ button: "right" });
    await page.getByTestId("menu-delete").click();
    expect(await shapes(page)).toHaveLength(1);
    // change the remaining zone to a triangle (its label then sits among the appliances, so do this last)
    await page.locator("[data-testid=shape-label]").first().click({ button: "right" });
    await page.getByTestId("menu-shape-kind").click();
    await page.getByTestId("menu-kind-triangle").click();
    expect((await shapes(page))[0]!.kind).toBe("triangle");
  });

  test("canvas menu adds a zone at the pointer; Delete removes a selected zone; undo restores; export, import and density keep zones", async ({ page }) => {
    await open(page);
    const canvas = (await page.getByTestId("canvas").boundingBox())!;
    const at = { x: canvas.x + 110, y: canvas.y + 90 };
    await page.mouse.click(at.x, at.y, { button: "right" });
    await page.getByTestId("menu-add-shape").click();
    await page.getByTestId("menu-shape-ellipse").click();
    const b = await shapeBox(page, "shape-01");
    expect(Math.abs(b.x + b.width / 2 - at.x)).toBeLessThan(3);
    expect(Math.abs(b.y + b.height / 2 - at.y)).toBeLessThan(3);
    await page.getByTestId("shapes-button").click();
    await page.getByTestId("add-shape-triangle").click();
    expect(await shapes(page)).toHaveLength(2);
    await page.keyboard.press("Delete"); // the selected (newest) zone
    expect(await shapes(page)).toHaveLength(1);
    await page.getByTestId("undo").click();
    expect(await shapes(page)).toHaveLength(2);
    // export / import round trip
    const dl = page.waitForEvent("download");
    await page.getByTestId("export").click();
    const text = readFileSync((await (await dl).path())!, "utf8");
    expect(JSON.parse(text).editor.shapes).toHaveLength(2);
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-empty").click();
    expect(await shapes(page)).toHaveLength(0);
    await page.getByTestId("import-input").setInputFiles({ name: "z.json", mimeType: "application/json", buffer: Buffer.from(text) });
    await expect(page.locator("[data-testid=shape-node]")).toHaveCount(2);
    // density scales zones with the appliances
    const w0 = (await shapes(page))[0]!.width;
    await page.getByTestId("density-expanded").click();
    expect((await shapes(page))[0]!.width).toBeGreaterThan(w0);
    // hostile colours are rejected on import
    const bad = JSON.parse(text);
    bad.editor.shapes[0].fill = "red;x";
    await page.getByTestId("import-input").setInputFiles({ name: "bad.json", mimeType: "application/json", buffer: Buffer.from(JSON.stringify(bad)) });
    await expect(page.getByTestId("import-errors")).toContainText("fill");
  });
});

test.describe("responsive and themes", () => {
  test("narrow window: header wraps instead of overflowing; Export and theme stay reachable", async ({ page }) => {
    await page.setViewportSize({ width: 900, height: 700 });
    await open(page);
    expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
    for (const id of ["export", "import", "new-menu", "undo"]) {
      const b = (await page.getByTestId(id).boundingBox())!;
      expect(b.x + b.width, id).toBeLessThanOrEqual(900);
    }
    const theme = (await page.locator("select[aria-label=Theme]").boundingBox())!;
    expect(theme.x + theme.width).toBeLessThanOrEqual(900);
    await expect(page.getByTestId("canvas-frame")).toBeVisible();
  });

  test("dark theme: logos get a light chip so dark brand marks stay visible", async ({ page }) => {
    await open(page);
    await page.locator("select[aria-label=Theme]").selectOption("night");
    const bg = await page.locator("[data-testid=catalog-tile] .apl-logo").first().evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(bg).not.toBe("rgba(0, 0, 0, 0)");
    const nodeLogo = await node(page, "nginx-01").getByTestId("node-logo").evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(nodeLogo).not.toBe("rgba(0, 0, 0, 0)");
    await page.locator("select[aria-label=Theme]").selectOption("light");
    const light = await node(page, "nginx-01").getByTestId("node-logo").evaluate((el) => getComputedStyle(el).backgroundColor);
    expect(light).toBe("rgba(0, 0, 0, 0)");
  });
});

test.describe("transitions", () => {
  test("menus, dialogs, plugs and cables animate; reduced motion disables them", async ({ page }) => {
    await open(page);
    await node(page, "nginx-01").click({ button: "right" });
    const anim = (sel: string) => page.locator(sel).first().evaluate((el) => { const c = getComputedStyle(el); return { name: c.animationName, dur: c.animationDuration }; });
    expect((await anim("[data-testid=context-menu]")).name).toContain("apl-pop");
    expect((await anim("[data-testid=plug-head]")).name).toContain("apl-plug-in");
    expect((await anim(".react-flow__edge .apl-cable")).name).toContain("apl-draw");
    await page.keyboard.press("Escape");
    await page.emulateMedia({ reducedMotion: "reduce" });
    await node(page, "nginx-01").click({ button: "right" });
    const d = await anim("[data-testid=context-menu]");
    expect(parseFloat(d.dur)).toBeLessThan(0.001); // 0.01ms → seconds
    // zoom-to-fit is instant under reduced motion: no intermediate viewport frames
    await page.keyboard.press("Escape");
    await page.getByRole("button", { name: "Zoom in" }).click();
    const z0 = (await viewport(page)).zoom;
    await page.getByRole("button", { name: "Zoom in" }).click();
    expect((await viewport(page)).zoom).toBeGreaterThan(z0);
  });
});

test.describe("terminals and labels", () => {
  test("every port shows a short text label; labels never cover the logo; jacks are RJ45-style and toggle off", async ({ page }) => {
    await open(page);
    const n = node(page, "nginx-01");
    await expect(n.getByTestId("port-label")).toHaveText(["in", "net", "db", "up"]);
    await expect(node(page, "haproxy-01").getByTestId("port-label")).toHaveText(["in", "net", "out", "mon"]);
    const logo = (await n.getByTestId("node-logo").boundingBox())!;
    for (const l of await page.getByTestId("port-label").all()) {
      const b = (await l.boundingBox())!;
      const owner = await l.evaluate((el) => el.closest("[data-testid=appliance-node]")!.getAttribute("data-node-id"));
      if (owner !== "nginx-01") continue;
      expect(b.x + b.width <= logo.x + 0.5 || b.x >= logo.x + logo.width - 0.5, "label must not overlap the logo").toBe(true);
    }
    await expect(n.locator(".apl-jack[data-kind=input]")).toHaveCount(2); // in + net
    await expect(n.locator(".apl-jack[data-kind=output]")).toHaveCount(2); // db + up
    // link labels carry the protocol
    await expect(page.getByTestId("link-label").first()).toBeVisible();
    await page.getByTestId("toggle-labels").click();
    await expect(page.getByTestId("port-label").first()).toBeHidden();
    await expect(page.getByTestId("link-label")).toHaveCount(0);
    await page.getByTestId("toggle-labels").click();
    await expect(page.getByTestId("port-label").first()).toBeVisible();
  });
});

test.describe("cables and plugs", () => {
  test("connected jacks show a plug head in the cable's protocol colour; free jacks stay empty", async ({ page }) => {
    await open(page);
    // one plug head per plugged port (6 cables share 9 ports; postgres "sql" takes three cables)
    const heads = await page.getByTestId("plug-head").count();
    expect(heads).toBe(9);
    await expect(handle(page, "gateway-01", "wan").getByTestId("plug-head")).toHaveCount(0);
    await expect(handle(page, "gateway-01", "lan").getByTestId("plug-head")).toHaveAttribute("data-type", "http");
    await expect(handle(page, "postgresql-01", "postgres").getByTestId("plug-head")).toHaveAttribute("data-type", "postgres");
    // deleting a cable unplugs both ends
    await page.evaluate(() => (window as unknown as { __studio: { getState: () => { setSelection: (s: unknown) => void } } }).__studio.getState().setSelection({ nodes: [], edges: ["conn-01"] }));
    await page.keyboard.press("Delete");
    await expect(page.getByTestId("plug-head")).toHaveCount(7);
    await expect(handle(page, "gateway-01", "lan").getByTestId("plug-head")).toHaveCount(0);
  });

  test("a multi-link port takes several cables and shows ×N; a single-link port refuses a second cable", async ({ page }) => {
    await open(page);
    // postgresql "sql" input is multi-link with 3 cables already
    await expect(handle(page, "postgresql-01", "postgres").getByTestId("plug-count")).toHaveText("×3");
    // nginx db output is single-link and already used → a second cable from it is refused with a reason
    const before = await edgeCount(page);
    await dragHandle(page, handle(page, "nginx-01", "db"), handle(page, "postgresql-01", "postgres"));
    expect(await edgeCount(page)).toBe(before);
    await expect(page.getByTestId("toast").last()).toContainText(/already connected|only one/);
    // make it multi-link in the inspector, then connect a second cable from another port to the same jack
    await openProps(page, "nginx-01", "ports");
    await page.locator("[data-testid=port-row][data-port-id=db] [data-testid=port-multi]").check();
    await expect(handle(page, "nginx-01", "db")).toHaveAttribute("aria-label", /multiple cables/);
  });

  test("dragging a plug end re-plugs the cable into another compatible port", async ({ page }) => {
    await open(page);
    const before = (await assemblyState(page)).spec.connections.find((c) => c.id === "conn-03")!; // haproxy -> nginx-02
    expect(before.target.applianceId).toBe("nginx-02");
    // add a third web server to re-plug into
    await page.locator("[data-testid=catalog-tile][data-class-id=nginx]").dragTo(page.getByTestId("canvas"), { targetPosition: { x: 300, y: 560 } });
    const newId = (await assemblyState(page)).spec.appliances.map((a) => a.id).find((id) => !["gateway-01", "haproxy-01", "nginx-01", "nginx-02", "postgresql-01", "grafana-01"].includes(id))!;
    // grab the cable end near nginx-02's "in" jack and drop it on the new node's "in" jack
    const jack = await center(handle(page, "nginx-02", "http"));
    const dest = await center(handle(page, newId, "http"));
    await page.mouse.move(jack.x - 14 * (await viewport(page)).zoom, jack.y);
    await page.mouse.down();
    await page.mouse.move((jack.x + dest.x) / 2, (jack.y + dest.y) / 2, { steps: 8 });
    await page.mouse.move(dest.x, dest.y, { steps: 8 });
    await page.mouse.up();
    await page.waitForTimeout(150);
    const after = (await assemblyState(page)).spec.connections.find((c) => c.id === "conn-03")!;
    expect(after.target.applianceId).toBe(newId);
    await expectEdgesAnchored(page);
    await page.getByTestId("undo").click();
    expect((await assemblyState(page)).spec.connections.find((c) => c.id === "conn-03")!.target.applianceId).toBe("nginx-02");
  });
});

test.describe("custom appliances", () => {
  test("design a custom appliance with typed ports, place it, connect it, export/import it", async ({ page }) => {
    await open(page);
    await page.getByTestId("new-appliance").click();
    await expect(page.getByTestId("designer")).toBeVisible();
    await page.getByTestId("designer-name").fill("Session cache");
    await page.getByTestId("designer-add-input").click();
    await page.getByTestId("designer-add-output").click();
    await expect(page.getByTestId("designer-port")).toHaveCount(5);
    // make the first input a required http port named "in"
    const first = page.getByTestId("designer-port").first();
    await first.getByLabel("Port type").selectOption("http");
    await expect(page.getByTestId("designer-preview").locator("span").first()).toBeVisible();
    await page.getByTestId("designer-save").click();
    await expect(page.getByTestId("toast").last()).toContainText("Session cache");

    const tile = page.locator("[data-testid=catalog-tile][data-class-id=custom-session-cache]");
    await expect(tile).toBeVisible();
    await tile.click();
    const st = await assemblyState(page);
    const added = st.spec.appliances.find((a) => a.id.startsWith("custom-session-cache"))!;
    expect(added.ports.length).toBe(5);
    const h = await flowSize(page, added.id);
    expect(h.height).toBeCloseTo(60 + 18 * 3, 0); // in, added input and net

    // wire load balancer → custom input (http) is valid
    await dragHandle(page, handle(page, "haproxy-01", "backend"), handle(page, added.id, "in"));
    expect((await assemblyState(page)).spec.connections.length).toBe(7);

    // export embeds the class; wiping the catalog and re-importing restores it
    const dl = page.waitForEvent("download");
    await page.getByTestId("export").click();
    const text = readFileSync((await (await dl).path())!, "utf8");
    expect(JSON.parse(text).catalog.classes[0].id).toBe("custom-session-cache");
    await page.evaluate(() => localStorage.removeItem("apelora.studio.catalog.v1"));
    await page.reload();
    await page.waitForSelector("[data-testid=appliance-node]");
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-empty").click();
    await page.getByTestId("import-input").setInputFiles({ name: "c.json", mimeType: "application/json", buffer: Buffer.from(text) });
    await page.waitForSelector("[data-testid=appliance-node]");
    await expect(page.locator("[data-testid=catalog-tile][data-class-id=custom-session-cache]")).toBeVisible();
    await expect(page.getByTestId("error-count")).toHaveCount(0);
  });

  test("save an instance with extra ports as a reusable appliance; designer validates names", async ({ page }) => {
    await open(page);
    await openProps(page, "grafana-01", "ports");
    await addPorts(page, "output", 2);
    await page.getByTestId("props-tab-general").click();
    await page.getByTestId("save-as-class").click();
    await page.getByTestId("designer-name").fill("");
    await page.getByTestId("designer-save").click();
    await expect(page.getByTestId("designer-errors")).toContainText("name");
    await page.getByTestId("designer-name").fill("Dash pack");
    await page.getByTestId("designer-save").click();
    await expect(page.locator("[data-testid=catalog-tile][data-class-id=custom-dash-pack]")).toBeVisible();
    // editing and deleting a custom class
    const tile = page.locator("[data-testid=catalog-tile][data-class-id=custom-dash-pack]");
    await tile.hover();
    await page.getByRole("button", { name: "Edit Dash pack" }).click();
    await page.getByTestId("designer-delete").click();
    await expect(tile).toHaveCount(0);
  });
});

test.describe("catalog", () => {
  test("sections collapse, search filters, click adds exactly one instance", async ({ page }) => {
    await open(page);
    const section = page.locator("[data-testid=catalog-section][data-category='Databases & Cache']");
    const dbCount = APPLIANCE_CLASSES.filter((c) => c.category === "Databases & Cache").length;
    expect(dbCount).toBeGreaterThanOrEqual(12);
    await expect(section.getByTestId("catalog-tile")).toHaveCount(dbCount);
    await section.getByRole("button").first().click();
    await expect(section.getByTestId("catalog-tile")).toHaveCount(0);
    await section.getByRole("button").first().click();
    await expect(section.getByTestId("catalog-tile")).toHaveCount(dbCount);

    await page.getByTestId("catalog-search").fill("redi");
    await expect(page.getByTestId("catalog-tile")).toHaveCount(1);
    await expect(page.getByTestId("catalog-section")).toHaveCount(1);
    const before = (await assemblyState(page)).spec.appliances.length;
    await page.getByTestId("catalog-tile").click();
    expect((await assemblyState(page)).spec.appliances.length).toBe(before + 1);
    await page.getByTestId("catalog-search").fill("zzzz");
    await expect(page.getByText("No appliances match")).toBeVisible();
  });

  test("Custom section is always visible (with an empty state) and fills when you create one; identity and Microsoft Windows sections exist", async ({ page }) => {
    await open(page);
    const custom = page.locator("[data-testid=catalog-section][data-category=Custom]");
    await expect(custom).toBeVisible();
    await expect(page.getByTestId("catalog-section").first()).toHaveAttribute("data-category", "Custom"); // first in the list
    await expect(page.getByTestId("custom-empty")).toBeVisible();
    await expect(custom.getByTestId("catalog-tile")).toHaveCount(0);
    // searching hides the empty Custom section
    await page.getByTestId("catalog-search").fill("keycloak");
    await expect(custom).toHaveCount(0);
    await page.getByTestId("catalog-search").fill("");
    // create one from the empty state
    await page.getByTestId("custom-empty-new").click();
    await page.getByTestId("designer-name").fill("My service");
    await page.getByTestId("designer-save").click();
    await expect(custom.getByTestId("catalog-tile")).toHaveCount(1);
    await expect(page.getByTestId("custom-empty")).toHaveCount(0);
    // identity providers
    const ident = page.locator("[data-testid=catalog-section][data-category='Security & Identity']");
    for (const id of ["keycloak", "authelia", "authentik", "pocketid"]) await expect(ident.locator(`[data-class-id=${id}]`)).toBeVisible();
    // Microsoft Windows
    const win = page.locator("[data-testid=catalog-section][data-category='Microsoft Windows']");
    await expect(win.getByTestId("catalog-tile")).toHaveCount(APPLIANCE_CLASSES.filter((c) => c.category === "Microsoft Windows").length);
    for (const id of ["windows-server", "windows-11", "active-directory", "iis", "sql-server", "dotnet"]) await expect(win.locator(`[data-class-id=${id}]`)).toBeVisible();
    // a Windows appliance has the usual net terminal, ports and configuration
    await win.locator("[data-class-id=sql-server]").click();
    const id = (await assemblyState(page)).spec.appliances.find((a) => a.id.startsWith("sql-server"))!.id;
    await openProps(page, id, "config");
    await expect(page.getByTestId("config-form")).toBeVisible();
    await expect(page.getByTestId("cfg-edition")).toBeVisible();
  });

  test("tiles show every built-in appliance, grouped, with collapse all / expand all", async ({ page }) => {
    await open(page);
    await expect(page.getByTestId("catalog-tile")).toHaveCount(APPLIANCE_CLASSES.length);
    await expect(page.getByTestId("catalog-count")).toHaveText(`${APPLIANCE_CLASSES.length} appliances`);
    await page.getByTestId("catalog-collapse-all").click();
    await expect(page.getByTestId("catalog-tile")).toHaveCount(0);
    await page.getByTestId("catalog-expand-all").click();
    await expect(page.getByTestId("catalog-tile")).toHaveCount(APPLIANCE_CLASSES.length);
    expect(await page.getByTestId("catalog-section").count()).toBeGreaterThanOrEqual(6);
  });
});

test.describe("drag and drop placement", () => {
  for (const [label, zoomClicks] of [["50%-ish", -3], ["100%-ish", 0], ["150%-ish", 2]] as const) {
    test(`dropped appliance lands under the cursor at ${label} zoom, also after panning`, async ({ page }) => {
      await open(page);
      const buttons = zoomClicks < 0 ? "Zoom out" : "Zoom in";
      for (let i = 0; i < Math.abs(zoomClicks); i++) { await page.getByRole("button", { name: buttons }).click(); await page.waitForTimeout(220); }
      const tile = page.locator("[data-testid=catalog-tile][data-class-id=redis]");
      const canvas = (await page.getByTestId("canvas").boundingBox())!;
      const target = { x: canvas.x + canvas.width * 0.3, y: canvas.y + canvas.height * 0.82 };
      const drop = async () => {
        const before = (await assemblyState(page)).spec.appliances.map((a) => a.id);
        await tile.dragTo(page.getByTestId("canvas"), { targetPosition: { x: target.x - canvas.x, y: target.y - canvas.y } });
        const after = (await assemblyState(page)).spec.appliances.map((a) => a.id);
        const id = after.find((x) => !before.includes(x))!;
        const c = await center(node(page, id));
        expect(Math.abs(c.x - target.x), "x").toBeLessThan(3);
        expect(Math.abs(c.y - target.y), "y").toBeLessThan(3);
      };
      await drop();
      // pan the canvas by dragging the empty pane, then drop again
      await page.mouse.move(canvas.x + canvas.width * 0.8, canvas.y + 40);
      await page.mouse.down();
      await page.mouse.move(canvas.x + canvas.width * 0.8 - 90, canvas.y + 70, { steps: 5 });
      await page.mouse.up();
      await drop();
    });
  }
});

test.describe("editing", () => {
  test("moving, resizing and density changes keep edges anchored", async ({ page }) => {
    await open(page);
    const before = await center(node(page, "nginx-01"));
    await page.mouse.move(before.x, before.y - 20);
    await page.mouse.down();
    await page.mouse.move(before.x + 60, before.y + 10, { steps: 6 });
    await page.mouse.up();
    await page.waitForTimeout(100);
    expect((await center(node(page, "nginx-01"))).x).toBeGreaterThan(before.x + 40);
    await expectEdgesAnchored(page);

    await node(page, "nginx-01").click();
    const n = await node(page, "nginx-01").boundingBox();
    const grip = page.locator(".react-flow__node[data-id=nginx-01] .react-flow__resize-control");
    const g = (await grip.boundingBox())!;
    await page.mouse.move(g.x + g.width / 2, g.y + g.height / 2);
    await page.mouse.down();
    await page.mouse.move(g.x + 60, g.y + 50, { steps: 6 });
    await page.mouse.up();
    await page.waitForTimeout(100);
    const after = await node(page, "nginx-01").boundingBox();
    expect(after!.width).toBeGreaterThan(n!.width + 30);
    expect(after!.height).toBeGreaterThan(n!.height + 20);
    await expectEdgesAnchored(page);

    for (const d of ["expanded", "comfortable", "compact"]) {
      await page.getByTestId(`density-${d}`).click();
      await page.waitForTimeout(150);
      await expectEdgesAnchored(page);
    }
    // manual size survives density switches (documented behaviour); other nodes follow the preset
    const st = await assemblyState(page);
    expect(st.editor.nodes.find((x) => x.id === "nginx-01")!.custom).toBe(true);
    const sz = await flowSize(page, "haproxy-01");
    expect(sz.width).toBeCloseTo(128, 0);
    expect(sz.height).toBeCloseTo(96, 0);
  });

  test("adding 3–8 ports grows only the affected node; edges stay attached", async ({ page }) => {
    await open(page);
    const otherBefore = await flowSize(page, "postgresql-01");
    await openProps(page, "grafana-01", "ports");
    await addPorts(page, "input", 4);
    await page.waitForTimeout(150);
    let s = await flowSize(page, "grafana-01");
    expect(s.width).toBeCloseTo(128, 0);
    expect(s.height).toBeGreaterThan(96);
    expect(s.height).toBeCloseTo(60 + 18 * 6, 0); // ui + net + 4 added = 6 inputs
    await addPorts(page, "input", 3);
    await page.waitForTimeout(150);
    s = await flowSize(page, "grafana-01");
    expect(s.height).toBeCloseTo(60 + 18 * 9, 0);
    expect(await flowSize(page, "postgresql-01")).toEqual(otherBefore);
    await expectEdgesAnchored(page);
    // logo is still centered horizontally and no port overlaps it
    const logo = (await node(page, "grafana-01").getByTestId("node-logo").boundingBox())!;
    for (const h of await node(page, "grafana-01").locator(".react-flow__handle").all()) {
      const hb = (await h.boundingBox())!;
      expect(hb.x + hb.width <= logo.x || hb.x >= logo.x + logo.width).toBe(true);
    }
    // removing ports shrinks the minimum but never deletes unrelated connections
    const edges = await edgeCount(page);
    await page.locator("[data-testid=port-row][data-port-id=in-7] [data-testid=delete-port]").click();
    expect(await edgeCount(page)).toBe(edges);
  });

  test("deleting a connected port removes only its connections and can be undone", async ({ page }) => {
    await open(page);
    await openProps(page, "haproxy-01", "ports");
    const edges = await edgeCount(page);
    await page.locator("[data-testid=port-row][data-port-id=backend] [data-testid=delete-port]").click();
    expect(await edgeCount(page)).toBe(edges - 2);
    await expect(page.getByTestId("toast").first()).toContainText("2 connections");
    await page.getByTestId("undo").click();
    expect(await edgeCount(page)).toBe(edges);
    await expectEdgesAnchored(page);
  });

  test("connections: valid accepted; wrong direction, type mismatch and duplicates rejected with a reason", async ({ page }) => {
    await open(page);
    // add Apache (mysql out) + MariaDB (mysql in) and connect them
    await page.locator("[data-testid=catalog-tile][data-class-id=apache]").dragTo(page.getByTestId("canvas"), { targetPosition: { x: 120, y: 560 } });
    await page.locator("[data-testid=catalog-tile][data-class-id=mariadb]").dragTo(page.getByTestId("canvas"), { targetPosition: { x: 480, y: 560 } });
    const st = await assemblyState(page);
    const apache = st.spec.appliances.find((a) => a.id.startsWith("apache"))!.id;
    const maria = st.spec.appliances.find((a) => a.id.startsWith("mariadb"))!.id;
    const n0 = await edgeCount(page);

    // type mismatch: apache mysql output → haproxy http input
    await dragHandle(page, handle(page, apache, "db"), handle(page, "haproxy-01", "frontend"));
    expect(await edgeCount(page)).toBe(n0);
    await expect(page.getByTestId("toast").last()).toContainText(/cannot feed|type/i);

    // valid
    await dragHandle(page, handle(page, apache, "db"), handle(page, maria, "mysql"));
    expect(await edgeCount(page)).toBe(n0 + 1);
    // duplicate
    await dragHandle(page, handle(page, apache, "db"), handle(page, maria, "mysql"));
    expect(await edgeCount(page)).toBe(n0 + 1);
    // wrong direction: input → input
    await dragHandle(page, handle(page, maria, "mysql"), handle(page, "postgresql-01", "postgres"));
    expect(await edgeCount(page)).toBe(n0 + 1);
    // dragging from the input side to a compatible output connects (oriented automatically)
    await dragHandle(page, handle(page, maria, "mysql"), handle(page, apache, "db"));
    expect(await edgeCount(page)).toBe(n0 + 1); // already connected → still one
    await expectEdgesAnchored(page);
  });

  test("property edits, delete, undo/redo and keyboard shortcuts", async ({ page }) => {
    await open(page);
    await openProps(page, "nginx-01");
    await page.getByTestId("instance-name").fill("web-front");
    await page.getByTestId("instance-name").press("Enter");
    await expect(node(page, "nginx-01").locator(".apl-node-header")).toContainText("web-front");
    await page.getByTestId("res-cpu").fill("4");
    await page.getByTestId("res-cpu").press("Enter");
    expect((await assemblyState(page)).spec.appliances.find((a) => a.id === "nginx-01")!.resources.cpu).toBe(4);
    await page.getByTestId("res-cpu").fill("0");
    await page.getByTestId("res-cpu").press("Enter");
    expect((await assemblyState(page)).spec.appliances.find((a) => a.id === "nginx-01")!.resources.cpu).toBe(4); // invalid input rejected

    await node(page, "nginx-02").click();
    await page.keyboard.press("Delete");
    expect((await assemblyState(page)).spec.appliances.length).toBe(5);
    await page.keyboard.press("Control+z");
    expect((await assemblyState(page)).spec.appliances.length).toBe(6);
    await expectEdgesAnchored(page);
    await page.keyboard.press("Control+Shift+z");
    expect((await assemblyState(page)).spec.appliances.length).toBe(5);
    await page.keyboard.press("Control+z");
    // delete a single selected edge
    await page.evaluate(() => (window as unknown as { __studio: { getState: () => { setSelection: (s: unknown) => void; openProps: (t: unknown) => void } } }).__studio.getState().setSelection({ nodes: [], edges: ["conn-01"] }));
    await page.evaluate(() => (window as unknown as { __studio: { getState: () => { openProps: (t: unknown) => void } } }).__studio.getState().openProps({ kind: "cable", id: "conn-01" }));
    await expect(page.getByTestId("inspector-connection")).toBeVisible();
    await page.getByTestId("props-close").click();
    await page.keyboard.press("Delete");
    expect((await assemblyState(page)).spec.connections.some((c) => c.id === "conn-01")).toBe(false);
    await page.getByTestId("undo").click();
    expect((await assemblyState(page)).spec.connections.some((c) => c.id === "conn-01")).toBe(true);
  });
});

test.describe("assembly files", () => {
  test("export is deterministic JSON; import round-trips; bad files are rejected without changing the assembly", async ({ page }) => {
    await open(page);
    const dl1 = page.waitForEvent("download");
    await page.getByTestId("export").click();
    const d1 = await dl1;
    const path1 = await d1.path();
    const text1 = readFileSync(path1, "utf8");
    const dl2 = page.waitForEvent("download");
    await page.getByTestId("export").click();
    expect(readFileSync(await (await dl2).path(), "utf8")).toBe(text1);
    expect(JSON.parse(text1).apiVersion).toBe("fabric.apelora/v1alpha1");

    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-empty").click();
    expect((await assemblyState(page)).spec.appliances.length).toBe(0);
    await page.getByTestId("import-input").setInputFiles({ name: "stack.json", mimeType: "application/json", buffer: Buffer.from(text1) });
    await page.waitForSelector("[data-testid=appliance-node]");
    expect((await assemblyState(page)).spec.appliances.length).toBe(6);
    await page.waitForTimeout(400);
    await expectEdgesAnchored(page);

    await page.getByTestId("import-input").setInputFiles({ name: "bad.json", mimeType: "application/json", buffer: Buffer.from('{"apiVersion":"nope"}') });
    await expect(page.getByTestId("import-errors")).toBeVisible();
    await page.getByRole("button", { name: "Close" }).click();
    expect((await assemblyState(page)).spec.appliances.length).toBe(6);
    await page.getByTestId("import-input").setInputFiles({ name: "junk.json", mimeType: "application/json", buffer: Buffer.from("not json") });
    await expect(page.getByTestId("import-errors")).toContainText("Not valid JSON");
  });

  test("work persists locally across reload, including layout and manual size", async ({ page }) => {
    await open(page);
    await openProps(page, "nginx-01");
    await page.getByTestId("instance-name").fill("persisted");
    await page.getByTestId("instance-name").press("Enter");
    await page.getByTestId("density-comfortable").click();
    await expect.poll(() => page.evaluate(() => localStorage.getItem("apelora.studio.v1") ?? ""), { timeout: 4000 }).toContain("comfortable");
    const before = await assemblyState(page);
    await page.reload();
    await page.waitForSelector("[data-testid=appliance-node]");
    const after = await assemblyState(page);
    const norm = (a: typeof before) => ({
      ...a,
      spec: { appliances: [...a.spec.appliances].sort((x, y) => x.id.localeCompare(y.id)), connections: [...a.spec.connections].sort((x, y) => x.id.localeCompare(y.id)) },
      editor: { ...a.editor, nodes: [...a.editor.nodes].sort((x, y) => x.id.localeCompare(y.id)) },
    });
    expect(norm(after)).toEqual(norm(before));
    expect(after.editor.density).toBe("comfortable");
    const sz = await flowSize(page, "haproxy-01");
    expect(sz.width).toBeCloseTo(152, 0);
    expect(sz.height).toBeCloseTo(112, 0);
  });

  test("corrupted local storage is kept as a backup instead of being overwritten", async ({ page }) => {
    await page.addInitScript(() => { if (!sessionStorage.getItem("__s")) { sessionStorage.setItem("__s", "1"); localStorage.setItem("apelora.studio.v1", JSON.stringify({ version: 1, savedAt: "x", assembly: "{\"broken\":true}" })); } });
    await page.goto("/");
    await page.waitForSelector("[data-testid=appliance-node]");
    await expect(page.getByTestId("toast").first()).toContainText("backup was kept");
    expect(await page.evaluate(() => localStorage.getItem("apelora.studio.v1.invalid"))).toContain("broken");
  });
});

test.describe("validation and quality", () => {
  test("validation panel lists issues with correction hints and focuses the appliance", async ({ page }) => {
    await open(page);
    await expect(page.getByTestId("validation-panel")).toContainText("No issues");
    await page.locator("[data-testid=catalog-tile][data-class-id=nginx]").click();
    await expect(page.getByTestId("error-count")).toBeVisible();
    await page.getByTestId("validation-toggle").click();
    await expect(page.getByTestId("diagnostics")).toContainText("required input");
    await page.getByTestId("diagnostics").getByRole("button").first().click();
    await expect(page.getByTestId("inspector-diagnostics")).toContainText("required input");
    await expect(page.getByTestId("props-tab-issues")).toHaveAttribute("aria-selected", "true");
  });

  test("keyboard: tab reaches catalog tiles with visible focus; zoom shortcuts work; reduced motion is honoured", async ({ page }) => {
    await page.emulateMedia({ reducedMotion: "reduce" });
    await open(page);
    await page.getByTestId("catalog-search").focus();
    await page.keyboard.press("Tab");
    const outline = await page.evaluate(() => { const el = document.activeElement as HTMLElement; return { tag: el.tagName, label: el.getAttribute("aria-expanded") ?? el.getAttribute("aria-label") }; });
    expect(outline.tag).toBe("BUTTON");
    await page.locator("[data-testid=canvas]").click({ position: { x: 20, y: 20 } });
    const z0 = (await viewport(page)).zoom;
    await page.keyboard.press("+");
    await page.waitForTimeout(250);
    expect((await viewport(page)).zoom).toBeGreaterThan(z0);
    await page.keyboard.press("f");
    await page.waitForTimeout(300);
    expect(Math.abs((await viewport(page)).zoom - z0)).toBeLessThan(0.05);
    for (const b of await page.getByRole("button").all()) {
      const name = (await b.getAttribute("aria-label")) || (await b.innerText()) || (await b.getAttribute("title"));
      expect((name ?? "").trim().length, `button without accessible name: ${await b.evaluate((e) => e.outerHTML.slice(0, 120))}`).toBeGreaterThan(0);
    }
  });

  test("no console errors and no network requests beyond localhost", async ({ page }) => {
    const errors: string[] = [];
    const external: string[] = [];
    page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
    page.on("pageerror", (e) => errors.push(e.message));
    page.on("request", (r) => { const u = new URL(r.url()); if (!["127.0.0.1", "localhost"].includes(u.hostname) && u.protocol.startsWith("http")) external.push(r.url()); });
    await open(page);
    await page.getByTestId("density-expanded").click();
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-sample").click();
    await page.waitForTimeout(500);
    expect(errors).toEqual([]);
    expect(external).toEqual([]);
  });
});

test.describe("canvas grid", () => {
  test("grid is a visible line grid aligned to the snap step; the toolbar button hides and shows it", async ({ page }) => {
    await open(page);
    const btn = page.getByRole("button", { name: "Toggle grid" });
    const layers = page.locator(".react-flow__background");
    await expect(btn).toHaveAttribute("aria-pressed", "true");
    await expect(layers).toHaveCount(2); // minor + major lines
    const { zoom } = await viewport(page);
    const minor = page.locator(".react-flow__background.apl-grid-minor pattern");
    const major = page.locator(".react-flow__background.apl-grid-major pattern");
    expect(Number(await minor.getAttribute("width")) / zoom).toBeCloseTo(16, 1); // 2 × the 8-unit snap step
    expect(Number(await major.getAttribute("width")) / zoom).toBeCloseTo(128, 1); // one node width
    await expect(page.locator(".apl-grid-minor .react-flow__background-pattern.lines").first()).toBeAttached();
    // lines must stand out from the canvas: compare luminance of stroke vs canvas background
    const lum = (c: string) => { const [r, g, b] = c.match(/[\d.]+/g)!.map(Number); return 0.2126 * r! + 0.7152 * g! + 0.0722 * b!; };
    const canvas = await resolvedColor(page, ".apl-canvas-frame .react-flow");
    const stroke = await page.locator(".apl-grid-major .react-flow__background-pattern").first().evaluate((el) => getComputedStyle(el).stroke);
    expect(lum(canvas) - lum(stroke)).toBeGreaterThan(20);
    await btn.click();
    await expect(btn).toHaveAttribute("aria-pressed", "false");
    await expect(layers).toHaveCount(0);
    await btn.click();
    await expect(layers).toHaveCount(2);
  });
});
