//! Every tool a batch needs (`Initialize-Tools`), and a read-only status view for Settings.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::install::Installer;
use super::svt::{self, Encoder};
use super::topaz::{self, Topaz};
use super::vapoursynth::{
    self as vs, DEBAND_PACKAGES, DEBAND_PLUGINS, REQUIRED_PLUGINS, VS_PACKAGES, VS_VERSION_PIN,
};
use super::{
    ProbeTools, env_dir, exe, find_exe, find_python, mlrt, private, version_text, winget_links,
};
use crate::gpu::GpuChoice;
use crate::process::Runner;
use crate::settings::Backend;
use crate::{Error, Result};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolSet {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub mkvmerge: PathBuf,
    pub python: PathBuf,
    pub vspipe: PathBuf,
    /// The f3kdb deband plugin is installed.
    pub deband: bool,
    /// The folder with `vsmlrt.py`; `None` when not needed (plan only).
    pub mlrt: Option<PathBuf>,
    /// `<tools>/vs-mlrt`, where `upscale.vpy` loads the runtime DLLs from.
    pub mlrt_root: PathBuf,
    pub encoder: Encoder,
    /// HandBrakeCLI, found when a 4K file needs it.
    pub handbrake: Option<PathBuf>,
    pub topaz: Option<Topaz>,
}

impl ToolSet {
    pub fn probe_tools(&self) -> ProbeTools {
        ProbeTools {
            ffmpeg: self.ffmpeg.clone(),
            ffprobe: self.ffprobe.clone(),
            mkvmerge: self.mkvmerge.clone(),
            python: Some(self.python.clone()),
            vspipe: Some(self.vspipe.clone()),
        }
    }
}

/// What `ensure` sets up.
#[derive(Debug, Clone, Copy)]
pub struct Needs<'a> {
    pub gpu: &'a GpuChoice,
    /// vs-mlrt (not for a plan).
    pub mlrt: bool,
    /// Install the deband plugin if missing.
    pub deband: bool,
    /// HandBrakeCLI (4K sources, or when updating).
    pub handbrake: bool,
}

fn links(name: &str) -> Vec<PathBuf> {
    winget_links()
        .map(|d| d.join(exe(name)))
        .into_iter()
        .collect()
}

pub fn find_handbrake() -> Option<PathBuf> {
    find_exe("HandBrakeCLI", &links("HandBrakeCLI"))
}

/// SvtAv1EncApp: in `<tools>\svt-av1` (dropped there by hand; no release ships Windows builds),
/// or also on PATH in system mode.
pub fn find_svt_app(tools_dir: &Path, system: bool) -> Option<PathBuf> {
    let private = tools_dir.join("svt-av1").join(exe("SvtAv1EncApp"));
    if system {
        find_exe("SvtAv1EncApp", &[private])
    } else {
        private.is_file().then_some(private)
    }
}

/// Finds, installs and (when updating) upgrades every tool: in the private tools folder, or
/// system-wide with `Installer::system` as the script did.
pub fn ensure(inst: &Installer, needs: Needs) -> Result<ToolSet> {
    if inst.system {
        ensure_system(inst, needs)
    } else {
        ensure_private(inst, needs)
    }
}

/// The tools planning needs (probing and analysis), installed into the private folder when
/// missing. vs-mlrt is left for the batch.
pub fn ensure_probe(inst: &Installer) -> Result<ProbeTools> {
    if inst.system {
        let mut t = ProbeTools::find()?;
        t.python = find_python(inst.runner);
        t.vspipe = vs::find_vspipe(inst.runner, t.python.as_deref());
        return Ok(t);
    }
    let _lock = private::INSTALL_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (ffmpeg, ffprobe) = private_ffmpeg(inst)?;
    let mkvmerge = private::init_mkvtoolnix(inst)?;
    let (python, vspipe) = private_vapoursynth(inst)?;
    Ok(ProbeTools {
        ffmpeg,
        ffprobe,
        mkvmerge,
        python: Some(python),
        vspipe: Some(vspipe),
    })
}

