import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { api, emptySnapshot, setDemo } from "../lib/api";
import { useQueue } from "../queue/store";
import { demoDisc, enableDemo } from "./demo";

describe("Rip sheet", () => {
  afterEach(() => {
    setDemo(null);
    vi.restoreAllMocks();
    useQueue.setState({ snapshot: emptySnapshot, pendingDisc: null, currentDisc: null });
  });

  it("opens for a disc with the episodes ticked, a name and the next episode", async () => {
    enableDemo();
    render(<App />);
    act(() => useQueue.getState().setPendingDisc(demoDisc));
    const sheet = await screen.findByRole("dialog", { name: "Rip disc" });
    expect(within(sheet).getByText(/3 episodes of about 24 min, plus 1 double-length/)).toBeInTheDocument();

    // Episodes and the double episode are ticked; "play all" and the extra aren't
    const box = (i: number) => within(sheet).getByRole("checkbox", { name: `Title ${i}` });
    expect([1, 2, 3, 4].map((i) => (box(i) as HTMLInputElement).checked)).toEqual([true, true, true, true]);
    expect([0, 5].map((i) => (box(i) as HTMLInputElement).checked)).toEqual([false, false]);

    // The disc label becomes the name; the library suggests episode 7
    expect(within(sheet).getByRole("textbox", { name: "Name" })).toHaveValue("Show Alpha");
    expect(await within(sheet).findByRole("textbox", { name: /First episode/ })).toHaveValue("7");
    expect(await within(sheet).findByText(/Show Alpha S01E09-E10\.mkv/)).toBeInTheDocument();

    // English, Japanese and Spanish are on by default; Chinese is one click away
    const chip = (name: string) => within(sheet).getByRole("button", { name });
    expect(["English", "Japanese", "Spanish", "Chinese"].map((n) => chip(n).getAttribute("aria-pressed"))).toEqual([
      "true",
      "true",
      "true",
      "false",
    ]);
    fireEvent.click(chip("Chinese"));
    expect(chip("Chinese")).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(chip("Keep all"));
    expect(chip("Keep all")).toHaveAttribute("aria-pressed", "true");

    // Unticking a title renumbers the rest
    fireEvent.click(box(2));
    expect(await within(sheet).findByText(/Show Alpha S01E08-E09\.mkv/)).toBeInTheDocument();

    // Rip sends the ticked titles in order, each with its library file, the profile and languages
    const start = vi.spyOn(api, "ripStart").mockResolvedValue(undefined);
    fireEvent.click(within(sheet).getByRole("button", { name: "Rip 3 titles" }));
    expect(start).toHaveBeenCalledTimes(1);
    const request = start.mock.calls[0][0];
    expect(request.titles.map((t) => [t.index, t.target.split("/").pop()])).toEqual([
      [1, "Show Alpha S01E07.mkv"],
      [3, "Show Alpha S01E08-E09.mkv"],
      [4, "Show Alpha S01E10.mkv"],
    ]);
    expect(request.titles[0].bytes).toBeGreaterThan(0);
    expect(request.options).toEqual({ Content: "anime-av1", KeepLanguages: ["all"] });
    expect(await screen.findByRole("button", { name: "Add files" })).toBeInTheDocument();
    expect(screen.queryByRole("dialog", { name: "Rip disc" })).toBeNull();
  });

  it("a TMDB match fills the name and year", async () => {
    enableDemo();
    render(<App />);
    act(() => useQueue.getState().setPendingDisc(demoDisc));
    const sheet = await screen.findByRole("dialog", { name: "Rip disc" });
    const results = await within(sheet).findByRole("list", { name: "Search results" }, { timeout: 2000 });
    fireEvent.click(within(results).getByRole("button", { name: /Show Alpha \(1992\) · TV/ }));
    expect(within(sheet).getByRole("textbox", { name: "Year" })).toHaveValue("1992");
  });

  it("an inserted disc opens the sheet; ejecting it closes it", async () => {
    enableDemo();
    render(<App />);
    act(() => useQueue.getState().apply({ type: "disc", disc: demoDisc }));
    expect(await screen.findByRole("dialog", { name: "Rip disc" })).toBeInTheDocument();
    // Closed, it can be reopened from the queue while the disc is in
    fireEvent.click(screen.getByRole("button", { name: "Close" }));
    fireEvent.click(screen.getByRole("button", { name: "Rip disc" }));
    expect(await screen.findByRole("dialog", { name: "Rip disc" })).toBeInTheDocument();
    act(() => useQueue.getState().apply({ type: "discRemoved" }));
    expect(screen.queryByRole("dialog", { name: "Rip disc" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Rip disc" })).toBeNull();
  });
});
