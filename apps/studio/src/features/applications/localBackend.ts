import { sampleCommerceStack, lookupClass } from "@apelora/appliance-catalog";
import { applicationName, ensureNics, parseAssembly, randomUid, serializeAssembly, validateAssembly, type ApplicationAssembly } from "@apelora/assembly-model";

import { loadAssembly } from "../editor/persistence";
import { AppError, type AppBackend, type AppDetailView, type AppView, type EnvChoice, type ListResult, type NewApplicationInput, type OperationKind, type SaveResult, type VersionInfo } from "./model";

const KEY = "apelora.studio.apps.v1";
const NOT_CONNECTED = "Connect to a control plane to start, stop or restart applications";

interface LocalVersion { revision: number; label: string | null; draftVersion: number; createdAt: string; document: unknown }
interface LocalApp {
  name: string;
  description: string;
  icon: string;
  createdAt: string;
  updatedAt: string;
  archived: boolean;
  draft: { version: number; document: unknown; updatedAt: string };
  versions: LocalVersion[];
}
interface Store { version: 1; apps: LocalApp[]; seeded: boolean }

const now = () => new Date().toISOString();

function read(): Store {
  try {
    const raw = JSON.parse(localStorage.getItem(KEY) ?? "null") as Store | null;
    if (raw && raw.version === 1 && Array.isArray(raw.apps)) return raw;
  } catch { /* unreadable: start again */ }
  return { version: 1, apps: [], seeded: false };
}

function write(s: Store): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(s));
  } catch {
    throw new AppError("This browser's storage is full or unavailable, so the change was not saved.", "storage");
  }
}

/** Validation errors of a document: what stops it from being published. */
function problems(document: unknown): string[] {
  const parsed = parseAssembly(JSON.stringify(document));
  if (!parsed.ok) return parsed.errors;
  return validateAssembly(ensureNics(parsed.assembly, lookupClass), lookupClass).filter((d) => d.severity === "error").map((d) => d.message);
}

function view(a: LocalApp): AppView {
  const errors = problems(a.draft.document).length;
  const latest = a.versions[a.versions.length - 1];
  const published = latest && latest.draftVersion === a.draft.version;
  const unavailable = { allowed: false, reason: NOT_CONNECTED };
  const count = (a.draft.document as ApplicationAssembly | undefined)?.spec?.appliances?.length ?? 0;
  return {
    name: a.name,
    latestRevision: latest?.revision ?? null,
    createdAt: a.createdAt,
    updatedAt: a.updatedAt,
    description: a.description,
    icon: a.icon,
    project: { id: "local", slug: "local", name: "This browser", organizationId: "local", organizationSlug: "local" },
    environment: null,
    archived: a.archived,
    design: { status: published ? "published" : errors === 0 ? "validated" : "draft", draftVersion: a.draft.version, validationErrors: errors, publishedVersion: latest?.revision ?? null, hasUnpublishedChanges: !published && !!latest, updatedAt: a.draft.updatedAt },
    deployment: {
      status: "unknown",
      observedAt: now(),
      stale: true,
      source: "none",
      deployedVersion: null,
      appliances: count,
      replicas: { desired: 0, running: 0, healthy: 0, unhealthy: 0 },
      lastActivityAt: a.updatedAt,
      operation: null,
    },
    actions: { start: unavailable, stop: unavailable, restart: unavailable },
  };
}

/** Seeds the Applications section with the diagram the Assembly Library already holds (or the commerce sample). */
function seed(s: Store): Store {
  if (s.seeded) return s;
  const legacy = loadAssembly().assembly ?? sampleCommerceStack();
  const name = applicationName(legacy.metadata.name);
  if (!s.apps.some((a) => a.name === name)) {
    const t = now();
    s.apps.push({ name, description: "Web shop: gateway, load balancer, two NGINX servers, PostgreSQL and Grafana.", icon: "nginx", createdAt: t, updatedAt: t, archived: false, draft: { version: 1, document: JSON.parse(serializeAssembly(legacy)) as unknown, updatedAt: t }, versions: [] });
  }
  s.seeded = true;
  write(s);
  return s;
}

function find(s: Store, name: string): LocalApp {
  const a = s.apps.find((x) => x.name === name);
  if (!a) throw new AppError(`Application ${name} does not exist in this browser.`, "not_found");
  return a;
}