fn private_ffmpeg(inst: &Installer) -> Result<(PathBuf, PathBuf)> {
    let ffmpeg = private::init_ffmpeg(inst)?;
    let ffprobe = ffmpeg
        .parent()
        .map(|d| d.join(exe("ffprobe")))
        .filter(|p| p.is_file())
        .ok_or_else(|| Error::Tool("ffprobe not found next to ffmpeg.".into()))?;
    Ok((ffmpeg, ffprobe))
}

/// Private Python with VapourSynth R79 and its plugins working.
fn private_vapoursynth(inst: &Installer) -> Result<(PathBuf, PathBuf)> {
    let python = private::init_python(inst)?;
    let vspipe = private::vspipe_exe(&inst.tools_dir);
    let runner = inst.runner;
    let vs_ver = vs::vapoursynth_version(runner, &python);
    let working = |v: Option<u32>| {
        v == Some(VS_VERSION_PIN) && vs::test_vapoursynth(runner, Some(&vspipe), REQUIRED_PLUGINS)
    };
    if inst.update || !working(vs_ver) {
        private::install_vapoursynth(inst, &python)?;
        let vs_ver = vs::vapoursynth_version(runner, &python);
        if !working(vs_ver) {
            return Err(Error::Tool(format!(
                "VapourSynth R{VS_VERSION_PIN} or one of its plugins ({}) is not working in {} (found {}).",
                REQUIRED_PLUGINS.join(", "),
                python.display(),
                vs_ver.map_or("none".to_string(), |v| format!("R{v}"))
            )));
        }
    }
    Ok((python, vspipe))
}

/// Every tool from the private folder, installing what's missing there.
fn ensure_private(inst: &Installer, needs: Needs) -> Result<ToolSet> {
    let (runner, log) = (inst.runner, inst.log);
    log.info("Checking tools ...");
    let _lock = private::INSTALL_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (ffmpeg, ffprobe) = private_ffmpeg(inst)?;
    let mkvmerge = private::init_mkvtoolnix(inst)?;
    let (python, vspipe) = private_vapoursynth(inst)?;

    let has_deband = || {
        DEBAND_PLUGINS
            .iter()
            .any(|p| vs::test_vapoursynth(runner, Some(&vspipe), &[p]))
    };
    let mut deband = has_deband();
    if !deband && needs.deband {
        deband = private::install_deband(inst, &python) && has_deband();
        if !deband {
            log.warn("No f3kdb deband plugin could be installed; debanding is off for this run.");
        }
    }

    let mlrt = if needs.mlrt {
        Some(mlrt::init_vs_mlrt(inst, needs.gpu.mlrt_flavor())?)
    } else {
        None
    };

    // SVT-AV1 comes with the BtbN FFmpeg; a newer SvtAv1EncApp in <tools>\svt-av1 wins
    let encoder = svt::choose_encoder(
        runner,
        find_svt_app(&inst.tools_dir, false).as_deref(),
        Some(&ffmpeg),
        &ffmpeg,
    )
    .ok_or_else(|| Error::Tool("The FFmpeg in the tools folder has no libsvtav1.".into()))?;
    let latest = inst
        .http_json(svt::SVT_RELEASES)
        .ok()
        .and_then(|r| svt::latest_release(&r));
    log.info(&svt::encoder_log_line(&encoder, latest));

    let handbrake = Some(private::init_handbrake(inst)?);
    log.info("  Tools ready.");
    Ok(ToolSet {
        ffmpeg,
        ffprobe,
        mkvmerge,
        python,
        vspipe,
        deband,
        mlrt,
        mlrt_root: inst.tools_dir.join("vs-mlrt"),
        encoder,
        handbrake,
        topaz: None,
    })
}

