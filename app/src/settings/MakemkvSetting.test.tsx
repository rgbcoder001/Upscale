import { act, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import App from "../App";
import { api, emptySnapshot, setDemo } from "../lib/api";
import { useQueue } from "../queue/store";
import { demoDisc, enableDemo } from "../rip/demo";

describe("Settings → Ripping → MakeMKV", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    useQueue.setState({ snapshot: emptySnapshot, view: "queue", pendingDisc: null });
    setDemo(null);
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

  it("shows the trial and registers the user's key", async () => {
    vi.spyOn(api, "settings").mockResolvedValue({ outputDir: "/Out", toolsDir: "/Tools", lastProfile: "anime-av1" });
    vi.spyOn(api, "toolsStatus").mockResolvedValue([{ name: "MakeMKV", ok: true, version: "2.0.0", note: "" }]);
    vi.spyOn(api, "makemkvStatus").mockResolvedValue({ state: "trial", daysLeft: 30, days: 30 });
    const register = vi.spyOn(api, "makemkvRegister").mockResolvedValue([]);
    render(<App />);
    act(() => useQueue.getState().setView("settings"));
    expect(await screen.findByText("Trial: 30 of 30 days left")).toBeInTheDocument();
    const field = screen.getByLabelText("MakeMKV key");
    expect(field).toHaveAttribute("type", "password");
    expect(screen.getByRole("button", { name: "Register" })).toBeDisabled();
    fireEvent.change(field, { target: { value: " T-mykey " } });
    fireEvent.click(screen.getByRole("button", { name: "Register" }));
    const registered = await screen.findByText(/✓ Registered since/);
    expect(registered.className).toMatch(/text-green/);
    expect(register).toHaveBeenCalledWith(" T-mykey ");
    expect(field).toHaveValue("");
  });

  it("the Rip sheet says to register once the trial or key has expired", async () => {
    enableDemo();
    vi.spyOn(api, "makemkvStatus").mockResolvedValue({ state: "expired", message: "Evaluation period has expired." });
    render(<App />);
    act(() => useQueue.getState().setPendingDisc(demoDisc));
    const sheet = await screen.findByRole("dialog", { name: "Rip disc" });
    expect(within(sheet).getByText("Register MakeMKV in Settings → Ripping to rip discs.")).toBeInTheDocument();
  });
});
