import AxeBuilder from "@axe-core/playwright";
import { expect, test, type Page } from "@playwright/test";

import {
  ACME, ALEX, BILLING, CUSTOM_ROLE, GLOBEX, GROUP_ACME, GROUP_PLATFORM, ME, NOW, OFF, PROD, ROLES, SAM, SHOP, SVC_AUTO, SVC_INCUS,
  assignment, auditItem, base, error, items, previewOk, stub, uid, whoami, type Call,
} from "./identity-fixtures";

// The Identity & Access pages against a fully stubbed control plane: nothing reaches a real server.

test.beforeEach(async ({ page }) => { await page.route("**/api/**", (route) => route.abort()); });

const cluster = { json: { source: { provider: "mock", observedAt: NOW }, server: { clustered: true, serverVersion: null }, members: [] } };
const bodyOf = (calls: Call[], line: string) => calls.filter((c) => `${c.method} ${c.path}` === line).map((c) => c.body);
const countOf = (calls: Call[], line: string) => calls.filter((c) => `${c.method} ${c.path}` === line).length;
const confirmDialog = (page: Page) => page.locator("dialog[open]").getByTestId("confirm-dialog");
const confirm = (page: Page) => page.locator("dialog[open]").getByTestId("confirm-action").click();

test.describe("navigation", () => {
  test("offers only the pages whose permissions the caller holds somewhere", async ({ page }) => {
    await stub(page, { ...base(whoami(["audit.read", "user.view", "organization.view"], { platform: [] })), "GET /api/v1/cluster": cluster });
    await page.goto("/");
    const group = page.getByRole("navigation", { name: "Console" }).getByRole("group", { name: "Identity & Access" });
    await expect(group.getByRole("link")).toHaveText(["Users", "Groups", "Organizations", "Project memberships", "Roles & permissions", "Access policies", "Audit history"]);
    await expect(group.getByRole("link", { name: "Service identities" })).toHaveCount(0);
  });

  test("no group at all when the caller holds nothing relevant", async ({ page }) => {
    await stub(page, { ...base(whoami(["application.deploy"], { platform: [] })), "GET /api/v1/cluster": cluster });
    await page.goto("/");
    await expect(page.getByTestId("whoami")).toContainText("alex");
    await expect(page.getByTestId("nav-identity")).toHaveCount(0);
    await expect(page.getByRole("link", { name: "Instances" })).toBeVisible();
  });

  test("a legacy token keeps its existing views and gets the identity pages its capabilities allow", async ({ page }) => {
    const me = whoami(["user.view", "user.manage", "role.assign", "audit.read", "organization.view", "service_identity.manage"], { kind: "legacy-token", name: "ops-token" });
    await stub(page, { ...base(me), "GET /api/v1/cluster": cluster });
    await page.goto("/");
    const nav = page.getByRole("navigation", { name: "Console" });
    await expect(nav.getByRole("link", { name: "Membership", exact: true })).toBeVisible();
    await expect(nav.getByRole("link", { name: "Audit log" })).toBeVisible();
    await expect(nav.getByRole("link", { name: "Service identities" })).toBeVisible();
    await nav.getByRole("link", { name: "Users" }).click();
    await expect(page).toHaveURL(/#\/identity\/users$/);
    await expect(page.getByRole("heading", { level: 1, name: /^Users/ })).toBeVisible();
    await expect(nav.getByRole("link", { name: "Users" })).toHaveAttribute("aria-current", "page");
  });

  test("a page the caller does not hold falls back to the overview instead of an error", async ({ page }) => {
    await stub(page, { ...base(whoami(["audit.read"], { platform: [] })), "GET /api/v1/cluster": cluster });
    await page.goto("/#/identity/services");
    await expect(page.getByRole("heading", { level: 1, name: "Overview" })).toBeVisible();
    await expect(page.getByTestId("services")).toHaveCount(0);
  });
});

test.describe("users", () => {
  test("shows a loading state, then people with their state in words", async ({ page }) => {
    await stub(page, { ...base(), "GET /api/v1/identity/users": { json: { items: [ALEX, SAM, OFF] }, delayMs: 500 } });
    await page.goto("/#/identity/users");
    await expect(page.getByTestId("users-loading")).toBeVisible();
    const rows = page.getByTestId("user-row");
    await expect(rows).toHaveCount(3);
    await expect(rows.nth(0)).toContainText("Alex Admin");
    await expect(rows.nth(0)).toContainText("(you)");
    await expect(rows.nth(0)).toContainText("Active");
    await expect(rows.nth(2)).toContainText("Disabled");
    await expect(rows.nth(2)).toContainText("—");
    await expect(page.locator(".btn-primary")).toHaveCount(0);
  });

  test("empty and error states; a 403 shows the server's message and Retry recovers", async ({ page }) => {
    let denied = true;
    await stub(page, { ...base(), "GET /api/v1/identity/users": () => (denied ? error(403, "forbidden", "you may not do this (user.view)") : items([])) });
    await page.goto("/#/identity/users");
    await expect(page.getByTestId("users-error")).toHaveText("you may not do this (user.view)");
    denied = false;
    await page.getByRole("button", { name: "Try again" }).click();
    await expect(page.getByTestId("users-empty")).toHaveText("No one has signed in yet.");
  });

  test("disabling and enabling ask first, send the request and report server refusals", async ({ page }) => {
    const calls = await stub(page, {
      ...base(),
      [`POST /api/v1/identity/users/${SAM.id}/disable`]: error(409, "conflict", "someone else has to disable this account"),
      [`POST /api/v1/identity/users/${OFF.id}/enable`]: { status: 204 },
    });
    await page.goto("/#/identity/users");
    await expect(page.getByRole("button", { name: "Disable Alex Admin" })).toHaveCount(0);

    await page.getByRole("button", { name: "Disable Sam Support" }).click();
    await expect(confirmDialog(page)).toContainText("tokens stop working at once");
    await expect(page.locator("dialog[open]").getByRole("button", { name: "Cancel" })).toBeFocused();
    await confirm(page);
    await expect(page.getByTestId("users-action-error")).toHaveText("someone else has to disable this account");
    expect(countOf(calls, `POST /api/v1/identity/users/${SAM.id}/disable`)).toBe(1);

    await page.getByRole("button", { name: "Enable Olga Offboarded" }).click();
    await confirm(page);
    await expect(page.getByTestId("users-notice")).toContainText("Olga Offboarded is enabled again");
    expect(countOf(calls, `POST /api/v1/identity/users/${OFF.id}/enable`)).toBe(1);
  });

  test("a person without user.manage gets no disable or enable buttons", async ({ page }) => {
    await stub(page, base(whoami(["user.view", "role.assign"], { platform: [] })));
    await page.goto("/#/identity/users");
    await expect(page.getByTestId("user-row")).toHaveCount(3);
    await expect(page.getByRole("button", { name: /^(Disable|Enable) / })).toHaveCount(0);
    await expect(page.getByRole("button", { name: "Assign a role to Sam Support" })).toBeVisible();
    await expect(page.getByTestId("emergency-open")).toHaveCount(0);
  });

  test("looking someone up needs three characters, says when nobody matches and leads to the assignment dialog", async ({ page }) => {
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/users/lookup": (c) => (c.query.get("q") === "sam@example.org" ? items([SAM]) : items([])),
    });
    await page.goto("/#/identity/users");
    const lookup = page.getByTestId("users-lookup");
    await lookup.getByTestId("users-lookup-input").fill("sa");
    await expect(lookup.getByTestId("users-lookup-find")).toBeDisabled();
    await lookup.getByTestId("users-lookup-input").fill("nobody@example.org");
    await lookup.getByTestId("users-lookup-find").click();
    await expect(lookup.getByTestId("users-lookup-none")).toContainText("No one has signed in with the user name or e-mail “nobody@example.org”");
    await lookup.getByTestId("users-lookup-input").fill("sam@example.org");
    await lookup.getByTestId("users-lookup-input").press("Enter");
    await expect(lookup.getByTestId("users-lookup-result")).toContainText("Sam Support");
    await lookup.getByRole("button", { name: "Assign a role…: Sam Support" }).click();
    const dialog = page.getByRole("dialog", { name: "Assign a role" });
    await expect(dialog.getByTestId("assignment-person")).toContainText("Sam Support");
    expect(calls.filter((c) => c.path.endsWith("/lookup")).map((c) => c.query.get("q"))).toEqual(["nobody@example.org", "sam@example.org"]);
  });
});

