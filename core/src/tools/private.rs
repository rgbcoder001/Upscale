//! The private tools folder: every tool lives under `<tools>` and is
//! downloaded there, checked against its published SHA-256. Nothing is installed system-wide
//! and the user's own Python is never touched.
//!
//! ```text
//! <tools>\python\          python-build-standalone 3.12 + VapourSynth R79 and plugins (pip)
//! <tools>\vs-mlrt\         vs-mlrt (cuda or vk)
//! <tools>\ffmpeg\          BtbN FFmpeg (ffmpeg, ffprobe, libsvtav1)
//! <tools>\mkvtoolnix\      MKVToolNix portable
//! <tools>\handbrake\       HandBrakeCLI
//! <tools>\7zip\7zr.exe     for the vs-mlrt 7z archive
//! ```

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde_json::Value;

use super::install::Installer;
use super::pins::{self, Pinned, Wheel};
use super::vapoursynth::{DEBAND_PACKAGES, VS_VERSION_PIN};
use super::{exe, find_file};
use crate::{Error, Result};

/// One install at a time (planning and a batch start can both ask for tools).
pub static INSTALL_LOCK: Mutex<()> = Mutex::new(());

/// What is installed in a tools folder (nothing is downloaded).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Located {
    pub python: Option<PathBuf>,
    pub vspipe: Option<PathBuf>,
    pub ffmpeg: Option<PathBuf>,
    pub ffprobe: Option<PathBuf>,
    pub mkvmerge: Option<PathBuf>,
    pub handbrake: Option<PathBuf>,
    pub seven_zip: Option<PathBuf>,
}

fn existing(p: PathBuf) -> Option<PathBuf> {
    p.is_file().then_some(p)
}

pub fn python_exe(tools: &Path) -> PathBuf {
    tools.join("python").join(exe("python"))
}

/// The real vspipe inside the VapourSynth package. pip's `Scriptsspipe.exe` is a launcher
/// with the Python path baked in, so it stops working when the tools folder moves.
pub fn vspipe_exe(tools: &Path) -> PathBuf {
    tools
        .join("python")
        .join("Lib")
        .join("site-packages")
        .join("vapoursynth")
        .join(exe("vspipe"))
}

pub fn locate(tools: &Path) -> Located {
    let ffmpeg = find_file(&tools.join("ffmpeg"), &exe("ffmpeg"));
    Located {
        python: existing(python_exe(tools)),
        vspipe: existing(vspipe_exe(tools)),
        ffprobe: ffmpeg
            .as_deref()
            .and_then(Path::parent)
            .and_then(|d| existing(d.join(exe("ffprobe")))),
        ffmpeg,
        mkvmerge: find_file(&tools.join("mkvtoolnix"), &exe("mkvmerge")),
        handbrake: find_file(&tools.join("handbrake"), &exe("HandBrakeCLI")),
        seven_zip: existing(tools.join("7zip").join("7zr.exe")),
    }
}

