// The Add sheet's options, turned into job options (the script's setting names, see
// core/src/settings.rs). Anything left on "Auto" or blank is not set, so folder settings
// files and automatic analysis still decide it.

import { type OriginalFile, PROFILE_LABELS, type ProfileId } from "../lib/api";

/** Mirrors `DEFAULT_SVT_PARAMS` in core/src/settings.rs. */
export const DEFAULT_SVT_PARAMS =
  "enable-qm=1:qm-min=0:chroma-qm-min=4:enable-variance-boost=1:variance-boost-strength=3:adaptive-film-grain=1:film-grain=2";
/** Mirrors `LIVE_SVT_PARAMS` in core/src/settings.rs. */
export const LIVE_SVT_PARAMS =
  "enable-qm=1:qm-min=2:chroma-qm-min=8:qp-scale-compress-strength=1:variance-boost-strength=2:adaptive-film-grain=1:film-grain=4";

/** Profiles that upscale with Topaz (and take a `tvai_up` filter). */
export const usesTopaz = (p: ProfileId) => p === "anime-topaz" || p === "cg";
/** Profiles whose defaults are English audio and no subtitles. */
export const englishDefaults = (p: ProfileId) => p === "cg" || p === "live";
export const defaultCrf = (p: ProfileId) => (p === "live" ? 14 : 16);

/** The original-file choices, in the order the Add sheet and Settings show them. */
export const ORIGINAL_LABELS: Record<OriginalFile, string> = {
  keep: "Keep",
  delete: "Delete (Recycle Bin)",
  replace: "Replace with the new file",
};

export interface AddForm {
  profile: ProfileId;
  crf: string;
  preset: string;
  denoise: "auto" | "none" | "light" | "strong";
  native: "auto" | "off";
  topazFilter: string;
  filmGrain: boolean;
  defaultAudio: string;
  defaultSubtitles: string;
  testClip: boolean;
  /** Chunked encoding that survives closing the app. */
  resumable: boolean;
  /** After a checked encode: keep the source, delete it, or put the new file in its place. */
  original: OriginalFile;
  /** Extra settings as a JSON object, for power users. */
  advanced: string;
}

export function defaultForm(profile: ProfileId, original: OriginalFile = "keep"): AddForm {
  return {
    profile,
    crf: "",
    preset: "",
    denoise: "auto",
    native: "auto",
    topazFilter: "",
    filmGrain: true,
    defaultAudio: "",
    defaultSubtitles: "",
    testClip: false,
    resumable: true,
    original,
    advanced: "",
  };
}

/** SVT params without film-grain synthesis (what the script's -CompareGrain used). */
export function withoutGrain(params: string): string {
  const kept = params.split(":").filter((kv) => kv && !/^(adaptive-)?film-grain(-denoise)?=/.test(kv));
  return [...kept, "film-grain=0"].join(":");
}

const noise = { none: -1, light: 0, strong: 3 } as const;

/** Set by the engine on a ripped title; it keeps them when the job is edited. */
const RIP_KEYS = ["OutputDir", "DeleteSource", "SourceIsRip"];

/**
 * The form for a job's existing options (editing a queued job): the reverse of
 * `buildOptions`. Whatever the form has no field for goes to Advanced, so nothing is lost.
 * `fallback` is the profile when the options name none (a settings file decides it).
 */
export function formFromOptions(options: Record<string, unknown>, fallback: ProfileId): AddForm {
  const o = { ...options };
  if (o.SourceIsRip === true) for (const k of RIP_KEYS) delete o[k];
  const take = (k: string) => {
    const v = o[k];
    delete o[k];
    return v;
  };
  const content = o.Content;
  const profile = typeof content === "string" && content in PROFILE_LABELS ? (take("Content") as ProfileId) : fallback;
  const f = defaultForm(profile);
  if (Number.isInteger(o.Crf)) f.crf = String(take("Crf"));
  if (Number.isInteger(o.Preset)) f.preset = String(take("Preset"));
  if (profile === "anime") {
    const level = (Object.keys(noise) as (keyof typeof noise)[]).find((k) => noise[k] === o.Noise);
    if (level) {
      f.denoise = level;
      delete o.Noise;
    }
    if (o.NativeHeight === 0) {
      f.native = "off";
      delete o.NativeHeight;
    }
  } else if (usesTopaz(profile) && typeof o.TopazFilter === "string") {
    f.topazFilter = take("TopazFilter") as string;
  }
  if (o.SvtParams === withoutGrain(profile === "live" ? LIVE_SVT_PARAMS : DEFAULT_SVT_PARAMS)) {
    f.filmGrain = false;
    delete o.SvtParams;
  }
  if (typeof o.DefaultAudioLanguage === "string") f.defaultAudio = take("DefaultAudioLanguage") as string;
  if (typeof o.DefaultSubtitleLanguage === "string") f.defaultSubtitles = take("DefaultSubtitleLanguage") as string;
  if (o.ResumableEncoding === false) {
    f.resumable = false;
    delete o.ResumableEncoding;
  }
  if (o.ReplaceSource === true) {
    f.original = "replace";
    delete o.ReplaceSource;
  } else if (o.DeleteSource === true) {
    f.original = "delete";
    delete o.DeleteSource;
  }
  if (o.Frames === 720 && o.Start === -1) {
    f.testClip = true;
    delete o.Frames;
    delete o.Start;
  }
  if (Object.keys(o).length > 0) f.advanced = JSON.stringify(o);
  return f;
}

/** Throws with a readable message when the advanced JSON is invalid. */
export function buildOptions(f: AddForm): Record<string, unknown> {
  const o: Record<string, unknown> = { Content: f.profile };
  const int = (v: string, name: string) => {
    const n = Number(v);
    if (!Number.isInteger(n)) throw new Error(`${name} must be a whole number`);
    return n;
  };
  if (f.crf.trim()) o.Crf = int(f.crf, "CRF");
  if (f.preset.trim()) o.Preset = int(f.preset, "Preset");
  if (f.profile === "anime") {
    if (f.denoise !== "auto") o.Noise = noise[f.denoise];
    if (f.native === "off") o.NativeHeight = 0;
  } else if (usesTopaz(f.profile) && f.topazFilter.trim()) {
    o.TopazFilter = f.topazFilter.trim();
  }
  if (!f.filmGrain) o.SvtParams = withoutGrain(f.profile === "live" ? LIVE_SVT_PARAMS : DEFAULT_SVT_PARAMS);
  if (f.defaultAudio.trim()) o.DefaultAudioLanguage = f.defaultAudio.trim().toLowerCase();
  if (f.defaultSubtitles.trim()) o.DefaultSubtitleLanguage = f.defaultSubtitles.trim().toLowerCase();
  // On by default (core/src/settings.rs); only opting out is a job option
  if (!f.resumable) o.ResumableEncoding = false;
  if (f.original === "delete") o.DeleteSource = true;
  if (f.original === "replace") o.ReplaceSource = true;
  if (f.testClip) {
    o.Frames = 720;
    o.Start = -1;
  }
  if (f.advanced.trim()) {
    let extra: unknown;
    try {
      extra = JSON.parse(f.advanced);
    } catch {
      throw new Error("Advanced settings must be a JSON object");
    }
    if (typeof extra !== "object" || extra === null || Array.isArray(extra)) {
      throw new Error("Advanced settings must be a JSON object");
    }
    Object.assign(o, extra);
  }
  return o;
}
