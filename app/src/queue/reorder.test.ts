import { describe, expect, it } from "vitest";
import { dropIndex } from "./reorder";

describe("queue reordering", () => {
  it("lands where the line was drawn", () => {
    // Queue a b c d e; insertion points 0..5
    expect(dropIndex(4, 0)).toBe(0); // e to the top
    expect(dropIndex(0, 5)).toBe(4); // a to the bottom
    expect(dropIndex(1, 3)).toBe(2); // b below c
    expect(dropIndex(3, 1)).toBe(1); // d above b
    expect(dropIndex(2, 2)).toBe(2); // dropped on itself: no move
    expect(dropIndex(2, 3)).toBe(2); // just below itself: no move
  });
});
