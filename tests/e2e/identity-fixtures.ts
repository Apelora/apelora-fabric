import type { Page, Request } from "@playwright/test";

// A stubbed control plane for the Identity & Access pages: nothing here reaches a real server.

export const NOW = "2026-10-20T09:00:00Z";
export const uid = (n: number) => `00000000-0000-4000-8000-${String(n).padStart(12, "0")}`;
export const NIL = "00000000-0000-0000-0000-000000000000";

export const ME = uid(1);
export const ALEX = { id: ME, username: "alex", email: "alex@example.org", displayName: "Alex Admin", issuer: "https://login.example.org/realms/apelora", createdAt: NOW, lastSeenAt: NOW, disabled: false };
export const SAM = { id: uid(2), username: "sam", email: "sam@example.org", displayName: "Sam Support", issuer: "https://login.example.org/realms/apelora", createdAt: NOW, lastSeenAt: "2026-10-19T12:00:00Z", disabled: false };
export const OFF = { id: uid(3), username: "olga", email: "olga@example.org", displayName: "Olga Offboarded", issuer: "https://login.example.org/realms/apelora", createdAt: NOW, lastSeenAt: null, disabled: true };

export const ACME = { id: uid(10), slug: "acme", name: "Acme Corp", createdAt: NOW, disabled: false };
export const GLOBEX = { id: uid(11), slug: "globex", name: "Globex", createdAt: NOW, disabled: false };
export const SHOP = { id: uid(20), organizationId: ACME.id, organizationSlug: "acme", slug: "shop", name: "Shop", createdAt: NOW };
export const BILLING = { id: uid(21), organizationId: ACME.id, organizationSlug: "acme", slug: "billing", name: "Billing", createdAt: NOW };
export const PROD = {
  id: uid(30), projectId: SHOP.id, slug: "prod", name: "Production", kind: "production", incusProject: "acme-shop-prod", incusProvisioned: false,
  incusProjectConfig: { restricted: "true", "restricted.containers.privilege": "unprivileged", "restricted.devices.disk": "managed" },
};

export const GROUP_PLATFORM = { id: uid(40), organizationId: null, name: "Platform admins", externalName: "apelora-admins", description: "From the identity provider" };
export const GROUP_ACME = { id: uid(41), organizationId: ACME.id, name: "Acme developers", externalName: null, description: "Acme's developers" };
export const SVC_AUTO = { id: uid(50), name: "ci-pipeline", kind: "automation", backend: "apelora", operations: [], credentialRef: null, restrictions: {}, createdBy: `user:${ME}`, createdAt: NOW, expiresAt: "2027-01-18T09:00:00Z", disabled: false, lastUsedAt: null, apiAccess: true };
export const SVC_INCUS = { id: uid(51), name: "tenant-deployer", kind: "workload", backend: "incus", operations: ["instance.create", "instance.state", "instance.delete", "instance.read"], credentialRef: "incus/deploy-client", restrictions: {}, createdBy: `user:${ME}`, createdAt: NOW, expiresAt: null, disabled: false, lastUsedAt: NOW, apiAccess: false };

// Permission catalogue (a representative subset; `class` and `floor` drive the pages).
type Level = "platform" | "organization" | "project" | "application" | "pool";
const perm = (id: string, cls: string, floor: Level, description: string, needsApproval = false) => ({ id, class: cls, floor, description, needsApproval });
export const PERMISSIONS = [
  perm("organization.view", "tenant", "organization", "See an organization and its projects"),
  perm("project.view", "tenant", "project", "See a project and its environments"),
  perm("project.create", "tenant", "organization", "Create projects in an organization"),
  perm("environment.manage", "tenant", "project", "Create and change environments"),
  perm("application.view", "tenant", "application", "See applications and their status"),
  perm("application.deploy", "tenant", "application", "Deploy a design to its environment"),
  perm("user.view", "tenant", "project", "See users with access in the scope"),
  perm("group.manage", "tenant", "organization", "Create groups and change their members"),
  perm("role.assign", "tenant", "project", "Grant and revoke roles in the scope"),
  perm("audit.read", "tenant", "application", "Read audit events of the scope"),
  perm("approval.request", "tenant", "application", "Ask for approval of a protected action"),
  perm("approval.grant", "tenant", "project", "Approve or reject another person's request"),
  perm("cluster.view", "infrastructure", "platform", "See the Incus cluster"),
  perm("cluster.manage", "infrastructure", "platform", "Join tokens, enrolment holds and other cluster changes"),
  perm("cluster.evacuate_unsafe", "infrastructure", "platform", "Evacuate a member although a workload would lose service", true),
  perm("hardware.view", "infrastructure", "pool", "See physical machines"),
  perm("hardware.power", "infrastructure", "pool", "Power a physical machine on, off or restart it", true),
  perm("organization.create", "security", "platform", "Create organizations"),
  perm("role.manage", "security", "platform", "Define custom roles"),
  perm("service_identity.manage", "security", "platform", "Register and disable service identities"),
  perm("access_policy.manage", "security", "platform", "Change access policies"),
  perm("platform.emergency_access", "security", "platform", "Grant time-limited emergency access"),
  perm("user.manage", "security", "platform", "Create and reset local accounts, disable users and see every user"),
  perm("platform.security.manage", "security", "platform", "Configure sign-in: identity providers and API tokens"),
];
export const ALL_PERMISSION_IDS = PERMISSIONS.map((p) => p.id);

