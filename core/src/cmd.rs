//! Every external command line of a job, built by pure functions from the plan (script lines
//! 1377–1621 and `Save-Comparisons`). Running them is `pipeline`'s job.

use std::ffi::OsString;
use std::path::Path;

use crate::analyze::Crop;
use crate::batch::Batch;
use crate::color::VideoColor;
use crate::plan::FilePlan;
use crate::settings::{Model, Settings};
use crate::tools::topaz::topaz_chain;
use crate::tracks::{Defaults, Eac3, TrackPlan};
use crate::util::fmt_inv;

fn os<I, S>(items: I) -> Vec<OsString>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    items.into_iter().map(Into::into).collect()
}

fn joined(prefix: &str, path: &Path) -> OsString {
    let mut s = OsString::from(prefix);
    s.push(path);
    s
}

/// The `-a key=value` script arguments for `upscale.vpy`.
pub fn vpy_args(
    b: &Batch,
    p: &FilePlan,
    mlrt: &Path,
    mlrt_root: &Path,
    deband_on: bool,
) -> Vec<OsString> {
    let s = &b.resolved.settings;
    let topaz = s.use_topaz();
    let c = p.crop.unwrap_or_default();
    let n = &p.native;
    // The script passes its $Model variable, which stays empty on the Topaz route
    let model = if topaz {
        s.model.map_or("", Model::as_str)
    } else {
        b.model.as_str()
    };
    let mut pairs: Vec<OsString> = vec![
        joined("src=", &p.file.path),
        // Topaz upscales prepared frames; without an upscale they go straight to the encoder
        format!(
            "mode={}",
            if topaz || !p.upscale {
                "prep"
            } else {
                "upscale"
            }
        )
        .into(),
        joined("mlrt=", mlrt),
        joined("mlrt_root=", mlrt_root),
        format!("model={model}").into(),
        format!("backend={}", b.backend.as_str()).into(),
        format!("noise={}", p.noise).into(),
        format!("matrix={}", p.matrix_in).into(),
        format!("sar={}", p.sar).into(),
        format!("ivtc={}", p.ivtc_mode).into(),
        format!("field={}", p.field_order).into(),
        format!("crop_l={}", c.l).into(),
        format!("crop_r={}", c.r).into(),
        format!("crop_t={}", c.t).into(),
        format!("crop_b={}", c.b).into(),
        format!("native_height={}", n.height).into(),
        format!("native_kernel={}", n.kernel).into(),
        format!("native_b={}", fmt_inv(n.b, 4)).into(),
        format!("native_c={}", fmt_inv(n.c, 4)).into(),
        format!("native_taps={}", n.taps).into(),
        format!("deband={}", deband_on as u8).into(),
        format!("temporal={}", s.temporal as u8).into(),
        format!("temporal_strength={}", s.temporal_strength).into(),
        format!("height={}", s.height).into(),
        format!("start={}", p.start).into(),
        format!("frames={}", s.frames).into(),
    ];
    if b.model == Model::Onnx && !topaz {
        pairs.push(joined("onnx=", b.onnx.as_deref().unwrap_or(Path::new(""))));
    }
    pairs
        .into_iter()
        .flat_map(|kv| [OsString::from("-a"), kv])
        .collect()
}

/// The script arguments with `start=`/`frames=` replaced (one chunk of an encode).
pub fn with_range(script_args: &[OsString], start: i64, frames: u64) -> Vec<OsString> {
    script_args
        .iter()
        .map(|a| match a.to_str() {
            Some(s) if s.starts_with("start=") => format!("start={start}").into(),
            Some(s) if s.starts_with("frames=") => format!("frames={frames}").into(),
            _ => a.clone(),
        })
        .collect()
}

/// The script arguments in `prep` mode: decode, IVTC, crop and trim, no upscale.
pub fn with_prep_mode(script_args: &[OsString]) -> Vec<OsString> {
    script_args
        .iter()
        .map(|a| match a.to_str() {
            Some(s) if s.starts_with("mode=") => OsString::from("mode=prep"),
            _ => a.clone(),
        })
        .collect()
}