test.describe("assignment dialog", () => {
  const open = async (page: Page, handlers: Parameters<typeof stub>[1]) => {
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/users/lookup": items([SAM]),
      "POST /api/v1/identity/assignments/preview": { json: previewOk() },
      ...handlers,
    });
    await page.goto("/#/identity/users");
    await page.getByRole("button", { name: "Assign a role to Sam Support" }).click();
    return { calls, dialog: page.getByRole("dialog", { name: "Assign a role" }) };
  };

  test("previews what changes, with every permission's origin, and saves only what was previewed", async ({ page }) => {
    const { calls, dialog } = await open(page, {
      "POST /api/v1/identity/assignments": (c) => ({ status: 201, json: assignment(1, (c.body as { principal: unknown }).principal, "Application Operator", { type: "organization", id: ACME.id }) }),
    });
    await expect(dialog.getByTestId("preview-waiting")).toContainText("Choose a role, where");
    await expect(dialog.getByTestId("assignment-save")).toBeDisabled();
    await dialog.getByTestId("assignment-role").selectOption({ label: "Application Operator" });
    await expect(dialog.getByTestId("assignment-role-note")).toContainText("Can be assigned at: Project, Application");
    await dialog.getByTestId("assignment-scope-type").selectOption("organization");
    await dialog.getByTestId("assignment-org").selectOption({ label: "Acme Corp" });

    const preview = dialog.getByTestId("assignment-preview");
    await expect(preview.getByTestId("preview-allowed")).toHaveText("You can save this assignment.");
    await expect(preview.getByTestId("preview-summary")).toContainText("Sam Support can do 1 thing at Organization Acme Corp now and would be able to do 2 afterwards; this adds 1.");
    await expect(preview.getByTestId("preview-added")).toContainText("application.deploy");
    await expect(preview.getByTestId("preview-inert")).toContainText("approval.request");
    await expect(preview.getByTestId("preview-warning")).toContainText("have no effect at this scope");
    const deploy = preview.locator('[data-testid=effective-permission][data-permission="application.deploy"]');
    await expect(deploy).toContainText("Application Operator at Organization Acme Corp");
    await expect(deploy).toContainText("This new assignment");
    const view = preview.getByTestId("preview-after").locator('[data-permission="application.view"]');
    await expect(view.getByTestId("origin")).toHaveCount(2);
    await expect(view.getByTestId("origin").first()).toContainText("Viewer at Organization Acme Corp via group Acme developers");
    await expect(dialog.getByTestId("assignment-save")).toBeEnabled();

    await dialog.getByTestId("assignment-save").click();
    await expect(page.getByTestId("users-notice")).toContainText("Application Operator assigned to Sam Support at Organization Acme Corp");
    await expect(dialog).toHaveCount(0);
    expect(bodyOf(calls, "POST /api/v1/identity/assignments")).toEqual([{
      principal: { type: "user", id: SAM.id }, role: { type: "builtin", key: "application-operator" }, scope: { type: "organization", id: ACME.id },
      conditions: { requireMfa: false, environments: null }, expiresAt: null, reason: null,
    }]);
  });

  test("Save is disabled and the server's reason shown when the administrator may not save it", async ({ page }) => {
    const { dialog } = await open(page, {
      "POST /api/v1/identity/assignments/preview": (c) => ((c.body as { role: { key: string } }).role.key === "platform-super-admin"
        ? { json: { ...previewOk(), allowed: false, error: "you cannot grant more than you hold: platform.emergency_access" } }
        : { json: previewOk() }),
    });
    await dialog.getByTestId("assignment-role").selectOption({ label: "Application Operator" });
    await dialog.getByTestId("assignment-scope-type").selectOption("organization");
    await dialog.getByTestId("assignment-org").selectOption({ label: "Acme Corp" });
    await expect(dialog.getByTestId("assignment-save")).toBeEnabled();
    await dialog.getByTestId("assignment-role").selectOption({ label: "Platform Super Admin" });
    await expect(dialog.getByTestId("preview-denied")).toContainText("You cannot save this assignment");
    await expect(dialog.getByTestId("preview-denied")).toContainText("You cannot grant more than you hold: platform.emergency_access");
    await expect(dialog.getByTestId("assignment-save")).toBeDisabled();
    await expect(dialog.getByTestId("assignment-role-note")).toContainText("Can be assigned at: Platform");
  });

  test("a server refusal of the preview itself (for example the scope) is shown and blocks saving", async ({ page }) => {
    const { dialog } = await open(page, { "POST /api/v1/identity/assignments/preview": error(403, "forbidden", "you may not do this (role.assign)") });
    await dialog.getByTestId("assignment-role").selectOption({ label: "Viewer" });
    await dialog.getByTestId("assignment-scope-type").selectOption("platform");
    await expect(dialog.getByTestId("preview-error")).toContainText("Not allowed: you may not do this (role.assign)");
    await expect(dialog.getByTestId("assignment-save")).toBeDisabled();
  });

  test("changes made together are previewed once (debounced) and the shown preview always matches the form", async ({ page }) => {
    const { calls, dialog } = await open(page, {});
    await dialog.getByTestId("assignment-role").selectOption({ label: "Viewer" });
    await dialog.getByTestId("assignment-scope-type").selectOption("platform");
    await expect(dialog.getByTestId("preview-allowed")).toBeVisible();
    const before = countOf(calls, "POST /api/v1/identity/assignments/preview");
    await dialog.getByTestId("assignment-mfa").check();
    await dialog.getByTestId("assignment-env-production").check();
    await dialog.getByTestId("assignment-env-staging").check();
    await expect(dialog.getByTestId("preview-allowed")).toBeVisible();
    await expect.poll(() => countOf(calls, "POST /api/v1/identity/assignments/preview")).toBe(before + 1);
    const last = bodyOf(calls, "POST /api/v1/identity/assignments/preview").at(-1) as { conditions: unknown };
    expect(last.conditions).toEqual({ requireMfa: true, environments: ["production", "staging"] });
  });

  test("an expiry in the past is refused before anything is sent", async ({ page }) => {
    const { dialog } = await open(page, {});
    await dialog.getByTestId("assignment-role").selectOption({ label: "Viewer" });
    await dialog.getByTestId("assignment-scope-type").selectOption("platform");
    await dialog.getByTestId("assignment-expires").fill("2020-01-01T10:00");
    await expect(dialog.getByText("The expiry must be in the future.")).toBeVisible();
    await expect(dialog.getByTestId("preview-waiting")).toContainText("Fix the expiry");
    await expect(dialog.getByTestId("assignment-save")).toBeDisabled();
  });

  test("Escape closes it and the form starts fresh next time", async ({ page }) => {
    const { dialog } = await open(page, {});
    await dialog.getByTestId("assignment-role").selectOption({ label: "Viewer" });
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await page.getByRole("button", { name: "Assign a role to Sam Support" }).click();
    await expect(page.getByRole("dialog", { name: "Assign a role" }).getByTestId("assignment-role")).toHaveValue("");
  });

  test("a group or an automation identity can be the principal", async ({ page }) => {
    const { calls, dialog } = await open(page, {});
    await dialog.getByText("A group", { exact: true }).click();
    await dialog.getByTestId("assignment-group").selectOption({ label: "Acme developers" });
    await dialog.getByTestId("assignment-role").selectOption({ label: "Viewer" });
    await dialog.getByTestId("assignment-scope-type").selectOption("organization");
    await dialog.getByTestId("assignment-org").selectOption({ label: "Acme Corp" });
    await expect(dialog.getByTestId("preview-allowed")).toBeVisible();
    expect((bodyOf(calls, "POST /api/v1/identity/assignments/preview").at(-1) as { principal: unknown }).principal).toEqual({ type: "group", id: GROUP_ACME.id });
    await dialog.getByText("An automation identity", { exact: true }).click();
    await expect(dialog.getByTestId("assignment-service")).toContainText("ci-pipeline");
    await expect(dialog.getByTestId("assignment-service")).not.toContainText("tenant-deployer");
  });
});

test.describe("emergency access", () => {
  test("is offered to platform administrators only, warns plainly and validates before sending", async ({ page }) => {
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/users/lookup": items([SAM]),
      "POST /api/v1/identity/emergency-access": { status: 201, json: assignment(2, { type: "user", id: SAM.id }, "Fabric Administrator", { type: "platform" }, { kind: "emergency" }) },
    });
    await page.goto("/#/identity/users");
    await page.getByTestId("emergency-open").click();
    const dialog = page.getByRole("dialog", { name: "Emergency access" });
    await expect(dialog.getByTestId("emergency-warning")).toContainText("audited and time-limited");
    await expect(dialog.getByTestId("emergency-submit")).toBeDisabled();
    await dialog.getByTestId("emergency-lookup-input").fill("sam@example.org");
    await dialog.getByTestId("emergency-lookup-find").click();
    await dialog.getByRole("button", { name: "Choose: Sam Support" }).click();
    await dialog.getByTestId("emergency-role").selectOption({ label: "Fabric Administrator" });
    await dialog.getByTestId("emergency-reason").fill("too short");
    await expect(dialog.getByTestId("emergency-submit")).toBeDisabled();
    await dialog.getByTestId("emergency-reason").fill("Primary on-call is unreachable during the outage");
    await dialog.getByTestId("emergency-minutes").fill("3");
    await expect(dialog.getByText("Emergency access lasts 5 to 240 minutes.")).toBeVisible();
    await dialog.getByTestId("emergency-minutes").fill("45");
    await expect(dialog.getByTestId("emergency-submit")).toBeDisabled();
    await dialog.getByTestId("emergency-ack").check();
    await dialog.getByTestId("emergency-submit").click();
    await expect(page.getByTestId("users-notice")).toContainText("Emergency access granted to Sam Support as Fabric Administrator for 45 minutes");
    expect(bodyOf(calls, "POST /api/v1/identity/emergency-access")).toEqual([{ userId: SAM.id, role: "fabric-administrator", reason: "Primary on-call is unreachable during the outage", minutes: 45 }]);
  });

  test("shows the server's refusal inside the dialog", async ({ page }) => {
    await stub(page, {
      ...base(),
      "GET /api/v1/identity/users/lookup": items([SAM]),
      "POST /api/v1/identity/emergency-access": error(409, "self_grant", "you cannot grant yourself emergency access; ask another administrator"),
    });
    await page.goto("/#/identity/users");
    await page.getByTestId("emergency-open").click();
    const dialog = page.getByRole("dialog", { name: "Emergency access" });
    await dialog.getByTestId("emergency-lookup-input").fill("sam@example.org");
    await dialog.getByTestId("emergency-lookup-find").click();
    await dialog.getByRole("button", { name: "Choose: Sam Support" }).click();
    await dialog.getByTestId("emergency-role").selectOption({ label: "Fabric Administrator" });
    await dialog.getByTestId("emergency-reason").fill("Primary on-call is unreachable");
    await dialog.getByTestId("emergency-ack").check();
    await dialog.getByTestId("emergency-submit").click();
    await expect(dialog.getByTestId("emergency-error")).toHaveText("you cannot grant yourself emergency access; ask another administrator");
  });
});

