import { describe, expect, it } from "vitest";
import { buildOptions, defaultForm, withoutGrain } from "../queue/options";
import type { Job } from "./api";
import { bytes, duration, overall, percentChange, savings, sizeSummary, statusLabel } from "./format";

const job = (state: Job["state"], extra: Partial<Job> = {}): Job =>
  ({ id: state, file: "", name: "", group: "", input: "", options: {}, state, plan: [], ...extra }) as Job;

describe("format", () => {
  it("durations and sizes", () => {
    expect(duration(31)).toBe("0:31");
    expect(duration(3723)).toBe("1:02:03");
    expect(duration(undefined)).toBe("");
    expect(bytes(5 * 2 ** 30)).toBe("5.0 GB");
    expect(bytes(820 * 2 ** 20)).toBe("820 MB");
  });

  it("sizes before and after", () => {
    const GB = 2 ** 30;
    expect(percentChange(6.37 * GB, 2.02 * GB)).toBe("−68%");
    expect(sizeSummary(job("queued", { sourceBytes: 6.37 * GB })).text).toBe("6.4 GB");
    expect(sizeSummary(job("done", { sourceBytes: 6.37 * GB, outputBytes: 2.02 * GB })).text).toBe(
      "6.4 GB → 2.0 GB (−68%)",
    );
    const running = job("running", {
      sourceBytes: 6.37 * GB,
      progress: { framesDone: 1, framesTotal: 2, fps: 4, paused: false, projectedBytes: 2 * GB },
    });
    expect(sizeSummary(running).text).toBe("6.4 GB → ~2.0 GB (−69%)");
    expect(
      savings([
        job("done", { sourceBytes: 6 * GB, outputBytes: 2 * GB }),
        job("done", { sourceBytes: 6 * GB, outputBytes: 2 * GB }),
        job("queued", { sourceBytes: 6 * GB }),
      ]),
    ).toEqual({ done: 2, before: 12 * GB, after: 4 * GB });
  });

  it("status and overall progress", () => {
    const running = job("running", {
      stage: "video",
      progress: { framesDone: 50, framesTotal: 100, fps: 1, paused: false },
    });
    expect(statusLabel(running)).toBe("Upscaling + encoding");
    expect(statusLabel({ ...running, progress: { ...running.progress!, paused: true } })).toBe("Paused");
    expect(overall([job("done"), running, job("queued"), job("failed")])).toBeCloseTo(1.5 / 3);
  });
});

describe("Add sheet options", () => {
  it("leaves automatic settings unset", () => {
    expect(buildOptions(defaultForm("anime"))).toEqual({ Content: "anime" });
  });

  it("maps every option to a script setting", () => {
    const o = buildOptions({
      ...defaultForm("anime"),
      crf: "12",
      preset: "4",
      denoise: "light",
      native: "off",
      filmGrain: false,
      defaultSubtitles: "None",
      testClip: true,
      resumable: false,
      deleteSource: true,
      advanced: '{"Temporal": true}',
    });
    expect(o).toEqual({
      Content: "anime",
      Crf: 12,
      Preset: 4,
      Noise: 0,
      NativeHeight: 0,
      SvtParams: "enable-qm=1:qm-min=0:chroma-qm-min=4:enable-variance-boost=1:variance-boost-strength=3:film-grain=0",
      DefaultSubtitleLanguage: "none",
      ResumableEncoding: false,
      DeleteSource: true,
      Frames: 720,
      Start: -1,
      Temporal: true,
    });
  });

  it("CG takes a Topaz filter and rejects bad input", () => {
    expect(buildOptions({ ...defaultForm("cg"), topazFilter: " model=prob-4:x=1 ", denoise: "strong" })).toEqual({
      Content: "cg",
      TopazFilter: "model=prob-4:x=1",
    });
    expect(() => buildOptions({ ...defaultForm("cg"), crf: "1.5" })).toThrow("CRF must be a whole number");
    expect(() => buildOptions({ ...defaultForm("cg"), advanced: "[1]" })).toThrow("JSON object");
  });

  it("Anime AV1 and live action take no upscaler options", () => {
    const extras = { topazFilter: "model=prob-4", denoise: "strong", native: "off" } as const;
    expect(buildOptions({ ...defaultForm("anime-av1"), ...extras })).toEqual({ Content: "anime-av1" });
    expect(buildOptions({ ...defaultForm("live"), ...extras })).toEqual({ Content: "live" });
  });

  it("turning film grain off keeps the live-action settings", () => {
    expect(buildOptions({ ...defaultForm("live"), filmGrain: false })).toEqual({
      Content: "live",
      SvtParams:
        "enable-qm=1:qm-min=2:chroma-qm-min=8:qp-scale-compress-strength=1:variance-boost-strength=2:film-grain=0",
    });
  });

  it("Anime (Topaz) takes a Topaz filter, not the vs-mlrt options", () => {
    expect(
      buildOptions({ ...defaultForm("anime-topaz"), topazFilter: "model=prob-4", denoise: "strong", native: "off" }),
    ).toEqual({ Content: "anime-topaz", TopazFilter: "model=prob-4" });
  });

  it("removes film grain like the script's grain A/B", () => {
    expect(withoutGrain("a=1:film-grain=8:film-grain-denoise=0:adaptive-film-grain=1")).toBe("a=1:film-grain=0");
  });
});
