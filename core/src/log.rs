//! Human-readable log lines (the script's `Write-Log`).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;

pub trait Log {
    fn info(&self, msg: &str);
    fn warn(&self, msg: &str) {
        self.info(&format!("WARNING: {msg}"));
    }
}

/// Discards everything.
pub struct NullLog;

impl Log for NullLog {
    fn info(&self, _: &str) {}
}

/// Collects lines in memory (tests, and the app's plan view).
#[derive(Default)]
pub struct MemoryLog(pub Mutex<Vec<String>>);

impl Log for MemoryLog {
    fn info(&self, msg: &str) {
        self.0.lock().expect("log lock").push(msg.to_string());
    }
}

/// Writes to stderr and appends `[yyyy-MM-dd HH:mm:ss] line` to a log file.
pub struct FileLog {
    file: Mutex<Option<File>>,
}

impl FileLog {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        let file = OpenOptions::new().create(true).append(true).open(path)?;
        Ok(Self {
            file: Mutex::new(Some(file)),
        })
    }

    pub fn stderr_only() -> Self {
        Self {
            file: Mutex::new(None),
        }
    }
}

impl Log for FileLog {
    fn info(&self, msg: &str) {
        eprintln!("{msg}");
        if let Some(f) = self.file.lock().expect("log lock").as_mut() {
            let _ = writeln!(
                f,
                "[{}] {msg}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
            );
        }
    }
}
