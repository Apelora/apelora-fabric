import { randomBytes } from "node:crypto";

import { expect, test, type Browser, type Page } from "@playwright/test";

import { adminToken, api, canConnect, consoleUrl, enabled, inInstance, instanceType, ipv4ByInstance, probe, studioUrl, type AppStatus } from "./lab";

// REAL INCUS TESTED, through the screens: an administrator signs in to the Console, creates a
// local account and gives it a role; that person signs in to the Studio with a password, builds
// applications on the canvas, deploys and operates them. Every claim is checked against the
// cluster itself: where instances run, their addresses, the service address answering from both
// replicas, connections enforced, data kept, service names resolved.
// Run with scripts/test-cluster/ui-e2e.sh (it starts everything and cleans up).

test.skip(!enabled, "real cluster only: bash scripts/test-cluster/ui-e2e.sh");
test.describe.configure({ mode: "serial" });

const viewport = { width: 1440, height: 900 };
const username = "e2e.operator";
const password = `lab ${randomBytes(9).toString("hex")} words`;
const DEPLOY_MS = 6 * 60_000;
let admin: Page;
let studio: Page;

async function newPage(browser: Browser, baseURL: string): Promise<Page> {
  const ctx = await browser.newContext({ baseURL, viewport });
  await ctx.addInitScript(() => localStorage.setItem("apelora.studio.tour.v1", "1"));
  return ctx.newPage();
}

async function waitForStatus(app: string, ok: (s: AppStatus) => boolean, timeout = DEPLOY_MS): Promise<AppStatus> {
  let last: AppStatus | undefined;
  await expect.poll(async () => { last = await api<AppStatus>(`/applications/${app}/status`); return ok(last); }, { timeout, intervals: [3000] }).toBe(true);
  return last!;
}

// --- Studio steps -------------------------------------------------------------------------

async function newApplication(name: string) {
  await studio.goto("/studio/applications");
  await studio.locator("main").getByRole("button", { name: "New application" }).first().click();
  const d = studio.getByTestId("new-application-dialog");
  await d.getByTestId("new-app-name").fill(name);
  await d.getByTestId("from-blank").check();
  await d.getByTestId("new-app-create").click();
  await expect(studio).toHaveURL(new RegExp(`/studio/applications/${name}/designer$`));
  await expect(studio.getByTestId("canvas")).toBeVisible();
}

/** Drags a catalog tile onto the canvas (found with the catalog's search) and returns the new module's id. */
async function place(classId: string, fx: number, fy: number): Promise<string> {
  const before = await studio.locator("[data-testid=module-node]").evaluateAll((ns) => ns.map((n) => n.getAttribute("data-node-id")));
  await studio.getByTestId("catalog-search").fill(classId);
  const canvas = studio.getByTestId("canvas");
  const box = (await canvas.boundingBox())!;
  await studio.locator(`[data-testid=catalog-tile][data-class-id="${classId}"]`).dragTo(canvas, { targetPosition: { x: box.width * fx, y: box.height * fy } });
  await studio.getByTestId("catalog-search").fill("");
  await expect(studio.locator("[data-testid=module-node]")).toHaveCount(before.length + 1);
  const after = await studio.locator("[data-testid=module-node]").evaluateAll((ns) => ns.map((n) => n.getAttribute("data-node-id")));
  return after.find((id) => !before.includes(id))!;
}

async function properties(id: string, tab?: "ports") {
  await studio.locator(`[data-testid=module-node][data-node-id="${id}"]`).dblclick();
  await expect(studio.getByTestId("props-title")).toBeVisible();
  if (tab) await studio.getByTestId(`props-tab-${tab}`).click();
}

/** Gives a module an HTTP output named `web` (a client calling a web tier) and returns its port id. */
async function addHttpOutput(id: string): Promise<string> {
  await properties(id, "ports");
  const insp = studio.getByTestId("inspector");
  await insp.getByTestId("add-output").click();
  const row = insp.getByTestId("port-row").last();
  await row.locator("select").selectOption("http");
  const name = row.getByRole("textbox").first();
  await name.fill("web");
  await name.press("Enter");
  await studio.keyboard.press("Escape");
  const outputs = await studio.locator(`.react-flow__node[data-id="${id}"] .react-flow__handle.source`).evaluateAll((hs) => hs.map((h) => h.getAttribute("data-port-id")));
  return outputs[outputs.length - 1]!;
}

