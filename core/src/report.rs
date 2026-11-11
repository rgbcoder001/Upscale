//! The plan report (`Write-Plan`): the same lines the script's `-Plan` prints.

use crate::batch::Batch;
use crate::disk::{GB, space_needed};
use crate::plan::{FilePlan, Route};
use crate::settings::Model;
use crate::tracks::{Track, default_tracks};
use crate::util::fmt_n;

pub fn noise_label(level: i32) -> String {
    match level {
        -1 => "none".into(),
        0 => "light".into(),
        3 => "strong".into(),
        n => n.to_string(),
    }
}

/// `#3 HDMV PGS [eng] Signs and Songs`
fn track_label(t: &Track) -> String {
    let mut s = format!("#{} {} [{}]", t.id, t.codec, t.iso);
    if let Some(name) = t.track_name.as_deref().filter(|n| !n.is_empty()) {
        s.push(' ');
        s.push_str(name);
    }
    s
}

fn file_lines(b: &Batch, p: &FilePlan, lines: &mut Vec<String>) {
    file_lines_into(b, p, lines)
}

/// One file's section of the plan report.
pub fn file_report(b: &Batch, p: &FilePlan) -> Vec<String> {
    let mut lines = Vec::new();
    file_lines_into(b, p, &mut lines);
    if p.route != Route::Skip {
        lines.push(disk_line(
            space_needed(p.file.size, p.route),
            crate::disk::free_space(&b.out_dir),
        ));
    }
    lines
}

/// `  Disk:      about 26 GB needed (374 GB free on the output drive)`
pub fn disk_line(need: u64, free: Option<u64>) -> String {
    let free = free.map_or("free space unknown".to_string(), |f| {
        format!(
            "{} GB free on the output drive",
            fmt_n(f as f64 / GB as f64, 0)
        )
    });
    format!(
        "  Disk:      about {} GB needed ({free})",
        fmt_n(need as f64 / GB as f64, 0)
    )
}