test.describe("groups", () => {
  test("lists groups; a provider-managed group explains why its members cannot be edited", async ({ page }) => {
    await stub(page, {
      ...base(),
      [`GET /api/v1/identity/groups/${GROUP_PLATFORM.id}/members`]: items([SAM]),
    });
    await page.goto("/#/identity/groups");
    const rows = page.getByTestId("group-row");
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0)).toContainText("Identity provider");
    await expect(rows.nth(1)).toContainText("Acme Corp");
    await expect(rows.nth(1)).toContainText("Managed here");
    await page.getByRole("button", { name: "Members of Platform admins" }).click();
    await expect(page.getByTestId("group-managed-note")).toContainText("come from the identity provider (group name apelora-admins)");
    await expect(page.getByTestId("group-member-row")).toHaveCount(1);
    await expect(page.getByTestId("group-add")).toHaveCount(0);
    await expect(page.getByRole("button", { name: /^Remove / })).toHaveCount(0);
  });

  test("adds a person by exact lookup and removes one after confirming", async ({ page }) => {
    let members = [ALEX];
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/users/lookup": items([SAM]),
      [`GET /api/v1/identity/groups/${GROUP_ACME.id}/members`]: () => items(members),
      [`PUT /api/v1/identity/groups/${GROUP_ACME.id}/members/${SAM.id}`]: () => { members = [ALEX, SAM]; return { status: 204 }; },
      [`DELETE /api/v1/identity/groups/${GROUP_ACME.id}/members/${ALEX.id}`]: () => { members = [SAM]; return { status: 204 }; },
    });
    await page.goto("/#/identity/groups");
    await page.getByRole("button", { name: "Members of Acme developers" }).click();
    await expect(page.getByTestId("group-member-row")).toHaveCount(1);
    const add = page.getByTestId("group-add");
    await add.getByTestId("group-add-input").fill("sam@example.org");
    await add.getByTestId("group-add-find").click();
    await add.getByRole("button", { name: "Add to group: Sam Support" }).click();
    await expect(page.getByTestId("group-notice")).toContainText("Sam Support is now a member of Acme developers");
    await expect(page.getByTestId("group-member-row")).toHaveCount(2);
    await page.getByRole("button", { name: "Remove Alex Admin from Acme developers" }).click();
    await expect(confirmDialog(page)).toContainText("ends on their next request");
    await confirm(page);
    await expect(page.getByTestId("group-member-row")).toHaveCount(1);
    expect(countOf(calls, `PUT /api/v1/identity/groups/${GROUP_ACME.id}/members/${SAM.id}`)).toBe(1);
    expect(countOf(calls, `DELETE /api/v1/identity/groups/${GROUP_ACME.id}/members/${ALEX.id}`)).toBe(1);
  });

  test("a platform administrator can map a provider group; an organization manager cannot and must pick an organization", async ({ page }) => {
    const calls = await stub(page, {
      ...base(),
      "POST /api/v1/identity/groups": (c) => ({ status: 201, json: { id: uid(45), description: "", ...(c.body as object) } }),
      [`GET /api/v1/identity/groups/${uid(45)}/members`]: items([]),
    });
    await page.goto("/#/identity/groups");
    await page.getByTestId("group-open").click();
    const dialog = page.getByRole("dialog", { name: "Create a group" });
    await dialog.getByTestId("group-name").fill("Ops on-call");
    await dialog.getByTestId("group-external").fill("ops-oncall");
    await dialog.getByTestId("group-submit").click();
    await expect(page.getByTestId("groups-notice")).toContainText("Group Ops on-call created");
    expect(bodyOf(calls, "POST /api/v1/identity/groups")).toEqual([{ name: "Ops on-call", description: "", organizationId: null, externalName: "ops-oncall" }]);

    // A caller who manages groups only inside organizations.
    await page.unrouteAll({ behavior: "ignoreErrors" });
    await page.route("**/api/**", (route) => route.abort());
    const calls2 = await stub(page, {
      ...base(whoami(["group.manage", "user.view", "organization.view"], { platform: [] })),
      "POST /api/v1/identity/groups": { status: 403, json: { error: { code: "forbidden", message: "you may not do this (group.manage)" } } },
    });
    await page.reload();
    await page.getByTestId("group-open").click();
    await expect(dialog.getByTestId("group-external")).toHaveCount(0);
    await expect(dialog.getByTestId("group-submit")).toBeDisabled();
    await dialog.getByTestId("group-name").fill("Team");
    await expect(dialog.getByTestId("group-submit")).toBeDisabled();
    await dialog.getByTestId("group-org").selectOption({ label: "Globex" });
    await dialog.getByTestId("group-submit").click();
    await expect(dialog.getByTestId("group-error")).toHaveText("Not allowed: you may not do this (group.manage)");
    expect(bodyOf(calls2, "POST /api/v1/identity/groups")).toEqual([{ name: "Team", description: "", organizationId: GLOBEX.id, externalName: null }]);
  });

  test("empty and error states", async ({ page }) => {
    await stub(page, { ...base(), "GET /api/v1/identity/groups": items([]) });
    await page.goto("/#/identity/groups");
    await expect(page.getByTestId("groups-empty")).toHaveText("No groups yet.");
    await page.unrouteAll({ behavior: "ignoreErrors" });
    await page.route("**/api/**", (route) => route.abort());
    await stub(page, { ...base(), "GET /api/v1/identity/groups": error(403, "forbidden", "you may not do this (user.view)") });
    await page.reload();
    await expect(page.getByTestId("groups-error")).toHaveText("you may not do this (user.view)");
  });
});

test.describe("organizations", () => {
  const envs = { json: { items: [PROD] } };
  test("drills from an organization to its projects and environments and says plainly that the Incus project is not created yet", async ({ page }) => {
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/projects": (c) => items(c.query.get("organization") === ACME.id ? [SHOP, BILLING] : []),
      [`GET /api/v1/identity/projects/${SHOP.id}/environments`]: envs,
    });
    await page.goto("/#/identity/organizations");
    await expect(page.getByTestId("organization-item")).toHaveCount(2);
    await page.getByRole("button", { name: "Expand organization Acme Corp" }).click();
    await expect(page.getByTestId("project-item")).toHaveCount(2);
    // (React's strict mode in development mounts effects twice, so the same request can repeat.)
    expect([...new Set(calls.filter((c) => c.path === "/api/v1/identity/projects").map((c) => c.query.get("organization")))]).toEqual([ACME.id]);
    await page.getByRole("button", { name: "Expand project Shop" }).click();
    const env = page.getByTestId("environment");
    await expect(env).toContainText("Production");
    await expect(env.getByTestId("env-incus-project")).toHaveText("acme-shop-prod");
    await expect(env.getByTestId("env-not-provisioned")).toHaveText("Incus project not created by Apelora yet — an administrator creates it with these settings.");
    const config = env.getByTestId("env-config");
    await expect(config.locator("pre")).toBeHidden();
    await config.locator("summary").click();
    await expect(config.locator("pre")).toContainText("restricted=true");
    await expect(config.locator("pre")).toContainText("restricted.containers.privilege=unprivileged");
    await page.getByRole("button", { name: "Collapse organization Acme Corp" }).click();
    await expect(page.getByTestId("project-item")).toHaveCount(0);
  });

  test("creating an organization, a project and an environment sends validated bodies", async ({ page }) => {
    const orgs: unknown[] = [ACME, GLOBEX];
    const projects: unknown[] = [];
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/organizations": () => items(orgs),
      "POST /api/v1/identity/organizations": (c) => { const o = { id: uid(12), createdAt: NOW, disabled: false, ...(c.body as object) }; orgs.push(o); return { status: 201, json: o }; },
      "GET /api/v1/identity/projects": () => items(projects),
      [`POST /api/v1/identity/organizations/${uid(12)}/projects`]: (c) => { const p = { id: uid(22), organizationId: uid(12), organizationSlug: "initech", createdAt: NOW, ...(c.body as object) }; projects.push(p); return { status: 201, json: p }; },
      [`GET /api/v1/identity/projects/${uid(22)}/environments`]: items([]),
      [`POST /api/v1/identity/projects/${uid(22)}/environments`]: (c) => ({ status: 201, json: { ...PROD, id: uid(31), projectId: uid(22), ...(c.body as object), incusProject: "initech-site-staging" } }),
    });
    await page.goto("/#/identity/organizations");
    await page.getByTestId("org-open").click();
    const dialog = page.getByRole("dialog", { name: "Create an organization" });
    await dialog.getByTestId("org-dialog-name").fill("Initech");
    await dialog.getByTestId("org-dialog-slug").fill("1bad");
    await expect(dialog.getByTestId("org-dialog-submit")).toBeDisabled();
    await dialog.getByTestId("org-dialog-slug").fill("initech");
    await dialog.getByTestId("org-dialog-submit").click();
    await expect(page.getByTestId("org-notice")).toContainText("Organization Initech created");

    await page.getByTestId("project-open").click();
    const pd = page.getByRole("dialog", { name: "Create a project in Initech" });
    await pd.getByTestId("project-dialog-name").fill("Site");
    await pd.getByTestId("project-dialog-slug").fill("site");
    await pd.getByTestId("project-dialog-submit").click();
    await expect(page.getByTestId("project-notice")).toContainText("Project Site created");

    await page.getByTestId("env-open").click();
    const ed = page.getByRole("dialog", { name: "Create an environment in Site" });
    await ed.getByTestId("env-dialog-name").fill("Staging");
    await ed.getByTestId("env-dialog-slug").fill("staging");
    await ed.getByTestId("env-kind").selectOption("staging");
    await ed.getByTestId("env-dialog-submit").click();
    await expect(page.getByTestId("env-notice")).toContainText("Environment Staging created. The Incus project initech-site-staging still has to be created by an administrator.");
    expect(bodyOf(calls, "POST /api/v1/identity/organizations")).toEqual([{ name: "Initech", slug: "initech" }]);
    expect(bodyOf(calls, `POST /api/v1/identity/organizations/${uid(12)}/projects`)).toEqual([{ name: "Site", slug: "site" }]);
    expect(bodyOf(calls, `POST /api/v1/identity/projects/${uid(22)}/environments`)).toEqual([{ name: "Staging", slug: "staging", kind: "staging", incusProject: null }]);
  });

  test("creating is hidden without the permission and server errors stay in the dialog", async ({ page }) => {
    await stub(page, {
      ...base(whoami(["organization.view", "project.view", "project.create"], { platform: [] })),
      [`POST /api/v1/identity/organizations/${ACME.id}/projects`]: error(409, "conflict", "a project with that identifier already exists"),
      "GET /api/v1/identity/projects": items([SHOP]),
    });
    await page.goto("/#/identity/organizations");
    await expect(page.getByTestId("org-open")).toHaveCount(0);
    await page.getByRole("button", { name: "Expand organization Acme Corp" }).click();
    await page.getByTestId("project-open").click();
    const pd = page.getByRole("dialog", { name: "Create a project in Acme Corp" });
    await pd.getByTestId("project-dialog-name").fill("Shop");
    await pd.getByTestId("project-dialog-slug").fill("shop");
    await pd.getByTestId("project-dialog-submit").click();
    await expect(pd.getByTestId("project-dialog-error")).toHaveText("a project with that identifier already exists");
  });

  test("empty and error states of the organization list and of a project list", async ({ page }) => {
    await stub(page, { ...base(), "GET /api/v1/identity/organizations": items([ACME]), "GET /api/v1/identity/projects": error(403, "forbidden", "you may not do this (project.view)") });
    await page.goto("/#/identity/organizations");
    await page.getByRole("button", { name: "Expand organization Acme Corp" }).click();
    await expect(page.getByTestId("projects-section-error")).toHaveText("you may not do this (project.view)");
    await page.unrouteAll({ behavior: "ignoreErrors" });
    await page.route("**/api/**", (route) => route.abort());
    await stub(page, { ...base(), "GET /api/v1/identity/organizations": items([]) });
    await page.reload();
    await expect(page.getByTestId("orgs-empty")).toHaveText("No organizations yet.");
  });
});

