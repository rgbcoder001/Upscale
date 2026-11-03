//! Planning a batch: settings, tools, GPU and model, then a plan for every file (script lines
//! 1206–1291). Shared by `upscale-cli plan`, `run` and the app.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::analyze::{MediaAnalyzer, resolve_analysis};
use crate::files::{VideoFile, list_videos};
use crate::gpu::GpuChoice;
use crate::log::Log;
use crate::plan::{FilePlan, Route, plan_file};
use crate::probe::{probe_audio, probe_source, probe_tracks};
use crate::process::Runner;
use crate::settings::{self, Analysis, Backend, Model, Resolved};
use crate::tools::vapoursynth::{DEBAND_PLUGINS, test_vapoursynth};
use crate::tools::{ProbeTools, find_scripts_dir};
use crate::tracks::plan_tracks;
use crate::{Error, Result};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Batch {
    pub input: PathBuf,
    pub out_dir: PathBuf,
    pub resolved: Resolved,
    pub model: Model,
    pub onnx: Option<PathBuf>,
    /// `Real-CUGAN`, the ONNX file name, or `Topaz prob-4`.
    pub model_label: String,
    pub backend: Backend,
    pub gpu: GpuChoice,
    /// The f3kdb deband plugin is installed (checked only when Deband is on).
    pub deband_available: bool,
    pub plans: Vec<FilePlan>,
}

/// Model, ONNX file and label. `search_dir` is where a lone `.onnx` file is picked up.
fn resolve_model(
    r: &Resolved,
    search_dir: &Path,
    log: &dyn Log,
) -> Result<(Model, Option<PathBuf>, String)> {
    let s = &r.settings;
    if !s.upscales() {
        return Ok((Model::Cugan, None, "no upscale".into()));
    }
    if s.use_topaz() {
        let first = s.topaz_filter.split(':').next().unwrap_or("");
        let model = first.strip_prefix("model=").unwrap_or(first);
        return Ok((
            s.model.unwrap_or(Model::Cugan),
            s.onnx.clone(),
            format!("Topaz {model}"),
        ));
    }
    let mut onnx = s.onnx.clone();
    let model = match s.model {
        Some(m) => m,
        None if onnx.is_some() => Model::Onnx,
        None => {
            let mut found: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(search_dir)
                .map(|rd| {
                    rd.filter_map(|e| e.ok())
                        .map(|e| e.path())
                        .filter(|p| {
                            p.is_file()
                                && p.extension()
                                    .is_some_and(|e| e.eq_ignore_ascii_case("onnx"))
                        })
                        .filter_map(|p| Some((p.metadata().ok()?.modified().ok()?, p)))
                        .collect()
                })
                .unwrap_or_default();
            found.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
            match found.first() {
                Some((_, newest)) => {
                    if found.len() > 1 {
                        log.warn(&format!(
                            "Several .onnx models in {}; using newest: {}",
                            search_dir.display(),
                            newest.file_name().unwrap_or_default().to_string_lossy()
                        ));
                    }
                    onnx = Some(newest.clone());
                    Model::Onnx
                }
                None => Model::Cugan,
            }
        }
    };
    if model == Model::Onnx {
        let path = onnx.as_deref().filter(|p| p.is_file()).ok_or_else(|| {
            Error::Settings(format!(
                "ONNX model not found: {}",
                onnx.as_deref().unwrap_or(Path::new("")).display()
            ))
        })?;
        let path = std::path::absolute(path)?;
        let label = path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned();
        return Ok((model, Some(path), label));
    }
    Ok((model, onnx, "Real-CUGAN".into()))
}

/// Reads settings and every file of `input` (a file or a folder), and plans each one.
pub fn prepare(
    input: &Path,
    options: &Map<String, Value>,
    out_dir: &Path,
    runner: &dyn Runner,
    gpu: &GpuChoice,
    tools: &ProbeTools,
    log: &dyn Log,
) -> Result<Batch> {
    let input = std::path::absolute(input)?;
    let videos = list_videos(&input)?;
    prepare_files(&input, videos, options, out_dir, runner, gpu, tools, log)
}

/// Job options merged with the `upscale-settings.json` files for `input` and its parents, with
/// this machine's memory limits ([`settings::limit_memory`]).
pub fn resolve_settings(input: &Path, options: &Map<String, Value>) -> Result<Resolved> {
    let dir = settings::settings_dir(&std::path::absolute(input)?);
    let mut r = settings::resolve(options, &settings::find_settings_files(&dir))?;
    settings::limit_memory(
        &mut r,
        cfg!(target_os = "macos"),
        crate::platform::total_memory(),
    );
    Ok(r)
}

/// Whether each episode can be planned on its own, so the queue can start the first one while
/// the rest are still being analysed. Only automatic analysis on the vs-mlrt route has a season
/// consensus (native resolution, denoise); the Topaz route and encodes without an upscale
/// skip that analysis altogether.
pub fn plans_independently(r: &Resolved) -> bool {
    let s = &r.settings;
    s.use_topaz() || !s.upscales() || s.analysis != Analysis::Auto
}

