//! The private tools folder (PLAN.md Phase 4): every tool lives under `<tools>` and is
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
use std::sync::{LazyLock, Mutex};

use regex::Regex;
use serde_json::Value;

use super::fetch::{Download, parse_sidecar};
use super::install::{Asset, Installer, release_assets};
use super::vapoursynth::{DEBAND_PACKAGES, VS_VERSION_PIN};
use super::{exe, find_file};
use crate::{Error, Result};

pub const PYTHON_RELEASE: &str =
    "https://api.github.com/repos/astral-sh/python-build-standalone/releases/latest";
pub const SEVEN_ZIP_RELEASE: &str = "https://api.github.com/repos/ip7z/7zip/releases/latest";
pub const HANDBRAKE_RELEASE: &str =
    "https://api.github.com/repos/HandBrake/HandBrake/releases/latest";
pub const MKVTOOLNIX_LATEST: &str = "https://mkvtoolnix.download/latest-release.xml";

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

/// Downloads and unpacks an archive into `<tools>\<name>`, replacing what was there.
fn install_archive(inst: &Installer, d: &Download, name: &str) -> Result<PathBuf> {
    let file = inst.fetch(d)?;
    let root = inst.tools_dir.join(name);
    let staging = inst.tools_dir.join(format!("{name}.new"));
    let _ = std::fs::remove_dir_all(&staging);
    inst.extract_zip(&file, &staging)?;
    inst.replace_dir(&staging, &root)?;
    write_stamp(&root, &format!("{} {}", d.name, d.sha256))?;
    Ok(root)
}

fn pick(release: &Value, re: &Regex) -> Option<Asset> {
    release_assets(release)
        .into_iter()
        .find(|a| re.is_match(&a.name))
}

static PYTHON_ASSET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^cpython-3\.12\.\d+\+\d+-x86_64-pc-windows-msvc-install_only\.tar\.gz$")
        .expect("valid regex")
});
static HANDBRAKE_ASSET: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^HandBrakeCLI-[\d.]+-win-x86_64\.zip$").expect("valid regex"));
static MKV_VERSION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?s)<latest-source>.*?<version>([\d.]+)</version>").expect("valid regex")
});

/// The Python 3.12 build (full stdlib and pip, not the stripped variant).
pub fn pick_python_asset(release: &Value) -> Option<Asset> {
    pick(release, &PYTHON_ASSET)
}

pub fn pick_handbrake_asset(release: &Value) -> Option<Asset> {
    pick(release, &HANDBRAKE_ASSET)
}

pub fn mkvtoolnix_version(xml: &str) -> Option<String> {
    MKV_VERSION.captures(xml).map(|c| c[1].to_string())
}

/// `7zr.exe`, the standalone 7-Zip console extractor.
pub fn init_7zr(inst: &Installer) -> Result<PathBuf> {
    let dir = inst.tools_dir.join("7zip");
    let exe = dir.join("7zr.exe");
    if exe.is_file() && !inst.update {
        return Ok(exe);
    }
    let rel = inst.http_json(SEVEN_ZIP_RELEASE)?;
    let asset = release_assets(&rel)
        .into_iter()
        .find(|a| a.name == "7zr.exe")
        .ok_or_else(|| Error::Tool("the 7-Zip release has no 7zr.exe".into()))?;
    let d = asset.download()?;
    let want = format!("{} {}", d.name, d.sha256);
    if exe.is_file() && stamp(&dir) == want {
        return Ok(exe);
    }
    let file = inst.fetch(&d)?;
    std::fs::create_dir_all(&dir)?;
    std::fs::copy(file, &exe)?;
    write_stamp(&dir, &want)?;
    Ok(exe)
}

