import { isTauri } from "@tauri-apps/api/core";
import { ProgressBarStatus, getCurrentWindow } from "@tauri-apps/api/window";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { useEffect, useRef, useState } from "react";
import { api, onEngineEvent, type AppInfo, type ProfileId } from "./lib/api";
import { overall } from "./lib/format";
import { AddSheet } from "./queue/AddSheet";
import { QueueView } from "./queue/QueueView";
import { useQueue, type View } from "./queue/store";
import { SettingsView } from "./settings/SettingsView";

/** Window title and taskbar progress follow the queue; a toast says when it finishes. */
function useWindowProgress() {
  const { jobs, running, paused } = useQueue((s) => s.snapshot);
  const wasRunning = useRef(false);
  const progress = running ? overall(jobs) : null;

  useEffect(() => {
    if (!isTauri()) return;
    const w = getCurrentWindow();
    const pct = progress == null ? null : Math.round(progress * 100);
    w.setTitle(pct == null ? "Upscale" : `Upscale - ${pct}%${paused ? " (paused)" : ""}`);
    w.setProgressBar(
      pct == null
        ? { status: ProgressBarStatus.None }
        : { status: paused ? ProgressBarStatus.Paused : ProgressBarStatus.Normal, progress: pct },
    );
  }, [progress, paused]);

  useEffect(() => {
    if (wasRunning.current && !running && isTauri()) {
      const done = jobs.filter((j) => j.state === "done").length;
      const failed = jobs.filter((j) => j.state === "failed").length;
      (async () => {
        let granted = await isPermissionGranted();
        if (!granted) granted = (await requestPermission()) === "granted";
        if (granted) {
          sendNotification({
            title: "Upscale queue finished",
            body: `${done} done${failed ? `, ${failed} failed` : ""}`,
          });
        }
      })();
    }
    wasRunning.current = running;
  }, [running, jobs]);
}

export default function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [lastProfile, setLastProfile] = useState<ProfileId>("anime-topaz");
  const view = useQueue((s) => s.view);
  const pendingAdd = useQueue((s) => s.pendingAdd);

  useEffect(() => {
    api.appInfo().then(setInfo).catch(() => setInfo(null));
    api.snapshot().then((s) => useQueue.getState().setSnapshot(s));
    const unlisten = onEngineEvent((e) => useQueue.getState().apply(e));
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  useEffect(() => {
    // The Add sheet remembers the profile picked last time
    if (pendingAdd) api.settings().then((s) => s && setLastProfile(s.lastProfile));
  }, [pendingAdd]);

  useWindowProgress();

  const tab = (v: View, label: string) => (
    <button
      type="button"
      role="tab"
      aria-selected={view === v}
      onClick={() => useQueue.getState().setView(v)}
      className={`px-3 py-1 text-sm ${view === v ? "border-b-2 border-blue-600 font-semibold" : "text-neutral-500"}`}
    >
      {label}
    </button>
  );

  return (
    <div className="flex h-screen flex-col">
      <header className="flex items-center gap-2 px-4 pt-2" role="tablist">
        <h1 className="sr-only">Upscale</h1>
        {tab("queue", "Queue")}
        {tab("settings", "Settings")}
      </header>
      <main className="min-h-0 flex-1">{view === "queue" ? <QueueView /> : <SettingsView />}</main>
      <footer className="border-t border-neutral-200 px-4 py-1 text-xs text-neutral-500 dark:border-neutral-700">
        {info ? `core ${info.coreVersion}${info.gpu ? ` · ${info.gpu}` : ""}` : "core not connected"}
      </footer>
      {pendingAdd && <AddSheet key={pendingAdd.join("|")} paths={pendingAdd} lastProfile={lastProfile} />}
    </div>
  );
}
