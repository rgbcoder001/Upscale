import { getVersion } from "@tauri-apps/api/app";
import { isTauri } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { checkForUpdate, installAndRestart, useUpdates } from "../lib/updates";
import { useQueue } from "../queue/store";

const button = "rounded px-3 py-1.5 text-sm hover:bg-neutral-200 disabled:opacity-40 dark:hover:bg-neutral-800";

function progressText(p: { done: number; total: number }) {
  const mb = (n: number) => (n / 2 ** 20).toFixed(1);
  return p.total > 0 ? `Downloading ${mb(p.done)} of ${mb(p.total)} MB ...` : "Downloading ...";
}

/** The install button: not while jobs run, since installing restarts the app. */
function InstallButton({ primary }: { primary?: boolean }) {
  const running = useQueue((s) => s.snapshot.running);
  const installing = useUpdates((s) => s.installing);
  return (
    <button
      type="button"
      className={primary ? "rounded bg-blue-600 px-3 py-1.5 text-sm font-medium text-white disabled:opacity-40" : button}
      disabled={running || installing != null}
      title={running ? "Installing restarts the app: stop the queue first" : undefined}
      onClick={installAndRestart}
    >
      Install and restart
    </button>
  );
}

/** A strip above the views when the start-up check found a newer release. */
export function UpdateBanner() {
  const { update, installing, dismissed, error } = useUpdates();
  const running = useQueue((s) => s.snapshot.running);
  if (!update || dismissed) return null;
  return (
    <div className="flex items-center gap-3 border-b border-blue-200 bg-blue-50 px-4 py-2 text-sm dark:border-blue-900 dark:bg-blue-950">
      <span>
        Upscale {update.version} is available.
        {running && !installing && " It can be installed once the queue has stopped."}
      </span>
      {installing && <span className="text-neutral-600">{progressText(installing)}</span>}
      {error && <span className="text-red-700">{error}</span>}
      <div className="flex-1" />
      <InstallButton primary />
      <button type="button" className={button} disabled={installing != null} onClick={() => useUpdates.setState({ dismissed: true })}>
        Later
      </button>
    </div>
  );
}

/** Settings → App updates. */
export function UpdatesSection() {
  const [version, setVersion] = useState<string | null>(null);
  const { checking, update, upToDate, error, installing } = useUpdates();
  useEffect(() => {
    if (isTauri()) getVersion().then(setVersion);
  }, []);
  return (
    <section className="mt-8 max-w-3xl">
      <div className="flex items-center gap-2">
        <h2 className="text-base font-semibold">App updates</h2>
        <div className="flex-1" />
        <button type="button" className={button} disabled={checking || installing != null} onClick={checkForUpdate}>
          {checking ? "Checking ..." : "Check for updates"}
        </button>
      </div>
      <p className="mt-1 text-sm text-neutral-600 dark:text-neutral-300">
        Version {version ?? "?"}.{" "}
        {update ? `Version ${update.version} is available.` : upToDate ? "This is the latest version." : ""}
      </p>
      <p className="mt-0.5 text-xs text-neutral-500">
        Checked at start-up against the latest GitHub release. Updates are signed; the queue and settings are kept.
      </p>
      {update && (
        <div className="mt-3 space-y-2">
          {update.body && (
            <pre className="max-h-48 overflow-auto whitespace-pre-wrap rounded bg-neutral-100 p-3 text-xs dark:bg-neutral-800">{update.body}</pre>
          )}
          <div className="flex items-center gap-3">
            <InstallButton primary />
            {installing && <span className="text-sm text-neutral-600">{progressText(installing)}</span>}
          </div>
        </div>
      )}
      {error && <p className="mt-2 text-sm text-red-700">Update check failed: {error}</p>}
    </section>
  );
}
