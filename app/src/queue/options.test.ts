import { describe, expect, it } from "vitest";
import { buildOptions, DEFAULT_SVT_PARAMS, formFromOptions, LIVE_SVT_PARAMS, withoutGrain } from "./options";

describe("editing a job's options", () => {
  it("the form built from options gives the same options back", () => {
    const cases: Record<string, unknown>[] = [
      { Content: "anime-topaz" },
      { Content: "anime", Crf: 18, Preset: 4, Noise: 0, NativeHeight: 0 },
      { Content: "cg", TopazFilter: "model=prob-4:scale=0", DefaultAudioLanguage: "jpn", DefaultSubtitleLanguage: "eng" },
      { Content: "anime-av1", SvtParams: withoutGrain(DEFAULT_SVT_PARAMS), ResumableEncoding: false, ReplaceSource: true },
      { Content: "live", SvtParams: withoutGrain(LIVE_SVT_PARAMS), DeleteSource: true },
      { Content: "anime-topaz", Frames: 720, Start: -1 },
      // No field for these: they travel through Advanced
      { Content: "anime", Temporal: true, SvtParams: "film-grain=8", Noise: 2, Frames: 240 },
    ];
    for (const options of cases) {
      expect(buildOptions(formFromOptions(options, "anime-topaz")), JSON.stringify(options)).toEqual(options);
    }
  });

  it("fills the fields the Add sheet has", () => {
    const f = formFromOptions({ Content: "anime", Crf: 18, Noise: 3, ReplaceSource: true, Temporal: true }, "cg");
    expect(f).toMatchObject({ profile: "anime", crf: "18", denoise: "strong", original: "replace", advanced: '{"Temporal":true}' });
    // Without a Content option a settings file decides the profile: the fallback is shown
    expect(formFromOptions({}, "cg").profile).toBe("cg");
    expect(buildOptions(formFromOptions({ Content: "nonsense" }, "cg"))).toEqual({ Content: "nonsense" });
  });

  it("leaves a ripped title's own settings to the engine", () => {
    const rip = { Content: "anime-topaz", OutputDir: "D:\\\\h2tj\\\\Show\\\\Season 1", DeleteSource: true, SourceIsRip: true, Crf: 15 };
    const f = formFromOptions(rip, "anime-topaz");
    expect(f).toMatchObject({ original: "keep", advanced: "", crf: "15" });
    expect(buildOptions(f)).toEqual({ Content: "anime-topaz", Crf: 15 });
  });
});
