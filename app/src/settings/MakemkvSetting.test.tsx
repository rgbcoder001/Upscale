import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { api, emptySnapshot } from "../lib/api";
import { useQueue } from "../queue/store";

describe("Settings → Ripping → MakeMKV", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    useQueue.setState({ snapshot: emptySnapshot, view: "queue" });
  });

  it("installs only after the user confirms, once", async () => {
    vi.spyOn(api, "settings").mockResolvedValue({ outputDir: "/Out", toolsDir: "/Tools", lastProfile: "anime-av1" });
    vi.spyOn(api, "toolsStatus").mockResolvedValue([{ name: "MakeMKV", ok: false, note: "for ripping discs" }]);
    const install = vi.spyOn(api, "makemkvInstall").mockResolvedValue(["MakeMKV is ready: /Applications/MakeMKV.app"]);
    render(<App />);
    act(() => useQueue.getState().setView("settings"));

    // Cancel: nothing installed
    fireEvent.click(await screen.findByRole("button", { name: "Install MakeMKV" }));
    expect(screen.getByRole("alertdialog", { name: "Install MakeMKV?" })).toHaveTextContent(/isn't notarized by Apple/);
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("alertdialog")).toBeNull();
    expect(install).not.toHaveBeenCalled();

    // Confirm: installed, and the result is shown
    fireEvent.click(screen.getByRole("button", { name: "Install MakeMKV" }));
    fireEvent.click(screen.getByRole("button", { name: "Install" }));
    expect(await screen.findByText(/MakeMKV is ready/)).toBeInTheDocument();
    expect(install).toHaveBeenCalledTimes(1);
  });
});
