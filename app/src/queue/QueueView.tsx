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
import { dropIndex, insertionAt } from "./reorder";
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
      <button type="button" className={button} onClick={() => useQueue.getState().select(null)}>
        Clear selection
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
  id: string;
  from: number;
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
}: {
  job: Job;
  index: number;
  count: number;
  drag: RowDrag | null;
  onDragStart: (e: React.PointerEvent, index: number) => void;
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
      } ${drag?.id === job.id ? "opacity-50" : ""} ${drag && drag.over === index ? "border-t-2 border-t-blue-600" : ""} ${
        drag && drag.over === count && index === count - 1 ? "border-b-2 border-b-blue-600" : ""
      }`}
      onClick={(e) =>
        !drag && useQueue.getState().clickRow(job.id, e.shiftKey ? "range" : e.ctrlKey || e.metaKey ? "toggle" : "single")
      }
    >
      <div className="flex items-center gap-3">
        <span
          role="button"
          tabIndex={-1}
          aria-label={`Drag to reorder ${job.name}`}
          title="Drag to reorder"
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
          <button type="button" className="px-1 disabled:opacity-30" aria-label="Move up" disabled={index === 0} onClick={() => api.move(job.id, -1)}>
            ↑
          </button>
          <button type="button" className="px-1 disabled:opacity-30" aria-label="Move down" disabled={index === count - 1} onClick={() => api.move(job.id, 1)}>
            ↓
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

export function QueueView() {
  const jobs = useQueue((s) => s.snapshot.jobs);
  const selected = useQueue((s) => s.snapshot.jobs.find((j) => j.id === s.selectedId) ?? null);
  const [dragging, setDragging] = useState(false);
  const [rowDrag, setRowDrag] = useState<RowDrag | null>(null);
  const drag = useRef<RowDrag | null>(null);
  // Stable, so the memoised rows keep their props: the job is looked up when the drag starts
  const startRowDrag = useCallback((e: React.PointerEvent, index: number) => {
    if (e.button !== 0) return;
    e.preventDefault();
    e.stopPropagation();
    const d = { id: useQueue.getState().snapshot.jobs[index].id, from: index, over: index };
    drag.current = d;
    setRowDrag(d);
    const move = (ev: PointerEvent) => {
      const over = insertionAt(ev.clientX, ev.clientY);
      if (over !== null && drag.current && over !== drag.current.over) {
        drag.current = { ...drag.current, over };
        setRowDrag(drag.current);
      }
    };
    const up = () => {
      window.removeEventListener("pointermove", move);
      window.removeEventListener("pointerup", up);
      const d = drag.current;
      drag.current = null;
      setRowDrag(null);
      if (d) {
        const to = dropIndex(d.from, d.over);
        if (to !== d.from) api.moveTo(d.id, to);
      }
    };
    window.addEventListener("pointermove", move);
    window.addEventListener("pointerup", up);
  }, []);

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
        <div className={`relative min-w-0 flex-1 overflow-auto ${dragging ? "ring-2 ring-inset ring-blue-500" : ""}`}>
          {jobs.length === 0 ? (
            <div className="m-4 flex h-[calc(100%-2rem)] flex-col items-center justify-center rounded-lg border-2 border-dashed border-neutral-300 text-neutral-500 dark:border-neutral-700">
              <p className="text-base font-medium">The queue is empty</p>
              <p className="mt-1 text-sm">Drop videos or folders here, or use Add files.</p>
            </div>
          ) : (
            <ul>
              {jobs.map((j, i) => (
                <JobRow key={j.id} job={j} index={i} count={jobs.length} drag={rowDrag} onDragStart={startRowDrag} />
              ))}
            </ul>
          )}
        </div>
        {selected && <DetailPanel job={selected} />}
      </div>
    </div>
  );
}
