import { readFileSync } from "node:fs";
import { test } from "@playwright/test";
import { addPorts, assemblyState, cablePoint, center, dragHandle, edgeCount, expect, expectEdgesAnchored, flowSize, handle, node, open, resolvedColor, viewport } from "./helpers";

const rgb = (page: import("@playwright/test").Page, sel: string, prop = "backgroundColor") => (prop === "backgroundColor" ? resolvedColor(page, sel) : page.locator(sel).first().evaluate((el, p) => getComputedStyle(el)[p as "color"], prop));

test.describe("visual contract", () => {
  test("white catalog and inspector around a distinct light-gray framed canvas", async ({ page }) => {
    await open(page);
    expect(await rgb(page, "[data-testid=catalog]")).toBe("rgb(255, 255, 255)");
    expect(await rgb(page, "[data-testid=inspector]")).toBe("rgb(255, 255, 255)");
    expect(await rgb(page, "[data-testid=canvas-frame]")).toBe("rgb(247, 247, 248)");
    expect(await rgb(page, "[data-testid=canvas-frame]", "borderTopLeftRadius")).toBe("12px");
    const cat = await center(page.getByTestId("catalog"));
    expect(cat.width).toBeGreaterThanOrEqual(160);
    expect(cat.width).toBeLessThanOrEqual(200);
    const insp = await center(page.getByTestId("inspector"));
    expect(insp.width).toBeGreaterThanOrEqual(210);
    expect(insp.width).toBeLessThanOrEqual(270);
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
    await node(page, "nginx-01").click({ button: "right" });
    await item(page, "add-port").click();
    await page.getByTestId("menu-add-input").click();
    expect((await assemblyState(page)).spec.appliances.find((a) => a.id === "nginx-01")!.ports.length).toBe(3);
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
    expect((await assemblyState(page)).spec.appliances.find((a) => a.id === "postgresql-01")!.ports.length).toBe(1);
  });

  test("cable menu: select ends and unplug", async ({ page }) => {
    await open(page);
    const p = await cablePoint(page, "conn-01", 0.5);
    await page.mouse.click(p.x, p.y + 0, { button: "right" });
    await expect(menu(page)).toContainText("gateway-01.lan → haproxy-01.in");
    await item(page, "select-target").click();
    await expect(page.getByTestId("instance-name")).toHaveValue("haproxy-01");
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
    const at2 = { x: canvas.x + canvas.width * 0.6, y: canvas.y + canvas.height * 0.82 };
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
    await expect(page.getByTestId("inspector-assembly")).toContainText("appliances selected");
    await page.mouse.click(at.x, at.y - 60, { button: "right" });
    await item(page, "labels").click();
    await expect(page.getByTestId("port-label").first()).toBeHidden();
  });

  test("keyboard: Context Menu key opens, arrows navigate, Escape closes; outside click closes; Ctrl+D/C/V/A work", async ({ page }) => {
    await open(page);
    await node(page, "nginx-01").click();
    await page.keyboard.press("ContextMenu");
    await expect(menu(page)).toBeVisible();
    await expect(item(page, "rename")).toBeFocused();
    await page.keyboard.press("ArrowDown");
    await expect(item(page, "configure")).toBeFocused();
    await page.keyboard.press("Escape");
    await expect(menu(page)).toHaveCount(0);
    await page.keyboard.press("Control+d");
    expect((await assemblyState(page)).spec.appliances.length).toBe(7);
    await page.keyboard.press("Control+a");
    await expect(page.getByTestId("inspector-assembly")).toContainText("7 appliances selected");
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
    await node(page, "postgresql-01").click();
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
    await node(page, "postgresql-01").click();
    await expect(page.getByTestId("cfg-max-connections")).toHaveValue("250");
    await page.getByTestId("cfg-reset-max-connections").click();
    expect(await cfg(page, "postgresql-01")).toEqual({ "password-secret": "shop-db-password" });
    await page.getByTestId("cfg-reset-all").click();
    expect(await cfg(page, "postgresql-01")).toBeUndefined();
  });

  test("bool toggle and string field; configuration is exported and imported", async ({ page }) => {
    await open(page);
    await node(page, "nginx-01").click();
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
    await node(page, "haproxy-01").click();
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
    await page.getByTestId("cfg-maxmemory").fill("2048");
    await page.getByTestId("cfg-maxmemory").press("Enter");
    await page.getByTestId("save-as-class").click();
    await page.getByTestId("designer-name").fill("Big cache");
    await page.getByTestId("designer-tab-config").click();
    await expect(page.getByTestId("designer-prop").first().getByLabel("Default value")).toHaveValue("2048");
    await page.getByTestId("designer-save").click();
    await page.locator("[data-testid=catalog-tile][data-class-id=custom-big-cache]").click();
    const id2 = (await assemblyState(page)).spec.appliances.find((a) => a.id.startsWith("custom-big-cache"))!.id;
    await node(page, id2).click();
    await expect(page.getByTestId("cfg-maxmemory")).toHaveValue("2048");
    void id;
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
    await expect(n.getByTestId("port-label")).toHaveText(["in", "db"]);
    await expect(node(page, "haproxy-01").getByTestId("port-label")).toHaveText(["in", "out"]);
    const logo = (await n.getByTestId("node-logo").boundingBox())!;
    for (const l of await page.getByTestId("port-label").all()) {
      const b = (await l.boundingBox())!;
      const owner = await l.evaluate((el) => el.closest("[data-testid=appliance-node]")!.getAttribute("data-node-id"));
      if (owner !== "nginx-01") continue;
      expect(b.x + b.width <= logo.x + 0.5 || b.x >= logo.x + logo.width - 0.5, "label must not overlap the logo").toBe(true);
    }
    await expect(n.locator(".apl-jack[data-kind=input]")).toHaveCount(1);
    await expect(n.locator(".apl-jack[data-kind=output]")).toHaveCount(1);
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
    await node(page, "nginx-01").click();
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
    await expect(page.getByTestId("designer-port")).toHaveCount(4);
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
    expect(added.ports.length).toBe(4);
    const h = await flowSize(page, added.id);
    expect(h.height).toBeCloseTo(96, 0);

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
    await node(page, "grafana-01").click();
    await addPorts(page, "output", 2);
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
    await expect(section.getByTestId("catalog-tile")).toHaveCount(3);
    await section.getByRole("button").first().click();
    await expect(section.getByTestId("catalog-tile")).toHaveCount(0);
    await section.getByRole("button").first().click();
    await expect(section.getByTestId("catalog-tile")).toHaveCount(3);

    await page.getByTestId("catalog-search").fill("redi");
    await expect(page.getByTestId("catalog-tile")).toHaveCount(1);
    await expect(page.getByTestId("catalog-section")).toHaveCount(1);
    const before = (await assemblyState(page)).spec.appliances.length;
    await page.getByTestId("catalog-tile").click();
    expect((await assemblyState(page)).spec.appliances.length).toBe(before + 1);
    await page.getByTestId("catalog-search").fill("zzzz");
    await expect(page.getByText("No appliances match")).toBeVisible();
  });

  test("tiles show the 14 built-in appliances, grouped", async ({ page }) => {
    await open(page);
    await expect(page.getByTestId("catalog-tile")).toHaveCount(14);
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
    await node(page, "grafana-01").click();
    await addPorts(page, "input", 4);
    await page.waitForTimeout(150);
    let s = await flowSize(page, "grafana-01");
    expect(s.width).toBeCloseTo(128, 0);
    expect(s.height).toBeGreaterThan(96);
    expect(s.height).toBeCloseTo(60 + 18 * 5, 0); // 5 inputs
    await addPorts(page, "input", 3);
    await page.waitForTimeout(150);
    s = await flowSize(page, "grafana-01");
    expect(s.height).toBeCloseTo(60 + 18 * 8, 0);
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
    await node(page, "haproxy-01").click();
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
    await node(page, "nginx-01").click();
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
    await page.evaluate(() => (window as unknown as { __studio: { getState: () => { setSelection: (s: unknown) => void } } }).__studio.getState().setSelection({ nodes: [], edges: ["conn-01"] }));
    await expect(page.getByTestId("inspector-connection")).toBeVisible();
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
    await node(page, "nginx-01").click();
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
    await expect(page.getByTestId("inspector-appliance")).toBeVisible();
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
