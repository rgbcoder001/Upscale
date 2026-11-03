import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useState } from "react";
import { api, type AppSettings, type OriginalFile, type ToolStatus } from "../lib/api";
import { ORIGINAL_LABELS } from "../queue/options";
import { UpdatesSection } from "./Updates";

const input =
  "min-w-0 flex-1 rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-600 dark:bg-neutral-800";
const button = "rounded px-3 py-1.5 text-sm hover:bg-neutral-200 disabled:opacity-40 dark:hover:bg-neutral-800";

function FolderField({ label, value, onChange, hint }: { label: string; value: string; onChange: (v: string) => void; hint: string }) {
  const browse = async () => {
    const dir = await open({ directory: true, defaultPath: value || undefined });
    if (typeof dir === "string") onChange(dir);
  };
  return (
    <div>
      <label className="block text-sm text-neutral-600 dark:text-neutral-300">{label}</label>
      <div className="mt-1 flex gap-2">
        <input className={input} value={value} onChange={(e) => onChange(e.target.value)} aria-label={label} />
        <button type="button" className={button} onClick={browse}>
          Browse
        </button>
      </div>
      <p className="mt-0.5 text-xs text-neutral-500">{hint}</p>
    </div>
  );
}

export function SettingsView() {
  const [settings, setSettings] = useState<AppSettings | null>(null);
  const [saved, setSaved] = useState(false);
  const [tools, setTools] = useState<ToolStatus[] | null>(null);
  const [busy, setBusy] = useState<string | null>("Checking tools ...");
  const [updateLog, setUpdateLog] = useState<string[]>([]);

  const loadTools = () =>
    api.toolsStatus().then((t) => {
      setTools(t);
      setBusy(null);
    });
  const refresh = () => {
    setBusy("Checking tools ...");
    loadTools();
  };
  useEffect(() => {
    api.settings().then(setSettings);
    loadTools();
  }, []);

  const save = async () => {
    if (!settings) return;
    await api.setSettings(settings);
    setSaved(true);
    setTimeout(() => setSaved(false), 1500);
  };
  const update = async () => {
    setBusy("Updating tools (this can take a while: vs-mlrt is several GB) ...");
    setUpdateLog(await api.toolsUpdate());
    refresh();
  };

  return (
    <div className="h-full overflow-auto p-6">
      <section className="max-w-3xl space-y-4">
        <h2 className="text-base font-semibold">Folders</h2>
        {settings && (
          <>
            <FolderField
              label="Output folder"
              value={settings.outputDir}
              onChange={(outputDir) => setSettings({ ...settings, outputDir })}
              hint="One folder for every profile: finished files, logs, analysis cache and comparison images."
            />
            <FolderField
              label="Tools folder"
              value={settings.toolsDir}
              onChange={(toolsDir) => setSettings({ ...settings, toolsDir })}
              hint="Python with VapourSynth, vs-mlrt, FFmpeg, MKVToolNix and HandBrakeCLI are downloaded here (about 4–5 GB, checked against SHA-256). Point it at an existing tools folder to reuse its vs-mlrt and FFmpeg."
            />
            <FolderField
              label="Temp folder"
              value={settings.tempDir ?? ""}
              onChange={(tempDir) => setSettings({ ...settings, tempDir: tempDir || null })}
              hint="Chunks of resumable encodes and temporary audio. Empty = a work folder in this computer's local app data, so it's never shared with another machine."
            />
            <label className="flex items-center gap-2 text-sm">
              <input
                type="checkbox"
                checked={settings.useSystemTools ?? false}
                onChange={(e) => setSettings({ ...settings, useSystemTools: e.target.checked })}
              />
              Use system tools (PATH, winget, your own Python) instead of the tools folder
            </label>
            <h2 className="pt-4 text-base font-semibold">New jobs</h2>
            <div>
              <label className="block text-sm text-neutral-600 dark:text-neutral-300" htmlFor="original-file">
                Original file
              </label>
              <select
                id="original-file"
                className={`${input} mt-1 w-full max-w-xs`}
                value={settings.originalFile ?? "keep"}
                onChange={(e) => setSettings({ ...settings, originalFile: e.target.value as OriginalFile })}
              >
                {(Object.keys(ORIGINAL_LABELS) as OriginalFile[]).map((o) => (
                  <option key={o} value={o}>
                    {ORIGINAL_LABELS[o]}
                  </option>
                ))}
              </select>
              <p className="mt-0.5 text-xs text-neutral-500">
                What the Add sheet starts with. You can still change it for each folder or file you add.
              </p>
            </div>
            <div className="flex items-center gap-3">
              <button type="button" className="rounded bg-blue-600 px-3 py-1.5 text-sm font-medium text-white" onClick={save}>
                Save
              </button>
              {saved && <span className="text-sm text-green-700">Saved</span>}
            </div>
          </>
        )}
      </section>

      <section className="mt-8 max-w-5xl">
        <div className="flex items-center gap-2">
          <h2 className="text-base font-semibold">Tools</h2>
          <div className="flex-1" />
          {busy && <span className="text-sm text-neutral-500">{busy}</span>}
          <button type="button" className={button} disabled={busy != null} onClick={refresh}>
            Refresh
          </button>
          <button type="button" className={button} disabled={busy != null} onClick={update}>
            Update tools
          </button>
        </div>
        <p className="mt-1 text-xs text-neutral-500">
          Missing tools are installed when a batch starts. Update tools also upgrades them and runs the self-tests.
        </p>
        <table className="mt-3 w-full text-left text-sm">
          <thead className="text-xs text-neutral-500">
            <tr>
              <th className="py-1 pr-2" />
              <th className="py-1 pr-4">Tool</th>
              <th className="py-1 pr-4">Version</th>
              <th className="py-1">Path / note</th>
            </tr>
          </thead>
          <tbody>
            {(tools ?? []).map((t) => (
              <tr key={t.name} className="border-t border-neutral-200 align-top dark:border-neutral-800">
                <td className={`py-1 pr-2 ${t.ok ? "text-green-600" : "text-amber-600"}`}>{t.ok ? "✓" : "–"}</td>
                <td className="py-1 pr-4 whitespace-nowrap">{t.name}</td>
                <td className="py-1 pr-4 whitespace-nowrap">{t.version ?? ""}</td>
                <td className="py-1 break-all text-neutral-600 dark:text-neutral-400">
                  {t.path}
                  {t.note && <span className="block text-xs text-neutral-500">{t.note}</span>}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
        {updateLog.length > 0 && (
          <pre className="mt-4 max-h-64 overflow-auto rounded bg-neutral-100 p-3 font-mono text-xs dark:bg-neutral-800">{updateLog.join("\n")}</pre>
        )}
      </section>

      <UpdatesSection />
    </div>
  );
}
