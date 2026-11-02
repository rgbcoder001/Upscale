//! Which audio and subtitle tracks to keep, which EAC3 companions to make, and which tracks
//! are default (`Get-TrackPlan`, `Get-DefaultTracks`).

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::lang::{audio_format_label, audio_track_name, iso_lang};
use crate::probe::{AudioStream, MkvTrack};

static SIGNS_SONGS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)sign|song").expect("valid regex"));
/// Subtitles that transcribe the dub: kept, never default.
static DUBTITLES: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)dubtitle").expect("valid regex"));
static COMMENTARY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new("(?i)commentary").expect("valid regex"));

/// `DefaultAudioLanguage` / `DefaultSubtitleLanguage` value that decides from the tracks.
pub const AUTO: &str = "auto";
pub const FULL_SUBS_NAME: &str = "English Subtitles";
pub const SIGNS_SUBS_NAME: &str = "Signs and Songs";

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Track {
    pub id: i64,
    pub codec: String,
    /// ISO 639-2 language.
    pub iso: String,
    /// The source's track name.
    pub track_name: Option<String>,
    pub forced: bool,
    /// Audio only: "Language - Format Channels".
    pub new_name: Option<String>,
    /// Audio only.
    pub channels: i64,
    /// Size from the source's statistics tags, if it has them.
    pub bytes: Option<u64>,
}