/// ffmpeg scene-cut detection on a y4m stream from stdin (scaled down: cuts don't need detail).
pub fn scdet_args() -> Vec<OsString> {
    os([
        "-hide_banner",
        "-nostats",
        "-f",
        "yuv4mpegpipe",
        "-i",
        "-",
        "-vf",
        "scale=640:-2,scdet=threshold=10",
        "-an",
        "-f",
        "null",
        "-",
    ])
}

/// mkvmerge appending the chunk streams into one video file.
pub fn join_args(out: &Path, chunks: &[std::path::PathBuf]) -> Vec<OsString> {
    let mut a = os(["-o"]);
    a.push(out.into());
    for (i, c) in chunks.iter().enumerate() {
        if i > 0 {
            a.push("+".into());
        }
        a.push(c.into());
    }
    a
}

/// `vspipe --info <args> upscale.vpy -`: the expected frame count.
pub fn vspipe_info_args(script_args: &[OsString], vpy: &Path) -> Vec<OsString> {
    let mut a = os(["--info"]);
    a.extend_from_slice(script_args);
    a.extend([vpy.into(), "-".into()]);
    a
}

/// `vspipe -c y4m <args> upscale.vpy -`: frames to stdout.
pub fn vspipe_y4m_args(script_args: &[OsString], vpy: &Path) -> Vec<OsString> {
    let mut a = os(["-c", "y4m"]);
    a.extend_from_slice(script_args);
    a.extend([vpy.into(), "-".into()]);
    a
}

/// Topaz's ffmpeg: y4m in, `tvai_up` at the output size, y4m out.
pub fn topaz_args(filter: &str, w: u32, h: u32) -> Vec<OsString> {
    os([
        "-hide_banner",
        "-nostdin",
        "-nostats",
        "-v",
        "warning",
        "-f",
        "yuv4mpegpipe",
        "-i",
        "-",
        "-sws_flags",
        "spline+accurate_rnd+full_chroma_int",
        "-vf",
        &topaz_chain(filter, w, h),
        "-f",
        "yuv4mpegpipe",
        "-strict",
        "-1",
        "-",
    ])
}

/// `key=value:key=value` → `(key, value)` pairs.
pub fn svt_param_pairs(params: &str) -> Vec<(String, String)> {
    params
        .split(':')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (k.trim().to_string(), v.trim().to_string())
        })
        .collect()
}

pub fn svt_app_args(s: &Settings, vc: &VideoColor, out: &Path) -> Vec<OsString> {
    let mut a = os([
        "-i".to_string(),
        "stdin".into(),
        "--crf".into(),
        s.crf.to_string(),
        "--preset".into(),
        s.preset.to_string(),
        "--input-depth".into(),
        "10".into(),
        "--color-primaries".into(),
        vc.prim_code.to_string(),
        "--transfer-characteristics".into(),
        vc.trc_code.to_string(),
        "--matrix-coefficients".into(),
        vc.mat_code.to_string(),
    ]);
    for (k, v) in svt_param_pairs(&s.svt_params) {
        a.push(format!("--{k}").into());
        a.push(v.into());
    }
    a.push("-b".into());
    a.push(out.into());
    a
}

pub fn ffmpeg_svt_args(s: &Settings, vc: &VideoColor, out: &Path) -> Vec<OsString> {
    let mut a = os([
        "-hide_banner".to_string(),
        "-v".into(),
        "error".into(),
        "-stats".into(),
        "-f".into(),
        "yuv4mpegpipe".into(),
        "-i".into(),
        "-".into(),
        "-c:v".into(),
        "libsvtav1".into(),
        "-crf".into(),
        s.crf.to_string(),
        "-preset".into(),
        s.preset.to_string(),
        "-pix_fmt".into(),
        "yuv420p10le".into(),
        "-color_primaries".into(),
        vc.prim_name.clone(),
        "-color_trc".into(),
        vc.trc_name.clone(),
        "-colorspace".into(),
        vc.mat_name.clone(),
    ]);
    if !s.svt_params.is_empty() {
        a.extend(os(["-svtav1-params", &s.svt_params]));
    }
    a.extend(os(["-y", "-f", "ivf"]));
    a.push(out.into());
    a
}

