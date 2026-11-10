import { useEffect, useRef, useState } from "react";
import { api, type Job } from "../lib/api";
import { canEdit } from "../lib/format";
import { CompareView } from "./CompareView";
import { useQueue } from "./store";

type Tab = "plan" | "log" | "compare";

function LogView({ job }: { job: Job }) {
  const [text, setText] = useState("");
  const pre = useRef<HTMLPreElement>(null);
  useEffect(() => {
    let alive = true;
    const load = () =>
      api.jobLog(job.id).then((t) => {
        if (!alive) return;
        setText(t);
        const el = pre.current;
        if (el && el.scrollHeight - el.scrollTop - el.clientHeight < 40) el.scrollTop = el.scrollHeight;
      });
    load();
    // A live tail while the job runs
    const timer = job.state === "running" ? setInterval(load, 2000) : undefined;
    return () => {
      alive = false;
      if (timer) clearInterval(timer);
    };
  }, [job.id, job.state]);
  return (
    <pre ref={pre} className="h-full overflow-auto whitespace-pre-wrap p-3 font-mono text-xs leading-5">
      {text || "No log yet. It starts when the job runs."}
    </pre>
  );
}

export function DetailPanel({ job }: { job: Job }) {
  const [tab, setTab] = useState<Tab>("plan");
  const tabClass = (t: Tab) =>
    `px-3 py-1.5 text-sm ${tab === t ? "border-b-2 border-blue-600 font-medium" : "text-neutral-500"}`;
  return (
    <aside className="flex w-[30rem] min-w-0 shrink-0 flex-col border-l border-neutral-200 dark:border-neutral-700" aria-label="File detail">
      <div className="flex items-center border-b border-neutral-200 px-2 dark:border-neutral-700">
        <button type="button" className={tabClass("plan")} onClick={() => setTab("plan")}>
          Plan
        </button>
        <button type="button" className={tabClass("log")} onClick={() => setTab("log")}>
          Log
        </button>
        {job.state === "done" && (
          <button type="button" className={tabClass("compare")} onClick={() => setTab("compare")}>
            Compare
          </button>
        )}
        <div className="flex-1" />
        {canEdit(job) && (
          <button
            type="button"
            className="px-2 text-sm text-blue-700 hover:underline dark:text-blue-300"
            onClick={() => useQueue.getState().setPendingEdit({ ids: [job.id], options: job.options, label: job.name })}
          >
            Edit options
          </button>
        )}
        {job.output && (
          <button type="button" className="px-2 text-sm text-blue-700 hover:underline dark:text-blue-300" onClick={() => api.showInFolder(job.output!)}>
            Show in folder
          </button>
        )}
        {job.log && (
          <button type="button" className="px-2 text-sm text-blue-700 hover:underline dark:text-blue-300" onClick={() => api.openPath(job.log!)}>
            Open log
          </button>
        )}
        <button type="button" className="px-2 text-neutral-500" aria-label="Close" onClick={() => useQueue.getState().select(null)}>
          ✕
        </button>
      </div>
      <div className="min-h-0 flex-1">
        {tab === "plan" ? (
          <pre className="h-full overflow-auto whitespace-pre-wrap p-3 font-mono text-xs leading-5">
            {job.plan.length > 0 ? job.plan.join("\n") : job.state === "analysing" ? "Analysing ..." : (job.message ?? "No plan.")}
            {job.message && job.plan.length > 0 ? `\n\n${job.message}` : ""}
          </pre>
        ) : tab === "log" ? (
          <LogView job={job} />
        ) : (
          <CompareView job={job} />
        )}
      </div>
    </aside>
  );
}
