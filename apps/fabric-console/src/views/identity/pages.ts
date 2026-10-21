/**
 * The Identity & Access pages, in navigation order. A page is offered when the caller holds at
 * least one of its permissions somewhere (from `whoami.capabilities`); the server still decides
 * every request.
 */
export const IDENTITY_PAGES = [
  { id: "identity/users", label: "Users", any: ["user.view", "user.manage"] },
  { id: "identity/groups", label: "Groups", any: ["user.view", "group.manage"] },
  { id: "identity/organizations", label: "Organizations", any: ["organization.view"] },
  // Each environment's Incus project, its restrictions and limit use, and quotas (views/identity/Tenancy.tsx).
  { id: "identity/tenancy", label: "Tenancy & quotas", any: ["organization.view", "project.view", "quota.manage"] },
  { id: "identity/memberships", label: "Project memberships", any: ["user.view", "role.assign"] },
  { id: "identity/roles", label: "Roles & permissions", any: ["role.assign", "role.manage", "user.view"] },
  { id: "identity/services", label: "Service identities", any: ["service_identity.manage"] },
  { id: "identity/sign-in", label: "Sign-in", any: ["platform.security.manage"] },
  // Everyone's sign-ins and personal tokens (views/identity/Credentials.tsx).
  { id: "identity/credentials", label: "Sign-ins & tokens", any: ["user.manage", "token.manage"] },
  { id: "identity/policies", label: "Access policies", any: ["access_policy.manage", "audit.read", "approval.request", "approval.grant"] },
  { id: "identity/audit", label: "Audit history", any: ["audit.read"] },
  // Who is inside which instance right now, and ending it (`#/sessions`; views/Sessions.tsx).
  { id: "sessions", label: "Sessions", any: ["instance.console", "instance.desktop", "application.operate"] },
] as const;

export type IdentityPageId = (typeof IDENTITY_PAGES)[number]["id"];
export const isIdentityPage = (id: string): id is IdentityPageId => IDENTITY_PAGES.some((p) => p.id === id);
