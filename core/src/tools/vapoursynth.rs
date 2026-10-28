//! VapourSynth: vspipe, the pinned release and the required plugins (`Find-Vspipe`,
//! `Get-VapourSynthVersion`, `Test-VapourSynth`).

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use super::{env_dir, exe, find_exe};
use crate::process::Runner;

/// VapourSynth is pinned to R79: vs-mlrt's plugins still use VapourSynth's API 3, which R80
/// removed (github.com/AmusementClub/vs-mlrt/issues/171). BestSource 22+ requires R80, so it
/// is held below 22. Raise both once vs-mlrt publishes API 4 builds.
pub const VS_VERSION_PIN: u32 = 79;

/// akarin provides the Expr filter vs-mlrt uses internally (e.g. in Real-CUGAN).
pub const REQUIRED_PLUGINS: &[&str] = &["bs", "vivtc", "descale", "mv", "akarin"];

pub const VS_PACKAGES: &[&str] = &[
    "vapoursynth==79",
    "vapoursynth-bestsource<22",
    "vapoursynth-vivtc",
    "vapoursynth-descale",
    "vapoursynth-mvtools",
    "vapoursynth-akarin",
];

/// Optional deband plugins (either works), and their pip packages.
pub const DEBAND_PLUGINS: &[&str] = &["neo_f3kdb", "f3kdb"];
pub const DEBAND_PACKAGES: &[&str] = &["vapoursynth-neo-f3kdb", "vapoursynth-f3kdb"];

fn os(args: &[&str]) -> Vec<OsString> {
    args.iter().map(OsString::from).collect()
}

/// Python's script folders (system and user), where pip puts `vspipe` and `vapoursynth`.
pub fn python_script_dirs(runner: &dyn Runner, python: &Path) -> Vec<PathBuf> {
    let probe = "import sysconfig; print(sysconfig.get_path('scripts')); print(sysconfig.get_path('scripts', 'nt_user'))";
    runner
        .output(python, &os(&["-c", probe]))
        .map(|o| {
            o.stdout
                .lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(PathBuf::from)
                .collect()
        })
        .unwrap_or_default()
}

pub fn find_vspipe(runner: &dyn Runner, python: Option<&Path>) -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = python
        .map(|py| {
            python_script_dirs(runner, py)
                .into_iter()
                .map(|d| d.join(exe("vspipe")))
                .collect()
        })
        .unwrap_or_default();
    candidates.extend(env_dir(
        "ProgramFiles",
        &["VapourSynth", "core", &exe("vspipe")],
    ));
    find_exe("vspipe", &candidates)
}

/// The installed VapourSynth release number (e.g. 79).
pub fn vapoursynth_version(runner: &dyn Runner, python: &Path) -> Option<u32> {
    let probe = r#"import vapoursynth as v; c = v.core; cv = getattr(c, "core_version", None); print(cv.release_major if cv else c.version_number())"#;
    let out = runner
        .output(python, &os(&["-W", "ignore", "-c", probe]))
        .ok()?;
    out.stdout
        .lines()
        .map(str::trim)
        .rfind(|l| !l.is_empty())?
        .parse()
        .ok()
}

/// A script that loads every plugin namespace and outputs one blank frame.
pub fn test_script(plugins: &[&str]) -> String {
    let mut lines = vec!["from vapoursynth import core".to_string()];
    lines.extend(plugins.iter().map(|p| format!("core.{p}")));
    lines.push("core.std.BlankClip(length=1).set_output()".into());
    lines.join("\n") + "\n"
}

/// True when vspipe runs and every listed plugin namespace is available.
pub fn test_vapoursynth(runner: &dyn Runner, vspipe: Option<&Path>, plugins: &[&str]) -> bool {
    static N: AtomicU32 = AtomicU32::new(0);
    let Some(vspipe) = vspipe else {
        return false;
    };
    let path = std::env::temp_dir().join(format!(
        "upscale-vstest-{}-{}.vpy",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    if std::fs::write(&path, test_script(plugins)).is_err() {
        return false;
    }
    let mut args = os(&["--info"]);
    args.push(path.clone().into());
    args.push("-".into());
    let ok = runner.output(vspipe, &args).is_ok_and(|o| o.success());
    let _ = std::fs::remove_file(&path);
    ok
}

/// `Frames: N` from `vspipe --info`.
pub fn info_frames(text: &str) -> Option<u64> {
    text.lines()
        .find_map(|l| l.trim().strip_prefix("Frames:"))
        .and_then(|n| n.trim().parse().ok())
}

/// The frame rate from `vspipe --info` (`FPS: 24000/1001 (23.976 fps)`).
pub fn info_fps(text: &str) -> Option<f64> {
    let line = text.lines().find_map(|l| l.trim().strip_prefix("FPS:"))?;
    crate::util::parse_rational(line.split_whitespace().next()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plugin_test_script() {
        assert_eq!(
            test_script(&["bs", "akarin"]),
            "from vapoursynth import core\ncore.bs\ncore.akarin\ncore.std.BlankClip(length=1).set_output()\n"
        );
    }

    #[test]
    fn vspipe_info_frames() {
        let info = "Width: 3840\nHeight: 2160\nFrames: 73272\nFPS: 24000/1001 (23.976 fps)\nFormat Name: YUV420P10\n";
        assert_eq!(info_frames(info), Some(73272));
        assert_eq!(info_frames("Error"), None);
        assert!((info_fps(info).unwrap() - 23.976).abs() < 0.001);
    }
}
