import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";
import App from "./App";
import { api, emptySnapshot, type Job } from "./lib/api";
import { useQueue } from "./queue/store";

function job(over: Partial<Job>): Job {
  return {
    id: "j1",
    file: "C:\\k3wz\\ep01.mkv",
    name: "ep01.mkv",
    group: "g",
    input: "C:\\k3wz",
    options: {},
    state: "queued",
    plan: ["ep01.mkv", "  Route:     upscale  1920x1080 h264 23.976 fps, SAR 1:1, matrix 709"],
    profile: "anime",
    route: "Upscale 1080p → 2160p",
    ...over,
  };
}

describe("App", () => {
  beforeEach(() =>
    useQueue.setState({ snapshot: emptySnapshot, selectedId: null, checked: [], view: "queue", pendingAdd: null, pendingEdit: null }),
  );

  it("opens on an empty queue", () => {
    render(<App />);
    expect(screen.getByRole("tab", { name: "Queue" })).toHaveAttribute("aria-selected", "true");
    expect(screen.getByText("The queue is empty")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start" })).toBeDisabled();
  });

  it("shows a running job's progress and its plan", () => {
    render(<App />);
    act(() =>
      useQueue.getState().apply({
        type: "queue",
        snapshot: {
          ...emptySnapshot,
          running: true,
          jobs: [
            job({
              state: "running",
              stage: "video",
              progress: {
                framesDone: 360,
                framesTotal: 720,
                fps: 11.5,
                etaSeconds: 31,
                projectedBytes: 5 * 2 ** 30,
                paused: false,
                chunk: [2, 3],
              },
            }),
          ],
        },
      }),
    );
    expect(screen.getByText("Upscaling + encoding")).toBeInTheDocument();
    expect(screen.getByText(/chunk 2\/3 · 360\/720 · 11\.5 fps · ETA 0:31 · ~5\.0 GB/)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Pause" })).toBeEnabled();

    fireEvent.click(screen.getByText("ep01.mkv"));
    expect(screen.getByRole("complementary", { name: "File detail" })).toHaveTextContent(/Route:\s+upscale/);
  });

  it("offers a Compare tab for finished files", async () => {
    render(<App />);
    // Let the startup snapshot load first, or it replaces the queue below
    await act(async () => {});
    act(() =>
      useQueue.getState().apply({
        type: "queue",
        snapshot: { ...emptySnapshot, jobs: [job({ state: "done", output: "C:\\h2tj\\ep01.mkv" })] },
      }),
    );
    fireEvent.click(screen.getByText("ep01.mkv"));
    fireEvent.click(screen.getByRole("button", { name: "Compare" }));
    // Outside Tauri there are no images
    expect(await screen.findByText(/No comparison images/)).toBeInTheDocument();
  });

  it("asks for a profile when files are added", async () => {
    render(<App />);
    act(() => useQueue.getState().setPendingAdd(["C:\\k3wz\\Show"]));
    expect(await screen.findByRole("dialog", { name: "Add to queue" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: "Anime (Topaz)" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getByRole("radio", { name: "Anime" })).toHaveAttribute("aria-checked", "false");
    fireEvent.click(screen.getByRole("radio", { name: "CG" }));
    expect(screen.getByRole("radio", { name: "CG" })).toHaveAttribute("aria-checked", "true");
    expect(screen.getAllByRole("radio").map((r) => r.textContent)).toEqual([
      "Anime (Topaz)",
      "Anime",
      "Anime AV1",
      "CG",
      "Live action",
    ]);
  });

  it("shows the original-file choice up front, starting from the Settings default", async () => {
    const settings = vi.spyOn(api, "settings").mockResolvedValue({
      outputDir: "D:\\h2tj",
      toolsDir: "D:\\qv7x",
      lastProfile: "anime-av1",
      originalFile: "replace",
    });
    render(<App />);
    act(() => useQueue.getState().setPendingAdd(["C:\\k3wz\\Show"]));
    await screen.findByRole("dialog", { name: "Add to queue" });
    // Visible without opening Options
    expect(screen.getByRole("button", { name: /Options/ })).toHaveAttribute("aria-expanded", "false");
    const original = screen.getByRole("combobox", { name: /^Original file/ });
    expect(original).toHaveValue("replace");
    expect(screen.getByRole("radio", { name: "Anime AV1" })).toHaveAttribute("aria-checked", "true");
    fireEvent.change(original, { target: { value: "keep" } });
    expect(original).toHaveValue("keep");
    settings.mockRestore();
  });

  it("edits a waiting job's options, starting from the ones it has", async () => {
    const setOptions = vi.spyOn(api, "setOptions").mockResolvedValue(1);
    render(<App />);
    await act(async () => {});
    act(() =>
      useQueue.getState().apply({
        type: "queue",
        snapshot: { ...emptySnapshot, jobs: [job({ options: { Content: "cg", Crf: 18, Temporal: true } })] },
      }),
    );
    fireEvent.click(screen.getByText("ep01.mkv"));
    fireEvent.click(screen.getByRole("button", { name: "Edit options" }));
    const sheet = await screen.findByRole("dialog", { name: "Edit options" });
    expect(sheet).toHaveTextContent("ep01.mkv");
    expect(screen.getByRole("radio", { name: "CG" })).toHaveAttribute("aria-checked", "true");
    const crf = screen.getByRole("textbox", { name: /^CRF/ });
    expect(crf).toHaveValue("18");
    fireEvent.change(crf, { target: { value: "20" } });
    fireEvent.click(screen.getByRole("button", { name: "Save" }));
    await act(async () => {});
    expect(setOptions).toHaveBeenCalledWith(["j1"], { Content: "cg", Crf: 20, Temporal: true });
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    setOptions.mockRestore();
  });

  it("running and finished jobs can't be edited", async () => {
    render(<App />);
    await act(async () => {});
    act(() =>
      useQueue.getState().apply({
        type: "queue",
        snapshot: { ...emptySnapshot, running: true, jobs: [job({ state: "running", stage: "video" })] },
      }),
    );
    fireEvent.click(screen.getByText("ep01.mkv"));
    expect(screen.queryByRole("button", { name: "Edit options" })).not.toBeInTheDocument();
  });

  it("acts on several selected jobs, and on every failed one", async () => {
    const retry = vi.spyOn(api, "retry").mockResolvedValue(undefined);
    const remove = vi.spyOn(api, "remove").mockResolvedValue(undefined);
    const retryFailed = vi.spyOn(api, "retryFailed").mockResolvedValue(undefined);
    render(<App />);
    await act(async () => {});
    act(() =>
      useQueue.getState().apply({
        type: "queue",
        snapshot: {
          ...emptySnapshot,
          jobs: [
            job({ id: "a", name: "a.mkv", state: "failed", message: "encoder exited with 1" }),
            job({ id: "b", name: "b.mkv" }),
            job({ id: "c", name: "c.mkv", state: "cancelled" }),
            job({ id: "d", name: "d.mkv", state: "done" }),
          ],
        },
      }),
    );
    // Nothing selected, or one row: no bulk bar
    expect(screen.queryByRole("toolbar", { name: "Selected jobs" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("a.mkv"));
    expect(screen.queryByRole("toolbar", { name: "Selected jobs" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByText("c.mkv"), { shiftKey: true });
    const bar = screen.getByRole("toolbar", { name: "Selected jobs" });
    expect(bar).toHaveTextContent("3 selected");
    fireEvent.click(within(bar).getByRole("button", { name: "Retry" }));
    await act(async () => {});
    expect(retry.mock.calls.map((c) => c[0])).toEqual(["a", "c"]);
    fireEvent.click(screen.getByText("d.mkv"), { ctrlKey: true });
    expect(bar).toHaveTextContent("4 selected");
    fireEvent.click(within(bar).getByRole("button", { name: "Remove" }));
    await act(async () => {});
    expect(remove.mock.calls.map((c) => c[0])).toEqual(["a", "b", "c", "d"]);
    // The removed rows leave the selection with the next snapshot
    act(() => useQueue.getState().apply({ type: "queue", snapshot: { ...emptySnapshot, jobs: [job({ id: "e", name: "e.mkv", state: "failed" })] } }));
    expect(screen.queryByRole("toolbar", { name: "Selected jobs" })).not.toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Retry failed" }));
    expect(retryFailed).toHaveBeenCalled();
    for (const spy of [retry, remove, retryFailed]) spy.mockRestore();
  });
});
