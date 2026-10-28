//! Source facts from `ffprobe` and `mkvmerge -J` (`Get-SourceVideoInfo`, the probes in
//! `Get-TrackPlan`). Parsing is separate from running the tools so fixtures can drive tests.

use std::ffi::OsString;
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::process::Runner;
use crate::util::parse_rational;
use crate::{Error, Result};

/// Mastering display metadata (rational strings as ffprobe prints them, e.g. `34000/50000`).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Mdm {
    pub red_x: Option<String>,
    pub red_y: Option<String>,
    pub green_x: Option<String>,
    pub green_y: Option<String>,
    pub blue_x: Option<String>,
    pub blue_y: Option<String>,
    pub white_point_x: Option<String>,
    pub white_point_y: Option<String>,
    pub max_luminance: Option<String>,
    pub min_luminance: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cll {
    pub max_content: i64,
    pub max_average: i64,
}

/// The first video stream.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceInfo {
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub sar: Option<String>,
    pub field_order: Option<String>,
    pub frame_rate: f64,
    pub duration: f64,
    pub primaries: Option<String>,
    pub transfer: Option<String>,
    pub matrix: Option<String>,
    pub mdm: Option<Mdm>,
    pub cll: Option<Cll>,
    pub dv_profile: Option<i64>,
    pub hdr10_plus: bool,
}

/// One mkvmerge track.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MkvTrack {
    pub id: i64,
    /// `video`, `audio` or `subtitles`.
    pub kind: String,
    pub codec: String,
    pub language: Option<String>,
    pub language_ietf: Option<String>,
    pub audio_channels: i64,
    pub track_name: Option<String>,
    pub forced: bool,
}

/// One ffprobe audio stream (used for the EAC3 companions).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioStream {
    pub index: i64,
    pub codec_name: String,
    pub profile: Option<String>,
    pub channels: i64,
    pub language: Option<String>,
    pub title: Option<String>,
}

/// Property lookup that ignores case, as PowerShell's does on parsed JSON.
fn get<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    let obj = v.as_object()?;
    obj.get(key)
        .or_else(|| {
            obj.iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, v)| v)
        })
        .filter(|v| !v.is_null())
}

