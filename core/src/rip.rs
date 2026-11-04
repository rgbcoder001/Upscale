//! Ripping a disc with MakeMKV and handing each title to the queue:
//! titles rip one at a time into the local work folder, each is renamed to its Jellyfin name,
//! and queued with the profile, languages and library folder the Rip sheet chose. The raw rip
//! is deleted after a checked encode (`DeleteSource`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// What the Rip sheet starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RipRequest {
    /// MakeMKV's source for the drive (`dev:/dev/rdisk8`).
    pub source: String,
    pub titles: Vec<RipTitle>,
    /// Job options for every title: `Content`, `KeepLanguages`.
    pub options: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RipTitle {
    /// MakeMKV's title index.
    pub index: u32,
    /// The finished file in the library (`…/Season 1/Show S01E07.mkv`).
    pub target: PathBuf,
    /// Expected size, for the free-space check.
    #[serde(default)]
    pub bytes: u64,
}

/// A disc title's ripping progress, shown on its queue row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RipProgress {
    pub source: String,
    pub index: u32,
    /// The disc's name ("Sample Movie").
    pub disc: String,
    pub bytes_total: u64,
    pub bytes_done: u64,
    /// Average speed so far, MB/s.
    pub mb_per_s: f64,
    pub eta_seconds: Option<u64>,
}

impl RipProgress {
    pub fn new(source: &str, index: u32, disc: &str, bytes_total: u64) -> Self {
        Self {
            source: source.into(),
            index,
            disc: disc.into(),
            bytes_total,
            bytes_done: 0,
            mb_per_s: 0.0,
            eta_seconds: None,
        }
    }

    /// The progress at `fraction` done after `elapsed`: bytes, average speed and time left.
    pub fn at(&self, fraction: f64, elapsed: std::time::Duration) -> Self {
        let done = (self.bytes_total as f64 * fraction) as u64;
        let secs = elapsed.as_secs_f64();
        let rate = if secs > 0.0 { done as f64 / secs } else { 0.0 };
        Self {
            bytes_done: done,
            mb_per_s: rate / (1u64 << 20) as f64,
            eta_seconds: (rate > 0.0 && fraction > 0.01)
                .then(|| ((self.bytes_total - done.min(self.bytes_total)) as f64 / rate) as u64),
            ..self.clone()
        }
    }
}

/// `makemkvcon -r --progress=-same mkv <source> <title> <dir>`: one title into `dir`, with
/// progress lines on stdout.
pub fn rip_args(source: &str, index: u32, dir: &Path) -> Vec<OsString> {
    let mut a: Vec<OsString> = ["-r", "--progress=-same", "mkv", source]
        .map(OsString::from)
        .into();
    a.push(index.to_string().into());
    a.push(dir.into());
    a
}

/// The overall fraction from a `PRGV:current,total,max` line (0.0–1.0).
pub fn progress(line: &str) -> Option<f64> {
    let rest = line.strip_prefix("PRGV:")?;
    let mut f = rest.split(',').map(|v| v.trim().parse::<f64>().ok());
    let (_, total, max) = (f.next()??, f.next()??, f.next()??);
    (max > 0.0).then(|| (total / max).clamp(0.0, 1.0))
}

/// The queue options for one ripped title: the sheet's options, the title's library folder
/// (`OutputDir`), and the raw rip deleted for good after a checked encode.
pub fn job_options(base: &Map<String, Value>, target: &Path) -> Map<String, Value> {
    let mut o = base.clone();
    if let Some(dir) = target.parent() {
        o.insert(
            "OutputDir".into(),
            Value::String(dir.to_string_lossy().into_owned()),
        );
    }
    o.insert("DeleteSource".into(), Value::Bool(true));
    o.insert("SourceIsRip".into(), Value::Bool(true));
    o
}

/// Where a title rips to inside the rip folder: its own empty folder, so the file MakeMKV
/// names is easy to find.
pub fn title_dir(rips: &Path, index: u32) -> PathBuf {
    rips.join(format!("title-{index}"))
}

/// The raw rip renamed to the title's Jellyfin name, in the rip folder. The encode names its
/// output after this file, so it's what lands in the library.
pub fn named_rip(rips: &Path, target: &Path) -> PathBuf {
    rips.join(target.file_name().unwrap_or_default())
}

/// The `.mkv` MakeMKV wrote into a title folder.
pub fn find_rip(dir: &Path) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| p.extension().is_some_and(|e| e.eq_ignore_ascii_case("mkv")))
}

/// `dev:/dev/rdisk8` → `/dev/disk8` for `diskutil eject`; `None` where that doesn't apply.
pub fn eject_device(source: &str) -> Option<String> {
    let dev = source.strip_prefix("dev:")?;
    dev.starts_with("/dev/")
        .then(|| dev.replacen("/dev/rdisk", "/dev/disk", 1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn makemkv_arguments() {
        let args: Vec<String> = rip_args("dev:/dev/rdisk8", 2, Path::new("/w/rips/title-2"))
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "-r",
                "--progress=-same",
                "mkv",
                "dev:/dev/rdisk8",
                "2",
                "/w/rips/title-2"
            ]
        );
    }

    #[test]
    fn progress_lines() {
        assert_eq!(progress("PRGV:120,32768,65536"), Some(0.5));
        assert_eq!(progress("PRGV:0,0,65536"), Some(0.0));
        assert_eq!(progress("PRGV:0,0,0"), None);
        assert_eq!(progress("PRGT:5018,0,\"Saving to MKV file\""), None);
    }

    #[test]
    fn each_title_becomes_a_queue_job() {
        let base = json!({ "Content": "anime-av1", "KeepLanguages": ["eng", "jpn", "und"] });
        let o = job_options(
            base.as_object().unwrap(),
            Path::new("/L/Show/Season 1/Show S01E07.mkv"),
        );
        assert_eq!(
            Value::Object(o),
            json!({
                "Content": "anime-av1",
                "KeepLanguages": ["eng", "jpn", "und"],
                "OutputDir": "/L/Show/Season 1",
                "DeleteSource": true,
                "SourceIsRip": true
            })
        );
        let rips = Path::new("/w/rips");
        assert_eq!(title_dir(rips, 3), Path::new("/w/rips/title-3"));
        assert_eq!(
            named_rip(rips, Path::new("/L/Show/Season 1/Show S01E07.mkv")),
            Path::new("/w/rips/Show S01E07.mkv")
        );
    }

    #[test]
    fn progress_on_the_row() {
        let p = RipProgress::new("dev:/dev/rdisk8", 2, "Sample Movie", 30 << 30);
        let half = p.at(0.5, std::time::Duration::from_secs(768));
        assert_eq!(half.bytes_done, 15 << 30);
        assert!((half.mb_per_s - 20.0).abs() < 0.01);
        assert_eq!(half.eta_seconds, Some(768));
        assert_eq!(
            p.at(0.0, std::time::Duration::from_secs(0)).eta_seconds,
            None
        );
    }

    #[test]
    fn finding_and_ejecting() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(find_rip(dir.path()), None);
        std::fs::write(dir.path().join("Sample Movie_t02.mkv"), b"").unwrap();
        assert_eq!(
            find_rip(dir.path()),
            Some(dir.path().join("Sample Movie_t02.mkv"))
        );
        assert_eq!(
            eject_device("dev:/dev/rdisk8").as_deref(),
            Some("/dev/disk8")
        );
        assert_eq!(eject_device("dev:D:"), None);
    }
}
