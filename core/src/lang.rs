//! Language codes and audio track names (`Get-IsoLang`, `Get-LanguageName`,
//! `Get-AudioFormatLabel`, `Get-ChannelLabel`, `Get-AudioTrackName` in the script).

use std::sync::LazyLock;

use regex::Regex;

const IETF_TO_ISO: &[(&str, &str)] = &[
    ("en", "eng"),
    ("ja", "jpn"),
    ("fr", "fre"),
    ("de", "ger"),
    ("es", "spa"),
    ("it", "ita"),
    ("zh", "chi"),
    ("ko", "kor"),
];

const LANGUAGE_NAMES: &[(&str, &str)] = &[
    ("eng", "English"),
    ("jpn", "Japanese"),
    ("und", "Unknown"),
    ("fre", "French"),
    ("fra", "French"),
    ("ger", "German"),
    ("deu", "German"),
    ("spa", "Spanish"),
    ("ita", "Italian"),
    ("por", "Portuguese"),
    ("chi", "Chinese"),
    ("zho", "Chinese"),
    ("kor", "Korean"),
    ("rus", "Russian"),
    ("ara", "Arabic"),
    ("dut", "Dutch"),
    ("nld", "Dutch"),
    ("pol", "Polish"),
    ("tur", "Turkish"),
    ("swe", "Swedish"),
    ("nor", "Norwegian"),
    ("dan", "Danish"),
    ("fin", "Finnish"),
    ("hun", "Hungarian"),
    ("cze", "Czech"),
    ("ces", "Czech"),
    ("ron", "Romanian"),
    ("rum", "Romanian"),
    ("gre", "Greek"),
    ("ell", "Greek"),
    ("heb", "Hebrew"),
    ("hin", "Hindi"),
    ("tha", "Thai"),
    ("vie", "Vietnamese"),
    ("ind", "Indonesian"),
    ("may", "Malay"),
    ("msa", "Malay"),
    ("ukr", "Ukrainian"),
    ("fil", "Filipino"),
    ("tgl", "Tagalog"),
    ("cat", "Catalan"),
    ("hrv", "Croatian"),
    ("slo", "Slovak"),
    ("slk", "Slovak"),
];

/// Same order as the script's `switch -Regex`: the first match wins. PowerShell regexes are
/// case-insensitive.
static FORMAT_LABELS: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
    [
        ("TrueHD.*Atmos", "TrueHD Atmos"),
        ("TrueHD", "TrueHD"),
        ("DTS-HD Master", "DTS-HD MA"),
        ("DTS-HD High", "DTS-HD HRA"),
        ("DTS-ES", "DTS-ES"),
        ("DTS", "DTS"),
        ("E-AC-3|EAC3|E-AC3", "DD+"),
        ("AC-3|AC3", "DD"),
        ("AAC", "AAC"),
        ("FLAC", "FLAC"),
        ("PCM", "PCM"),
        ("Opus", "Opus"),
        ("MP3", "MP3"),
        ("MP2", "MP2"),
        ("Vorbis", "Vorbis"),
    ]
    .into_iter()
    .map(|(re, label)| {
        (
            Regex::new(&format!("(?i){re}")).expect("valid regex"),
            label,
        )
    })
    .collect()
});

static COMMENT: LazyLock<Regex> = LazyLock::new(|| Regex::new("(?i)comment").expect("valid regex"));

/// Normalises a track language to ISO 639-2 (`eng`, `jpn`, `und`, …), using the IETF tag
/// when the plain language is missing or `und`.
pub fn iso_lang(lang: Option<&str>, ietf: Option<&str>) -> String {
    let l = lang.map(|s| s.trim().to_lowercase()).unwrap_or_default();
    if !l.is_empty() && l != "und" {
        return l;
    }
    let i = ietf.map(|s| s.trim().to_lowercase()).unwrap_or_default();
    if !i.is_empty() {
        let prefix = i.split('-').next().unwrap_or("");
        if let Some((_, iso)) = IETF_TO_ISO.iter().find(|(k, _)| *k == prefix) {
            return (*iso).to_string();
        }
        if !prefix.is_empty() && prefix != "und" {
            return prefix.to_string();
        }
    }
    "und".to_string()
}

