// Typed wrappers around the Tauri commands in src-tauri/src/lib.rs, and the shapes of
// upscale-core's queue types (core/src/queue.rs). Outside Tauri (tests, a plain browser)
// every call resolves to an empty value.

import { invoke, isTauri } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export type ProfileId = "anime-topaz" | "anime" | "anime-av1" | "cg" | "live";

/** Button labels, in the order the Add sheet shows them (the default first). */
export const PROFILE_LABELS: Record<ProfileId, string> = {
  "anime-topaz": "Anime (Topaz)",
  anime: "Anime",
  "anime-av1": "Anime AV1",
  cg: "CG",
  live: "Live action",
};

export type JobState =
  | "analysing"
  | "queued"
  | "running"
  | "done"
  | "failed"
  | "skipped"
  | "cancelled"
  | "interrupted";

export type Stage = "reading" | "scenes" | "video" | "joining" | "verify" | "audio" | "muxing" | "compare";

export interface JobProgress {
  framesDone: number;
  framesTotal: number;
  fps: number;
  etaSeconds?: number | null;
  projectedBytes?: number | null;
  paused: boolean;
  /** Chunked encodes: [chunk, of]. */
  chunk?: [number, number] | null;
}

export interface Job {
  id: string;
  file: string;
  name: string;
  group: string;
  input: string;
  options: Record<string, unknown>;
  state: JobState;
  profile?: ProfileId | null;
  route?: string | null;
  plan: string[];
  stage?: Stage;
  progress?: JobProgress;
  output?: string | null;
  message?: string | null;
  log?: string | null;
  /** Size of the source file. */
  sourceBytes?: number | null;
  /** Size of the finished output file. */
  outputBytes?: number | null;
}

export interface QueueSnapshot {
  jobs: Job[];
  running: boolean;
  paused: boolean;
  stopping: boolean;
  status: string;
}

export interface AppSettings {
  outputDir: string;
  toolsDir: string;
  lastProfile: ProfileId;
  /** Chunks and temporary audio; null = <output>\.work. */
  tempDir?: string | null;
  /** Tools from PATH, installed system-wide (the script's way), instead of the private folder. */
  useSystemTools?: boolean;
  /** What the Add sheet starts with for the original file. */
  originalFile?: OriginalFile;
}

/** After a checked encode: keep the source, delete it, or put the new file in its place. */
export type OriginalFile = "keep" | "delete" | "replace";

export interface ToolStatus {
  name: string;
  version?: string | null;
  path?: string | null;
  ok: boolean;
  note: string;
}

export interface AppInfo {
  coreVersion: string;
  gpu?: string | null;
}

export type EngineEvent =
  | { type: "queue"; snapshot: QueueSnapshot }
  | { type: "job"; job: Job }
  | { type: "status"; status: string }
  | { type: "log"; jobId: string; line: string };

export const emptySnapshot: QueueSnapshot = { jobs: [], running: false, paused: false, stopping: false, status: "" };

async function call<T>(cmd: string, args: Record<string, unknown> | undefined, fallback: T): Promise<T> {
  if (!isTauri()) return fallback;
  return invoke<T>(cmd, args);
}

export const api = {
  appInfo: () => call<AppInfo | null>("app_info", undefined, null),
  snapshot: () => call("queue_snapshot", undefined, emptySnapshot),
  add: (paths: string[], options: Record<string, unknown>) => call("queue_add", { paths, options }, 0),
  remove: (id: string) => call("queue_remove", { id }, undefined),
  move: (id: string, delta: number) => call("queue_move", { id, delta }, undefined),
  retry: (id: string) => call("queue_retry", { id }, undefined),
  clearFinished: () => call("queue_clear_finished", undefined, undefined),
  start: () => call("queue_start", undefined, undefined),
  pause: () => call("queue_pause", undefined, undefined),
  resume: () => call("queue_resume", undefined, undefined),
  stopAfter: (on: boolean) => call("queue_stop_after", { on }, undefined),
  cancel: () => call("queue_cancel", undefined, undefined),
  jobLog: (id: string) => call("job_log", { id }, ""),
  compareImages: (id: string) => call<string[]>("job_compare_images", { id }, []),
  settings: () => call<AppSettings | null>("settings_get", undefined, null),
  setSettings: (settings: AppSettings) => call("settings_set", { settings }, undefined),
  toolsStatus: () => call<ToolStatus[]>("tools_status", undefined, []),
  toolsUpdate: () => call<string[]>("tools_update", undefined, []),
  showInFolder: (path: string) => call("show_in_folder", { path }, undefined),
  openPath: (path: string) => call("open_path", { path }, undefined),
};

export async function onEngineEvent(handler: (e: EngineEvent) => void): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<EngineEvent>("engine", (e) => handler(e.payload));
}