test.describe("project memberships", () => {
  const list = [
    assignment(1, { type: "user", id: ME }, "Platform Super Admin", { type: "platform" }),
    assignment(2, { type: "group", id: GROUP_ACME.id }, "Viewer", { type: "organization", id: ACME.id }, { conditions: { requireMfa: true, environments: ["production"] } }),
    assignment(3, { type: "user", id: SAM.id }, "Application Operator", { type: "project", id: SHOP.id }, { reason: "Covers the shop launch" }),
    assignment(4, { type: "service", id: SVC_AUTO.id }, "Viewer", { type: "project", id: BILLING.id }),
  ];

  test("groups assignments by scope and shows what applies to a chosen project, including what it inherits", async ({ page }) => {
    const calls = await stub(page, { ...base(), "GET /api/v1/identity/assignments": items(list) });
    await page.goto("/#/identity/memberships");
    await expect(page.getByTestId("membership-group")).toHaveCount(4);
    expect(calls.find((c) => c.path === "/api/v1/identity/assignments")!.query.get("includeRevoked")).toBe("false");
    const org = page.getByRole("region", { name: "Organization Acme Corp" });
    await expect(org).toContainText("Acme developers");
    await expect(org).toContainText("multi-factor sign-in required; only in production");

    await page.getByTestId("membership-mode").selectOption("project");
    await expect(page.getByTestId("memberships-choose")).toContainText("Choose a project");
    await page.getByTestId("membership-project").selectOption({ label: "acme / Shop" });
    const groups = page.getByTestId("membership-group");
    await expect(groups).toHaveCount(3);
    await expect(groups.nth(0)).toContainText("Platform");
    await expect(groups.nth(0)).toContainText("Inherited");
    await expect(groups.nth(1)).toContainText("Organization Acme Corp");
    await expect(groups.nth(1)).toContainText("Inherited");
    await expect(groups.nth(2)).toContainText("Project acme / Shop");
    await expect(groups.nth(2)).not.toContainText("Inherited");
    await expect(groups.nth(2)).toContainText("Sam Support");
    await expect(groups.nth(2)).toContainText("Application Operator");
    await expect(page.getByText("ci-pipeline")).toHaveCount(0);
  });

  test("revoking asks first and shows the server's refusal, for example the last super admin", async ({ page }) => {
    let revoked = false;
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/assignments": () => items(revoked ? list.filter((a) => a.id !== uid(103)) : list),
      [`DELETE /api/v1/identity/assignments/${uid(101)}`]: error(409, "last_super_admin", "the last platform super admin cannot be revoked; grant the role to someone else first"),
      [`DELETE /api/v1/identity/assignments/${uid(103)}`]: () => { revoked = true; return { status: 204 }; },
    });
    await page.goto("/#/identity/memberships");
    await page.getByRole("button", { name: "Revoke Platform Super Admin from Alex Admin" }).click();
    await expect(confirmDialog(page)).toContainText("They lose this role at Platform on their next request");
    await expect(page.locator("dialog[open]").getByRole("button", { name: "Cancel" })).toBeFocused();
    await page.locator("dialog[open]").getByRole("button", { name: "Cancel" }).click();
    expect(countOf(calls, `DELETE /api/v1/identity/assignments/${uid(101)}`)).toBe(0);
    await page.getByRole("button", { name: "Revoke Platform Super Admin from Alex Admin" }).click();
    await confirm(page);
    await expect(page.getByTestId("memberships-error")).toContainText("the last platform super admin cannot be revoked");
    await page.getByRole("button", { name: "Revoke Application Operator from Sam Support" }).click();
    await confirm(page);
    await expect(page.getByTestId("memberships-notice")).toContainText("Application Operator was revoked from Sam Support at Project acme / Shop");
    await expect(page.getByTestId("assignment-row")).toHaveCount(3);
  });

  test("the assignment dialog starts at the chosen scope", async ({ page }) => {
    await stub(page, { ...base(), "GET /api/v1/identity/assignments": items(list) });
    await page.goto("/#/identity/memberships");
    await page.getByTestId("membership-mode").selectOption("project");
    await page.getByTestId("membership-project").selectOption({ label: "acme / Shop" });
    await page.getByTestId("assign-open").click();
    const dialog = page.getByRole("dialog", { name: "Assign a role" });
    await expect(dialog.getByTestId("assignment-scope-type")).toHaveValue("project");
    await expect(dialog.getByTestId("assignment-project")).toHaveValue(SHOP.id);
  });

  test("empty and error states", async ({ page }) => {
    await stub(page, { ...base(), "GET /api/v1/identity/assignments": items([]) });
    await page.goto("/#/identity/memberships");
    await expect(page.getByTestId("memberships-empty")).toHaveText("No role assignments in the scopes you can see.");
    await page.unrouteAll({ behavior: "ignoreErrors" });
    await page.route("**/api/**", (route) => route.abort());
    await stub(page, { ...base(), "GET /api/v1/identity/assignments": error(403, "forbidden", "you may not do this (user.view)") });
    await page.reload();
    await expect(page.getByTestId("memberships-error")).toHaveText("you may not do this (user.view)");
  });
});

test.describe("roles and permissions", () => {
  test("lists the nine predefined roles and the custom ones with their permissions grouped by class", async ({ page }) => {
    await stub(page, base());
    await page.goto("/#/identity/roles");
    const builtin = page.getByTestId("builtin-roles");
    await expect(builtin.getByTestId("role-card")).toHaveCount(9);
    await expect(builtin).toContainText("Predefined roles (9)");
    const hw = builtin.locator('[data-role="builtin:hardware-operator"]');
    await expect(hw.getByTestId("role-scopes")).toHaveText("Can be assigned at: Platform, Hardware pool.");
    await hw.locator("summary").click();
    await expect(hw).toContainText("Infrastructure (2)");
    const power = hw.getByTestId("role-permission").filter({ hasText: "hardware.power" });
    await expect(power).toContainText("Needs approval");
    await expect(power).toContainText("narrowest scope: Hardware pool");
    const custom = page.getByTestId("custom-roles").getByTestId("role-card");
    await expect(custom).toHaveCount(1);
    await expect(custom).toContainText("Release manager");
    await expect(custom).toContainText("Custom");
  });

  test("the permission catalogue can be filtered by class", async ({ page }) => {
    await stub(page, base());
    await page.goto("/#/identity/roles");
    const rows = page.getByTestId("catalogue-row");
    await expect(rows).toHaveCount(24);
    await page.getByTestId("catalogue-class").selectOption("security");
    await expect(rows).toHaveCount(7);
    await expect(rows.first()).toContainText("Security");
    await page.getByTestId("catalogue-class").selectOption("infrastructure");
    await expect(rows.filter({ hasText: "Needs approval" })).toHaveCount(2);
  });

  test("creating a custom role shows where it can be assigned as permissions are ticked, and sends the explicit list", async ({ page }) => {
    const calls = await stub(page, {
      ...base(),
      "POST /api/v1/identity/roles": (c) => ({ status: 201, json: { id: `custom:${uid(61)}`, reference: { type: "custom", id: uid(61) }, builtin: false, scopes: ["platform", "organization", "project", "application"], ...(c.body as object) } }),
    });
    await page.goto("/#/identity/roles");
    await page.getByTestId("role-open").click();
    const dialog = page.getByRole("dialog", { name: "Create a custom role" });
    await expect(dialog.getByTestId("role-summary")).toContainText("Tick at least one permission");
    await expect(dialog.getByTestId("role-submit")).toBeDisabled();
    await dialog.getByTestId("role-name").fill("Platform Super Admin");
    await expect(dialog.getByText("A role with that name already exists.")).toBeVisible();
    await dialog.getByTestId("role-name").fill("Shop viewer");
    await dialog.getByTestId("perm-application.view").check();
    await expect(dialog.getByTestId("role-levels")).toHaveText("Platform, Organization, Project, Application");
    await dialog.getByTestId("perm-role.assign").check();
    // Down to the narrowest scope among the permissions: application.view reaches applications.
    await expect(dialog.getByTestId("role-levels")).toHaveText("Platform, Organization, Project, Application");
    await dialog.getByTestId("perm-hardware.view").check();
    await expect(dialog.getByTestId("role-levels")).toHaveText("Platform");
    await dialog.getByTestId("perm-hardware.view").uncheck();
    await dialog.getByTestId("perm-application.view").uncheck();
    await expect(dialog.getByTestId("role-levels")).toHaveText("Platform, Organization, Project");
    await dialog.getByTestId("perm-application.view").check();
    await dialog.getByTestId("role-filter").fill("deploy");
    await expect(dialog.getByTestId("perm-application.deploy")).toBeVisible();
    await expect(dialog.getByTestId("perm-application.view")).toHaveCount(0);
    await dialog.getByTestId("role-filter").fill("");
    await dialog.getByTestId("role-submit").click();
    await expect(page.getByTestId("roles-notice")).toContainText("The role Shop viewer was created. It can be assigned at: Platform, Organization, Project, Application.");
    expect(bodyOf(calls, "POST /api/v1/identity/roles")).toEqual([{ name: "Shop viewer", description: "", permissions: ["application.view", "role.assign"] }]);
  });

  test("permissions the caller does not hold cannot be ticked", async ({ page }) => {
    await stub(page, base(whoami(["role.manage", "application.view", "role.assign"], { platform: ["role.manage", "application.view"] })));
    await page.goto("/#/identity/roles");
    await page.getByTestId("role-open").click();
    const dialog = page.getByRole("dialog", { name: "Create a custom role" });
    await expect(dialog.getByTestId("perm-application.view")).toBeEnabled();
    await expect(dialog.getByTestId("perm-role.assign")).toBeDisabled();
    await expect(dialog.getByText("You do not hold this permission, so you cannot include it.").first()).toBeVisible();
  });

  test("deleting a custom role asks first and shows the server's refusal when it is in use", async ({ page }) => {
    const calls = await stub(page, { ...base(), [`DELETE /api/v1/identity/roles/${uid(60)}`]: error(409, "role_in_use", "that role is still assigned") });
    await page.goto("/#/identity/roles");
    await page.getByRole("button", { name: "Delete the role Release manager" }).click();
    await expect(confirmDialog(page)).toContainText("cannot be deleted");
    await confirm(page);
    await expect(page.getByTestId("roles-error")).toHaveText("that role is still assigned");
    expect(countOf(calls, `DELETE /api/v1/identity/roles/${uid(60)}`)).toBe(1);
  });

  test("without role.manage there is no create or delete, and a failed load can be retried", async ({ page }) => {
    let fail = true;
    await stub(page, { ...base(whoami(["user.view"], { platform: [] })), "GET /api/v1/identity/roles": () => (fail ? error(403, "forbidden", "you may not do this (role.assign)") : items([...ROLES, CUSTOM_ROLE])) });
    await page.goto("/#/identity/roles");
    await expect(page.getByTestId("roles-error")).toHaveText("you may not do this (role.assign)");
    fail = false;
    await page.getByRole("button", { name: "Try again" }).click();
    await expect(page.getByTestId("role-card")).toHaveCount(10);
    await expect(page.getByTestId("role-open")).toHaveCount(0);
    await expect(page.getByRole("button", { name: /^Delete the role/ })).toHaveCount(0);
  });
});

