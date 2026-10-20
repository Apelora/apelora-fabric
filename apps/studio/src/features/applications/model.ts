import type { ApplicationDetail, ApplicationInfo, AuditItem, DraftDto, OperationDto, VersionInfo } from "@apelora/api-client";

/**
 * What the Studio shows for an application. The control plane's answer is used as it is; the
 * browser-only backend fills the same shape and says plainly that it cannot know the runtime
 * state (`source: "none"`, status `unknown`).
 */
export type AppView = Omit<ApplicationInfo, "deployment"> & {
  deployment: Omit<ApplicationInfo["deployment"], "source"> & { source: "mock" | "incus" | "none" };
};
export type AppDetailView = Omit<ApplicationDetail, "application"> & { application: AppView };
export type { AuditItem, DraftDto, OperationDto, VersionInfo };

export type DeploymentStatus = ApplicationInfo["deployment"]["status"];
export type DesignStatus = ApplicationInfo["design"]["status"];
export type OperationKind = OperationDto["kind"];

export interface EnvChoice {
  id: string;
  label: string;
  project: string;
  organization: string;
  kind: string;
}

export interface ListResult {
  items: AppView[];
  observedAt: string;
  stale: boolean;
}

export interface NewApplicationInput {
  name: string;
  description: string;
  icon: string;
  environmentId: string;
  document?: unknown;
}

export type SaveResult = { ok: true; version: number } | { ok: false; conflict: true; currentVersion: number };

/** Error shown to people: a message and whether retrying could help. */
export class AppError extends Error {
  constructor(message: string, readonly code = "error") {
    super(message);
    this.name = "AppError";
  }
}

export interface AppBackend {
  readonly kind: "api" | "local";
  /** What the people looking at the page are told about where the data lives. */
  readonly label: string;
  list(includeArchived?: boolean): Promise<ListResult>;
  get(name: string): Promise<AppDetailView>;
  environments(): Promise<EnvChoice[]>;
  create(input: NewApplicationInput): Promise<AppView>;
  update(name: string, patch: { description?: string; icon?: string }): Promise<AppView>;
  getDraft(name: string): Promise<{ version: number; document: unknown; validationErrors: string[] } | null>;
  saveDraft(name: string, document: unknown, baseVersion: number | null): Promise<SaveResult>;
  publish(name: string, label?: string): Promise<VersionInfo>;
  versionDocument(name: string, revision: number): Promise<unknown>;
  operate(name: string, action: OperationKind, confirmDisruption: boolean): Promise<OperationDto>;
  operation(name: string, id: string): Promise<OperationDto>;
  events(name: string): Promise<AuditItem[]>;
  archive(name: string, archived: boolean): Promise<void>;
}

export const RUNTIME_LABEL: Record<DeploymentStatus, string> = {
  "not-deployed": "Not deployed",
  starting: "Starting",
  running: "Running",
  degraded: "Degraded",
  stopping: "Stopping",
  stopped: "Stopped",
  failed: "Failed",
  unknown: "Unknown",
};
export const DESIGN_LABEL: Record<DesignStatus, string> = { draft: "Draft", validated: "Validated", published: "Published" };

/** Statuses that need someone's attention. */
export const NEEDS_ATTENTION: ReadonlySet<DeploymentStatus> = new Set(["degraded", "failed"]);

/** "2/2 healthy", "Not running" … — one line a row or card can show. */
export function healthLine(a: AppView): string {
  const d = a.deployment;
  if (d.status === "not-deployed") return "Not deployed";
  if (d.status === "unknown") return "Unknown";
  if (d.status === "stopped") return "Not running";
  if (d.replicas.desired === 0) return "—";
  return `${d.replicas.healthy}/${d.replicas.desired} healthy`;
}
