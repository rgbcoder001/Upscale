//! Every decision made before encoding a file (`Get-FileProfile`, `Get-CropArg`, `Get-CropLabel`).

use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

use crate::analyze::{Analyzer, Crop};
use crate::color::video_color;
use crate::files::VideoFile;
use crate::probe::SourceInfo;
use crate::settings::{Analysis, CropMode, IvtcSetting, Model, Resolved};
use crate::tracks::TrackPlan;
use crate::util::round_even;

static SAR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[1-9]\d*:[1-9]\d*$").expect("valid regex"));

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Route {
    /// Source below the target height: upscale (vs-mlrt or Topaz) and encode.
    Upscale,
    /// Source at the target height and not AV1: encode only (keeps HDR10/HDR10+/DV).
    Handbrake,
    /// Already AV1 at the target height.
    Skip,
}

/// Descale settings for the native-resolution step (`native_height` 0 = off).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Native {
    pub height: u32,
    pub width: u32,
    pub kernel: String,
    pub b: f64,
    pub c: f64,
    pub taps: u32,
    /// Shown in the plan, e.g. `bicubic_catrom`.
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePlan {
    pub file: VideoFile,
    pub src: SourceInfo,
    pub route: Route,
    /// Why a 4K file is skipped or encoded only.
    pub note: String,
    pub matrix_in: String,
    pub sar: String,
    /// `off`, `decimate` (NTSC) or `match` (PAL).
    pub ivtc_mode: String,
    pub ivtc_note: String,
    pub field_order: String,
    pub crop: Option<Crop>,
    pub auto_noise: bool,
    pub auto_native: bool,
    /// `analyze.py` result (milestone 1B).
    pub analysis: Option<serde_json::Value>,
    pub noise: i32,
    pub noise_note: String,
    pub native: Native,
    pub native_note: String,
    pub out_w: u32,
    pub out_h: u32,
    /// First frame of a test clip.
    pub start: i64,
    pub tracks: Option<TrackPlan>,
}

impl FilePlan {
    /// `L,R,T,B` for `analyze.py --crop`.
    pub fn crop_arg(&self) -> String {
        let c = self.crop.unwrap_or_default();
        format!("{},{},{},{}", c.l, c.r, c.t, c.b)
    }

    /// `pillarbox L240 R240 T0 B0 -> 1440x1080`, or `none`.
    pub fn crop_label(&self) -> String {
        let Some(c) = self.crop else {
            return "none".into();
        };
        let kind = if c.l + c.r > 0 && c.t + c.b == 0 {
            "pillarbox"
        } else if c.t + c.b > 0 && c.l + c.r == 0 {
            "letterbox"
        } else {
            "borders"
        };
        let w = self.src.width.saturating_sub(c.l + c.r);
        let h = self.src.height.saturating_sub(c.t + c.b);
        format!("{kind} L{} R{} T{} B{} -> {w}x{h}", c.l, c.r, c.t, c.b)
    }
}

fn is_ntsc(fps: f64) -> bool {
    (fps - 29.97).abs() < 0.05
}

fn is_pal(fps: f64) -> bool {
    (fps - 25.0).abs() < 0.05
}