/// HandBrakeCLI for 4K sources and live action: carries HDR10, HDR10+ and Dolby Vision.
/// `clip` = (start, frames).
/// Where a HandBrake encode starts: a frame number (test clips), or a timestamp on HandBrake's
/// 90 kHz clock. `--start-at frame:N` makes HandBrake decode everything before N, so pieces of
/// a chunked encode start by timestamp, which it seeks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HbStart {
    Frame(i64),
    Pts(u64),
}

/// The 90 kHz start for frame `frame`, half a frame early, so millisecond rounding in the
/// container's timestamps can't move it onto the next frame.
pub fn hb_start_pts(frame: u64, fps: f64) -> u64 {
    ((frame as f64 - 0.5).max(0.0) / fps * 90_000.0).round() as u64
}

pub fn handbrake_args(
    src: &Path,
    out: &Path,
    s: &Settings,
    clip: Option<(HbStart, u64)>,
) -> Vec<OsString> {
    let mut a = os(["-i"]);
    a.push(src.into());
    a.push("-o".into());
    a.push(out.into());
    a.extend(os([
        "-f".to_string(),
        "av_mkv".into(),
        "-e".into(),
        "svt_av1_10bit".into(),
        "-q".into(),
        s.crf.to_string(),
        "--encoder-preset".into(),
        s.preset.to_string(),
    ]));
    if !s.tune.is_empty() {
        a.extend(os(["--encoder-tune", &s.tune]));
    }
    a.extend(os([
        "--hdr-dynamic-metadata".to_string(),
        "all".into(),
        "--crop-mode".into(),
        "none".into(),
        "--no-comb-detect".into(),
        "--no-decomb".into(),
        "--no-deinterlace".into(),
        "--no-detelecine".into(),
        "-a".into(),
        "none".into(),
        "-s".into(),
        "none".into(),
        "--no-markers".into(),
    ]));
    if !s.svt_params.is_empty() {
        a.extend(os(["-x", &s.svt_params]));
    }
    if let Some((start, frames)) = clip {
        match start {
            HbStart::Frame(0) | HbStart::Pts(0) => {}
            HbStart::Frame(n) => a.extend(os(["--start-at".to_string(), format!("frame:{n}")])),
            HbStart::Pts(t) => a.extend(os(["--start-at".to_string(), format!("pts:{t}")])),
        }
        a.extend(os(["--stop-at".to_string(), format!("frame:{frames}")]));
    }
    a
}

/// ffprobe listing of the first video stream's packets (timestamps and flags), to find the
/// source's keyframes: chunked HandBrake encodes cut there, where HandBrake starts exactly.
pub fn keyframe_args(file: &Path) -> Vec<OsString> {
    let mut a = os([
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-show_entries",
        "packet=pts_time,dts_time,flags",
        "-of",
        "csv=p=0",
    ]);
    a.push(file.into());
    a
}

/// Keyframe frame numbers from `keyframe_args` output (`pts,dts,flags` lines; VC-1 in MKV has
/// no pts, so the dts is used then), sorted and unique.
pub fn parse_keyframes(text: &str, fps: f64) -> Vec<u64> {
    let mut frames: Vec<u64> = text
        .lines()
        .filter_map(|l| {
            let mut f = l.split(',');
            let (pts, dts, flags) = (f.next()?, f.next()?, f.next()?);
            if !flags.starts_with('K') {
                return None;
            }
            let t: f64 = pts.parse().or_else(|_| dts.parse()).ok()?;
            Some((t * fps).round().max(0.0) as u64)
        })
        .filter(|f| *f > 0)
        .collect();
    frames.sort_unstable();
    frames.dedup();
    frames
}

