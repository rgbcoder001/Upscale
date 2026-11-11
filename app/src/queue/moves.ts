// Moving jobs in the queue from the row menu, the selection bar, the keyboard and dragging.

import { api } from "../lib/api";
import { changesOrder, inQueueOrder, type Place, placeIndex } from "./reorder";
import { useQueue } from "./store";

/**
 * The jobs an action on row `id` applies to: the whole selection when the row is part of a
 * selection of several, else just that row. Without a row: the selection.
 */
export function targetsFor(id?: string): string[] {
  const { checked, selectedId, snapshot } = useQueue.getState();
  const order = snapshot.jobs.map((j) => j.id);
  if (id === undefined) return inQueueOrder(order, checked.length > 0 ? checked : selectedId ? [selectedId] : []);
  return checked.length > 1 && checked.includes(id) ? inQueueOrder(order, checked) : [id];
}

/** Moves `ids` together to the top, one place up or down, or the bottom, and keeps them in view. */
export async function moveJobs(ids: string[], place: Place) {
  const order = useQueue.getState().snapshot.jobs.map((j) => j.id);
  const picked = inQueueOrder(order, ids);
  if (picked.length === 0) return;
  const index = placeIndex(order, picked, place);
  if (!changesOrder(order, picked, index)) return;
  await moveJobsTo(picked, index);
}

/** Moves `ids` (in queue order) to `index` among the jobs that stay in place. */
export async function moveJobsTo(ids: string[], index: number) {
  useQueue.getState().setReveal(ids[0]);
  await api.moveMany(ids, index);
}
