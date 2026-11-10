import { create } from "zustand";
import { emptySnapshot, type Disc, type EngineEvent, type QueueSnapshot } from "../lib/api";
import { type ClickMode, clickSelection, pruneSelection } from "./selection";

/** Jobs whose options are being edited (the Add sheet, started from their options). */
export interface EditTarget {
  ids: string[];
  options: Record<string, unknown>;
  /** What the sheet says is being edited: a file name, or "3 files". */
  label: string;
}

export type View = "queue" | "settings";

interface QueueStore {
  snapshot: QueueSnapshot;
  selectedId: string | null;
  /** Every selected row (Ctrl/Shift-click); the actions on several jobs use these. */
  checked: string[];
  /** Jobs waiting for the Edit options sheet. */
  pendingEdit: EditTarget | null;
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
  /** A click on a queue row: plain, Ctrl/Cmd (toggle) or Shift (range). */
  clickRow: (id: string, mode: ClickMode) => void;
  setPendingEdit: (edit: EditTarget | null) => void;
  setView: (v: View) => void;
  setPendingAdd: (paths: string[] | null) => void;
  setPendingDisc: (disc: Disc | null) => void;
}

export const useQueue = create<QueueStore>((set) => ({
  snapshot: emptySnapshot,
  selectedId: null,
  checked: [],
  pendingEdit: null,
  view: "queue",
  pendingAdd: null,
  pendingDisc: null,
  currentDisc: null,
  apply: (e) =>
    set((s) => {
      switch (e.type) {
        case "queue": {
          // Rows that left the queue leave the selection
          const kept = pruneSelection(
            e.snapshot.jobs.map((j) => j.id),
            { primary: s.selectedId, checked: s.checked },
          );
          return { snapshot: e.snapshot, selectedId: kept.primary, checked: kept.checked };
        }
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
  select: (selectedId) => set({ selectedId, checked: selectedId ? [selectedId] : [] }),
  clickRow: (id, mode) =>
    set((s) => {
      const next = clickSelection(
        s.snapshot.jobs.map((j) => j.id),
        { primary: s.selectedId, checked: s.checked },
        id,
        mode,
      );
      return { selectedId: next.primary, checked: next.checked };
    }),
  setPendingEdit: (pendingEdit) => set({ pendingEdit }),
  setView: (view) => set({ view }),
  setPendingAdd: (pendingAdd) => set({ pendingAdd }),
  setPendingDisc: (pendingDisc) => set({ pendingDisc }),
}));
