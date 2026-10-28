//! Picture analysis (`Get-BlackBorders`, `Get-InterlaceDetect`, `Invoke-SourceAnalysis`,
//! `Resolve-Analysis`): black borders and telecine from ffmpeg, native resolution and noise
//! from `analyze.py` (cached per file), then season consensus over a folder.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;
use serde_json::{Value, json};

use crate::files::VideoFile;
use crate::log::Log;
use crate::plan::{FilePlan, Native, Route};
use crate::probe::SourceInfo;
use crate::process::Runner;
use crate::report::noise_label;
use crate::settings::Analysis;
use crate::util::{fmt_inv, fmt_n};

/// Bump to invalidate cached `analyze.py` results (the script's `$AnalysisVersion`).
pub const ANALYSIS_VERSION: u32 = 2;

/// A native-resolution match must be this clear to be used by a single episode on its own.
/// Weaker matches (analyze.py's 0.06 up to this) are only used when the season agrees on them:
/// on real discs, isolated weak matches turned out to be false (different answers per episode).
pub const NATIVE_STRONG: f64 = 0.12;

/// Black borders to remove, in pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct Crop {
    pub l: u32,
    pub r: u32,
    pub t: u32,
    pub b: u32,
}

/// `idet` result over the sampled frames.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InterlaceDetect {
    pub interlaced: bool,
    /// `tff` or `bff`.
    pub field: &'static str,
    pub detail: String,
}

pub trait Analyzer {
    /// `Get-BlackBorders`; `None` = no crop.
    fn black_borders(&self, file: &Path, src: &SourceInfo) -> Option<Crop>;
    /// `Get-InterlaceDetect`; `None` = not enough frames to decide.
    fn interlace(&self, file: &Path, src: &SourceInfo) -> Option<InterlaceDetect>;
    /// `Invoke-SourceAnalysis`: the `analyze.py` result, or `None` (warned) when it failed.
    fn source_analysis(
        &self,
        _plan: &FilePlan,
        _native: bool,
        _noise: bool,
        _log: &dyn Log,
    ) -> Option<Value> {
        None
    }
}

/// No picture analysis: nothing cropped, metadata trusted.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoAnalysis;

impl Analyzer for NoAnalysis {
    fn black_borders(&self, _: &Path, _: &SourceInfo) -> Option<Crop> {
        None
    }
    fn interlace(&self, _: &Path, _: &SourceInfo) -> Option<InterlaceDetect> {
        None
    }
}

static CROP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"crop=(\d+):(\d+):(\d+):(\d+)").expect("valid regex"));
static IDET: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)Multi frame detection:\s*TFF:\s*(\d+)\s*BFF:\s*(\d+)\s*Progressive:\s*(\d+)")
        .expect("valid regex")
});

/// Sample points for cropdetect (fractions of the duration), 24 frames each.
pub const CROP_SAMPLES: [f64; 5] = [0.2, 0.35, 0.5, 0.65, 0.8];
/// Sample points for idet, 200 frames each.
pub const IDET_SAMPLES: [f64; 2] = [0.3, 0.6];

/// Borders (left, top, right, bottom) from the last `crop=w:h:x:y` in cropdetect's output.
/// Right/bottom may be negative when cropdetect reports more than the frame.
pub fn parse_cropdetect(text: &str, width: u32, height: u32) -> Option<[i64; 4]> {
    let c = CROP.captures_iter(text).last()?;
    let n = |i: usize| c[i].parse::<i64>().unwrap_or(0);
    let (w, h, x, y) = (n(1), n(2), n(3), n(4));
    Some([x, y, width as i64 - x - w, height as i64 - y - h])
}

/// The least aggressive crop across all samples, so a dark scene can't eat real picture.
pub fn crop_from_samples(samples: &[[i64; 4]], width: u32, height: u32) -> Option<Crop> {
    if samples.len() < 3 {
        return None;
    }
    let side = |i: usize| {
        let min = samples.iter().map(|s| s[i]).min().unwrap_or(0);
        let even = (min.max(0) / 2 * 2) as u32;
        // ignore 1-3 px noise
        if even < 4 { 0 } else { even }
    };
    let c = Crop {
        l: side(0),
        t: side(1),
        r: side(2),
        b: side(3),
    };
    if c.l + c.r + c.t + c.b == 0 {
        return None;
    }
    // Never crop away more than 40% in either direction (protects mostly-black title cards)
    if (c.l + c.r) as f64 > width as f64 * 0.4 || (c.t + c.b) as f64 > height as f64 * 0.4 {
        return None;
    }
    Some(c)
}

