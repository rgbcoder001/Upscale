//! Settings and their precedence (script lines 29–147): job options > nearest
//! `upscale-settings.json` > farther settings files > content (CG) defaults > built-in.
//! Anything set explicitly is never overridden by analysis (see [`Resolved::is_explicit`]).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value};

use crate::{Error, ProfileId, Result};

pub const SETTINGS_FILE: &str = "upscale-settings.json";

pub const DEFAULT_TOPAZ_FILTER: &str = "model=prob-4:scale=0:preblur=0.25:noise=0.75:details=0.75:halo=0.15:blur=0.5:compression=0.5:estimate=8:blend=0.75:device=0:vram=1:instances=1";
pub const DEFAULT_SVT_PARAMS: &str = "enable-qm=1:qm-min=0:chroma-qm-min=4:enable-variance-boost=1:variance-boost-strength=3:adaptive-film-grain=1:film-grain=2";
/// The owner's live-action settings (given 2026-11-01), used with CRF 14 and the SSIM tune.
pub const LIVE_SVT_PARAMS: &str = "enable-qm=1:qm-min=2:chroma-qm-min=8:qp-scale-compress-strength=1:variance-boost-strength=2:adaptive-film-grain=1:film-grain=4";
/// HandBrakeCLI's SVT-AV1 tunes (`--encoder-tune-list svt_av1_10bit`, HandBrake 1.11.2).
const TUNES: &[&str] = &["", "vq", "psnr", "ssim", "iq", "ms-ssim", "fastdecode"];

/// Keys allowed in `upscale-settings.json` (`$allowedSettings`).
pub const FILE_KEYS: &[&str] = &[
    "Content",
    "Upscaler",
    "TopazFilter",
    "TopazPath",
    "Model",
    "Onnx",
    "Noise",
    "Matrix",
    "Analysis",
    "Crop",
    "Screenshots",
    "Ivtc",
    "NativeHeight",
    "NativeKernel",
    "NativeB",
    "NativeC",
    "NativeTaps",
    "Temporal",
    "TemporalStrength",
    "Deband",
    "Crf",
    "Preset",
    "SvtParams",
    "Tune",
    "KeepLanguages",
    "DefaultAudioLanguage",
    "DefaultSubtitleLanguage",
    "ResumableEncoding",
];

/// Keys only a job option can set (script parameters that a settings file can't).
pub const OPTION_ONLY_KEYS: &[&str] = &["Height", "Start", "Frames", "Backend"];

macro_rules! choice {
    ($(#[$m:meta])* $name:ident { $($variant:ident = $text:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
        #[serde(rename_all = "lowercase")]
        pub enum $name { $($variant),+ }

        impl $name {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $text),+ }
            }
            /// Case-insensitive, like PowerShell's `ValidateSet`.
            fn parse(s: &str) -> Option<Self> {
                let s = s.trim().to_ascii_lowercase();
                match s.as_str() { $($text => Some(Self::$variant),)+ _ => None }
            }
            fn allowed() -> &'static str {
                concat!($($text, " "),+)
            }
        }
    };
}

choice!(Upscaler { Auto = "auto", Vs = "vs", Topaz = "topaz", None = "none" });
choice!(Model { Cugan = "cugan", Onnx = "onnx" });
choice!(Backend { Trt = "trt", Cuda = "cuda", Dml = "dml", Ncnn = "ncnn" });
choice!(Analysis { Auto = "auto", Episode = "episode", Off = "off" });
choice!(CropMode { Auto = "auto", Off = "off" });
choice!(IvtcSetting { Auto = "auto", On = "on", Off = "off" });

