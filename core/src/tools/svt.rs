//! The SVT-AV1 encoder: the newest of SvtAv1EncApp, the BtbN FFmpeg build and the system
//! FFmpeg (`Get-SvtAppVersion`, `Get-FfmpegSvtVersion`, `Initialize-BtbnFfmpeg`,
//! `Get-LatestSvtVersion`, the encoder choice in `Initialize-Tools`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::Value;

use super::install::{Asset, Installer, release_assets};
use super::{Version, find_file};
use crate::process::Runner;

pub const BTBN_LATEST: &str =
    "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/tags/latest";
pub const SVT_RELEASES: &str =
    "https://gitlab.com/api/v4/projects/AOMediaCodec%2FSVT-AV1/releases?per_page=20";

static APP_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"v?(\d+\.\d+\.\d+)").expect("valid regex"));
static LIB_VERSION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"Lib v?(\d+\.\d+\.\d+)").expect("valid regex"));
static RELEASE_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^v(\d+\.\d+\.\d+)$").expect("valid regex"));
static BTBN_RELEASE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^ffmpeg-n([\d.]+)-latest-win64-gpl-[\d.]+\.zip$").expect("valid regex")
});

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EncoderKind {
    /// `SvtAv1EncApp` reading y4m from stdin.
    SvtApp,
    /// `ffmpeg -c:v libsvtav1`.
    Ffmpeg,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Encoder {
    pub kind: EncoderKind,
    pub path: PathBuf,
    pub version: Version,
    pub label: String,
    /// Tie-break: SvtAv1EncApp 0, BtbN 1, other FFmpeg 2.
    pub rank: u8,
}

pub fn parse_app_version(text: &str) -> Option<Version> {
    APP_VERSION
        .captures(text)
        .and_then(|c| Version::parse(&c[1]))
}

/// The libsvtav1 version from an encode's log; 0.0.0 when it encoded but printed none.
pub fn parse_ffmpeg_version(text: &str, encoded: bool) -> Option<Version> {
    if !encoded {
        return None;
    }
    Some(
        LIB_VERSION
            .captures(text)
            .and_then(|c| Version::parse(&c[1]))
            .unwrap_or_default(),
    )
}

pub fn ffmpeg_probe_args() -> Vec<OsString> {
    [
        "-hide_banner",
        "-f",
        "lavfi",
        "-i",
        "color=c=black:s=320x240:d=0.1",
        "-frames:v",
        "1",
        "-c:v",
        "libsvtav1",
        "-f",
        "null",
        "-",
    ]
    .map(OsString::from)
    .into()
}

pub fn svt_app_version(runner: &dyn Runner, app: &Path) -> Option<Version> {
    let out = runner.output(app, &[OsString::from("--version")]).ok()?;
    parse_app_version(&format!("{}{}", out.stdout, out.stderr))
}

pub fn ffmpeg_svt_version(runner: &dyn Runner, ffmpeg: &Path) -> Option<Version> {
    let out = runner.output(ffmpeg, &ffmpeg_probe_args()).ok()?;
    parse_ffmpeg_version(&format!("{}{}", out.stdout, out.stderr), out.success())
}

/// Newest version wins; on a tie prefer SvtAv1EncApp, then the BtbN build.
pub fn pick_encoder(mut encoders: Vec<Encoder>) -> Option<Encoder> {
    encoders.sort_by(|a, b| b.version.cmp(&a.version).then(a.rank.cmp(&b.rank)));
    encoders.into_iter().next()
}

/// The newest FFmpeg *release* branch build (all variants carry the same SVT-AV1), else master.
pub fn pick_btbn_asset(release: &Value) -> Option<Asset> {
    let assets = release_assets(release);
    let release_version = |a: &Asset| {
        BTBN_RELEASE.captures(&a.name).map(|c| {
            let mut parts: Vec<u32> = c[1].split('.').filter_map(|p| p.parse().ok()).collect();
            parts.resize(4, 0);
            parts
        })
    };
    assets
        .iter()
        .filter_map(|a| release_version(a).map(|v| (v, a)))
        .max_by(|x, y| x.0.cmp(&y.0))
        .map(|(_, a)| a.clone())
        .or_else(|| {
            assets
                .iter()
                .find(|a| a.name == "ffmpeg-master-latest-win64-gpl.zip")
                .cloned()
        })
}

/// The newest `vX.Y.Z` among GitLab releases.
pub fn latest_release(releases: &Value) -> Option<Version> {
    releases
        .as_array()?
        .iter()
        .filter_map(|r| {
            RELEASE_TAG
                .captures(r.get("tag_name")?.as_str()?)
                .and_then(|c| Version::parse(&c[1]))
        })
        .max()
}

/// The BtbN FFmpeg in `<tools>/ffmpeg`, downloaded or refreshed when updating. A failed
/// update is a warning: the existing build (or none) is used. BtbN only builds FFmpeg for
/// Windows and Linux, so nothing is downloaded on macOS.
pub fn init_btbn(inst: &Installer) -> Option<PathBuf> {
    let root = inst.tools_dir.join("ffmpeg");
    let existing = find_file(&root, &super::exe("ffmpeg"));
    if !inst.update || !cfg!(windows) {
        return existing;
    }
    let result = (|| -> crate::Result<PathBuf> {
        let rel = inst.http_json(BTBN_LATEST)?;
        let asset = pick_btbn_asset(&rel)
            .ok_or_else(|| crate::Error::Tool("no Windows build in the latest release".into()))?;
        let stamp = root.join("SOURCE.txt");
        let want = format!("{} {}", asset.name, asset.updated_at);
        if let Some(exe) = &existing
            && std::fs::read_to_string(&stamp).is_ok_and(|s| s.trim() == want)
        {
            inst.log
                .info(&format!("  FFmpeg (BtbN) {} is current", asset.name));
            return Ok(exe.clone());
        }
        inst.log.info(&format!(
            "  Downloading FFmpeg (BtbN) {} ({} MB) ...",
            asset.name,
            crate::util::fmt_n(asset.size as f64 / (1u64 << 20) as f64, 0)
        ));
        let zip = inst.fetch(&asset.download()?)?;
        let staging = inst.tools_dir.join("ffmpeg.new");
        let _ = std::fs::remove_dir_all(&staging);
        inst.extract_zip(&zip, &staging)?;
        inst.replace_dir(&staging, &root)?;
        std::fs::write(&stamp, format!("{want}\n"))?;
        let _ = std::fs::remove_file(&zip);
        let exe = find_file(&root, &super::exe("ffmpeg"))
            .ok_or_else(|| crate::Error::Tool("ffmpeg.exe not found in the download".into()))?;
        inst.log
            .info(&format!("  FFmpeg (BtbN) installed: {}", asset.name));
        Ok(exe)
    })();
    match result {
        Ok(exe) => Some(exe),
        Err(e) => {
            inst.log.warn(&format!(
                "Couldn't update the BtbN FFmpeg build ({e}); using the FFmpeg already installed."
            ));
            existing
        }
    }
}

/// Finds every SVT-AV1 encoder and picks one.
pub fn choose_encoder(
    runner: &dyn Runner,
    svt_app: Option<&Path>,
    btbn: Option<&Path>,
    ffmpeg: &Path,
) -> Option<Encoder> {
    let mut encoders = Vec::new();
    if let Some(app) = svt_app
        && let Some(version) = svt_app_version(runner, app)
    {
        encoders.push(Encoder {
            kind: EncoderKind::SvtApp,
            path: app.into(),
            version,
            label: "SvtAv1EncApp".into(),
            rank: 0,
        });
    }
    let mut ffmpegs: Vec<&Path> = btbn.into_iter().collect();
    if !ffmpegs.contains(&ffmpeg) {
        ffmpegs.push(ffmpeg);
    }
    for ff in ffmpegs {
        if let Some(version) = ffmpeg_svt_version(runner, ff) {
            let is_btbn = Some(ff) == btbn;
            encoders.push(Encoder {
                kind: EncoderKind::Ffmpeg,
                path: ff.into(),
                version,
                label: if is_btbn {
                    "FFmpeg (BtbN build) libsvtav1"
                } else {
                    "FFmpeg libsvtav1"
                }
                .into(),
                rank: if is_btbn { 1 } else { 2 },
            });
        }
    }
    pick_encoder(encoders)
}

/// The log line for the chosen encoder.
pub fn encoder_log_line(enc: &Encoder, latest: Option<Version>) -> String {
    match latest {
        // Not an error: encoding works fine; it just isn't the newest encoder release
        Some(l) if enc.version < l => format!(
            "  SVT-AV1 v{} via {} (newest release is v{l}; fine to use; Update tools fetches a newer build when one is available)",
            enc.version, enc.label
        ),
        Some(_) => format!("  SVT-AV1 v{} via {} (latest)", enc.version, enc.label),
        None => format!(
            "  SVT-AV1 v{} via {} (could not check for updates)",
            enc.version, enc.label
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn enc(kind: EncoderKind, v: Version, rank: u8) -> Encoder {
        Encoder {
            kind,
            path: PathBuf::new(),
            version: v,
            label: String::new(),
            rank,
        }
    }

    #[test]
    fn version_banners() {
        assert_eq!(
            parse_app_version("SVT-AV1-PSY v3.0.2-A (release)"),
            Some(Version(3, 0, 2))
        );
        assert_eq!(parse_app_version("SVT-AV1 4.2.0"), Some(Version(4, 2, 0)));
        let log = "Svt[info]: -------------------------------------------\nSvt[info]: SVT [version]:\tSVT-AV1 Encoder Lib v4.1.0\n";
        assert_eq!(parse_ffmpeg_version(log, true), Some(Version(4, 1, 0)));
        assert_eq!(
            parse_ffmpeg_version("no banner", true),
            Some(Version(0, 0, 0))
        );
        assert_eq!(parse_ffmpeg_version(log, false), None, "libsvtav1 missing");
    }

    #[test]
    fn newest_wins_then_rank() {
        let best = pick_encoder(vec![
            enc(EncoderKind::Ffmpeg, Version(4, 1, 0), 2),
            enc(EncoderKind::Ffmpeg, Version(4, 2, 0), 1),
            enc(EncoderKind::SvtApp, Version(4, 2, 0), 0),
        ])
        .unwrap();
        assert_eq!((best.kind, best.rank), (EncoderKind::SvtApp, 0));
        let best = pick_encoder(vec![
            enc(EncoderKind::SvtApp, Version(4, 0, 0), 0),
            enc(EncoderKind::Ffmpeg, Version(4, 2, 0), 2),
        ])
        .unwrap();
        assert_eq!(best.rank, 2);
        assert!(pick_encoder(vec![]).is_none());
    }

    #[test]
    fn btbn_prefers_the_newest_release_branch() {
        let rel = json!({ "assets": [
            { "name": "ffmpeg-master-latest-win64-gpl.zip" },
            { "name": "ffmpeg-n8.0-latest-win64-gpl-8.0.zip" },
            { "name": "ffmpeg-n9.0-latest-win64-gpl-9.0.zip" },
            { "name": "ffmpeg-n9.0-latest-win64-lgpl-9.0.zip" },
            { "name": "ffmpeg-n10.0-latest-linux64-gpl-10.0.tar.xz" }] });
        assert_eq!(
            pick_btbn_asset(&rel).unwrap().name,
            "ffmpeg-n9.0-latest-win64-gpl-9.0.zip"
        );
        let rel = json!({ "assets": [{ "name": "ffmpeg-master-latest-win64-gpl.zip" }] });
        assert_eq!(
            pick_btbn_asset(&rel).unwrap().name,
            "ffmpeg-master-latest-win64-gpl.zip"
        );
    }

    #[test]
    fn latest_svt_release() {
        let rels = json!([{ "tag_name": "v4.2.0" }, { "tag_name": "v4.10.0-rc1" }, { "tag_name": "v4.1.0" }]);
        assert_eq!(latest_release(&rels), Some(Version(4, 2, 0)));
    }

    #[test]
    fn log_lines() {
        let e = Encoder {
            label: "FFmpeg libsvtav1".into(),
            ..enc(EncoderKind::Ffmpeg, Version(4, 1, 0), 2)
        };
        assert_eq!(
            encoder_log_line(&e, Some(Version(4, 2, 0))),
            "  SVT-AV1 v4.1.0 via FFmpeg libsvtav1 (newest release is v4.2.0; fine to use; Update tools fetches a newer build when one is available)"
        );
        assert_eq!(
            encoder_log_line(&e, Some(Version(4, 1, 0))),
            "  SVT-AV1 v4.1.0 via FFmpeg libsvtav1 (latest)"
        );
    }
}