fn stamp(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("SOURCE.txt"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn write_stamp(dir: &Path, value: &str) -> Result<()> {
    std::fs::write(dir.join("SOURCE.txt"), format!("{value}\n"))?;
    Ok(())
}

/// Installs the pinned build `pin` into `<tools>\<name>`, unless it's already there, and
/// returns the path of `exe_name` inside it. `inner` is the archive's own top folder, when it
/// has one that the tool folder replaces. Nothing is fetched when the stamp matches: tools only
/// change when Upscale's pins do ([`pins`]).
fn ensure_pinned(
    inst: &Installer,
    pin: &Pinned,
    name: &str,
    exe_name: &str,
    inner: Option<&str>,
    label: &str,
) -> Result<PathBuf> {
    let root = inst.tools_dir.join(name);
    let current = find_file(&root, exe_name);
    if let Some(exe) = &current
        && stamp(&root) == pin.stamp()
    {
        if inst.update {
            inst.log
                .info(&format!("  {label} ({}) is current", pin.name));
        }
        return Ok(exe.clone());
    }
    let installed = (|| -> Result<PathBuf> {
        let file = inst.fetch(&pin.download())?;
        let staging = inst.tools_dir.join(format!("{name}.new"));
        let _ = std::fs::remove_dir_all(&staging);
        inst.extract_zip(&file, &staging)?;
        let unpacked = inner.map_or(staging.clone(), |d| staging.join(d));
        inst.replace_dir(&unpacked, &root)?;
        let _ = std::fs::remove_dir_all(&staging);
        write_stamp(&root, &pin.stamp())?;
        let _ = std::fs::remove_file(&file);
        find_file(&root, exe_name)
            .ok_or_else(|| Error::Tool(format!("{exe_name} not found in {}", pin.name)))
    })();
    match (installed, current) {
        (Ok(exe), _) => {
            inst.log.info(&format!("  {label} installed: {}", pin.name));
            Ok(exe)
        }
        // A tool that worked stays in use when its replacement can't be installed
        (Err(e), Some(old)) => {
            inst.log.warn(&format!(
                "Couldn't install {label} {} ({e}); using the version already installed.",
                pin.name
            ));
            Ok(old)
        }
        (Err(e), None) => Err(e),
    }
}

/// `7zr.exe`, the standalone 7-Zip console extractor.
pub fn init_7zr(inst: &Installer) -> Result<PathBuf> {
    let pin = &pins::SEVEN_ZIP;
    let dir = inst.tools_dir.join("7zip");
    let exe = dir.join("7zr.exe");
    if exe.is_file() && stamp(&dir) == pin.stamp() {
        return Ok(exe);
    }
    let file = inst.fetch(&pin.download())?;
    std::fs::create_dir_all(&dir)?;
    std::fs::copy(file, &exe)?;
    write_stamp(&dir, &pin.stamp())?;
    Ok(exe)
}

/// Python 3.12 from python-build-standalone, in `<tools>\python`.
pub fn init_python(inst: &Installer) -> Result<PathBuf> {
    // The archive holds a top-level python\ folder
    let exe_name = exe("python");
    let python = ensure_pinned(
        inst,
        &pins::PYTHON,
        "python",
        &exe_name,
        Some("python"),
        "Python",
    )?;
    // `find_file` searches below the folder; the interpreter is the one at its top
    let top = python_exe(&inst.tools_dir);
    Ok(if top.is_file() { top } else { python })
}

/// The newest release of a PyPI package (the optional deband plugin, which isn't pinned).
fn numeric(v: &str) -> Option<Vec<u64>> {
    v.split('.').map(|p| p.parse().ok()).collect()
}

/// The newest Windows wheel of `json` (PyPI's `/pypi/<name>/json`): (version, sha256).
/// Pre-releases (versions with letters) are skipped.
pub fn pick_wheel(json: &Value) -> Option<(String, String)> {
    let releases = json.get("releases")?.as_object()?;
    let wheel = |files: &Value| {
        files.as_array()?.iter().find_map(|f| {
            let name = f.get("filename")?.as_str()?;
            let ok = name.ends_with("win_amd64.whl") || name.ends_with("-none-any.whl");
            ok.then(|| f.pointer("/digests/sha256")?.as_str().map(str::to_string))?
        })
    };
    releases
        .iter()
        .filter(|(v, _)| numeric(v).is_some())
        .filter_map(|(v, files)| Some((numeric(v).unwrap_or_default(), v.clone(), wheel(files)?)))
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(_, v, sha)| (v, sha))
}

/// A pip requirements file with a hash per package (`--require-hashes`).
pub fn requirements(pins: &[(String, String, String)]) -> String {
    pins.iter()
        .map(|(name, version, sha)| format!("{name}=={version} --hash=sha256:{sha}\n"))
        .collect()
}

/// The installed wheels as pip reports them (`name==version`, lower case), to tell whether the
/// pinned set is already there.
fn installed_wheels(inst: &Installer, python: &Path) -> Vec<String> {
    let args = ["-m", "pip", "freeze", "--disable-pip-version-check"].map(OsString::from);
    inst.runner
        .output(python, &args)
        .map(|o| {
            o.stdout
                .lines()
                .map(|l| l.trim().to_ascii_lowercase().replace('_', "-"))
                .collect()
        })
        .unwrap_or_default()
}

/// Whether every pinned wheel is installed at its pinned version.
pub fn wheels_current(installed: &[String], wheels: &[Wheel]) -> bool {
    wheels.iter().all(|w| {
        let want = format!("{}=={}", w.name, w.version).to_ascii_lowercase();
        installed.contains(&want)
    })
}

/// pip-installs hash-checked wheels into the private Python: (name, version, sha256).
fn pip_install_pinned(
    inst: &Installer,
    python: &Path,
    pins: &[(String, String, String)],
) -> Result<()> {
    let file = inst.downloads_dir().join("requirements.txt");
    std::fs::create_dir_all(inst.downloads_dir())?;
    std::fs::write(&file, requirements(pins))?;
    let shown: Vec<String> = pins.iter().map(|(n, v, _)| format!("{n}=={v}")).collect();
    inst.log.info(&format!("  pip install {}", shown.join(" ")));
    let mut args: Vec<OsString> = [
        "-m",
        "pip",
        "install",
        "--disable-pip-version-check",
        "--no-deps",
        "--only-binary",
        ":all:",
        "--require-hashes",
        "--upgrade",
        "-r",
    ]
    .map(OsString::from)
    .into();
    args.push(file.into());
    let out = inst.runner.output(python, &args)?;
    if !out.success() {
        let tail: Vec<&str> = out.stderr.lines().rev().take(3).collect();
        return Err(Error::Tool(format!(
            "pip failed: {}",
            tail.into_iter().rev().collect::<Vec<_>>().join(" | ")
        )));
    }
    // VapourSynth installed through pip needs a one-time "vapoursynth config" (run through
    // python -m, not the Scripts launcher, which breaks when the tools folder moves)
    let _ = inst
        .runner
        .output(python, &["-m", "vapoursynth", "config"].map(OsString::from));
    Ok(())
}

/// VapourSynth and its plugins at their pinned versions ([`pins::VS_WHEELS`]).
pub fn install_vapoursynth(inst: &Installer, python: &Path) -> Result<()> {
    debug_assert_eq!(pins::VS_WHEELS[0].version, VS_VERSION_PIN.to_string());
    if wheels_current(&installed_wheels(inst, python), pins::VS_WHEELS) {
        return Ok(());
    }
    let wheels: Vec<(String, String, String)> = pins::VS_WHEELS
        .iter()
        .map(|w| (w.name.into(), w.version.into(), w.sha256.into()))
        .collect();
    pip_install_pinned(inst, python, &wheels)
}

/// The optional f3kdb deband plugin, newest release; true when one installed.
pub fn install_deband(inst: &Installer, python: &Path) -> bool {
    DEBAND_PACKAGES.iter().any(|pkg| {
        let newest = inst
            .http_json(&format!("https://pypi.org/pypi/{pkg}/json"))
            .ok()
            .and_then(|json| pick_wheel(&json));
        newest.is_some_and(|(version, sha)| {
            pip_install_pinned(inst, python, &[(pkg.to_string(), version, sha)]).is_ok()
        })
    })
}

/// FFmpeg (BtbN build, with libsvtav1) in `<tools>\ffmpeg`.
pub fn init_ffmpeg(inst: &Installer) -> Result<PathBuf> {
    ensure_pinned(
        inst,
        &pins::FFMPEG,
        "ffmpeg",
        &exe("ffmpeg"),
        None,
        "FFmpeg (BtbN)",
    )
}

/// MKVToolNix portable in `<tools>\mkvtoolnix`.
pub fn init_mkvtoolnix(inst: &Installer) -> Result<PathBuf> {
    ensure_pinned(
        inst,
        &pins::MKVTOOLNIX,
        "mkvtoolnix",
        &exe("mkvmerge"),
        None,
        "MKVToolNix",
    )
}

/// HandBrakeCLI in `<tools>\handbrake`.
pub fn init_handbrake(inst: &Installer) -> Result<PathBuf> {
    ensure_pinned(
        inst,
        &pins::HANDBRAKE,
        "handbrake",
        &exe("HandBrakeCLI"),
        None,
        "HandBrakeCLI",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn locates_only_inside_the_tools_folder() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(
            locate(dir.path()),
            Located::default(),
            "PATH is not consulted"
        );
        let t = dir.path();
        // The executables' names on this OS (`ffmpeg.exe` on Windows, `ffmpeg` elsewhere)
        for f in [
            format!("python/{}", exe("python")),
            format!("python/Lib/site-packages/vapoursynth/{}", exe("vspipe")),
            format!("ffmpeg/ffmpeg-n9.0/bin/{}", exe("ffmpeg")),
            format!("ffmpeg/ffmpeg-n9.0/bin/{}", exe("ffprobe")),
            format!("mkvtoolnix/mkvtoolnix/{}", exe("mkvmerge")),
            format!("handbrake/{}", exe("HandBrakeCLI")),
            "7zip/7zr.exe".into(),
        ] {
            let p = t.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "").unwrap();
        }
        let l = locate(t);
        assert!(
            l.ffprobe
                .unwrap()
                .ends_with(format!("bin/{}", exe("ffprobe")))
        );
        assert!(
            l.mkvmerge
                .unwrap()
                .ends_with(format!("mkvtoolnix/{}", exe("mkvmerge")))
        );
        assert!(
            l.python.is_some()
                && l.vspipe.is_some()
                && l.handbrake.is_some()
                && l.seven_zip.is_some()
        );
    }

    #[test]
    fn wheels_and_requirements() {
        let file =
            |name: &str, sha: &str| json!({ "filename": name, "digests": { "sha256": sha } });
        let deband = json!({ "releases": {
            "2.0": [file("f3kdb-2.0.tar.gz", "src"), file("f3kdb-2.0-py3-none-win_amd64.whl", "w20")],
            "10.0": [file("f3kdb-10.0-py3-none-win_amd64.whl", "w10")],
            "11rc1": [file("f3kdb-11rc1-py3-none-win_amd64.whl", "x")] } });
        assert_eq!(
            pick_wheel(&deband),
            Some(("10.0".into(), "w10".into())),
            "the newest numbered release, compared as numbers; pre-releases are skipped"
        );
        let sdist = json!({ "releases": { "1": [file("f3kdb-1.tar.gz", "src")] } });
        assert_eq!(pick_wheel(&sdist), None, "a wheel, not the source");
        assert_eq!(
            requirements(&[
                ("vapoursynth".into(), "79".into(), "w79".into()),
                ("vapoursynth-bestsource".into(), "21".into(), "a21".into())
            ]),
            "vapoursynth==79 --hash=sha256:w79\nvapoursynth-bestsource==21 --hash=sha256:a21\n"
        );
        // pip freeze writes underscores and capitals as the packages do
        let installed: Vec<String> = [
            "VapourSynth==79",
            "vapoursynth_bestsource==21.0",
            "vapoursynth-vivtc==2.0",
            "vapoursynth-descale==12",
            "vapoursynth-mvtools==29",
            "vapoursynth-akarin==1.5.0",
            "pip==25.3",
        ]
        .iter()
        .map(|l| l.to_ascii_lowercase().replace('_', "-"))
        .collect();
        assert!(wheels_current(&installed, pins::VS_WHEELS));
        assert!(
            !wheels_current(&installed[1..], pins::VS_WHEELS),
            "VapourSynth itself is missing"
        );
        let older: Vec<String> = installed
            .iter()
            .map(|l| l.replace("akarin==1.5.0", "akarin==1.4.0"))
            .collect();
        assert!(
            !wheels_current(&older, pins::VS_WHEELS),
            "a plugin at another version"
        );
    }
}
