import { create } from "zustand";
import { emptySnapshot, type EngineEvent, type QueueSnapshot } from "../lib/api";

export type View = "queue" | "settings";

interface QueueStore {
  snapshot: QueueSnapshot;
  selectedId: string | null;
  view: View;
  /** Paths waiting for the Add sheet. */
  pendingAdd: string[] | null;
  apply: (e: EngineEvent) => void;
  setSnapshot: (s: QueueSnapshot) => void;
  select: (id: string | null) => void;
  setView: (v: View) => void;
  setPendingAdd: (paths: string[] | null) => void;
}

export const useQueue = create<QueueStore>((set) => ({
  snapshot: emptySnapshot,
  selectedId: null,
  view: "queue",
  pendingAdd: null,
  apply: (e) =>
    set((s) => {
      switch (e.type) {
        case "queue":
          return { snapshot: e.snapshot };
        case "status":
          return { snapshot: { ...s.snapshot, status: e.status } };
        case "job":
          return {
            snapshot: { ...s.snapshot, jobs: s.snapshot.jobs.map((j) => (j.id === e.job.id ? e.job : j)) },
          };
        default:
          return {};
      }
    }),
  setSnapshot: (snapshot) => set({ snapshot }),
  select: (selectedId) => set({ selectedId }),
  setView: (view) => set({ view }),
  setPendingAdd: (pendingAdd) => set({ pendingAdd }),
}));
