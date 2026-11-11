// Small helpers for the Rip sheet.

const DISC_MARKERS = /\b(disc|disk|d|season|s|vol|volume|part|bd|bluray|blu ray|dvd|uhd|4k)\s*\d*\b/gi;

/** A search query from a disc label: `SHOW_ALPHA_D2` → `Show Alpha`. */
export function queryFromDiscName(name: string): string {
  const words = name
    .replace(/[_.]+/g, " ")
    .replace(DISC_MARKERS, " ")
    .replace(/\s+/g, " ")
    .trim();
  return words
    .toLowerCase()
    .split(" ")
    .filter(Boolean)
    .map((w) => w[0].toUpperCase() + w.slice(1))
    .join(" ");
}

export function minutes(seconds: number): string {
  const m = Math.round(seconds / 60);
  return m >= 60 ? `${Math.floor(m / 60)} h ${m % 60} min` : `${m} min`;
}

export function gigabytes(bytes: number): string {
  return `${(bytes / 2 ** 30).toFixed(1)} GB`;
}

/** The last two parts of a path, for showing a long library path compactly. */
export function shortPath(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts.slice(-3).join("/");
}

/** Each title's episode number counting on from `first`; a double-length episode takes two. */
export function episodeNumbers(titles: { index: number; double: boolean }[], first: number): Map<number, number> {
  const numbers = new Map<number, number>();
  let next = Math.max(1, first);
  for (const t of titles) {
    numbers.set(t.index, next);
    next += t.double ? 2 : 1;
  }
  return numbers;
}

/** A path's file name without `.mkv`. */
export function fileStem(path: string): string {
  const file = path.split(/[\\/]/).pop() ?? "";
  return file.replace(/\.mkv$/i, "");
}

/** The first file name two paths share (ignoring case, as Windows and macOS do), if any. */
export function sharedName(paths: string[]): string | null {
  const seen = new Set<string>();
  for (const p of paths) {
    const key = p.toLowerCase().replace(/\\/g, "/");
    if (seen.has(key)) return p.split(/[\\/]/).pop() ?? p;
    seen.add(key);
  }
  return null;
}
