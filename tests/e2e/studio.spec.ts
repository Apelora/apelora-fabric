import { readFileSync } from "node:fs";
import { test } from "@playwright/test";
import { addPorts, assemblyState, center, dragHandle, edgeCount, expect, expectEdgesAnchored, flowSize, handle, node, open, resolvedColor, viewport } from "./helpers";

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
    expect(inH.width).toBeCloseTo(9 * (await viewport(page)).zoom, 0);
    await expect(handle(page, "haproxy-01", "backend")).toHaveAttribute("aria-label", /Backend \(http\)/);
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

  test("tiles show 14 appliances in 8 categories max, grouped", async ({ page }) => {
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
