import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { memo, useCallback, useEffect, useRef, useState } from "react";
import { api, type Job } from "../lib/api";
import {
  bytes,
  canEdit,
  canRemove,
  duration,
  hasFailed,
  fraction,
  percentChange,
  ripFraction,
  ripText,
  savedText,
  savings,
  sizeSummary,
  statusLabel,
} from "../lib/format";
import { DetailPanel } from "./DetailPanel";
import { moveJobs, moveJobsTo, targetsFor } from "./moves";
import { changesOrder, dropIndexMany, insertionAt, type Place, placeIndex } from "./reorder";
import { useQueue } from "./store";

const VIDEO_EXTENSIONS = ["mkv", "mp4", "m2ts", "m4v", "avi", "webm"];

const button =
  "rounded px-3 py-1.5 text-sm hover:bg-neutral-200 disabled:opacity-40 disabled:hover:bg-transparent dark:hover:bg-neutral-800";

async function pick(directory: boolean) {
  const chosen = await open({
    multiple: true,
    directory,
    filters: directory ? undefined : [{ name: "Videos", extensions: VIDEO_EXTENSIONS }],
  });
  const paths = chosen == null ? [] : Array.isArray(chosen) ? chosen : [chosen];
  if (paths.length > 0) useQueue.getState().setPendingAdd(paths);
}

function Toolbar() {
  const { jobs, running, paused, stopping, status, ripping } = useQueue((s) => s.snapshot);
  const runnable = jobs.some((j) => j.state === "queued" || j.state === "interrupted" || j.state === "analysing");
  const finished = jobs.some((j) => j.state === "done" || j.state === "skipped");
  const failed = jobs.some(hasFailed);
  const disc = useQueue((s) => s.currentDisc);
  return (
    <div className="flex flex-wrap items-center gap-1 border-b border-neutral-200 px-4 py-2 dark:border-neutral-700">
      <button type="button" className="rounded bg-blue-600 px-3 py-1.5 text-sm font-medium text-white" onClick={() => pick(false)}>
        Add files
      </button>
      <button type="button" className={button} onClick={() => pick(true)}>
        Add folder
      </button>
      {ripping ? (
        <button type="button" className={button} onClick={() => api.ripCancel()}>
          Cancel rip
        </button>
      ) : (
        disc && (
          <button type="button" className={button} title={disc.name} onClick={() => useQueue.getState().setPendingDisc(disc)}>
            Rip disc
          </button>
        )
      )}
      <div className="mx-2 h-5 w-px bg-neutral-300 dark:bg-neutral-700" />
      <button type="button" className={button} disabled={running || !runnable} onClick={() => api.start()}>
        Start
      </button>
      <button
        type="button"
        className={button}
        disabled={!running}
        title="Stops reading frames; the GPU memory stays allocated while paused"
        onClick={() => (paused ? api.resume() : api.pause())}
      >
        {paused ? "Resume" : "Pause"}
      </button>
      <button type="button" className={button} disabled={!running} aria-pressed={stopping} onClick={() => api.stopAfter(!stopping)}>
        {stopping ? "Keep going" : "Stop after current"}
      </button>
      <button type="button" className={button} disabled={!running} onClick={() => api.cancel()}>
        Cancel
      </button>
      <div className="flex-1" />
      {status && <span className="text-sm text-neutral-500">{status}</span>}
      {failed && (
        <>
          <button type="button" className={button} onClick={() => api.retryFailed()}>
            Retry failed
          </button>
          <button type="button" className={button} onClick={() => api.clearFailed()}>
            Remove failed
          </button>
        </>
      )}
      <button type="button" className={button} disabled={!finished} onClick={() => api.clearFinished()}>
        Clear finished
      </button>
    </div>
  );
}

