//! Headless driver for `upscale-core` (PLAN.md Phase 1).

use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde_json::{Map, Value};
use upscale_core::batch::{self, Batch};
use upscale_core::disk::free_space;
use upscale_core::gpu::{list_gpus, pick_gpu};
use upscale_core::log::{FileLog, Log};
use upscale_core::pipeline::{Control, JobResult, RunEnv};
use upscale_core::process::Runner;
use upscale_core::process::SystemRunner;
use upscale_core::report::render_plan;
use upscale_core::settings::{find_settings_files, settings_dir};
use upscale_core::settings::{parse_option, resolve};
use upscale_core::tools::install::Installer;
use upscale_core::tools::set::{Needs, ToolStatus};
use upscale_core::tools::{self, default_tools_dir, find_scripts_dir};
use upscale_core::{Event, ProfileId};

#[derive(Parser)]
#[command(name = "upscale-cli", version = upscale_core::VERSION, about = "Upscale anime and CG to 4K AV1")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Args)]
struct JobArgs {
    /// A video file, or a folder of them.
    path: PathBuf,
    /// Content profile (default: from upscale-settings.json, else anime).
    #[arg(long)]
    profile: Option<ProfileId>,
    /// Output folder (default: <input folder>\upscaled).
    #[arg(long)]
    out: Option<PathBuf>,
    /// Any script setting as Key=Value, e.g. --set Crf=12 --set Ivtc=off. Repeatable.
    #[arg(long = "set", value_name = "KEY=VALUE")]
    set: Vec<String>,
}

impl JobArgs {
    fn options(&self) -> Result<Map<String, Value>> {
        let mut map = Map::new();
        for kv in &self.set {
            let (k, v) = parse_option(kv).map_err(anyhow::Error::msg)?;
            map.insert(k, v);
        }
        if let Some(p) = self.profile {
            map.insert("Content".into(), Value::String(p.to_string()));
        }
        Ok(map)
    }

    fn out_dir(&self) -> Result<PathBuf> {
        let out = match &self.out {
            Some(o) => o.clone(),
            None => batch::default_out_dir(&std::path::absolute(&self.path)?),
        };
        Ok(std::path::absolute(out)?)
    }
}

#[derive(Subcommand)]
enum Command {
    /// Find, install and self-test the external tools.
    Tools {
        /// Install missing tools and update all of them (the script's -UpdateTools).
        #[arg(long)]
        update: bool,
        /// Private tools folder (vs-mlrt, BtbN FFmpeg). Default: <app data>/tools.
        #[arg(long)]
        tools_dir: Option<PathBuf>,
        /// Skip the vs-mlrt and Topaz self-tests.
        #[arg(long)]
        no_selftest: bool,
        /// Settings that affect tools, e.g. --set TopazPath=D:\Topaz --set Backend=ncnn.
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
    },
    /// Print the processing plan for a file or folder (like the script's -Plan).
    Plan {
        #[command(flatten)]
        job: JobArgs,
        /// Print the plan as JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// Encode the same section unchunked and chunked, and compare size, VMAF and SSIM.
    Ab {
        /// A video file.
        path: PathBuf,
        /// Minutes from the middle of the file (0 = the whole file).
        #[arg(long, default_value_t = 12.0)]
        minutes: f64,
        /// Where the two encodes and the report go (default: <input folder>\upscaled).
        #[arg(long)]
        out: Option<PathBuf>,
        /// Any script setting as Key=Value. Repeatable.
        #[arg(long = "set", value_name = "KEY=VALUE")]
        set: Vec<String>,
        /// Private tools folder. Default: <app data>/tools.
        #[arg(long)]
        tools_dir: Option<PathBuf>,
    },
    /// Process a file or folder, printing progress as JSON lines.
    Run {
        #[command(flatten)]
        job: JobArgs,
        /// Encode a 30 s clip (720 frames) from the middle, video only.
        #[arg(long)]
        test_clip: bool,
        /// Keep the intermediate video stream after muxing.
        #[arg(long)]
        keep_ivf: bool,
        /// Private tools folder (vs-mlrt, BtbN FFmpeg). Default: <app data>/tools.
        #[arg(long)]
        tools_dir: Option<PathBuf>,
    },
}

fn timestamp() -> String {
    chrono::Local::now().format("%Y%m%d-%H%M%S").to_string()
}

fn plan(job: &JobArgs, json: bool) -> Result<()> {
    if !job.path.exists() {
        bail!("'{}' does not exist", job.path.display());
    }
    let out_dir = job.out_dir()?;
    let log_dir = out_dir.join("logs");
    std::fs::create_dir_all(&log_dir).with_context(|| format!("creating {}", log_dir.display()))?;
    let stamp = timestamp();
    let log = FileLog::create(&log_dir.join(format!("plan-{stamp}.log")))?;

    let gpu = pick_gpu(&list_gpus(&SystemRunner));
    log.info(&gpu.log_line());
    let b: Batch = batch::prepare(
        &job.path,
        &job.options()?,
        &out_dir,
        &SystemRunner,
        &gpu,
        &log,
    )?;
    if b.plans.is_empty() {
        println!("No video files found in '{}'.", b.input.display());
        return Ok(());
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&b)?);
        return Ok(());
    }
    let lines = render_plan(&b, free_space(&out_dir));
    let file = log_dir.join(format!("plan-{stamp}.txt"));
    std::fs::write(&file, lines.join("\r\n") + "\r\n")?;
    for l in &lines {
        println!("{l}");
    }
    log.info(&format!("Plan written to {}", file.display()));
    Ok(())
}

