import { useEffect, useState } from "react";
import { getAppInfo } from "./lib/core";
import { QueueView } from "./queue/QueueView";

export default function App() {
  const [coreVersion, setCoreVersion] = useState<string | null>(null);

  useEffect(() => {
    getAppInfo()
      .then((info) => setCoreVersion(info?.coreVersion ?? null))
      .catch(() => setCoreVersion(null));
  }, []);

  return (
    <div className="flex h-screen flex-col">
      <header className="flex items-center justify-between px-4 py-3">
        <h1 className="text-lg font-semibold">Queue</h1>
      </header>
      <main className="min-h-0 flex-1">
        <QueueView />
      </main>
      <footer className="border-t border-neutral-200 px-4 py-1 text-xs text-neutral-500 dark:border-neutral-700">
        {coreVersion ? `core ${coreVersion}` : "core not connected"}
      </footer>
    </div>
  );
}
