// Which queue rows are selected. A plain click picks one row (its detail panel opens);
// Ctrl/Cmd-click adds or removes a row and Shift-click takes the rows in between, for the
// actions on several jobs at once (QueueView's bulk bar).

export type ClickMode = "single" | "toggle" | "range";

export interface Selection {
  /** The row whose detail panel is open, and where a Shift-click range starts. */
  primary: string | null;
  /** Every selected row, in the order they were picked. */
  checked: string[];
}

export const noSelection: Selection = { primary: null, checked: [] };

/** The selection after a click on row `id`; `order` is the queue's ids, top to bottom. */
export function clickSelection(order: string[], current: Selection, id: string, mode: ClickMode): Selection {
  if (mode === "toggle") {
    if (current.checked.includes(id)) {
      const checked = current.checked.filter((x) => x !== id);
      const primary = current.primary === id ? (checked[checked.length - 1] ?? null) : current.primary;
      return { primary, checked };
    }
    return { primary: id, checked: [...current.checked, id] };
  }
  if (mode === "range") {
    const from = order.indexOf(current.primary ?? id);
    const to = order.indexOf(id);
    if (from < 0 || to < 0) return { primary: id, checked: [id] };
    const [a, b] = from <= to ? [from, to] : [to, from];
    return { primary: current.primary ?? id, checked: order.slice(a, b + 1) };
  }
  // A second click on the only selected row closes it
  if (current.primary === id && current.checked.length <= 1) return noSelection;
  return { primary: id, checked: [id] };
}

/** The selection without rows that have left the queue. */
export function pruneSelection(order: string[], current: Selection): Selection {
  const checked = current.checked.filter((id) => order.includes(id));
  if (checked.length === current.checked.length) return current;
  const primary = current.primary && order.includes(current.primary) ? current.primary : (checked[checked.length - 1] ?? null);
  return { primary, checked };
}