fn print_table(rows: &[ToolStatus]) {
    let width = rows.iter().map(|r| r.name.len()).max().unwrap_or(0);
    for r in rows {
        let mark = if r.ok { "ok " } else { "-- " };
        let version = r.version.as_deref().unwrap_or("");
        let mut detail = r
            .path
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default();
        if !r.note.is_empty() {
            if !detail.is_empty() {
                detail.push_str("  ");
            }
            detail.push_str(&format!("({})", r.note));
        }
        println!("{mark}{:<width$}  {version:<12} {detail}", r.name);
    }
}

fn tools(
    update: bool,
    tools_dir: Option<PathBuf>,
    no_selftest: bool,
    set: &[String],
) -> Result<()> {
    let runner = SystemRunner;
    let log = FileLog::stderr_only();
    let tools_dir = std::path::absolute(tools_dir.unwrap_or_else(default_tools_dir))?;
    let mut options = Map::new();
    for kv in set {
        let (k, v) = parse_option(kv).map_err(anyhow::Error::msg)?;
        options.insert(k, v);
    }
    let settings = resolve(&options, &[])?.settings;
    let gpu = pick_gpu(&list_gpus(&runner));
    log.info(&gpu.log_line());
    let backend = settings.backend.unwrap_or_else(|| gpu.default_backend());
    let vpy = find_scripts_dir().map(|d| d.join("upscale.vpy"));
    let topaz_dir = settings.topaz_dir();

    if update {
        std::fs::create_dir_all(&tools_dir)?;
        let inst = Installer {
            runner: &runner,
            log: &log,
            tools_dir: tools_dir.clone(),
            update: true,
        };
        let set = tools::set::ensure(
            &inst,
            Needs {
                gpu: &gpu,
                mlrt: true,
                deband: settings.deband,
                handbrake: true,
            },
        )?;
        if !no_selftest {
            let vpy = vpy
                .as_deref()
                .context("upscale.vpy not found (set UPSCALE_SCRIPTS_DIR)")?;
            let mlrt = set.mlrt.as_deref().context("vs-mlrt not installed")?;
            tools::mlrt::selftest(
                &runner,
                &set.vspipe,
                vpy,
                backend,
                mlrt,
                &set.mlrt_root,
                &log,
            )?;
        }
        if let Some(dir) = topaz_dir
            .as_deref()
            .filter(|d| d.join("ffmpeg.exe").is_file())
        {
            let tested = tools::topaz::init_topaz(&runner, dir, &log).and_then(|t| {
                if no_selftest {
                    Ok(())
                } else {
                    tools::topaz::selftest(&runner, &t, &settings.topaz_filter, &log)
                }
            });
            if let Err(e) = tested {
                log.warn(&format!("Topaz (used for CG films): {e}"));
            }
        } else {
            log.info(&format!(
                "  Topaz Video AI not found in {} (only needed for CG films)",
                topaz_dir
                    .as_deref()
                    .map(|d| d.display().to_string())
                    .unwrap_or_default()
            ));
        }
        log.info("All tools are installed and up to date.");
    }

    let (rows, _) = tools::set::inspect(&runner, &tools_dir, &gpu, topaz_dir.as_deref());
    println!("Tools folder: {}", tools_dir.display());
    print_table(&rows);
    if update || no_selftest {
        return Ok(());
    }

    // Self-tests of whatever is installed; a failure is reported and makes the exit code non-zero
    let mut failed = false;
    let find = |name: &str| {
        rows.iter()
            .find(|r| r.name == name && r.ok)
            .and_then(|r| r.path.clone())
    };
    match (find("VapourSynth"), find("vs-mlrt"), vpy.as_deref()) {
        (Some(vspipe), Some(mlrt), Some(vpy)) => {
            if let Err(e) = tools::mlrt::selftest(
                &runner,
                &vspipe,
                vpy,
                backend,
                &mlrt,
                &tools_dir.join("vs-mlrt"),
                &log,
            ) {
                log.warn(&e.to_string());
                failed = true;
            }
        }
        _ => log.info("  vs-mlrt self-test skipped (VapourSynth, vs-mlrt or upscale.vpy missing)"),
    }
    if let Some(dir) = topaz_dir
        .as_deref()
        .filter(|d| d.join("ffmpeg.exe").is_file())
    {
        let tested = tools::topaz::init_topaz(&runner, dir, &log)
            .and_then(|t| tools::topaz::selftest(&runner, &t, &settings.topaz_filter, &log));
        if let Err(e) = tested {
            log.warn(&format!("Topaz (used for CG films): {e}"));
            failed = true;
        }
    }
    if failed {
        bail!("a self-test failed");
    }
    Ok(())
}

