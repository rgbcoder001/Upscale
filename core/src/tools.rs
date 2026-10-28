//! Finding external tools. Milestone 1C adds versions, installs, updates and self-tests.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use crate::process::Runner;
use crate::{Error, Result};

/// `name` on PATH, else the first candidate that exists (`Find-Exe`).
pub fn find_exe(name: &str, candidates: &[PathBuf]) -> Option<PathBuf> {
    which::which(name)
        .ok()
        .or_else(|| candidates.iter().find(|c| c.is_file()).cloned())
}

fn env_dir(var: &str, parts: &[&str]) -> Option<PathBuf> {
    let mut p = PathBuf::from(std::env::var_os(var)?);
    for part in parts {
        p.push(part);
    }
    Some(p)
}

/// Where winget puts command-line aliases (`%LOCALAPPDATA%\Microsoft\WinGet\Links`).
pub fn winget_links() -> Option<PathBuf> {
    env_dir("LOCALAPPDATA", &["Microsoft", "WinGet", "Links"])
}

fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Python 3.12+ (`Get-Python`): `py -3`, else `python` unless it's the Microsoft Store stub.
pub fn find_python(runner: &dyn Runner) -> Option<PathBuf> {
    const PROBE: &str =
        r#"import sys; print(sys.executable if sys.version_info >= (3, 12) else "")"#;
    let ask = |exe: &Path, args: &[&str]| {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let out = runner.output(exe, &args).ok()?;
        let last = out
            .stdout
            .lines()
            .map(str::trim)
            .rfind(|l| !l.is_empty())?
            .to_string();
        Some(PathBuf::from(last))
    };
    if let Some(py) = find_exe("py", &[])
        && let Some(exe) = ask(&py, &["-3", "-c", PROBE])
    {
        return Some(exe);
    }
    let python = find_exe("python", &[])?;
    if python
        .to_string_lossy()
        .to_lowercase()
        .contains("windowsapps")
    {
        return None;
    }
    ask(&python, &["-c", PROBE])
}

/// The folder with `upscale.vpy` and `analyze.py`: `UPSCALE_SCRIPTS_DIR`, `scripts\` next to
/// the executable (installed app), or `resources\scripts` above it (a dev build).
pub fn find_scripts_dir() -> Option<PathBuf> {
    let has_scripts = |d: &Path| d.join("analyze.py").is_file() && d.join("upscale.vpy").is_file();
    if let Some(dir) = std::env::var_os("UPSCALE_SCRIPTS_DIR").map(PathBuf::from) {
        return has_scripts(&dir).then_some(dir);
    }
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let installed = exe_dir.join("scripts");
    if has_scripts(&installed) {
        return Some(installed);
    }
    exe_dir
        .ancestors()
        .map(|d| d.join("resources").join("scripts"))
        .find(|d| has_scripts(d))
}

/// The tools the planner needs: probing and analysis.
#[derive(Debug, Clone)]
pub struct ProbeTools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub mkvmerge: PathBuf,
}

impl ProbeTools {
    pub fn find() -> Result<Self> {
        let links: Vec<PathBuf> = winget_links().into_iter().collect();
        let in_links = |name: &str| links.iter().map(|d| d.join(exe(name))).collect::<Vec<_>>();
        let missing = |tool, hint: &str| Error::ToolNotFound {
            tool,
            hint: hint.into(),
        };

        let ffmpeg = find_exe("ffmpeg", &in_links("ffmpeg"))
            .ok_or_else(|| missing("ffmpeg", " (install it with: winget install Gyan.FFmpeg)"))?;
        // ffprobe ships next to ffmpeg
        let mut ffprobe_candidates = in_links("ffprobe");
        ffprobe_candidates.extend(ffmpeg.parent().map(|d| d.join(exe("ffprobe"))));
        let ffprobe = find_exe("ffprobe", &ffprobe_candidates)
            .ok_or_else(|| missing("ffprobe", " next to ffmpeg"))?;
        let mkv_candidates: Vec<PathBuf> = env_dir("ProgramFiles", &["MKVToolNix"])
            .map(|d| d.join(exe("mkvmerge")))
            .into_iter()
            .collect();
        let mkvmerge = find_exe("mkvmerge", &mkv_candidates).ok_or_else(|| {
            missing(
                "mkvmerge",
                " (install it with: winget install MoritzBunkus.MKVToolNix)",
            )
        })?;
        Ok(Self {
            ffmpeg,
            ffprobe,
            mkvmerge,
        })
    }

    pub fn ffprobe(&self) -> &Path {
        &self.ffprobe
    }
}
