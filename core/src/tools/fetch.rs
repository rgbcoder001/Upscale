//! Downloads checked against SHA-256. Files land in `<tools>\.downloads`
//! as `.part`, are hashed, and are renamed only when the hash matches; a verified file that is
//! already there is reused, which matters for the multi-GB vs-mlrt package.

use std::ffi::OsString;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

use sha2::{Digest, Sha256};

use super::find_exe;
use super::install::Installer;
use crate::{Error, Result};

/// A file to download and the hash it must have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Download {
    pub name: String,
    pub url: String,
    /// Lowercase hex SHA-256.
    pub sha256: String,
    /// Expected size when known (shown in progress, checked before hashing).
    pub size: Option<u64>,
}

/// `sha256:ab12…` (GitHub asset digests) → `ab12…`.
pub fn parse_digest(digest: &str) -> Option<String> {
    let hex = digest.trim().strip_prefix("sha256:")?;
    is_sha256(hex).then(|| hex.to_ascii_lowercase())
}

/// The hash for `file` in a `sha256sum`-style sidecar (`<hex>  name`, `<hex> *name`) or a bare hex line.
pub fn parse_sidecar(text: &str, file: &str) -> Option<String> {
    text.lines().find_map(|line| {
        let mut parts = line.split_whitespace();
        let hex = parts.next()?;
        let name = parts.next().map(|n| n.trim_start_matches('*'));
        (is_sha256(hex) && name.is_none_or(|n| n == file || n.ends_with(&format!("/{file}"))))
            .then(|| hex.to_ascii_lowercase())
    })
}

pub(crate) fn is_sha256(hex: &str) -> bool {
    hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit())
}

pub fn sha256_file(path: &Path) -> std::io::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// curl for a download into `part`: follows redirects, continues a part that's already there
/// (`-C -`), retries, and gives up on a connection that can't be made within 30 s or that
/// carries under 1 KB/s for two minutes, instead of waiting on it for ever.
pub fn curl_args(part: &Path, url: &str) -> Vec<OsString> {
    let mut args: Vec<OsString> = [
        "-sSL",
        "--fail",
        "--retry",
        "3",
        "--connect-timeout",
        "30",
        "--speed-limit",
        "1024",
        "--speed-time",
        "120",
        "-C",
        "-",
        "-o",
    ]
    .map(OsString::from)
    .into();
    args.push(part.into());
    args.push(url.into());
    args
}

