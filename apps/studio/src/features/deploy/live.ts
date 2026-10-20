import { create } from "zustand";

import type { ApplianceStatus, ProviderMode } from "@apelora/api-client";

import { client, messageOf } from "./api";

/**
 * Live state of a deployed application, by appliance id. Kept apart from the design: the
 * overlay shows it on the canvas but never changes the document.
 */
type LiveState = {
  application: string | null;
  project: string | null;
  provider: ProviderMode | null;
  observedAt: string | null;
  /** Every instance (replica) of each appliance. */
  byAppliance: Record<string, ApplianceStatus[]>;
  /** Service address per appliance (stateless appliances others connect to). */
  services: Record<string, string>;
  error: string | null;
  watch: (application: string, project: string) => void;
  /** Reads the state now (e.g. right after jobs finish) instead of waiting for the next poll. */
  refresh: () => Promise<void>;
  stop: () => void;
};

const POLL_MS = 5000;

/** One state for an appliance's instances: all running, some running (degraded), or the first's. */
export function liveSummary(instances: readonly ApplianceStatus[] | undefined, service?: string): { state: string; label: string } {
  if (!instances?.length) return { state: "absent", label: "Not deployed" };
  const running = instances.filter((i) => i.status === "Running").length;
  const state = running === instances.length ? "Running" : running > 0 ? "Degraded" : instances[0]!.status;
  const lines = instances.map((i) => `${i.instance}: ${i.status} on ${i.location}`);
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
        byAppliance: s.items
          .filter((i) => i.project === project)
          .reduce<Record<string, ApplianceStatus[]>>((m, i) => ({ ...m, [i.appliance]: [...(m[i.appliance] ?? []), i] }), {}),
        services: Object.fromEntries(s.services.filter((x) => x.project === project).map((x) => [x.appliance, x.address])),
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
    byAppliance: {},
    services: {},
    error: null,
    watch: (application, project) => {
      if (timer) clearInterval(timer);
      set({ application, project, byAppliance: {}, services: {}, error: null });
      void poll();
      timer = setInterval(() => void poll(), POLL_MS);
    },
    refresh: poll,
    stop: () => {
      if (timer) clearInterval(timer);
      timer = null;
      set({ application: null, project: null, provider: null, observedAt: null, byAppliance: {}, services: {}, error: null });
    },
  };
});