impl Track {
    fn name(&self) -> &str {
        self.track_name.as_deref().unwrap_or("")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Eac3 {
    pub stream_index: i64,
    /// e.g. `TRUEHD` or `DTS (DTS-HD MA)`.
    pub codec: String,
    pub channels: i64,
    pub out_channels: i64,
    pub bitrate: &'static str,
    pub lang_code: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TrackPlan {
    pub audio_tracks: Vec<Track>,
    pub sub_tracks: Vec<Track>,
    /// Ids of the kept tracks, in source order.
    pub keep_audio: Vec<i64>,
    pub keep_subs: Vec<i64>,
    /// No audio in a kept language: everything is kept rather than producing a silent file.
    pub audio_fallback: bool,
    pub eac3: Vec<Eac3>,
}

impl TrackPlan {
    pub fn kept_audio(&self) -> impl Iterator<Item = &Track> {
        self.audio_tracks
            .iter()
            .filter(|t| self.keep_audio.contains(&t.id))
    }
    pub fn dropped_audio(&self) -> impl Iterator<Item = &Track> {
        self.audio_tracks
            .iter()
            .filter(|t| !self.keep_audio.contains(&t.id))
    }
    pub fn kept_subs(&self) -> impl Iterator<Item = &Track> {
        self.sub_tracks
            .iter()
            .filter(|t| self.keep_subs.contains(&t.id))
    }
    pub fn dropped_subs(&self) -> impl Iterator<Item = &Track> {
        self.sub_tracks
            .iter()
            .filter(|t| !self.keep_subs.contains(&t.id))
    }
}

fn keeps(keep_languages: &[String], iso: &str) -> bool {
    keep_languages.iter().any(|l| l.eq_ignore_ascii_case(iso))
}

/// Codecs that get an EAC3 companion track.
fn wants_eac3(codec: &str) -> bool {
    codec == "truehd" || codec == "flac" || codec.starts_with("pcm") || codec == "dts"
}

pub fn plan_tracks(
    tracks: &[MkvTrack],
    audio_streams: &[AudioStream],
    keep_languages: &[String],
) -> TrackPlan {
    let to_track = |t: &MkvTrack| {
        let iso = iso_lang(t.language.as_deref(), t.language_ietf.as_deref());
        let new_name = (t.kind == "audio").then(|| {
            audio_track_name(
                &iso,
                &audio_format_label(&t.codec),
                t.audio_channels,
                t.track_name.as_deref(),
            )
        });
        Track {
            id: t.id,
            codec: t.codec.clone(),
            iso,
            track_name: t.track_name.clone(),
            forced: t.forced,
            new_name,
            channels: t.audio_channels,
            bytes: t.bytes,
        }
    };
    let audio: Vec<Track> = tracks
        .iter()
        .filter(|t| t.kind == "audio")
        .map(to_track)
        .collect();
    let subs: Vec<Track> = tracks
        .iter()
        .filter(|t| t.kind == "subtitles")
        .map(to_track)
        .collect();

    let mut keep_audio: Vec<i64> = audio
        .iter()
        .filter(|t| keeps(keep_languages, &t.iso))
        .map(|t| t.id)
        .collect();
    let keep_subs: Vec<i64> = subs
        .iter()
        .filter(|t| keeps(keep_languages, &t.iso))
        .map(|t| t.id)
        .collect();
    let audio_fallback = !audio.is_empty() && keep_audio.is_empty();
    if audio_fallback {
        keep_audio = audio.iter().map(|t| t.id).collect();
    }

    let eac3 = audio_streams
        .iter()
        .filter_map(|s| {
            let codec = s.codec_name.to_lowercase();
            if !wants_eac3(&codec) {
                return None;
            }
            let lang = iso_lang(s.language.as_deref(), None);
            if !audio_fallback && !keeps(keep_languages, &lang) {
                return None;
            }
            // FFmpeg's EAC3 encoder tops out at 5.1
            let out_channels = s.channels.min(6);
            let mut label = codec.to_uppercase();
            if let Some(p) = s.profile.as_deref().filter(|p| !p.is_empty()) {
                label.push_str(&format!(" ({p})"));
            }
            Some(Eac3 {
                stream_index: s.index,
                codec: label,
                channels: s.channels,
                out_channels,
                bitrate: if out_channels >= 6 { "960k" } else { "320k" },
                name: audio_track_name(&lang, "DD+", out_channels, s.title.as_deref()),
                lang_code: lang,
            })
        })
        .collect();

    TrackPlan {
        audio_tracks: audio,
        sub_tracks: subs,
        keep_audio,
        keep_subs,
        audio_fallback,
        eac3,
    }
}

/// Default tracks. `eac3_langs` are the companions that exist (planned, or actually encoded).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Defaults {
    /// Index into `eac3_langs` of the default companion.
    pub eac3: Option<usize>,
    /// The default source audio track, when no companion is default.
    pub audio_id: Option<i64>,
    pub sub_id: Option<i64>,
    /// The default audio language (Auto resolved).
    pub audio_lang: String,
    /// No subtitle track on by default (forced tracks keep the source's flags).
    pub subs_off: bool,
    /// Why Auto picked the audio language, e.g. `eng 5.1 beats jpn 2.0`.
    pub audio_reason: Option<String>,
    /// Why Auto picked the subtitles, e.g. `full subtitles for jpn audio`.
    pub sub_reason: Option<String>,
    /// Subtitle tracks Auto renames: (track id, new name).
    pub sub_names: Vec<(i64, String)>,
}

fn channel_label(ch: i64) -> String {
    match ch {
        0 => "?".into(),
        1 => "1.0".into(),
        2 => "2.0".into(),
        n => format!("{}.1", n - 1),
    }
}

/// Auto audio: English when its best track has more channels than the best Japanese one,
/// otherwise Japanese. Commentary tracks don't count.
fn auto_audio(plan: &TrackPlan) -> (String, String) {
    let best = |lang: &str| {
        plan.kept_audio()
            .filter(|t| t.iso.eq_ignore_ascii_case(lang) && !COMMENTARY.is_match(t.name()))
            .map(|t| t.channels)
            .max()
    };
    match (best("eng"), best("jpn")) {
        (Some(e), Some(j)) if e > j => (
            "eng".into(),
            format!("eng {} beats jpn {}", channel_label(e), channel_label(j)),
        ),
        (Some(e), Some(j)) => (
            "jpn".into(),
            format!("jpn {} vs eng {}", channel_label(j), channel_label(e)),
        ),
        (Some(_), None) => ("eng".into(), "only eng audio".into()),
        (None, Some(_)) => ("jpn".into(), "only jpn audio".into()),
        (None, None) => ("jpn".into(), "no eng or jpn audio".into()),
    }
}

/// English subtitles split into the full track (for Japanese audio) and Signs and Songs (for
/// English audio). With two or more tracks, the smaller is signs when it holds under half the
/// data of the larger; otherwise (and without sizes) the names decide.
struct EnglishSubs<'a> {
    full: Option<&'a Track>,
    signs: Option<&'a Track>,
    /// Exactly these two tracks, told apart: safe to rename.
    rename: bool,
}

fn by_name<'a>(c: &[&'a Track]) -> EnglishSubs<'a> {
    EnglishSubs {
        full: c.iter().find(|t| !SIGNS_SONGS.is_match(t.name())).copied(),
        signs: c.iter().find(|t| SIGNS_SONGS.is_match(t.name())).copied(),
        rename: false,
    }
}