/// ffprobe packet count of the first video stream.
pub fn count_packets_args(file: &Path) -> Vec<OsString> {
    let mut a = os([
        "-v",
        "error",
        "-select_streams",
        "v:0",
        "-count_packets",
        "-show_entries",
        "stream=nb_read_packets",
        "-of",
        "csv=p=0",
    ]);
    a.push(file.into());
    a
}

pub fn eac3_args(src: &Path, e: &Eac3, out: &Path) -> Vec<OsString> {
    let mut a = os(["-y", "-v", "error", "-stats", "-i"]);
    a.push(src.into());
    a.extend(os([
        "-map".to_string(),
        format!("0:{}", e.stream_index),
        "-vn".into(),
        "-c:a".into(),
        "eac3".into(),
        "-b:a".into(),
        e.bitrate.into(),
    ]));
    if e.channels > 6 {
        a.extend(os(["-ac", "6"]));
    }
    a.push(out.into());
    a
}

/// An EAC3 companion that was encoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewTrack {
    pub path: std::path::PathBuf,
    pub name: String,
    pub lang: String,
}

/// What the default-flag decisions log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DefaultsNote {
    Audio(String),
    NoAudioInLanguage,
    Subtitles(String),
    /// No subtitles on by default; the text is Auto's reason, if any.
    SubtitlesNone(Option<String>),
    NoSubtitlesInLanguage,
}

