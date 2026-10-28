//! Running external tools for their output. Commands are always argument vectors, never shell
//! strings. Long-running pipeline processes (Job Objects, the pump) come in Phase 1D.

use std::ffi::OsString;
use std::path::Path;
use std::process::{Command, Stdio};

/// Captured result of a finished process. Output is decoded lossily: these are text tools.
#[derive(Debug, Clone, Default)]
pub struct Output {
    pub status: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

impl Output {
    pub fn success(&self) -> bool {
        self.status == Some(0)
    }
}

/// Runs a tool to completion. A trait so probe and analysis code can be tested with canned output.
pub trait Runner {
    fn output(&self, exe: &Path, args: &[OsString]) -> std::io::Result<Output> {
        self.output_env(exe, args, &[])
    }
    /// Like [`Runner::output`], with extra environment variables for the child.
    fn output_env(
        &self,
        exe: &Path,
        args: &[OsString],
        env: &[(OsString, OsString)],
    ) -> std::io::Result<Output>;
}

/// Runs real processes: no console window on Windows, below-normal priority.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn output_env(
        &self,
        exe: &Path,
        args: &[OsString],
        env: &[(OsString, OsString)],
    ) -> std::io::Result<Output> {
        let out = command(exe)
            .args(args)
            .envs(env.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .output()?;
        Ok(Output {
            status: out.status.code(),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// A `Command` for `exe` with the platform flags every child process gets.
pub fn command(exe: &Path) -> Command {
    #[allow(unused_mut)]
    let mut cmd = Command::new(exe);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        cmd.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
    }
    cmd
}
