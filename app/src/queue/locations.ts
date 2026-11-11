// Where a job's files are: the source (or the disc and the rip in progress), where the
// finished file goes, and the log. Shown at the top of the detail panel.

import type { Job } from "../lib/api";

export interface Place {
  label: string;
  /** A file or folder, with Show in folder and Copy; null for text only (the disc's name). */
  path: string | null;
  text: string;
  /** Still to come: the file isn't there yet. */
  pending?: boolean;
}

/** `dir` + `name` with the separator `dir` already uses. */
function join(dir: string, name: string) {
  const sep = dir.includes("\\") ? "\\" : "/";
  return dir.endsWith(sep) ? dir + name : dir + sep + name;
}

const isRipOnly = (job: Job) => job.route?.startsWith("Rip only") ?? false;

export function jobLocations(job: Job): Place[] {
  const places: Place[] = [];
  const out = (label: string, path: string, pending: boolean) => places.push({ label, path, text: path, pending });

  if (job.rip && (job.state === "waitingToRip" || job.state === "ripping")) {
    places.push({ label: "Disc", path: null, text: `${job.rip.disc}, title ${job.rip.index}` });
    if (job.state === "ripping") out("Ripping into", join(job.input, `title-${job.rip.index}`), false);
    if (isRipOnly(job)) {
      if (job.plannedOutput) out("Saved to", job.plannedOutput, true);
    } else {
      out("Raw rip", job.file, true);
      if (job.plannedOutput) out("Encoded to", job.plannedOutput, true);
    }
  } else {
    out(isRipOnly(job) && job.state === "done" ? "Raw rip" : "Source", job.file, false);
    if (isRipOnly(job) && job.state === "done") {
      // Nothing encoded here: the rip is the file in the library
    } else if (job.output) {
      out("Output", job.output, false);
    } else if (job.plannedOutput) {
      out("Saves to", job.plannedOutput, true);
    } else if (job.options.ReplaceSource === true && job.state !== "skipped") {
      places.push({ label: "Saves to", path: null, text: "In place of the source (Replace original)", pending: true });
    }
  }
  if (job.log) out("Log", job.log, false);
  return places;
}
