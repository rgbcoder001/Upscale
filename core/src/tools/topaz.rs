//! Topaz Video AI 7.x for CG films (`Initialize-Topaz`, `Get-TopazChain`, `Test-Topaz`).
//! Uses the ffmpeg.exe that ships with Video AI 7.x and its `tvai_up` filter. The newer "Topaz
//! Video" app (1.1+) removed this command-line ffmpeg, so stay on Video AI 7.x for this route.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Serialize;

use super::{env_dir, exe};
use crate::log::Log;
use crate::process::Runner;
use crate::{Error, Result};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Topaz {
    pub ffmpeg: PathBuf,
    /// The app's product version, or `unknown version`.
    pub version: String,
    /// `TVAI_MODEL_DIR` / `TVAI_MODEL_DATA_DIR` for every Topaz process (the app sets these for
    /// its own exports; a command-line run has to set them itself).
    pub env: Vec<(String, String)>,
    pub signed_in: bool,
}

impl Topaz {
    pub fn env_os(&self) -> Vec<(OsString, OsString)> {
        self.env.iter().map(|(k, v)| (k.into(), v.into())).collect()
    }
}

/// Finds Topaz in `dir`; warns when the sign-in file is missing.
pub fn init_topaz(runner: &dyn Runner, dir: &Path, log: &dyn Log) -> Result<Topaz> {
    let ffmpeg = dir.join(exe("ffmpeg"));
    if !ffmpeg.is_file() {
        return Err(Error::Tool(format!(
            "Topaz Video AI's ffmpeg.exe was not found in '{}'. CG mode upscales with Topaz Video AI 7.x. \
             Install it there, set \"TopazPath\" in upscale-settings.json, or set Upscaler to vs to use the built-in upscaler.",
            dir.display()
        )));
    }
    let default_models = env_dir(
        "ProgramData",
        &["Topaz Labs LLC", "Topaz Video AI", "models"],
    )
    .unwrap_or_default();
    let var = |name: &str| {
        std::env::var_os(name)
            .map(PathBuf::from)
            .filter(|p| !p.as_os_str().is_empty())
    };
    let model_dir = var("TVAI_MODEL_DIR").unwrap_or_else(|| default_models.clone());
    let data_dir = var("TVAI_MODEL_DATA_DIR").unwrap_or(default_models);
    let signed_in = model_dir.join("auth.tpz").is_file();
    if !signed_in {
        log.warn(&format!(
            "Topaz sign-in file not found in {}. If the self-test fails, open Topaz Video AI once and sign in.",
            model_dir.display()
        ));
    }
    Ok(Topaz {
        version: product_version(runner, &dir.join("Topaz Video AI.exe"))
            .unwrap_or_else(|| "unknown version".into()),
        ffmpeg,
        env: vec![
            (
                "TVAI_MODEL_DIR".into(),
                model_dir.to_string_lossy().into_owned(),
            ),
            (
                "TVAI_MODEL_DATA_DIR".into(),
                data_dir.to_string_lossy().into_owned(),
            ),
        ],
        signed_in,
    })
}

/// A Windows executable's ProductVersion. The path goes in through an environment variable,
/// so it is never part of a command string.
fn product_version(runner: &dyn Runner, exe: &Path) -> Option<String> {
    if !cfg!(windows) || !exe.is_file() {
        return None;
    }
    let ps = super::find_exe("powershell", &[])?;
    let args: Vec<OsString> = [
        "-NoProfile",
        "-NonInteractive",
        "-Command",
        "(Get-Item -LiteralPath $env:UPSCALE_EXE).VersionInfo.ProductVersion",
    ]
    .map(OsString::from)
    .into();
    let out = runner
        .output_env(&ps, &args, &[("UPSCALE_EXE".into(), exe.into())])
        .ok()?;
    Some(out.stdout.trim().to_string()).filter(|v| !v.is_empty())
}

/// y4m carries no colour info: tag the frames BT.709 (upscale.vpy prep converts to it), run
/// the model at the output size, then resize exactly to it as the app does (also corrects
/// anamorphic sources, since W x H is the display size). `filter` is the `tvai_up` settings;
/// the script left out the `tvai_up=` name, which ffmpeg rejects (see CLAUDE.md).
pub fn topaz_chain(filter: &str, w: u32, h: u32) -> String {
    let settings = filter.strip_prefix("tvai_up=").unwrap_or(filter);
    format!(
        "setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709,\
         tvai_up={settings}:w={w}:h={h},scale=w={w}:h={h}:flags=lanczos,setsar=1,format=yuv420p10le"
    )
}