/// Reads `pause`, `resume`, `stop` (after the current file), `continue` and `cancel` lines.
fn control_from_stdin(control: Arc<Control>) {
    std::thread::spawn(move || {
        for line in std::io::stdin().lines() {
            let Ok(line) = line else { break };
            match line.trim().to_ascii_lowercase().as_str() {
                "pause" => control.pause(),
                "resume" => control.resume(),
                "stop" => control.stop_after_current(true),
                "continue" => control.stop_after_current(false),
                "cancel" => control.cancel(),
                "" => {}
                other => {
                    eprintln!("unknown command '{other}' (pause, resume, stop, continue, cancel)")
                }
            }
        }
    });
}

fn run(job: &JobArgs, test_clip: bool, keep_ivf: bool, tools_dir: Option<PathBuf>) -> Result<()> {
    if !job.path.exists() {
        bail!("'{}' does not exist", job.path.display());
    }
    let mut options = job.options()?;
    if test_clip {
        options.entry("Frames").or_insert(720.into());
        options.entry("Start").or_insert((-1).into());
    }
    let out_dir = job.out_dir()?;
    let Some((_, results)) = execute(&job.path, &options, &out_dir, tools_dir, keep_ivf, true)?
    else {
        return Ok(());
    };
    if results
        .iter()
        .any(|r| r.result.starts_with("FAILED") || r.result == "cancelled")
    {
        bail!("not every file finished");
    }
    Ok(())
}