/// System-wide tools: PATH, winget and pip into the global Python, as the script does.
fn ensure_system(inst: &Installer, needs: Needs) -> Result<ToolSet> {
    let (runner, log) = (inst.runner, inst.log);
    log.info("Checking tools ...");
    let mkv_candidates: Vec<PathBuf> = env_dir("ProgramFiles", &["MKVToolNix", &exe("mkvmerge")])
        .into_iter()
        .collect();
    let mkvmerge = inst.winget_tool("MoritzBunkus.MKVToolNix", "mkvmerge", &mkv_candidates)?;
    let ffmpeg = inst.winget_tool("Gyan.FFmpeg", "ffmpeg", &links("ffmpeg"))?;
    let mut ffprobe_candidates = links("ffprobe");
    ffprobe_candidates.extend(ffmpeg.parent().map(|d| d.join(exe("ffprobe"))));
    let ffprobe = find_exe("ffprobe", &ffprobe_candidates)
        .ok_or_else(|| Error::Tool("ffprobe not found next to ffmpeg.".into()))?;

    // Python 3.12+ (VapourSynth R75 and later require it)
    let python = match find_python(runner) {
        Some(p) => {
            if inst.update {
                inst.winget("Python.Python.3.13", true)?;
            }
            p
        }
        None => {
            inst.winget("Python.Python.3.13", false)?;
            find_python(runner).ok_or_else(|| {
                Error::Tool("Python 3.12+ not found after install. Restart the app (or open a new terminal) and try again.".into())
            })?
        }
    };

    // VapourSynth (pinned) + plugins
    let mut vspipe = vs::find_vspipe(runner, Some(&python));
    let mut vs_ver = vs::vapoursynth_version(runner, &python);
    if let Some(v) = vs_ver.filter(|v| *v != VS_VERSION_PIN) {
        log.info(&format!("  VapourSynth R{v} is installed; switching to R{VS_VERSION_PIN} (the newest release vs-mlrt's plugins work with)"));
    }
    if inst.update
        || vs_ver != Some(VS_VERSION_PIN)
        || !vs::test_vapoursynth(runner, vspipe.as_deref(), REQUIRED_PLUGINS)
    {
        inst.pip_install(&python, VS_PACKAGES);
        vspipe = vs::find_vspipe(runner, Some(&python));
        vs_ver = vs::vapoursynth_version(runner, &python);
        if vs_ver != Some(VS_VERSION_PIN)
            || !vs::test_vapoursynth(runner, vspipe.as_deref(), REQUIRED_PLUGINS)
        {
            return Err(Error::Tool(format!(
                "VapourSynth R{VS_VERSION_PIN} or one of its plugins ({}) is not working (found {} at '{}'). Run 'vspipe --version' to diagnose.",
                REQUIRED_PLUGINS.join(", "),
                vs_ver.map_or("none".to_string(), |v| format!("R{v}")),
                vspipe
                    .as_deref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default()
            )));
        }
    }
    let vspipe = vspipe.ok_or_else(|| Error::Tool("vspipe not found".into()))?;

    // Optional deband plugin (only installed when Deband is on)
    let has_deband = || {
        DEBAND_PLUGINS
            .iter()
            .any(|p| vs::test_vapoursynth(runner, Some(&vspipe), &[p]))
    };
    let mut deband = has_deband();
    if !deband && needs.deband {
        for pkg in DEBAND_PACKAGES {
            if inst.pip_install(&python, &[pkg]) {
                break;
            }
        }
        deband = has_deband();
        if !deband {
            log.warn("No f3kdb deband plugin could be installed; debanding is off for this run.");
        }
    }

    let mlrt = if needs.mlrt {
        Some(mlrt::init_vs_mlrt(inst, needs.gpu.mlrt_flavor())?)
    } else {
        None
    };

    // SVT-AV1: the newest of SvtAv1EncApp, the BtbN FFmpeg build and the winget FFmpeg
    let btbn = svt::init_btbn(inst);
    let encoder = svt::choose_encoder(
        runner,
        find_svt_app(&inst.tools_dir, true).as_deref(),
        btbn.as_deref(),
        &ffmpeg,
    )
    .ok_or_else(|| {
        Error::Tool(
            "No SVT-AV1 encoder found: install SvtAv1EncApp, or an FFmpeg build with libsvtav1."
                .into(),
        )
    })?;
    let latest = inst
        .http_json(svt::SVT_RELEASES)
        .ok()
        .and_then(|r| svt::latest_release(&r));
    log.info(&svt::encoder_log_line(&encoder, latest));

    let handbrake = if needs.handbrake {
        Some(inst.winget_tool(
            "HandBrake.HandBrake.CLI",
            "HandBrakeCLI",
            &links("HandBrakeCLI"),
        )?)
    } else {
        find_handbrake()
    };

    log.info("  Tools ready.");
    Ok(ToolSet {
        ffmpeg,
        ffprobe,
        mkvmerge,
        python,
        vspipe,
        deband,
        mlrt,
        mlrt_root: inst.tools_dir.join("vs-mlrt"),
        encoder,
        handbrake,
        topaz: None,
    })
}