/// TFF/BFF/progressive counts from the last idet summary in the output.
pub fn parse_idet(text: &str) -> Option<[u64; 3]> {
    let c = IDET.captures_iter(text).last()?;
    let n = |i: usize| c[i].parse::<u64>().unwrap_or(0);
    Some([n(1), n(2), n(3)])
}

/// Telecine shows up as mostly TFF/BFF frames.
pub fn interlace_from_counts(tff: u64, bff: u64, prog: u64) -> Option<InterlaceDetect> {
    let total = tff + bff + prog;
    if total < 50 {
        return None;
    }
    Some(InterlaceDetect {
        interlaced: (tff + bff) as f64 / total as f64 > 0.3,
        field: if bff > tff { "bff" } else { "tff" },
        detail: format!("idet TFF {tff}, BFF {bff}, progressive {prog}"),
    })
}

fn seek(duration: f64, frac: f64) -> String {
    fmt_inv((duration * frac).max(0.0), 3)
}

pub fn cropdetect_args(file: &Path, at: &str) -> Vec<OsString> {
    let mut v: Vec<OsString> = ["-hide_banner", "-nostats", "-ss", at, "-i"]
        .map(OsString::from)
        .into();
    v.push(file.into());
    v.extend(
        [
            "-an",
            "-sn",
            "-dn",
            "-vf",
            "cropdetect=limit=0.094:round=2:reset=0",
            "-frames:v",
            "24",
            "-f",
            "null",
            "-",
        ]
        .map(OsString::from),
    );
    v
}

pub fn idet_args(file: &Path, at: &str) -> Vec<OsString> {
    let mut v: Vec<OsString> = ["-hide_banner", "-nostats", "-ss", at, "-i"]
        .map(OsString::from)
        .into();
    v.push(file.into());
    v.extend(
        [
            "-an",
            "-sn",
            "-dn",
            "-vf",
            "idet",
            "-frames:v",
            "200",
            "-f",
            "null",
            "-",
        ]
        .map(OsString::from),
    );
    v
}

/// `analyze.py` arguments (after the interpreter).
pub fn analyze_py_args(
    script: &Path,
    file: &Path,
    ivtc: &str,
    field: &str,
    crop: &str,
    native: bool,
    noise: bool,
) -> Vec<OsString> {
    let mut v: Vec<OsString> = vec![script.into(), "--src".into(), file.into()];
    v.extend(["--ivtc", ivtc, "--field", field, "--crop", crop].map(OsString::from));
    if native {
        v.push("--native".into());
    }
    if noise {
        v.push("--noise".into());
    }
    v
}

/// Cache key: size, mtime, analysis version, which analyses ran, IVTC mode and crop.
pub fn cache_key(f: &VideoFile, native: bool, noise: bool, ivtc: &str, crop: &str) -> String {
    format!(
        "{}|{}|v{ANALYSIS_VERSION}|native{}|noise{}|{ivtc}|crop{crop}",
        f.size, f.mtime_ticks, native as u8, noise as u8
    )
}

/// Analysis with ffmpeg and `analyze.py`.
pub struct MediaAnalyzer<'a> {
    pub runner: &'a dyn Runner,
    pub ffmpeg: PathBuf,
    /// Python 3.12+ with VapourSynth; `None` makes source analysis fail with a warning.
    pub python: Option<PathBuf>,
    pub analyze_py: Option<PathBuf>,
    /// `<out>\logs\analysis`.
    pub cache_dir: PathBuf,
}

impl MediaAnalyzer<'_> {
    fn ffmpeg_text(&self, args: &[OsString]) -> String {
        self.runner
            .output(&self.ffmpeg, args)
            .map(|o| o.stderr + &o.stdout)
            .unwrap_or_default()
    }
}

