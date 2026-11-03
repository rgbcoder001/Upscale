//! Jellyfin file names for ripped titles (RIPPING.md, "Naming"). A show or movie library that
//! already exists decides the style: flat `Name (Year).mkv` files or a folder per movie,
//! `Season 1` or `Season 01`, the file prefix (`The Example Show S01E01` in a folder
//! named `Example Show`), and whether episode titles follow ` - `.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

static EPISODE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)^(.*?)[ ._-]*S(\d{1,2})E(\d{1,3})(?:-?E(\d{1,3}))?(.*)$").expect("valid regex")
});
static SEASON_DIR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^season\s*(\d+)$").expect("valid regex"));
static YEAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\s*\((\d{4})\)\s*$").expect("valid regex"));

const VIDEO: &[&str] = &["mkv", "mp4", "m4v", "avi", "ts", "m2ts"];

/// A name that's a valid file name on Windows and macOS: `:` becomes ` -`, the other
/// reserved characters go, and trailing dots and spaces are trimmed.
pub fn safe_name(name: &str) -> String {
    let s: String = name
        .replace(": ", " - ")
        .replace(':', "-")
        .chars()
        .filter(|c| !matches!(c, '/' | '\\' | '?' | '*' | '<' | '>' | '|' | '"') && !c.is_control())
        .collect();
    let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
    s.trim_end_matches(['.', ' ']).to_string()
}

/// `Name (Year)`, or `Name` without a year.
pub fn with_year(name: &str, year: Option<u32>) -> String {
    match year {
        Some(y) => format!("{} ({y})", safe_name(name)),
        None => safe_name(name),
    }
}

/// For matching folder names: lowercase letters and digits only, without a trailing year or
/// a leading "the".
fn key(name: &str) -> String {
    let s = YEAR.replace(name, "").to_lowercase();
    let s = s.strip_prefix("the ").unwrap_or(&s);
    s.chars().filter(char::is_ascii_alphanumeric).collect()
}

fn is_video(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO.iter().any(|v| v.eq_ignore_ascii_case(e)))
}

fn entries(dir: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(dir)
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path()).collect())
        .unwrap_or_default()
}

/// Where a movie goes: flat in the root when the library keeps movies as files there (most
/// do), else in its own folder.
pub fn movie_path(root: &Path, name: &str, year: Option<u32>) -> PathBuf {
    let file = format!("{}.mkv", with_year(name, year));
    let items = entries(root);
    let files = items.iter().filter(|p| p.is_file() && is_video(p)).count();
    let folders = items.iter().filter(|p| p.is_dir()).count();
    if files >= folders {
        root.join(file)
    } else {
        root.join(with_year(name, year)).join(file)
    }
}

/// How a show's files are named in its library.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShowLayout {
    /// The show's folder (existing or new).
    pub dir: PathBuf,
    pub exists: bool,
    /// What episode files start with (`The Example Show`).
    pub prefix: String,
    /// `Season 01` instead of `Season 1`.
    pub padded_seasons: bool,
    /// Episode titles after ` - `.
    pub episode_titles: bool,
    /// The last episode already there, as (season, episode); seasons over 0 only.
    pub last: Option<(u32, u32)>,
}

/// The show's folder in `root` (matched loosely: case, punctuation, a year and "The" don't
/// matter) and the style its files use. A new show gets `Name (Year)`, `Season 1` and no
/// episode titles, like most of the library.
pub fn show_layout(root: &Path, name: &str, year: Option<u32>) -> ShowLayout {
    let want = key(name);
    let existing = entries(root).into_iter().filter(|p| p.is_dir()).find(|p| {
        p.file_name()
            .is_some_and(|n| key(&n.to_string_lossy()) == want)
    });
    let Some(dir) = existing else {
        return ShowLayout {
            dir: root.join(with_year(name, year)),
            exists: false,
            prefix: safe_name(name),
            padded_seasons: false,
            episode_titles: false,
            last: None,
        };
    };
    let folder_name = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut layout = ShowLayout {
        prefix: YEAR.replace(&folder_name, "").into_owned(),
        dir: dir.clone(),
        exists: true,
        padded_seasons: false,
        episode_titles: false,
        last: None,
    };
    let mut prefix_set = false;
    for season_dir in entries(&dir).into_iter().filter(|p| p.is_dir()) {
        let season_name = season_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let Some(c) = SEASON_DIR.captures(&season_name) else {
            continue;
        };
        if c[1].len() == 2 && c[1].starts_with('0') {
            layout.padded_seasons = true;
        }
        for file in entries(&season_dir)
            .into_iter()
            .filter(|p| p.is_file() && is_video(p))
        {
            let stem = file
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let Some(m) = EPISODE.captures(&stem) else {
                continue;
            };
            let season: u32 = m[2].parse().unwrap_or(0);
            let last_ep: u32 = m
                .get(4)
                .unwrap_or_else(|| m.get(3).expect("episode"))
                .as_str()
                .parse()
                .unwrap_or(0);
            if !prefix_set && !m[1].trim().is_empty() {
                layout.prefix = m[1].trim().to_string();
                prefix_set = true;
            }
            if m[5].trim_start().starts_with("- ") {
                layout.episode_titles = true;
            }
            if season > 0 && layout.last.is_none_or(|l| (season, last_ep) > l) {
                layout.last = Some((season, last_ep));
            }
        }
    }
    layout
}

