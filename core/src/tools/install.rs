//! Installing and updating tools: winget, pip, GitHub/GitLab release lookups, downloads and
//! archive extraction (`Invoke-Winget`, `Initialize-WingetTool`, `Invoke-PipInstall`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde_json::Value;

use super::{find_exe, vapoursynth::python_script_dirs};
use crate::log::Log;
use crate::process::Runner;
use crate::{Error, Result};

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

/// Download progress: (file, bytes done, bytes total or 0).
pub type ProgressFn = dyn Fn(&str, u64, u64) + Send + Sync;

/// Everything that installs: the process runner, the log, the tools folder, and whether to
/// upgrade tools that are already installed (`-UpdateTools`).
pub struct Installer<'a> {
    pub runner: &'a dyn Runner,
    pub log: &'a dyn Log,
    pub tools_dir: PathBuf,
    pub update: bool,
    /// Use and install tools system-wide (PATH, winget, the global Python) as the script did,
    /// instead of the private tools folder.
    pub system: bool,
    /// Download progress: (file, bytes done, bytes total or 0).
    pub progress: Option<&'a ProgressFn>,
}

/// One downloadable release asset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
    pub size: u64,
    pub updated_at: String,
    /// Lowercase hex SHA-256 from GitHub's asset `digest`, when given.
    pub sha256: Option<String>,
}

impl Asset {
    /// The verified download for this asset; an asset without a digest can't be installed.
    pub fn download(&self) -> crate::Result<super::fetch::Download> {
        let sha256 = self.sha256.clone().ok_or_else(|| {
            Error::Tool(format!(
                "{} has no SHA-256 digest to check it against",
                self.name
            ))
        })?;
        Ok(super::fetch::Download {
            name: self.name.clone(),
            url: self.url.clone(),
            sha256,
            size: Some(self.size),
        })
    }
}