fn english_subs(plan: &TrackPlan) -> EnglishSubs<'_> {
    let mut c: Vec<&Track> = plan
        .kept_subs()
        .filter(|t| t.iso == "eng" && !t.forced && !DUBTITLES.is_match(t.name()))
        .collect();
    if c.len() < 2 || c.iter().any(|t| t.bytes.is_none()) {
        return by_name(&c);
    }
    c.sort_by_key(|t| std::cmp::Reverse(t.bytes));
    let (large, small) = (c[0], c[c.len() - 1]);
    if small.bytes.unwrap_or(0).saturating_mul(2) >= large.bytes.unwrap_or(0) {
        return by_name(&c);
    }
    EnglishSubs {
        full: Some(large),
        signs: Some(small),
        rename: c.len() == 2,
    }
}

pub fn default_tracks(
    plan: &TrackPlan,
    eac3_langs: &[&str],
    default_audio: &str,
    default_subs: &str,
) -> Defaults {
    let (audio_lang, audio_reason) = if default_audio.eq_ignore_ascii_case(AUTO) {
        let (l, r) = auto_audio(plan);
        (l, Some(r))
    } else {
        (default_audio.to_string(), None)
    };
    let eac3 = eac3_langs
        .iter()
        .position(|l| l.eq_ignore_ascii_case(&audio_lang));
    let audio_id = if eac3.is_some() {
        None
    } else {
        plan.kept_audio()
            .find(|t| t.iso.eq_ignore_ascii_case(&audio_lang))
            .map(|t| t.id)
    };
    let mut d = Defaults {
        eac3,
        audio_id,
        audio_reason,
        audio_lang: audio_lang.clone(),
        ..Defaults::default()
    };

    if default_subs.eq_ignore_ascii_case("none") {
        d.subs_off = true;
    } else if default_subs.eq_ignore_ascii_case(AUTO) {
        let subs = english_subs(plan);
        if subs.rename {
            for (t, name) in [(subs.full, FULL_SUBS_NAME), (subs.signs, SIGNS_SUBS_NAME)] {
                if let Some(t) = t.filter(|t| t.name() != name) {
                    d.sub_names.push((t.id, name.to_string()));
                }
            }
        }
        let (pick, what) = if audio_lang.eq_ignore_ascii_case("eng") {
            (subs.signs, "signs and songs")
        } else {
            (subs.full, "full subtitles")
        };
        d.sub_id = pick.map(|t| t.id);
        d.subs_off = pick.is_none();
        d.sub_reason = Some(match pick {
            Some(_) => format!("{what} for {audio_lang} audio"),
            None => format!("no {what} for {audio_lang} audio"),
        });
    } else {
        let in_lang: Vec<&Track> = plan
            .kept_subs()
            .filter(|t| t.iso.eq_ignore_ascii_case(default_subs) && !DUBTITLES.is_match(t.name()))
            .collect();
        d.sub_id = in_lang
            .iter()
            .find(|t| !t.forced && !SIGNS_SONGS.is_match(t.name()))
            .or(in_lang.first())
            .map(|t| t.id);
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{parse_audio_streams, parse_mkvmerge};
    use std::path::Path;

    fn langs(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    fn fixture(rel: &str) -> String {
        std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(rel),
        )
        .unwrap()
    }

    fn mkv(
        id: i64,
        kind: &str,
        codec: &str,
        lang: &str,
        ch: i64,
        name: Option<&str>,
        forced: bool,
    ) -> MkvTrack {
        MkvTrack {
            id,
            kind: kind.into(),
            codec: codec.into(),
            language: Some(lang.into()),
            language_ietf: None,
            audio_channels: ch,
            track_name: name.map(Into::into),
            forced,
            bytes: None,
        }
    }

    fn stream(
        index: i64,
        codec: &str,
        profile: Option<&str>,
        ch: i64,
        lang: &str,
        title: Option<&str>,
    ) -> AudioStream {
        AudioStream {
            index,
            codec_name: codec.into(),
            profile: profile.map(Into::into),
            channels: ch,
            language: Some(lang.into()),
            title: title.map(Into::into),
        }
    }

    fn sub(id: i64, name: Option<&str>, bytes: u64) -> MkvTrack {
        MkvTrack {
            bytes: Some(bytes),
            ..mkv(id, "subtitles", "HDMV PGS", "eng", 0, name, false)
        }
    }

    fn auto(tracks: &[MkvTrack]) -> Defaults {
        let p = plan_tracks(tracks, &[], &langs(&["eng", "jpn", "und"]));
        default_tracks(&p, &[], AUTO, AUTO)
    }

    #[test]
    fn auto_prefers_english_with_more_channels_and_signs_for_it() {
        // Black Clover: English TrueHD 5.1, Japanese TrueHD 2.0
        let tracks = [
            mkv(1, "audio", "TrueHD", "eng", 6, None, false),
            mkv(2, "audio", "TrueHD", "jpn", 2, None, false),
            sub(3, Some("Signs and Songs"), 400_000),
            sub(4, Some("English Subtitles"), 7_000_000),
        ];
        let d = auto(&tracks);
        assert_eq!((d.audio_id, d.sub_id), (Some(1), Some(3)));
        assert_eq!(d.audio_reason.as_deref(), Some("eng 5.1 beats jpn 2.0"));
        assert!(d.sub_names.is_empty(), "already named");

        // Same channels: Japanese, with the full subtitles
        let mut tracks = tracks;
        tracks[0].audio_channels = 2;
        let d = auto(&tracks);
        assert_eq!((d.audio_id, d.sub_id), (Some(2), Some(4)));
    }

    #[test]
    fn auto_tells_subtitles_apart_by_size_and_renames_them() {
        let tracks = [
            mkv(1, "audio", "FLAC", "jpn", 2, None, false),
            mkv(2, "audio", "FLAC", "eng", 2, None, false),
            sub(3, Some("Dialogue"), 9_000_000),
            sub(4, None, 300_000),
        ];
        let d = auto(&tracks);
        assert_eq!(d.sub_id, Some(3));
        assert_eq!(
            d.sub_names,
            [
                (3, FULL_SUBS_NAME.to_string()),
                (4, SIGNS_SUBS_NAME.to_string())
            ]
        );

        // Two full tracks of about the same size: no renames, names decide
        let tracks = [
            mkv(1, "audio", "FLAC", "eng", 6, None, false),
            mkv(2, "audio", "FLAC", "jpn", 2, None, false),
            sub(3, Some("Honorifics"), 9_000_000),
            sub(4, Some("No honorifics"), 8_000_000),
        ];
        let d = auto(&tracks);
        assert!(d.sub_names.is_empty());
        assert_eq!(
            (d.sub_id, d.subs_off),
            (None, true),
            "no signs track for eng"
        );
    }

    #[test]
    fn dubtitles_are_kept_but_never_default() {
        let tracks = [
            mkv(1, "audio", "FLAC", "jpn", 2, None, false),
            mkv(2, "audio", "FLAC", "eng", 2, None, false),
            sub(3, Some("Dubtitles"), 12_000_000),
            sub(4, Some("Signs & Songs"), 300_000),
            sub(5, Some("Full Subtitles"), 9_000_000),
        ];
        let p = plan_tracks(&tracks, &[], &langs(&["eng", "jpn"]));
        assert_eq!(p.keep_subs, [3, 4, 5]);
        let d = default_tracks(&p, &[], AUTO, AUTO);
        assert_eq!(d.sub_id, Some(5));
        assert_eq!(
            d.sub_names,
            [
                (5, FULL_SUBS_NAME.to_string()),
                (4, SIGNS_SUBS_NAME.to_string())
            ],
            "the dubtitles track is left out of the comparison"
        );
        assert_eq!(default_tracks(&p, &[], "jpn", "eng").sub_id, Some(5));

        let only_dub = [
            mkv(1, "audio", "FLAC", "jpn", 2, None, false),
            sub(2, Some("English (Dubtitle)"), 12_000_000),
        ];
        let p = plan_tracks(&only_dub, &[], &langs(&["eng", "jpn"]));
        assert_eq!(default_tracks(&p, &[], "jpn", "eng").sub_id, None);
        assert_eq!(default_tracks(&p, &[], AUTO, AUTO).sub_id, None);
    }

    #[test]
    fn akito_plan() {
        let tracks = parse_mkvmerge(&fixture("akito1/mkvmerge.json")).unwrap();
        let audio = parse_audio_streams(&fixture("akito1/ffprobe-audio.json")).unwrap();
        let p = plan_tracks(&tracks, &audio, &langs(&["eng", "jpn", "und"]));
        assert_eq!(p.keep_audio, [1, 2]);
        assert_eq!(p.keep_subs, [3, 4]);
        let names: Vec<_> = p
            .kept_audio()
            .map(|t| t.new_name.clone().unwrap())
            .collect();
        assert_eq!(names, ["English - TrueHD 5.1", "Japanese - TrueHD 5.1"]);
        let e: Vec<_> = p
            .eac3
            .iter()
            .map(|e| (e.stream_index, e.codec.as_str(), e.bitrate, e.name.as_str()))
            .collect();
        assert_eq!(
            e,
            [
                (1, "TRUEHD", "960k", "English - DD+ 5.1"),
                (2, "TRUEHD", "960k", "Japanese - DD+ 5.1")
            ]
        );

        let langs_made: Vec<&str> = p.eac3.iter().map(|e| e.lang_code.as_str()).collect();
        let d = default_tracks(&p, &langs_made, "jpn", "eng");
        assert_eq!(
            (d.eac3, d.audio_id, d.sub_id),
            (Some(1), None, Some(4)),
            "skips forced Signs and Songs"
        );
        let d = default_tracks(&p, &langs_made, "eng", "none");
        assert_eq!((d.eac3, d.audio_id, d.sub_id), (Some(0), None, None));
    }

    #[test]
    fn drops_other_languages_and_names_commentary() {
        let tracks = [
            mkv(1, "audio", "AC-3", "jpn", 2, None, false),
            mkv(2, "audio", "AC-3", "fre", 6, None, false),
            mkv(
                3,
                "audio",
                "AC-3",
                "eng",
                2,
                Some("Commentary with the director"),
                false,
            ),
            mkv(
                4,
                "subtitles",
                "SubStationAlpha",
                "eng",
                0,
                Some("Signs/Songs"),
                false,
            ),
            mkv(
                5,
                "subtitles",
                "SubStationAlpha",
                "eng",
                0,
                Some("Full"),
                false,
            ),
            mkv(6, "subtitles", "SubStationAlpha", "spa", 0, None, false),
        ];
        let p = plan_tracks(&tracks, &[], &langs(&["eng", "jpn", "und"]));
        assert_eq!(p.keep_audio, [1, 3]);
        assert_eq!(p.dropped_audio().map(|t| t.id).collect::<Vec<_>>(), [2]);
        assert_eq!(p.keep_subs, [4, 5]);
        assert_eq!(
            p.audio_tracks[2].new_name.as_deref(),
            Some("English - DD 2.0 (Commentary)")
        );
        assert!(p.eac3.is_empty());
        let d = default_tracks(&p, &[], "jpn", "eng");
        assert_eq!((d.eac3, d.audio_id, d.sub_id), (None, Some(1), Some(5)));
    }

    #[test]
    fn falls_back_to_first_subtitle_when_all_are_signs_or_forced() {
        let tracks = [
            mkv(1, "audio", "FLAC", "jpn", 2, None, false),
            mkv(2, "subtitles", "PGS", "eng", 0, Some("Songs"), false),
            mkv(3, "subtitles", "PGS", "eng", 0, None, true),
        ];
        let p = plan_tracks(&tracks, &[], &langs(&["eng", "jpn"]));
        assert_eq!(default_tracks(&p, &[], "jpn", "eng").sub_id, Some(2));
        let d = default_tracks(&p, &[], "kor", "kor");
        assert_eq!((d.eac3, d.audio_id, d.sub_id), (None, None, None));
    }

    #[test]
    fn keeps_all_audio_when_no_language_matches() {
        let tracks = [mkv(
            1,
            "audio",
            "DTS-HD Master Audio",
            "fre",
            8,
            None,
            false,
        )];
        let audio = [stream(1, "dts", Some("DTS-HD MA"), 8, "fre", None)];
        let p = plan_tracks(&tracks, &audio, &langs(&["eng", "jpn"]));
        assert!(p.audio_fallback);
        assert_eq!(p.keep_audio, [1]);
        assert_eq!(
            p.eac3.len(),
            1,
            "fallback also makes companions for every language"
        );
        let e = &p.eac3[0];
        assert_eq!(
            (e.codec.as_str(), e.out_channels, e.bitrate, e.name.as_str()),
            ("DTS (DTS-HD MA)", 6, "960k", "French - DD+ 5.1")
        );
    }

    #[test]
    fn eac3_selection_and_bitrate() {
        let audio = [
            stream(1, "pcm_s24le", None, 2, "jpn", None),
            stream(2, "ac3", None, 6, "jpn", None),
            stream(3, "flac", None, 6, "eng", Some("Commentary")),
            stream(4, "truehd", None, 8, "ger", None),
        ];
        let p = plan_tracks(
            &[mkv(1, "audio", "PCM", "jpn", 2, None, false)],
            &audio,
            &langs(&["eng", "jpn"]),
        );
        let e: Vec<_> = p
            .eac3
            .iter()
            .map(|e| (e.stream_index, e.bitrate, e.name.as_str()))
            .collect();
        assert_eq!(
            e,
            [
                (1, "320k", "Japanese - DD+ 2.0"),
                (3, "960k", "English - DD+ 5.1 (Commentary)")
            ]
        );
    }
}