const builtin = (key: string, name: string, description: string, scopes: Level[], permissions: string[]) =>
  ({ id: `builtin:${key}`, reference: { type: "builtin", key }, name, description, builtin: true, permissions, scopes });
export const ROLES = [
  builtin("platform-super-admin", "Platform Super Admin", "Everything, including identity and emergency access.", ["platform"], ALL_PERMISSION_IDS),
  builtin("fabric-administrator", "Fabric Administrator", "Runs the cluster.", ["platform"], ["cluster.view", "cluster.evacuate_unsafe", "audit.read", "approval.grant"]),
  builtin("hardware-operator", "Hardware Operator", "Commissions and powers machines.", ["platform", "pool"], ["hardware.view", "hardware.power"]),
  builtin("organization-owner", "Organization Owner", "Runs one organization.", ["organization"], ["organization.view", "project.view", "project.create", "environment.manage", "user.view", "group.manage", "role.assign"]),
  builtin("project-administrator", "Project Administrator", "Runs one project.", ["project"], ["project.view", "environment.manage", "user.view", "role.assign"]),
  builtin("application-designer", "Application Designer", "Edits application graphs.", ["project", "application"], ["project.view", "application.view"]),
  builtin("application-operator", "Application Operator", "Deploys and operates applications.", ["project", "application"], ["project.view", "application.view", "application.deploy", "approval.request"]),
  builtin("viewer", "Viewer", "Reads what the scope contains.", ["platform", "organization", "project", "application"], ["organization.view", "project.view", "application.view", "cluster.view"]),
  builtin("auditor", "Auditor", "Reads the scope and its audit trail.", ["platform", "organization", "project", "application"], ["organization.view", "project.view", "application.view", "audit.read", "user.view"]),
];
export const CUSTOM_ROLE = { id: `custom:${uid(60)}`, reference: { type: "custom", id: uid(60) }, name: "Release manager", description: "Deploys and approves", builtin: false, permissions: ["application.view", "application.deploy", "approval.grant"], scopes: ["platform", "organization", "project"] };

const grant = (roleName: string, scope: unknown, extra: Record<string, unknown> = {}) => ({ assignment: uid(70), emergency: false, role: { type: "builtin", key: "viewer" }, roleName, scope, ...extra });
export const effective = (permission: string, cls: string, grants: unknown[]) => ({ permission, class: cls, description: PERMISSIONS.find((p) => p.id === permission)?.description ?? "", grants });
export const previewOk = (scopeOrg = ACME.id) => ({
  allowed: true, error: null,
  before: [effective("application.view", "tenant", [grant("Viewer", { type: "organization", id: scopeOrg }, { viaGroup: GROUP_ACME.id })])],
  after: [
    effective("application.view", "tenant", [grant("Viewer", { type: "organization", id: scopeOrg }, { viaGroup: GROUP_ACME.id }), grant("Application Operator", { type: "organization", id: scopeOrg }, { assignment: NIL })]),
    effective("application.deploy", "tenant", [grant("Application Operator", { type: "organization", id: scopeOrg }, { assignment: NIL })]),
  ],
  added: ["application.deploy"], inert: ["approval.request"], warnings: ["2 permission(s) in this role have no effect at this scope."],
});

export const assignment = (n: number, principal: unknown, roleName: string, scope: unknown, extra: Record<string, unknown> = {}) => ({
  id: uid(100 + n), principal, role: { type: "builtin", key: roleName.toLowerCase().replace(/ /g, "-") }, roleName, scope, conditions: { requireMfa: false, environments: null },
  kind: "standard", expiresAt: null, reason: null, createdBy: `user:${ME}`, createdAt: NOW, revokedAt: null, ...extra,
});

export const auditItem = (id: number, action: string, outcome: string, extra: Record<string, unknown> = {}) => ({
  id, at: `2026-10-20T08:${String(59 - (id % 60)).padStart(2, "0")}:00Z`, actor: `user:${ME}`, actorType: "user", action, target: `target-${id}`, outcome, scope: null, detail: {}, ...extra,
});

