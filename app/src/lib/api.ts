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
  /** A disc is being ripped. */
  ripping?: boolean;
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
  /** Jellyfin library folders ripped discs go into. */
  libraryFolders?: LibraryFolder[];
  /** TMDB API Read Access Token; empty = names are typed by hand. */
  tmdbToken?: string;
  /** Languages a rip keeps by default (ISO 639-2). */
  keepLanguages?: string[];
  /** The user approved installing MakeMKV (not notarized by Apple) in Settings → Ripping. */
  makemkvApproved?: boolean;
}

export type LibraryKind = "movies" | "shows";

/** MakeMKV's trial or registration, as last seen (core/src/tools/makemkv.rs). */
export type MakemkvStatus =
  | { state: "unknown" }
  | { state: "trial"; daysLeft: number; days: number }
  | { state: "registered"; since?: string }
  | { state: "expired"; message: string };

export interface LibraryFolder {
  /** The category's name ("Anime Shows"); empty = the folder's own name. */
  name?: string;
  path: string;
  /** How files in it are named. */
  kind: LibraryKind;
}

/** A library folder's name, or its folder's name when it has none. */
export function folderLabel(f: LibraryFolder): string {
  return f.name?.trim() || f.path.split(/[\\/]/).filter(Boolean).pop() || f.path;
}

// Disc ripping (core/src/disc.rs, library.rs, tmdb.rs; RIPPING.md)

export type TrackKind = "video" | "audio" | "subtitle";

export interface DiscTitle {
  index: number;
  seconds: number;
  bytes: number;
  chapters: number;
  source: string;
  segments: number[];
  tracks: { kind: TrackKind; language: string }[];
}

export interface Disc {
  name: string;
  titles: DiscTitle[];
  /** What MakeMKV opens it as (`dev:/dev/rdisk8`). */
  source?: string;
}

export type DiscKind = "movie" | "episodes" | "unknown";
export type Role = "main" | "alternateCut" | "episode" | "doubleEpisode" | "playAll" | "duplicate" | "extra";

export interface Pick {
  index: number;
  role: Role;
  ticked: boolean;
  reason: string;
}

export interface DiscChoice {
  selection: { kind: DiscKind; summary: string; picks: Pick[] };
  languages: { code: string; name: string }[];
}

export interface TmdbMatch {
  id: number;
  kind: "movie" | "show";
  name: string;
  year: number | null;
  originalLanguage: string;
  overview: string;
}

export interface TmdbSeason {
  number: number;
  episodes: number;
}

export interface TmdbEpisode {
  number: number;
  name: string;
  runtime: number | null;
}

export interface ShowInfo {
  layout: { dir: string; exists: boolean; prefix: string; last: [number, number] | null; folders?: [string, number, number, number][] };
  season: number;
  episode: number;
}

/** What the Rip sheet starts (core/src/rip.rs). */
export interface RipRequest {
  source: string;
  titles: { index: number; target: string; bytes: number }[];
  options: Record<string, unknown>;
}

export interface NameRequest {
  root: string;
  kind: LibraryKind;
  name: string;
  year: number | null;
  season: number;
  firstEpisode: number;
  titles: { index: number; double: boolean }[];
  episodeNames: [number, string][];
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
  | { type: "log"; jobId: string; line: string }
  | { type: "disc"; disc: Disc }
  | { type: "discRemoved" };

export const emptySnapshot: QueueSnapshot = { jobs: [], running: false, paused: false, stopping: false, status: "", ripping: false };

/** Canned answers for a plain browser with `?demo=rip` (src/rip/demo.ts), to try the Rip sheet without Tauri. */
let demo: ((cmd: string, args: Record<string, unknown> | undefined) => unknown) | null = null;
export function setDemo(handler: typeof demo) {
  demo = handler;
}

async function call<T>(cmd: string, args: Record<string, unknown> | undefined, fallback: T): Promise<T> {
  if (!isTauri()) {
    const answer = demo?.(cmd, args);
    return answer === undefined ? fallback : (answer as T);
  }
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
  makemkvInstall: () => call<string[]>("makemkv_install", undefined, []),
  makemkvStatus: () => call<MakemkvStatus>("makemkv_status", undefined, { state: "unknown" }),
  makemkvCheck: () => call<MakemkvStatus>("makemkv_check", undefined, { state: "unknown" }),
  makemkvRegister: (key: string) => call<string[]>("makemkv_register", { key }, []),
  ripStart: (request: RipRequest) => call("rip_start", { request }, undefined),
  ripCancel: () => call("rip_cancel", undefined, undefined),
  discCurrent: () => call<Disc | null>("disc_current", undefined, null),
  discChoose: (disc: Disc) => call<DiscChoice | null>("disc_choose", { disc }, null),
  tmdbSearch: (query: string) => call<TmdbMatch[]>("tmdb_search", { query }, []),
  tmdbShow: (id: number) => call<TmdbSeason[]>("tmdb_show", { id }, []),
  tmdbSeason: (id: number, season: number) => call<TmdbEpisode[]>("tmdb_season", { id, season }, []),
  libraryShow: (root: string, name: string, year: number | null, seasons: TmdbSeason[]) =>
    call<ShowInfo | null>("library_show", { root, name, year, seasons }, null),
  libraryNames: (request: NameRequest) => call<{ index: number; path: string }[]>("library_names", { request }, []),
};

export async function onEngineEvent(handler: (e: EngineEvent) => void): Promise<UnlistenFn> {
  if (!isTauri()) return () => {};
  return listen<EngineEvent>("engine", (e) => handler(e.payload));
}
