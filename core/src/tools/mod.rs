//! External tools: finding them, their versions, installing and updating them, and the
//! self-tests that run before a batch (`Initialize-Tools` and friends in the script).

pub mod fetch;
pub mod install;
pub mod mlrt;
pub mod private;
pub mod set;
pub mod svt;
pub mod topaz;
pub mod vapoursynth;

use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::process::Runner;
use crate::{Error, Result};

/// A three-part version (`4.2.0`), compared numerically.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default, Serialize)]
pub struct Version(pub u32, pub u32, pub u32);

impl Version {
    /// `4.2.0` or `v4.2.0`.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().trim_start_matches('v');
        let mut parts = s.split('.').map(|p| p.parse::<u32>());
        let v = Self(
            parts.next()?.ok()?,
            parts.next()?.ok()?,
            parts.next()?.ok()?,
        );
        parts.next().is_none().then_some(v)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.0, self.1, self.2)
    }
}

static VERSION_TEXT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"v?(\d+(?:\.\d+)+)").expect("valid regex"));

/// The first dotted version number in a tool's banner, for display (`83.0`, `7.1.1`).
pub fn version_text(banner: &str) -> Option<String> {
    let line = banner.lines().map(str::trim).find(|l| !l.is_empty())?;
    // "ffmpeg version 2026-06-15-git-44d082edc8-full_build-www.gyan.dev": git builds carry a date
    if let Some(token) = line
        .split_once(" version ")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        && !VERSION_TEXT.is_match(token.split('-').next().unwrap_or(""))
    {
        let short = token.split("-full_build").next().unwrap_or(token);
        return Some(
            short
                .split("-essentials_build")
                .next()
                .unwrap_or(short)
                .to_string(),
        );
    }
    VERSION_TEXT.captures(line).map(|c| c[1].to_string())
}

/// The app's private tools folder: `UPSCALE_TOOLS_DIR`, else `<app data>/tools`.
pub fn default_tools_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("UPSCALE_TOOLS_DIR") {
        return PathBuf::from(dir);
    }
    app_data_dir().join("tools")
}

/// Where chunks and temporary audio go by default: on this machine's own disk, never shared
/// with another machine (`%LOCALAPPDATA%\com.upscale.app\work` on Windows, `<app data>/work`
/// elsewhere).
pub fn default_work_dir() -> PathBuf {
    let local = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("com.upscale.app"))
    } else {
        None
    };
    local.unwrap_or_else(app_data_dir).join("work")
}

/// The VapourSynth venv on macOS (`~/upscale-venv`), which the app uses without it being
/// activated and creates when it's missing: Homebrew's Python refuses a system-wide
/// `pip install`.
pub fn mac_venv_dir() -> Option<PathBuf> {
    env_dir("HOME", &["upscale-venv"])
}
/// Homebrew's bin folders (Apple Silicon, then Intel).
const BREW_BINS: [&str; 2] = ["/opt/homebrew/bin", "/usr/local/bin"];

/// PATH for a macOS app opened from Finder, which only gets `/usr/bin:/bin:/usr/sbin:/sbin`:
/// the venv's `bin` first (its `python3.12` and `vspipe` win), then the current entries, then
/// the Homebrew folders that are missing. Entries already there keep their place.
fn gui_path(current: Vec<PathBuf>, venv: Option<PathBuf>, brew: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = venv.into_iter().filter(|v| !current.contains(v)).collect();
    out.extend(current);
    for b in brew {
        if !out.contains(&b) {
            out.push(b);
        }
    }
    out
}

/// Lets the macOS app find the Homebrew tools and the VapourSynth venv the way the CLI does
/// from a terminal (see [`gui_path`]). Does nothing elsewhere. Call it at startup, before any
/// other thread exists.
pub fn add_mac_tool_dirs() {
    if !cfg!(target_os = "macos") {
        return;
    }
    let current: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    let venv = mac_venv_dir().map(|d| d.join("bin")).filter(|d| d.is_dir());
    let brew = BREW_BINS
        .iter()
        .map(PathBuf::from)
        .filter(|d| d.is_dir())
        .collect();
    if let Ok(path) = std::env::join_paths(gui_path(current, venv, brew)) {
        // SAFETY: called once at startup on the main thread, before any other thread can read
        // the environment.
        unsafe { std::env::set_var("PATH", path) };
    }
}

/// Per-user app data (Tauri's `app_data_dir` for the `com.upscale.app` identifier).
pub fn app_data_dir() -> PathBuf {
    const ID: &str = "com.upscale.app";
    let home = || {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default()
    };
    if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(home)
            .join(ID)
    } else if cfg!(target_os = "macos") {
        home().join("Library").join("Application Support").join(ID)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home().join(".local").join("share"))
            .join(ID)
    }
}

/// The first file called `name` under `root`: a folder's own files before its subfolders
/// (`Get-ChildItem -Recurse -Filter`).
pub fn find_file(root: &Path, name: &str) -> Option<PathBuf> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .collect();
    entries.sort();
    if let Some(f) = entries
        .iter()
        .find(|p| p.is_file() && p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(name)))
    {
        return Some(f.clone());
    }
    entries
        .iter()
        .filter(|p| p.is_dir())
        .find_map(|d| find_file(d, name))
}

/// `name` on PATH, else the first candidate that exists (`Find-Exe`).
pub fn find_exe(name: &str, candidates: &[PathBuf]) -> Option<PathBuf> {
    which::which(name)
        .ok()
        .or_else(|| candidates.iter().find(|c| c.is_file()).cloned())
}