async function connect(from: string, fromPort: string, to: string, toPort: string) {
  const edges = await studio.locator(".react-flow__edge").count();
  const a = (await studio.locator(`.react-flow__node[data-id="${from}"] .react-flow__handle[data-port-id="${fromPort}"]`).boundingBox())!;
  const b = (await studio.locator(`.react-flow__node[data-id="${to}"] .react-flow__handle[data-port-id="${toPort}"]`).boundingBox())!;
  await studio.mouse.move(a.x + a.width / 2, a.y + a.height / 2);
  await studio.mouse.down();
  await studio.mouse.move((a.x + b.x) / 2, (a.y + b.y) / 2, { steps: 6 });
  await studio.mouse.move(b.x + b.width / 2, b.y + b.height / 2, { steps: 6 });
  await studio.mouse.up();
  await expect(studio.locator(".react-flow__edge")).toHaveCount(edges + 1);
}

async function publish(app: string, label: string) {
  await expect(studio.getByTestId("save-state")).toHaveText("All changes saved");
  await studio.getByRole("button", { name: "Publish version…" }).click();
  await studio.getByTestId("publish-label").fill(label);
  await studio.getByTestId("publish-confirm").click();
  const closed = await studio.getByTestId("publish-confirm").waitFor({ state: "detached", timeout: 30_000 }).then(() => true, () => false);
  if (!closed) throw new Error(`the publish dialog stayed open: ${await studio.getByTestId("publish-confirm").locator("xpath=ancestor::*[@role='dialog' or self::dialog][1]").innerText()}`);
  const detail = await api<{ application: { design: { publishedVersion: number | null } } }>(`/applications/${app}`);
  expect(detail.application.design.publishedVersion, "a version was published").not.toBeNull();
}

/** Deploys the latest version from the Applications list and waits until every job succeeded. */
async function deploy(app: string, changes: number) {
  await studio.goto("/studio/applications");
  await studio.locator(`[data-testid=app-card][data-app="${app}"]`).getByTestId("action-deploy").click();
  await studio.getByTestId("deploy-version-continue").click();
  const dialog = studio.getByTestId("deploy-dialog");
  await dialog.getByTestId("deploy-preview").click();
  await expect(dialog.getByTestId("plan-row")).toHaveCount(changes);
  await expect(dialog.getByTestId("deploy-apply")).toHaveText(`Deploy ${changes} change${changes === 1 ? "" : "s"}`);
  await dialog.getByTestId("deploy-apply").click();
  const jobs = dialog.getByTestId("deploy-job");
  await expect(jobs).toHaveCount(changes);
  for (let i = 0; i < changes; i++) await expect(jobs.nth(i)).toContainText("succeeded", { timeout: DEPLOY_MS });
  await dialog.getByTestId("deploy-done").click();
}

/** Stop (confirming the interruption when the server asks), then Start, from the application's card. */
async function stopAndStart(app: string) {
  await studio.goto("/studio/applications");
  const card = studio.locator(`[data-testid=app-card][data-app="${app}"]`);
  await card.getByTestId("action-stop").click();
  await studio.getByTestId("lifecycle-confirm").click();
  // A module with one instance is interrupted: the server asks again, and so does the dialog.
  // (`isVisible` does not wait; the server's answer takes a moment.)
  const again = studio.getByRole("button", { name: "Stop and interrupt service" });
  if (await again.waitFor({ state: "visible", timeout: 10_000 }).then(() => true, () => false)) await again.click();
  await waitForStatus(app, (s) => s.items.length > 0 && s.items.every((i) => i.status === "Stopped"));
  await expect(card.getByTestId("action-start")).toBeEnabled();
  await card.getByTestId("action-start").click();
  const confirmStart = studio.getByTestId("lifecycle-confirm");
  if (await confirmStart.waitFor({ state: "visible", timeout: 3000 }).then(() => true, () => false)) await confirmStart.click();
  await waitForStatus(app, (s) => s.items.length > 0 && s.items.every((i) => i.status === "Running"));
}

// --- The run ------------------------------------------------------------------------------

test.beforeAll(async ({ browser }) => {
  admin = await newPage(browser, consoleUrl);
  studio = await newPage(browser, studioUrl);
});

