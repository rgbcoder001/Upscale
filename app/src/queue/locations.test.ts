import { describe, expect, it } from "vitest";
import type { Job } from "../lib/api";
import { jobLocations } from "./locations";

const RIPS = "C:\\Work\\rips";
const LIB = "D:\\Library\\Lantern Street\\Season 1\\Lantern Street S01E07.mkv";

const job = (extra: Partial<Job>): Job =>
  ({ id: "j", file: "", name: "", group: "", input: "", options: {}, state: "queued", plan: [], ...extra }) as Job;

const rip = { source: "disc:0", index: 3, disc: "LANTERN_S1_D2", bytesTotal: 1, bytesDone: 0, mbPerS: 0, etaSeconds: null };

const summary = (j: Job) => jobLocations(j).map((p) => `${p.label}: ${p.text}${p.pending ? " (pending)" : ""}`);

describe("job locations", () => {
  it("a queued file: where it is and where it's saved", () => {
    expect(
      summary(
        job({
          file: "D:\\Shows\\Lantern Street S01E01.mkv",
          plannedOutput: "D:\\Upscaled\\Lantern Street S01E01.mkv",
          log: "D:\\Upscaled\\logs\\Lantern Street S01E01.log",
        }),
      ),
    ).toEqual([
      "Source: D:\\Shows\\Lantern Street S01E01.mkv",
      "Saves to: D:\\Upscaled\\Lantern Street S01E01.mkv (pending)",
      "Log: D:\\Upscaled\\logs\\Lantern Street S01E01.log",
    ]);
  });

  it("a finished file shows its output; Replace original says so", () => {
    expect(summary(job({ state: "done", file: "a.mkv", output: "b.mkv", plannedOutput: "b.mkv" }))).toEqual([
      "Source: a.mkv",
      "Output: b.mkv",
    ]);
    expect(summary(job({ file: "a.mkv", options: { ReplaceSource: true } }))).toEqual([
      "Source: a.mkv",
      "Saves to: In place of the source (Replace original) (pending)",
    ]);
  });

  it("a title being ripped: the disc, the folder it's ripping into, the raw rip and the encode", () => {
    expect(
      summary(
        job({
          state: "ripping",
          rip,
          input: RIPS,
          file: `${RIPS}\\Lantern Street S01E07.mkv`,
          plannedOutput: LIB,
          route: "Rip title 3 from LANTERN_S1_D2",
        }),
      ),
    ).toEqual([
      "Disc: LANTERN_S1_D2, title 3",
      "Ripping into: C:\\Work\\rips\\title-3",
      "Raw rip: C:\\Work\\rips\\Lantern Street S01E07.mkv (pending)",
      `Encoded to: ${LIB} (pending)`,
    ]);
  });

  it("rip only: saved to the library, then the raw rip is there", () => {
    const base = { rip, input: RIPS, file: `${RIPS}\\Lantern Street S01E07.mkv`, plannedOutput: LIB, route: "Rip only: title 3 from LANTERN_S1_D2" };
    expect(summary(job({ ...base, state: "waitingToRip" }))).toEqual(["Disc: LANTERN_S1_D2, title 3", `Saved to: ${LIB} (pending)`]);
    expect(summary(job({ ...base, rip: undefined, state: "done", file: LIB, output: LIB }))).toEqual([`Raw rip: ${LIB}`]);
  });
});