impl Analyzer for MediaAnalyzer<'_> {
    fn black_borders(&self, file: &Path, src: &SourceInfo) -> Option<Crop> {
        if src.duration <= 0.0 {
            return None;
        }
        let samples: Vec<[i64; 4]> = CROP_SAMPLES
            .iter()
            .filter_map(|f| {
                parse_cropdetect(
                    &self.ffmpeg_text(&cropdetect_args(file, &seek(src.duration, *f))),
                    src.width,
                    src.height,
                )
            })
            .collect();
        crop_from_samples(&samples, src.width, src.height)
    }

    fn interlace(&self, file: &Path, src: &SourceInfo) -> Option<InterlaceDetect> {
        let mut sum = [0u64; 3];
        for f in IDET_SAMPLES {
            if let Some(c) = parse_idet(&self.ffmpeg_text(&idet_args(file, &seek(src.duration, f))))
            {
                for i in 0..3 {
                    sum[i] += c[i];
                }
            }
        }
        interlace_from_counts(sum[0], sum[1], sum[2])
    }

    fn source_analysis(
        &self,
        p: &FilePlan,
        native: bool,
        noise: bool,
        log: &dyn Log,
    ) -> Option<Value> {
        let stem = p.file.stem();
        let cache_file = self.cache_dir.join(format!("{stem}.json"));
        let err_file = self.cache_dir.join(format!("{stem}.stderr.log"));
        let key = cache_key(&p.file, native, noise, &p.ivtc_mode, &p.crop_arg());

        if let Some(cached) = std::fs::read_to_string(&cache_file)
            .ok()
            .and_then(|t| serde_json::from_str::<Value>(t.trim_start_matches('\u{feff}')).ok())
            && cached.get("key").and_then(Value::as_str) == Some(key.as_str())
        {
            return cached.get("result").cloned();
        }

        let fail = |why: &str| {
            log.warn(&format!(
                "Analysis failed for {} ({why}); using default settings for it.",
                p.file.name
            ));
            None
        };
        let (Some(python), Some(script)) = (&self.python, &self.analyze_py) else {
            return fail("Python 3.12+ with VapourSynth or analyze.py not found");
        };
        log.info(&format!(
            "  Analyzing {} (native resolution / noise; about a minute) ...",
            p.file.name
        ));
        let _ = std::fs::create_dir_all(&self.cache_dir);
        let args = analyze_py_args(
            script,
            &p.file.path,
            &p.ivtc_mode,
            &p.field_order,
            &p.crop_arg(),
            native,
            noise,
        );
        let out = match self.runner.output(python, &args) {
            Ok(o) => o,
            Err(e) => return fail(&e.to_string()),
        };
        let _ = std::fs::write(&err_file, &out.stderr);
        let result: Option<Value> = out
            .stdout
            .lines()
            .rfind(|l| !l.trim().is_empty())
            .and_then(|l| serde_json::from_str(l).ok());
        match result {
            Some(r) if r.get("error").is_none_or(Value::is_null) => {
                let entry = json!({ "key": key, "result": r });
                let _ = std::fs::write(
                    &cache_file,
                    serde_json::to_string_pretty(&entry).unwrap_or_default(),
                );
                log.info(&format!("    done in {}s", num(r.get("seconds"))));
                Some(r)
            }
            Some(r) => fail(&num(r.get("error"))),
            None => fail(&format!("no output; see {}", err_file.display())),
        }
    }
}

/// A JSON value as PowerShell would interpolate it into a string.
fn num(v: Option<&Value>) -> String {
    match v {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => match n.as_i64() {
            Some(i) => i.to_string(),
            None => n.as_f64().map(|f| f.to_string()).unwrap_or_default(),
        },
        Some(Value::Bool(b)) => if *b { "True" } else { "False" }.into(),
        Some(other) => other.to_string(),
    }
}

fn f64_of(v: Option<&Value>) -> f64 {
    v.and_then(Value::as_f64).unwrap_or(0.0)
}

fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

/// `analysis.native` when present and not null.
fn native_of(p: &FilePlan) -> Option<&Value> {
    p.analysis.as_ref()?.get("native").filter(|v| v.is_object())
}