fn file_lines_into(b: &Batch, p: &FilePlan, lines: &mut Vec<String>) {
    let s = &b.resolved.settings;
    let src = &p.src;
    lines.push(p.file.name.clone());
    lines.push(format!("  Source:    {}", p.file.path.display()));
    match p.route {
        Route::Skip => lines.push(format!("  Route:     skip ({})", p.note)),
        Route::Handbrake => lines.push(format!(
            "  Route:     encode only with HandBrake ({})",
            p.note
        )),
        Route::Upscale => {
            lines.push(format!(
                "  Route:     {}  {}x{} {} {} fps, SAR {}, matrix {}",
                if p.upscale { "upscale" } else { "encode only" },
                src.width,
                src.height,
                src.codec,
                fmt_n(src.frame_rate, 3),
                p.sar,
                p.matrix_in
            ));
            lines.push(format!("  Crop:      {}", p.crop_label()));
            lines.push(format!("  IVTC:      {}  [{}]", p.ivtc_mode, p.ivtc_note));
            if !p.upscale {
                lines.push("  Upscale:   none".into());
                let mut out = format!("  Output:    {}x{}", p.out_w, p.out_h);
                if let Some((w, h)) = p.display_dimensions() {
                    out.push_str(&format!(" (displayed {w}x{h})"));
                }
                lines.push(out);
            } else if s.use_topaz() {
                let model = s.topaz_filter.split(':').next().unwrap_or("");
                lines.push(format!(
                    "  Upscale:   Topaz {model} (denoise/detail handled by Topaz)"
                ));
                lines.push(format!("  Output:    {}x{}", p.out_w, p.out_h));
            } else {
                let nat = if p.native.height > 0 {
                    format!("{}p {}", p.native.height, p.native.name)
                } else {
                    "none".into()
                };
                lines.push(format!("  Native:    {nat}  [{}]", p.native_note));
                if b.model == Model::Cugan {
                    lines.push(format!(
                        "  Denoise:   {}  [{}]",
                        noise_label(p.noise),
                        p.noise_note
                    ));
                }
                let mut out = format!("  Output:    {}x{}", p.out_w, p.out_h);
                if s.temporal {
                    out.push_str(", temporal");
                }
                if s.deband && b.deband_available {
                    out.push_str(", deband");
                }
                lines.push(out);
            }
        }
    }

    let Some(tp) = p.tracks.as_ref().filter(|_| p.route != Route::Skip) else {
        return;
    };
    let langs: Vec<&str> = tp.eac3.iter().map(|e| e.lang_code.as_str()).collect();
    let d = default_tracks(
        tp,
        &langs,
        &s.default_audio_language,
        &s.default_subtitle_language,
    );

    let keep_a: Vec<String> = tp
        .kept_audio()
        .map(|t| format!("#{} '{}'", t.id, t.new_name.as_deref().unwrap_or("")))
        .collect();
    let drop_a: Vec<String> = tp.dropped_audio().map(track_label).collect();
    let mut audio = format!("  Audio:     keep {}", keep_a.join("; "));
    if !drop_a.is_empty() {
        audio.push_str(&format!(" | drop {}", drop_a.join("; ")));
    }
    lines.push(audio);
    for (i, e) in tp.eac3.iter().enumerate() {
        lines.push(format!(
            "  EAC3:      #{} {} {}ch -> '{}' {}{}",
            e.stream_index,
            e.codec,
            e.channels,
            e.name,
            e.bitrate,
            if d.eac3 == Some(i) { "  (default)" } else { "" }
        ));
    }
    if let Some(id) = d.audio_id {
        lines.push(format!("  Default A: source #{id}"));
    }
    if let Some(r) = &d.audio_reason {
        lines.push(format!("  Auto A:    {} ({r})", d.audio_lang));
    }
    let keep_s: Vec<String> = tp.kept_subs().map(track_label).collect();
    let drop_s: Vec<String> = tp.dropped_subs().map(track_label).collect();
    let mut subs = format!(
        "  Subs:      keep {}",
        if keep_s.is_empty() {
            "none".to_string()
        } else {
            keep_s.join("; ")
        }
    );
    if !drop_s.is_empty() {
        subs.push_str(&format!(" | drop {}", drop_s.join("; ")));
    }
    lines.push(subs);
    for (id, name) in &d.sub_names {
        lines.push(format!("  Rename S:  #{id} -> '{name}'"));
    }
    let why = d
        .sub_reason
        .as_ref()
        .map(|r| format!(" ({r})"))
        .unwrap_or_default();
    if let Some(id) = d.sub_id {
        let name = d
            .sub_names
            .iter()
            .find(|(i, _)| *i == id)
            .map(|(_, n)| n.as_str())
            .or_else(|| {
                tp.sub_tracks
                    .iter()
                    .find(|t| t.id == id)
                    .and_then(|t| t.track_name.as_deref())
            })
            .filter(|n| !n.is_empty());
        lines.push(format!(
            "  Default S: #{id}{}{why}",
            name.map(|n| format!(" {n}")).unwrap_or_default()
        ));
    } else if d.subs_off && !tp.keep_subs.is_empty() {
        lines.push(format!(
            "  Default S: none{why}, forced tracks keep their flag"
        ));
    }
}

/// The plan report. `free` is the free space on the output drive, if known.
pub fn render_plan(b: &Batch, free: Option<u64>) -> Vec<String> {
    let s = &b.resolved.settings;
    let mut lines = vec![
        format!(
            "Plan for {} file(s) -> {}   [content: {}]",
            b.plans.len(),
            b.out_dir.display(),
            s.content
        ),
        format!(
            "Model: {} | backend: {} | CRF {}, preset {} | {}",
            b.model_label,
            b.backend.as_str(),
            s.crf,
            s.preset,
            s.svt_params
        ),
        String::new(),
    ];
    for p in &b.plans {
        file_lines(b, p, &mut lines);
        lines.push(String::new());
    }
    let need: u64 = b
        .plans
        .iter()
        .filter(|p| p.route != Route::Skip)
        .map(|p| space_needed(p.file.size, p.route))
        .sum();
    let free = free.map_or("free space unknown".to_string(), |f| {
        format!("{} GB free", fmt_n(f as f64 / GB as f64, 0))
    });
    lines.push(format!(
        "Disk: about {} GB needed in total (peak per episode is much less); {free}",
        fmt_n(need as f64 / GB as f64, 0)
    ));
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disk_lines() {
        assert_eq!(
            disk_line(26 * GB, Some(374 * GB)),
            "  Disk:      about 26 GB needed (374 GB free on the output drive)"
        );
        assert_eq!(
            disk_line(10 * GB, None),
            "  Disk:      about 10 GB needed (free space unknown)"
        );
    }
}