function SavingsBar({ jobs }: { jobs: Job[] }) {
  const { done, before, after } = savings(jobs);
  if (done === 0) return null;
  const saved = savedText(before, after);
  return (
    <div className="border-b border-neutral-200 px-4 py-1.5 text-xs tabular-nums text-neutral-500 dark:border-neutral-700">
      {done} of {jobs.length} done · {bytes(before)} → {bytes(after)} ({percentChange(before, after)})
      {saved.text && (
        <>
          {" · "}
          <span className={`font-medium ${saved.grew ? "text-amber-600 dark:text-amber-400" : "text-green-700 dark:text-green-400"}`}>
            {saved.text}
          </span>
        </>
      )}
    </div>
  );
}

/** Actions on several selected rows (Ctrl/Shift-click). */
function BulkBar() {
  const checked = useQueue((s) => s.checked);
  const jobs = useQueue((s) => s.snapshot.jobs);
  if (checked.length < 2) return null;
  const picked = jobs.filter((j) => checked.includes(j.id));
  const editable = picked.filter(canEdit);
  const retryable = picked.filter(hasFailed);
  const removable = picked.filter(canRemove);
  const each = async (list: Job[], action: (id: string) => Promise<unknown>) => {
    for (const j of list) await action(j.id);
  };
  return (
    <div
      className="flex flex-wrap items-center gap-1 border-b border-neutral-200 bg-blue-50 px-4 py-1 text-sm dark:border-neutral-700 dark:bg-blue-950/40"
      role="toolbar"
      aria-label="Selected jobs"
    >
      <span className="mr-2 tabular-nums">{picked.length} selected</span>
      <MoveButtons ids={picked.map((j) => j.id)} />
      <div className="mx-1 h-5 w-px bg-neutral-300 dark:bg-neutral-700" />
      <button
        type="button"
        className={button}
        disabled={editable.length === 0}
        title="Starts from the first selected job's options"
        onClick={() =>
          useQueue.getState().setPendingEdit({
            ids: editable.map((j) => j.id),
            options: editable[0].options,
            label: `${editable.length} files`,
          })
        }
      >
        Edit options
      </button>
      <button type="button" className={button} disabled={retryable.length === 0} onClick={() => each(retryable, api.retry)}>
        Retry
      </button>
      <button type="button" className={button} disabled={removable.length === 0} onClick={() => each(removable, api.remove)}>
        Remove
      </button>
      <div className="flex-1" />
      <button type="button" className={button} title="Esc" onClick={() => useQueue.getState().select(null)}>
        Clear selection
      </button>
    </div>
  );
}

const placeLabels: Record<Place, { label: string; keys: string }> = {
  top: { label: "Move to top", keys: "Alt+Home" },
  up: { label: "Move up", keys: "Alt+↑" },
  down: { label: "Move down", keys: "Alt+↓" },
  bottom: { label: "Move to bottom", keys: "Alt+End" },
};
const places: Place[] = ["top", "up", "down", "bottom"];

/** Small arrows: to the top (a bar above), up, down, to the bottom (a bar below). */
function PlaceIcon({ place }: { place: Place }) {
  const up = place === "top" || place === "up";
  return (
    <svg viewBox="0 0 16 16" className="h-3.5 w-3.5" fill="none" stroke="currentColor" strokeWidth="1.6" aria-hidden="true">
      {place === "top" && <path d="M3 2.5h10" />}
      {place === "bottom" && <path d="M3 13.5h10" />}
      <path d={up ? "M8 13V5M4.5 8.5 8 5l3.5 3.5" : "M8 3v8M4.5 7.5 8 11l3.5-3.5"} />
    </svg>
  );
}

/** Move to top / up / down / bottom for several selected jobs. */
function MoveButtons({ ids }: { ids: string[] }) {
  return (
    <>
      {places.map((place) => (
        <button
          key={place}
          type="button"
          className={`${button} flex items-center gap-1`}
          title={`${placeLabels[place].label} (${placeLabels[place].keys})`}
          aria-label={placeLabels[place].label}
          onClick={() => moveJobs(ids, place)}
        >
          <PlaceIcon place={place} />
          {place === "top" ? "Top" : place === "bottom" ? "Bottom" : null}
        </button>
      ))}
    </>
  );
}

