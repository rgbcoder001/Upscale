import { act, fireEvent, render, screen } from "@testing-library/react";
import { beforeEach, describe, expect, it } from "vitest";
import App from "./App";
import { emptySnapshot, type Job } from "./lib/api";
import { useQueue } from "./queue/store";

function job(over: Partial<Job>): Job {
  return {
    id: "j1",
    file: "C:\\Rips\\ep01.mkv",
    name: "ep01.mkv",
    group: "g",
    input: "C:\\Rips",
    options: {},
    state: "queued",
    plan: ["ep01.mkv", "  Route:     upscale  1920x1080 h264 23.976 fps, SAR 1:1, matrix 709"],
    profile: "anime",
    route: "Upscale 1080p → 2160p",
    ...over,
  };
}

describe("App", () => {
  beforeEach(() => useQueue.setState({ snapshot: emptySnapshot, selectedId: null, view: "queue", pendingAdd: null }));

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
        snapshot: { ...emptySnapshot, jobs: [job({ state: "done", output: "C:\\out\\ep01.mkv" })] },
      }),
    );
    fireEvent.click(screen.getByText("ep01.mkv"));
    fireEvent.click(screen.getByRole("button", { name: "Compare" }));
    // Outside Tauri there are no images
    expect(await screen.findByText(/No comparison images/)).toBeInTheDocument();
  });

  it("asks for a profile when files are added", () => {
    render(<App />);
    act(() => useQueue.getState().setPendingAdd(["C:\\Rips\\Show"]));
    expect(screen.getByRole("dialog", { name: "Add to queue" })).toBeInTheDocument();
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
});
