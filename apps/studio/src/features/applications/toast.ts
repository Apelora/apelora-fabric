import { create } from "zustand";

export interface AppToast { id: number; kind: "info" | "success" | "warning" | "error"; text: string }

let seq = 1;
export const useAppToasts = create<{ toasts: AppToast[]; push: (kind: AppToast["kind"], text: string) => void; dismiss: (id: number) => void }>((set, get) => ({
  toasts: [],
  push: (kind, text) => {
    const id = seq++;
    set((s) => ({ toasts: [...s.toasts.slice(-3), { id, kind, text }] }));
    setTimeout(() => get().dismiss(id), kind === "error" ? 8000 : 4000);
  },
  dismiss: (id) => set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
}));
