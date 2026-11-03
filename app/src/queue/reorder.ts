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