test("an administrator signs in to the Fabric Console and sees the real cluster", async () => {
  await admin.goto("/");
  await admin.getByLabel("API token").fill(adminToken());
  await admin.getByRole("button", { name: "Sign in" }).click();
  await expect(admin.getByText(/Connected · Incus \d/)).toBeVisible();
  const cluster = await api<{ members: { name: string; status: string }[] }>("/cluster");
  const online = cluster.members.filter((m) => m.status === "Online").length;
  expect(online, "the test needs three online members").toBeGreaterThanOrEqual(3);
  await expect(admin.getByRole("heading", { name: /Cluster members/ })).toContainText(`(${cluster.members.length})`);
});

test("the administrator creates a local account, the person sets a password, and gets a role", async ({ browser }) => {
  await admin.getByRole("link", { name: "Users", exact: true }).click();
  await admin.getByRole("button", { name: "Create local account…" }).click();
  const create = admin.getByRole("dialog");
  await create.getByLabel("Username").fill(username);
  await create.getByLabel("Display name (optional)").fill("E2E Operator");
  await create.getByRole("button", { name: "Create account" }).click();
  const link = await create.getByLabel("Setup link").inputValue();
  await create.getByRole("button", { name: "I have sent it" }).click();

  // The person opens the link in their own browser.
  const person = await newPage(browser, consoleUrl);
  await person.goto(link);
  await person.getByLabel("New password").fill(password);
  await person.getByLabel("Type it again").fill(password);
  await person.getByRole("button", { name: "Set password" }).click();
  await expect(person.getByRole("heading", { name: "Your password is set" })).toBeVisible();
  await person.context().close();

  await admin.getByRole("button", { name: "Assign a role to E2E Operator" }).click();
  const assign = admin.getByRole("dialog");
  await assign.getByLabel("Role").selectOption({ label: "Project Administrator" });
  await assign.getByLabel("Where").selectOption({ label: "Project" });
  await assign.getByLabel("Project").selectOption({ index: 1 });
  await expect(assign).toContainText("application.deploy");
  await assign.getByRole("button", { name: "Save assignment" }).click();
  await expect(admin.locator("main")).toContainText("Project Administrator assigned to E2E Operator");

  await admin.getByRole("link", { name: "Sign-in", exact: true }).click();
  await admin.getByRole("button", { name: "Turn on local accounts" }).click();
  await expect(admin.locator("main")).toContainText("1 account can sign in");
});

test("the person signs in to the Studio with their username and password", async () => {
  await studio.goto("/studio/applications");
  await studio.getByTestId("source-chip").click();
  const d = studio.getByTestId("connect-dialog");
  await d.getByTestId("connect-username").fill(username);
  await d.getByTestId("connect-password").fill(password);
  await d.getByTestId("connect-password-submit").click();
  await expect(studio.getByTestId("source-chip")).toContainText(`Control plane · ${username}`);
});

