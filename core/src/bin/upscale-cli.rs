//! Headless driver for `upscale-core` (PLAN.md Phase 1). Subcommands are stubs until then.

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use upscale_core::{Error, ProfileId};

#[derive(Parser)]
#[command(name = "upscale-cli", version = upscale_core::VERSION, about = "Upscale anime and CG to 4K AV1")]
struct Cli {
    #[command(subcommand)]
    command: Command,
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
        path: PathBuf,
        #[arg(long)]
        profile: Option<ProfileId>,
    },
    /// Process a file or folder, printing progress as JSON lines.
    Run {
        path: PathBuf,
        #[arg(long)]
        profile: Option<ProfileId>,
        /// Encode a 30 s clip from the middle, video only.
        #[arg(long)]
        test_clip: bool,
    },
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    let result: upscale_core::Result<()> = match cli.command {
        Command::Tools { .. } => Err(Error::NotImplemented("tools")),
        Command::Plan { .. } => Err(Error::NotImplemented("plan")),
        Command::Run { .. } => Err(Error::NotImplemented("run")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