/** Where the right-click menu opened, and on which row. */
interface MenuAt {
  id: string;
  x: number;
  y: number;
}

/** The right-click menu on a row; acts on the whole selection when the row is part of it. */
function RowMenu({ at, onClose }: { at: MenuAt; onClose: () => void }) {
  const jobs = useQueue((s) => s.snapshot.jobs);
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState({ left: at.x, top: at.y });
  const ids = targetsFor(at.id);
  const picked = jobs.filter((j) => ids.includes(j.id));
  const editable = picked.filter(canEdit);
  const retryable = picked.filter(hasFailed);
  const removable = picked.filter(canRemove);
  const order = jobs.map((j) => j.id);
  useEffect(() => {
    // Kept inside the window
    const el = ref.current;
    if (!el) return;
    const r = el.getBoundingClientRect();
    setPos({
      left: Math.max(4, Math.min(at.x, window.innerWidth - r.width - 4)),
      top: Math.max(4, Math.min(at.y, window.innerHeight - r.height - 4)),
    });
    el.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  }, [at]);
  useEffect(() => {
    const away = (e: Event) => {
      if (!ref.current?.contains(e.target as Node)) onClose();
    };
    const key = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        e.stopPropagation();
        onClose();
      }
    };
    window.addEventListener("pointerdown", away, true);
    window.addEventListener("keydown", key, true);
    window.addEventListener("blur", onClose);
    window.addEventListener("resize", onClose);
    document.addEventListener("scroll", onClose, true);
    return () => {
      window.removeEventListener("pointerdown", away, true);
      window.removeEventListener("keydown", key, true);
      window.removeEventListener("blur", onClose);
      window.removeEventListener("resize", onClose);
      document.removeEventListener("scroll", onClose, true);
    };
  }, [onClose]);
  const item =
    "flex w-full items-center gap-2 rounded px-2 py-1 text-left text-sm hover:bg-neutral-100 focus:bg-neutral-100 focus:outline-none disabled:opacity-40 disabled:hover:bg-transparent dark:hover:bg-neutral-800 dark:focus:bg-neutral-800";
  const act = (f: () => unknown) => () => {
    onClose();
    f();
  };
  const each = (list: Job[], action: (id: string) => Promise<unknown>) => async () => {
    for (const j of list) await action(j.id);
  };
  return (
    <div
      ref={ref}
      role="menu"
      aria-label={ids.length > 1 ? `${ids.length} selected jobs` : picked[0]?.name}
      className="fixed z-50 min-w-56 rounded-md border border-neutral-200 bg-white p-1 shadow-lg dark:border-neutral-700 dark:bg-neutral-900"
      style={pos}
      onContextMenu={(e) => e.preventDefault()}
    >
      {ids.length > 1 && <div className="px-2 py-1 text-xs text-neutral-500">{ids.length} selected</div>}
      {places.map((place) => (
        <button
          key={place}
          type="button"
          role="menuitem"
          className={item}
          disabled={!changesOrder(order, ids, placeIndex(order, ids, place))}
          onClick={act(() => moveJobs(ids, place))}
        >
          <PlaceIcon place={place} />
          <span className="flex-1">{placeLabels[place].label}</span>
          <span className="text-xs text-neutral-400">{placeLabels[place].keys}</span>
        </button>
      ))}
      <div className="my-1 h-px bg-neutral-200 dark:bg-neutral-700" />
      <button
        type="button"
        role="menuitem"
        className={item}
        disabled={editable.length === 0}
        onClick={act(() =>
          useQueue.getState().setPendingEdit({
            ids: editable.map((j) => j.id),
            options: editable[0].options,
            label: editable.length > 1 ? `${editable.length} files` : editable[0].name,
          }),
        )}
      >
        <span className="w-3.5" />
        Edit options
      </button>
      <button type="button" role="menuitem" className={item} disabled={retryable.length === 0} onClick={act(each(retryable, api.retry))}>
        <span className="w-3.5" />
        Retry
      </button>
      <div className="my-1 h-px bg-neutral-200 dark:bg-neutral-700" />
      <button type="button" role="menuitem" className={item} onClick={act(() => useQueue.getState().selectAll())}>
        <span className="w-3.5" />
        <span className="flex-1">Select all</span>
        <span className="text-xs text-neutral-400">Ctrl+A</span>
      </button>
      <button
        type="button"
        role="menuitem"
        className={`${item} text-red-600 dark:text-red-400`}
        disabled={removable.length === 0}
        onClick={act(each(removable, api.remove))}
      >
        <span className="w-3.5" />
        <span className="flex-1">{removable.length > 1 ? `Remove ${removable.length}` : "Remove"}</span>
      </button>
    </div>
  );
}