const NATIVE_KERNELS: &[&str] = &["bilinear", "bicubic", "lanczos", "spline36"];
const MATRICES: &[&str] = &["", "709", "170m", "470bg"];
const NOISE_LEVELS: &[i64] = &[-1, 0, 3];

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub content: ProfileId,
    pub upscaler: Upscaler,
    pub topaz_filter: String,
    pub topaz_path: Option<PathBuf>,
    /// `None` until resolved: ONNX if a model file is given or found, else Real-CUGAN.
    pub model: Option<Model>,
    pub onnx: Option<PathBuf>,
    /// `None` until resolved from the GPU: `trt` on NVIDIA, `ncnn` otherwise.
    pub backend: Option<Backend>,
    pub noise: i32,
    /// Source matrix; empty = detect.
    pub matrix: String,
    pub height: u32,
    pub analysis: Analysis,
    pub crop: CropMode,
    pub ivtc: IvtcSetting,
    pub native_height: u32,
    pub native_kernel: String,
    pub native_b: f64,
    pub native_c: f64,
    pub native_taps: u32,
    pub temporal: bool,
    pub temporal_strength: i32,
    pub deband: bool,
    pub crf: u32,
    pub preset: i32,
    pub svt_params: String,
    /// HandBrake's `--encoder-tune` (HandBrake route only); empty = HandBrake's default.
    pub tune: String,
    pub keep_languages: Vec<String>,
    pub default_audio_language: String,
    /// `none` = no subtitle track on by default.
    pub default_subtitle_language: String,
    /// Test clips: first frame, `-1` = middle of the episode.
    pub start: i64,
    /// Test clips: number of frames, `0` = whole episode.
    pub frames: u64,
    pub screenshots: u32,
    /// Chunked encoding that survives a closed app (PLAN.md §3.3). On by default since the A/B
    /// on 2026-10-28; encodes too short for two chunks are never chunked.
    pub resumable_encoding: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            content: ProfileId::AnimeTopaz,
            upscaler: Upscaler::Auto,
            topaz_filter: DEFAULT_TOPAZ_FILTER.into(),
            topaz_path: None,
            model: None,
            onnx: None,
            backend: None,
            noise: -1,
            matrix: String::new(),
            height: 2160,
            analysis: Analysis::Auto,
            crop: CropMode::Auto,
            ivtc: IvtcSetting::Auto,
            native_height: 0,
            native_kernel: "bicubic".into(),
            native_b: 0.0,
            native_c: 0.5,
            native_taps: 3,
            temporal: false,
            temporal_strength: 0,
            deband: false,
            crf: 16,
            preset: 5,
            svt_params: DEFAULT_SVT_PARAMS.into(),
            tune: String::new(),
            keep_languages: vec!["eng".into(), "jpn".into(), "und".into()],
            default_audio_language: "jpn".into(),
            default_subtitle_language: "eng".into(),
            start: 0,
            frames: 0,
            screenshots: 4,
            resumable_encoding: true,
        }
    }
}

impl Settings {
    /// Sources below the target height are upscaled, except with Anime AV1 and live action
    /// (unless `Upscaler` names an upscaler) or when `Upscaler` is `none`.
    pub fn upscales(&self) -> bool {
        match self.upscaler {
            Upscaler::None => false,
            Upscaler::Auto => !matches!(self.content, ProfileId::AnimeAv1 | ProfileId::Live),
            Upscaler::Vs | Upscaler::Topaz => true,
        }
    }

    /// Topaz upscales CG and Anime (Topaz) by default, or whenever `Upscaler` is `topaz`.
    pub fn use_topaz(&self) -> bool {
        self.upscaler == Upscaler::Topaz
            || (self.upscaler == Upscaler::Auto
                && matches!(self.content, ProfileId::Cg | ProfileId::AnimeTopaz))
    }

    /// Live action without an upscaler goes through HandBrake at every source size.
    pub fn handbrake_always(&self) -> bool {
        self.content == ProfileId::Live && !self.upscales()
    }

    /// The Topaz install folder: the setting, else `%ProgramFiles%\Topaz Labs LLC\Topaz Video AI`.
    pub fn topaz_dir(&self) -> Option<PathBuf> {
        self.topaz_path.clone().or_else(|| {
            std::env::var_os("ProgramFiles").map(|pf| {
                PathBuf::from(pf)
                    .join("Topaz Labs LLC")
                    .join("Topaz Video AI")
            })
        })
    }

