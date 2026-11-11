import { describe, expect, it } from "vitest";
import { episodeNumbers, fileStem, gigabytes, minutes, queryFromDiscName, sharedName, shortPath } from "./names";

describe("rip names", () => {
  it("turns disc labels into search queries", () => {
    expect(queryFromDiscName("SHOW_ALPHA_D2")).toBe("Show Alpha");
    expect(queryFromDiscName("THE_SAMPLE_PICTURE")).toBe("The Sample Picture");
    expect(queryFromDiscName("SHOW_BETA_SEASON1_DISC_3")).toBe("Show Beta");
    expect(queryFromDiscName("Picture.BD")).toBe("Picture");
  });

  it("formats durations, sizes and paths", () => {
    expect(minutes(1430)).toBe("24 min");
    expect(minutes(9120)).toBe("2 h 32 min");
    expect(gigabytes(4.1 * 2 ** 30)).toBe("4.1 GB");
    expect(shortPath("/Volumes/zt6k/Anime Shows/Show Alpha/Season 1/x.mkv")).toBe("Show Alpha/Season 1/x.mkv");
  });

  it("numbers episodes, finds file names and names used twice", () => {
    const n = episodeNumbers(
      [
        { index: 4, double: false },
        { index: 2, double: true },
        { index: 7, double: false },
      ],
      7,
    );
    expect([...n]).toEqual([
      [4, 7],
      [2, 8],
      [7, 10],
    ]);
    expect(fileStem("D:\\L\\Season 1\\Show S01E03.mkv")).toBe("Show S01E03");
    expect(sharedName(["/L/a.mkv", "/L/b.mkv"])).toBeNull();
    expect(sharedName(["/L/A.mkv", "/L/b.mkv", "/L/a.mkv"])).toBe("a.mkv");
  });
});