function stateColor(job: Job) {
  switch (job.state) {
    case "done":
      return "text-green-700 dark:text-green-400";
    case "failed":
    case "cancelled":
      return "text-red-600 dark:text-red-400";
    case "interrupted":
      return "text-amber-600 dark:text-amber-400";
    case "running":
    case "ripping":
      return "text-blue-700 dark:text-blue-300";
    default:
      return "text-neutral-500";
  }
}

/**
 * Dragging a row by its handle to reorder the queue. Pointer events, not HTML5 drag and drop:
 * the window's own file drop (Add files by dropping) would swallow those on Windows.
 * `over` is the insertion point, 0..jobs.length.
 */
interface RowDrag {
  /** The dragged jobs: the selection when the handle's row is part of one, in queue order. */
  ids: string[];
  over: number;
}

// Memoised: a progress event replaces only the running job's object, so the other rows
// (hundreds, in a long queue) don't render again every second.
const JobRow = memo(function JobRow({
  job,
  index,
  count,
  drag,
  onDragStart,
  onMenu,
}: {
  job: Job;
  index: number;
  count: number;
  drag: RowDrag | null;
  onDragStart: (e: React.PointerEvent, index: number) => void;
  onMenu: (e: React.MouseEvent, id: string) => void;
}) {
  const selected = useQueue((s) => s.selectedId === job.id || (s.checked.length > 1 && s.checked.includes(job.id)));
  const f = fraction(job);
  const p = job.progress;
  // A waiting or ripping disc title can be removed: that title is skipped or stopped
  const idle = job.state !== "running" && job.state !== "analysing";
  const rf = ripFraction(job);
  const size = sizeSummary(job);
  return (
    <li
      data-queue-index={index}
      // Off-screen rows are neither laid out nor drawn (content-visibility), so a long queue
      // costs little to scroll and update
      className={`cursor-pointer [contain-intrinsic-size:auto_3.5rem] [content-visibility:auto] border-b border-neutral-200 px-4 py-2 text-sm dark:border-neutral-800 ${
        selected ? "bg-blue-50 dark:bg-blue-950/40" : "hover:bg-neutral-100 dark:hover:bg-neutral-800/60"
      } ${drag?.ids.includes(job.id) ? "opacity-50" : ""} ${drag && drag.over === index ? "border-t-2 border-t-blue-600" : ""} ${
        drag && drag.over === count && index === count - 1 ? "border-b-2 border-b-blue-600" : ""
      }`}
      onClick={(e) =>
        !drag && useQueue.getState().clickRow(job.id, e.shiftKey ? "range" : e.ctrlKey || e.metaKey ? "toggle" : "single")
      }
      onContextMenu={(e) => onMenu(e, job.id)}
    >
      <div className="flex items-center gap-3">
        <span
          role="button"
          tabIndex={-1}
          aria-label={`Drag to reorder ${job.name}`}
          title="Drag to reorder (drags every selected row when this one is selected)"
          className="cursor-grab touch-none select-none px-0.5 text-neutral-400 active:cursor-grabbing"
          onPointerDown={(e) => onDragStart(e, index)}
          onClick={(e) => e.stopPropagation()}
        >
          ⠿
        </span>
        <span className="min-w-0 flex-1 truncate font-medium" title={job.file}>
          {job.name}
        </span>
        {job.profile && (
          <span className="rounded bg-neutral-200 px-1.5 py-0.5 text-xs uppercase dark:bg-neutral-700">{job.profile}</span>
        )}
        <span className="w-36 shrink-0 truncate text-neutral-500">{job.route ?? ""}</span>
        <span className="w-52 shrink-0 truncate text-right tabular-nums text-neutral-500" title={size.title}>
          {size.text}
        </span>
        <span className={`w-40 shrink-0 truncate ${stateColor(job)}`} title={job.message ?? undefined}>
          {statusLabel(job)}
        </span>
        <span className="flex gap-0.5" onClick={(e) => e.stopPropagation()}>
          <button
            type="button"
            className="px-1 disabled:opacity-30"
            aria-label="Move to top"
            title="Move to top"
            disabled={index === 0}
            onClick={() => moveJobs([job.id], "top")}
          >
            <PlaceIcon place="top" />
          </button>
          <button type="button" className="px-1 disabled:opacity-30" aria-label="Move up" title="Move up" disabled={index === 0} onClick={() => api.move(job.id, -1)}>
            ↑
          </button>
          <button
            type="button"
            className="px-1 disabled:opacity-30"
            aria-label="Move down"
            title="Move down"
            disabled={index === count - 1}
            onClick={() => api.move(job.id, 1)}
          >
            ↓
          </button>
          <button
            type="button"
            className="px-1 disabled:opacity-30"
            aria-label="Move to bottom"
            title="Move to bottom"
            disabled={index === count - 1}
            onClick={() => moveJobs([job.id], "bottom")}
          >
            <PlaceIcon place="bottom" />
          </button>
          {(job.state === "failed" || job.state === "cancelled") && (
            <button type="button" className="px-1" aria-label="Re-run" onClick={() => api.retry(job.id)}>
              ↻
            </button>
          )}
          <button type="button" className="px-1 disabled:opacity-30" aria-label="Remove" disabled={!idle} onClick={() => api.remove(job.id)}>
            ✕
          </button>
        </span>
      </div>
      {job.state === "running" && (
        <div className="mt-1.5 flex items-center gap-3">
          <div className="h-1.5 flex-1 overflow-hidden rounded bg-neutral-200 dark:bg-neutral-700">
            <div className="h-full bg-blue-600 transition-[width]" style={{ width: `${(f ?? 0) * 100}%` }} />
          </div>
          <span className="w-72 text-right text-xs tabular-nums text-neutral-500">
            {p?.chunk ? `chunk ${p.chunk[0]}/${p.chunk[1]} · ` : ""}
            {p ? `${p.framesDone}/${p.framesTotal} · ${p.fps.toFixed(1)} fps` : ""}
            {p?.etaSeconds != null ? ` · ETA ${duration(p.etaSeconds)}` : ""}
            {p?.projectedBytes ? ` · ~${bytes(p.projectedBytes)}` : ""}
          </span>
        </div>
      )}
      {job.state === "ripping" && (
        <div className="mt-1.5 flex items-center gap-3">
          <div className="h-1.5 flex-1 overflow-hidden rounded bg-neutral-200 dark:bg-neutral-700">
            <div className="h-full bg-blue-600 transition-[width]" style={{ width: `${(rf ?? 0) * 100}%` }} />
          </div>
          <span className="w-72 text-right text-xs tabular-nums text-neutral-500">{ripText(job)}</span>
        </div>
      )}
      {job.message && job.state !== "running" && (
        <p className="mt-1 truncate text-xs text-neutral-500" title={job.message}>
          {job.message}
        </p>
      )}
    </li>
  );
});