    /// Applies one setting. `base` resolves a relative `Onnx` path. Errors are messages
    /// about the value, like PowerShell's parameter validation.
    fn set(
        &mut self,
        key: &str,
        v: &Value,
        base: Option<&Path>,
    ) -> std::result::Result<(), String> {
        fn choice<T>(
            v: &Value,
            parse: fn(&str) -> Option<T>,
            allowed: &str,
        ) -> std::result::Result<T, String> {
            let s = as_string(v)?;
            parse(&s).ok_or_else(|| format!("'{s}' is not one of: {}", allowed.trim()))
        }
        match key {
            "Content" => {
                let s = as_string(v)?;
                self.content = s.parse().map_err(|_| {
                    format!("'{s}' is not one of: anime-topaz anime anime-av1 cg live")
                })?;
            }
            "Upscaler" => self.upscaler = choice(v, Upscaler::parse, Upscaler::allowed())?,
            "TopazFilter" => self.topaz_filter = as_string(v)?,
            "TopazPath" => self.topaz_path = Some(PathBuf::from(as_string(v)?)),
            "Model" => self.model = Some(choice(v, Model::parse, Model::allowed())?),
            "Onnx" => {
                let p = PathBuf::from(as_string(v)?);
                self.onnx = Some(match base {
                    Some(dir) if p.is_relative() && !p.as_os_str().is_empty() => dir.join(p),
                    _ => p,
                });
            }
            "Backend" => self.backend = Some(choice(v, Backend::parse, Backend::allowed())?),
            "Noise" => {
                let n = as_int(v)?;
                if !NOISE_LEVELS.contains(&n) {
                    return Err(format!("{n} is not one of: -1 0 3"));
                }
                self.noise = n as i32;
            }
            "Matrix" => {
                let s = as_string(v)?.trim().to_lowercase();
                if !MATRICES.contains(&s.as_str()) {
                    return Err(format!("'{s}' is not one of: (empty) 709 170m 470bg"));
                }
                self.matrix = s;
            }
            "Height" => self.height = as_uint(v)?,
            "Analysis" => self.analysis = choice(v, Analysis::parse, Analysis::allowed())?,
            "Crop" => self.crop = choice(v, CropMode::parse, CropMode::allowed())?,
            "Ivtc" => self.ivtc = choice(v, IvtcSetting::parse, IvtcSetting::allowed())?,
            "NativeHeight" => self.native_height = as_uint(v)?,
            "NativeKernel" => {
                let s = as_string(v)?.trim().to_lowercase();
                if !NATIVE_KERNELS.contains(&s.as_str()) {
                    return Err(format!("'{s}' is not one of: {}", NATIVE_KERNELS.join(" ")));
                }
                self.native_kernel = s;
            }
            "NativeB" => self.native_b = as_f64(v)?,
            "NativeC" => self.native_c = as_f64(v)?,
            "NativeTaps" => self.native_taps = as_uint(v)?,
            "Temporal" => self.temporal = as_bool(v)?,
            "TemporalStrength" => self.temporal_strength = as_int(v)? as i32,
            "Deband" => self.deband = as_bool(v)?,
            "Crf" => self.crf = as_uint(v)?,
            "Preset" => self.preset = as_int(v)? as i32,
            "SvtParams" => self.svt_params = as_string(v)?,
            "Tune" => {
                let s = as_string(v)?.trim().to_lowercase();
                if !TUNES.contains(&s.as_str()) {
                    return Err(format!("'{s}' is not one of: {}", TUNES[1..].join(" ")));
                }
                self.tune = s;
            }
            "KeepLanguages" => self.keep_languages = as_list(v)?,
            "DefaultAudioLanguage" => self.default_audio_language = as_string(v)?,
            "DefaultSubtitleLanguage" => self.default_subtitle_language = as_string(v)?,
            "Start" => self.start = as_int(v)?,
            "Frames" => self.frames = as_uint(v)? as u64,
            "Screenshots" => self.screenshots = as_uint(v)?,
            "ResumableEncoding" => self.resumable_encoding = as_bool(v)?,
            _ => return Err("unknown setting".into()),
        }
        Ok(())
    }
}

fn as_string(v: &Value) -> std::result::Result<String, String> {
    match v {
        Value::String(s) => Ok(s.clone()),
        Value::Number(n) => Ok(n.to_string()),
        Value::Bool(b) => Ok(if *b { "True" } else { "False" }.into()),
        Value::Null => Ok(String::new()),
        _ => Err(format!("expected text, got {v}")),
    }
}

fn as_int(v: &Value) -> std::result::Result<i64, String> {
    match v {
        Value::Number(n) => n
            .as_i64()
            .or_else(|| n.as_f64().map(crate::util::round_even))
            .ok_or_else(|| format!("{n} is not a whole number")),
        Value::String(s) => s
            .trim()
            .parse()
            .map_err(|_| format!("'{s}' is not a whole number")),
        _ => Err(format!("expected a number, got {v}")),
    }
}

