// A sample TV disc for trying the Rip sheet in a plain browser (`pnpm dev`, then open
// /?demo=rip), without Tauri, a drive or a TMDB token. Never used inside the app.

import { setDemo, type Disc, type DiscChoice, type NameRequest } from "../lib/api";
import { useQueue } from "../queue/store";

const track = (kind: "video" | "audio" | "subtitle", language: string) => ({ kind, language });
const tracks = [
  track("video", ""),
  track("audio", "jpn"),
  track("audio", "eng"),
  track("audio", "spa"),
  track("subtitle", "eng"),
  track("subtitle", "spa"),
  track("subtitle", "chi"),
];
const title = (index: number, minutes: number, gb: number, source: string) => ({
  index,
  seconds: minutes * 60,
  bytes: gb * 2 ** 30,
  chapters: 6,
  source,
  segments: [],
  tracks,
});

export const demoDisc: Disc = {
  name: "SHOW_ALPHA_D2",
  titles: [
    title(0, 96, 16.4, "00800.mpls"),
    title(1, 23.6, 4.0, "00801.mpls"),
    title(2, 23.9, 4.1, "00802.mpls"),
    title(3, 47.2, 8.1, "00803.mpls"),
    title(4, 23.7, 4.2, "00804.mpls"),
    title(5, 1.5, 0.2, "00805.mpls"),
  ],
};

const demoChoice: DiscChoice = {
  selection: {
    kind: "episodes",
    summary: "3 episodes of about 24 min, plus 1 double-length",
    picks: [
      { index: 1, role: "episode", ticked: true, reason: "Episode, 24 min" },
      { index: 2, role: "episode", ticked: true, reason: "Episode, 24 min" },
      { index: 3, role: "doubleEpisode", ticked: true, reason: "Double-length episode, 47 min" },
      { index: 4, role: "episode", ticked: true, reason: "Episode, 24 min" },
      { index: 0, role: "playAll", ticked: false, reason: "Plays the episodes back to back" },
      { index: 5, role: "extra", ticked: false, reason: "Short, 2 min" },
    ],
  },
  languages: [
    { code: "eng", name: "English" },
    { code: "spa", name: "Spanish" },
    { code: "chi", name: "Chinese" },
    { code: "jpn", name: "Japanese" },
  ],
};

const SHOWS = "/zt6k/Anime Shows";

export function enableDemo() {
  setDemo((cmd, args) => {
    switch (cmd) {
      case "settings_get":
        return {
          outputDir: "/zt6k/Upscaled",
          toolsDir: "",
          lastProfile: "anime-av1",
          libraryFolders: [
            { name: "Anime Shows", path: SHOWS, kind: "shows" },
            { name: "TV Shows", path: "/zt6k/TV Shows", kind: "shows" },
            { name: "Anime Movies", path: "/zt6k/Anime Movies", kind: "movies" },
          ],
          keepLanguages: ["eng", "jpn", "spa"],
        };
      case "disc_choose":
        return demoChoice;
      case "tmdb_search":
        return [
          { id: 2304, kind: "show", name: "Show Alpha", year: 1992, originalLanguage: "ja", overview: "A detective story in a harbor town." },
          { id: 207, kind: "show", name: "Show Alpha", year: 2023, originalLanguage: "ja", overview: "Live-action series." },
          { id: 11, kind: "movie", name: "Show Alpha: The Movie", year: 1993, originalLanguage: "ja", overview: "" },
        ];
      case "tmdb_show":
        return [
          { number: 1, episodes: 28 },
          { number: 2, episodes: 38 },
        ];
      case "tmdb_season":
        return Array.from({ length: 28 }, (_, i) => ({ number: i + 1, name: `Episode ${i + 1} title`, runtime: 23 }));
      case "library_show":
        return {
          layout: { dir: `${SHOWS}/Show Alpha`, exists: true, prefix: "Show Alpha", last: [1, 6] },
          season: 1,
          episode: 7,
        };
      case "queue_move_to": {
        // Reorders the queue shown, as the engine would
        const { id, index } = args as { id: string; index: number };
        const { snapshot } = useQueue.getState();
        const jobs = snapshot.jobs.filter((j) => j.id !== id);
        const moved = snapshot.jobs.find((j) => j.id === id);
        if (moved) jobs.splice(Math.min(index, jobs.length), 0, moved);
        useQueue.getState().setSnapshot({ ...snapshot, jobs });
        return null;
      }
      case "library_names": {
        const r = (args as { request: NameRequest }).request;
        let next = r.firstEpisode;
        return r.titles.map((t) => {
          const ep = t.episode ?? next;
          next += t.double ? 2 : 1;
          const n = String(ep).padStart(2, "0");
          const name = t.double ? `S0${r.season}E${n}-E${String(ep + 1).padStart(2, "0")}` : `S0${r.season}E${n}`;
          const file = t.fileName?.trim().replace(/\.mkv$/i, "") || `${r.name} ${name}`;
          return { index: t.index, path: `${r.root}/${r.name}/Season ${r.season}/${file}.mkv` };
        });
      }
      default:
        return undefined;
    }
  });
}