/// Full English name for a language code ("English"); unknown codes are returned unchanged.
pub fn language_name(tag: &str) -> String {
    if tag.is_empty() {
        return "Unknown".into();
    }
    let key = tag.trim().to_lowercase();
    LANGUAGE_NAMES
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, name)| (*name).to_string())
        .unwrap_or_else(|| tag.to_string())
}

/// Short format name from mkvmerge's codec description ("TrueHD", "DD+", …).
pub fn audio_format_label(codec: &str) -> String {
    FORMAT_LABELS
        .iter()
        .find(|(re, _)| re.is_match(codec))
        .map(|(_, label)| (*label).to_string())
        .unwrap_or_else(|| codec.to_string())
}

pub fn channel_label(channels: i64) -> String {
    match channels {
        1 => "1.0".into(),
        2 => "2.0".into(),
        3 => "2.1".into(),
        4 => "4.0".into(),
        5 => "5.0".into(),
        6 => "5.1".into(),
        7 => "6.1".into(),
        8 => "7.1".into(),
        n => format!("{n}ch"),
    }
}

/// "Language - Format Channels", keeping a commentary marker from the original title.
pub fn audio_track_name(
    lang: &str,
    format: &str,
    channels: i64,
    original_title: Option<&str>,
) -> String {
    let mut name = format!(
        "{} - {} {}",
        language_name(lang),
        format,
        channel_label(channels)
    );
    if original_title.is_some_and(|t| COMMENT.is_match(t)) {
        name.push_str(" (Commentary)");
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_normalisation() {
        assert_eq!(iso_lang(Some("eng"), Some("en")), "eng");
        assert_eq!(iso_lang(Some("JPN "), None), "jpn");
        assert_eq!(iso_lang(Some("und"), Some("ja-JP")), "jpn");
        assert_eq!(iso_lang(None, Some("pt-BR")), "pt");
        assert_eq!(iso_lang(Some("und"), Some("und")), "und");
        assert_eq!(iso_lang(None, None), "und");
    }

    #[test]
    fn format_labels_first_match_wins() {
        assert_eq!(audio_format_label("TrueHD Atmos"), "TrueHD Atmos");
        assert_eq!(audio_format_label("TrueHD"), "TrueHD");
        assert_eq!(audio_format_label("DTS-HD Master Audio"), "DTS-HD MA");
        assert_eq!(
            audio_format_label("DTS-HD High Resolution Audio"),
            "DTS-HD HRA"
        );
        assert_eq!(audio_format_label("DTS-ES"), "DTS-ES");
        assert_eq!(audio_format_label("DTS"), "DTS");
        assert_eq!(audio_format_label("E-AC-3"), "DD+");
        assert_eq!(audio_format_label("AC-3"), "DD");
        assert_eq!(audio_format_label("AAC"), "AAC");
        assert_eq!(audio_format_label("FLAC"), "FLAC");
        assert_eq!(audio_format_label("PCM"), "PCM");
        assert_eq!(audio_format_label("opus"), "Opus");
        assert_eq!(audio_format_label("MP3"), "MP3");
        assert_eq!(audio_format_label("MP2"), "MP2");
        assert_eq!(audio_format_label("Vorbis"), "Vorbis");
        assert_eq!(audio_format_label("ALAC"), "ALAC");
    }

    #[test]
    fn track_names() {
        assert_eq!(
            audio_track_name("jpn", "TrueHD", 6, None),
            "Japanese - TrueHD 5.1"
        );
        assert_eq!(
            audio_track_name("eng", "DD+", 6, Some("x")),
            "English - DD+ 5.1"
        );
        assert_eq!(audio_track_name("jpn", "DD", 2, None), "Japanese - DD 2.0");
        assert_eq!(
            audio_track_name("eng", "AC-3", 2, Some("Director's Commentary")),
            "English - AC-3 2.0 (Commentary)"
        );
        assert_eq!(audio_track_name("xyz", "AAC", 10, None), "xyz - AAC 10ch");
        assert_eq!(channel_label(0), "0ch");
    }
}