/// Plans `videos` together (season consensus spans them). `input` is the folder or file the
/// settings files are looked up from. `tools` come from `tools::set::ensure_probe`.
#[allow(clippy::too_many_arguments)]
pub fn prepare_files(
    input: &Path,
    videos: Vec<VideoFile>,
    options: &Map<String, Value>,
    out_dir: &Path,
    runner: &dyn Runner,
    gpu: &GpuChoice,
    tools: &ProbeTools,
    log: &dyn Log,
) -> Result<Batch> {
    let input = std::path::absolute(input)?;
    let dir = settings::settings_dir(&input);
    let resolved = resolve_settings(&input, options)?;
    for w in &resolved.warnings {
        log.warn(w);
    }
    for n in &resolved.notes {
        log.info(n);
    }

    let backend = resolved
        .settings
        .backend
        .unwrap_or_else(|| gpu.default_backend());
    let (model, onnx, model_label) = resolve_model(&resolved, &dir, log)?;

    let s = &resolved.settings;
    if !resolved.applied.is_empty() {
        let files: Vec<String> = resolved
            .files
            .iter()
            .map(|f| f.display().to_string())
            .collect();
        let shown: Vec<String> = resolved
            .applied
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        log.info(&format!(
            "Folder settings ({}): {}",
            files.join(", "),
            shown.join(", ")
        ));
    }
    log.info(&format!(
        "Content: {} | default audio: {} | default subtitles: {}",
        s.content, s.default_audio_language, s.default_subtitle_language
    ));
    if !s.upscales() {
        let tune = if s.tune.is_empty() {
            String::new()
        } else {
            format!(", HandBrake tune {}", s.tune)
        };
        log.info(&format!(
            "Upscaler: none (encode only) | CRF {}, preset {}{tune}",
            s.crf, s.preset
        ));
    } else if s.use_topaz() {
        log.info(&format!(
            "Upscaler: Topaz Video AI | {} | CRF {}, preset {}",
            s.topaz_filter, s.crf, s.preset
        ));
    } else {
        log.info(&format!(
            "Model: {model_label} | backend: {} | CRF {}, preset {}",
            backend.as_str(),
            s.crf,
            s.preset
        ));
    }
    log.info(&format!("SVT params: {}", s.svt_params));
    log.info(&format!(
        "Found {} file(s); output -> {}",
        videos.len(),
        out_dir.display()
    ));
    log.info("Reading files ...");

    let python = tools.python.clone();
    // The plan shows ", deband" only when the plugin is there
    let deband_available = s.deband
        && DEBAND_PLUGINS
            .iter()
            .any(|p| test_vapoursynth(runner, tools.vspipe.as_deref(), &[p]));
    let analyzer = MediaAnalyzer {
        runner,
        ffmpeg: tools.ffmpeg.clone(),
        python,
        analyze_py: find_scripts_dir().map(|d| d.join("analyze.py")),
        cache_dir: out_dir.join("logs").join("analysis"),
    };
    let mut plans = Vec::with_capacity(videos.len());
    for file in videos {
        let src = probe_source(runner, &tools.ffprobe, &file.path)?;
        let path = file.path.clone();
        let mut p = plan_file(&resolved, model, file, src, &analyzer);
        if p.route != Route::Skip {
            let tracks = probe_tracks(runner, &tools.mkvmerge, &path)?;
            let audio = probe_audio(runner, &tools.ffprobe, &path)?;
            p.tracks = Some(plan_tracks(&tracks, &audio, &s.keep_languages));
        }
        plans.push(p);
    }
    resolve_analysis(&mut plans, s.analysis, &analyzer, log);

    Ok(Batch {
        input,
        out_dir: out_dir.to_path_buf(),
        model,
        onnx,
        model_label,
        backend,
        gpu: gpu.clone(),
        deband_available,
        plans,
        resolved,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn resolved(opts: Value) -> Resolved {
        settings::resolve(opts.as_object().unwrap(), &[]).unwrap()
    }

    #[test]
    fn only_vs_mlrt_auto_analysis_needs_the_whole_season() {
        assert!(plans_independently(&resolved(json!({}))), "Anime (Topaz)");
        assert!(plans_independently(&resolved(json!({ "Content": "cg" }))));
        assert!(!plans_independently(&resolved(
            json!({ "Content": "anime" })
        )));
        assert!(plans_independently(&resolved(
            json!({ "Content": "anime", "Analysis": "episode" })
        )));
        assert!(plans_independently(&resolved(
            json!({ "Content": "anime", "Upscaler": "topaz" })
        )));
        assert!(plans_independently(&resolved(
            json!({ "Content": "anime-av1" })
        )));
        assert!(plans_independently(&resolved(json!({ "Content": "live" }))));
    }
}