/// mkvmerge: the video stream (+ colour tags), then the source's kept audio/subs/chapters/
/// attachments, then the EAC3 companions. `tracks` is `None` for a video-only mux (HandBrake
/// output, test clips).
#[allow(clippy::too_many_arguments)]
pub fn mkvmerge_args(
    partial: &Path,
    video: &Path,
    passthrough: bool,
    vc: &VideoColor,
    source: &Path,
    tracks: Option<(&TrackPlan, &[NewTrack], &Defaults)>,
) -> (Vec<OsString>, Vec<DefaultsNote>) {
    let mut a = os(["--output"]);
    a.push(partial.into());
    let mut notes = Vec::new();
    if passthrough {
        // HandBrake already wrote colour, HDR and Dolby Vision info; take only its video track
        a.extend(os([
            "--no-audio",
            "--no-subtitles",
            "--no-chapters",
            "--no-attachments",
            "--no-global-tags",
        ]));
    } else {
        a.extend(os(vc.mkv_opts.iter().map(String::as_str)));
    }
    a.push(video.into());
    let Some((tp, new_tracks, d)) = tracks else {
        return (a, notes);
    };

    let flag = |a: &mut Vec<OsString>, id: i64, on: bool| {
        a.push("--default-track-flag".into());
        a.push(format!("{id}:{}", if on { "yes" } else { "no" }).into());
    };
    let reason = |r: &Option<String>| r.as_ref().map(|r| format!(" ({r})")).unwrap_or_default();
    let set_audio_flags = d.eac3.is_some() || d.audio_id.is_some();
    if set_audio_flags {
        let label = match d.eac3 {
            Some(i) => new_tracks[i].name.clone(),
            None => {
                let id = d.audio_id.unwrap_or_default();
                let name = tp
                    .kept_audio()
                    .find(|t| t.id == id)
                    .and_then(|t| t.new_name.clone())
                    .unwrap_or_default();
                format!("#{id} {name}")
            }
        };
        notes.push(DefaultsNote::Audio(format!(
            "{label}{}",
            reason(&d.audio_reason)
        )));
        for t in tp.kept_audio() {
            flag(&mut a, t.id, Some(t.id) == d.audio_id);
        }
    } else {
        notes.push(DefaultsNote::NoAudioInLanguage);
    }
    if let Some(sub) = d.sub_id {
        let name = d
            .sub_names
            .iter()
            .find(|(id, _)| *id == sub)
            .map(|(_, n)| n.clone())
            .or_else(|| {
                tp.sub_tracks
                    .iter()
                    .find(|t| t.id == sub)
                    .and_then(|t| t.track_name.clone())
            })
            .unwrap_or_default();
        notes.push(DefaultsNote::Subtitles(format!(
            "#{sub} {name}{}",
            reason(&d.sub_reason)
        )));
        for t in tp.kept_subs() {
            flag(&mut a, t.id, t.id == sub);
        }
    } else if d.subs_off {
        // No subtitles on by default; forced tracks keep the source's flags (players show them anyway)
        let off: Vec<i64> = tp.kept_subs().filter(|t| !t.forced).map(|t| t.id).collect();
        for id in &off {
            flag(&mut a, *id, false);
        }
        if !off.is_empty() {
            notes.push(DefaultsNote::SubtitlesNone(d.sub_reason.clone()));
        }
    } else if !tp.keep_subs.is_empty() {
        notes.push(DefaultsNote::NoSubtitlesInLanguage);
    }

    let ids = |v: &[i64]| v.iter().map(i64::to_string).collect::<Vec<_>>().join(",");
    if tp.keep_audio.is_empty() {
        a.push("--no-audio".into());
    } else {
        a.extend(os(["--audio-tracks".to_string(), ids(&tp.keep_audio)]));
    }
    if tp.keep_subs.is_empty() {
        a.push("--no-subtitles".into());
    } else {
        a.extend(os(["--subtitle-tracks".to_string(), ids(&tp.keep_subs)]));
    }
    // Name every kept source audio track "Language - Format Channels"
    for t in tp.kept_audio() {
        a.extend(os([
            "--track-name".to_string(),
            format!("{}:{}", t.id, t.new_name.as_deref().unwrap_or("")),
        ]));
    }
    for (id, name) in &d.sub_names {
        a.extend(os(["--track-name".to_string(), format!("{id}:{name}")]));
    }
    a.push("--no-video".into());
    a.push(source.into());

    for (i, t) in new_tracks.iter().enumerate() {
        a.extend(os([
            "--language".to_string(),
            format!("0:{}", t.lang),
            "--track-name".into(),
            format!("0:{}", t.name),
        ]));
        if set_audio_flags {
            a.extend(os([
                "--default-track-flag".to_string(),
                format!("0:{}", if d.eac3 == Some(i) { "yes" } else { "no" }),
            ]));
        }
        a.push(t.path.as_path().into());
    }
    (a, notes)
}