/// Python 3.12 from python-build-standalone, in `<tools>\python`.
pub fn init_python(inst: &Installer) -> Result<PathBuf> {
    let exe = python_exe(&inst.tools_dir);
    if exe.is_file() && !inst.update {
        return Ok(exe);
    }
    let rel = inst.http_json(PYTHON_RELEASE)?;
    let asset = pick_python_asset(&rel).ok_or_else(|| {
        Error::Tool(
            "no Python 3.12 Windows build in the latest python-build-standalone release".into(),
        )
    })?;
    let d = asset.download()?;
    let root = inst.tools_dir.join("python");
    if exe.is_file() && stamp(&root) == format!("{} {}", d.name, d.sha256) {
        inst.log.info(&format!("  Python ({}) is current", d.name));
        return Ok(exe);
    }
    // The archive holds a top-level python\ folder
    let file = inst.fetch(&d)?;
    let staging = inst.tools_dir.join("python.new");
    let _ = std::fs::remove_dir_all(&staging);
    inst.extract_zip(&file, &staging)?;
    inst.replace_dir(&staging.join("python"), &root)?;
    let _ = std::fs::remove_dir_all(&staging);
    write_stamp(&root, &format!("{} {}", d.name, d.sha256))?;
    inst.log.info(&format!("  Python installed: {}", d.name));
    Ok(exe)
}

/// Which release of a PyPI package to install.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pin {
    Exact(&'static str),
    /// The newest release whose major version is below this.
    Below(u64),
    Latest,
}

/// VapourSynth (pinned to R79) and the plugins `upscale.vpy` needs (see `VS_PACKAGES`).
pub const VS_WHEELS: &[(&str, Pin)] = &[
    ("vapoursynth", Pin::Exact("79")),
    ("vapoursynth-bestsource", Pin::Below(22)),
    ("vapoursynth-vivtc", Pin::Latest),
    ("vapoursynth-descale", Pin::Latest),
    ("vapoursynth-mvtools", Pin::Latest),
    ("vapoursynth-akarin", Pin::Latest),
];

fn numeric(v: &str) -> Option<Vec<u64>> {
    v.split('.').map(|p| p.parse().ok()).collect()
}