/** Scrolls the row for job `id` into view, if it's in the list. */
function revealRow(id: string) {
  const i = useQueue.getState().snapshot.jobs.findIndex((j) => j.id === id);
  if (i >= 0) document.querySelector(`[data-queue-index="${i}"]`)?.scrollIntoView({ block: "nearest" });
}

/** Whether a key press belongs to a text field or an open sheet rather than the queue. */
function keysElsewhere(e: KeyboardEvent) {
  const t = e.target as HTMLElement | null;
  if (t?.closest("input, textarea, select, [contenteditable=true], [role=dialog], [role=menu]")) return true;
  const s = useQueue.getState();
  return s.view !== "queue" || s.pendingAdd !== null || s.pendingEdit !== null || s.pendingDisc !== null;
}

/**
 * Queue shortcuts: ↑/↓ (and Home/End) pick the previous or next row, Ctrl/Cmd+A selects every
 * row, Esc clears the selection, and Alt+↑/↓/Home/End move the selected rows.
 */
function useQueueKeys() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || keysElsewhere(e)) return;
      const s = useQueue.getState();
      const order = s.snapshot.jobs.map((j) => j.id);
      if (order.length === 0) return;
      const moves: Record<string, Place> = { ArrowUp: "up", ArrowDown: "down", Home: "top", End: "bottom" };
      if (e.altKey && !e.ctrlKey && !e.metaKey && moves[e.key]) {
        e.preventDefault();
        moveJobs(targetsFor(), moves[e.key]);
        return;
      }
      if (e.altKey) return;
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "a") {
        e.preventDefault();
        s.selectAll();
        return;
      }
      if (e.ctrlKey || e.metaKey) return;
      if (e.key === "Escape" && (s.selectedId || s.checked.length > 0)) {
        e.preventDefault();
        s.select(null);
        return;
      }
      const at = s.selectedId ? order.indexOf(s.selectedId) : -1;
      const next =
        e.key === "ArrowDown"
          ? Math.min(order.length - 1, at + 1)
          : e.key === "ArrowUp"
            ? Math.max(0, at < 0 ? order.length - 1 : at - 1)
            : e.key === "Home"
              ? 0
              : e.key === "End"
                ? order.length - 1
                : null;
      if (next === null) return;
      e.preventDefault();
      if (e.shiftKey && s.selectedId) s.clickRow(order[next], "range");
      else s.select(order[next]);
      revealRow(order[next]);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