/// Tools, self-tests, plan and run for one input. `None` when there is nothing to do.
fn execute(
    path: &Path,
    options: &Map<String, Value>,
    out_dir: &Path,
    tools_dir: Option<PathBuf>,
    keep_ivf: bool,
    print_events: bool,
) -> Result<Option<(Batch, Vec<JobResult>)>> {
    let log_dir = out_dir.join("logs");
    std::fs::create_dir_all(&log_dir).with_context(|| format!("creating {}", log_dir.display()))?;
    let log = FileLog::create(&log_dir.join(format!("batch-{}.log", timestamp())))?;
    let runner = SystemRunner;

    // GPU, tools and self-tests first: a broken tool stops the batch before any file
    let gpu = pick_gpu(&list_gpus(&runner));
    log.info(&gpu.log_line());
    let input = std::path::absolute(path)?;
    let settings = resolve(options, &find_settings_files(&settings_dir(&input)))?.settings;
    let tools_dir = std::path::absolute(tools_dir.unwrap_or_else(default_tools_dir))?;
    std::fs::create_dir_all(&tools_dir)?;
    let installer = Installer {
        runner: &runner,
        log: &log,
        tools_dir,
        update: false,
    };
    let mut toolset = tools::set::ensure(
        &installer,
        Needs {
            gpu: &gpu,
            mlrt: true,
            deband: settings.deband,
            handbrake: false,
        },
    )?;
    let vpy = find_scripts_dir()
        .context("upscale.vpy not found (set UPSCALE_SCRIPTS_DIR)")?
        .join("upscale.vpy");
    let backend = settings.backend.unwrap_or_else(|| gpu.default_backend());
    if settings.use_topaz() {
        let dir = settings.topaz_dir().context("no Topaz folder")?;
        let t = tools::topaz::init_topaz(&runner, &dir, &log)?;
        tools::topaz::selftest(&runner, &t, &settings.topaz_filter, &log)?;
        toolset.topaz = Some(t);
    } else {
        let mlrt = toolset.mlrt.clone().context("vs-mlrt not installed")?;
        tools::mlrt::selftest(
            &runner,
            &toolset.vspipe,
            &vpy,
            backend,
            &mlrt,
            &toolset.mlrt_root,
            &log,
        )?;
    }

    let b = batch::prepare(path, options, out_dir, &runner, &gpu, &log)?;
    if b.plans.is_empty() {
        println!("No video files found in '{}'.", b.input.display());
        return Ok(None);
    }

    let rt = tokio::runtime::Runtime::new()?;
    let control = Control::new();
    control_from_stdin(control.clone());
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Event>();
    let results = rt.block_on(async {
        let printer = tokio::spawn(async move {
            while let Some(e) = rx.recv().await {
                if print_events {
                    println!("{}", e.to_json_line());
                }
            }
        });
        let ctrl_c = control.clone();
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                eprintln!("Cancelling ...");
                ctrl_c.cancel();
            }
        });
        let mut env = RunEnv {
            batch: &b,
            tools: toolset,
            vpy,
            runner: &runner,
            installer: &installer,
            log: &log,
            events: tx,
            control,
            keep_ivf,
            temp_dir: out_dir.join(".work"),
            restart: false,
        };
        let results = env.run_batch().await;
        drop(env);
        let _ = printer.await;
        results
    });
    Ok(Some((b, results)))
}

/// The one finished `.mkv` in an output folder.
fn finished_mkv(dir: &Path) -> Result<PathBuf> {
    std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .find(|p| {
            p.extension().is_some_and(|e| e == "mkv")
                && !p.to_string_lossy().ends_with(".partial.mkv")
        })
        .with_context(|| format!("no finished file in {}", dir.display()))
}

/// A score from an ffmpeg comparison filter's log (`VMAF score: 99.1`, `SSIM ... All:0.99`).
fn ffmpeg_score(ffmpeg: &Path, a: &Path, b: &Path, filter: &str, pattern: &str) -> Option<String> {
    let args: Vec<std::ffi::OsString> = vec![
        "-hide_banner".into(),
        "-i".into(),
        a.into(),
        "-i".into(),
        b.into(),
        "-lavfi".into(),
        filter.into(),
        "-f".into(),
        "null".into(),
        "-".into(),
    ];
    let out = SystemRunner.output(ffmpeg, &args).ok()?;
    let re = regex::Regex::new(pattern).ok()?;
    re.captures_iter(&out.stderr)
        .last()
        .map(|c| c[1].to_string())
}