/// One row of the Settings → Tools table.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolStatus {
    pub name: String,
    pub version: Option<String>,
    pub path: Option<PathBuf>,
    pub ok: bool,
    pub note: String,
}

fn banner(runner: &dyn Runner, exe: &Path, args: &[&str]) -> Option<String> {
    let args: Vec<OsString> = args.iter().map(OsString::from).collect();
    let out = runner.output(exe, &args).ok()?;
    version_text(&out.stdout).or_else(|| version_text(&out.stderr))
}

fn row(name: &str, path: Option<PathBuf>, version: Option<String>, note: &str) -> ToolStatus {
    ToolStatus {
        name: name.into(),
        ok: path.is_some(),
        version,
        path,
        note: note.into(),
    }
}

/// What's installed, without installing anything.
pub fn inspect(
    runner: &dyn Runner,
    tools_dir: &Path,
    gpu: &GpuChoice,
    topaz_dir: Option<&Path>,
    system: bool,
) -> (Vec<ToolStatus>, Option<Encoder>) {
    let mut rows = Vec::new();
    // The private folder only, unless system tools are in use
    let located = private::locate(tools_dir);
    let mkv_candidates: Vec<PathBuf> = env_dir("ProgramFiles", &["MKVToolNix", &exe("mkvmerge")])
        .into_iter()
        .collect();
    let mkvmerge = if system {
        find_exe("mkvmerge", &mkv_candidates)
    } else {
        located.mkvmerge.clone()
    };
    let v = mkvmerge
        .as_deref()
        .and_then(|p| banner(runner, p, &["--version"]));
    rows.push(row("mkvmerge", mkvmerge, v, ""));

    let ffmpeg = if system {
        find_exe("ffmpeg", &links("ffmpeg"))
    } else {
        located.ffmpeg.clone()
    };
    let v = ffmpeg
        .as_deref()
        .and_then(|p| banner(runner, p, &["-version"]));
    rows.push(row("FFmpeg", ffmpeg.clone(), v, ""));
    let mut ffprobe_candidates = links("ffprobe");
    ffprobe_candidates.extend(
        ffmpeg
            .as_deref()
            .and_then(Path::parent)
            .map(|d| d.join(exe("ffprobe"))),
    );
    let ffprobe = if system {
        find_exe("ffprobe", &ffprobe_candidates)
    } else {
        located.ffprobe.clone()
    };
    let v = ffprobe
        .as_deref()
        .and_then(|p| banner(runner, p, &["-version"]));
    rows.push(row("ffprobe", ffprobe, v, ""));

    let python = if system {
        find_python(runner)
    } else {
        located.python.clone()
    };
    let v = python
        .as_deref()
        .and_then(|p| banner(runner, p, &["--version"]));
    rows.push(row(
        "Python",
        python.clone(),
        v,
        if python.is_none() {
            "installed into the tools folder on first run"
        } else {
            ""
        },
    ));

    let vspipe = if system {
        vs::find_vspipe(runner, python.as_deref())
    } else {
        located.vspipe.clone()
    };
    let vs_ver = python
        .as_deref()
        .and_then(|p| vs::vapoursynth_version(runner, p));
    let plugins_ok = vs::test_vapoursynth(runner, vspipe.as_deref(), REQUIRED_PLUGINS);
    let deband = DEBAND_PLUGINS
        .iter()
        .any(|p| vs::test_vapoursynth(runner, vspipe.as_deref(), &[p]));
    let note = match (vs_ver, plugins_ok) {
        (Some(VS_VERSION_PIN), true) => format!(
            "plugins OK{}",
            if deband { ", deband available" } else { "" }
        ),
        (Some(v), _) if v != VS_VERSION_PIN => {
            format!("R{v} installed; R{VS_VERSION_PIN} needed (Update tools switches)")
        }
        _ => format!("a plugin is missing ({})", REQUIRED_PLUGINS.join(", ")),
    };
    let mut r = row(
        "VapourSynth",
        vspipe,
        vs_ver.map(|v| format!("R{v}")),
        &note,
    );
    r.ok = r.ok && vs_ver == Some(VS_VERSION_PIN) && plugins_ok;
    rows.push(r);

    let root = tools_dir.join("vs-mlrt");
    let mlrt_dir = mlrt::find_mlrt_dir(&root);
    let installed = mlrt::installed_version(&root);
    let flavor = gpu.mlrt_flavor();
    let note = if mlrt_dir.is_none() {
        format!("not installed ({flavor} package is downloaded on first run)")
    } else if !installed.ends_with(&format!("-{flavor}")) {
        format!("{flavor} package needed for this GPU")
    } else {
        String::new()
    };
    let mut r = row(
        "vs-mlrt",
        mlrt_dir,
        Some(installed).filter(|s| !s.is_empty()),
        &note,
    );
    r.ok = r.ok && note.is_empty();
    rows.push(r);

    let btbn = super::find_file(&tools_dir.join("ffmpeg"), &exe("ffmpeg"));
    let svt_app = find_svt_app(tools_dir, system);
    let encoder = ffmpeg
        .as_deref()
        .and_then(|ff| svt::choose_encoder(runner, svt_app.as_deref(), btbn.as_deref(), ff));
    match &encoder {
        Some(e) => rows.push(row(
            "SVT-AV1",
            Some(e.path.clone()),
            Some(e.version.to_string()),
            &format!("via {}", e.label),
        )),
        None => rows.push(row(
            "SVT-AV1",
            None,
            None,
            "no SvtAv1EncApp or FFmpeg with libsvtav1",
        )),
    }

    let hb = if system {
        find_handbrake()
    } else {
        located.handbrake.clone()
    };
    let v = hb
        .as_deref()
        .and_then(|p| banner(runner, p, &["--version"]));
    rows.push(row("HandBrakeCLI", hb, v, "for 4K sources"));

    match topaz_dir.map(|d| topaz::init_topaz(runner, d, &crate::log::NullLog)) {
        Some(Ok(t)) => {
            let note = if t.signed_in {
                "signed in"
            } else {
                "not signed in: open Topaz Video AI once and sign in"
            };
            rows.push(row("Topaz Video AI", Some(t.ffmpeg), Some(t.version), note));
        }
        _ => rows.push(row(
            "Topaz Video AI",
            None,
            None,
            "only needed for CG films",
        )),
    }

    rows.push(ToolStatus {
        name: "GPU".into(),
        ok: gpu.name.is_some(),
        version: None,
        path: None,
        note: gpu.log_line().trim_start_matches("GPU: ").to_string(),
    });
    rows.push(ToolStatus {
        name: "Backend".into(),
        ok: true,
        version: Some(gpu.default_backend().as_str().into()),
        path: None,
        note: match gpu.default_backend() {
            Backend::Trt => {
                "TensorRT; the first run at a new resolution builds an engine (minutes)".into()
            }
            _ => "ncnn (Vulkan)".into(),
        },
    });
    (rows, encoder)
}
