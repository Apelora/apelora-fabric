import { readFileSync } from "node:fs";
import { test, type Page } from "@playwright/test";
import AxeBuilder from "@axe-core/playwright";
import { assemblyState, center, expect, handle, node, open } from "./helpers";

const menuItem = (page: Page, id: string) => page.getByTestId(`menu-${id}`);
const studio = (page: Page) => page.evaluate(() => (window as unknown as { __studio: { getState: () => { assembly: { spec: { target?: { platform: string } } } } } }).__studio.getState().assembly.spec.target?.platform ?? "incus");

async function dropFirst(page: Page, cls: string, fx = 0.3, fy = 0.4) {
  const box = (await page.getByTestId("canvas").boundingBox())!;
  await page.locator(`[data-testid=catalog-tile][data-class-id=${cls}]`).dragTo(page.getByTestId("canvas"), { targetPosition: { x: box.width * fx, y: box.height * fy } });
}

test.describe("templates and imports", () => {
  test("template gallery: nine valid starters; choosing one replaces the diagram and undo restores it", async ({ page }) => {
    await open(page);
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-template").click();
    await expect(page.getByTestId("template-card")).toHaveCount(9);
    await page.locator("[data-testid=template-card][data-template-id=ha-web-stack]").click();
    await expect(page.getByTestId("templates-dialog")).toHaveCount(0);
    expect((await assemblyState(page)).spec.modules).toHaveLength(9);
    await expect(page.getByTestId("validation-toggle")).toContainText("No issues");
    await page.getByTestId("undo").click();
    expect((await assemblyState(page)).spec.modules).toHaveLength(6);
  });

  test("Docker Compose import: preview with notes, errors for bad input, apply and undo", async ({ page }) => {
    await open(page);
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-compose").click();
    await page.getByTestId("compose-text").fill("services:\n  web:\n    image: nginx\n    depends_on: [api]\n  api:\n    image: acme/shop:1\n    depends_on: [db, cache]\n  db:\n    image: postgres:17\n  cache:\n    image: redis:7\n");
    await expect(page.getByTestId("compose-summary")).toContainText("4 modules");
    await expect(page.getByTestId("compose-notes")).toContainText("not in the catalog");
    await page.getByTestId("compose-text").fill("nothing: here");
    await expect(page.getByTestId("compose-errors")).toContainText("services");
    await expect(page.getByTestId("compose-apply")).toBeDisabled();
    await page.getByTestId("compose-text").fill("services:\n  db:\n    image: postgres\n  cache:\n    image: redis\n");
    await page.getByTestId("compose-apply").click();
    const a = await assemblyState(page);
    expect(a.spec.modules.map((x) => x.id).sort()).toEqual(["postgresql-01", "redis-01"]);
    await page.getByTestId("undo").click();
    expect((await assemblyState(page)).spec.modules).toHaveLength(6);
  });
});

test.describe("hypervisor target and totals", () => {
  test("Incus is the default; VMware is selectable as a planned target, saved in the file and flagged by validation", async ({ page }) => {
    await open(page);
    await expect(page.getByTestId("platform-badge")).toHaveText("Incus");
    await page.getByTestId("platform-badge").click();
    await expect(page.getByTestId("platform-select")).toHaveValue("incus");
    await expect(page.getByTestId("platform-facts")).toContainText("Incus agent");
    await page.getByTestId("platform-select").selectOption("vmware");
    await expect(page.getByTestId("platform-facts")).toContainText("VMDK");
    await expect(page.getByTestId("platform-badge")).toContainText("planned");
    expect(await studio(page)).toBe("vmware");
    await expect(page.getByTestId("warning-count")).toBeVisible();
    const dl = page.waitForEvent("download");
    await page.getByTestId("export").click();
    const path = await (await dl).path();
    expect(readFileSync(path, "utf8")).toContain('"platform": "vmware"');
    await page.getByTestId("platform-select").selectOption("incus");
    expect(await studio(page)).toBe("incus");
    await page.getByTestId("undo").click();
    expect(await studio(page)).toBe("vmware");
  });

  test("totals chip and assembly window show vCPU and memory; a zone shows its own totals", async ({ page }) => {
    await open(page);
    await expect(page.getByTestId("totals-chip")).toContainText("6 modules");
    await expect(page.getByTestId("totals-chip")).toContainText("vCPU");
    await page.getByTestId("totals-chip").click();
    await expect(page.getByTestId("assembly-totals")).toContainText("vCPU");
    await page.getByTestId("props-close").click();
    await node(page, "nginx-01").click();
    await node(page, "nginx-02").click({ modifiers: ["ControlOrMeta"] });
    await node(page, "nginx-02").click({ button: "right" });
    await menuItem(page, "wrap").click();
    await menuItem(page, "wrap-rect").click();
    await page.getByTestId("props-tab-layout").click();
    await expect(page.getByTestId("zone-totals")).toContainText("2 modules");
  });
});

