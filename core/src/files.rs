//! Finding input videos and the facts the cache key needs.

use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use serde::Serialize;

use crate::Result;

pub const VIDEO_EXTENSIONS: &[&str] = &["mkv", "mp4", "m2ts", "m4v", "avi", "webm"];

/// .NET ticks (100 ns since 0001-01-01) at the Unix epoch.
const TICKS_AT_UNIX_EPOCH: i64 = 621_355_968_000_000_000;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoFile {
    pub path: PathBuf,
    pub name: String,
    pub size: u64,
    /// Last write time in .NET ticks (`LastWriteTimeUtc.Ticks`), for the analysis cache key.
    pub mtime_ticks: i64,
}

impl VideoFile {
    pub fn from_path(path: &Path) -> Result<Self> {
        let meta = std::fs::metadata(path)?;
        let mtime_ticks = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| TICKS_AT_UNIX_EPOCH + (d.as_nanos() / 100) as i64);
        Ok(Self {
            path: path.to_path_buf(),
            name: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            size: meta.len(),
            mtime_ticks,
        })
    }

    /// File name without the extension.
    pub fn stem(&self) -> String {
        self.path
            .file_stem()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

pub fn is_video(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| VIDEO_EXTENSIONS.iter().any(|v| v.eq_ignore_ascii_case(e)))
}

/// A folder's videos (not recursive), sorted by name; or the single file given.
pub fn list_videos(input: &Path) -> Result<Vec<VideoFile>> {
    if !input.is_dir() {
        return Ok(vec![VideoFile::from_path(input)?]);
    }
    let mut paths: Vec<PathBuf> = std::fs::read_dir(input)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.is_file() && is_video(p))
        .collect();
    paths.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    paths.iter().map(|p| VideoFile::from_path(p)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lists_videos_sorted_and_filtered() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["b.MKV", "a.mp4", "notes.txt", "c.m2ts"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        std::fs::create_dir(dir.path().join("sub.mkv")).unwrap();
        let names: Vec<_> = list_videos(dir.path())
            .unwrap()
            .into_iter()
            .map(|f| f.name)
            .collect();
        assert_eq!(names, ["a.mp4", "b.MKV", "c.m2ts"]);
    }

    #[test]
    fn ticks_match_dotnet() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.mkv");
        std::fs::write(&p, b"x").unwrap();
        let f = std::fs::File::options().write(true).open(&p).unwrap();
        // 2024-09-07 12:25:37 UTC
        f.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1_725_711_937))
            .unwrap();
        assert_eq!(
            VideoFile::from_path(&p).unwrap().mtime_ticks,
            638_613_087_370_000_000
        );
    }
}
