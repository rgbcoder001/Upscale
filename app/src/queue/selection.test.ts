import { describe, expect, it } from "vitest";
import { clickSelection, noSelection, pruneSelection } from "./selection";

const order = ["a", "b", "c", "d", "e"];

describe("queue selection", () => {
  it("a plain click picks one row, and a second click on it lets go", () => {
    const one = clickSelection(order, noSelection, "b", "single");
    expect(one).toEqual({ primary: "b", checked: ["b"] });
    expect(clickSelection(order, one, "c", "single")).toEqual({ primary: "c", checked: ["c"] });
    expect(clickSelection(order, one, "b", "single")).toEqual(noSelection);
  });

  it("a plain click on one of several selected rows keeps just that row", () => {
    const several = { primary: "b", checked: ["b", "c", "d"] };
    expect(clickSelection(order, several, "b", "single")).toEqual({ primary: "b", checked: ["b"] });
  });

  it("Ctrl-click adds and removes rows", () => {
    let s = clickSelection(order, noSelection, "b", "single");
    s = clickSelection(order, s, "d", "toggle");
    expect(s).toEqual({ primary: "d", checked: ["b", "d"] });
    s = clickSelection(order, s, "d", "toggle");
    expect(s).toEqual({ primary: "b", checked: ["b"] });
    s = clickSelection(order, s, "b", "toggle");
    expect(s).toEqual(noSelection);
  });

  it("Shift-click takes the rows in between, in either direction", () => {
    const from = clickSelection(order, noSelection, "d", "single");
    expect(clickSelection(order, from, "b", "range")).toEqual({ primary: "d", checked: ["b", "c", "d"] });
    expect(clickSelection(order, from, "e", "range")).toEqual({ primary: "d", checked: ["d", "e"] });
    // Nothing selected yet: just that row
    expect(clickSelection(order, noSelection, "c", "range")).toEqual({ primary: "c", checked: ["c"] });
  });

  it("forgets rows that left the queue", () => {
    const s = { primary: "b", checked: ["a", "b", "c"] };
    expect(pruneSelection(order, s)).toBe(s);
    expect(pruneSelection(["a", "c"], s)).toEqual({ primary: "c", checked: ["a", "c"] });
    expect(pruneSelection([], s)).toEqual(noSelection);
  });
});