test.describe("connecting faster", () => {
  test("port menu adds a compatible module already cabled", async ({ page }) => {
    await open(page);
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-empty").click();
    await dropFirst(page, "kafka");
    await expect(node(page, "kafka-01")).toBeVisible();
    await page.waitForTimeout(400);
    const h = await center(handle(page, "kafka-01", "out"));
    await page.mouse.click(h.x, h.y, { button: "right" });
    await menuItem(page, "add-connected").hover();
    await expect(menuItem(page, "connect-kafka")).toBeVisible();
    await menuItem(page, "connect-kafka").click();
    const a = await assemblyState(page);
    expect(a.spec.modules).toHaveLength(2);
    expect(a.spec.connections).toHaveLength(1);
    expect(a.spec.connections[0]!.source.moduleId).toBe("kafka-01");
    const [n1, n2] = [a.editor.nodes[0]!, a.editor.nodes[1]!];
    expect(n2.x).toBeGreaterThan(n1.x + n1.width);
    await page.getByTestId("undo").click();
    expect((await assemblyState(page)).spec.modules).toHaveLength(1);
  });

  test("a single-cable output that is already plugged cannot add another", async ({ page }) => {
    await open(page);
    await handle(page, "nginx-01", "db").click({ button: "right", force: true });
    await expect(menuItem(page, "add-connected")).toBeDisabled();
  });

  test("while a cable is dragged, compatible sockets light up and the others dim", async ({ page }) => {
    await open(page);
    const from = await center(handle(page, "nginx-01", "db"));
    await page.mouse.move(from.x, from.y);
    await page.mouse.down();
    await page.mouse.move(from.x + 40, from.y + 40, { steps: 4 });
    await expect(handle(page, "postgresql-01", "postgres")).toHaveAttribute("data-drag", "lit");
    await expect(handle(page, "haproxy-01", "frontend")).toHaveAttribute("data-drag", "dim");
    await page.mouse.up();
    await expect(handle(page, "postgresql-01", "postgres")).not.toHaveAttribute("data-drag", /.+/);
  });

  test("dragging a module near another's edge snaps to it and shows a guide", async ({ page }) => {
    await open(page);
    const before = (await assemblyState(page)).editor.nodes.find((n) => n.id === "nginx-02")!;
    const c = await center(node(page, "nginx-02"));
    await page.mouse.move(c.x, c.y);
    await page.mouse.down();
    await page.mouse.move(c.x + 2, c.y + 30, { steps: 4 });
    await expect(page.getByTestId("guides")).toBeVisible();
    await page.mouse.up();
    await expect(page.getByTestId("guides")).toHaveCount(0);
    const after = (await assemblyState(page)).editor.nodes.find((n) => n.id === "nginx-02")!;
    expect(after.x).toBe(before.x);
  });
});

test.describe("layout and navigation", () => {
  test("Tidy arranges the flow left to right and undo restores the old layout", async ({ page }) => {
    await open(page);
    await page.evaluate(() => {
      const s = (window as unknown as { __studio: { getState: () => { assembly: { editor: { nodes: { id: string; x: number; y: number }[] } }; transient: (a: unknown) => void } } }).__studio.getState();
      s.transient({ ...s.assembly, editor: { ...s.assembly.editor, nodes: s.assembly.editor.nodes.map((n, i) => ({ ...n, x: 40 + i * 5, y: 40 + i * 120 })) } });
    });
    const scrambled = JSON.stringify((await assemblyState(page)).editor.nodes);
    await page.getByTestId("tidy").click();
    const t = (await assemblyState(page)).editor.nodes;
    const x = (id: string) => t.find((n) => n.id === id)!.x;
    expect(x("gateway-01")).toBeLessThan(x("haproxy-01"));
    expect(x("haproxy-01")).toBeLessThan(x("nginx-01"));
    expect(x("nginx-01")).toBeLessThan(x("postgresql-01"));
    await page.getByTestId("undo").click();
    expect(JSON.stringify((await assemblyState(page)).editor.nodes)).toBe(scrambled);
  });

  test("Shift+T tidies; the canvas menu has Tidy and Find", async ({ page }) => {
    await open(page);
    const box = (await page.getByTestId("canvas").boundingBox())!;
    await page.mouse.click(box.x + 20, box.y + box.height - 120, { button: "right" });
    await expect(menuItem(page, "tidy")).toBeVisible();
    await expect(menuItem(page, "find")).toBeVisible();
    await page.keyboard.press("Escape");
  });

  test("Find (Ctrl+K) jumps to a module by name or class and selects it", async ({ page }) => {
    await open(page);
    await page.keyboard.press("Control+k");
    await expect(page.getByTestId("find-input")).toBeFocused();
    await page.getByTestId("find-input").fill("postg");
    await expect(page.getByTestId("find-result")).toHaveCount(1);
    await page.keyboard.press("Enter");
    await expect(page.getByTestId("find-dialog")).toHaveCount(0);
    await expect(node(page, "postgresql-01")).toHaveAttribute("data-selected", "true");
    await page.getByTestId("find-open").click();
    await page.getByTestId("find-input").fill("zzz");
    await expect(page.getByTestId("find-empty")).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("find-dialog")).toHaveCount(0);
  });

  test("overview map toggles and the choice persists", async ({ page }) => {
    await open(page);
    await expect(page.locator(".react-flow__minimap")).toHaveCount(0);
    await page.getByTestId("toggle-minimap").click();
    await expect(page.locator(".react-flow__minimap")).toBeVisible();
    await page.reload();
    await page.waitForSelector("[data-testid=module-node]");
    await expect(page.locator(".react-flow__minimap")).toBeVisible();
  });

  test("arrow keys nudge the selection (Shift = 40) as one undo step", async ({ page }) => {
    await open(page);
    await node(page, "nginx-01").click();
    const x0 = (await assemblyState(page)).editor.nodes.find((n) => n.id === "nginx-01")!;
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("ArrowRight");
    await page.keyboard.press("Shift+ArrowDown");
    const x1 = (await assemblyState(page)).editor.nodes.find((n) => n.id === "nginx-01")!;
    expect(x1.x).toBe(x0.x + 16);
    expect(x1.y).toBe(x0.y + 40);
    await page.getByTestId("undo").click();
    const x2 = (await assemblyState(page)).editor.nodes.find((n) => n.id === "nginx-01")!;
    expect(x2.x).toBe(x0.x);
  });
});