test.describe("a web tier: a client and two NGINX replicas", () => {
  const app = "e2e-web";
  let svc = "";

  test("is built on the canvas, published and deployed", async () => {
    await newApplication(app);
    const client = await place("debian", 0.3, 0.45);
    const web = await place("nginx", 0.75, 0.45);
    await studio.getByRole("button", { name: /Tidy/ }).click();
    const out = await addHttpOutput(client);
    await connect(client, out, web, "http");
    await expect(studio.getByText("No issues")).toBeVisible();
    await properties(web);
    await studio.getByTestId("replicas").fill("2");
    await studio.getByTestId("replicas").press("Enter");
    await studio.keyboard.press("Escape");
    await publish(app, "lab web tier");
    await deploy(app, 3);
    const s = await waitForStatus(app, (x) => x.items.length === 3 && x.items.every((i) => i.status === "Running") && x.services.length === 1);
    const replicas = s.items.filter((i) => i.module === web);
    expect(new Set(replicas.map((r) => r.location)).size, "replicas on different members").toBe(2);
    svc = s.services[0]!.address;
  });

  test("its canvas shows what runs when opened again", async () => {
    await studio.goto(`/studio/applications/${app}/designer`);
    const live = studio.locator(`[data-node-id="nginx-01"] [data-testid=live-status]`);
    await expect(live).toHaveAttribute("data-state", "Running");
    await expect(live).toHaveAttribute("title", new RegExp(`Service address ${svc.replace(/\./g, "\\.")}`));
    await expect(studio.getByTestId("mock-badge")).toHaveText("Live · 3 running");
  });

  test("has addresses, answers on its service address from both replicas, and opens only the drawn connection", async () => {
    const client = `${app}-debian-01`;
    const [r1, r2] = [`${app}-nginx-01`, `${app}-nginx-01-r2`];
    for (const r of [r1, r2]) inInstance(r, `echo ${r} > /usr/share/nginx/html/index.html`);
    const ip = ipv4ByInstance();
    for (const i of [client, r1, r2]) expect(ip[i]?.length, `${i} has an IPv4 address`).toBeGreaterThan(0);
    expect(inInstance(client, "echo $APELORA_SERVICE_NGINX_01").trim()).toBe(svc);
    await expect.poll(() => { const answers = probe(client, svc); return !answers.includes("FAIL") && answers.includes(r1) && answers.includes(r2); }, { timeout: 90_000, intervals: [5000] }).toBe(true);
    expect(canConnect(client, ip[r1]![0]!, 80), "client → web on 80: drawn").toBe(true);
    expect(canConnect(client, ip[r1]![0]!, 443), "client → web on 443: not drawn").toBe(false);
    expect(canConnect(r1, ip[r2]![0]!, 80), "replica → replica: not drawn").toBe(false);
    expect(canConnect(r1, ip[client]![0]!, 22), "web → client: not drawn").toBe(false);
  });

  test("appears in the Fabric Console with its service address and passing port checks", async () => {
    // The worker decides health after its checks agree for a while; the page shows the state when it loads.
    await waitForStatus(app, (s) => s.items.filter((i) => i.module === "nginx-01").every((i) => i.health?.health === "healthy"), 3 * 60_000);
    await admin.getByRole("link", { name: "Applications", exact: true }).click();
    const row = admin.locator("main tr", { hasText: app }).first();
    await expect(row).toContainText("3 of 3 running");
    await expect(row).toContainText(svc);
    await admin.locator("main").getByText("Show details").first().click();
    await expect(admin.locator("main").getByText("Passing")).toHaveCount(2, { timeout: 60_000 });
    await admin.getByRole("link", { name: "Instances", exact: true }).click();
    await expect(admin.locator("main tr", { hasText: `${app}-` })).toHaveCount(3);
  });

  test("stops (asking again for the single client) and starts, and serves again", async () => {
    await stopAndStart(app);
    await expect.poll(() => !probe(`${app}-debian-01`, svc).includes("FAIL"), { timeout: 90_000, intervals: [5000] }).toBe(true);
  });
});

test.describe("data and names: a client with a volume, a web module and the resolver", () => {
  const app = "e2e-data";

  test("keeps the volume's data across a stop and start, and the client resolves the service name", async () => {
    await newApplication(app);
    const client = await place("debian", 0.25, 0.4);
    const web = await place("nginx", 0.7, 0.4);
    await place("resolver", 0.45, 0.75);
    await studio.getByRole("button", { name: /Tidy/ }).click();
    const out = await addHttpOutput(client);
    await connect(client, out, web, "http");
    await properties(client);
    await studio.getByTestId("add-volume").click();
    await expect(studio.getByTestId("volume-row")).toHaveCount(1);
    const path = await studio.getByTestId("volume-row").getByTestId("volume-path").inputValue();
    await studio.keyboard.press("Escape");
    await publish(app, "lab data and names");
    await deploy(app, 3);
    const s = await waitForStatus(app, (x) => x.items.length === 3 && x.items.every((i) => i.status === "Running") && x.services.length > 0);
    const svc = s.services.find((x) => x.module === web)!.address;

    const instance = `${app}-${client}`;
    inInstance(instance, `echo kept-by-apelora > ${path}/proof && sync`);
    // The restart also gives the client the resolver as its name server (it is set at boot).
    await stopAndStart(app);
    expect(inInstance(instance, `cat ${path}/proof`).trim()).toBe("kept-by-apelora");
    await expect.poll(() => inInstance(instance, `getent hosts ${web}.${app}.internal || true`).trim().split(/\s+/)[0], { timeout: 120_000, intervals: [5000] }).toBe(svc);
  });
});

test.describe("a virtual machine", () => {
  const app = "e2e-vm";

  test("is deployed from the canvas, runs and gets an address", async () => {
    await newApplication(app);
    const vm = await place("linux-base", 0.5, 0.45);
    await publish(app, "lab virtual machine");
    await deploy(app, 1);
    await waitForStatus(app, (x) => x.items.length === 1 && x.items[0]!.status === "Running", 10 * 60_000);
    const instance = `${app}-${vm}`;
    await expect.poll(() => ipv4ByInstance()[instance]?.length ?? 0, { timeout: 5 * 60_000, intervals: [10_000] }).toBeGreaterThan(0);
    expect(instanceType(instance)).toBe("virtual-machine");
  });
});