// How close to the list's top or bottom edge (px) a drag starts scrolling it, and how fast.
const EDGE = 56;
const MAX_SCROLL = 24;

export function QueueView() {
  const jobs = useQueue((s) => s.snapshot.jobs);
  const selected = useQueue((s) => s.snapshot.jobs.find((j) => j.id === s.selectedId) ?? null);
  const reveal = useQueue((s) => s.reveal);
  const [dragging, setDragging] = useState(false);
  const [rowDrag, setRowDrag] = useState<RowDrag | null>(null);
  const [menu, setMenu] = useState<MenuAt | null>(null);
  const drag = useRef<RowDrag | null>(null);
  const list = useRef<HTMLDivElement>(null);
  const badge = useRef<HTMLDivElement>(null);
  useQueueKeys();

  // Stable, so the memoised rows keep their props: the jobs are looked up when the drag starts
  const startRowDrag = useCallback((e: React.PointerEvent, index: number) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const { jobs } = useQueue.getState().snapshot;
    const ids = targetsFor(jobs[index].id);
    const d: RowDrag = { ids, over: index };
    drag.current = d;
    setRowDrag(d);
    let pointer = { x: e.clientX, y: e.clientY };
    const label = badge.current;
    if (label) {
      label.textContent = ids.length > 1 ? `${ids.length} jobs` : jobs[index].name;
      label.style.transform = `translate(${pointer.x + 14}px, ${pointer.y + 10}px)`;
      label.hidden = false;
    }
    const track = () => {
      const over = insertionAt(pointer.x, pointer.y);
      if (over !== null && drag.current && over !== drag.current.over) {
        drag.current = { ...drag.current, over };
        setRowDrag(drag.current);
      }
    };
    // Near the list's top or bottom edge the list scrolls, faster the closer the pointer is,
    // so a job can be dragged anywhere in a long queue
    let frame = 0;
    const scroll = () => {
      const el = list.current;
      if (el && drag.current) {
        const r = el.getBoundingClientRect();
        const step =
          pointer.y < r.top + EDGE
            ? -Math.min(MAX_SCROLL, Math.ceil(((r.top + EDGE - pointer.y) / EDGE) * MAX_SCROLL))
            : pointer.y > r.bottom - EDGE
              ? Math.min(MAX_SCROLL, Math.ceil(((pointer.y - (r.bottom - EDGE)) / EDGE) * MAX_SCROLL))
              : 0;
        if (step !== 0) {
          el.scrollTop += step;
          // Inside the list, so the row under the pointer counts even past the edge
          pointer = { x: pointer.x, y: Math.max(r.top + 1, Math.min(pointer.y, r.bottom - 1)) };
          track();
        }
      }
      frame = requestAnimationFrame(scroll);
    };
    frame = requestAnimationFrame(scroll);
    const move = (ev: PointerEvent) => {
      pointer = { x: ev.clientX, y: ev.clientY };
      if (label) label.style.transform = `translate(${pointer.x + 14}px, ${pointer.y + 10}px)`;
      track();
    };
    const up = () => {
      cancelAnimationFrame(frame);
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      window.removeEventListener("pointercancel", up);
      if (label) label.hidden = true;
      const d = drag.current;
      drag.current = null;
      setRowDrag(null);
      if (d) {
        const order = useQueue.getState().snapshot.jobs.map((j) => j.id);
        const to = dropIndexMany(order, d.ids, d.over);
        if (changesOrder(order, d.ids, to)) moveJobsTo(d.ids, to);
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
    window.addEventListener("pointercancel", up);
  }, []);

  // A right-click on a row outside the selection selects that row first, like a file list
  const openMenu = useCallback((e: React.MouseEvent, id: string) => {
    e.preventDefault();
    const s = useQueue.getState();
    if (!s.checked.includes(id)) s.select(id);
    setMenu({ id, x: e.clientX, y: e.clientY });
  }, []);
  const closeMenu = useCallback(() => setMenu(null), []);

  // Moved rows stay in view (Move to top from far down the list)
  useEffect(() => {
    if (!reveal) return;
    revealRow(reveal);
    useQueue.getState().setReveal(null);
  }, [jobs, reveal]);

  useEffect(() => {
    if (!isTauri()) return;
    const unlisten = getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === "enter" || e.payload.type === "over") setDragging(true);
      else setDragging(false);
      if (e.payload.type === "drop" && e.payload.paths.length > 0) useQueue.getState().setPendingAdd(e.payload.paths);
    });
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  return (
    <div className="flex h-full flex-col">
      <Toolbar />
      <SavingsBar jobs={jobs} />
      <BulkBar />
      <div className="flex min-h-0 flex-1">
        <div ref={list} className={`relative min-w-0 flex-1 overflow-auto ${dragging ? "ring-2 ring-inset ring-blue-500" : ""}`}>
          {jobs.length === 0 ? (
            <div className="m-4 flex h-[calc(100%-2rem)] flex-col items-center justify-center rounded-lg border-2 border-dashed border-neutral-300 text-neutral-500 dark:border-neutral-700">
              <p className="text-base font-medium">The queue is empty</p>
              <p className="mt-1 text-sm">Drop videos or folders here, or use Add files.</p>
            </div>
          ) : (
            <ul className={rowDrag ? "select-none" : undefined}>
              {jobs.map((j, i) => (
                <JobRow key={j.id} job={j} index={i} count={jobs.length} drag={rowDrag} onDragStart={startRowDrag} onMenu={openMenu} />
              ))}
            </ul>
          )}
        </div>
        {selected && <DetailPanel job={selected} />}
      </div>
      <div
        ref={badge}
        hidden
        aria-hidden="true"
        className="pointer-events-none fixed left-0 top-0 z-50 max-w-64 truncate rounded bg-blue-600 px-2 py-0.5 text-xs font-medium text-white shadow"
      />
      {menu && <RowMenu at={menu} onClose={closeMenu} />}
    </div>
  );
}
