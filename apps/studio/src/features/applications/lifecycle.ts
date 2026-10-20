import { create } from "zustand";

import { useBackend } from "./backend";
import { AppError, type AppView, type OperationKind } from "./model";
import { useAppToasts } from "./toast";

export interface Tracked {
  kind: OperationKind;
  /** `submitting`: sent, not yet accepted. `running`: the control plane owns it and we follow its progress. */
  phase: "submitting" | "running";
  opId?: string;
  done: number;
  total: number;
}

interface LifecycleState {
  tracked: Record<string, Tracked>;
  /** Stop and restart ask first; this is the application being asked about. */
  asking: { app: AppView; kind: OperationKind } | null;
  /** Bumped whenever an operation finishes, so lists and pages reload their status. */
  bump: number;
  ask: (app: AppView, kind: OperationKind) => void;
  close: () => void;
  run: (name: string, kind: OperationKind, confirmDisruption: boolean) => Promise<{ ok: true } | { ok: false; code: string; message: string }>;
}

const POLL_MS = 1500;
const VERB: Record<OperationKind, string> = { start: "Start", stop: "Stop", restart: "Restart" };

export const useLifecycle = create<LifecycleState>((set, get) => ({
  tracked: {},
  asking: null,
  bump: 0,
  ask: (app, kind) => set({ asking: { app, kind } }),
  close: () => set({ asking: null }),
  run: async (name, kind, confirmDisruption) => {
    // One operation per application: a second click while one is submitted or running does nothing.
    if (get().tracked[name]) return { ok: false, code: "operation_in_progress", message: "An operation is already in progress." };
    const track = (t: Tracked | null) => set((s) => { const next = { ...s.tracked }; if (t) next[name] = t; else delete next[name]; return { tracked: next }; });
    track({ kind, phase: "submitting", done: 0, total: 0 });
    const { backend } = useBackend.getState();
    const toast = useAppToasts.getState().push;
    let op;
    try {
      op = await backend.operate(name, kind, confirmDisruption);
    } catch (e) {
      track(null);
      const err = e instanceof AppError ? e : new AppError(e instanceof Error ? e.message : String(e));
      set((s) => ({ bump: s.bump + 1 }));
      return { ok: false, code: err.code, message: err.message };
    }
    track({ kind, phase: "running", opId: op.id, done: op.stepsDone, total: op.stepsTotal });
    // Follow the backend's own record of progress; success is declared only when it says so.
    void (async () => {
      let current = op;
      try {
        while (current.state === "pending" || current.state === "running") {
          await new Promise((r) => setTimeout(r, POLL_MS));
          current = await backend.operation(name, op.id);
          track({ kind, phase: "running", opId: op.id, done: current.stepsDone, total: current.stepsTotal });
        }
        if (current.state === "succeeded") toast("success", `${VERB[kind]} of ${name} finished: the control plane confirmed every step.`);
        else toast("error", `${VERB[kind]} of ${name} failed${current.error ? `: ${current.error}` : "."} ${current.stepsDone} of ${current.stepsTotal} steps had completed.`);
      } catch (e) {
        toast("error", `Lost track of the ${kind} of ${name}: ${e instanceof Error ? e.message : String(e)}. Its real state is shown on the application.`);
      } finally {
        track(null);
        set((s) => ({ bump: s.bump + 1 }));
      }
    })();
    return { ok: true };
  },
}));
