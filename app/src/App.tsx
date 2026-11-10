import { isTauri } from "@tauri-apps/api/core";
import { ProgressBarStatus, getCurrentWindow } from "@tauri-apps/api/window";
import { isPermissionGranted, requestPermission, sendNotification } from "@tauri-apps/plugin-notification";
import { useEffect, useRef, useState } from "react";
import { api, onEngineEvent, PROFILE_LABELS, type AppInfo, type Disc, type DiscChoice, type ProfileId } from "./lib/api";
import { overall } from "./lib/format";
import { checkForUpdate } from "./lib/updates";
import { AddSheet, type AddDefaults } from "./queue/AddSheet";
import { RipSheet, type RipDefaults } from "./rip/RipSheet";
import { QueueView } from "./queue/QueueView";
import { useQueue, type View } from "./queue/store";
import { SettingsView } from "./settings/SettingsView";
import { UpdateBanner } from "./settings/Updates";

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

/** The profile an edited job's options name, else the default one (a settings file decides). */
function editProfile(options: Record<string, unknown>): ProfileId {
  const c = options.Content;
  return typeof c === "string" && c in PROFILE_LABELS ? (c as ProfileId) : "anime-topaz";
}

export default function App() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [addDefaults, setAddDefaults] = useState<{ paths: string[]; defaults: AddDefaults } | null>(null);
  const view = useQueue((s) => s.view);
  const pendingAdd = useQueue((s) => s.pendingAdd);
  const pendingDisc = useQueue((s) => s.pendingDisc);
  const pendingEdit = useQueue((s) => s.pendingEdit);
  const [rip, setRip] = useState<{ disc: Disc; choice: DiscChoice; defaults: RipDefaults } | null>(null);

  useEffect(() => {
    api.appInfo().then(setInfo).catch(() => setInfo(null));
    api.snapshot().then((s) => useQueue.getState().setSnapshot(s));
    // A disc read before the window opened: offer it in the queue (its sheet opened then)
    api.discCurrent().then((d) => d && useQueue.setState({ currentDisc: d }));
    const unlisten = onEngineEvent((e) => useQueue.getState().apply(e));
    // A newer release shows as a banner; nothing is downloaded until you choose Install
    checkForUpdate();
    return () => {
      unlisten.then((f) => f());
    };
  }, []);

  useEffect(() => {
    // The Add sheet starts with the profile picked last time and the Settings default for the
    // original file. It opens once they're loaded for this add, so it never starts from an old
    // default.
    if (!pendingAdd) return;
    let current = true;
    api.settings().then((s) => {
      if (current) {
        setAddDefaults({
          paths: pendingAdd,
          defaults: { profile: s?.lastProfile ?? "anime-topaz", original: s?.originalFile ?? "keep" },
        });
      }
    });
    return () => {
      current = false;
    };
  }, [pendingAdd]);

  useEffect(() => {
    // The Rip sheet opens once the disc's titles are chosen and the Settings defaults are loaded
    if (!pendingDisc) return;
    let current = true;
    Promise.all([api.discChoose(pendingDisc), api.settings(), api.makemkvStatus()]).then(([choice, s, status]) => {
      if (current && choice) {
        setRip({
          disc: pendingDisc,
          choice,
          defaults: {
            folders: s?.libraryFolders ?? [],
            languages: s?.keepLanguages ?? ["eng", "jpn", "spa"],
            profile: s?.lastProfile ?? "anime-topaz",
            canRip: status.state !== "expired",
          },
        });
      }
    });
    return () => {
      current = false;
    };
  }, [pendingDisc]);

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
      <UpdateBanner />
      <header className="flex items-center gap-2 px-4 pt-2" role="tablist">
        <h1 className="sr-only">Upscale</h1>
        {tab("queue", "Queue")}
        {tab("settings", "Settings")}
      </header>
      <main className="min-h-0 flex-1">{view === "queue" ? <QueueView /> : <SettingsView />}</main>
      <footer className="border-t border-neutral-200 px-4 py-1 text-xs text-neutral-500 dark:border-neutral-700">
        {info ? `core ${info.coreVersion}${info.gpu ? ` · ${info.gpu}` : ""}` : "core not connected"}
      </footer>
      {pendingDisc && rip?.disc === pendingDisc && (
        <RipSheet key={pendingDisc.name} disc={pendingDisc} choice={rip.choice} defaults={rip.defaults} />
      )}
      {pendingAdd && addDefaults?.paths === pendingAdd && (
        <AddSheet key={pendingAdd.join("|")} paths={pendingAdd} defaults={addDefaults.defaults} />
      )}
      {pendingEdit && (
        <AddSheet
          key={`edit ${pendingEdit.ids.join("|")}`}
          paths={[]}
          defaults={{ profile: editProfile(pendingEdit.options), original: "keep" }}
          edit={pendingEdit}
        />
      )}
    </div>
  );
}