/// One comparison image: the source (cropped, scaled to the output size) left, the final file right.
#[allow(clippy::too_many_arguments)]
pub fn compare_args(
    source: &Path,
    src_at: f64,
    mkv: &Path,
    at: f64,
    crop: Option<Crop>,
    src_w: u32,
    src_h: u32,
    out_w: u32,
    out_h: u32,
    png: &Path,
) -> Vec<OsString> {
    let crop_filter = crop.map_or(String::new(), |c| {
        format!(
            "crop={}:{}:{}:{},",
            src_w - c.l - c.r,
            src_h - c.t - c.b,
            c.l,
            c.t
        )
    });
    let fc = format!(
        "[0:v]{crop_filter}scale={out_w}:{out_h}:flags=lanczos,setsar=1,format=rgb24[a];[1:v]setsar=1,format=rgb24[b];[a][b]hstack=inputs=2"
    );
    let mut a = os(["-v", "error", "-y", "-ss", &fmt_inv(src_at, 3), "-i"]);
    a.push(source.into());
    a.extend(os(["-ss", &fmt_inv(at, 3), "-i"]));
    a.push(mkv.into());
    a.extend(os([
        "-filter_complex",
        &fc,
        "-frames:v",
        "1",
        "-update",
        "1",
    ]));
    a.push(png.into());
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keyframes_from_packets() {
        // Sample Movie's VC-1 has no pts; 600.725 s is frame 14403 at 23.976 fps
        let text = "N/A,599.724000,K__\nN/A,599.765000,___\nN/A,600.725000,K__\n0.5,0.4,K__\n";
        assert_eq!(parse_keyframes(text, 24000.0 / 1001.0), [12, 14379, 14403]);
        let args: Vec<String> = keyframe_args(Path::new("/r/m.mkv"))
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
        assert_eq!(args.last().unwrap(), "/r/m.mkv");
        assert!(args.contains(&"packet=pts_time,dts_time,flags".to_string()));
    }
    use crate::color::video_color;
    use crate::files::VideoFile;
    use crate::gpu::GpuChoice;
    use crate::plan::plan_file;
    use crate::probe::{parse_audio_streams, parse_mkvmerge, parse_source_info};
    use crate::settings::{Backend, resolve};
    use crate::tracks::{default_tracks, plan_tracks};
    use serde_json::{Value, json};
    use std::path::PathBuf;

    fn fixture(rel: &str) -> String {
        std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(rel),
        )
        .unwrap()
    }

    fn strings(a: &[OsString]) -> Vec<String> {
        a.iter().map(|s| s.to_string_lossy().into_owned()).collect()
    }

    const SRC: &str = r"C:\k3wz\Demo Show 1 The Arrival (2012).mkv";

    /// The vs-mlrt anime profile unless the test picks another one.
    fn demo_show(options: Value) -> (Batch, FilePlan) {
        let mut options = options.as_object().cloned().unwrap_or_default();
        options.entry("Content").or_insert(json!("anime"));
        let r = resolve(&options, &[]).unwrap();
        let src = parse_source_info(
            &fixture("demo_show1/ffprobe-video.json"),
            &fixture("demo_show1/ffprobe-frame.json"),
        )
        .unwrap();
        let file = VideoFile {
            path: SRC.into(),
            name: "demo_show1.mkv".into(),
            size: 12_123_589_155,
            mtime_ticks: 0,
        };
        let mut p = plan_file(&r, Model::Cugan, file, src, &crate::analyze::NoAnalysis);
        let tracks = parse_mkvmerge(&fixture("demo_show1/mkvmerge.json")).unwrap();
        let audio = parse_audio_streams(&fixture("demo_show1/ffprobe-audio.json")).unwrap();
        p.tracks = Some(plan_tracks(&tracks, &audio, &r.settings.keep_languages));
        let label = if r.settings.use_topaz() {
            "Topaz prob-4"
        } else {
            "Real-CUGAN"
        };
        let b = Batch {
            input: PathBuf::from(r"C:\k3wz"),
            out_dir: PathBuf::from(r"C:\k3wz\upscaled"),
            model: Model::Cugan,
            onnx: None,
            model_label: label.into(),
            backend: Backend::Trt,
            gpu: GpuChoice::default(),
            deband_available: false,
            plans: vec![p.clone()],
            resolved: r,
        };
        (b, p)
    }

    #[test]
    fn vspipe_upscale_and_prep() {
        let (b, p) = demo_show(json!({}));
        let mlrt = Path::new(r"C:\w4lc\vs-mlrt");
        let args = vpy_args(&b, &p, mlrt, mlrt, false);
        insta::assert_json_snapshot!(
            "vspipe_y4m_anime",
            strings(&vspipe_y4m_args(&args, Path::new("upscale.vpy")))
        );
        assert_eq!(
            strings(&vspipe_info_args(&args, Path::new("u.vpy")))[..3],
            ["--info", "-a", &format!("src={SRC}")]
        );
        let (b, p) = demo_show(json!({ "Content": "cg" }));
        let args = strings(&vpy_args(&b, &p, mlrt, mlrt, false));
        assert!(args.contains(&"mode=prep".to_string()) && args.contains(&"model=".to_string()));
        // Anime AV1: prepared frames straight to the encoder
        let (b, p) = demo_show(json!({ "Content": "anime-av1" }));
        let args = strings(&vpy_args(&b, &p, mlrt, mlrt, false));
        assert!(args.contains(&"mode=prep".to_string()));
    }

    #[test]
    fn live_action_handbrake() {
        let (b, _) = demo_show(json!({ "Content": "live" }));
        insta::assert_json_snapshot!(
            "handbrake_live",
            strings(&handbrake_args(
                Path::new("in.mkv"),
                Path::new("out.video.mkv"),
                &b.resolved.settings,
                None
            ))
        );
    }

    #[test]
    fn encoders() {
        let (b, p) = demo_show(json!({}));
        let s = &b.resolved.settings;
        let vc = video_color(&p.src, false);
        let ivf = Path::new(r"C:\k3wz\upscaled\demo_show1.ivf");
        insta::assert_json_snapshot!("svt_app", strings(&svt_app_args(s, &vc, ivf)));
        insta::assert_json_snapshot!("ffmpeg_svt", strings(&ffmpeg_svt_args(s, &vc, ivf)));
        insta::assert_json_snapshot!("topaz", strings(&topaz_args(&s.topaz_filter, 3840, 2160)));
        insta::assert_json_snapshot!(
            "handbrake_clip",
            strings(&handbrake_args(
                Path::new("in.mkv"),
                Path::new("out.video.mkv"),
                s,
                Some((HbStart::Frame(1000), 720))
            ))
        );
    }

    #[test]
    fn handbrake_pieces_start_by_timestamp() {
        let (b, _) = demo_show(json!({ "Content": "live" }));
        let tail = |clip| {
            let a = strings(&handbrake_args(
                Path::new("in.mkv"),
                Path::new("p.mkv"),
                &b.resolved.settings,
                Some(clip),
            ));
            a[a.len() - 4..].to_vec()
        };
        // Frame 14403 at 23.976 fps: 600.725 s, started half a frame early
        let fps = 24000.0 / 1001.0;
        assert_eq!(hb_start_pts(14403, fps), 54_063_384);
        assert_eq!(
            tail((HbStart::Pts(hb_start_pts(14403, fps)), 14386)),
            ["--start-at", "pts:54063384", "--stop-at", "frame:14386"]
        );
        // The first piece needs no start
        let a = strings(&handbrake_args(
            Path::new("in.mkv"),
            Path::new("p.mkv"),
            &b.resolved.settings,
            Some((HbStart::Pts(0), 100)),
        ));
        assert!(!a.contains(&"--start-at".to_string()));
        assert_eq!(hb_start_pts(0, fps), 0);
    }

    #[test]
    fn audio_and_mux() {
        let (b, p) = demo_show(json!({}));
        let s = &b.resolved.settings;
        let tp = p.tracks.as_ref().unwrap();
        insta::assert_json_snapshot!(
            "eac3",
            strings(&eac3_args(
                Path::new(SRC),
                &tp.eac3[0],
                Path::new(r"C:\n9pd\a_stream1.eac3")
            ))
        );
        let new_tracks: Vec<NewTrack> = tp
            .eac3
            .iter()
            .map(|e| NewTrack {
                path: format!(r"C:\n9pd\a_stream{}.eac3", e.stream_index).into(),
                name: e.name.clone(),
                lang: e.lang_code.clone(),
            })
            .collect();
        let langs: Vec<&str> = new_tracks.iter().map(|t| t.lang.as_str()).collect();
        let d = default_tracks(
            tp,
            &langs,
            &s.default_audio_language,
            &s.default_subtitle_language,
        );
        let vc = video_color(&p.src, false);
        let (args, notes) = mkvmerge_args(
            Path::new(r"C:\k3wz\upscaled\demo_show1.partial.mkv"),
            Path::new(r"C:\k3wz\upscaled\demo_show1.ivf"),
            false,
            &vc,
            Path::new(SRC),
            Some((tp, &new_tracks, &d)),
        );
        insta::assert_json_snapshot!("mkvmerge_anime", strings(&args));
        assert_eq!(
            notes,
            [
                DefaultsNote::Audio("#2 Japanese - TrueHD 5.1 (jpn 5.1 vs eng 5.1)".into()),
                DefaultsNote::Subtitles(
                    "#4 English Subtitles (full subtitles for jpn audio)".into()
                )
            ]
        );

        // CG: English default, no subtitles on except forced ones
        let (b, p) = demo_show(json!({ "Content": "cg" }));
        let s = &b.resolved.settings;
        let tp = p.tracks.as_ref().unwrap();
        let d = default_tracks(
            tp,
            &langs,
            &s.default_audio_language,
            &s.default_subtitle_language,
        );
        let (args, notes) = mkvmerge_args(
            Path::new("p.mkv"),
            Path::new("v.ivf"),
            false,
            &vc,
            Path::new("s.mkv"),
            Some((tp, &new_tracks, &d)),
        );
        let args = strings(&args);
        assert!(
            args.windows(2)
                .any(|w| w == ["--default-track-flag", "4:no"])
        );
        assert!(
            !args.iter().any(|a| a.starts_with("3:")),
            "the forced track keeps its flag"
        );
        assert!(
            args.windows(2)
                .any(|w| w == ["--default-track-flag", "1:yes"]),
            "the English TrueHD track stays default"
        );
        assert!(
            !args
                .windows(2)
                .any(|w| w == ["--default-track-flag", "0:yes"])
        );
        assert_eq!(
            notes,
            [
                DefaultsNote::Audio("#1 English - TrueHD 5.1".into()),
                DefaultsNote::SubtitlesNone(None)
            ]
        );

        // HandBrake output: video only
        let (args, _) = mkvmerge_args(
            Path::new("p.mkv"),
            Path::new("v.video.mkv"),
            true,
            &vc,
            Path::new("s.mkv"),
            None,
        );
        assert_eq!(
            strings(&args),
            [
                "--output",
                "p.mkv",
                "--no-audio",
                "--no-subtitles",
                "--no-chapters",
                "--no-attachments",
                "--no-global-tags",
                "v.video.mkv"
            ]
        );
    }

    #[test]
    fn chunk_commands() {
        let (b, p) = demo_show(json!({ "ResumableEncoding": true }));
        let mlrt = Path::new("m");
        let args = vpy_args(&b, &p, mlrt, mlrt, false);
        let chunk = strings(&with_range(&args, 5760, 5616));
        assert!(
            chunk.contains(&"start=5760".to_string()) && chunk.contains(&"frames=5616".to_string())
        );
        assert_eq!(chunk.len(), args.len());
        assert!(strings(&with_prep_mode(&args)).contains(&"mode=prep".to_string()));
        assert_eq!(
            strings(&join_args(
                Path::new("v.mkv"),
                &["c0.ivf".into(), "c1.ivf".into(), "c2.ivf".into()]
            )),
            ["-o", "v.mkv", "c0.ivf", "+", "c1.ivf", "+", "c2.ivf"]
        );
    }

    #[test]
    fn verify_and_compare() {
        assert_eq!(
            strings(&count_packets_args(Path::new("x.ivf"))),
            [
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-count_packets",
                "-show_entries",
                "stream=nb_read_packets",
                "-of",
                "csv=p=0",
                "x.ivf"
            ]
        );
        let crop = Some(Crop {
            l: 240,
            r: 240,
            t: 0,
            b: 0,
        });
        insta::assert_json_snapshot!(
            "compare",
            strings(&compare_args(
                Path::new("src.mkv"),
                611.2108,
                Path::new("out.mkv"),
                611.2108,
                crop,
                1920,
                1080,
                2880,
                2160,
                Path::new(r"compare\x_01.png")
            ))
        );
    }
}
