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
