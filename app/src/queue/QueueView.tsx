import { isTauri } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { api, type Job } from "../lib/api";
import { bytes, duration, fraction, statusLabel } from "../lib/format";
import { DetailPanel } from "./DetailPanel";
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
  const { jobs, running, paused, stopping, status } = useQueue((s) => s.snapshot);
  const runnable = jobs.some((j) => j.state === "queued" || j.state === "interrupted" || j.state === "analysing");
  const finished = jobs.some((j) => j.state === "done" || j.state === "skipped");
  return (
    <div className="flex flex-wrap items-center gap-1 border-b border-neutral-200 px-4 py-2 dark:border-neutral-700">
      <button type="button" className="rounded bg-blue-600 px-3 py-1.5 text-sm font-medium text-white" onClick={() => pick(false)}>
        Add files
      </button>
      <button type="button" className={button} onClick={() => pick(true)}>
        Add folder
      </button>
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
      <button type="button" className={button} disabled={!finished} onClick={() => api.clearFinished()}>
        Clear finished
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
      return "text-blue-700 dark:text-blue-300";
    default:
      return "text-neutral-500";
  }
}

function JobRow({ job, index, count }: { job: Job; index: number; count: number }) {
  const selected = useQueue((s) => s.selectedId === job.id);
  const f = fraction(job);
  const p = job.progress;
  const idle = job.state !== "running" && job.state !== "analysing";
  return (
    <li
      className={`cursor-pointer border-b border-neutral-200 px-4 py-2 text-sm dark:border-neutral-800 ${
        selected ? "bg-blue-50 dark:bg-blue-950/40" : "hover:bg-neutral-100 dark:hover:bg-neutral-800/60"
      }`}
      onClick={() => useQueue.getState().select(selected ? null : job.id)}
    >
      <div className="flex items-center gap-3">
        <span className="min-w-0 flex-1 truncate font-medium" title={job.file}>
          {job.name}
        </span>
        {job.profile && (
          <span className="rounded bg-neutral-200 px-1.5 py-0.5 text-xs uppercase dark:bg-neutral-700">{job.profile}</span>
        )}
        <span className="w-36 shrink-0 truncate text-neutral-500">{job.route ?? ""}</span>
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
            {p ? `${p.framesDone}/${p.framesTotal} · ${p.fps.toFixed(1)} fps` : ""}
            {p?.etaSeconds != null ? ` · ETA ${duration(p.etaSeconds)}` : ""}
            {p?.projectedBytes ? ` · ~${bytes(p.projectedBytes)}` : ""}
          </span>
        </div>
      )}
      {job.message && job.state !== "running" && <p className="mt-1 truncate text-xs text-neutral-500">{job.message}</p>}
    </li>
  );
}

export function QueueView() {
  const jobs = useQueue((s) => s.snapshot.jobs);
  const selected = useQueue((s) => s.snapshot.jobs.find((j) => j.id === s.selectedId) ?? null);
  const [dragging, setDragging] = useState(false);

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
                <JobRow key={j.id} job={j} index={i} count={jobs.length} />
              ))}
            </ul>
          )}
        </div>
        {selected && <DetailPanel job={selected} />}
      </div>
    </div>
  );
}
