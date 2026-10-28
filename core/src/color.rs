//! Colour settings for one encode (`Get-VideoColor`): CICP codes for SVT-AV1 and mkvmerge,
//! FFmpeg names, HDR10 static metadata.

use serde::Serialize;

use crate::probe::SourceInfo;
use crate::util::{fmt_inv, parse_rational};

const PRIMARIES: &[(&str, u32)] = &[
    ("bt709", 1),
    ("bt470m", 4),
    ("bt470bg", 5),
    ("smpte170m", 6),
    ("smpte240m", 7),
    ("bt2020", 9),
];
const TRANSFER: &[(&str, u32)] = &[
    ("bt709", 1),
    ("smpte170m", 6),
    ("bt2020-10", 14),
    ("bt2020-12", 15),
    ("smpte2084", 16),
    ("arib-std-b67", 18),
];
const MATRIX: &[(&str, u32)] = &[
    ("bt709", 1),
    ("bt470bg", 5),
    ("smpte170m", 6),
    ("bt2020nc", 9),
    ("bt2020c", 10),
];

fn lookup(table: &[(&str, u32)], name: &str) -> Option<u32> {
    table.iter().find(|(k, _)| *k == name).map(|(_, v)| *v)
}

/// The source's name if the table knows it, else `bt709`.
fn pick(table: &[(&str, u32)], name: Option<&str>) -> (String, u32) {
    match name.map(str::to_lowercase) {
        Some(n) if lookup(table, &n).is_some() => {
            let code = lookup(table, &n).expect("checked");
            (n, code)
        }
        _ => ("bt709".into(), 1),
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoColor {
    pub prim_name: String,
    pub prim_code: u32,
    pub trc_name: String,
    pub trc_code: u32,
    pub mat_name: String,
    pub mat_code: u32,
    /// "SDR", "HDR10" or "HLG".
    pub label: &'static str,
    /// Extra SVT params (`mastering-display=…`, `content-light=…`).
    pub svt_extra: Vec<String>,
    /// mkvmerge options for the video track (track id 0).
    pub mkv_opts: Vec<String>,
}

/// Colour settings: the source's own (`passthrough`, 4K encode-only) or BT.709 (upscaled).
pub fn video_color(src: &SourceInfo, passthrough: bool) -> VideoColor {
    let ((prim_name, prim_code), (trc_name, trc_code), (mat_name, mat_code)) = if passthrough {
        (
            pick(PRIMARIES, src.primaries.as_deref()),
            pick(TRANSFER, src.transfer.as_deref()),
            pick(MATRIX, src.matrix.as_deref()),
        )
    } else {
        (
            ("bt709".into(), 1),
            ("bt709".into(), 1),
            ("bt709".into(), 1),
        )
    };
    let label = match trc_code {
        16 => "HDR10",
        18 => "HLG",
        _ => "SDR",
    };
    let mut svt_extra = Vec::new();
    let mut mkv_opts = vec![
        "--colour-primaries".to_string(),
        format!("0:{prim_code}"),
        "--colour-transfer-characteristics".to_string(),
        format!("0:{trc_code}"),
        "--colour-matrix-coefficients".to_string(),
        format!("0:{mat_code}"),
    ];

    if passthrough && let Some(d) = &src.mdm {
        let f =
            |s: &Option<String>| fmt_inv(s.as_deref().and_then(parse_rational).unwrap_or(0.0), 4);
        let (rx, ry, gx, gy) = (f(&d.red_x), f(&d.red_y), f(&d.green_x), f(&d.green_y));
        let (bx, by, wx, wy) = (
            f(&d.blue_x),
            f(&d.blue_y),
            f(&d.white_point_x),
            f(&d.white_point_y),
        );
        let (lmax, lmin) = (f(&d.max_luminance), f(&d.min_luminance));
        svt_extra.push(format!(
            "mastering-display=G({gx},{gy})B({bx},{by})R({rx},{ry})WP({wx},{wy})L({lmax},{lmin})"
        ));
        mkv_opts.extend([
            "--chromaticity-coordinates".to_string(),
            format!("0:{rx},{ry},{gx},{gy},{bx},{by}"),
            "--white-colour-coordinates".to_string(),
            format!("0:{wx},{wy}"),
            "--max-luminance".to_string(),
            format!("0:{lmax}"),
            "--min-luminance".to_string(),
            format!("0:{lmin}"),
        ]);
    }
    if passthrough
        && let Some(c) = &src.cll
        && c.max_content != 0
    {
        svt_extra.push(format!("content-light={},{}", c.max_content, c.max_average));
        mkv_opts.extend([
            "--max-content-light".to_string(),
            format!("0:{}", c.max_content),
            "--max-frame-light".to_string(),
            format!("0:{}", c.max_average),
        ]);
    }

    VideoColor {
        prim_name,
        prim_code,
        trc_name,
        trc_code,
        mat_name,
        mat_code,
        label,
        svt_extra,
        mkv_opts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::probe::{Cll, Mdm};

    fn hdr_source() -> SourceInfo {
        SourceInfo {
            primaries: Some("bt2020".into()),
            transfer: Some("smpte2084".into()),
            matrix: Some("bt2020nc".into()),
            mdm: Some(Mdm {
                red_x: Some("34000/50000".into()),
                red_y: Some("16000/50000".into()),
                green_x: Some("13250/50000".into()),
                green_y: Some("34500/50000".into()),
                blue_x: Some("7500/50000".into()),
                blue_y: Some("3000/50000".into()),
                white_point_x: Some("15635/50000".into()),
                white_point_y: Some("16450/50000".into()),
                max_luminance: Some("10000000/10000".into()),
                min_luminance: Some("50/10000".into()),
            }),
            cll: Some(Cll {
                max_content: 1000,
                max_average: 400,
            }),
            ..SourceInfo::default()
        }
    }

    #[test]
    fn upscale_is_always_bt709() {
        let c = video_color(&hdr_source(), false);
        assert_eq!((c.prim_code, c.trc_code, c.mat_code), (1, 1, 1));
        assert_eq!(c.label, "SDR");
        assert!(c.svt_extra.is_empty());
        assert_eq!(
            c.mkv_opts,
            [
                "--colour-primaries",
                "0:1",
                "--colour-transfer-characteristics",
                "0:1",
                "--colour-matrix-coefficients",
                "0:1"
            ]
        );
    }

    #[test]
    fn passthrough_keeps_hdr10_metadata() {
        let c = video_color(&hdr_source(), true);
        assert_eq!((c.prim_code, c.trc_code, c.mat_code), (9, 16, 9));
        assert_eq!(c.label, "HDR10");
        assert_eq!(
            c.svt_extra,
            [
                "mastering-display=G(0.265,0.69)B(0.15,0.06)R(0.68,0.32)WP(0.3127,0.329)L(1000,0.005)",
                "content-light=1000,400"
            ]
        );
        assert!(
            c.mkv_opts
                .contains(&"0:0.68,0.32,0.265,0.69,0.15,0.06".to_string())
        );
        assert!(c.mkv_opts.contains(&"0:400".to_string()));
    }

    #[test]
    fn unknown_tags_fall_back_to_bt709() {
        let src = SourceInfo {
            primaries: Some("film".into()),
            ..SourceInfo::default()
        };
        let c = video_color(&src, true);
        assert_eq!(c.prim_name, "bt709");
        assert_eq!(c.label, "SDR");
    }
}