/// The first `model=` part of the filter, e.g. `model=prob-4`.
pub fn filter_model(filter: &str) -> &str {
    let settings = filter.strip_prefix("tvai_up=").unwrap_or(filter);
    settings.split(':').next().unwrap_or("")
}

pub fn selftest_args(filter: &str) -> Vec<OsString> {
    let vf = format!("format=yuv420p10le,{}", topaz_chain(filter, 1280, 720));
    [
        "-hide_banner",
        "-nostdin",
        "-v",
        "error",
        "-sws_flags",
        "spline+accurate_rnd+full_chroma_int",
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=640x360:r=24:d=0.5",
        "-vf",
        &vf,
        "-f",
        "yuv4mpegpipe",
        "-strict",
        "-1",
        "-",
    ]
    .map(OsString::from)
    .into()
}

/// Checks a self-test's exit code and that stdout is a clean y4m stream (Topaz's own messages
/// must go to stderr, or they would corrupt the frames sent to SVT-AV1).
pub fn check_selftest(
    status: Option<i32>,
    stdout_head: &str,
    stderr: &str,
    filter: &str,
) -> Result<()> {
    if status != Some(0) {
        let tail: Vec<&str> = stderr
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .collect();
        let reason = tail[tail.len().saturating_sub(3)..].join(" | ");
        return Err(Error::Tool(format!(
            "Topaz Video AI failed its self-test ({filter}): {reason}. Open Topaz Video AI, check you're signed in, \
             and export a few seconds with this model once so it downloads."
        )));
    }
    let head: String = stdout_head.chars().take(9).collect();
    if head != "YUV4MPEG2" {
        return Err(Error::Tool(format!(
            "Topaz Video AI's ffmpeg wrote something other than video to its output (starts with '{head}'), \
             so it can't be piped to the encoder. Please send the batch log."
        )));
    }
    Ok(())
}

/// Upscales a short test pattern with the configured model, so a missing sign-in, model or
/// GPU problem stops the batch up front with Topaz's own error.
pub fn selftest(runner: &dyn Runner, topaz: &Topaz, filter: &str, log: &dyn Log) -> Result<()> {
    let out = runner.output_env(&topaz.ffmpeg, &selftest_args(filter), &topaz.env_os())?;
    check_selftest(out.status, &out.stdout, &out.stderr, filter)?;
    log.info(&format!(
        "  Topaz self-test passed (Video AI {}, {})",
        topaz.version,
        filter_model(filter)
    ));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::DEFAULT_TOPAZ_FILTER;

    #[test]
    fn chain_matches_the_script() {
        assert_eq!(
            topaz_chain("model=prob-4:scale=0", 3840, 2160),
            "setparams=range=tv:color_primaries=bt709:color_trc=bt709:colorspace=bt709,tvai_up=model=prob-4:scale=0:w=3840:h=2160,\
             scale=w=3840:h=2160:flags=lanczos,setsar=1,format=yuv420p10le"
        );
        assert_eq!(
            topaz_chain("tvai_up=model=x", 2, 2),
            topaz_chain("model=x", 2, 2),
            "a pasted tvai_up= is accepted"
        );
        assert_eq!(filter_model(DEFAULT_TOPAZ_FILTER), "model=prob-4");
        assert_eq!(filter_model("tvai_up=model=prob-4:x=1"), "model=prob-4");
    }

    #[test]
    fn selftest_checks() {
        assert!(check_selftest(Some(0), "YUV4MPEG2 W1280", "", "f").is_ok());
        let e = check_selftest(Some(0), "[tvai] lo", "", "f")
            .unwrap_err()
            .to_string();
        assert!(e.contains("starts with '[tvai] lo'"));
        let e = check_selftest(Some(1), "", "a\n\nb\nc\nd\n", "model=x")
            .unwrap_err()
            .to_string();
        assert!(e.starts_with("Topaz Video AI failed its self-test (model=x): b | c | d."));
    }
}