test.describe("service identities", () => {
  const TOKEN = "aps_" + "1f".repeat(32);

  test("lists kinds, allowed operations and credential names; says Apelora never stores secrets", async ({ page }) => {
    await stub(page, base());
    await page.goto("/#/identity/services");
    const rows = page.getByTestId("service-row");
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0)).toContainText("Automation");
    await expect(rows.nth(0)).toContainText("API token (hashed)");
    await expect(rows.nth(1)).toContainText("Workload");
    await expect(rows.nth(1).getByTestId("credential-ref")).toHaveText("incus/deploy-client");
    await rows.nth(1).locator("summary").click();
    await expect(rows.nth(1)).toContainText("instance.create");
    await expect(page.getByTestId("no-secrets-note")).toContainText("Apelora never stores secrets");
  });

  test("an automation token is shown once in a dialog and is gone from the page and the browser after closing", async ({ page }) => {
    const calls = await stub(page, {
      ...base(),
      "POST /api/v1/identity/service-identities": { status: 201, json: { token: TOKEN, identity: { ...SVC_AUTO, id: uid(52), name: "deploy-bot" } } },
    });
    await page.goto("/#/identity/services");
    await page.getByTestId("service-open").click();
    const dialog = page.getByRole("dialog", { name: "Create a service identity" });
    await dialog.getByTestId("service-name").fill("deploy-bot");
    await dialog.getByTestId("service-days").fill("30");
    await dialog.getByTestId("service-submit").click();

    const secret = page.getByRole("dialog", { name: "API token for deploy-bot" });
    await expect(secret.getByTestId("secret-value")).toHaveValue(TOKEN);
    await expect(secret).toContainText("Shown only once");
    expect(bodyOf(calls, "POST /api/v1/identity/service-identities")).toEqual([{ name: "deploy-bot", kind: "automation", credentialRef: null, expiresInDays: 30, restrictions: {} }]);

    await secret.getByTestId("secret-close").click();
    await expect(secret).toHaveCount(0);
    await expect(page.getByTestId("secret-value")).toHaveCount(0);
    expect(await page.content()).not.toContain(TOKEN);
    await expect(page.getByTestId("services-notice")).not.toContainText(TOKEN);
    const stored = await page.evaluate(() => JSON.stringify([{ ...localStorage }, { ...sessionStorage }, location.href]));
    expect(stored).not.toContain(TOKEN);
    await page.reload();
    expect(await page.content()).not.toContain(TOKEN);
  });

  test("Escape also discards the token", async ({ page }) => {
    await stub(page, { ...base(), "POST /api/v1/identity/service-identities": { status: 201, json: { token: TOKEN, identity: { ...SVC_AUTO, id: uid(52), name: "deploy-bot" } } } });
    await page.goto("/#/identity/services");
    await page.getByTestId("service-open").click();
    const dialog = page.getByRole("dialog", { name: "Create a service identity" });
    await dialog.getByTestId("service-name").fill("deploy-bot");
    await dialog.getByTestId("service-submit").click();
    await expect(page.getByTestId("secret-value")).toHaveValue(TOKEN);
    await page.keyboard.press("Escape");
    await expect(page.getByTestId("secret-value")).toHaveCount(0);
    expect(await page.content()).not.toContain(TOKEN);
  });

  test("an outbound identity records only the credential's name, which is validated", async ({ page }) => {
    const calls = await stub(page, {
      ...base(),
      "POST /api/v1/identity/service-identities": (c) => ({ status: 201, json: { token: null, identity: { ...SVC_INCUS, id: uid(53), ...(c.body as object), backend: "incus", operations: [], apiAccess: false } } }),
    });
    await page.goto("/#/identity/services");
    await page.getByTestId("service-open").click();
    const dialog = page.getByRole("dialog", { name: "Create a service identity" });
    await dialog.getByTestId("service-name").fill("net-manager");
    await dialog.getByTestId("service-kind").selectOption("network");
    await expect(dialog.getByTestId("service-kind-note")).toContainText("manages networks, ACLs and load balancers");
    await expect(dialog.getByTestId("service-days")).toHaveCount(0);
    await dialog.getByTestId("service-credential").fill("my secret value");
    await expect(dialog.getByTestId("service-submit")).toBeDisabled();
    await dialog.getByTestId("service-credential").fill("incus/network-client");
    await dialog.getByTestId("service-restrictions").fill("[1]");
    await expect(dialog.getByText("Restrictions must be a JSON object")).toBeVisible();
    await dialog.getByTestId("service-restrictions").fill('{"projects": ["acme-prod"]}');
    await dialog.getByTestId("service-submit").click();
    await expect(page.getByTestId("services-notice")).toContainText("Apelora recorded only the credential name incus/network-client, never the secret");
    await expect(page.getByTestId("secret-dialog")).toBeHidden();
    expect(bodyOf(calls, "POST /api/v1/identity/service-identities")).toEqual([{ name: "net-manager", kind: "network", credentialRef: "incus/network-client", expiresInDays: null, restrictions: { projects: ["acme-prod"] } }]);
  });

  test("disabling and enabling ask first; a 403 from the server is shown", async ({ page }) => {
    let disabled = false;
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/service-identities": () => items([{ ...SVC_AUTO, disabled }, SVC_INCUS]),
      [`POST /api/v1/identity/service-identities/${SVC_AUTO.id}/disable`]: () => { disabled = true; return { status: 204 }; },
      [`POST /api/v1/identity/service-identities/${SVC_AUTO.id}/enable`]: error(403, "forbidden", "you may not do this (service_identity.manage)"),
    });
    await page.goto("/#/identity/services");
    await page.getByRole("button", { name: "Disable ci-pipeline" }).click();
    await expect(confirmDialog(page)).toContainText("stops working at once");
    await confirm(page);
    await expect(page.getByTestId("service-row").first()).toContainText("Disabled");
    await page.getByRole("button", { name: "Enable ci-pipeline" }).click();
    await confirm(page);
    await expect(page.getByTestId("services-action-error")).toHaveText("Not allowed: you may not do this (service_identity.manage)");
    expect(countOf(calls, `POST /api/v1/identity/service-identities/${SVC_AUTO.id}/disable`)).toBe(1);
  });

  test("empty and error states", async ({ page }) => {
    await stub(page, { ...base(), "GET /api/v1/identity/service-identities": items([]) });
    await page.goto("/#/identity/services");
    await expect(page.getByTestId("services-empty")).toHaveText("No service identities yet.");
    await page.unrouteAll({ behavior: "ignoreErrors" });
    await page.route("**/api/**", (route) => route.abort());
    await stub(page, { ...base(), "GET /api/v1/identity/service-identities": error(403, "forbidden", "you may not do this (service_identity.manage)") });
    await page.reload();
    await expect(page.getByTestId("services-error")).toHaveText("you may not do this (service_identity.manage)");
  });
});

test.describe("sign-in settings", () => {
  const ENV = { id: null, source: "environment", name: "Example Corp", issuer: "https://login.example.org/realms/apelora", audiences: ["apelora-api"], clientId: "apelora-console", scopes: ["openid", "profile", "email"], groupsClaim: "groups", usernameClaim: "preferred_username", mfaAmr: ["mfa"], mfaAcr: [], authorizedParties: [], enabled: true, reachable: true, error: null, updatedBy: null };
  const MAIN = { ...ENV, id: uid(70), source: "console", name: "Globex staff", issuer: "https://id.globex.example/oidc", clientId: null, updatedBy: `user:${ME}` };
  const settings = (providers: unknown[], apiTokens = { enabled: true, lockedByEnvironment: false }) => ({ json: { providers, apiTokens } });

  test("lists providers in words; the environment's is read-only", async ({ page }) => {
    await stub(page, { ...base(), "GET /api/v1/identity/sign-in": settings([ENV, { ...MAIN, reachable: false, error: "the provider's keys could not be read" }]) });
    await page.goto("/#/identity/sign-in");
    const rows = page.getByTestId("provider-row");
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0)).toContainText("set by environment variables");
    await expect(rows.nth(0)).toContainText("Answers");
    await expect(rows.nth(0).getByRole("button")).toHaveCount(0);
    await expect(rows.nth(1)).toContainText("Tokens only, no button");
    await expect(rows.nth(1)).toContainText("Not reachable");
    await expect(rows.nth(1)).toContainText("the provider's keys could not be read");
    await expect(rows.nth(1).getByRole("button", { name: "Edit Globex staff" })).toBeVisible();
  });

  test("without platform.security.manage the page is not offered", async ({ page }) => {
    await stub(page, { ...base(whoami(["user.view"])), "GET /api/v1/cluster": cluster });
    await page.goto("/#/identity/sign-in");
    await expect(page.getByRole("heading", { level: 1, name: "Overview" })).toBeVisible();
    await expect(page.getByRole("link", { name: "Sign-in" })).toHaveCount(0);
  });

  test("adding a provider checks the connection, validates the issuer and sends the cleaned-up fields", async ({ page }) => {
    let list: unknown[] = [];
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/sign-in": () => settings(list),
      "POST /api/v1/identity/sign-in/check": { json: { authorizationEndpoint: "https://id.globex.example/oidc/authorize", tokenEndpoint: "https://id.globex.example/oidc/token", jwksUri: "https://id.globex.example/oidc/keys", keys: 2 } },
      "POST /api/v1/identity/sign-in/providers": () => { list = [MAIN]; return { status: 201, json: MAIN }; },
    });
    await page.goto("/#/identity/sign-in");
    await expect(page.getByTestId("no-providers")).toBeVisible();
    await page.getByTestId("provider-add").click();
    const dialog = page.getByTestId("provider-dialog");
    await dialog.getByTestId("provider-name").fill("Globex staff");
    await dialog.getByTestId("provider-issuer").fill("http://id.globex.example/oidc");
    await expect(dialog.getByText("Use https://")).toBeVisible();
    await expect(dialog.getByTestId("provider-check")).toBeDisabled();
    await expect(dialog.getByTestId("provider-save")).toBeDisabled();
    await dialog.getByTestId("provider-issuer").fill("https://id.globex.example/oidc");
    await dialog.getByTestId("provider-check").click();
    await expect(dialog.getByTestId("provider-check-ok")).toContainText("2 signing keys");
    await expect(dialog.getByTestId("provider-check-ok")).toContainText("id.globex.example");
    await expect(dialog.getByTestId("provider-save")).toBeDisabled();
    await dialog.getByTestId("provider-audiences").fill("apelora-api, apelora-cli");
    await dialog.getByTestId("provider-client").fill("apelora-console");
    await dialog.getByTestId("provider-save").click();
    await expect(dialog).toBeHidden();
    await expect(page.getByTestId("provider-row")).toHaveCount(1);
    expect(bodyOf(calls, "POST /api/v1/identity/sign-in/check")).toEqual([{ issuer: "https://id.globex.example/oidc" }]);
    expect(bodyOf(calls, "POST /api/v1/identity/sign-in/providers")).toEqual([{
      name: "Globex staff", issuer: "https://id.globex.example/oidc", audiences: ["apelora-api", "apelora-cli"], clientId: "apelora-console",
      scopes: ["openid", "profile", "email"], groupsClaim: "groups", usernameClaim: "preferred_username", mfaAmr: ["mfa", "otp", "hwk"], mfaAcr: [], authorizedParties: [], enabled: true,
    }]);
  });

  test("a provider that does not answer, and a server refusal, are shown inside the dialog", async ({ page }) => {
    await stub(page, {
      ...base(),
      "GET /api/v1/identity/sign-in": settings([]),
      "POST /api/v1/identity/sign-in/check": error(422, "invalid", "discovery failed: connection refused"),
      "POST /api/v1/identity/sign-in/providers": error(409, "conflict", "a provider with this issuer exists"),
    });
    await page.goto("/#/identity/sign-in");
    await page.getByTestId("provider-add").click();
    const dialog = page.getByTestId("provider-dialog");
    await dialog.getByTestId("provider-name").fill("Again");
    await dialog.getByTestId("provider-issuer").fill("https://id.globex.example/oidc");
    await dialog.getByTestId("provider-check").click();
    await expect(dialog.getByTestId("provider-check-error")).toContainText("connection refused");
    await dialog.getByTestId("provider-audiences").fill("apelora-api");
    await dialog.getByTestId("provider-save").click();
    await expect(dialog.getByTestId("provider-save-error")).toHaveText("a provider with this issuer exists");
    await expect(dialog).toBeVisible();
  });

  test("editing keeps the fields, turning off sends enabled false, removing needs the name typed", async ({ page }) => {
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/sign-in": settings([MAIN]),
      [`PUT /api/v1/identity/sign-in/providers/${MAIN.id}`]: { json: MAIN },
      [`DELETE /api/v1/identity/sign-in/providers/${MAIN.id}`]: { status: 204 },
    });
    await page.goto("/#/identity/sign-in");
    await page.getByRole("button", { name: "Edit Globex staff" }).click();
    const dialog = page.getByTestId("provider-dialog");
    await expect(dialog.getByTestId("provider-issuer")).toHaveValue(MAIN.issuer);
    await expect(dialog.getByTestId("provider-audiences")).toHaveValue("apelora-api");
    await dialog.getByTestId("provider-name").fill("Globex people");
    await dialog.getByTestId("provider-save").click();
    await expect(dialog).toBeHidden();
    await page.getByRole("button", { name: "Turn off Globex staff" }).click();
    const puts = bodyOf(calls, `PUT /api/v1/identity/sign-in/providers/${MAIN.id}`) as Array<Record<string, unknown>>;
    await expect.poll(() => puts.length).toBe(2);
    expect(puts[0]).toMatchObject({ name: "Globex people", issuer: MAIN.issuer, enabled: true });
    expect(puts[1]).toMatchObject({ name: "Globex staff", enabled: false });

    await page.getByRole("button", { name: "Remove Globex staff" }).click();
    await expect(confirmDialog(page)).toContainText("accounts and roles in Apelora stay");
    await expect(page.locator("dialog[open]").getByTestId("confirm-action")).toBeDisabled();
    await page.locator("dialog[open]").getByRole("textbox").fill("Globex staff");
    await confirm(page);
    await expect.poll(() => countOf(calls, `DELETE /api/v1/identity/sign-in/providers/${MAIN.id}`)).toBe(1);
    expect(bodyOf(calls, `DELETE /api/v1/identity/sign-in/providers/${MAIN.id}`)).toEqual([{ confirm: "Globex staff" }]);
  });

  test("turning API tokens off asks first and shows the lockout refusal", async ({ page }) => {
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/sign-in": settings([MAIN]),
      "PUT /api/v1/identity/sign-in/api-tokens": error(409, "conflict", "sign in with an identity provider first: an API token cannot turn API tokens off"),
    });
    await page.goto("/#/identity/sign-in");
    await page.getByTestId("api-tokens-toggle").click();
    await expect(confirmDialog(page)).toContainText("Every API token stops working at once");
    await confirm(page);
    await expect(page.getByTestId("sign-in-settings-error")).toContainText("an API token cannot turn API tokens off");
    expect(bodyOf(calls, "PUT /api/v1/identity/sign-in/api-tokens")).toEqual([{ enabled: false }]);
  });

  test("API tokens turned off by the environment offer no button", async ({ page }) => {
    await stub(page, { ...base(), "GET /api/v1/identity/sign-in": settings([MAIN], { enabled: false, lockedByEnvironment: true }) });
    await page.goto("/#/identity/sign-in");
    await expect(page.getByTestId("api-tokens-setting")).toContainText("APELORA_LEGACY_TOKENS=disabled");
    await expect(page.getByTestId("api-tokens-toggle")).toHaveCount(0);
  });
});

