import { describe, expect, it } from "vitest";
import { gigabytes, minutes, queryFromDiscName, shortPath } from "./names";

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
});