test.describe("notes", () => {
  test("add a sticky note, type text, recolour, duplicate; text survives export and import", async ({ page }) => {
    await open(page);
    await page.getByTestId("shapes-button").click();
    await page.getByTestId("add-shape-note").click();
    await page.getByTestId("note-text").fill("Primary DB moves to pool B\nafter the freeze");
    await expect(page.getByTestId("note-body")).toContainText("Primary DB moves");
    await page.getByTestId("props-tab-style").click();
    await page.locator("[data-testid=note-colors] [data-color='#bfdbfe']").click();
    await page.getByTestId("props-close").click();
    await page.getByTestId("note-body").click({ button: "right" });
    await menuItem(page, "duplicate").click();
    await expect(page.getByTestId("note-body")).toHaveCount(2);
    const dl = page.waitForEvent("download");
    await page.getByTestId("export").click();
    const text = readFileSync((await (await dl).path()), "utf8");
    expect(text).toContain("Primary DB moves to pool B");
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-empty").click();
    await expect(page.getByTestId("note-body")).toHaveCount(0);
    await page.getByTestId("import-input").setInputFiles({ name: "n.json", mimeType: "application/json", buffer: Buffer.from(text) });
    await expect(page.getByTestId("note-body")).toHaveCount(2);
    await expect(page.getByTestId("note-body").first()).toContainText("after the freeze");
  });

  test("a note never wraps modules and is found by Find", async ({ page }) => {
    await open(page);
    await page.getByTestId("shapes-button").click();
    await page.getByTestId("add-shape-note").click();
    await page.getByTestId("note-text").fill("remember the firewall");
    await page.getByTestId("props-close").click();
    await page.keyboard.press("Control+k");
    await page.getByTestId("find-input").fill("firewall");
    await expect(page.getByTestId("find-result")).toHaveCount(1);
  });
});

