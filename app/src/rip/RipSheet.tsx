import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useState } from "react";
import {
  api,
  folderLabel,
  PROFILE_LABELS,
  type Disc,
  type DiscChoice,
  type LibraryFolder,
  type LibraryKind,
  type ProfileId,
  type ShowInfo,
  type TmdbMatch,
  type TmdbSeason,
} from "../lib/api";
import { useQueue } from "../queue/store";
import { gigabytes, minutes, queryFromDiscName, shortPath } from "./names";

const input =
  "w-full rounded border border-neutral-300 bg-white px-2 py-1 text-sm dark:border-neutral-600 dark:bg-neutral-800";
const label = "mb-1 block text-sm text-neutral-600 dark:text-neutral-300";
/** The "Choose a folder…" entry in Save to. */
const CHOOSE = "::choose-folder::";

/** What the sheet starts with, from Settings. */
export interface RipDefaults {
  folders: LibraryFolder[];
  languages: string[];
  profile: ProfileId;
  /** MakeMKV may rip: registered or in its trial. */
  canRip: boolean;
}

/** The Rip sheet: what to rip, its Jellyfin name, the languages and the profile. */
export function RipSheet({ disc, choice, defaults }: { disc: Disc; choice: DiscChoice; defaults: RipDefaults }) {
  const close = () => useQueue.getState().setPendingDisc(null);
  const { selection } = choice;

  const [ticked, setTicked] = useState(() => new Set(selection.picks.filter((p) => p.ticked).map((p) => p.index)));
  const [kind, setKind] = useState<LibraryKind>(selection.kind === "movie" ? "movies" : "shows");
  const foldersOf = (k: LibraryKind) => defaults.folders.filter((f) => f.kind === k);
  const libraryOf = (path: string) => defaults.folders.find((f) => f.path === path);
  const [folder, setFolder] = useState(() => (foldersOf(kind)[0] ?? defaults.folders[0])?.path ?? "");
  /** Folders picked with "Choose a folder…" for this rip only. */
  const [custom, setCustom] = useState<string[]>([]);

  const [query, setQuery] = useState(() => queryFromDiscName(disc.name));
  const [results, setResults] = useState<TmdbMatch[]>([]);
  const [searchError, setSearchError] = useState<string | null>(null);
  const [match, setMatch] = useState<TmdbMatch | null>(null);
  const [name, setName] = useState(() => queryFromDiscName(disc.name));
  const [year, setYear] = useState("");
  const [seasons, setSeasons] = useState<TmdbSeason[]>([]);
  const [suggested, setSuggested] = useState<ShowInfo | null>(null);
  const [seasonEdit, setSeasonEdit] = useState<string | null>(null);
  const [episodeEdit, setEpisodeEdit] = useState<string | null>(null);
  const [episodeNames, setEpisodeNames] = useState<[number, string][]>([]);
  const [names, setNames] = useState<{ index: number; path: string }[]>([]);

  const present = choice.languages;
  const [langs, setLangs] = useState(() => new Set(defaults.languages.filter((l) => present.some((p) => p.code === l))));
  const [keepAll, setKeepAll] = useState(false);
  const [profile, setProfile] = useState<ProfileId>(defaults.profile);

  const yearNum = /^\d{4}$/.test(year) ? Number(year) : null;
  const season = seasonEdit !== null ? Number(seasonEdit) || 0 : (suggested?.season ?? 1);
  const firstEpisode = episodeEdit !== null ? Number(episodeEdit) || 1 : (suggested?.episode ?? 1);
  const order = useMemo(() => selection.picks.filter((p) => ticked.has(p.index)), [selection, ticked]);

  // TMDB search, a moment after typing stops
  useEffect(() => {
    if (!query.trim()) return;
    const t = setTimeout(() => {
      api
        .tmdbSearch(query)
        .then((r) => {
          setResults(r);
          setSearchError(null);
        })
        .catch((e) => setSearchError(String(e)));
    }, 400);
    return () => clearTimeout(t);
  }, [query]);

  // A show's seasons, for rolling over to the next season
  useEffect(() => {
    if (match?.kind !== "show") return;
    let current = true;
    api.tmdbShow(match.id).then((s) => current && setSeasons(s)).catch(() => {});
    return () => {
      current = false;
    };
  }, [match]);

  // Where the show is in the library and the first episode to suggest
  useEffect(() => {
    if (kind !== "shows" || !folder || !name.trim()) return;
    let current = true;
    api.libraryShow(folder, name, yearNum, seasons).then((s) => current && setSuggested(s));
    return () => {
      current = false;
    };
  }, [kind, folder, name, yearNum, seasons]);

  // Episode titles for the season
  useEffect(() => {
    if (match?.kind !== "show" || kind !== "shows") return;
    let current = true;
    api
      .tmdbSeason(match.id, season)
      .then((eps) => current && setEpisodeNames(eps.map((e) => [e.number, e.name])))
      .catch(() => {});
    return () => {
      current = false;
    };
  }, [match, kind, season]);

  // The file each ticked title will become
  const request = useMemo(
    () => ({
      root: folder,
      kind,
      name,
      year: yearNum,
      season,
      firstEpisode,
      titles: order.map((p) => ({ index: p.index, double: p.role === "doubleEpisode" })),
      episodeNames,
    }),
    [folder, kind, name, yearNum, season, firstEpisode, order, episodeNames],
  );
  useEffect(() => {
    if (!folder || !name.trim()) return;
    let current = true;
    api.libraryNames(request).then((n) => current && setNames(n));
    return () => {
      current = false;
    };
  }, [request, folder, name]);

  const [ripError, setRipError] = useState<string | null>(null);
  const ready = defaults.canRip && ticked.size > 0 && !!folder && !!name.trim() && names.length === ticked.size;
  const rip = async () => {
    const keep = keepAll ? ["all"] : [...langs, "und"];
    try {
      await api.ripStart({
        source: disc.source ?? "",
        titles: order.map((p) => ({
          index: p.index,
          target: nameOf(p.index) ?? "",
          bytes: titleOf(p.index)?.bytes ?? 0,
        })),
        options: { Content: profile, KeepLanguages: keep },
      });
      close();
    } catch (e) {
      setRipError(String(e instanceof Error ? e.message : e));
    }
  };

  const choose = (m: TmdbMatch) => {
    setMatch(m);
    setName(m.name);
    setYear(m.year ? String(m.year) : "");
    const k: LibraryKind = m.kind === "movie" ? "movies" : "shows";
    if (k !== kind) switchKind(k);
  };
  // The type decides how files are named. A library category of the other type is swapped for
  // one of this type; a folder chosen for this rip stays.
  const switchKind = (k: LibraryKind) => {
    setKind(k);
    const lib = libraryOf(folder);
    if (lib && lib.kind !== k && foldersOf(k)[0]) setFolder(foldersOf(k)[0].path);
  };
  const chooseFolder = async (value: string) => {
    if (value !== CHOOSE) {
      setFolder(value);
      const lib = libraryOf(value);
      if (lib) setKind(lib.kind);
      return;
    }
    const dir = await open({ directory: true, defaultPath: folder || undefined });
    if (typeof dir === "string") {
      setCustom((c) => (c.includes(dir) || libraryOf(dir) ? c : [...c, dir]));
      chooseFolder(dir);
    }
  };
  const toggle = (set: Set<string | number>, v: string | number) => {
    const next = new Set(set);
    if (next.has(v)) next.delete(v);
    else next.add(v);
    return next;
  };
  const titleOf = (i: number) => disc.titles.find((t) => t.index === i);
  const nameOf = (i: number) => names.find((n) => n.index === i)?.path;

  return (
    <div className="fixed inset-0 z-10 flex items-center justify-center bg-black/40" role="dialog" aria-label="Rip disc">
      <div className="max-h-[92vh] w-[44rem] overflow-auto rounded-lg bg-white p-5 shadow-xl dark:bg-neutral-900">
        <h2 className="text-base font-semibold">Rip disc</h2>
        <p className="mt-1 text-sm text-neutral-500">
          {disc.name} · {selection.summary}
        </p>

        <section className="mt-4" aria-label="Titles">
          <table className="w-full text-left text-sm">
            <tbody>
              {selection.picks.map((p) => {
                const t = titleOf(p.index);
                const target = ticked.has(p.index) ? nameOf(p.index) : undefined;
                return (
                  <tr key={p.index} className="border-t border-neutral-200 align-top dark:border-neutral-800">
                    <td className="py-1 pr-2">
                      <input
                        type="checkbox"
                        aria-label={`Title ${p.index}`}
                        checked={ticked.has(p.index)}
                        onChange={() => setTicked((s) => toggle(s, p.index) as Set<number>)}
                      />
                    </td>
                    <td className="py-1 pr-3 whitespace-nowrap text-neutral-500">Title {p.index}</td>
                    <td className="py-1 pr-3 whitespace-nowrap">{t ? minutes(t.seconds) : ""}</td>
                    <td className="py-1 pr-3 whitespace-nowrap text-neutral-500">{t ? gigabytes(t.bytes) : ""}</td>
                    <td className="py-1">
                      {p.reason}
                      {target && <span className="block text-xs break-all text-blue-700 dark:text-blue-300">→ {shortPath(target)}</span>}
                    </td>
                  </tr>
                );
              })}
            </tbody>
          </table>
        </section>

        <section className="mt-4 grid grid-cols-2 gap-3" aria-label="Name">
          <label className="col-span-2 block text-sm">
            <span className={label}>Search TMDB</span>
            <input className={input} value={query} onChange={(e) => setQuery(e.target.value)} />
          </label>
          {searchError && <p className="col-span-2 text-xs text-amber-700">{searchError}</p>}
          {results.length > 0 && (
            <ul className="col-span-2 max-h-36 overflow-auto rounded border border-neutral-200 text-sm dark:border-neutral-700" aria-label="Search results">
              {results.slice(0, 8).map((m) => (
                <li key={`${m.kind}-${m.id}`}>
                  <button
                    type="button"
                    onClick={() => choose(m)}
                    className={`w-full px-2 py-1 text-left hover:bg-neutral-100 dark:hover:bg-neutral-800 ${match?.id === m.id ? "bg-blue-50 dark:bg-blue-950" : ""}`}
                  >
                    {m.name}
                    {m.year ? ` (${m.year})` : ""} · {m.kind === "movie" ? "Movie" : "TV"}
                    {m.overview && <span className="block truncate text-xs text-neutral-500">{m.overview}</span>}
                  </button>
                </li>
              ))}
            </ul>
          )}
          <label className="block text-sm">
            <span className={label}>Name</span>
            <input className={input} value={name} onChange={(e) => setName(e.target.value)} />
          </label>
          <label className="block text-sm">
            <span className={label}>Year</span>
            <input className={input} inputMode="numeric" value={year} onChange={(e) => setYear(e.target.value)} />
          </label>
          <div className="block text-sm" role="radiogroup" aria-label="Type">
            <span className={label}>Type</span>
            {(["movies", "shows"] as LibraryKind[]).map((k) => (
              <label key={k} className="mr-4 inline-flex items-center gap-1">
                <input type="radio" name="kind" checked={kind === k} onChange={() => switchKind(k)} />
                {k === "movies" ? "Movie" : "Show"}
              </label>
            ))}
          </div>
          <label className="block text-sm">
            <span className={label}>Save to</span>
            <select className={input} value={folder} onChange={(e) => chooseFolder(e.target.value)}>
              {!folder && <option value="">Pick a library or a folder</option>}
              {defaults.folders.length > 0 && (
                <optgroup label="Libraries">
                  {defaults.folders.map((f) => (
                    <option key={f.path} value={f.path}>
                      {folderLabel(f)} · {f.kind === "movies" ? "Movies" : "Shows"}
                    </option>
                  ))}
                </optgroup>
              )}
              {custom.length > 0 && (
                <optgroup label="This rip">
                  {custom.map((c) => (
                    <option key={c} value={c}>
                      {c}
                    </option>
                  ))}
                </optgroup>
              )}
              <option value={CHOOSE}>Choose a folder…</option>
            </select>
          </label>
          {kind === "shows" && (
            <>
              <label className="block text-sm">
                <span className={label}>Season</span>
                <input className={input} inputMode="numeric" value={seasonEdit ?? String(season)} onChange={(e) => setSeasonEdit(e.target.value)} />
              </label>
              <label className="block text-sm">
                <span className={label}>First episode</span>
                <input className={input} inputMode="numeric" value={episodeEdit ?? String(firstEpisode)} onChange={(e) => setEpisodeEdit(e.target.value)} />
                {suggested?.layout.last && episodeEdit === null && (
                  <span className="mt-0.5 block text-xs text-neutral-500">
                    The library has up to S{String(suggested.layout.last[0]).padStart(2, "0")}E{String(suggested.layout.last[1]).padStart(2, "0")}
                  </span>
                )}
              </label>
            </>
          )}
        </section>

        <section className="mt-4" aria-label="Languages">
          <span className={label}>Languages to keep</span>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              aria-pressed={keepAll}
              onClick={() => setKeepAll((k) => !k)}
              className={`rounded-full border px-3 py-0.5 text-sm ${keepAll ? "border-blue-600 bg-blue-50 text-blue-700 dark:bg-blue-950 dark:text-blue-200" : "border-neutral-300 dark:border-neutral-600"}`}
            >
              Keep all
            </button>
            {present.map((l) => {
              const on = keepAll || langs.has(l.code);
              return (
                <button
                  key={l.code}
                  type="button"
                  aria-pressed={on}
                  disabled={keepAll}
                  onClick={() => setLangs((s) => toggle(s, l.code) as Set<string>)}
                  className={`rounded-full border px-3 py-0.5 text-sm disabled:opacity-60 ${on ? "border-blue-600 bg-blue-50 text-blue-700 dark:bg-blue-950 dark:text-blue-200" : "border-neutral-300 dark:border-neutral-600"}`}
                >
                  {l.name}
                </button>
              );
            })}
          </div>
        </section>

        <section className="mt-4" aria-label="Profile">
          <span className={label}>Profile</span>
          <div className="grid grid-cols-5 gap-2" role="radiogroup" aria-label="Profile">
            {(Object.keys(PROFILE_LABELS) as ProfileId[]).map((p) => (
              <button
                key={p}
                type="button"
                role="radio"
                aria-checked={profile === p}
                onClick={() => setProfile(p)}
                className={`rounded border px-2 py-1.5 text-xs font-medium ${profile === p ? "border-blue-600 bg-blue-50 text-blue-700 dark:bg-blue-950 dark:text-blue-200" : "border-neutral-300 dark:border-neutral-600"}`}
              >
                {PROFILE_LABELS[p]}
              </button>
            ))}
          </div>
        </section>

        <div className="mt-5 flex items-center justify-end gap-2">
          <span className="mr-auto text-xs text-neutral-500">
            {!defaults.canRip ? (
              <span className="text-red-600">Register MakeMKV in Settings → Ripping to rip discs.</span>
            ) : ripError ? (
              <span className="text-red-600">{ripError}</span>
            ) : !folder ? (
              "Pick where it goes: a library from Settings → Ripping, or Choose a folder…"
            ) : (
              "Each title starts encoding as soon as it's ripped."
            )}
          </span>
          <button type="button" className="rounded px-3 py-1.5 text-sm hover:bg-neutral-100 dark:hover:bg-neutral-800" onClick={close}>
            Close
          </button>
          <button
            type="button"
            disabled={!ready}
            onClick={rip}
            className="rounded bg-blue-600 px-3 py-1.5 text-sm font-medium text-white disabled:opacity-40"
          >
            Rip {ticked.size} {ticked.size === 1 ? "title" : "titles"}
          </button>
        </div>
      </div>
    </div>
  );
}
