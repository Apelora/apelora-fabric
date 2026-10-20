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
  byAppliance: Record<string, ApplianceStatus>;
  error: string | null;
  watch: (application: string, project: string) => void;
  /** Reads the state now (e.g. right after jobs finish) instead of waiting for the next poll. */
  refresh: () => Promise<void>;
  stop: () => void;
};

const POLL_MS = 5000;
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
        byAppliance: Object.fromEntries(s.items.filter((i) => i.project === project).map((i) => [i.appliance, i])),
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
    error: null,
    watch: (application, project) => {
      if (timer) clearInterval(timer);
      set({ application, project, byAppliance: {}, error: null });
      void poll();
      timer = setInterval(() => void poll(), POLL_MS);
    },
    refresh: poll,
    stop: () => {
      if (timer) clearInterval(timer);
      timer = null;
      set({ application: null, project: null, provider: null, observedAt: null, byAppliance: {}, error: null });
    },
  };
});