/// A Windows wheel of `json` (PyPI's `/pypi/<name>/json`): (version, sha256), honouring `pin`.
/// Pre-releases (versions with letters) are skipped.
pub fn pick_wheel(json: &Value, pin: Pin) -> Option<(String, String)> {
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
        .filter(|(v, _)| match pin {
            Pin::Exact(want) => v.as_str() == want,
            Pin::Below(major) => numeric(v).is_some_and(|n| n[0] < major),
            Pin::Latest => numeric(v).is_some(),
        })
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

/// pip-installs hash-checked wheels into the private Python.
fn pip_install_pinned(inst: &Installer, python: &Path, packages: &[(&str, Pin)]) -> Result<()> {
    let mut pins = Vec::new();
    for (name, pin) in packages {
        let json = inst.http_json(&format!("https://pypi.org/pypi/{name}/json"))?;
        let (version, sha) = pick_wheel(&json, *pin).ok_or_else(|| {
            Error::Tool(format!("no Windows wheel of {name} for {pin:?} on PyPI"))
        })?;
        pins.push((name.to_string(), version, sha));
    }
    let file = inst.downloads_dir().join("requirements.txt");
    std::fs::create_dir_all(inst.downloads_dir())?;
    std::fs::write(&file, requirements(&pins))?;
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

pub fn install_vapoursynth(inst: &Installer, python: &Path) -> Result<()> {
    debug_assert_eq!(VS_WHEELS[0].1, Pin::Exact("79"));
    let _ = VS_VERSION_PIN;
    pip_install_pinned(inst, python, VS_WHEELS)
}

/// The optional f3kdb deband plugin; true when one installed.
pub fn install_deband(inst: &Installer, python: &Path) -> bool {
    DEBAND_PACKAGES
        .iter()
        .any(|pkg| pip_install_pinned(inst, python, &[(pkg, Pin::Latest)]).is_ok())
}

/// BtbN FFmpeg in `<tools>\ffmpeg`. Updating keeps the old build when the download fails.
pub fn init_ffmpeg(inst: &Installer) -> Result<PathBuf> {
    let root = inst.tools_dir.join("ffmpeg");
    let current = find_file(&root, &exe("ffmpeg"));
    if let Some(exe) = &current
        && !inst.update
    {
        return Ok(exe.clone());
    }
    let fetched = (|| -> Result<PathBuf> {
        let rel = inst.http_json(super::svt::BTBN_LATEST)?;
        let asset = super::svt::pick_btbn_asset(&rel)
            .ok_or_else(|| Error::Tool("no Windows build in the latest BtbN release".into()))?;
        // Same stamp as the script, so a folder it installed is recognised as current
        let want = format!("{} {}", asset.name, asset.updated_at);
        if let Some(exe) = &current
            && stamp(&root) == want
        {
            inst.log
                .info(&format!("  FFmpeg (BtbN) {} is current", asset.name));
            return Ok(exe.clone());
        }
        install_archive(inst, &asset.download()?, "ffmpeg")?;
        write_stamp(&root, &want)?;
        inst.log
            .info(&format!("  FFmpeg (BtbN) installed: {}", asset.name));
        find_file(&root, &exe("ffmpeg"))
            .ok_or_else(|| Error::Tool("ffmpeg.exe not found in the download".into()))
    })();
    match (fetched, current) {
        (Ok(exe), _) => Ok(exe),
        (Err(e), Some(old)) => {
            inst.log.warn(&format!(
                "Couldn't update the BtbN FFmpeg build ({e}); using the one already installed."
            ));
            Ok(old)
        }
        (Err(e), None) => Err(e),
    }
}

/// MKVToolNix portable in `<tools>\mkvtoolnix`.
pub fn init_mkvtoolnix(inst: &Installer) -> Result<PathBuf> {
    let root = inst.tools_dir.join("mkvtoolnix");
    let current = find_file(&root, &exe("mkvmerge"));
    if let Some(exe) = &current
        && !inst.update
    {
        return Ok(exe.clone());
    }
    let xml = inst.http_text(MKVTOOLNIX_LATEST)?;
    let version = mkvtoolnix_version(&xml)
        .ok_or_else(|| Error::Tool("couldn't read the MKVToolNix version".into()))?;
    let name = format!("mkvtoolnix-64-bit-{version}.zip");
    let url = format!("https://mkvtoolnix.download/windows/releases/{version}/{name}");
    let sha256 = parse_sidecar(&inst.http_text(&format!("{url}.sha256"))?, &name)
        .ok_or_else(|| Error::Tool(format!("no SHA-256 published for {name}")))?;
    let d = Download {
        name,
        url,
        sha256,
        size: None,
    };
    if let Some(exe) = &current
        && stamp(&root) == format!("{} {}", d.name, d.sha256)
    {
        inst.log.info(&format!("  MKVToolNix {version} is current"));
        return Ok(exe.clone());
    }
    install_archive(inst, &d, "mkvtoolnix")?;
    inst.log.info(&format!("  MKVToolNix {version} installed"));
    find_file(&root, &exe("mkvmerge"))
        .ok_or_else(|| Error::Tool("mkvmerge.exe not found in the download".into()))
}

/// HandBrakeCLI in `<tools>\handbrake`.
pub fn init_handbrake(inst: &Installer) -> Result<PathBuf> {
    let root = inst.tools_dir.join("handbrake");
    let current = find_file(&root, &exe("HandBrakeCLI"));
    if let Some(exe) = &current
        && !inst.update
    {
        return Ok(exe.clone());
    }
    let rel = inst.http_json(HANDBRAKE_RELEASE)?;
    let asset = pick_handbrake_asset(&rel)
        .ok_or_else(|| Error::Tool("no HandBrakeCLI Windows build in the latest release".into()))?;
    let d = asset.download()?;
    if let Some(exe) = &current
        && stamp(&root) == format!("{} {}", d.name, d.sha256)
    {
        inst.log
            .info(&format!("  HandBrakeCLI ({}) is current", d.name));
        return Ok(exe.clone());
    }
    install_archive(inst, &d, "handbrake")?;
    inst.log
        .info(&format!("  HandBrakeCLI installed: {}", d.name));
    find_file(&root, &exe("HandBrakeCLI"))
        .ok_or_else(|| Error::Tool("HandBrakeCLI.exe not found in the download".into()))
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
    fn release_assets_are_picked() {
        let rel = json!({ "assets": [
            { "name": "cpython-3.12.16+20261026-x86_64-pc-windows-msvc-install_only_stripped.tar.gz", "digest": "sha256:aa" },
            { "name": "cpython-3.13.9+20261026-x86_64-pc-windows-msvc-install_only.tar.gz" },
            { "name": "cpython-3.12.16+20261026-x86_64-pc-windows-msvc-install_only.tar.gz", "size": 30 },
            { "name": "HandBrakeCLI-1.11.2-win-aarch64.zip" },
            { "name": "HandBrakeCLI-1.11.2-win-x86_64.zip.sig" },
            { "name": "HandBrakeCLI-1.11.2-win-x86_64.zip" }] });
        assert_eq!(
            pick_python_asset(&rel).unwrap().name,
            "cpython-3.12.16+20261026-x86_64-pc-windows-msvc-install_only.tar.gz"
        );
        assert_eq!(
            pick_handbrake_asset(&rel).unwrap().name,
            "HandBrakeCLI-1.11.2-win-x86_64.zip"
        );
        assert!(
            pick_python_asset(&rel).unwrap().download().is_err(),
            "no digest, no install"
        );
        let xml = "<mkvtoolnix-releases><latest-source><source-code-url>x</source-code-url><version>103.0</version></latest-source><latest-windows-pre><version></version></latest-windows-pre></mkvtoolnix-releases>";
        assert_eq!(mkvtoolnix_version(xml).as_deref(), Some("103.0"));
    }

    #[test]
    fn wheels_and_requirements() {
        let file =
            |name: &str, sha: &str| json!({ "filename": name, "digests": { "sha256": sha } });
        let bs = json!({ "releases": {
            "20": [file("vapoursynth_bestsource-20-py3-none-win_amd64.whl", "a20")],
            "21": [file("vapoursynth_bestsource-21-py3-none-win_amd64.whl", "a21")],
            "22": [file("vapoursynth_bestsource-22-py3-none-win_amd64.whl", "a22")],
            "23rc1": [file("vapoursynth_bestsource-23rc1-py3-none-win_amd64.whl", "x")] } });
        assert_eq!(
            pick_wheel(&bs, Pin::Below(22)),
            Some(("21".into(), "a21".into()))
        );
        assert_eq!(
            pick_wheel(&bs, Pin::Latest),
            Some(("22".into(), "a22".into())),
            "pre-releases are skipped"
        );
        let vs = json!({ "releases": {
            "79": [file("vapoursynth-79.tar.gz", "src"), file("vapoursynth-79-cp312-abi3-win_amd64.whl", "w79")],
            "80": [file("vapoursynth-80-cp312-abi3-win_amd64.whl", "w80")] } });
        assert_eq!(
            pick_wheel(&vs, Pin::Exact("79")),
            Some(("79".into(), "w79".into())),
            "the wheel, not the sdist"
        );
        assert_eq!(
            requirements(&[
                ("vapoursynth".into(), "79".into(), "w79".into()),
                ("vapoursynth-bestsource".into(), "21".into(), "a21".into())
            ]),
            "vapoursynth==79 --hash=sha256:w79\nvapoursynth-bestsource==21 --hash=sha256:a21\n"
        );
        assert_eq!(
            VS_WHEELS.len(),
            super::super::vapoursynth::VS_PACKAGES.len(),
            "the same packages as the script"
        );
    }
}
