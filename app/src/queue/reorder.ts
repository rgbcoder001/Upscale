// Reordering the queue by dragging a row's handle (QueueView).

/** The insertion point under the pointer: before a row in its top half, after it in the bottom half. */
export function insertionAt(x: number, y: number): number | null {
  const row = document.elementFromPoint(x, y)?.closest<HTMLElement>("[data-queue-index]");
  if (!row) return null;
  const i = Number(row.dataset.queueIndex);
  const r = row.getBoundingClientRect();
  return y > r.top + r.height / 2 ? i + 1 : i;
}

/** Where a job dragged from `from` to insertion point `over` ends up once it's out of its old place. */
export function dropIndex(from: number, over: number): number {
  return over > from ? over - 1 : over;
}

// Moving several jobs at once (a dragged selection, Move to top / up / down / bottom). They keep
// their queue order and land together at an index among the jobs that stay in place, which is
// what the engine's queue_move_many takes.

export type Place = "top" | "up" | "down" | "bottom";

/** `ids` in queue order, without ones that have left the queue. */
export function inQueueOrder(order: string[], ids: string[]): string[] {
  const picked = new Set(ids);
  return order.filter((id) => picked.has(id));
}

/** The queue after moving `ids` to `index` among the rest (the engine does the same). */
export function applyMove(order: string[], ids: string[], index: number): string[] {
  const picked = new Set(ids);
  const moved = order.filter((id) => picked.has(id));
  const rest = order.filter((id) => !picked.has(id));
  const at = Math.max(0, Math.min(index, rest.length));
  return [...rest.slice(0, at), ...moved, ...rest.slice(at)];
}

/** Where `ids` land among the rest when dropped at insertion point `over` (0..order.length). */
export function dropIndexMany(order: string[], ids: string[], over: number): number {
  const picked = new Set(ids);
  return order.slice(0, over).filter((id) => !picked.has(id)).length;
}

/** The index (among the rest) for a Move to top / up / down / bottom of `ids`. */
export function placeIndex(order: string[], ids: string[], place: Place): number {
  const picked = new Set(ids);
  const rest = order.length - picked.size;
  const positions = order.flatMap((id, i) => (picked.has(id) ? [i] : []));
  if (positions.length === 0) return 0;
  switch (place) {
    case "top":
      return 0;
    case "bottom":
      return rest;
    // One step past the nearest job that isn't moving; a scattered selection closes up there
    case "up":
      return Math.max(0, dropIndexMany(order, ids, positions[0]) - 1);
    case "down":
      return Math.min(rest, dropIndexMany(order, ids, positions[positions.length - 1]) + 1);
  }
}

/** Whether moving would change anything. */
export function changesOrder(order: string[], ids: string[], index: number): boolean {
  const after = applyMove(order, ids, index);
  return after.some((id, i) => id !== order[i]);
}
