import { create } from "zustand";
import { emptySnapshot, type Disc, type EngineEvent, type QueueSnapshot } from "../lib/api";

export type View = "queue" | "settings";

interface QueueStore {
  snapshot: QueueSnapshot;
  selectedId: string | null;
  view: View;
  /** Paths waiting for the Add sheet. */
  pendingAdd: string[] | null;
  /** A disc waiting for the Rip sheet. */
  pendingDisc: Disc | null;
  /** The disc in the drive (the queue's Rip disc button reopens its sheet). */
  currentDisc: Disc | null;
  apply: (e: EngineEvent) => void;
  setSnapshot: (s: QueueSnapshot) => void;
  select: (id: string | null) => void;
  setView: (v: View) => void;
  setPendingAdd: (paths: string[] | null) => void;
  setPendingDisc: (disc: Disc | null) => void;
}

export const useQueue = create<QueueStore>((set) => ({
  snapshot: emptySnapshot,
  selectedId: null,
  view: "queue",
  pendingAdd: null,
  pendingDisc: null,
  currentDisc: null,
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
        case "disc":
          // A new disc opens the Rip sheet
          return { currentDisc: e.disc, pendingDisc: e.disc };
        case "discRemoved":
          return { currentDisc: null, pendingDisc: s.pendingDisc === s.currentDisc ? null : s.pendingDisc };
        default:
          return {};
      }
    }),
  setSnapshot: (snapshot) => set({ snapshot }),
  select: (selectedId) => set({ selectedId }),
  setView: (view) => set({ view }),
  setPendingAdd: (pendingAdd) => set({ pendingAdd }),
  setPendingDisc: (pendingDisc) => set({ pendingDisc }),
}));