fn as_uint(v: &Value) -> std::result::Result<u32, String> {
    let n = as_int(v)?;
    u32::try_from(n).map_err(|_| format!("{n} must be 0 or more"))
}

fn as_f64(v: &Value) -> std::result::Result<f64, String> {
    match v {
        Value::Number(n) => n.as_f64().ok_or_else(|| format!("{n} is not a number")),
        Value::String(s) => s
            .trim()
            .parse()
            .map_err(|_| format!("'{s}' is not a number")),
        _ => Err(format!("expected a number, got {v}")),
    }
}

fn as_bool(v: &Value) -> std::result::Result<bool, String> {
    match v {
        Value::Bool(b) => Ok(*b),
        Value::Number(n) => Ok(n.as_f64().is_some_and(|f| f != 0.0)),
        Value::String(s) if s.eq_ignore_ascii_case("true") => Ok(true),
        Value::String(s) if s.eq_ignore_ascii_case("false") => Ok(false),
        _ => Err(format!("expected true or false, got {v}")),
    }
}

/// A JSON list, or a comma-separated string.
fn as_list(v: &Value) -> std::result::Result<Vec<String>, String> {
    match v {
        Value::Array(items) => items.iter().map(as_string).collect(),
        Value::String(s) => Ok(s
            .split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect()),
        _ => Err(format!("expected a list, got {v}")),
    }
}

/// Settings after merging, with where they came from.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Resolved {
    pub settings: Settings,
    /// Keys set by a job option or a settings file. Analysis never overrides these.
    pub explicit: BTreeSet<String>,
    /// Settings files used, farthest first.
    pub files: Vec<PathBuf>,
    /// Values taken from settings files, as `(key, value)` in the order applied (for the log).
    pub applied: Vec<(String, String)>,
    pub warnings: Vec<String>,
}

impl Resolved {
    pub fn is_explicit(&self, key: &str) -> bool {
        self.explicit.contains(key)
    }
}

/// The folder settings are looked up from: the folder itself, or a file's parent.
pub fn settings_dir(input: &Path) -> PathBuf {
    if input.is_dir() {
        input.to_path_buf()
    } else {
        input.parent().map(Path::to_path_buf).unwrap_or_default()
    }
}

/// `upscale-settings.json` in `dir` and every parent, farthest first.
pub fn find_settings_files(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = dir
        .ancestors()
        .map(|d| d.join(SETTINGS_FILE))
        .filter(|p| p.is_file())
        .collect();
    found.reverse();
    found
}

fn display_value(v: &Value) -> String {
    match v {
        Value::Array(items) => items
            .iter()
            .map(|i| as_string(i).unwrap_or_else(|_| i.to_string()))
            .collect::<Vec<_>>()
            .join(","),
        other => as_string(other).unwrap_or_else(|_| other.to_string()),
    }
}

/// Merges job options with the settings files (farthest first) and applies the CG defaults.
pub fn resolve(options: &Map<String, Value>, files: &[PathBuf]) -> Result<Resolved> {
    let mut s = Settings::default();
    let mut explicit = BTreeSet::new();
    let mut applied = Vec::new();
    let mut warnings = Vec::new();

    for file in files {
        let text = std::fs::read_to_string(file)?;
        let text = text.trim_start_matches('\u{feff}');
        let json: Map<String, Value> = serde_json::from_str(text)
            .map_err(|e| Error::Settings(format!("{}: {e}", file.display())))?;
        let base = file.parent();
        for (key, value) in &json {
            let Some(&canonical) = FILE_KEYS.iter().find(|k| k.eq_ignore_ascii_case(key)) else {
                warnings.push(format!(
                    "{}: unknown setting '{key}' ignored",
                    file.display()
                ));
                continue;
            };
            if options.keys().any(|k| k.eq_ignore_ascii_case(canonical)) {
                continue;
            }
            s.set(canonical, value, base)
                .map_err(|e| Error::Settings(format!("{}: {canonical}: {e}", file.display())))?;
            explicit.insert(canonical.to_string());
            // An ordered dictionary in the script: a later file updates the value in place.
            let shown = display_value(value);
            match applied
                .iter_mut()
                .find(|(k, _): &&mut (String, String)| k == canonical)
            {
                Some(entry) => entry.1 = shown,
                None => applied.push((canonical.to_string(), shown)),
            }
        }
    }

    for (key, value) in options {
        let Some(&canonical) = FILE_KEYS
            .iter()
            .chain(OPTION_ONLY_KEYS)
            .find(|k| k.eq_ignore_ascii_case(key))
        else {
            return Err(Error::Settings(format!("unknown setting '{key}'")));
        };
        s.set(canonical, value, None)
            .map_err(|e| Error::Settings(format!("{canonical}: {e}")))?;
        explicit.insert(canonical.to_string());
    }

    if s.content == ProfileId::Live {
        if !explicit.contains("Crf") {
            s.crf = 14;
        }
        if !explicit.contains("SvtParams") {
            s.svt_params = LIVE_SVT_PARAMS.into();
        }
        if !explicit.contains("Tune") {
            s.tune = "ssim".into();
        }
    }
    if matches!(s.content, ProfileId::Cg | ProfileId::Live) {
        if !explicit.contains("Analysis") {
            s.analysis = Analysis::Off;
        }
        if !explicit.contains("DefaultAudioLanguage") {
            s.default_audio_language = "eng".into();
        }
        if !explicit.contains("DefaultSubtitleLanguage") {
            s.default_subtitle_language = "none".into();
        }
    }

    Ok(Resolved {
        settings: s,
        explicit,
        files: files.to_vec(),
        applied,
        warnings,
    })
}

