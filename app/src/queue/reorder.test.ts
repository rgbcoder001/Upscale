import { describe, expect, it } from "vitest";
import { applyMove, changesOrder, dropIndex, dropIndexMany, inQueueOrder, placeIndex } from "./reorder";

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

const order = ["a", "b", "c", "d", "e", "f"];

describe("moving several jobs", () => {
  it("keeps the queue's order whatever order they were picked in", () => {
    expect(inQueueOrder(order, ["e", "b", "gone"])).toEqual(["b", "e"]);
    expect(applyMove(order, ["e", "b"], 0)).toEqual(["b", "e", "a", "c", "d", "f"]);
  });

  it("drops a selection where the line was drawn", () => {
    // b and d dragged to just above f (insertion point 5): four jobs stay before it
    const index = dropIndexMany(order, ["b", "d"], 5);
    expect(index).toBe(3);
    expect(applyMove(order, ["b", "d"], index)).toEqual(["a", "c", "e", "b", "d", "f"]);
    // Dropped inside itself: they close up, no further
    expect(applyMove(order, ["b", "c"], dropIndexMany(order, ["b", "c"], 2))).toEqual(order);
  });

  it("moves to the top and the bottom", () => {
    expect(applyMove(order, ["c", "e"], placeIndex(order, ["c", "e"], "top"))).toEqual(["c", "e", "a", "b", "d", "f"]);
    expect(applyMove(order, ["a", "c"], placeIndex(order, ["a", "c"], "bottom"))).toEqual(["b", "d", "e", "f", "a", "c"]);
  });

  it("moves up and down one place, closing up a scattered selection", () => {
    expect(applyMove(order, ["c", "d"], placeIndex(order, ["c", "d"], "up"))).toEqual(["a", "c", "d", "b", "e", "f"]);
    expect(applyMove(order, ["c", "d"], placeIndex(order, ["c", "d"], "down"))).toEqual(["a", "b", "e", "c", "d", "f"]);
    expect(applyMove(order, ["b", "e"], placeIndex(order, ["b", "e"], "up"))).toEqual(["b", "e", "a", "c", "d", "f"]);
    // Already at the edge: nothing changes
    expect(changesOrder(order, ["a", "b"], placeIndex(order, ["a", "b"], "up"))).toBe(false);
    expect(changesOrder(order, ["f"], placeIndex(order, ["f"], "down"))).toBe(false);
    expect(changesOrder(order, ["a", "c"], placeIndex(order, ["a", "c"], "top"))).toBe(true);
  });
});
