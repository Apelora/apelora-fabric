import { useCallback, useEffect, useState } from "react";

import {
  ApiRequestError, newIdempotencyKey,
  type Class, type EnvKind, type GroupDto, type Level, type OrganizationDto, type PermissionDto, type PrincipalRef,
  type ProjectDto, type RoleRef, type Scope, type ServiceIdentityDto, type UserDto, type WhoAmI,
} from "@apelora/api-client";

import { client, messageOf, useLoad, type Load } from "./api";

// ---- what the caller may be offered -----------------------------------------------------------
// The UI only avoids dead ends; the server decides every request.

const hasAny = (list: string[] | undefined, perms: string[]) => perms.some((p) => list?.includes(p));
/** Holds at least one of the permissions at some scope. */
export const holds = (me: WhoAmI | null, ...perms: string[]) => !!me && hasAny(me.capabilities, perms);
/** Holds at least one of the permissions across the whole platform. */
export const holdsPlatform = (me: WhoAmI | null, ...perms: string[]) => !!me && hasAny(me.platformPermissions, perms);

/** How the audit trail and approvals name the caller (`user:<id>`, `service:<id>`, `token:<name>`). */
export function actorRef(me: WhoAmI): string {
  if (me.kind === "user" && me.id) return `user:${me.id}`;
  if (me.kind === "service" && me.id) return `service:${me.id}`;
  if (me.kind === "development") return "development";
  return `token:${me.name}`;
}

// ---- errors -----------------------------------------------------------------------------------

export const errorCode = (e: unknown): string | null => (e instanceof ApiRequestError ? e.code : null);

/** The server's message, made clearer for the two cases people hit most: refusals and self-approval. */
export function describeError(e: unknown): string {
  const message = messageOf(e);
  if (e instanceof ApiRequestError) {
    if (e.code === "self_approval") return `${message}. Another approver has to decide this request.`;
    if (e.status === 403) return `Not allowed: ${message}`;
  }
  return message;
}

/** Runs one action at a time, keeping its error (or 401 hand-off) in one place. */
export function useRun(onAuth: (message: string) => void) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = useCallback(async (action: () => Promise<unknown>): Promise<boolean> => {
    setBusy(true);
    setError(null);
    try {
      await action();
      return true;
    } catch (e) {
      if (e instanceof ApiRequestError && e.unauthenticated) onAuth(e.message);
      setError(describeError(e));
      return false;
    } finally {
      setBusy(false);
    }
  }, [onAuth]);
  return { busy, error, setError, run };
}

// ---- time -------------------------------------------------------------------------------------

export const fmtTime = (t: string | null | undefined) => (t ? (Number.isNaN(Date.parse(t)) ? t : new Date(t).toLocaleString()) : "—");
export const fmtDate = (t: string | null | undefined) => (t ? (Number.isNaN(Date.parse(t)) ? t : new Date(t).toLocaleDateString()) : "—");

/** The value of `value` after it stopped changing for `ms`. */
export function useDebounced<T>(value: T, ms: number): T {
  const [out, setOut] = useState(value);
  useEffect(() => {
    const id = setTimeout(() => setOut(value), ms);
    return () => clearTimeout(id);
  }, [value, ms]);
  return out;
}

export const newKey = newIdempotencyKey;

// ---- vocabulary -------------------------------------------------------------------------------

export const LEVEL_LABEL: Record<Level, string> = { platform: "Platform", organization: "Organization", project: "Project", application: "Application", pool: "Hardware pool" };
export const LEVEL_ORDER: Level[] = ["platform", "organization", "project", "application", "pool"];
export const CLASS_LABEL: Record<Class, string> = { tenant: "Tenant", infrastructure: "Infrastructure", security: "Security" };
export const CLASS_ORDER: Class[] = ["tenant", "infrastructure", "security"];
export const CLASS_NOTE: Record<Class, string> = {
  tenant: "Applications, projects, organizations and their members. Can be granted at any level down to the permission's narrowest scope.",
  infrastructure: "The cluster, networks, storage and physical hardware. Platform or hardware pool only.",
  security: "Platform identity, credentials, policy and emergency access. Platform only.",
};
export const ENV_KINDS: EnvKind[] = ["production", "staging", "development", "other"];
export const ENV_LABEL: Record<EnvKind, string> = { production: "Production", staging: "Staging", development: "Development", other: "Other" };

/**
 * The levels a custom role with these permissions can be assigned at. This mirrors the server's rule
 * (security permissions: platform only; infrastructure mixed with tenant: platform only; infrastructure
 * only: platform, plus pools when every permission is pool-level; tenant only: down to the narrowest
 * floor). The server's answer in the created role is the one that counts.
 */
export function customRoleLevels(selected: string[], catalogue: PermissionDto[]): Level[] {
  const defs = selected.map((id) => catalogue.find((p) => p.id === id)).filter((d): d is PermissionDto => !!d);
  if (defs.length === 0) return [];
  const has = (c: Class) => defs.some((d) => d.class === c);
  if (has("security") || (has("infrastructure") && has("tenant"))) return ["platform"];
  if (has("infrastructure")) return defs.every((d) => d.floor === "pool") ? ["platform", "pool"] : ["platform"];
  const tree: Level[] = ["platform", "organization", "project", "application"];
  const narrowest = Math.max(...defs.map((d) => LEVEL_ORDER.indexOf(d.floor)));
  return tree.filter((l) => LEVEL_ORDER.indexOf(l) <= narrowest);
}

// ---- references -------------------------------------------------------------------------------

export const NIL_ID = "00000000-0000-0000-0000-000000000000";
export const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i;
const short = (id: string) => id.slice(0, 8);