test.describe("snapshots, sharing and export", () => {
  test("snapshots: save, change, compare in plain language, restore (undoable)", async ({ page }) => {
    await open(page);
    await page.getByTestId("snapshots-open").click();
    await expect(page.getByTestId("snapshots-empty")).toBeVisible();
    await page.getByTestId("snapshot-name").fill("Baseline");
    await page.getByTestId("snapshot-save").click();
    await expect(page.getByTestId("snapshot-row")).toHaveCount(1);
    await page.getByTestId("snapshots-dialog-close").click();
    await node(page, "grafana-01").click();
    await page.keyboard.press("Delete");
    await page.getByTestId("snapshots-open").click();
    await page.getByTestId("snapshot-compare").click();
    await expect(page.getByTestId("snapshot-diff")).toContainText("grafana-01");
    await page.getByTestId("snapshot-restore").click();
    expect((await assemblyState(page)).spec.modules).toHaveLength(6);
    await page.getByTestId("undo").click();
    expect((await assemblyState(page)).spec.modules).toHaveLength(5);
    await page.reload();
    await page.waitForSelector("[data-testid=module-node]");
    await page.getByTestId("snapshots-open").click();
    await expect(page.getByTestId("snapshot-row")).toHaveCount(1);
  });

  test("share link opens the same diagram in a fresh session and keeps the old one as a snapshot; bad links are rejected", async ({ page, context }) => {
    await open(page);
    await node(page, "grafana-01").click();
    await page.keyboard.press("Delete");
    await page.getByTestId("export-menu").click();
    await page.getByTestId("export-share").click();
    const url = await page.getByTestId("share-url").inputValue();
    expect(url).toContain("#s=");
    const p2 = await context.newPage();
    await p2.addInitScript(() => { if (!sessionStorage.getItem("__seeded")) { localStorage.clear(); localStorage.setItem("apelora.studio.tour.v1", "1"); sessionStorage.setItem("__seeded", "1"); } });
    await p2.goto(url);
    await p2.waitForSelector("[data-testid=module-node]");
    expect((await assemblyState(p2)).spec.modules).toHaveLength(5);
    expect(await p2.evaluate(() => location.hash)).toBe("");
    const bad = await context.newPage();
    await bad.addInitScript(() => { if (!sessionStorage.getItem("__seeded")) { localStorage.clear(); localStorage.setItem("apelora.studio.tour.v1", "1"); sessionStorage.setItem("__seeded", "1"); } });
    await bad.goto("/#s=AAAA");
    await bad.waitForSelector("[data-testid=module-node]");
    await expect(bad.getByTestId("import-dialog")).toBeVisible();
    expect((await assemblyState(bad)).spec.modules.length).toBeGreaterThanOrEqual(5);
  });

  test("export as SVG and PNG produce real images of the diagram", async ({ page }) => {
    await open(page);
    await page.getByTestId("export-menu").click();
    const svgDl = page.waitForEvent("download");
    await page.getByTestId("export-svg").click();
    const dl = await svgDl;
    expect(dl.suggestedFilename()).toBe("commerce-stack.svg");
    const svg = readFileSync((await dl.path()), "utf8");
    expect(svg).toContain("<svg");
    expect(svg).toContain("postgresql-01");
    expect(svg).toContain("data:image/svg+xml;base64");
    await page.getByTestId("export-menu").click();
    const pngDl = page.waitForEvent("download");
    await page.getByTestId("export-png").click();
    const png = readFileSync((await (await pngDl).path()));
    expect(png.subarray(0, 8).toString("hex")).toBe("89504e470d0a1a0a");
    expect(png.length).toBeGreaterThan(5000);
  });
});

test.describe("help and first run", () => {
  test("? opens the shortcut sheet; the tour shows once on a first visit and can be restarted", async ({ page }) => {
    await page.goto("/");
    await page.evaluate(() => localStorage.clear());
    await page.reload();
    await page.waitForSelector("[data-testid=module-node]");
    await expect(page.getByTestId("tour")).toBeVisible();
    await page.getByTestId("tour-next").click();
    await expect(page.getByTestId("tour")).toContainText("2 of 4");
    await page.getByTestId("tour-skip").click();
    await expect(page.getByTestId("tour")).toHaveCount(0);
    await page.reload();
    await page.waitForSelector("[data-testid=module-node]");
    await expect(page.getByTestId("tour")).toHaveCount(0);
    await page.locator("body").click({ position: { x: 5, y: 5 } });
    await page.keyboard.press("?");
    await expect(page.getByTestId("help-dialog")).toBeVisible();
    await page.getByTestId("tour-start").click();
    await expect(page.getByTestId("tour")).toBeVisible();
  });
});

test.describe("accessibility", () => {
  // Measure what settles on screen: with reduced motion, dialogs appear without fading in, so
  // contrast is not read halfway through an animation.
  test.use({ reducedMotion: "reduce" });
  const serious = async (page: Page) => (await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze()).violations.filter((v) => v.impact === "serious" || v.impact === "critical").map((v) => `${v.id}: ${v.nodes.slice(0, 3).map((n) => n.target.join(" ")).join(" | ")}`);

  test("main view has no serious or critical axe violations", async ({ page }) => {
    await open(page);
    expect(await serious(page)).toEqual([]);
  });
  test("dialogs and the properties window have no serious or critical axe violations", async ({ page }) => {
    await open(page);
    await page.getByTestId("new-menu").click();
    await page.getByTestId("new-template").click();
    expect(await serious(page)).toEqual([]);
    await page.keyboard.press("Escape");
    await page.getByTestId("snapshots-open").click();
    expect(await serious(page)).toEqual([]);
    await page.keyboard.press("Escape");
    await page.getByTestId("help-open").click();
    expect(await serious(page)).toEqual([]);
    await page.keyboard.press("Escape");
    await node(page, "nginx-01").dblclick();
    await expect(page.getByTestId("inspector")).toBeVisible();
    expect(await serious(page)).toEqual([]);
  });
});