/// Parses `Key=Value` (a JSON value, or plain text) into an option map entry.
pub fn parse_option(kv: &str) -> std::result::Result<(String, Value), String> {
    let (k, v) = kv
        .split_once('=')
        .ok_or_else(|| format!("expected Key=Value, got '{kv}'"))?;
    let value = serde_json::from_str(v.trim()).unwrap_or_else(|_| Value::String(v.to_string()));
    Ok((k.trim().to_string(), value))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn opts(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn tree(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (rel, body) in files {
            let p = dir.path().join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        dir
    }

    #[test]
    fn defaults_match_the_script() {
        let r = resolve(&Map::new(), &[]).unwrap();
        let s = &r.settings;
        assert_eq!((s.crf, s.preset, s.height, s.noise), (16, 5, 2160, -1));
        assert_eq!(s.content, ProfileId::AnimeTopaz);
        assert_eq!(s.svt_params, DEFAULT_SVT_PARAMS);
        assert_eq!(s.keep_languages, ["eng", "jpn", "und"]);
        assert_eq!(
            (
                s.default_audio_language.as_str(),
                s.default_subtitle_language.as_str()
            ),
            ("jpn", "eng")
        );
        assert!(s.use_topaz(), "Anime (Topaz) is the default profile");
        assert_eq!(s.analysis, Analysis::Auto, "crop and IVTC still run");
        assert!(s.resumable_encoding, "on by default since the A/B");
        assert!(r.explicit.is_empty());
    }

    #[test]
    fn nearest_file_wins_and_parents_are_inherited() {
        let dir = tree(&[
            ("upscale-settings.json", r#"{ "Content": "cg", "Crf": 16 }"#),
            ("Show/upscale-settings.json", r#"{ "Crf": 18, "Bogus": 1 }"#),
            ("Show/S01/ep.mkv", ""),
        ]);
        let files = find_settings_files(&dir.path().join("Show/S01"));
        assert_eq!(files.len(), 2);
        assert!(files[0].starts_with(dir.path()) && files[1].parent().unwrap().ends_with("Show"));
        let r = resolve(&Map::new(), &files).unwrap();
        assert_eq!(r.settings.crf, 18);
        assert_eq!(r.settings.content, ProfileId::Cg);
        assert!(r.settings.use_topaz());
        assert_eq!(r.warnings.len(), 1);
        assert!(r.warnings[0].contains("unknown setting 'Bogus' ignored"));
        assert_eq!(
            r.applied,
            [
                ("Content".to_string(), "cg".to_string()),
                ("Crf".to_string(), "18".to_string())
            ]
        );
    }

    #[test]
    fn options_beat_files_and_cg_defaults_only_fill_gaps() {
        let dir = tree(&[(
            "upscale-settings.json",
            r#"{ "Content": "CG", "DefaultSubtitleLanguage": "eng" }"#,
        )]);
        let files = find_settings_files(dir.path());
        let r = resolve(&opts(json!({ "Content": "anime" })), &files).unwrap();
        assert_eq!(r.settings.content, ProfileId::Anime);
        assert_eq!(r.settings.default_audio_language, "jpn");
        assert!(!r.settings.use_topaz(), "plain anime keeps vs-mlrt");

        let r = resolve(&Map::new(), &files).unwrap();
        assert_eq!(r.settings.analysis, Analysis::Off);
        assert_eq!(r.settings.default_audio_language, "eng");
        assert_eq!(
            r.settings.default_subtitle_language, "eng",
            "set in the file, so not a CG default"
        );
        assert!(r.is_explicit("DefaultSubtitleLanguage"));
        assert!(!r.is_explicit("Analysis"));
    }

    #[test]
    fn anime_av1_and_live_action_do_not_upscale() {
        let r = resolve(&opts(json!({ "Content": "anime-av1" })), &[]).unwrap();
        let s = &r.settings;
        assert!(!s.upscales() && !s.use_topaz() && !s.handbrake_always());
        assert_eq!((s.crf, s.preset), (16, 5));
        assert_eq!(s.svt_params, DEFAULT_SVT_PARAMS);
        assert_eq!(s.analysis, Analysis::Auto, "crop and IVTC still run");
        assert_eq!(
            (
                s.default_audio_language.as_str(),
                s.default_subtitle_language.as_str()
            ),
            ("jpn", "eng")
        );

        let r = resolve(&opts(json!({ "Content": "live" })), &[]).unwrap();
        let s = &r.settings;
        assert!(!s.upscales() && !s.use_topaz() && s.handbrake_always());
        assert_eq!((s.crf, s.preset, s.tune.as_str()), (14, 5, "ssim"));
        assert_eq!(s.svt_params, LIVE_SVT_PARAMS);
        assert_eq!(s.analysis, Analysis::Off);
        assert_eq!(
            (
                s.default_audio_language.as_str(),
                s.default_subtitle_language.as_str()
            ),
            ("eng", "none")
        );

        // Explicit values win over the live-action defaults
        let r = resolve(
            &opts(json!({ "Content": "live", "Crf": 18, "Tune": "PSNR", "SvtParams": "a=1" })),
            &[],
        )
        .unwrap();
        let s = &r.settings;
        assert_eq!(
            (s.crf, s.tune.as_str(), s.svt_params.as_str()),
            (18, "psnr", "a=1")
        );
        assert!(resolve(&opts(json!({ "Tune": "fast" })), &[]).is_err());

        // Upscaler none turns any profile into encode-only
        let r = resolve(&opts(json!({ "Upscaler": "none" })), &[]).unwrap();
        assert!(!r.settings.upscales() && !r.settings.use_topaz());
    }

    #[test]
    fn relative_onnx_is_relative_to_its_settings_file() {
        let dir = tree(&[(
            "Show/upscale-settings.json",
            r#"{ "Onnx": "models\\x.onnx" }"#,
        )]);
        let r = resolve(&Map::new(), &find_settings_files(&dir.path().join("Show"))).unwrap();
        assert_eq!(
            r.settings.onnx.unwrap(),
            dir.path().join("Show").join("models\\x.onnx")
        );
    }

    #[test]
    fn validation_and_coercion() {
        let r = resolve(
            &opts(
                json!({ "crf": "12", "Noise": 3, "Temporal": true, "KeepLanguages": "eng, und" }),
            ),
            &[],
        )
        .unwrap();
        assert_eq!(r.settings.crf, 12);
        assert_eq!(r.settings.noise, 3);
        assert!(r.settings.temporal);
        assert_eq!(r.settings.keep_languages, ["eng", "und"]);
        assert!(resolve(&opts(json!({ "Noise": 2 })), &[]).is_err());
        assert!(resolve(&opts(json!({ "Analysis": "sometimes" })), &[]).is_err());
        assert!(resolve(&opts(json!({ "Nope": 1 })), &[]).is_err());
    }

    #[test]
    fn option_parsing() {
        assert_eq!(parse_option("Crf=12").unwrap(), ("Crf".into(), json!(12)));
        assert_eq!(
            parse_option("SvtParams=a=1:b=2").unwrap(),
            ("SvtParams".into(), json!("a=1:b=2"))
        );
        assert_eq!(
            parse_option(r#"KeepLanguages=["eng"]"#).unwrap().1,
            json!(["eng"])
        );
        assert!(parse_option("Crf").is_err());
    }
}