test.describe("access policies and approvals", () => {
  const mfa = { id: uid(80), name: "Production needs MFA", kind: "mfa", parameters: { environments: ["production"], infrastructure: true }, enabled: true, updatedBy: `user:${ME}`, updatedAt: NOW };
  const approvalPolicy = { id: uid(81), name: "Evacuations", kind: "approval", parameters: { actions: ["application.deploy"] }, enabled: false, updatedBy: "development", updatedAt: NOW };

  test("lists policies in words; creates and deletes them with validated forms", async ({ page }) => {
    let list = [mfa, approvalPolicy];
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/policies": () => items(list),
      "PUT /api/v1/identity/policies": (c) => { const b = c.body as { name: string; kind: string; parameters: object; enabled: boolean }; const p = { id: uid(82 + list.length), updatedBy: `user:${ME}`, updatedAt: NOW, ...b }; list = [...list, p]; return { json: p }; },
      [`DELETE /api/v1/identity/policies/${uid(81)}`]: () => { list = list.filter((p) => p.id !== uid(81)); return { status: 204 }; },
    });
    await page.goto("/#/identity/policies");
    const rows = page.getByTestId("policy-row");
    await expect(rows).toHaveCount(2);
    await expect(rows.nth(0)).toContainText("Needs multi-factor sign-in for actions in production environments and infrastructure actions.");
    await expect(rows.nth(1)).toContainText("Extra actions that need approval: application.deploy.");
    await expect(rows.nth(1)).toContainText("Disabled");

    await page.getByTestId("policy-open").click();
    const dialog = page.getByRole("dialog", { name: "Create an access policy" });
    await expect(dialog.getByTestId("policy-submit")).toBeDisabled();
    await dialog.getByTestId("policy-name").fill("Staging needs MFA");
    await expect(dialog.getByText("Choose at least one environment kind or infrastructure.")).toBeVisible();
    await dialog.getByTestId("policy-env-staging").check();
    await dialog.getByTestId("policy-submit").click();
    await expect(page.getByTestId("policies-notice")).toContainText("The policy Staging needs MFA was saved.");
    await expect(rows).toHaveCount(3);

    await page.getByTestId("policy-open").click();
    await dialog.getByTestId("policy-kind").selectOption("approval");
    await dialog.getByTestId("policy-name").fill("Deploys");
    await expect(dialog.getByTestId("policy-action-hardware.power")).toHaveCount(0);
    await dialog.getByTestId("policy-action-application.deploy").check();
    await dialog.getByTestId("policy-enabled").uncheck();
    await dialog.getByTestId("policy-submit").click();
    await expect(rows).toHaveCount(4);
    expect(bodyOf(calls, "PUT /api/v1/identity/policies")).toEqual([
      { name: "Staging needs MFA", kind: "mfa", enabled: true, parameters: { environments: ["staging"], infrastructure: false } },
      { name: "Deploys", kind: "approval", enabled: false, parameters: { actions: ["application.deploy"] } },
    ]);

    await page.getByRole("button", { name: "Delete Evacuations" }).click();
    await expect(confirmDialog(page)).toContainText("stops applying at once");
    await confirm(page);
    await expect(page.getByTestId("policies-notice")).toContainText("The policy Evacuations was deleted.");
    await expect(rows).toHaveCount(3);
  });

  test("editing keeps the name and kind and sends the changed parameters", async ({ page }) => {
    const calls = await stub(page, { ...base(), "GET /api/v1/identity/policies": items([mfa]), "PUT /api/v1/identity/policies": (c) => ({ json: { ...mfa, ...(c.body as object) } }) });
    await page.goto("/#/identity/policies");
    await page.getByRole("button", { name: "Edit Production needs MFA" }).click();
    const dialog = page.getByRole("dialog", { name: "Edit Production needs MFA" });
    await expect(dialog.getByTestId("policy-name")).toHaveAttribute("readonly", "");
    await expect(dialog.getByTestId("policy-kind")).toBeDisabled();
    await expect(dialog.getByTestId("policy-env-production")).toBeChecked();
    await dialog.getByTestId("policy-env-staging").check();
    await dialog.getByTestId("policy-submit").click();
    await expect(page.getByTestId("policies-notice")).toBeVisible();
    expect(bodyOf(calls, "PUT /api/v1/identity/policies")).toEqual([{ name: "Production needs MFA", kind: "mfa", enabled: true, parameters: { environments: ["production", "staging"], infrastructure: true } }]);
  });

  test("a reader of policies sees no way to change them; empty and error states", async ({ page }) => {
    await stub(page, { ...base(whoami(["audit.read"], { platform: [] })), "GET /api/v1/identity/policies": items([mfa]) });
    await page.goto("/#/identity/policies");
    await expect(page.getByTestId("policy-row")).toHaveCount(1);
    await expect(page.getByTestId("policy-open")).toHaveCount(0);
    await expect(page.getByRole("button", { name: /^(Edit|Delete) / })).toHaveCount(0);
    await expect(page.getByTestId("tab-approvals")).toHaveCount(0);
    await page.unrouteAll({ behavior: "ignoreErrors" });
    await page.route("**/api/**", (route) => route.abort());
    await stub(page, { ...base(), "GET /api/v1/identity/policies": error(403, "forbidden", "you may not do this (access_policy.manage)") });
    await page.reload();
    await expect(page.getByTestId("policies-error")).toHaveText("you may not do this (access_policy.manage)");
  });

  const ONE = (n: number, over: Record<string, unknown>) => ({ id: uid(90 + n), action: "hardware.power", target: `machine:rack1-0${n}`, requester: `user:${SAM.id}`, approver: null, state: "pending", reason: "Replace the fan", createdAt: NOW, expiresAt: "2026-10-20T09:30:00Z", ...over });

  test("approvals: decide, see self-approval refused plainly, cancel your own, copy the header for an approved request", async ({ page }) => {
    await page.context().grantPermissions(["clipboard-read", "clipboard-write"]);
    const list = [
      ONE(1, {}),
      ONE(2, { requester: `user:${ME}` }),
      ONE(3, { requester: `user:${ME}`, state: "approved", approver: `user:${SAM.id}` }),
      ONE(4, { state: "rejected", approver: `user:${ME}` }),
    ];
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/approvals": () => items(list),
      [`POST /api/v1/approvals/${uid(91)}/approve`]: { json: ONE(1, { state: "approved" }) },
      [`POST /api/v1/approvals/${uid(92)}/approve`]: error(403, "self_approval", "you cannot approve your own request"),
      [`POST /api/v1/approvals/${uid(92)}/cancel`]: { json: ONE(2, { state: "cancelled" }) },
      [`POST /api/v1/approvals/${uid(91)}/reject`]: error(409, "approval_invalid", "that request is not pending"),
    });
    await page.goto("/#/identity/policies");
    await page.getByTestId("tab-approvals").click();
    const rows = page.getByTestId("approval-row");
    await expect(rows).toHaveCount(4);
    await expect(rows.nth(0)).toContainText("Pending");
    await expect(rows.nth(0)).toContainText("hardware.power on machine:rack1-01");
    await expect(rows.nth(1).getByText("You")).toBeVisible();

    // Only the requester of an approved request is told how to use it.
    await expect(page.getByTestId("approval-hint")).toHaveCount(1);
    await expect(rows.nth(2).getByTestId("approval-hint")).toContainText(`Apelora-Approval: ${uid(93)}`);
    await rows.nth(2).getByRole("button", { name: /^Copy the header/ }).click();
    await expect(rows.nth(2).getByRole("button", { name: /^Copy the header/ })).toHaveText("Copied");
    expect(await page.evaluate(() => navigator.clipboard.readText())).toBe(`Apelora-Approval: ${uid(93)}`);

    await rows.nth(1).getByRole("button", { name: "Approve hardware.power on machine:rack1-02" }).click();
    await expect(page.getByTestId("approvals-error")).toHaveText("you cannot approve your own request. Another approver has to decide this request.");
    await rows.nth(0).getByRole("button", { name: "Reject hardware.power on machine:rack1-01" }).click();
    await expect(page.getByTestId("approvals-error")).toHaveText("that request is not pending");
    await rows.nth(0).getByRole("button", { name: "Approve hardware.power on machine:rack1-01" }).click();
    await expect(page.getByTestId("approvals-notice")).toContainText("Approved hardware.power on machine:rack1-01. The requester has 30 minutes to use it once.");
    await rows.nth(1).getByRole("button", { name: "Cancel the request for hardware.power on machine:rack1-02" }).click();
    await expect(page.getByTestId("approvals-notice")).toContainText("Your request for hardware.power on machine:rack1-02 was cancelled.");
    expect(countOf(calls, `POST /api/v1/approvals/${uid(91)}/approve`)).toBe(1);
    await expect(rows.nth(3).getByRole("button")).toHaveCount(0);
  });

  test("requesting approval offers the actions that need it and validates the target", async ({ page }) => {
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/policies": items([approvalPolicy, { ...mfa, id: uid(83) }]),
      "GET /api/v1/approvals": items([]),
      "POST /api/v1/approvals": (c) => ({ status: 201, json: ONE(5, { ...(c.body as object), requester: `user:${ME}` }) }),
    });
    await page.goto("/#/identity/policies");
    await page.getByTestId("tab-approvals").click();
    await expect(page.getByTestId("approvals-empty")).toHaveText("No approval requests.");
    await page.getByTestId("approval-open").click();
    const dialog = page.getByRole("dialog", { name: "Request approval" });
    // Always-protected actions, plus the extra ones from enabled approval policies (this one is disabled).
    await expect(dialog.getByTestId("approval-action").locator("option")).toHaveText(["Choose an action…", "cluster.evacuate_unsafe", "hardware.power"]);
    await dialog.getByTestId("approval-action").selectOption("hardware.power");
    await dialog.getByTestId("approval-target").fill("machine rack 7");
    await expect(dialog.getByTestId("approval-submit")).toBeDisabled();
    await dialog.getByTestId("approval-target").fill("machine:rack1-07");
    await dialog.getByTestId("approval-reason").fill("Replace the fan");
    await dialog.getByTestId("approval-submit").click();
    await expect(page.getByTestId("approvals-notice")).toContainText("Requested hardware.power on machine:rack1-07");
    expect(bodyOf(calls, "POST /api/v1/approvals")).toEqual([{ action: "hardware.power", target: "machine:rack1-07", reason: "Replace the fan" }]);
  });

  test("tabs work with the arrow keys, and approvals alone are enough to open the page", async ({ page }) => {
    await stub(page, { ...base(), "GET /api/v1/identity/policies": items([mfa]), "GET /api/v1/approvals": items([]) });
    await page.goto("/#/identity/policies");
    await page.getByTestId("tab-policies").focus();
    await page.keyboard.press("ArrowRight");
    await expect(page.getByTestId("tab-approvals")).toBeFocused();
    await expect(page.getByTestId("tab-approvals")).toHaveAttribute("aria-selected", "true");
    await expect(page.getByRole("tabpanel")).toContainText("No approval requests.");
    await page.unrouteAll({ behavior: "ignoreErrors" });
    await page.route("**/api/**", (route) => route.abort());
    await stub(page, { ...base(whoami(["approval.request"], { platform: ["approval.request"] })), "GET /api/v1/approvals": items([]) });
    await page.reload();
    await expect(page.getByTestId("tab-policies")).toHaveCount(0);
    await expect(page.getByTestId("tab-approvals")).toHaveAttribute("aria-selected", "true");
  });
});

