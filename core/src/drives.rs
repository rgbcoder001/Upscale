//! Finding a Blu-ray or DVD in a drive (RIPPING.md, "Detection"). Cheap file-system checks
//! only; MakeMKV reads a disc once it shows up.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::process::Runner;

/// A mounted disc and how MakeMKV opens it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscVolume {
    pub path: PathBuf,
    /// The volume label (`SAMPLE_MOVIE`).
    pub label: String,
    /// MakeMKV's source for the drive: `dev:/dev/rdisk8` (macOS), `dev:D:` (Windows).
    pub source: String,
}

/// A Blu-ray (`BDMV`) or DVD (`VIDEO_TS`) at the top of `root`.
pub fn is_video_disc(root: &Path) -> bool {
    root.join("BDMV").is_dir() || root.join("VIDEO_TS").is_dir()
}

/// `DeviceNode` from `diskutil info -plist` (`/dev/disk8`).
pub fn device_node(plist: &str) -> Option<String> {
    let after = plist.split("<key>DeviceNode</key>").nth(1)?;
    let value = after.split("<string>").nth(1)?.split("</string>").next()?;
    Some(value.trim().to_string()).filter(|v| v.starts_with("/dev/"))
}

/// MakeMKV's source for a macOS device: the raw node (`/dev/disk8` → `dev:/dev/rdisk8`), as
/// MakeMKV lists drives.
pub fn mac_source(device: &str) -> String {
    format!("dev:{}", device.replacen("/dev/disk", "/dev/rdisk", 1))
}

pub fn diskutil_args(volume: &Path) -> Vec<OsString> {
    vec!["info".into(), "-plist".into(), volume.into()]
}

/// The video discs mounted right now.
pub fn video_discs(runner: &dyn Runner) -> Vec<DiscVolume> {
    if cfg!(windows) {
        return ('D'..='Z')
            .map(|l| PathBuf::from(format!("{l}:\\")))
            .filter(|root| is_video_disc(root))
            .map(|root| {
                let letter = root.to_string_lossy().trim_end_matches('\\').to_string();
                DiscVolume {
                    label: letter.clone(),
                    source: format!("dev:{letter}"),
                    path: root,
                }
            })
            .collect();
    }
    let Ok(entries) = std::fs::read_dir("/Volumes") else {
        return Vec::new();
    };
    entries
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| is_video_disc(p))
        .filter_map(|path| {
            let out = runner
                .output(Path::new("/usr/sbin/diskutil"), &diskutil_args(&path))
                .ok()?;
            let device = device_node(&out.stdout)?;
            Some(DiscVolume {
                label: path.file_name()?.to_string_lossy().into_owned(),
                source: mac_source(&device),
                path,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_from_diskutil() {
        let plist = "<dict>\n\t<key>DeviceIdentifier</key>\n\t<string>disk8</string>\n\t<key>DeviceNode</key>\n\t<string>/dev/disk8</string>\n</dict>";
        assert_eq!(device_node(plist).as_deref(), Some("/dev/disk8"));
        assert_eq!(mac_source("/dev/disk8"), "dev:/dev/rdisk8");
        assert_eq!(device_node("<dict></dict>"), None);
    }

    #[test]
    fn blu_ray_and_dvd_folders() {
        let dir = tempfile::tempdir().unwrap();
        assert!(!is_video_disc(dir.path()));
        std::fs::create_dir(dir.path().join("BDMV")).unwrap();
        assert!(is_video_disc(dir.path()));
        let dvd = tempfile::tempdir().unwrap();
        std::fs::create_dir(dvd.path().join("VIDEO_TS")).unwrap();
        assert!(is_video_disc(dvd.path()));
    }
}