/// The `assets` of a GitHub release.
pub fn release_assets(release: &Value) -> Vec<Asset> {
    release
        .get("assets")
        .and_then(Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| Asset {
                    name: x.get("name").and_then(Value::as_str).unwrap_or("").into(),
                    url: x
                        .get("browser_download_url")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .into(),
                    size: x.get("size").and_then(Value::as_u64).unwrap_or(0),
                    updated_at: x
                        .get("updated_at")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .into(),
                    sha256: x
                        .get("digest")
                        .and_then(Value::as_str)
                        .and_then(super::fetch::parse_digest),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// A system-wide tool: its winget id on Windows, its Homebrew formula on macOS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Package {
    pub winget: &'static str,
    pub brew: &'static str,
}

pub const MKVTOOLNIX: Package = Package {
    winget: "MoritzBunkus.MKVToolNix",
    brew: "mkvtoolnix",
};
pub const FFMPEG: Package = Package {
    winget: "Gyan.FFmpeg",
    brew: "ffmpeg",
};
pub const PYTHON: Package = Package {
    winget: "Python.Python.3.13",
    brew: "python@3.12",
};
pub const HANDBRAKE: Package = Package {
    winget: "HandBrake.HandBrake.CLI",
    brew: "handbrake",
};
pub const SEVEN_ZIP: Package = Package {
    winget: "7zip.7zip",
    brew: "sevenzip",
};
/// Windows only: on macOS `tools::makemkv` installs it from makemkv.com (Homebrew's cask is
/// disabled because the app isn't notarized).
pub const MAKEMKV: Package = Package {
    winget: "GuinpinSoft.MakeMKV",
    brew: "",
};
/// Only upgraded on macOS (SvtAv1EncApp); on Windows SVT-AV1 comes with FFmpeg.
pub const SVT_AV1: Package = Package {
    winget: "",
    brew: "svt-av1",
};

impl Package {
    /// The name this OS's package manager knows it by.
    pub fn id(&self) -> &'static str {
        if cfg!(windows) {
            self.winget
        } else {
            self.brew
        }
    }
}

/// `brew install|upgrade <formula>`. Homebrew never asks questions with these.
pub fn brew_args(formula: &str, upgrade: bool) -> Vec<OsString> {
    os(&[if upgrade { "upgrade" } else { "install" }, formula])
}

pub fn winget_args(id: &str, upgrade: bool) -> Vec<OsString> {
    os(&[
        if upgrade { "upgrade" } else { "install" },
        "--id",
        id,
        "--exact",
        "--silent",
        "--accept-source-agreements",
        "--accept-package-agreements",
        "--disable-interactivity",
    ])
}

impl Installer<'_> {
    /// Installs `pkg` (or upgrades it) with winget on Windows and Homebrew elsewhere.
    pub fn package(&self, pkg: Package, upgrade: bool) -> Result<()> {
        if cfg!(windows) {
            self.winget(pkg.winget, upgrade)
        } else {
            self.brew(pkg.brew, upgrade)
        }
    }

    /// `brew install` / `brew upgrade`. Homebrew itself has to be installed by the user: its
    /// installer asks for an administrator password.
    fn brew(&self, formula: &str, upgrade: bool) -> Result<()> {
        let candidates: Vec<PathBuf> = ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
            .map(PathBuf::from)
            .into();
        let brew = find_exe("brew", &candidates).ok_or_else(|| {
            Error::Tool(format!(
                "Homebrew is not installed, so {formula} can't be installed. Install Homebrew from https://brew.sh, then try again."
            ))
        })?;
        self.log.info(&format!(
            "  brew {} {formula}",
            if upgrade { "upgrade" } else { "install" }
        ));
        let out = self.runner.output(&brew, &brew_args(formula, upgrade))?;
        if !out.success() {
            let last = out
                .stderr
                .lines()
                .map(str::trim)
                .rfind(|l| !l.is_empty())
                .unwrap_or("no output");
            let what = format!(
                "brew {} {formula} failed: {last}",
                if upgrade { "upgrade" } else { "install" }
            );
            // An older version still works; only a missing tool stops the batch
            if upgrade {
                self.log.warn(&what);
            } else {
                return Err(Error::Tool(what));
            }
        }
        Ok(())
    }

    fn winget(&self, id: &str, upgrade: bool) -> Result<()> {
        let winget = find_exe("winget", &[]).ok_or_else(|| {
            Error::Tool(format!(
                "winget is not available. Install 'App Installer' from the Microsoft Store, or install {id} manually."
            ))
        })?;
        self.log.info(&format!(
            "  winget {} {id}",
            if upgrade { "upgrade" } else { "install" }
        ));
        // winget's own exit codes cover "already up to date"; success is judged by finding the tool
        let _ = self.runner.output(&winget, &winget_args(id, upgrade))?;
        Ok(())
    }

    /// Finds a system tool; installs it if missing, upgrades it when updating.
    pub fn system_tool(&self, pkg: Package, exe: &str, candidates: &[PathBuf]) -> Result<PathBuf> {
        let mut path = find_exe(exe, candidates);
        if path.is_none() {
            self.package(pkg, false)?;
            path = find_exe(exe, candidates);
        } else if self.update {
            self.package(pkg, true)?;
            path = find_exe(exe, candidates);
        }
        path.ok_or_else(|| {
            Error::Tool(format!(
                "{exe} still not found after installing {}. Restart the app (or open a new terminal) and try again.",
                pkg.id()
            ))
        })
    }

    /// `pip install --upgrade`, then the one-time `vapoursynth config`. True when pip succeeded.
    pub fn pip_install(&self, python: &Path, packages: &[&str]) -> bool {
        self.log
            .info(&format!("  pip install --upgrade {}", packages.join(" ")));
        let mut args = os(&[
            "-m",
            "pip",
            "install",
            "--upgrade",
            "--disable-pip-version-check",
        ]);
        args.extend(packages.iter().map(OsString::from));
        let ok = self.runner.output(python, &args).is_ok_and(|o| o.success());
        // VapourSynth installed through pip needs a one-time "vapoursynth config"
        let candidates: Vec<PathBuf> = python_script_dirs(self.runner, python)
            .into_iter()
            .map(|d| d.join(super::exe("vapoursynth")))
            .collect();
        if let Some(cli) = find_exe("vapoursynth", &candidates) {
            let _ = self.runner.output(&cli, &os(&["config"]));
        }
        ok
    }

    fn curl(&self) -> Result<PathBuf> {
        find_exe("curl", &[]).ok_or_else(|| {
            Error::Tool(
                "curl not found (it ships with Windows 10 and later, and with macOS)".into(),
            )
        })
    }

    /// GET a JSON API (GitHub, GitLab).
    pub fn http_json(&self, url: &str) -> Result<Value> {
        let out = self.runner.output(
            &self.curl()?,
            &os(&["-sSL", "--fail", "--max-time", "30", url]),
        )?;
        if !out.success() {
            return Err(Error::Tool(format!("{url}: {}", out.stderr.trim())));
        }
        Ok(serde_json::from_str(&out.stdout)?)
    }

    /// GET a text file (MKVToolNix's release XML, `.sha256` sidecars).
    pub fn http_text(&self, url: &str) -> Result<String> {
        let out = self.runner.output(
            &self.curl()?,
            &os(&["-sSL", "--fail", "--max-time", "30", url]),
        )?;
        if !out.success() {
            return Err(Error::Tool(format!("{url}: {}", out.stderr.trim())));
        }
        Ok(out.stdout)
    }

    pub fn download(&self, url: &str, to: &Path) -> Result<()> {
        let mut args = os(&["-sSL", "--fail", "--retry", "3", "-o"]);
        args.push(to.into());
        args.push(url.into());
        let out = self.runner.output(&self.curl()?, &args)?;
        if !out.success() {
            return Err(Error::Tool(format!(
                "Download failed: {} ({})",
                to.file_name().unwrap_or_default().to_string_lossy(),
                out.stderr.trim()
            )));
        }
        Ok(())
    }

    /// Extracts a zip with the system `tar` (bsdtar reads zip on Windows 10+, macOS and Linux).
    pub fn extract_zip(&self, zip: &Path, to: &Path) -> Result<()> {
        std::fs::create_dir_all(to)?;
        let tar = find_exe("tar", &[]).ok_or_else(|| Error::Tool("tar not found".into()))?;
        let mut args = os(&["-xf"]);
        args.push(zip.into());
        args.push("-C".into());
        args.push(to.into());
        let out = self.runner.output(&tar, &args)?;
        if !out.success() {
            return Err(Error::Tool(format!(
                "Extracting {} failed: {}",
                zip.display(),
                out.stderr.trim()
            )));
        }
        Ok(())
    }

    /// Extracts a (multi-part) 7z archive with 7-Zip, installing 7-Zip if needed.
    pub fn extract_7z(&self, archive: &Path, to: &Path) -> Result<()> {
        let seven_zip = if self.system {
            let candidates: Vec<PathBuf> = super::env_dir("ProgramFiles", &["7-Zip", "7z.exe"])
                .into_iter()
                .collect();
            self.system_tool(SEVEN_ZIP, "7z", &candidates)?
        } else {
            super::private::init_7zr(self)?
        };
        let mut args = os(&["x"]);
        args.push(archive.into());
        let mut out_flag = OsString::from("-o");
        out_flag.push(to);
        args.push(out_flag);
        args.push("-y".into());
        let out = self.runner.output(&seven_zip, &args)?;
        if !out.success() {
            return Err(Error::Tool(format!(
                "Extracting {} failed: {}",
                archive.display(),
                out.stderr.trim()
            )));
        }
        Ok(())
    }

    /// Swaps `staging` in as `root`, removing the old install.
    pub fn replace_dir(&self, staging: &Path, root: &Path) -> Result<()> {
        if root.exists() {
            std::fs::remove_dir_all(root)?;
        }
        std::fs::rename(staging, root)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn winget_command() {
        let args: Vec<String> = winget_args("Gyan.FFmpeg", true)
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            args,
            [
                "upgrade",
                "--id",
                "Gyan.FFmpeg",
                "--exact",
                "--silent",
                "--accept-source-agreements",
                "--accept-package-agreements",
                "--disable-interactivity"
            ]
        );
    }

    #[test]
    fn brew_command() {
        let args = |upgrade| -> Vec<String> {
            brew_args("handbrake", upgrade)
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };
        assert_eq!(args(false), ["install", "handbrake"]);
        assert_eq!(args(true), ["upgrade", "handbrake"]);
        assert_eq!(
            HANDBRAKE.id(),
            if cfg!(windows) {
                "HandBrake.HandBrake.CLI"
            } else {
                "handbrake"
            }
        );
    }

    #[test]
    fn assets_from_a_release() {
        let rel = serde_json::json!({ "tag_name": "v15.16", "assets": [
            { "name": "a.7z", "browser_download_url": "https://x/a.7z", "size": 10, "updated_at": "2026-01-01T00:00:00Z" }] });
        assert_eq!(
            release_assets(&rel),
            [Asset {
                name: "a.7z".into(),
                url: "https://x/a.7z".into(),
                size: 10,
                updated_at: "2026-01-01T00:00:00Z".into(),
                sha256: None
            }]
        );
    }
}