test.describe("audit history", () => {
  const page1 = Array.from({ length: 50 }, (_, i) => auditItem(200 - i, i === 1 ? "role.assign" : "user.disable", i === 1 ? "denied" : "success", i === 1 ? { detail: { reason: "you cannot grant more than you hold: cluster.manage" }, scope: { organization: ACME.id, project: null, application: null } } : {}));
  const page2 = [auditItem(150, "group.create", "success"), auditItem(149, "role.revoke", "failed")];

  test("shows who did what to what, with the outcome in words and denials unmistakable", async ({ page }) => {
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/audit": (c) => (c.query.get("before") === "151" ? items(page2) : items(page1)),
    });
    await page.goto("/#/identity/audit");
    const rows = page.getByTestId("identity-audit-row");
    await expect(rows).toHaveCount(50);
    expect(calls.find((c) => c.path === "/api/v1/identity/audit")!.query.get("limit")).toBe("50");
    await expect(rows.nth(0)).toContainText("Alex Admin");
    await expect(rows.nth(0)).toContainText("Person");
    await expect(rows.nth(0)).toContainText("User disable");
    await expect(rows.nth(0)).toContainText("target-200");
    await expect(rows.nth(0).getByTestId("audit-outcome")).toHaveText("Success");
    const denied = rows.nth(1);
    await expect(denied).toHaveAttribute("data-outcome", "denied");
    await expect(denied.getByTestId("audit-outcome")).toHaveText("Denied");
    await expect(denied.getByTestId("audit-reason")).toHaveText("Reason: you cannot grant more than you hold: cluster.manage");
    await expect(denied).toContainText("Acme Corp");
    expect(await denied.evaluate((el) => getComputedStyle(el.querySelector("td")!).borderLeftWidth)).toBe("4px");
    await denied.locator("summary").click();
    await expect(denied.locator("pre")).toContainText('"reason"');
  });

  test("older events are appended using the last id; the end is announced", async ({ page }) => {
    const calls = await stub(page, { ...base(), "GET /api/v1/identity/audit": (c) => (c.query.get("before") === "151" ? items(page2) : items(page1)) });
    await page.goto("/#/identity/audit");
    await expect(page.getByTestId("identity-audit-row")).toHaveCount(50);
    await page.getByTestId("audit-older").click();
    await expect(page.getByTestId("identity-audit-row")).toHaveCount(52);
    await expect(page.getByTestId("audit-older")).toHaveCount(0);
    await expect(page.getByTestId("audit-end")).toHaveText("No older events.");
    await expect(page.getByTestId("identity-audit-row").last().getByTestId("audit-outcome")).toHaveText("Failed");
    // (React's strict mode in development mounts effects twice, so the first request can repeat.)
    expect([...new Set(calls.filter((c) => c.path === "/api/v1/identity/audit").map((c) => c.query.get("before")))]).toEqual([null, "151"]);
  });

  test("filters by outcome and by action prefix and says when nothing matches", async ({ page }) => {
    const calls = await stub(page, {
      ...base(),
      "GET /api/v1/identity/audit": (c) => (c.query.get("outcome") === "denied" && c.query.get("action") === "role." ? items([]) : c.query.get("outcome") === "denied" ? items([page1[1]]) : items(page1.slice(0, 5))),
    });
    await page.goto("/#/identity/audit");
    await expect(page.getByTestId("identity-audit-row")).toHaveCount(5);
    await expect(page.getByTestId("audit-end")).toBeVisible();
    await page.getByTestId("audit-outcome-filter").selectOption("denied");
    await expect(page.getByTestId("identity-audit-row")).toHaveCount(1);
    await page.getByTestId("audit-action").fill("role.");
    await expect(page.getByTestId("identity-audit-empty")).toHaveText("No events match these filters.");
    const last = calls.filter((c) => c.path === "/api/v1/identity/audit").at(-1)!;
    expect([last.query.get("outcome"), last.query.get("action")]).toEqual(["denied", "role."]);
    await page.getByRole("button", { name: "Clear filters" }).click();
    await expect(page.getByTestId("identity-audit-row")).toHaveCount(5);
  });

  test("empty and error states; a 403 shows the server's message and Retry recovers", async ({ page }) => {
    let denied = true;
    await stub(page, { ...base(), "GET /api/v1/identity/audit": () => (denied ? error(403, "forbidden", "you may not do this (audit.read)") : items([])) });
    await page.goto("/#/identity/audit");
    await expect(page.getByTestId("identity-audit-error")).toHaveText("you may not do this (audit.read)");
    denied = false;
    await page.getByRole("button", { name: "Try again" }).click();
    await expect(page.getByTestId("identity-audit-empty")).toHaveText("Nothing recorded yet.");
  });

  test("shows a loading state first", async ({ page }) => {
    await stub(page, { ...base(), "GET /api/v1/identity/audit": { json: { items: page1.slice(0, 3) }, delayMs: 500 } });
    await page.goto("/#/identity/audit");
    await expect(page.getByTestId("identity-audit-loading")).toBeVisible();
    await expect(page.getByTestId("identity-audit-row")).toHaveCount(3);
  });
});

