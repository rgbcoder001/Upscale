import { useQueue } from "./store";

const toolbar = ["Start", "Pause", "Stop after current", "Cancel"] as const;

export function QueueView() {
  const jobs = useQueue((s) => s.jobs);

  return (
    <div className="flex h-full flex-col">
      <div className="flex items-center gap-2 border-b border-neutral-200 px-4 py-2 dark:border-neutral-700">
        <button
          type="button"
          disabled
          className="rounded bg-blue-600 px-3 py-1.5 text-sm font-medium text-white disabled:opacity-40"
        >
          Add files
        </button>
        <div className="mx-2 h-5 w-px bg-neutral-300 dark:bg-neutral-700" />
        {toolbar.map((label) => (
          <button
            key={label}
            type="button"
            disabled={jobs.length === 0}
            className="rounded px-3 py-1.5 text-sm hover:bg-neutral-200 disabled:opacity-40 disabled:hover:bg-transparent dark:hover:bg-neutral-800"
          >
            {label}
          </button>
        ))}
      </div>

      {jobs.length === 0 ? (
        <div className="m-4 flex flex-1 flex-col items-center justify-center rounded-lg border-2 border-dashed border-neutral-300 text-neutral-500 dark:border-neutral-700">
          <p className="text-base font-medium">The queue is empty</p>
          <p className="mt-1 text-sm">Drop videos or folders here, or use Add files.</p>
        </div>
      ) : (
        <ul className="flex-1 overflow-auto">
          {jobs.map((job) => (
            <li key={job.id} className="border-b border-neutral-200 px-4 py-2 text-sm dark:border-neutral-800">
              {job.path}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
