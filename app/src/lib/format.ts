import type { Job, JobState, Stage } from "./api";

/** `1:02:03` or `4:05`. */
export function duration(seconds: number | null | undefined): string {
  if (seconds == null || !Number.isFinite(seconds)) return "";
  const s = Math.max(0, Math.round(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  const sec = String(s % 60).padStart(2, "0");
  return h > 0 ? `${h}:${String(m).padStart(2, "0")}:${sec}` : `${m}:${sec}`;
}

/** `1.4 GB`, `820 MB`. */
export function bytes(n: number | null | undefined): string {
  if (n == null || n <= 0) return "";
  const gb = n / 2 ** 30;
  if (gb >= 1) return `${gb.toFixed(1)} GB`;
  return `${Math.round(n / 2 ** 20)} MB`;
}

/** `-68%`: the change from `before` to `after`. */
export function percentChange(before: number | null | undefined, after: number | null | undefined): string {
  if (!before || !after) return "";
  const pct = Math.round(((after - before) / before) * 100);
  return `${pct > 0 ? "+" : pct < 0 ? "−" : ""}${Math.abs(pct)}%`;
}

/** The size column: the source, then the output (or the running encode's estimate, `~`). */
export function sizeSummary(job: Job): { text: string; title: string } {
  const src = job.sourceBytes;
  if (!src) return { text: "", title: "" };
  const done = job.state === "done" ? job.outputBytes : null;
  const projected = job.state === "running" ? job.progress?.projectedBytes : null;
  const out = done ?? projected;
  if (!out) return { text: bytes(src), title: "Original size" };
  const est = done ? "" : "~";
  return {
    text: `${bytes(src)} → ${est}${bytes(out)} (${percentChange(src, out)})`,
    title: done ? "Original → new size" : "Original → estimated new size",
  };
}

/** Totals over the finished jobs that have both sizes. */
export function savings(jobs: Job[]): { done: number; before: number; after: number } {
  let done = 0;
  let before = 0;
  let after = 0;
  for (const j of jobs) {
    if (j.state === "done" && j.sourceBytes && j.outputBytes) {
      done += 1;
      before += j.sourceBytes;
      after += j.outputBytes;
    }
  }
  return { done, before, after };
}

/** 0–1 progress of a job's current video stage, or null when unknown. */
export function fraction(job: Job): number | null {
  const p = job.progress;
  if (!p || p.framesTotal <= 0) return null;
  return Math.min(1, p.framesDone / p.framesTotal);
}

const stageLabels: Record<Stage, string> = {
  reading: "Reading source",
  scenes: "Finding scene cuts",
  video: "Upscaling + encoding",
  joining: "Joining chunks",
  verify: "Verifying",
  audio: "Audio",
  muxing: "Muxing",
  compare: "Screenshots",
};

const stateLabels: Record<JobState, string> = {
  analysing: "Analysing",
  queued: "Queued",
  running: "Running",
  done: "Done",
  failed: "Failed",
  skipped: "Skipped",
  cancelled: "Cancelled",
  interrupted: "Interrupted",
  waitingToRip: "Waiting to rip",
  ripping: "Ripping",
};

/** 0–1 of a disc title's rip, or null when it isn't ripping. */
export function ripFraction(job: Job): number | null {
  const r = job.rip;
  if (job.state !== "ripping" || !r || r.bytesTotal <= 0) return null;
  return Math.min(1, r.bytesDone / r.bytesTotal);
}

/** `12.3 of 29.9 GB · 20 MB/s · 14 min left` for a ripping title. */
export function ripText(job: Job): string {
  const r = job.rip;
  if (!r) return "";
  const gb = (n: number) => (n / 2 ** 30).toFixed(1);
  const parts = [`${gb(r.bytesDone)} of ${gb(r.bytesTotal)} GB`];
  if (r.mbPerS > 0) parts.push(`${r.mbPerS.toFixed(0)} MB/s`);
  if (r.etaSeconds != null) parts.push(`${duration(r.etaSeconds)} left`);
  return parts.join(" · ");
}

/** What the row shows as the job's status. */
export function statusLabel(job: Job): string {
  if (job.state === "running") {
    if (job.progress?.paused) return "Paused";
    return job.stage ? stageLabels[job.stage] : "Starting";
  }
  return stateLabels[job.state];
}

/** Progress over the whole queue: finished jobs plus the running one's fraction. */
export function overall(jobs: Job[]): number | null {
  const counted = jobs.filter((j) => j.state !== "skipped" && j.state !== "failed" && j.state !== "cancelled");
  if (counted.length === 0) return null;
  let done = 0;
  for (const j of counted) {
    if (j.state === "done") done += 1;
    else if (j.state === "running") done += fraction(j) ?? 0;
  }
  return done / counted.length;
}
