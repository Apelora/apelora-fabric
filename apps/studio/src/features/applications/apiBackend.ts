import { ApiRequestError, newIdempotencyKey } from "@apelora/api-client";

import { client } from "../deploy/api";
import { AppError, type AppBackend, type AppView, type EnvChoice, type SaveResult } from "./model";

const asError = (e: unknown): AppError => {
  if (e instanceof ApiRequestError) return new AppError(e.message, e.code);
  return new AppError(e instanceof Error ? e.message : String(e));
};

async function wrap<T>(run: () => Promise<T>): Promise<T> {
  try {
    return await run();
  } catch (e) {
    throw asError(e);
  }
}

/** Applications held by the Apelora control plane: the authoritative source of status and drafts. */
export function createApiBackend(): AppBackend {
  const backend: AppBackend = {
    kind: "api",
    label: "Control plane",
    list: (includeArchived = false) => wrap(async () => (await client.request("applications", { query: { includeArchived } })) as Awaited<ReturnType<AppBackend["list"]>>),
    get: (name) => wrap(async () => (await client.request("application", { params: { name } })) as Awaited<ReturnType<AppBackend["get"]>>),
    environments: () =>
      wrap(async () => {
        const projects = await client.request("projects_list", {});
        const out: EnvChoice[] = [];
        for (const p of projects.items) {
          const envs = await client.request("environments", { params: { project: p.id } });
          for (const e of envs.items) out.push({ id: e.id, label: `${p.organizationSlug} / ${p.name} · ${e.name}`, project: p.name, organization: p.organizationSlug, kind: e.kind });
        }
        return out;
      }),
    create: (input) => wrap(async () => (await client.request("create_application", { body: { name: input.name, description: input.description, icon: input.icon, environmentId: input.environmentId, document: input.document as Record<string, unknown> | undefined } })) as AppView),
    update: (name, patch) => wrap(async () => (await client.request("update_application", { params: { name }, body: patch })) as AppView),
    getDraft: async (name) => {
      try {
        const d = await client.request("application_draft", { params: { name } });
        return { version: d.version, document: d.document, validationErrors: d.validationErrors };
      } catch (e) {
        if (e instanceof ApiRequestError && e.code === "no_draft") return null;
        throw asError(e);
      }
    },
    saveDraft: async (name, document, baseVersion): Promise<SaveResult> => {
      try {
        const d = await client.request("save_application_draft", { params: { name }, body: { document: document as Record<string, unknown>, baseVersion: baseVersion ?? undefined } });
        return { ok: true, version: d.version };
      } catch (e) {
        if (e instanceof ApiRequestError && e.code === "draft_conflict") {
          const current = await backend.getDraft(name).catch(() => null);
          return { ok: false, conflict: true, currentVersion: current?.version ?? 0 };
        }
        throw asError(e);
      }
    },
    publish: (name, label) => wrap(() => client.request("publish_application", { params: { name }, body: { label: label || undefined } })),
    versionDocument: (name, revision) => wrap(async () => (await client.request("application_version", { params: { name, revision: String(revision) } })).document),
    operate: (name, action, confirmDisruption) => wrap(async () => (await client.request("operate_application", { params: { name }, body: { action, confirmDisruption }, idempotencyKey: newIdempotencyKey() })).operation),
    operation: (name, id) => wrap(() => client.request("application_operation", { params: { name, id } })),
    events: (name) => wrap(async () => (await client.request("application_events", { params: { name } })).items),
    archive: (name, archived) => wrap(async () => { await client.request(archived ? "archive_application" : "restore_application", { params: { name } }); }),
  };
  return backend;
}