fn get_str(v: &Value, key: &str) -> Option<String> {
    match get(v, key)? {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

fn get_int(v: &Value, key: &str) -> i64 {
    match get(v, key) {
        Some(Value::Number(n)) => n
            .as_i64()
            .unwrap_or_else(|| n.as_f64().map_or(0, crate::util::round_even)),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// PowerShell truthiness of a property: missing, null, false, 0 and "" are false.
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f != 0.0),
        Some(Value::String(s)) => !s.is_empty(),
        Some(_) => true,
    }
}

fn list<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    get(v, key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

/// Builds [`SourceInfo`] from `ffprobe -show_streams -show_format` (first video stream) and
/// `ffprobe -show_frames` for the first frame (HDR10 metadata usually lives on frames).
pub fn parse_source_info(streams_json: &str, frame_json: &str) -> Result<SourceInfo> {
    let probe: Value = serde_json::from_str(streams_json)?;
    let frames: Value = serde_json::from_str(frame_json).unwrap_or(Value::Null);
    let null = Value::Null;
    let stream = list(&probe, "streams").first().unwrap_or(&null);
    let frame = list(&frames, "frames").first().unwrap_or(&null);
    let side: Vec<&Value> = list(stream, "side_data_list")
        .iter()
        .chain(list(frame, "side_data_list"))
        .collect();
    let side_type = |v: &Value| get_str(v, "side_data_type").unwrap_or_default();

    let mdm = side
        .iter()
        .find(|s| side_type(s) == "Mastering display metadata" && truthy(get(s, "max_luminance")))
        .map(|s| Mdm {
            red_x: get_str(s, "red_x"),
            red_y: get_str(s, "red_y"),
            green_x: get_str(s, "green_x"),
            green_y: get_str(s, "green_y"),
            blue_x: get_str(s, "blue_x"),
            blue_y: get_str(s, "blue_y"),
            white_point_x: get_str(s, "white_point_x"),
            white_point_y: get_str(s, "white_point_y"),
            max_luminance: get_str(s, "max_luminance"),
            min_luminance: get_str(s, "min_luminance"),
        });
    let cll = side
        .iter()
        .find(|s| side_type(s) == "Content light level metadata")
        .map(|s| Cll {
            max_content: get_int(s, "max_content"),
            max_average: get_int(s, "max_average"),
        });
    let dv_profile = side
        .iter()
        .find(|s| side_type(s) == "DOVI configuration record")
        .map(|s| get_int(s, "dv_profile"));
    let hdr10_plus = side.iter().any(|s| {
        let t = side_type(s).to_lowercase();
        t.contains("hdr10+") || t.contains("2094-40")
    });

    let format = get(&probe, "format").unwrap_or(&null);
    Ok(SourceInfo {
        codec: get_str(stream, "codec_name").unwrap_or_default(),
        width: get_int(stream, "width").max(0) as u32,
        height: get_int(stream, "height").max(0) as u32,
        sar: get_str(stream, "sample_aspect_ratio"),
        field_order: get_str(stream, "field_order"),
        frame_rate: get_str(stream, "r_frame_rate")
            .and_then(|r| parse_rational(&r))
            .unwrap_or(0.0),
        duration: get_str(format, "duration")
            .and_then(|d| d.trim().parse().ok())
            .unwrap_or(0.0),
        primaries: get_str(stream, "color_primaries"),
        transfer: get_str(stream, "color_transfer"),
        matrix: get_str(stream, "color_space"),
        mdm,
        cll,
        dv_profile,
        hdr10_plus,
    })
}

/// Tracks from `mkvmerge -J`.
pub fn parse_mkvmerge(json: &str) -> Result<Vec<MkvTrack>> {
    let info: Value = serde_json::from_str(json)?;
    Ok(list(&info, "tracks")
        .iter()
        .map(|t| {
            let null = Value::Null;
            let p = get(t, "properties").unwrap_or(&null);
            MkvTrack {
                id: get_int(t, "id"),
                kind: get_str(t, "type").unwrap_or_default(),
                codec: get_str(t, "codec").unwrap_or_default(),
                language: get_str(p, "language"),
                language_ietf: get_str(p, "language_ietf"),
                audio_channels: get_int(p, "audio_channels"),
                track_name: get_str(p, "track_name"),
                forced: truthy(get(p, "forced_track")),
            }
        })
        .collect())
}

/// Audio streams from `ffprobe -show_streams -select_streams a`.
pub fn parse_audio_streams(json: &str) -> Result<Vec<AudioStream>> {
    let probe: Value = serde_json::from_str(json)?;
    Ok(list(&probe, "streams")
        .iter()
        .map(|s| {
            let null = Value::Null;
            let tags = get(s, "tags").unwrap_or(&null);
            AudioStream {
                index: get_int(s, "index"),
                codec_name: get_str(s, "codec_name").unwrap_or_default(),
                profile: get_str(s, "profile"),
                channels: get_int(s, "channels"),
                language: get_str(tags, "language"),
                title: get_str(tags, "title"),
            }
        })
        .collect())
}

fn os(args: &[&str], file: &Path) -> Vec<OsString> {
    let mut v: Vec<OsString> = args.iter().map(OsString::from).collect();
    v.push(file.into());
    v
}

pub fn source_info_args(file: &Path) -> Vec<OsString> {
    os(
        &[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_streams",
            "-show_format",
            "-of",
            "json",
        ],
        file,
    )
}

pub fn first_frame_args(file: &Path) -> Vec<OsString> {
    os(
        &[
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-read_intervals",
            "%+#1",
            "-show_frames",
            "-of",
            "json",
        ],
        file,
    )
}

pub fn audio_streams_args(file: &Path) -> Vec<OsString> {
    os(
        &[
            "-v",
            "quiet",
            "-print_format",
            "json",
            "-show_streams",
            "-select_streams",
            "a",
        ],
        file,
    )
}

pub fn mkvmerge_identify_args(file: &Path) -> Vec<OsString> {
    os(&["-J"], file)
}

fn run_json(
    runner: &dyn Runner,
    tool: &'static str,
    exe: &Path,
    args: Vec<OsString>,
    file: &Path,
) -> Result<String> {
    let out = runner.output(exe, &args)?;
    // mkvmerge -J exits 1 on warnings and still prints JSON.
    if out.stdout.trim().is_empty() {
        return Err(Error::Probe {
            tool,
            file: file.to_path_buf(),
            message: out.stderr.trim().to_string(),
        });
    }
    Ok(out.stdout)
}

pub fn probe_source(runner: &dyn Runner, ffprobe: &Path, file: &Path) -> Result<SourceInfo> {
    let streams = run_json(runner, "ffprobe", ffprobe, source_info_args(file), file)?;
    let frame =
        run_json(runner, "ffprobe", ffprobe, first_frame_args(file), file).unwrap_or_default();
    parse_source_info(&streams, &frame)
}

pub fn probe_tracks(runner: &dyn Runner, mkvmerge: &Path, file: &Path) -> Result<Vec<MkvTrack>> {
    parse_mkvmerge(&run_json(
        runner,
        "mkvmerge",
        mkvmerge,
        mkvmerge_identify_args(file),
        file,
    )?)
}

pub fn probe_audio(runner: &dyn Runner, ffprobe: &Path, file: &Path) -> Result<Vec<AudioStream>> {
    match run_json(runner, "ffprobe", ffprobe, audio_streams_args(file), file) {
        Ok(json) => parse_audio_streams(&json),
        // -v quiet prints nothing for a file without audio
        Err(Error::Probe { .. }) => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(rel: &str) -> String {
        std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures")
                .join(rel),
        )
        .unwrap()
    }

    #[test]
    fn demo_show_source_info() {
        let s = parse_source_info(
            &fixture("demo_show1/ffprobe-video.json"),
            &fixture("demo_show1/ffprobe-frame.json"),
        )
        .unwrap();
        assert_eq!(s.codec, "h264");
        assert_eq!((s.width, s.height), (1920, 1080));
        assert_eq!(s.sar.as_deref(), Some("1:1"));
        assert_eq!(s.field_order.as_deref(), Some("progressive"));
        assert!((s.frame_rate - 23.976).abs() < 0.001);
        assert!((s.duration - 3056.054).abs() < 1e-6);
        assert_eq!(s.matrix, None);
        assert!(s.mdm.is_none() && s.cll.is_none() && s.dv_profile.is_none() && !s.hdr10_plus);
    }

    #[test]
    fn hdr_side_data() {
        let streams = r#"{"streams":[{"codec_name":"hevc","width":3840,"height":2160,"r_frame_rate":"24000/1001",
            "color_transfer":"smpte2084","side_data_list":[{"side_data_type":"DOVI configuration record","dv_profile":8}]}],
            "format":{"duration":"100.0"}}"#;
        let frame = r#"{"frames":[{"side_data_list":[
            {"side_data_type":"Mastering display metadata","red_x":"34000/50000","max_luminance":"10000000/10000"},
            {"side_data_type":"Content light level metadata","max_content":1000,"max_average":400},
            {"side_data_type":"HDR Dynamic Metadata SMPTE2094-40 (HDR10+)"}]}]}"#;
        let s = parse_source_info(streams, frame).unwrap();
        assert_eq!(s.dv_profile, Some(8));
        assert!(s.hdr10_plus);
        assert_eq!(s.mdm.unwrap().red_x.as_deref(), Some("34000/50000"));
        assert_eq!(
            s.cll,
            Some(Cll {
                max_content: 1000,
                max_average: 400
            })
        );
    }

    #[test]
    fn mastering_data_without_luminance_is_ignored() {
        let frame = r#"{"frames":[{"side_data_list":[{"side_data_type":"Mastering display metadata","red_x":"1/2"}]}]}"#;
        let s = parse_source_info(r#"{"streams":[{}],"format":{}}"#, frame).unwrap();
        assert!(s.mdm.is_none());
        assert_eq!(s.frame_rate, 0.0);
    }

    #[test]
    fn demo_show_tracks() {
        let t = parse_mkvmerge(&fixture("demo_show1/mkvmerge.json")).unwrap();
        let summary: Vec<_> = t
            .iter()
            .map(|t| {
                (
                    t.id,
                    t.kind.as_str(),
                    t.codec.as_str(),
                    t.language.as_deref(),
                    t.forced,
                )
            })
            .collect();
        assert_eq!(
            summary,
            [
                (0, "video", "AVC/H.264/MPEG-4p10", Some("eng"), false),
                (1, "audio", "TrueHD", Some("eng"), false),
                (2, "audio", "TrueHD", Some("jpn"), false),
                (3, "subtitles", "HDMV PGS", Some("eng"), true),
                (4, "subtitles", "HDMV PGS", Some("eng"), false),
            ]
        );
        assert_eq!(t[3].track_name.as_deref(), Some("Signs and Songs"));
        assert_eq!(t[1].audio_channels, 6);
    }

    #[test]
    fn demo_show_audio_streams() {
        let a = parse_audio_streams(&fixture("demo_show1/ffprobe-audio.json")).unwrap();
        assert_eq!(a.len(), 2);
        assert_eq!(
            (a[0].index, a[0].codec_name.as_str(), a[0].channels),
            (1, "truehd", 6)
        );
        assert_eq!(a[1].language.as_deref(), Some("jpn"));
        assert_eq!(a[1].title.as_deref(), Some("Japanese - TrueHD 5.1"));
    }

    #[test]
    fn tags_are_case_insensitive() {
        let a = parse_audio_streams(
            r#"{"streams":[{"index":1,"codec_name":"dts","tags":{"LANGUAGE":"eng","TITLE":"x"}}]}"#,
        )
        .unwrap();
        assert_eq!(a[0].language.as_deref(), Some("eng"));
        assert_eq!(a[0].title.as_deref(), Some("x"));
    }
}
