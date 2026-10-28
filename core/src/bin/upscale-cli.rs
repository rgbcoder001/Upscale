//! Headless driver for `upscale-core` (PLAN.md Phase 1).

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use serde_json::{Map, Value};
use upscale_core::batch::{self, Batch};
use upscale_core::disk::free_space;
use upscale_core::log::{FileLog, Log};
use upscale_core::process::SystemRunner;
use upscale_core::report::render_plan;
use upscale_core::settings::parse_option;
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
        /// Install or update tools before the self-tests.
        #[arg(long)]
        update: bool,
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result = match &cli.command {
        Command::Tools { .. } => Err(Error::NotImplemented("tools").into()),
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
