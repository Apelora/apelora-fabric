import { create } from "zustand";

import type { ModuleStatus, ProviderMode } from "@apelora/api-client";

import { client, messageOf } from "./api";

/**
 * Live state of a deployed application, by module id. Kept apart from the design: the
 * overlay shows it on the canvas but never changes the document.
 */
type LiveState = {
  application: string | null;
  project: string | null;
  provider: ProviderMode | null;
  observedAt: string | null;
  /** Every instance (replica) of each module. */
  byModule: Record<string, ModuleStatus[]>;
  /** Service address per module (stateless modules others connect to). */
  services: Record<string, string>;
  error: string | null;
  watch: (application: string, project: string) => void;
  /** Reads the state now (e.g. right after jobs finish) instead of waiting for the next poll. */
  refresh: () => Promise<void>;
  stop: () => void;
};

const POLL_MS = 5000;

/**
 * One state for a module's instances: all serving (`Running`), some (`Degraded`), running
 * but all failing their check (`Unhealthy`), or the first instance's status. A replica serves
 * when it runs and the worker has not found it unhealthy.
 */
export function liveSummary(instances: readonly ModuleStatus[] | undefined, service?: string): { state: string; label: string } {
  if (!instances?.length) return { state: "absent", label: "Not deployed" };
  const running = instances.filter((i) => i.status === "Running");
  const serving = running.filter((i) => i.health?.health !== "unhealthy").length;
  const state = serving === instances.length ? "Running" : serving > 0 ? "Degraded" : running.length > 0 ? "Unhealthy" : instances[0]!.status;
  const lines = instances.map((i) => `${i.instance}: ${i.status}${i.health?.health === "unhealthy" ? ", failing its port check" : ""} on ${i.location}`);
  if (service) lines.push(`Service address ${service}`);
  return { state, label: lines.join("\n") };
}
let timer: ReturnType<typeof setInterval> | null = null;

export const useLive = create<LiveState>((set, get) => {
  const poll = async () => {
    const { application, project } = get();
    if (!application || !project) return;
    try {
      const s = await client.request("application_status", { params: { name: application } });
      if (get().application !== application) return;
      set({
        provider: s.source.provider,
        observedAt: s.source.observedAt,
        byModule: s.items
          .filter((i) => i.project === project)
          .reduce<Record<string, ModuleStatus[]>>((m, i) => ({ ...m, [i.module]: [...(m[i.module] ?? []), i] }), {}),
        services: Object.fromEntries(s.services.filter((x) => x.project === project).map((x) => [x.module, x.address])),
        error: null,
      });
    } catch (e) {
      set({ error: messageOf(e) });
    }
  };
  return {
    application: null,
    project: null,
    provider: null,
    observedAt: null,
    byModule: {},
    services: {},
    error: null,
    watch: (application, project) => {
      if (timer) clearInterval(timer);
      set({ application, project, byModule: {}, services: {}, error: null });
      void poll();
      timer = setInterval(() => void poll(), POLL_MS);
    },
    refresh: poll,
    stop: () => {
      if (timer) clearInterval(timer);
      timer = null;
      set({ application: null, project: null, provider: null, observedAt: null, byModule: {}, services: {}, error: null });
    },
  };
});