/// Plans one file. `model` is the resolved upscaler model (ignored on the Topaz route).
pub fn plan_file(
    r: &Resolved,
    model: Model,
    file: VideoFile,
    src: SourceInfo,
    analyzer: &dyn Analyzer,
) -> FilePlan {
    let s = &r.settings;
    let use_topaz = s.use_topaz();
    let (frames, duration) = (s.frames as f64, src.duration);
    let middle = |fps: f64| round_even((duration * fps / 2.0 - frames / 2.0).max(0.0));

    let mut p = FilePlan {
        file,
        route: Route::Upscale,
        note: String::new(),
        matrix_in: "709".into(),
        sar: "1:1".into(),
        ivtc_mode: "off".into(),
        ivtc_note: String::new(),
        field_order: "tff".into(),
        crop: None,
        auto_noise: false,
        auto_native: false,
        analysis: None,
        noise: s.noise,
        noise_note: "default".into(),
        native: Native {
            height: s.native_height,
            width: 0,
            kernel: s.native_kernel.clone(),
            b: s.native_b,
            c: s.native_c,
            taps: s.native_taps,
            name: s.native_kernel.clone(),
        },
        native_note: if s.native_height > 0 {
            "set by you"
        } else {
            "off"
        }
        .into(),
        out_w: 0,
        out_h: s.height,
        start: s.start,
        tracks: None,
        src,
    };
    // Test clip in the middle of the episode when Start is -1
    if s.start < 0 {
        p.start = middle(p.src.frame_rate);
    }

    let src = &p.src;
    if src.height >= s.height {
        let mut label = video_color(src, true).label.to_string();
        if let Some(dv) = src.dv_profile {
            label.push_str(&format!(" + Dolby Vision {dv}"));
        }
        if src.hdr10_plus {
            label.push_str(" + HDR10+");
        }
        if src.codec.eq_ignore_ascii_case("av1") {
            p.route = Route::Skip;
            p.note = format!("already {}p AV1", src.height);
        } else {
            p.route = Route::Handbrake;
            p.note = format!("{}p {} {label}", src.height, src.codec);
        }
        return p;
    }

    p.matrix_in = if !s.matrix.is_empty() {
        s.matrix.clone()
    } else {
        match src.matrix.as_deref().map(str::to_lowercase).as_deref() {
            Some("bt709") => "709".into(),
            Some("smpte170m") => "170m".into(),
            Some("bt470bg") => "470bg".into(),
            _ if src.height < 720 => "170m".into(),
            _ => "709".into(),
        }
    };
    p.sar = match src.sar.as_deref() {
        Some(sar) if SAR.is_match(sar) => sar.to_string(),
        _ => "1:1".into(),
    };
    let flagged = src
        .field_order
        .as_deref()
        .map(str::to_lowercase)
        .unwrap_or_default();
    let mut interlaced = matches!(flagged.as_str(), "tt" | "bb" | "tb" | "bt");
    p.field_order = if matches!(flagged.as_str(), "bb" | "bt") {
        "bff"
    } else {
        "tff"
    }
    .into();
    p.ivtc_note = if interlaced {
        format!("flagged {}", src.field_order.as_deref().unwrap_or(""))
    } else {
        "progressive".into()
    };
    let ntsc = is_ntsc(src.frame_rate);
    let pal = is_pal(src.frame_rate);
    // Only 29.97 / 25 fps material can be telecined or interlaced anime; check the picture
    if s.ivtc == IvtcSetting::Auto
        && !interlaced
        && (ntsc || pal)
        && let Some(det) = analyzer.interlace(&p.file.path, src)
    {
        p.ivtc_note = format!("flagged progressive; {}", det.detail);
        if det.interlaced {
            interlaced = true;
            p.field_order = det.field.into();
        }
    }
    let ntsc_mode = if ntsc { "decimate" } else { "match" };
    p.ivtc_mode = match s.ivtc {
        IvtcSetting::Off => "off",
        IvtcSetting::On => ntsc_mode,
        IvtcSetting::Auto if interlaced => ntsc_mode,
        IvtcSetting::Auto => "off",
    }
    .into();

    if s.crop == CropMode::Auto {
        p.crop = analyzer.black_borders(&p.file.path, src);
    }
    let c = p.crop.unwrap_or_default();
    let cw = src.width.saturating_sub(c.l + c.r) as f64;
    let ch = src.height.saturating_sub(c.t + c.b);
    let (sar_n, sar_d) = p.sar.split_once(':').expect("validated SAR");
    let sar = sar_n.parse::<f64>().unwrap_or(1.0) / sar_d.parse::<f64>().unwrap_or(1.0);
    if ch > 0 {
        p.out_w = (round_even(cw * sar * s.height as f64 / ch as f64 / 2.0) * 2).max(0) as u32;
    }

    // After decimation there are 4 frames for every 5 in the source
    if s.start < 0 && p.ivtc_mode == "decimate" {
        p.start = middle(src.frame_rate * 0.8);
    }

    // Topaz does its own denoise and works from the full frame: no analysis on that route
    let analysing = s.analysis != Analysis::Off && !use_topaz;
    p.auto_noise = analysing && model == Model::Cugan && !r.is_explicit("Noise");
    // Native-resolution detection only makes sense for progressive 1080p (anime Blu-rays)
    p.auto_native = analysing
        && !r.is_explicit("NativeHeight")
        && ch >= 700
        && (1000..=1100).contains(&src.height)
        && p.ivtc_mode == "off";
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analyze::{InterlaceDetect, NoAnalysis};
    use crate::settings::resolve;
    use serde_json::{Value, json};
    use std::path::{Path, PathBuf};

    /// The vs-mlrt anime profile unless the test picks another one.
    fn resolved(opts: Value) -> Resolved {
        let mut opts = opts.as_object().cloned().unwrap_or_default();
        opts.entry("Content").or_insert(json!("anime"));
        resolve(&opts, &[]).unwrap()
    }

    fn file() -> VideoFile {
        VideoFile {
            path: PathBuf::from("ep.mkv"),
            name: "ep.mkv".into(),
            size: 10,
            mtime_ticks: 0,
        }
    }

    fn src(w: u32, h: u32, codec: &str, fps: f64) -> SourceInfo {
        SourceInfo {
            codec: codec.into(),
            width: w,
            height: h,
            sar: Some("1:1".into()),
            field_order: Some("progressive".into()),
            frame_rate: fps,
            duration: 1440.0,
            ..SourceInfo::default()
        }
    }

    struct Fixed(Option<Crop>, Option<InterlaceDetect>);
    impl Analyzer for Fixed {
        fn black_borders(&self, _: &Path, _: &SourceInfo) -> Option<Crop> {
            self.0
        }
        fn interlace(&self, _: &Path, _: &SourceInfo) -> Option<InterlaceDetect> {
            self.1.clone()
        }
    }

    #[test]
    fn bluray_1080p_upscales_with_analysis() {
        let p = plan_file(
            &resolved(json!({})),
            Model::Cugan,
            file(),
            src(1920, 1080, "h264", 23.976),
            &NoAnalysis,
        );
        assert_eq!(p.route, Route::Upscale);
        assert_eq!((p.out_w, p.out_h), (3840, 2160));
        assert_eq!(
            (
                p.matrix_in.as_str(),
                p.ivtc_mode.as_str(),
                p.ivtc_note.as_str()
            ),
            ("709", "off", "progressive")
        );
        assert!(p.auto_noise && p.auto_native);
        assert_eq!(p.crop_label(), "none");
        assert_eq!(p.native_note, "off");
    }

    #[test]
    fn four_k_routes() {
        let r = resolved(json!({}));
        let p = plan_file(
            &r,
            Model::Cugan,
            file(),
            src(3840, 2160, "av1", 24.0),
            &NoAnalysis,
        );
        assert_eq!(
            (p.route, p.note.as_str()),
            (Route::Skip, "already 2160p AV1")
        );

        let mut hdr = src(3840, 2160, "hevc", 23.976);
        hdr.transfer = Some("smpte2084".into());
        hdr.dv_profile = Some(8);
        hdr.hdr10_plus = true;
        let p = plan_file(&r, Model::Cugan, file(), hdr, &NoAnalysis);
        assert_eq!(
            (p.route, p.note.as_str()),
            (
                Route::Handbrake,
                "2160p hevc HDR10 + Dolby Vision 8 + HDR10+"
            )
        );
    }

    #[test]
    fn dvd_anamorphic_telecine() {
        let mut dvd = src(720, 480, "mpeg2video", 30000.0 / 1001.0);
        dvd.sar = Some("32:27".into());
        let det = InterlaceDetect {
            interlaced: true,
            field: "bff",
            detail: "idet TFF 10, BFF 300, progressive 90".into(),
        };
        let crop = Crop {
            l: 8,
            r: 8,
            t: 0,
            b: 0,
        };
        let r = resolved(json!({ "Start": -1, "Frames": 720 }));
        let p = plan_file(&r, Model::Cugan, file(), dvd, &Fixed(Some(crop), Some(det)));
        assert_eq!(p.matrix_in, "170m", "SD without a matrix tag is BT.601");
        assert_eq!(
            (p.ivtc_mode.as_str(), p.field_order.as_str()),
            ("decimate", "bff")
        );
        assert_eq!(
            p.ivtc_note,
            "flagged progressive; idet TFF 10, BFF 300, progressive 90"
        );
        // 704 * 32/27 * 2160 / 480 = 3754.67 -> /2 = 1877.33 -> 1877 * 2
        assert_eq!(p.out_w, 3754);
        assert_eq!(p.crop_label(), "pillarbox L8 R8 T0 B0 -> 704x480");
        assert_eq!(p.crop_arg(), "8,8,0,0");
        // middle of 1440 s at 29.97 * 0.8 fps, minus half the clip
        assert_eq!(
            p.start,
            round_even(1440.0 * (30000.0 / 1001.0) * 0.8 / 2.0 - 360.0)
        );
        assert!(p.auto_noise && !p.auto_native);
    }

    #[test]
    fn ivtc_settings_and_flags() {
        let mut pal = src(720, 576, "mpeg2video", 25.0);
        pal.field_order = Some("tb".into());
        let p = plan_file(
            &resolved(json!({})),
            Model::Cugan,
            file(),
            pal.clone(),
            &NoAnalysis,
        );
        assert_eq!(
            (p.ivtc_mode.as_str(), p.ivtc_note.as_str()),
            ("match", "flagged tb")
        );
        let p = plan_file(
            &resolved(json!({ "Ivtc": "off" })),
            Model::Cugan,
            file(),
            pal,
            &NoAnalysis,
        );
        assert_eq!(p.ivtc_mode, "off");
        let p = plan_file(
            &resolved(json!({ "Ivtc": "on" })),
            Model::Cugan,
            file(),
            src(720, 480, "x", 29.97),
            &NoAnalysis,
        );
        assert_eq!(p.ivtc_mode, "decimate");
    }

    #[test]
    fn explicit_settings_switch_analysis_off() {
        let s = src(1920, 1080, "h264", 23.976);
        let p = plan_file(
            &resolved(json!({ "Noise": 0, "NativeHeight": 810 })),
            Model::Cugan,
            file(),
            s.clone(),
            &NoAnalysis,
        );
        assert!(!p.auto_noise && !p.auto_native);
        assert_eq!(
            (p.native.height, p.native_note.as_str()),
            (810, "set by you")
        );
        let p = plan_file(
            &resolved(json!({ "Content": "cg" })),
            Model::Cugan,
            file(),
            s.clone(),
            &NoAnalysis,
        );
        assert!(!p.auto_noise && !p.auto_native);
        let p = plan_file(&resolved(json!({})), Model::Onnx, file(), s, &NoAnalysis);
        assert!(
            !p.auto_noise && p.auto_native,
            "denoise analysis is Real-CUGAN only"
        );
    }
}