impl Installer<'_> {
    pub fn downloads_dir(&self) -> PathBuf {
        self.tools_dir.join(".downloads")
    }

    /// Downloads `d` (or reuses a verified copy) and returns its path.
    pub fn fetch(&self, d: &Download) -> Result<PathBuf> {
        let dir = self.downloads_dir();
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(&d.name);
        if path.is_file()
            && d.size
                .is_none_or(|s| path.metadata().is_ok_and(|m| m.len() == s))
            && sha256_file(&path)? == d.sha256
        {
            return Ok(path);
        }
        let _ = std::fs::remove_file(&path);
        // A part left by an interrupted download is continued, not started over (vs-mlrt is
        // about 4 GB); the hash check below still decides whether the result is right
        let part = dir.join(format!("{}.part", d.name));

        let mb = d
            .size
            .map(|s| {
                format!(
                    " ({} MB)",
                    crate::util::fmt_n(s as f64 / (1u64 << 20) as f64, 0)
                )
            })
            .unwrap_or_default();
        self.log.info(&format!("  Downloading {}{mb} ...", d.name));
        let curl = find_exe("curl", &[]).ok_or_else(|| {
            Error::Tool(
                "curl not found (it ships with Windows 10 and later, and with macOS)".into(),
            )
        })?;
        let args = curl_args(&part, &d.url);
        let whole = |part: &Path| {
            d.size
                .is_some_and(|s| part.metadata().is_ok_and(|m| m.len() == s))
        };
        // Twice at most: a part that can't be continued (the server has a new file, or no
        // support for ranges) is deleted and the download starts over once
        let mut first = true;
        while !whole(&part) {
            let continued = part.is_file();
            let mut child = crate::process::command(&curl)
                .args(&args)
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::piped())
                .spawn()?;
            let status = loop {
                if let Some(status) = child.try_wait()? {
                    break status;
                }
                if let Some(report) = self.progress {
                    let done = part.metadata().map(|m| m.len()).unwrap_or(0);
                    report(&d.name, done, d.size.unwrap_or(0));
                }
                std::thread::sleep(Duration::from_millis(500));
            };
            if status.success() {
                break;
            }
            let mut err = String::new();
            if let Some(mut e) = child.stderr.take() {
                let _ = e.read_to_string(&mut err);
            }
            let _ = std::fs::remove_file(&part);
            if !(continued && first) {
                return Err(Error::Tool(format!(
                    "Download failed: {} ({})",
                    d.name,
                    err.trim()
                )));
            }
            first = false;
        }
        if let Some(report) = self.progress {
            // A last report with done = total, so a progress line can finish
            let len = part.metadata().map(|m| m.len()).unwrap_or(0);
            report(&d.name, len, len);
        }
        let got = sha256_file(&part)?;
        if got != d.sha256 {
            let _ = std::fs::remove_file(&part);
            return Err(Error::Tool(format!(
                "checksum mismatch for {}: expected {}, got {got}",
                d.name, d.sha256
            )));
        }
        std::fs::rename(&part, &path)?;
        Ok(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HELLO: &str = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";

    #[test]
    fn digests() {
        assert_eq!(
            parse_digest(&format!("sha256:{}", HELLO.to_uppercase())).as_deref(),
            Some(HELLO)
        );
        assert_eq!(parse_digest("sha1:abc"), None);
        assert_eq!(parse_digest("sha256:abc"), None);
        assert_eq!(
            parse_sidecar(
                &format!("{HELLO}  mkvtoolnix-64-bit-103.0.zip\n"),
                "mkvtoolnix-64-bit-103.0.zip"
            )
            .as_deref(),
            Some(HELLO)
        );
        assert_eq!(parse_sidecar(&format!("{HELLO} *other.zip"), "x.zip"), None);
        assert_eq!(
            parse_sidecar(&format!("{HELLO}\n"), "x.zip").as_deref(),
            Some(HELLO),
            "a bare hash"
        );
    }

    #[test]
    fn hashes_files() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("h.txt");
        std::fs::write(&p, "hello").unwrap();
        assert_eq!(sha256_file(&p).unwrap(), HELLO);
    }

    #[test]
    fn a_verified_download_is_reused_without_network() {
        let dir = tempfile::tempdir().unwrap();
        let log = crate::log::MemoryLog::default();
        let inst = Installer {
            runner: &crate::process::SystemRunner,
            log: &log,
            tools_dir: dir.path().into(),
            update: false,
            system: false,
            progress: None,
        };
        std::fs::create_dir_all(inst.downloads_dir()).unwrap();
        std::fs::write(inst.downloads_dir().join("h.txt"), "hello").unwrap();
        // The URL is unreachable: only a reuse can succeed
        let d = Download {
            name: "h.txt".into(),
            url: "http://127.0.0.1:9/none".into(),
            sha256: HELLO.into(),
            size: Some(5),
        };
        assert_eq!(inst.fetch(&d).unwrap(), inst.downloads_dir().join("h.txt"));
        // A wrong hash on disk means a fresh download, which fails here and leaves nothing behind
        let bad = Download {
            sha256: "0".repeat(64),
            ..d
        };
        assert!(inst.fetch(&bad).is_err());
        assert!(!inst.downloads_dir().join("h.txt").exists());
        assert!(!inst.downloads_dir().join("h.txt.part").exists());
    }
}