fn confident_native(p: &FilePlan) -> Option<&Value> {
    native_of(p).filter(|n| truthy(n.get("confident")))
}

/// `analysis.noise` when it has a metric.
fn noise_metric(p: &FilePlan) -> Option<&Value> {
    p.analysis
        .as_ref()?
        .get("noise")?
        .get("metric")
        .filter(|m| !m.is_null())
}

fn native_setting(n: &Value) -> Native {
    let int = |k| {
        n.get(k)
            .and_then(Value::as_f64)
            .map_or(0, |f| f.round_ties_even() as u32)
    };
    let text = |k| n.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    Native {
        height: int("height"),
        width: int("width"),
        kernel: text("kernel"),
        b: f64_of(n.get("b")),
        c: f64_of(n.get("c")),
        taps: int("taps"),
        name: text("kernel_name"),
    }
}

fn noise_level(metric: f64) -> i32 {
    if metric < 0.8 {
        -1
    } else if metric < 1.6 {
        0
    } else {
        3
    }
}

/// Runs `analyze.py` where the plan asks for it, applies each episode's result, then the season
/// consensus for native resolution and denoise.
pub fn resolve_analysis(
    plans: &mut [FilePlan],
    mode: Analysis,
    analyzer: &dyn Analyzer,
    log: &dyn Log,
) {
    let up: Vec<usize> = (0..plans.len())
        .filter(|&i| {
            plans[i].route == Route::Upscale && (plans[i].auto_noise || plans[i].auto_native)
        })
        .collect();

    for &i in &up {
        let a = analyzer.source_analysis(&plans[i], plans[i].auto_native, plans[i].auto_noise, log);
        let p = &mut plans[i];
        p.analysis = a;
        if p.auto_noise
            && let Some(m) = noise_metric(p).cloned()
        {
            let level = p
                .analysis
                .as_ref()
                .and_then(|a| a.pointer("/noise/level"))
                .and_then(Value::as_f64)
                .unwrap_or(-1.0);
            p.noise = level.round_ties_even() as i32;
            p.noise_note = format!("episode noise {}", num(Some(&m)));
        }
        if p.auto_native
            && let Some(n) = native_of(p).cloned()
        {
            let prominence = f64_of(n.get("prominence"));
            let shown = |k| num(n.get(k));
            if truthy(n.get("confident")) && prominence >= NATIVE_STRONG {
                p.native = native_setting(&n);
                p.native_note = format!(
                    "episode: strong match, prominence {}, {}/{} frames",
                    shown("prominence"),
                    shown("agree"),
                    shown("frames")
                );
            } else if truthy(n.get("confident")) {
                p.native_note = format!(
                    "weak match {}p {} (prominence {} < {NATIVE_STRONG}); not used unless the season agrees",
                    shown("height"),
                    shown("kernel_name"),
                    shown("prominence")
                );
            } else {
                p.native_note = format!("not detected ({})", shown("reason"));
            }
        }
    }

    if mode != Analysis::Auto || up.len() < 3 {
        return;
    }

    // Native resolution: if 60%+ of episodes found the same match (weak or strong), use it for
    // every episode except one with a strong match of its own that differs (a special or recap).
    let nat: Vec<usize> = up
        .iter()
        .copied()
        .filter(|&i| plans[i].auto_native)
        .collect();
    let hits: Vec<usize> = nat
        .iter()
        .copied()
        .filter(|&i| confident_native(&plans[i]).is_some())
        .collect();
    if nat.len() >= 3 && !hits.is_empty() {
        let group_key = |i: usize| {
            let n = confident_native(&plans[i]).expect("hit");
            format!("{}|{}", num(n.get("height")), num(n.get("kernel_name"))).to_lowercase()
        };
        // Groups in first-seen order; the largest wins (ties: the first seen)
        let mut groups: Vec<(String, Vec<usize>)> = Vec::new();
        for &i in &hits {
            let k = group_key(i);
            match groups.iter_mut().find(|(g, _)| *g == k) {
                Some((_, members)) => members.push(i),
                None => groups.push((k, vec![i])),
            }
        }
        let top = groups.iter().fold(
            &groups[0],
            |best, g| if g.1.len() > best.1.len() { g } else { best },
        );
        let count = top.1.len();
        if count >= (nat.len() as f64 * 0.6).ceil() as usize {
            let reference = native_setting(confident_native(&plans[top.1[0]]).expect("hit"));
            log.info(&format!(
                "Season consensus: native {}x{} {} ({count}/{} episodes)",
                reference.width,
                reference.height,
                reference.name,
                nat.len()
            ));
            for &i in &nat {
                let p = &mut plans[i];
                let own = confident_native(p).cloned();
                let differs = own.as_ref().is_some_and(|o| {
                    native_setting(o).height != reference.height
                        || !num(o.get("kernel_name")).eq_ignore_ascii_case(&reference.name)
                });
                if differs
                    && own
                        .as_ref()
                        .is_some_and(|o| f64_of(o.get("prominence")) >= NATIVE_STRONG)
                {
                    p.native_note.push_str(&format!(
                        "; DIFFERS from season {}p {} -- kept episode result",
                        reference.height, reference.name
                    ));
                    log.warn(&format!(
                        "{}: native {}p {} differs from the season ({}p {}).",
                        p.file.name,
                        p.native.height,
                        p.native.name,
                        reference.height,
                        reference.name
                    ));
                } else {
                    p.native = reference.clone();
                    p.native_note = format!(
                        "season consensus ({count}/{} episodes); episode {}",
                        nat.len(),
                        p.native_note
                    );
                }
            }
        } else {
            log.info(&format!(
                "Season consensus: no native resolution shared by most episodes ({count}/{} agree); only strong per-episode matches are used.",
                nat.len()
            ));
        }
    }

    // Denoise: one level for the whole folder, from the median measured noise
    let noisy: Vec<usize> = up
        .iter()
        .copied()
        .filter(|&i| plans[i].auto_noise && noise_metric(&plans[i]).is_some())
        .collect();
    if noisy.len() >= 3 {
        let mut metrics: Vec<f64> = noisy
            .iter()
            .map(|&i| f64_of(noise_metric(&plans[i])))
            .collect();
        metrics.sort_by(f64::total_cmp);
        let median = metrics[(metrics.len() - 1) / 2];
        let level = noise_level(median);
        log.info(&format!(
            "Season consensus: denoise {} from median noise {} ({} episodes)",
            noise_label(level),
            fmt_n(median, 2),
            noisy.len()
        ));
        for &i in &noisy {
            let p = &mut plans[i];
            let episode = num(noise_metric(p));
            p.noise = level;
            p.noise_note = format!("season median {}; episode {episode}", fmt_n(median, 2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::MemoryLog;
    use crate::probe::SourceInfo;

    #[test]
    fn cropdetect_parsing_and_rules() {
        let out = "[Parsed_cropdetect_0 @ 0] x1:0 ... crop=1440:1080:240:0\n[Parsed_cropdetect_0 @ 0] crop=1436:1080:242:0";
        assert_eq!(parse_cropdetect(out, 1920, 1080), Some([242, 0, 242, 0]));
        assert_eq!(parse_cropdetect("nothing", 1920, 1080), None);

        let pillar = [[242, 0, 242, 0], [240, 0, 241, 0], [244, 2, 240, 3]];
        assert_eq!(
            crop_from_samples(&pillar, 1920, 1080),
            Some(Crop {
                l: 240,
                r: 240,
                t: 0,
                b: 0
            }),
            "minimum per side, even, < 4 ignored"
        );
        assert_eq!(
            crop_from_samples(&pillar[..2], 1920, 1080),
            None,
            "needs 3 samples"
        );
        assert_eq!(
            crop_from_samples(&[[2, 3, 1, 0]; 3], 1920, 1080),
            None,
            "only noise"
        );
        assert_eq!(
            crop_from_samples(&[[400, 0, 400, 0]; 3], 1920, 1080),
            None,
            "more than 40% removed"
        );
        assert_eq!(
            crop_from_samples(&[[8, 0, -2, 0]; 3], 720, 480),
            Some(Crop {
                l: 8,
                r: 0,
                t: 0,
                b: 0
            })
        );
    }

    #[test]
    fn idet_parsing_and_thresholds() {
        let out = "Single frame detection: TFF: 1 BFF: 0\n[Parsed_idet_0 @ 0] Multi frame detection: TFF:   12 BFF:   80 Progressive:  108 Undetermined: 0";
        assert_eq!(parse_idet(out), Some([12, 80, 108]));
        let d = interlace_from_counts(12, 80, 108).unwrap();
        assert!(d.interlaced);
        assert_eq!(d.field, "bff");
        assert_eq!(d.detail, "idet TFF 12, BFF 80, progressive 108");
        assert!(
            !interlace_from_counts(30, 0, 70).unwrap().interlaced,
            "exactly 0.3 is not interlaced"
        );
        assert!(
            interlace_from_counts(10, 10, 20).is_none(),
            "fewer than 50 frames"
        );
    }

    #[test]
    fn command_lines() {
        let f =
            Path::new(r"C:\k3wz\Sample Long Film The Ultimate Enthusiast's Collected Edition.mkv");
        let args: Vec<String> = cropdetect_args(f, &seek(3056.054, 0.2))
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        insta::assert_json_snapshot!(args);
        let args: Vec<String> = analyze_py_args(
            Path::new("analyze.py"),
            f,
            "decimate",
            "bff",
            "8,8,0,0",
            false,
            true,
        )
        .iter()
        .map(|a| a.to_string_lossy().into_owned())
        .collect();
        insta::assert_json_snapshot!(args);
        assert_eq!(seek(3056.054, 0.65), "1986.435");
    }

    #[test]
    fn cache_key_matches_the_script() {
        let f = VideoFile {
            path: "x.mkv".into(),
            name: "x.mkv".into(),
            size: 12_123_589_155,
            mtime_ticks: 638_613_088_506_174_730,
        };
        assert_eq!(
            cache_key(&f, true, true, "off", "0,0,0,0"),
            "12123589155|638613088506174730|v2|native1|noise1|off|crop0,0,0,0"
        );
    }

    /// Plans for the five Demo Show films with their real `analyze.py` results.
    fn demo_show_plans() -> Vec<FilePlan> {
        (1..=5)
            .map(|i| {
                let path = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(format!("tests/fixtures/analysis/demo_show{i}.json"));
                let cached: Value =
                    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
                plan(&format!("demo_show{i}.mkv"), cached["result"].clone())
            })
            .collect()
    }

    fn plan(name: &str, analysis: Value) -> FilePlan {
        let r = crate::settings::resolve(&serde_json::Map::new(), &[]).unwrap();
        let file = VideoFile {
            path: name.into(),
            name: name.into(),
            size: 1,
            mtime_ticks: 0,
        };
        let src = SourceInfo {
            codec: "h264".into(),
            width: 1920,
            height: 1080,
            frame_rate: 23.976,
            ..SourceInfo::default()
        };
        let mut p =
            crate::plan::plan_file(&r, crate::settings::Model::Cugan, file, src, &NoAnalysis);
        p.analysis = Some(analysis);
        p
    }

    /// Hands back each plan's preset analysis.
    struct Preset;
    impl Analyzer for Preset {
        fn black_borders(&self, _: &Path, _: &SourceInfo) -> Option<Crop> {
            None
        }
        fn interlace(&self, _: &Path, _: &SourceInfo) -> Option<InterlaceDetect> {
            None
        }
        fn source_analysis(&self, p: &FilePlan, _: bool, _: bool, _: &dyn Log) -> Option<Value> {
            p.analysis.clone()
        }
    }

    #[test]
    fn demo_show_season_has_no_native_consensus_and_no_denoise() {
        let mut plans = demo_show_plans();
        let log = MemoryLog::default();
        resolve_analysis(&mut plans, Analysis::Auto, &Preset, &log);
        let notes: Vec<(u32, &str, &str)> = plans
            .iter()
            .map(|p| {
                (
                    p.native.height,
                    p.native_note.as_str(),
                    p.noise_note.as_str(),
                )
            })
            .collect();
        assert_eq!(
            notes,
            [
                (
                    0,
                    "not detected (no clear dip (prominence 0.027 < 0.06))",
                    "season median 0.51; episode 0.594"
                ),
                (
                    0,
                    "weak match 945p bilinear (prominence 0.0661 < 0.12); not used unless the season agrees",
                    "season median 0.51; episode 0.51"
                ),
                (
                    0,
                    "weak match 945p bilinear (prominence 0.0661 < 0.12); not used unless the season agrees",
                    "season median 0.51; episode 0.515"
                ),
                (
                    0,
                    "not detected (no clear dip (prominence 0.059 < 0.06))",
                    "season median 0.51; episode 0.481"
                ),
                (
                    0,
                    "weak match 720p bicubic_catrom (prominence 0.0899 < 0.12); not used unless the season agrees",
                    "season median 0.51; episode 0.409"
                ),
            ]
        );
        assert!(plans.iter().all(|p| p.noise == -1));
        assert_eq!(
            *log.0.lock().unwrap(),
            [
                "Season consensus: no native resolution shared by most episodes (2/5 agree); only strong per-episode matches are used.",
                "Season consensus: denoise none from median noise 0.51 (5 episodes)",
            ]
        );
    }

    fn native(height: u32, kernel: &str, prominence: f64, confident: bool) -> Value {
        json!({ "native": { "height": height, "width": height * 16 / 9, "kernel": "bicubic", "b": 0.0, "c": 0.5, "taps": 3,
            "kernel_name": kernel, "prominence": prominence, "agree": 5, "frames": 5, "confident": confident, "reason": "r" },
            "noise": { "metric": 1.2, "level": 0 } })
    }

    #[test]
    fn season_agreement_lifts_weak_matches_but_keeps_a_strong_outlier() {
        let mut plans = vec![
            plan("e1.mkv", native(810, "bicubic", 0.08, true)),
            plan("e2.mkv", native(810, "bicubic", 0.07, true)),
            plan("e3.mkv", native(810, "bicubic", 0.20, true)),
            plan("e4.mkv", native(720, "bilinear", 0.15, true)),
            plan("e5.mkv", native(0, "", 0.01, false)),
        ];
        let log = MemoryLog::default();
        resolve_analysis(&mut plans, Analysis::Auto, &Preset, &log);
        let heights: Vec<u32> = plans.iter().map(|p| p.native.height).collect();
        assert_eq!(heights, [810, 810, 810, 720, 810]);
        assert_eq!(
            plans[0].native_note,
            "season consensus (3/5 episodes); episode weak match 810p bicubic (prominence 0.08 < 0.12); not used unless the season agrees"
        );
        assert_eq!(
            plans[3].native_note,
            "episode: strong match, prominence 0.15, 5/5 frames; DIFFERS from season 810p bicubic -- kept episode result"
        );
        assert_eq!(plans[1].noise, 0, "season median 1.2 is light");
        let log = log.0.lock().unwrap();
        assert_eq!(
            log[0],
            "Season consensus: native 1440x810 bicubic (3/5 episodes)"
        );
        assert_eq!(
            log[1],
            "WARNING: e4.mkv: native 720p bilinear differs from the season (810p bicubic)."
        );
    }

    #[test]
    fn episode_mode_and_small_folders_skip_consensus() {
        let mut plans = vec![
            plan("a.mkv", native(810, "bicubic", 0.13, true)),
            plan("b.mkv", native(810, "bicubic", 0.07, true)),
        ];
        resolve_analysis(&mut plans, Analysis::Auto, &Preset, &MemoryLog::default());
        assert_eq!((plans[0].native.height, plans[1].native.height), (810, 0));
        assert_eq!(plans[0].noise_note, "episode noise 1.2");

        let mut plans: Vec<FilePlan> = (0..3)
            .map(|i| plan(&format!("{i}.mkv"), native(810, "bicubic", 0.07, true)))
            .collect();
        resolve_analysis(
            &mut plans,
            Analysis::Episode,
            &Preset,
            &MemoryLog::default(),
        );
        assert!(plans.iter().all(|p| p.native.height == 0));
    }
}
