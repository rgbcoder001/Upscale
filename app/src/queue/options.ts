// The Add sheet's options, turned into job options (the script's setting names, see
// core/src/settings.rs). Anything left on "Auto" or blank is not set, so folder settings
// files and automatic analysis still decide it.

import type { ProfileId } from "../lib/api";

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
  original: "keep" | "delete" | "replace";
  /** Extra settings as a JSON object, for power users. */
  advanced: string;
}

export function defaultForm(profile: ProfileId): AddForm {
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
    original: "keep",
    advanced: "",
  };
}

/** SVT params without film-grain synthesis (what the script's -CompareGrain used). */
export function withoutGrain(params: string): string {
  const kept = params.split(":").filter((kv) => kv && !/^(adaptive-)?film-grain(-denoise)?=/.test(kv));
  return [...kept, "film-grain=0"].join(":");
}

const noise = { none: -1, light: 0, strong: 3 } as const;

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