// ---- the stub itself --------------------------------------------------------------------------

export type Result = { status?: number; json?: unknown; delayMs?: number };
export type Call = { method: string; path: string; query: URLSearchParams; body: unknown };
export type Handler = (call: Call) => Result | Promise<Result>;

/** The local-accounts setting when nobody has turned it on. */
export const LOCAL_OFF = { enabled: false, ready: 0, pending: 0 };

export const health = { status: "ok", service: "control-plane", version: "0.0.0", apiVersion: "v1", provider: "mock", auth: "disabled-development", incus: { connected: false }, checkedAt: "2026-10-20T00:00:00Z" };

/** `kind: "user"` callers with the given permissions; `platform` ones hold them across the platform. */
export function whoami(capabilities: string[], opts: { platform?: string[]; kind?: string; name?: string; credential?: string } = {}) {
  const platform = opts.platform ?? capabilities;
  const kind = opts.kind ?? "user";
  const credential = opts.credential ?? (kind === "legacy-token" ? "api-token" : kind === "service" ? "service-token" : "identity-provider");
  return { name: opts.name ?? "alex", id: kind === "legacy-token" ? null : ME, kind, role: kind === "legacy-token" ? "fabric-admin" : null, permissions: kind === "legacy-token" ? ["read-inventory", "read-audit", "manage-tokens", "manage-infrastructure", "design-applications", "deploy"] : [], deployProjects: [], capabilities, platformPermissions: platform, roles: [], credential };
}
export const SUPER = whoami(ALL_PERMISSION_IDS);

/** Routes every `/api/v1/**` call to a handler keyed `METHOD /path`; unknown calls answer 404 and are recorded. */
export async function stub(page: Page, handlers: Record<string, Handler | Result>): Promise<Call[]> {
  const calls: Call[] = [];
  await page.route("**/api/v1/**", async (route) => {
    const req: Request = route.request();
    const url = new URL(req.url());
    const call: Call = { method: req.method(), path: url.pathname, query: url.searchParams, body: req.postData() ? (JSON.parse(req.postData()!) as unknown) : null };
    calls.push(call);
    const h = handlers[`${call.method} ${call.path}`];
    if (!h) return route.fulfill({ status: 404, json: { error: { code: "not_found", message: `no stub for ${call.method} ${call.path}` } } });
    const r = typeof h === "function" ? await h(call) : h;
    if (r.delayMs) await new Promise((res) => setTimeout(res, r.delayMs));
    return r.json === undefined ? route.fulfill({ status: r.status ?? 204, body: "" }) : route.fulfill({ status: r.status ?? 200, json: r.json });
  });
  return calls;
}

export const error = (status: number, code: string, message: string): Result => ({ status, json: { error: { code, message } } });
export const items = (list: unknown[]): Result => ({ json: { items: list } });

/** The stubs every page needs, for a caller; tests add or replace routes. */
export function base(me: unknown = SUPER): Record<string, Handler | Result> {
  return {
    "GET /api/v1/health": { json: health },
    "GET /api/v1/whoami": { json: me },
    "GET /api/v1/identity/permissions": items(PERMISSIONS),
    "GET /api/v1/identity/roles": items([...ROLES, CUSTOM_ROLE]),
    "GET /api/v1/identity/organizations": items([ACME, GLOBEX]),
    "GET /api/v1/identity/projects": items([SHOP, BILLING]),
    "GET /api/v1/identity/groups": items([GROUP_PLATFORM, GROUP_ACME]),
    "GET /api/v1/identity/users": items([ALEX, SAM, OFF]),
    "GET /api/v1/identity/service-identities": items([SVC_AUTO, SVC_INCUS]),
    "GET /api/v1/identity/sign-in": { json: { providers: [{ id: uid(70), source: "console", name: "Globex staff", issuer: "https://id.globex.example/oidc", audiences: ["apelora-api"], clientId: "apelora-console", scopes: ["openid", "profile", "email"], groupsClaim: "groups", usernameClaim: "preferred_username", mfaAmr: ["mfa"], mfaAcr: [], authorizedParties: [], enabled: true, reachable: true, error: null, updatedBy: null }], apiTokens: { enabled: true, lockedByEnvironment: false }, localAccounts: LOCAL_OFF } },
    "GET /api/v1/identity/policies": items([]),
    "GET /api/v1/approvals": items([]),
    "GET /api/v1/identity/audit": items([]),
    "GET /api/v1/identity/assignments": items([]),
    "GET /api/v1/sessions": items([]),
  };
}
