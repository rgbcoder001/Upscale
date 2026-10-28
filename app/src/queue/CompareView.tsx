import { convertFileSrc, isTauri } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { api, type Job } from "../lib/api";

/** Each comparison image is source | final side by side; the slider shows them on top of each other. */
export function CompareView({ job }: { job: Job }) {
  const [images, setImages] = useState<string[] | null>(null);
  const [index, setIndex] = useState(0);
  const [split, setSplit] = useState(50);
  const [aspect, setAspect] = useState(16 / 9);

  useEffect(() => {
    api.compareImages(job.id).then((list) => {
      setImages(list);
      setIndex(0);
    });
  }, [job.id]);

  if (images == null) return <p className="p-3 text-sm text-neutral-500">Loading ...</p>;
  if (images.length === 0) {
    return <p className="p-3 text-sm text-neutral-500">No comparison images for this file (Screenshots is 0, or it was encoded without upscaling).</p>;
  }
  const src = isTauri() ? convertFileSrc(images[index]) : images[index];
  const half = (side: "left" | "right") => ({
    backgroundImage: `url("${src}")`,
    backgroundSize: "200% 100%",
    backgroundPosition: side,
  });

  return (
    <div className="flex h-full flex-col gap-2 overflow-auto p-3">
      {/* Only to learn the image size; the halves are drawn as backgrounds */}
      <img src={src} alt="" className="hidden" onLoad={(e) => setAspect(e.currentTarget.naturalWidth / 2 / e.currentTarget.naturalHeight)} />
      <div className="relative w-full select-none overflow-hidden rounded bg-black" style={{ aspectRatio: aspect }} data-testid="compare-frame">
        <div className="absolute inset-0" style={half("right")} />
        <div className="absolute inset-0" style={{ ...half("left"), clipPath: `inset(0 ${100 - split}% 0 0)` }} />
        <div className="absolute inset-y-0 w-px bg-white/80" style={{ left: `${split}%` }} />
        <span className="absolute left-2 top-2 rounded bg-black/60 px-1.5 py-0.5 text-xs text-white">Source (scaled)</span>
        <span className="absolute right-2 top-2 rounded bg-black/60 px-1.5 py-0.5 text-xs text-white">Upscaled</span>
      </div>
      <input
        type="range"
        min={0}
        max={100}
        value={split}
        aria-label="Before / after"
        onChange={(e) => setSplit(Number(e.target.value))}
      />
      <div className="flex flex-wrap gap-1">
        {images.map((path, i) => (
          <button
            key={path}
            type="button"
            className={`rounded px-2 py-0.5 text-xs ${i === index ? "bg-blue-600 text-white" : "bg-neutral-200 dark:bg-neutral-700"}`}
            onClick={() => setIndex(i)}
          >
            {i + 1}
          </button>
        ))}
        <div className="flex-1" />
        <button type="button" className="text-xs text-blue-700 hover:underline dark:text-blue-300" onClick={() => api.openPath(images[index])}>
          Open image
        </button>
      </div>
    </div>
  );
}