test.describe("quality", () => {
  // Axe samples colours at one instant, so a fading dialog or menu item would be measured half-way.
  const serious = async (page: Page) => (await page.addStyleTag({ content: "*, *::before, *::after { transition: none !important; animation: none !important; }" }), await new AxeBuilder({ page }).withTags(["wcag2a", "wcag2aa"]).analyze()).violations
    .filter((v) => v.impact === "serious" || v.impact === "critical")
    .map((v) => `${v.id}: ${v.nodes.slice(0, 3).map((n) => `${n.target.join(" ")} ${n.html.slice(0, 110)} ${(n.any[0]?.message ?? "").slice(0, 160)}`).join(" | ")}`);

  test("the Users page, the assignment dialog and the emergency dialog have no serious or critical axe violations", async ({ page }) => {
    await stub(page, { ...base(), "GET /api/v1/identity/users/lookup": items([SAM]), "POST /api/v1/identity/assignments/preview": { json: previewOk() } });
    await page.goto("/#/identity/users");
    await expect(page.getByTestId("user-row")).toHaveCount(3);
    expect(await serious(page)).toEqual([]);
    await page.getByRole("button", { name: "Assign a role to Sam Support" }).click();
    const dialog = page.getByRole("dialog", { name: "Assign a role" });
    await dialog.getByTestId("assignment-role").selectOption({ label: "Application Operator" });
    await dialog.getByTestId("assignment-scope-type").selectOption("organization");
    await dialog.getByTestId("assignment-org").selectOption({ label: "Acme Corp" });
    await expect(dialog.getByTestId("preview-allowed")).toBeVisible();
    expect(await serious(page)).toEqual([]);
    await page.keyboard.press("Escape");
    await page.getByTestId("emergency-open").click();
    await expect(page.getByTestId("emergency-warning")).toBeVisible();
    expect(await serious(page)).toEqual([]);
  });

  test("the other pages and their dialogs have no serious or critical axe violations", async ({ page }) => {
    await stub(page, {
      ...base(),
      "GET /api/v1/identity/assignments": items([assignment(1, { type: "user", id: ME }, "Platform Super Admin", { type: "platform" })]),
      "GET /api/v1/identity/policies": items([{ id: uid(80), name: "Production needs MFA", kind: "mfa", parameters: { environments: ["production"], infrastructure: true }, enabled: true, updatedBy: "x", updatedAt: NOW }]),
      "GET /api/v1/identity/audit": items([auditItem(2, "role.assign", "denied", { detail: { reason: "no" } }), auditItem(1, "user.disable", "success")]),
      "GET /api/v1/approvals": items([{ id: uid(91), action: "hardware.power", target: "machine:rack1-01", requester: `user:${SAM.id}`, approver: null, state: "pending", reason: "", createdAt: NOW, expiresAt: NOW }]),
      "GET /api/v1/identity/projects": items([SHOP]),
      [`GET /api/v1/identity/projects/${SHOP.id}/environments`]: items([PROD]),
      [`GET /api/v1/identity/groups/${GROUP_ACME.id}/members`]: items([SAM]),
    });
    for (const hash of ["groups", "memberships", "roles", "services", "sign-in", "policies", "audit"]) {
      await page.goto(`/#/identity/${hash}`);
      await expect(page.getByRole("heading", { level: 1 })).toBeVisible();
      await page.waitForLoadState("networkidle");
      expect(await serious(page), hash).toEqual([]);
    }
    await page.goto("/#/identity/groups");
    await page.getByRole("button", { name: "Members of Acme developers" }).click();
    await expect(page.getByTestId("group-member-row")).toHaveCount(1);
    expect(await serious(page), "group members").toEqual([]);
    await page.goto("/#/identity/organizations");
    await page.getByRole("button", { name: "Expand organization Acme Corp" }).click();
    await page.getByRole("button", { name: "Expand project Shop" }).click();
    await page.getByTestId("env-config").locator("summary").click();
    expect(await serious(page), "organizations").toEqual([]);
    await page.goto("/#/identity/roles");
    await page.getByTestId("role-open").click();
    await expect(page.getByRole("dialog", { name: "Create a custom role" })).toBeVisible();
    expect(await serious(page), "role dialog").toEqual([]);
    await page.keyboard.press("Escape");
    await page.goto("/#/identity/services");
    await page.getByTestId("service-open").click();
    expect(await serious(page), "service dialog").toEqual([]);
    await page.keyboard.press("Escape");
    await page.goto("/#/identity/sign-in");
    await page.getByRole("button", { name: "Edit Globex staff" }).click();
    await page.getByTestId("provider-advanced").locator("summary").click();
    expect(await serious(page), "provider dialog").toEqual([]);
  });

  test("each page keeps at most one primary button", async ({ page }) => {
    await stub(page, { ...base(), "GET /api/v1/identity/projects": items([SHOP]) });
    for (const hash of ["users", "groups", "organizations", "memberships", "roles", "services", "sign-in", "policies", "audit"]) {
      await page.goto(`/#/identity/${hash}`);
      await expect(page.getByRole("heading", { level: 1 }).first()).toBeVisible();
      await page.waitForLoadState("networkidle");
      expect(await page.locator(".btn-primary:visible").count(), hash).toBeLessThanOrEqual(1);
    }
    await page.goto("/#/identity/policies");
    await page.getByTestId("tab-approvals").click();
    await expect(page.getByTestId("approval-open")).toBeVisible();
    expect(await page.locator(".btn-primary:visible").count(), "approvals tab").toBe(1);
  });

  test("every page fits a phone without sideways scrolling", async ({ page }) => {
    await page.setViewportSize({ width: 390, height: 844 });
    await stub(page, {
      ...base(),
      "GET /api/v1/identity/assignments": items([assignment(1, { type: "user", id: ME }, "Platform Super Admin", { type: "platform" }), assignment(3, { type: "user", id: SAM.id }, "Application Operator", { type: "project", id: SHOP.id }, { reason: "Covers the shop launch" })]),
      "GET /api/v1/identity/audit": items([auditItem(2, "role.assign", "denied", { detail: { reason: "you cannot grant more than you hold: cluster.manage" }, target: "00000000-0000-4000-8000-000000000041" }), auditItem(1, "user.disable", "success")]),
      "GET /api/v1/approvals": items([{ id: uid(91), action: "hardware.power", target: "machine:rack1-01", requester: `user:${ME}`, approver: null, state: "approved", reason: "Replace the fan", createdAt: NOW, expiresAt: NOW }]),
      "GET /api/v1/identity/policies": items([{ id: uid(80), name: "Production needs MFA", kind: "mfa", parameters: { environments: ["production"], infrastructure: true }, enabled: true, updatedBy: "user:x", updatedAt: NOW }]),
      "GET /api/v1/identity/projects": items([SHOP]),
      [`GET /api/v1/identity/projects/${SHOP.id}/environments`]: items([PROD]),
      "POST /api/v1/identity/assignments/preview": { json: previewOk() },
    });
    const overflow = () => page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    const shots = process.env.SHOTS_DIR;
    for (const hash of ["users", "groups", "organizations", "memberships", "roles", "services", "sign-in", "policies", "audit"]) {
      await page.goto(`/#/identity/${hash}`);
      await expect(page.getByRole("heading", { level: 1 }).first()).toBeVisible();
      await page.waitForLoadState("networkidle");
      expect(await overflow(), `${hash} overflows horizontally at 390px`).toBeLessThanOrEqual(0);
      if (shots) await page.screenshot({ path: `${shots}/phone-${hash}.png`, fullPage: true });
    }
    await page.goto("/#/identity/organizations");
    await page.getByRole("button", { name: "Expand organization Acme Corp" }).click();
    await page.getByRole("button", { name: "Expand project Shop" }).click();
    expect(await overflow(), "expanded organizations").toBeLessThanOrEqual(0);
    await page.goto("/#/identity/users");
    await page.getByRole("button", { name: "Assign a role to Sam Support" }).click();
    const dialog = page.getByRole("dialog", { name: "Assign a role" });
    await dialog.getByTestId("assignment-role").selectOption({ label: "Application Operator" });
    await dialog.getByTestId("assignment-scope-type").selectOption("platform");
    await expect(dialog.getByTestId("preview-allowed")).toBeVisible();
    expect(await dialog.evaluate((el) => el.scrollWidth - el.clientWidth), "assignment dialog").toBeLessThanOrEqual(0);
    if (shots) await page.screenshot({ path: `${shots}/phone-assignment-dialog.png` });
  });

  test("desktop screenshots of each page (when SHOTS_DIR is set)", async ({ page }) => {
    const shots = process.env.SHOTS_DIR;
    test.skip(!shots, "set SHOTS_DIR to write screenshots");
    await stub(page, {
      ...base(),
      "GET /api/v1/identity/assignments": items([assignment(1, { type: "user", id: ME }, "Platform Super Admin", { type: "platform" }), assignment(2, { type: "group", id: GROUP_ACME.id }, "Viewer", { type: "organization", id: ACME.id }, { conditions: { requireMfa: true, environments: ["production"] } }), assignment(3, { type: "user", id: SAM.id }, "Application Operator", { type: "project", id: SHOP.id }, { reason: "Covers the shop launch" })]),
      "GET /api/v1/identity/audit": items([auditItem(3, "role.assign", "denied", { detail: { reason: "you cannot grant more than you hold: cluster.manage" }, scope: { organization: ACME.id, project: null, application: null } }), auditItem(2, "user.disable", "success"), auditItem(1, "role.revoke", "failed")]),
      "GET /api/v1/approvals": items([{ id: uid(91), action: "hardware.power", target: "machine:rack1-01", requester: `user:${ME}`, approver: null, state: "approved", reason: "Replace the fan", createdAt: NOW, expiresAt: NOW }]),
      "GET /api/v1/identity/policies": items([{ id: uid(80), name: "Production needs MFA", kind: "mfa", parameters: { environments: ["production"], infrastructure: true }, enabled: true, updatedBy: `user:${ME}`, updatedAt: NOW }]),
      [`GET /api/v1/identity/projects/${SHOP.id}/environments`]: items([PROD]),
      "POST /api/v1/identity/assignments/preview": { json: previewOk() },
      "GET /api/v1/identity/users/lookup": items([SAM]),
    });
    for (const hash of ["users", "groups", "organizations", "memberships", "roles", "services", "sign-in", "policies", "audit"]) {
      await page.goto(`/#/identity/${hash}`);
      await expect(page.getByRole("heading", { level: 1 }).first()).toBeVisible();
      await page.waitForLoadState("networkidle");
      await page.screenshot({ path: `${shots}/desktop-${hash}.png`, fullPage: true });
    }
    await page.goto("/#/identity/organizations");
    await page.getByRole("button", { name: "Expand organization Acme Corp" }).click();
    await page.getByRole("button", { name: "Expand project Shop" }).click();
    await page.getByTestId("env-config").locator("summary").click();
    await page.screenshot({ path: `${shots}/desktop-organizations-open.png`, fullPage: true });
    await page.goto("/#/identity/users");
    await page.getByRole("button", { name: "Assign a role to Sam Support" }).click();
    const dialog = page.getByRole("dialog", { name: "Assign a role" });
    await dialog.getByTestId("assignment-role").selectOption({ label: "Application Operator" });
    await dialog.getByTestId("assignment-scope-type").selectOption("organization");
    await dialog.getByTestId("assignment-org").selectOption({ label: "Acme Corp" });
    await expect(dialog.getByTestId("preview-allowed")).toBeVisible();
    await page.screenshot({ path: `${shots}/desktop-assignment-dialog.png` });
    await page.keyboard.press("Escape");
    await page.getByTestId("emergency-open").click();
    await page.screenshot({ path: `${shots}/desktop-emergency-dialog.png` });
  });
});
