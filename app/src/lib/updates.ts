import { isTauri } from "@tauri-apps/api/core";
import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { create } from "zustand";

/** What the app knows about updates: checked at start and from Settings. */
interface UpdateState {
  checking: boolean;
  /** A newer release, when one was found. */
  update: Update | null;
  /** The last check found nothing newer. */
  upToDate: boolean;
  error: string | null;
  /** Download progress while installing: bytes done / total (0 when unknown). */
  installing: { done: number; total: number } | null;
  dismissed: boolean;
}

export const useUpdates = create<UpdateState>(() => ({
  checking: false,
  update: null,
  upToDate: false,
  error: null,
  installing: null,
  dismissed: false,
}));

const set = useUpdates.setState;

/** Asks the latest GitHub release whether there's a newer version (signature-checked on install). */
export async function checkForUpdate(): Promise<void> {
  if (!isTauri() || useUpdates.getState().checking) return;
  set({ checking: true, error: null, upToDate: false });
  try {
    const update = await check();
    set({ update, upToDate: update == null, checking: false });
  } catch (e) {
    set({ error: String(e), checking: false });
  }
}

/** Downloads and installs the update, then restarts the app (the queue is kept). */
export async function installAndRestart(): Promise<void> {
  const { update } = useUpdates.getState();
  if (!update) return;
  set({ installing: { done: 0, total: 0 }, error: null });
  try {
    await update.downloadAndInstall((e) => {
      const cur = useUpdates.getState().installing ?? { done: 0, total: 0 };
      if (e.event === "Started") set({ installing: { done: 0, total: e.data.contentLength ?? 0 } });
      if (e.event === "Progress") set({ installing: { ...cur, done: cur.done + e.data.chunkLength } });
    });
    await relaunch();
  } catch (e) {
    set({ installing: null, error: String(e) });
  }
}