/// The first episode to suggest: one after the last one in the library, rolling over to the
/// next season when TMDB says the season is complete (`season_sizes`: episodes per season).
/// Without anything in the library, S01E01.
pub fn suggest_first(layout: &ShowLayout, season_sizes: &[(u32, u32)]) -> (u32, u32) {
    let Some((season, episode)) = layout.last else {
        return (1, 1);
    };
    let full = season_sizes
        .iter()
        .find(|(s, _)| *s == season)
        .is_some_and(|(_, count)| episode >= *count);
    let next_season_exists = season_sizes.iter().any(|(s, _)| *s == season + 1);
    if full && next_season_exists {
        (season + 1, 1)
    } else {
        (season, episode + 1)
    }
}

/// `Show/Season 1/Prefix S01E07 - Title.mkv`; a double episode covers two numbers
/// (`S01E07-E08`). The title is only used when the library uses titles.
pub fn episode_path(
    layout: &ShowLayout,
    season: u32,
    episode: u32,
    double: bool,
    title: Option<&str>,
) -> PathBuf {
    let season_dir = if season == 0 {
        "Specials".to_string()
    } else if layout.padded_seasons {
        format!("Season {season:02}")
    } else {
        format!("Season {season}")
    };
    let mut name = format!("{} S{season:02}E{episode:02}", safe_name(&layout.prefix));
    if double {
        name.push_str(&format!("-E{:02}", episode + 1));
    }
    if layout.episode_titles
        && let Some(t) = title.map(safe_name).filter(|t| !t.is_empty())
    {
        name.push_str(&format!(" - {t}"));
    }
    layout.dir.join(season_dir).join(format!("{name}.mkv"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(p: &Path) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, b"").unwrap();
    }

    #[test]
    fn safe_names() {
        assert_eq!(
            safe_name("Sample: The Sample Picture"),
            "Sample - The Sample Picture"
        );
        assert_eq!(safe_name("What If...? / Part 2."), "What If... Part 2");
        assert_eq!(safe_name("Re:Zero"), "Re-Zero");
        assert_eq!(
            with_year("The Sample Picture", Some(2008)),
            "The Sample Picture (2008)"
        );
    }

    #[test]
    fn movies_follow_the_library() {
        let flat = tempfile::tempdir().unwrap();
        touch(&flat.path().join("Film One (1988).mkv"));
        touch(&flat.path().join("Collection").join("A (2001).mkv"));
        assert_eq!(
            movie_path(flat.path(), "The Sample Picture", Some(2008)),
            flat.path().join("The Sample Picture (2008).mkv")
        );
        let foldered = tempfile::tempdir().unwrap();
        touch(
            &foldered
                .path()
                .join("Film One (1988)")
                .join("Film One (1988).mkv"),
        );
        assert_eq!(
            movie_path(foldered.path(), "The Sample Picture", Some(2008)),
            foldered
                .path()
                .join("The Sample Picture (2008)")
                .join("The Sample Picture (2008).mkv")
        );
    }

    #[test]
    fn an_existing_show_decides_the_style_and_the_next_episode() {
        let root = tempfile::tempdir().unwrap();
        let show = root.path().join("Example Show");
        for e in 1..=6 {
            touch(
                &show
                    .join("Season 1")
                    .join(format!("The Example Show S01E{e:02} - Ep {e}.mkv")),
            );
        }
        touch(
            &show
                .join("Specials")
                .join("The Example Show S00E01.mkv"),
        );
        touch(&show.join("Season 1").join("cover.jpg"));
        let l = show_layout(root.path(), "The Example Show", Some(2022));
        assert!(l.exists && l.episode_titles && !l.padded_seasons);
        assert_eq!(
            (l.dir.clone(), l.prefix.as_str(), l.last),
            (show.clone(), "The Example Show", Some((1, 6)))
        );
        assert_eq!(suggest_first(&l, &[]), (1, 7));
        assert_eq!(
            episode_path(&l, 1, 7, false, Some("The Hidden: Truth")),
            show.join("Season 1")
                .join("The Example Show S01E07 - The Hidden - Truth.mkv")
        );
    }

    #[test]
    fn a_complete_season_rolls_over_and_doubles_take_two_numbers() {
        let root = tempfile::tempdir().unwrap();
        let show = root.path().join("Show Alpha (1992)");
        touch(&show.join("Season 01").join("Show Alpha S01E01-E02.mkv"));
        let l = show_layout(root.path(), "show alpha", None);
        assert!(l.padded_seasons && !l.episode_titles);
        assert_eq!((l.prefix.as_str(), l.last), ("Show Alpha", Some((1, 2))));
        assert_eq!(suggest_first(&l, &[(1, 2), (2, 26)]), (2, 1));
        assert_eq!(suggest_first(&l, &[(1, 28)]), (1, 3));
        assert_eq!(
            episode_path(&l, 1, 3, true, Some("ignored")),
            show.join("Season 01").join("Show Alpha S01E03-E04.mkv")
        );
    }

    #[test]
    fn a_new_show() {
        let root = tempfile::tempdir().unwrap();
        let l = show_layout(root.path(), "Show Beta: A Long Journey's End", Some(2023));
        assert!(!l.exists);
        assert_eq!(suggest_first(&l, &[(1, 28)]), (1, 1));
        assert_eq!(
            episode_path(&l, 1, 1, false, Some("x")),
            root.path()
                .join("Show Beta - A Long Journey's End (2023)")
                .join("Season 1")
                .join("Show Beta - A Long Journey's End S01E01.mkv")
        );
    }
}