/**
 * Applications kept in this browser when no control plane is connected. Design work (drafts, versions,
 * icons) is real; runtime state is not knowable here, so it is `unknown` and nothing can be started.
 */
export function createLocalBackend(): AppBackend {
  const load = () => seed(read());
  return {
    kind: "local",
    label: "This browser",
    async list(includeArchived = false): Promise<ListResult> {
      const items = load().apps.filter((a) => includeArchived || !a.archived).map(view).sort((p, q) => p.name.localeCompare(q.name));
      return { items, observedAt: now(), stale: true };
    },
    async get(name): Promise<AppDetailView> {
      const a = find(load(), name);
      const versions: VersionInfo[] = [...a.versions].reverse().map((v) => ({ revision: v.revision, published: true, label: v.label, draftVersion: v.draftVersion, createdBy: "you", createdAt: v.createdAt }));
      return { application: view(a), appliances: [], versions, operations: [] };
    },
    async environments(): Promise<EnvChoice[]> {
      return [{ id: "local", label: "This browser", project: "This browser", organization: "local", kind: "other" }];
    },
    async create(input: NewApplicationInput): Promise<AppView> {
      const s = load();
      if (s.apps.some((a) => a.name === input.name)) throw new AppError(`An application named ${input.name} already exists.`, "conflict");
      const t = now();
      const document = input.document ?? JSON.parse(serializeAssembly({ apiVersion: "fabric.apelora/v1alpha1", kind: "ApplicationAssembly", metadata: { name: input.name, uid: `assembly-${randomUid()}` }, spec: { appliances: [], connections: [] }, editor: { layoutVersion: 1, density: "compact", nodes: [] } })) as unknown;
      const app: LocalApp = { name: input.name, description: input.description, icon: input.icon, createdAt: t, updatedAt: t, archived: false, draft: { version: 1, document, updatedAt: t }, versions: [] };
      s.apps.push(app);
      write(s);
      return view(app);
    },
    async update(name, patch): Promise<AppView> {
      const s = load();
      const a = find(s, name);
      if (patch.description !== undefined) a.description = patch.description;
      if (patch.icon !== undefined) a.icon = patch.icon;
      a.updatedAt = now();
      write(s);
      return view(a);
    },
    async getDraft(name) {
      const a = find(load(), name);
      return { version: a.draft.version, document: a.draft.document, validationErrors: problems(a.draft.document) };
    },
    async saveDraft(name, document, baseVersion): Promise<SaveResult> {
      const s = load();
      const a = find(s, name);
      if (baseVersion !== a.draft.version) return { ok: false, conflict: true, currentVersion: a.draft.version };
      a.draft = { version: a.draft.version + 1, document, updatedAt: now() };
      a.updatedAt = a.draft.updatedAt;
      write(s);
      return { ok: true, version: a.draft.version };
    },
    async publish(name, label): Promise<VersionInfo> {
      const s = load();
      const a = find(s, name);
      const errors = problems(a.draft.document);
      if (errors.length) throw new AppError(`The draft has problems: ${errors.slice(0, 5).join("; ")}`, "draft_invalid");
      const last = a.versions[a.versions.length - 1];
      if (last?.draftVersion === a.draft.version) throw new AppError(`This draft is already published as version ${last.revision}; change it to publish again.`, "already_published");
      const v: LocalVersion = { revision: (last?.revision ?? 0) + 1, label: label?.trim() || null, draftVersion: a.draft.version, createdAt: now(), document: a.draft.document };
      a.versions.push(v);
      a.updatedAt = v.createdAt;
      write(s);
      return { revision: v.revision, published: true, label: v.label, draftVersion: v.draftVersion, createdBy: "you", createdAt: v.createdAt };
    },
    async versionDocument(name, revision) {
      const v = find(load(), name).versions.find((x) => x.revision === revision);
      if (!v) throw new AppError(`Version ${revision} does not exist.`, "not_found");
      return v.document;
    },
    async operate(_name: string, _action: OperationKind): Promise<never> {
      throw new AppError(NOT_CONNECTED, "not_connected");
    },
    async operation(): Promise<never> {
      throw new AppError(NOT_CONNECTED, "not_connected");
    },
    async events() {
      return [];
    },
    async archive(name, archived) {
      const s = load();
      find(s, name).archived = archived;
      write(s);
    },
  };
}
