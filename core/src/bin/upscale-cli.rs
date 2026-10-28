//! Headless driver for `upscale-core` (PLAN.md Phase 1).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde_json::{Map, Value};
use upscale_core::batch::{self, Batch};
use upscale_core::disk::free_space;
use upscale_core::gpu::{list_gpus, pick_gpu};
use upscale_core::log::{FileLog, Log};
use upscale_core::process::SystemRunner;
use upscale_core::report::render_plan;
use upscale_core::settings::{parse_option, resolve};
use upscale_core::tools::install::Installer;
use upscale_core::tools::set::{Needs, ToolStatus};
use upscale_core::tools::{self, default_tools_dir, find_scripts_dir};
use upscale_core::{Error, ProfileId};

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
    /// Process a file or folder, printing progress as JSON lines.
    Run {
        #[command(flatten)]
        job: JobArgs,
        /// Encode a 30 s clip from the middle, video only.
        #[arg(long)]
        test_clip: bool,
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

    let b: Batch = batch::prepare(&job.path, &job.options()?, &out_dir, &SystemRunner, &log)?;
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
        Command::Run { .. } => Err(Error::NotImplemented("run").into()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}
