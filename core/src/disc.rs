//! A disc's titles (from MakeMKV's `info`, RIPPING.md) and which of them to rip. Choosing is a
//! pure function, so every disc layout has a test without a drive.

use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};

use crate::lang::{iso_lang, language_name};

/// Titles shorter than this are menus, logos or trailers.
pub const MIN_TITLE_SECS: f64 = 120.0;
/// A feature film is at least this long.
const MOVIE_SECS: f64 = 3600.0;
/// Episodes in one set are within this fraction of their median length.
const EPISODE_SPREAD: f64 = 0.25;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Disc {
    /// The disc's title when it has one ("Sample Movie"), else the volume name
    /// (`SHOW_ALPHA_D2`).
    pub name: String,
    pub titles: Vec<Title>,
    /// What MakeMKV opens it as (`dev:/dev/rdisk8`); empty for a disc that isn't in a drive.
    #[serde(default)]
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Title {
    /// MakeMKV's title index (what `mkv disc:N <index>` rips).
    pub index: u32,
    pub seconds: f64,
    pub bytes: u64,
    #[serde(default)]
    pub chapters: u32,
    /// Blu-ray playlist (`00801.mpls`) or DVD title name.
    #[serde(default)]
    pub source: String,
    /// The clips the title plays, in order (Blu-ray segment map); empty when unknown.
    #[serde(default)]
    pub segments: Vec<u32>,
    #[serde(default)]
    pub tracks: Vec<Track>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub kind: TrackKind,
    /// Language as the disc gives it (ISO 639-2 on Blu-ray).
    #[serde(default)]
    pub language: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TrackKind {
    Video,
    Audio,
    Subtitle,
}

/// What a disc holds, as far as the heuristic can tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiscKind {
    Movie,
    Episodes,
    Unknown,
}

/// Why a title is (or isn't) ticked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    /// The feature (or one of two features).
    Main,
    /// A second cut of the feature, about as long.
    AlternateCut,
    Episode,
    /// About twice an episode's length: a double-length special.
    DoubleEpisode,
    /// Plays the episodes back to back.
    PlayAll,
    /// The same video as an earlier title (another playlist for it).
    Duplicate,
    /// Too short, or doesn't fit the pattern: extras, trailers, menus.
    Extra,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Pick {
    pub index: u32,
    pub role: Role,
    pub ticked: bool,
    /// Shown next to the title, e.g. "Episode, 24 min" or "Same as title 3".
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Selection {
    pub kind: DiscKind,
    /// One line for the sheet, e.g. "4 episodes of about 24 min".
    pub summary: String,
    /// Every title; ticked episodes come first in playing order, then the rest by index.
    pub picks: Vec<Pick>,
}

fn minutes(secs: f64) -> String {
    format!("{} min", (secs / 60.0).round() as u64)
}

fn near(a: f64, b: f64, fraction: f64) -> bool {
    (a - b).abs() <= b * fraction
}

/// Playing order: the playlist number (`00801.mpls` → 801), then the title index.
fn order_key(t: &Title) -> (u32, u32) {
    let digits: String = t.source.chars().take_while(char::is_ascii_digit).collect();
    (digits.parse().unwrap_or(u32::MAX), t.index)
}

/// Picks the titles to rip (RIPPING.md, "Choosing titles").
pub fn choose_titles(disc: &Disc) -> Selection {
    let mut roles: Vec<(u32, Role, String)> = Vec::new();
    let mut kept: Vec<&Title> = Vec::new();
    for t in &disc.titles {
        if t.seconds < MIN_TITLE_SECS {
            roles.push((
                t.index,
                Role::Extra,
                format!("Short, {}", minutes(t.seconds)),
            ));
            continue;
        }
        // Episodes of one show are often within 1% of each other in length and size, so only
        // the segment map, or a near-exact match without one, means "the same video"
        let same = kept.iter().find(|k| {
            if !t.segments.is_empty() && !k.segments.is_empty() {
                k.segments == t.segments
            } else {
                (t.seconds - k.seconds).abs() <= 1.0 && near(t.bytes as f64, k.bytes as f64, 0.001)
            }
        });
        match same {
            Some(k) => roles.push((
                t.index,
                Role::Duplicate,
                format!("Same as title {}", k.index),
            )),
            None => kept.push(t),
        }
    }

    // The largest set of titles of about the same length
    let cluster: Vec<&Title> = kept
        .iter()
        .map(|t| {
            kept.iter()
                .copied()
                .filter(|o| near(o.seconds, t.seconds, EPISODE_SPREAD))
                .collect::<Vec<_>>()
        })
        .max_by(|a, b| {
            a.len()
                .cmp(&b.len())
                .then_with(|| total(a).total_cmp(&total(b)))
        })
        .unwrap_or_default();
    let median = median_secs(&cluster);
    // A feature that dominates makes it a movie even when short extras (deleted scenes) form
    // a cluster: at least an hour, twice as long as anything else, and not a title that plays
    // other titles back to back (its segments hold theirs, or it's as long as the cluster)
    let mut by_length: Vec<&Title> = kept.clone();
    by_length.sort_by(|a, b| b.seconds.total_cmp(&a.seconds));
    let plays_others = |t: &Title| {
        let inside = kept
            .iter()
            .filter(|o| o.index != t.index && !o.segments.is_empty())
            .filter(|o| o.segments.iter().all(|s| t.segments.contains(s)))
            .count();
        inside >= 2
            || (cluster.len() >= 2
                && !cluster.iter().any(|c| c.index == t.index)
                && near(t.seconds, total(&cluster), 0.1))
    };
    let dominant_feature = by_length.first().is_some_and(|f| {
        f.seconds >= MOVIE_SECS
            && !plays_others(f)
            && by_length
                .get(1)
                .is_none_or(|s| f.seconds >= 2.0 * s.seconds)
    });
    let episodes =
        !dominant_feature && (cluster.len() >= 3 || (cluster.len() == 2 && median < MOVIE_SECS));

    let kind = if episodes {
        let sum: f64 = cluster.iter().map(|t| t.seconds).sum();
        let segs: BTreeSet<u32> = cluster
            .iter()
            .flat_map(|t| t.segments.iter().copied())
            .collect();
        for t in &kept {
            let (role, reason) = if cluster.iter().any(|c| c.index == t.index) {
                (Role::Episode, format!("Episode, {}", minutes(t.seconds)))
            } else if near(t.seconds, sum, 0.1)
                || (!segs.is_empty() && segs.iter().all(|s| t.segments.contains(s)))
            {
                (Role::PlayAll, "Plays the episodes back to back".to_string())
            } else if near(t.seconds, 2.0 * median, EPISODE_SPREAD) {
                (
                    Role::DoubleEpisode,
                    format!("Double-length episode, {}", minutes(t.seconds)),
                )
            } else {
                (Role::Extra, format!("Extra, {}", minutes(t.seconds)))
            };
            roles.push((t.index, role, reason));
        }
        DiscKind::Episodes
    } else if let Some(longest) = kept
        .iter()
        .copied()
        .max_by(|a, b| a.seconds.total_cmp(&b.seconds))
    {
        if longest.seconds >= MOVIE_SECS {
            let features: Vec<&Title> = kept
                .iter()
                .copied()
                .filter(|t| t.seconds >= longest.seconds / 2.0 && t.seconds >= MOVIE_SECS)
                .collect();
            let alternate =
                features.len() == 2 && near(features[0].seconds, features[1].seconds, 0.15);
            let main = features
                .iter()
                .copied()
                .max_by_key(|t| t.bytes)
                .map(|t| t.index);
            for t in &kept {
                let (role, reason) = if !features.iter().any(|f| f.index == t.index) {
                    (Role::Extra, format!("Extra, {}", minutes(t.seconds)))
                } else if alternate && Some(t.index) != main {
                    (
                        Role::AlternateCut,
                        format!("Another cut, {}", minutes(t.seconds)),
                    )
                } else {
                    (Role::Main, format!("Feature, {}", minutes(t.seconds)))
                };
                roles.push((t.index, role, reason));
            }
            DiscKind::Movie
        } else {
            // One episode-length title, or nothing recognisable: offer the longest
            for t in &kept {
                let (role, reason) = if t.index == longest.index {
                    (
                        Role::Episode,
                        format!("Longest title, {}", minutes(t.seconds)),
                    )
                } else {
                    (Role::Extra, format!("Extra, {}", minutes(t.seconds)))
                };
                roles.push((t.index, role, reason));
            }
            DiscKind::Unknown
        }
    } else {
        DiscKind::Unknown
    };

    let by_index = |i: u32| disc.titles.iter().find(|t| t.index == i).expect("title");
    let mut picks: Vec<Pick> = roles
        .into_iter()
        .map(|(index, role, reason)| Pick {
            index,
            ticked: matches!(role, Role::Main | Role::Episode | Role::DoubleEpisode),
            role,
            reason,
        })
        .collect();
    picks.sort_by_key(|p| {
        let t = by_index(p.index);
        if p.ticked {
            (0, order_key(t))
        } else {
            (1, (t.index, 0))
        }
    });
    let ticked = picks.iter().filter(|p| p.ticked).count();
    let summary = match kind {
        DiscKind::Movie if ticked > 1 => format!("{ticked} features"),
        DiscKind::Movie => "Movie".to_string(),
        DiscKind::Episodes => {
            let doubles = picks
                .iter()
                .filter(|p| p.role == Role::DoubleEpisode)
                .count();
            let mut s = format!("{} episodes of about {}", ticked - doubles, minutes(median));
            if doubles > 0 {
                s.push_str(&format!(", plus {doubles} double-length"));
            }
            s
        }
        DiscKind::Unknown => "Couldn't tell; pick the titles to rip".to_string(),
    };
    Selection {
        kind,
        summary,
        picks,
    }
}

fn total(titles: &[&Title]) -> f64 {
    titles.iter().map(|t| t.seconds).sum()
}

fn median_secs(titles: &[&Title]) -> f64 {
    let mut s: Vec<f64> = titles.iter().map(|t| t.seconds).collect();
    s.sort_by(f64::total_cmp);
    match s.len() {
        0 => 0.0,
        n if n % 2 == 1 => s[n / 2],
        n => (s[n / 2 - 1] + s[n / 2]) / 2.0,
    }
}

/// The audio and subtitle languages on the disc's ticked titles (or all titles when none are
/// ticked), as `(ISO 639-2, name)`: spoken languages first (most audio tracks first), then
/// languages that only have subtitles.
pub fn languages(disc: &Disc, selection: &Selection) -> Vec<(String, String)> {
    let ticked: BTreeSet<u32> = selection
        .picks
        .iter()
        .filter(|p| p.ticked)
        .map(|p| p.index)
        .collect();
    // (language, audio tracks, all tracks)
    let mut counts: Vec<(String, usize, usize)> = Vec::new();
    for t in disc
        .titles
        .iter()
        .filter(|t| ticked.is_empty() || ticked.contains(&t.index))
    {
        for tr in t.tracks.iter().filter(|tr| tr.kind != TrackKind::Video) {
            let code = iso_lang(Some(&tr.language), None);
            let audio = usize::from(tr.kind == TrackKind::Audio);
            match counts.iter_mut().find(|(c, _, _)| *c == code) {
                Some(entry) => {
                    entry.1 += audio;
                    entry.2 += 1;
                }
                None => counts.push((code, audio, 1)),
            }
        }
    }
    counts.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| b.2.cmp(&a.2))
            .then_with(|| a.0.cmp(&b.0))
    });
    counts
        .into_iter()
        .map(|(code, _, _)| {
            let name = language_name(&code);
            (code, name)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const GB: u64 = 1 << 30;

    fn title(index: u32, minutes: f64, gb: f64) -> Title {
        Title {
            index,
            seconds: minutes * 60.0,
            bytes: (gb * GB as f64) as u64,
            chapters: 6,
            source: format!("{:05}.mpls", 800 + index),
            segments: vec![],
            tracks: vec![],
        }
    }

    fn disc(titles: Vec<Title>) -> Selection {
        choose_titles(&Disc {
            name: "TEST".into(),
            titles,
            source: String::new(),
        })
    }

    fn ticked(s: &Selection) -> Vec<u32> {
        s.picks
            .iter()
            .filter(|p| p.ticked)
            .map(|p| p.index)
            .collect()
    }

    fn role(s: &Selection, index: u32) -> Role {
        s.picks.iter().find(|p| p.index == index).unwrap().role
    }

    #[test]
    fn one_movie_with_extras() {
        let s = disc(vec![
            title(0, 0.5, 0.05),
            title(1, 152.0, 32.0),
            title(2, 12.0, 1.5),
            title(3, 4.0, 0.4),
        ]);
        assert_eq!(s.kind, DiscKind::Movie);
        assert_eq!(ticked(&s), [1]);
        assert_eq!(s.summary, "Movie");
        assert_eq!(role(&s, 0), Role::Extra);
        assert_eq!(role(&s, 2), Role::Extra);
    }

    #[test]
    fn two_cuts_tick_the_bigger_one() {
        let s = disc(vec![
            title(0, 152.0, 30.0),
            title(1, 164.0, 33.0),
            title(2, 20.0, 2.0),
        ]);
        assert_eq!(s.kind, DiscKind::Movie);
        assert_eq!(ticked(&s), [1]);
        assert_eq!(role(&s, 0), Role::AlternateCut);
    }

    #[test]
    fn a_double_feature_ticks_both() {
        let s = disc(vec![title(0, 95.0, 20.0), title(1, 130.0, 28.0)]);
        assert_eq!((s.kind, ticked(&s)), (DiscKind::Movie, vec![0, 1]));
        assert_eq!(s.summary, "2 features");
    }

    #[test]
    fn a_tv_disc_ticks_the_episodes_in_playing_order() {
        let mut titles = vec![
            title(0, 23.8, 4.1),
            title(1, 23.6, 4.0),
            title(2, 24.1, 4.2),
            title(3, 23.9, 4.1),
            title(4, 3.0, 0.3),
        ];
        // Playlists out of index order: title 3 plays first
        titles[3].source = "00700.mpls".into();
        let s = disc(titles);
        assert_eq!(s.kind, DiscKind::Episodes);
        assert_eq!(ticked(&s), [3, 0, 1, 2]);
        assert_eq!(s.summary, "4 episodes of about 24 min");
        assert_eq!(role(&s, 4), Role::Extra);
    }

    #[test]
    fn a_double_length_special_is_ticked_and_marked() {
        let s = disc(vec![
            title(0, 24.0, 4.0),
            title(1, 24.0, 4.1),
            title(2, 47.5, 8.0),
            title(3, 23.5, 3.9),
        ]);
        assert_eq!(ticked(&s), [0, 1, 2, 3]);
        assert_eq!(role(&s, 2), Role::DoubleEpisode);
        assert_eq!(
            s.summary,
            "3 episodes of about 24 min, plus 1 double-length"
        );
    }

    #[test]
    fn play_all_and_duplicate_playlists_are_skipped() {
        let mut titles = vec![
            title(0, 96.0, 16.2), // play all
            title(1, 24.0, 4.0),
            title(2, 24.0, 4.1),
            title(3, 24.0, 4.05),
            title(4, 24.0, 4.0),
        ];
        for (i, t) in titles.iter_mut().enumerate().skip(1) {
            t.segments = vec![i as u32];
        }
        titles[0].segments = vec![1, 2, 3, 4];
        // Another playlist for episode 1
        let mut again = title(5, 24.0, 4.0);
        again.segments = vec![1];
        titles.push(again);
        let s = disc(titles);
        assert_eq!(ticked(&s), [1, 2, 3, 4]);
        assert_eq!(role(&s, 0), Role::PlayAll);
        assert_eq!(role(&s, 5), Role::Duplicate);
        let dup = s.picks.iter().find(|p| p.index == 5).unwrap();
        assert_eq!(dup.reason, "Same as title 1");
    }

    #[test]
    fn a_movie_with_many_short_extras_is_still_a_movie() {
        // The shape of a real disc: a 2 h feature, a dozen 2-5 min deleted scenes and a title
        // that plays them back to back
        let mut titles = vec![title(0, 128.5, 29.9), title(1, 27.6, 2.4)];
        titles[0].segments = vec![20];
        titles[1].segments = (60..72).collect();
        for (i, m) in [2.1, 2.2, 4.9, 3.8, 4.1, 3.4, 4.2, 3.2, 3.8, 3.5, 3.5, 2.2]
            .iter()
            .enumerate()
        {
            let mut t = title(2 + i as u32, *m, 0.3);
            t.segments = vec![60 + i as u32];
            titles.push(t);
        }
        let s = disc(titles);
        assert_eq!((s.kind, ticked(&s)), (DiscKind::Movie, vec![0]));
    }

    #[test]
    fn play_all_without_segment_maps_is_not_a_movie() {
        // A DVD: no segment maps, a 96 min "play all" of four 24 min episodes
        let s = disc(vec![
            title(0, 96.2, 16.0),
            title(1, 24.0, 4.0),
            title(2, 24.1, 4.1),
            title(3, 23.9, 3.9),
            title(4, 24.2, 4.0),
        ]);
        assert_eq!((s.kind, ticked(&s)), (DiscKind::Episodes, vec![1, 2, 3, 4]));
        assert_eq!(role(&s, 0), Role::PlayAll);
    }

    #[test]
    fn hour_long_drama_episodes_are_not_a_movie() {
        let s = disc(vec![
            title(0, 58.0, 9.0),
            title(1, 61.0, 9.5),
            title(2, 59.0, 9.2),
        ]);
        assert_eq!(s.kind, DiscKind::Episodes);
        assert_eq!(ticked(&s), [0, 1, 2]);
    }

    #[test]
    fn decoy_playlists_collapse_to_one_feature() {
        // Some discs expose the feature through dozens of playlists of the same length
        let mut titles: Vec<Title> = (0..12).map(|i| title(i, 118.0, 25.0)).collect();
        for t in &mut titles {
            t.segments = vec![10, 11, 12, 13];
        }
        let s = disc(titles);
        assert_eq!((s.kind, ticked(&s)), (DiscKind::Movie, vec![0]));
        assert!((1..12).all(|i| role(&s, i) == Role::Duplicate));
    }

    #[test]
    fn a_single_episode_or_nothing_recognisable() {
        let s = disc(vec![title(0, 24.0, 4.0), title(1, 5.0, 0.5)]);
        assert_eq!((s.kind, ticked(&s)), (DiscKind::Unknown, vec![0]));
        let s = disc(vec![title(0, 1.0, 0.1)]);
        assert_eq!((s.kind, ticked(&s)), (DiscKind::Unknown, vec![]));
    }

    #[test]
    fn languages_of_the_ticked_titles() {
        let track = |kind, language: &str| Track {
            kind,
            language: language.into(),
        };
        let mut a = title(0, 24.0, 4.0);
        a.tracks = vec![
            track(TrackKind::Video, ""),
            track(TrackKind::Audio, "jpn"),
            track(TrackKind::Audio, "eng"),
            track(TrackKind::Subtitle, "eng"),
            track(TrackKind::Subtitle, "spa"),
        ];
        let mut b = a.clone();
        b.index = 1;
        let mut extra = title(2, 3.0, 0.3);
        extra.tracks = vec![track(TrackKind::Audio, "kor")];
        let d = Disc {
            name: "D".into(),
            titles: vec![a, b, extra],
            source: String::new(),
        };
        let s = choose_titles(&d);
        assert_eq!(
            languages(&d, &s),
            [
                ("eng".to_string(), "English".to_string()),
                ("jpn".to_string(), "Japanese".to_string()),
                ("spa".to_string(), "Spanish".to_string()),
            ]
        );
    }
}