export const roleKey = (r: RoleRef) => (r.type === "builtin" ? `builtin:${r.key}` : `custom:${r.id}`);
export const scopeKey = (s: Scope) => (s.type === "platform" ? "platform" : s.type === "pool" ? `pool:${s.name}` : `${s.type}:${s.id}`);
export const principalKey = (p: PrincipalRef) => `${p.type}:${p.id}`;
export const PRINCIPAL_LABEL: Record<PrincipalRef["type"], string> = { user: "Person", group: "Group", service: "Automation" };

export const personName = (u: UserDto) => u.displayName || u.username || u.email || `Person ${short(u.id)}`;

/** A slug as the server accepts it: 2–40 lowercase letters, digits and hyphens, starting with a letter. */
export const validSlug = (s: string) => s.length >= 2 && s.length <= 40 && /^[a-z][a-z0-9-]*$/.test(s) && !s.endsWith("-");
export const SLUG_HINT = "2–40 lowercase letters, digits and hyphens, starting with a letter";

// ---- directory: the names behind ids ----------------------------------------------------------

/** A list that may be unavailable to this caller ("off") instead of loading. */
export type Optional<T> = Load<T> | { kind: "off" };
export const dataOf = <T,>(o: Optional<T[]>): T[] => (o.kind === "ok" ? o.data : []);

/** Loads only when `enabled`; otherwise reports "off" without calling the API. */
function useOptional<T>(enabled: boolean, fetcher: () => Promise<T>, onAuth: (m: string) => void): [Optional<T>, () => void] {
  const [state, reload] = useLoad<T | null>(() => (enabled ? fetcher() : Promise.resolve(null)), onAuth);
  if (state.kind === "ok" && state.data === null) return [{ kind: "off" }, reload];
  return [state as Load<T>, reload];
}

export type Names = {
  org: (id: string) => string;
  project: (id: string) => string;
  group: (id: string) => string;
  user: (id: string) => string;
  service: (id: string) => string;
  scope: (s: Scope) => string;
  principal: (p: PrincipalRef) => string;
};

export function makeNames(d: { orgs: OrganizationDto[]; projects: ProjectDto[]; groups: GroupDto[]; users: UserDto[]; services: ServiceIdentityDto[] }): Names {
  const org = (id: string) => d.orgs.find((o) => o.id === id)?.name ?? `Organization ${short(id)}`;
  const project = (id: string) => {
    const p = d.projects.find((x) => x.id === id);
    return p ? `${p.organizationSlug} / ${p.name}` : `Project ${short(id)}`;
  };
  const group = (id: string) => d.groups.find((g) => g.id === id)?.name ?? `Group ${short(id)}`;
  const user = (id: string) => { const u = d.users.find((x) => x.id === id); return u ? personName(u) : `Person ${short(id)}`; };
  const service = (id: string) => d.services.find((s) => s.id === id)?.name ?? `Automation ${short(id)}`;
  return {
    org, project, group, user, service,
    scope: (s) => (s.type === "platform" ? "Platform" : s.type === "organization" ? `Organization ${org(s.id)}` : s.type === "project" ? `Project ${project(s.id)}` : s.type === "application" ? `Application ${short(s.id)}` : `Hardware pool ${s.name}`),
    principal: (p) => (p.type === "user" ? user(p.id) : p.type === "group" ? group(p.id) : service(p.id)),
  };
}

/** `user:<id>` and `service:<id>` become names where this caller may see them; other actors stay as recorded. */
export function actorLabel(actor: string, names: Names): string {
  const i = actor.indexOf(":");
  const kind = i < 0 ? "" : actor.slice(0, i);
  const id = i < 0 ? "" : actor.slice(i + 1);
  if (kind === "user" && id) return names.user(id);
  if (kind === "service" && id) return names.service(id);
  return actor;
}

export type Wanted = { orgs?: boolean; projects?: boolean; groups?: boolean; users?: boolean; services?: boolean };
export type Directory = {
  orgs: Optional<OrganizationDto[]>;
  projects: Optional<ProjectDto[]>;
  groups: Optional<GroupDto[]>;
  users: Optional<UserDto[]>;
  services: Optional<ServiceIdentityDto[]>;
  names: Names;
  reload: () => void;
};

/**
 * The organisations, projects, groups, people and automation identities this caller may list, for
 * pickers and for turning ids into names. What the caller may not list is "off", never guessed.
 */
export function useDirectory(me: WhoAmI | null, onAuth: (m: string) => void, want: Wanted): Directory {
  const [orgs, rOrgs] = useOptional(!!want.orgs && holds(me, "organization.view"), () => client.get("organizations").then((r) => r.items), onAuth);
  const [projects, rProjects] = useOptional(!!want.projects && holds(me, "project.view", "organization.view"), () => client.get("projects").then((r) => r.items), onAuth);
  const [groups, rGroups] = useOptional(!!want.groups && holds(me, "user.view", "group.manage"), () => client.get("groups").then((r) => r.items), onAuth);
  const [users, rUsers] = useOptional(!!want.users && holds(me, "user.view", "user.manage"), () => client.get("users").then((r) => r.items), onAuth);
  const [services, rServices] = useOptional(!!want.services && holdsPlatform(me, "service_identity.manage"), () => client.get("service_identities").then((r) => r.items), onAuth);
  const names = makeNames({ orgs: dataOf(orgs), projects: dataOf(projects), groups: dataOf(groups), users: dataOf(users), services: dataOf(services) });
  const reload = () => { rOrgs(); rProjects(); rGroups(); rUsers(); rServices(); };
  return { orgs, projects, groups, users, services, names, reload };
}
