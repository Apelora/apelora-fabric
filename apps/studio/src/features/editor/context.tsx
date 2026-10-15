import { createContext, useContext } from "react";
import { useStore } from "zustand";
import type { StudioState, StudioStore } from "./store";

export const StudioContext = createContext<StudioStore | null>(null);

export function useStudioApi(): StudioStore {
  const s = useContext(StudioContext);
  if (!s) throw new Error("StudioContext missing");
  return s;
}

export function useStudio<T>(selector: (s: StudioState) => T): T {
  return useStore(useStudioApi(), selector);
}
