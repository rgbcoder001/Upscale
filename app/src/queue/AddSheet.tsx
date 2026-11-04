import { useState } from "react";
import { api, type OriginalFile, PROFILE_LABELS, type ProfileId } from "../lib/api";
import { type AddForm, buildOptions, defaultCrf, defaultForm, englishDefaults, ORIGINAL_LABELS, usesTopaz } from "./options";
import { useQueue } from "./store";

const input =
  "w-full rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-600 dark:bg-neutral-800";

function Field({ label, hint, children }: { label: string; hint?: string; children: React.ReactNode }) {
  return (
    <label className="block text-sm">
      <span className="mb-1 block text-neutral-600 dark:text-neutral-300">{label}</span>
      {children}
      {hint && <span className="mt-0.5 block text-xs text-neutral-500">{hint}</span>}
    </label>
  );
}

/** What the sheet starts with: the profile picked last time and the Settings default. */
export interface AddDefaults {
  profile: ProfileId;
  original: OriginalFile;
}

export function AddSheet({ paths, defaults }: { paths: string[]; defaults: AddDefaults }) {
  const close = () => useQueue.getState().setPendingAdd(null);
  const [form, setForm] = useState<AddForm>(() => defaultForm(defaults.profile, defaults.original));
  const [showOptions, setShowOptions] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const set = <K extends keyof AddForm>(k: K, v: AddForm[K]) => setForm((f) => ({ ...f, [k]: v }));

  const add = async () => {
    try {
      const options = buildOptions(form);
      await api.add(paths, options);
      close();
    } catch (e) {
      setError(String(e instanceof Error ? e.message : e));
    }
  };

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-black/40" role="dialog" aria-label="Add to queue">
      <div className="max-h-[90vh] w-[30rem] overflow-auto rounded-lg bg-white p-5 shadow-xl dark:bg-neutral-900">
        <h2 className="text-base font-semibold">Add to queue</h2>
        <p className="mt-1 truncate text-sm text-neutral-500" title={paths.join("\n")}>
          {paths.length === 1 ? paths[0] : `${paths.length} items`}
        </p>

        <div className="mt-4 grid grid-cols-3 gap-2" role="radiogroup" aria-label="Profile">
          {(Object.keys(PROFILE_LABELS) as ProfileId[]).map((p) => (
            <button
              key={p}
              type="button"
              role="radio"
              aria-checked={form.profile === p}
              onClick={() => set("profile", p)}
              className={`rounded border px-2 py-2 text-sm font-medium whitespace-nowrap ${
                form.profile === p
                  ? "border-blue-600 bg-blue-50 text-blue-700 dark:bg-blue-950 dark:text-blue-200"
                  : "border-neutral-300 dark:border-neutral-600"
              }`}
            >
              {PROFILE_LABELS[p]}
            </button>
          ))}
        </div>
        <p className="mt-1 text-xs text-neutral-500">
          An upscale-settings.json in the folder (or above it) still applies, e.g. {'{ "Content": "cg" }'}.
        </p>

        <div className="mt-4">
          <Field
            label="Original file"
            hint="After the new file passes its frame and duration checks. Network drives have no Recycle Bin: deleted for good. Test clips never delete. The default is in Settings."
          >
            <select className={input} value={form.original} onChange={(e) => set("original", e.target.value as OriginalFile)}>
              {(Object.keys(ORIGINAL_LABELS) as OriginalFile[]).map((o) => (
                <option key={o} value={o}>
                  {ORIGINAL_LABELS[o]}
                </option>
              ))}
            </select>
          </Field>
        </div>

        <button
          type="button"
          className="mt-4 text-sm text-blue-700 hover:underline dark:text-blue-300"
          aria-expanded={showOptions}
          onClick={() => setShowOptions((s) => !s)}
        >
          {showOptions ? "▾" : "▸"} Options
        </button>
        {showOptions && (
          <div className="mt-2 grid grid-cols-2 gap-3">
            <Field label="CRF" hint={`Default ${defaultCrf(form.profile)}`}>
              <input className={input} inputMode="numeric" value={form.crf} onChange={(e) => set("crf", e.target.value)} />
            </Field>
            <Field label="Preset" hint="Default 5">
              <input className={input} inputMode="numeric" value={form.preset} onChange={(e) => set("preset", e.target.value)} />
            </Field>
            {form.profile === "anime" ? (
              <>
                <Field label="Denoise">
                  <select className={input} value={form.denoise} onChange={(e) => set("denoise", e.target.value as AddForm["denoise"])}>
                    <option value="auto">Auto</option>
                    <option value="none">None</option>
                    <option value="light">Light</option>
                    <option value="strong">Strong</option>
                  </select>
                </Field>
                <Field label="Native resolution">
                  <select className={input} value={form.native} onChange={(e) => set("native", e.target.value as AddForm["native"])}>
                    <option value="auto">Auto</option>
                    <option value="off">Off</option>
                  </select>
                </Field>
              </>
            ) : usesTopaz(form.profile) ? (
              <div className="col-span-2">
                <Field label="Topaz tvai_up settings" hint="Blank = the default preset (Proteus 4)">
                  <input className={input} value={form.topazFilter} onChange={(e) => set("topazFilter", e.target.value)} />
                </Field>
              </div>
            ) : null}
            <Field label="Default audio" hint={englishDefaults(form.profile) ? "Default eng" : "Default auto: eng if more channels, else jpn"}>
              <input className={input} value={form.defaultAudio} onChange={(e) => set("defaultAudio", e.target.value)} />
            </Field>
            <Field
              label="Default subtitles"
              hint={englishDefaults(form.profile) ? "Default none" : "Default auto: full subs for jpn, signs for eng"}
            >
              <input className={input} value={form.defaultSubtitles} onChange={(e) => set("defaultSubtitles", e.target.value)} />
            </Field>
            <label className="col-span-2 flex items-center gap-2 text-sm">
              <input type="checkbox" checked={form.filmGrain} onChange={(e) => set("filmGrain", e.target.checked)} />
              Film grain synthesis
            </label>
            <label className="col-span-2 flex items-center gap-2 text-sm" title="Encodes in chunks of about 4 minutes at scene cuts; after closing the app or a crash it continues with the next chunk">
              <input type="checkbox" checked={form.resumable} onChange={(e) => set("resumable", e.target.checked)} />
              Resumable encoding (survives closing the app)
            </label>
            <label className="col-span-2 flex items-center gap-2 text-sm">
              <input type="checkbox" checked={form.testClip} onChange={(e) => set("testClip", e.target.checked)} />
              Test clip (30 s from the middle, video only)
            </label>
            <div className="col-span-2">
              <Field label="Advanced (JSON)" hint='Any script setting, e.g. {"Temporal": true}'>
                <textarea
                  className={`${input} h-16 font-mono text-xs`}
                  value={form.advanced}
                  onChange={(e) => set("advanced", e.target.value)}
                />
              </Field>
            </div>
          </div>
        )}

        {error && <p className="mt-3 text-sm text-red-600">{error}</p>}
        <div className="mt-5 flex justify-end gap-2">
          <button type="button" className="rounded px-3 py-1.5 text-sm hover:bg-neutral-100 dark:hover:bg-neutral-800" onClick={close}>
            Cancel
          </button>
          <button type="button" className="rounded bg-blue-600 px-3 py-1.5 text-sm font-medium text-white" onClick={add}>
            Add
          </button>
        </div>
      </div>
    </div>
  );
}
