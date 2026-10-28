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
};

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