pub(crate) fn env_dir(var: &str, parts: &[&str]) -> Option<PathBuf> {
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

/// ` (install it with: winget install <id>)`, or the Homebrew formula outside Windows.
pub(crate) fn install_hint(winget_id: &str, brew_formula: &str) -> String {
    if cfg!(windows) {
        format!(" (install it with: winget install {winget_id})")
    } else {
        format!(" (install it with: brew install {brew_formula})")
    }
}

/// An executable's file name on this OS (`ffmpeg.exe` on Windows, `ffmpeg` elsewhere).
pub fn exe(name: &str) -> String {
    format!("{name}{}", std::env::consts::EXE_SUFFIX)
}

/// Python 3.12+ (`Get-Python`): `py -3`, else `python` unless it's the Microsoft Store stub.
/// Elsewhere `python3.12`, `python3`, then `python` (an activated venv provides all three).
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
    if !cfg!(windows) {
        return ["python3.12", "python3", "python"]
            .into_iter()
            .filter_map(|name| find_exe(name, &[]))
            .find_map(|python| ask(&python, &["-c", PROBE]));
    }
    let launchers: Vec<PathBuf> = [
        env_dir(
            "LOCALAPPDATA",
            &["Programs", "Python", "Launcher", "py.exe"],
        ),
        env_dir("WINDIR", &["py.exe"]),
    ]
    .into_iter()
    .flatten()
    .collect();
    if let Some(py) = find_exe("py", &launchers)
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
/// the executable (installed app), `Contents/Resources/scripts` (the macOS app bundle, whose
/// executables are in `Contents/MacOS`), or `resources\scripts` above it (a dev build).
pub fn find_scripts_dir() -> Option<PathBuf> {
    let has_scripts = |d: &Path| d.join("analyze.py").is_file() && d.join("upscale.vpy").is_file();
    if let Some(dir) = std::env::var_os("UPSCALE_SCRIPTS_DIR").map(PathBuf::from) {
        return has_scripts(&dir).then_some(dir);
    }
    let exe = std::env::current_exe().ok()?;
    let exe_dir = exe.parent()?;
    let installed = [
        Some(exe_dir.join("scripts")),
        exe_dir
            .parent()
            .map(|c| c.join("Resources").join("scripts")),
    ];
    if let Some(dir) = installed.into_iter().flatten().find(|d| has_scripts(d)) {
        return Some(dir);
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
    /// Python with VapourSynth, for `analyze.py`.
    pub python: Option<PathBuf>,
    pub vspipe: Option<PathBuf>,
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
            .ok_or_else(|| missing("ffmpeg", &install_hint("Gyan.FFmpeg", "ffmpeg")))?;
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
                &install_hint("MoritzBunkus.MKVToolNix", "mkvtoolnix"),
            )
        })?;
        Ok(Self {
            ffmpeg,
            ffprobe,
            mkvmerge,
            python: None,
            vspipe: None,
        })
    }

    pub fn ffprobe(&self) -> &Path {
        &self.ffprobe
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_app_path_gets_the_venv_first_and_homebrew_last() {
        let p = |s: &str| PathBuf::from(s);
        let finder = vec![p("/usr/bin"), p("/bin"), p("/usr/sbin"), p("/sbin")];
        assert_eq!(
            gui_path(
                finder.clone(),
                Some(p("/Users/u/upscale-venv/bin")),
                vec![p("/opt/homebrew/bin")]
            ),
            [
                "/Users/u/upscale-venv/bin",
                "/usr/bin",
                "/bin",
                "/usr/sbin",
                "/sbin",
                "/opt/homebrew/bin"
            ]
            .map(p)
        );
        // A terminal's PATH already has them: nothing moves
        let shell = vec![
            p("/Users/u/upscale-venv/bin"),
            p("/opt/homebrew/bin"),
            p("/usr/bin"),
        ];
        assert_eq!(
            gui_path(
                shell.clone(),
                Some(p("/Users/u/upscale-venv/bin")),
                vec![p("/opt/homebrew/bin")]
            ),
            shell
        );
        assert_eq!(gui_path(finder.clone(), None, vec![]), finder);
    }

    #[test]
    fn versions() {
        assert_eq!(Version::parse("v4.2.0"), Some(Version(4, 2, 0)));
        assert_eq!(Version::parse("4.2"), None);
        assert!(Version(4, 10, 0) > Version(4, 9, 9));
        assert_eq!(Version(4, 1, 0).to_string(), "4.1.0");
        assert_eq!(
            version_text("mkvmerge v83.0 ('Circle Of Friends') 64-bit").as_deref(),
            Some("83.0")
        );
        assert_eq!(
            version_text(
                "
HandBrake 1.9.2
"
            )
            .as_deref(),
            Some("1.9.2")
        );
        assert_eq!(
            version_text("ffmpeg version 7.1.1-full_build-www.gyan.dev").as_deref(),
            Some("7.1.1")
        );
        assert_eq!(
            version_text(
                "ffmpeg version 2026-06-15-git-44d082edc8-full_build-www.gyan.dev Copyright"
            )
            .as_deref(),
            Some("2026-06-15-git-44d082edc8")
        );
        assert_eq!(
            version_text("ffmpeg version n9.0-latest-win64-gpl-9.0").as_deref(),
            Some("9.0")
        );
    }

    #[test]
    fn finds_files_breadth_first_per_folder() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("a/b")).unwrap();
        std::fs::write(dir.path().join("a/b/vsmlrt.py"), "").unwrap();
        std::fs::write(dir.path().join("a/VSMLRT.py"), "").unwrap();
        assert_eq!(
            find_file(dir.path(), "vsmlrt.py").unwrap(),
            dir.path().join("a").join("VSMLRT.py")
        );
        assert_eq!(find_file(dir.path(), "none.py"), None);
    }
}