/// The same section encoded unchunked and chunked, compared on size, VMAF and SSIM.
fn ab(
    path: &Path,
    minutes: f64,
    out: Option<PathBuf>,
    set: &[String],
    tools_dir: Option<PathBuf>,
) -> Result<()> {
    let mut options = Map::new();
    for kv in set {
        let (k, v) = parse_option(kv).map_err(anyhow::Error::msg)?;
        options.insert(k, v);
    }
    let probe_tools = upscale_core::tools::ProbeTools::find()?;
    let src = upscale_core::probe::probe_source(&SystemRunner, &probe_tools.ffprobe, path)?;
    if minutes > 0.0 {
        let frames = (minutes * 60.0 * src.frame_rate).round() as u64;
        options.insert("Frames".into(), frames.into());
        options.insert("Start".into(), (-1).into());
    }
    let base = match out {
        Some(o) => std::path::absolute(o)?,
        None => batch::default_out_dir(&std::path::absolute(path)?),
    };
    let root = base.join(format!("ab-{}", timestamp()));
    let mut outputs = Vec::new();
    for (label, chunked) in [("unchunked", false), ("chunked", true)] {
        eprintln!("=== A/B: {label} ===");
        let mut o = options.clone();
        o.insert("ResumableEncoding".into(), chunked.into());
        let dir = root.join(label);
        execute(path, &o, &dir, tools_dir.clone(), false, false)?.context("nothing to encode")?;
        outputs.push(finished_mkv(&dir)?);
    }
    let size = |p: &Path| std::fs::metadata(p).map(|m| m.len()).unwrap_or(0);
    let (a, b) = (&outputs[0], &outputs[1]);
    // The BtbN build carries libvmaf; fall back to the system FFmpeg
    let tools_dir = std::path::absolute(tools_dir.unwrap_or_else(default_tools_dir))?;
    let ffmpegs: Vec<PathBuf> =
        upscale_core::tools::find_file(&tools_dir.join("ffmpeg"), "ffmpeg.exe")
            .into_iter()
            .chain([probe_tools.ffmpeg.clone()])
            .collect();
    eprintln!("=== A/B: comparing ===");
    let vmaf = ffmpegs.iter().find_map(|f| {
        ffmpeg_score(
            f,
            b,
            a,
            "[0:v][1:v]libvmaf=n_subsample=4:n_threads=16",
            r"VMAF score: ([\d.]+)",
        )
    });
    let ssim = ffmpegs
        .iter()
        .find_map(|f| ffmpeg_score(f, b, a, "[0:v][1:v]ssim", r"All:([\d.]+)"));
    let (sa, sb) = (size(a), size(b));
    let lines = [
        format!(
            "A/B for {} ({})",
            path.display(),
            if minutes > 0.0 {
                format!("{minutes} min from the middle")
            } else {
                "whole file".into()
            }
        ),
        format!("  Unchunked: {} bytes  {}", sa, a.display()),
        format!("  Chunked:   {} bytes  {}", sb, b.display()),
        format!(
            "  Size:      chunked is {:+.2}% vs unchunked",
            (sb as f64 / sa.max(1) as f64 - 1.0) * 100.0
        ),
        format!(
            "  VMAF:      {} (chunked vs unchunked; 100 = identical)",
            vmaf.as_deref().unwrap_or("unavailable (no libvmaf)")
        ),
        format!("  SSIM:      {}", ssim.as_deref().unwrap_or("unavailable")),
    ];
    std::fs::write(root.join("ab-report.txt"), lines.join("\r\n") + "\r\n")?;
    for l in &lines {
        println!("{l}");
    }
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Tools {
            update,
            tools_dir,
            no_selftest,
            set,
        } => tools(*update, tools_dir.clone(), *no_selftest, set),
        Command::Plan { job, json } => plan(job, *json),
        Command::Ab {
            path,
            minutes,
            out,
            set,
            tools_dir,
        } => ab(path, *minutes, out.clone(), set, tools_dir.clone()),
        Command::Run {
            job,
            test_clip,
            keep_ivf,
            tools_dir,
        } => run(job, *test_clip, *keep_ivf, tools_dir.clone()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
