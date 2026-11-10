//! Human-readable log lines (the script's `Write-Log`).

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

/// Batch and plan logs older than this are deleted (episode logs are kept).
pub const KEEP_RUN_LOGS: Duration = Duration::from_secs(30 * 24 * 60 * 60);
/// `app.log` is rotated to `app.log.1` when it grows past this.
pub const APP_LOG_MAX: u64 = 5 << 20;

/// Deletes `batch-*.log` and `plan-*.log` in `logs` older than `keep`. Episode logs
/// (`<name>.log`, a record of how each output was made) and the `analysis` cache stay.
/// Returns how many were deleted.
pub fn trim_run_logs(logs: &Path, keep: Duration) -> usize {
    let Ok(entries) = std::fs::read_dir(logs) else {
        return 0;
    };
    let now = SystemTime::now();
    entries
        .filter_map(|e| e.ok())
        .filter(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            (name.starts_with("batch-") || name.starts_with("plan-")) && name.ends_with(".log")
        })
        .filter(|e| {
            e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| now.duration_since(t).unwrap_or_default() > keep)
        })
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

/// Moves `path` to `<path>.1` (replacing an older one) when it's larger than `max` bytes.
pub fn rotate(path: &Path, max: u64) {
    if std::fs::metadata(path).is_ok_and(|m| m.len() > max) {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        let _ = std::fs::rename(path, old);
    }
}

/// Writes panics to `path` (`app.log`) before the usual handler runs. Release builds abort on
/// a panic, so without this a crash leaves nothing to look at afterwards.
pub fn log_panics(path: std::path::PathBuf) {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(&path) {
            let _ = writeln!(
                f,
                "[{}] PANIC: {info}",
                chrono::Local::now().format("%Y-%m-%d %H:%M:%S")
            );
            let _ = f.sync_all();
        }
        previous(info);
    }));
}

pub trait Log: Send + Sync {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn age(path: &Path, by: Duration) {
        File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - by)
            .unwrap();
    }

    #[test]
    fn old_run_logs_go_episode_logs_stay() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        std::fs::create_dir(d.join("analysis")).unwrap();
        for f in [
            "batch-20260901-010101.log",
            "plan-20260901-010101.log",
            "batch-20261101-010101.log",
            "Sample Series S01E001.log",
            "analysis/x.json",
        ] {
            std::fs::write(d.join(f), "x").unwrap();
        }
        let old = Duration::from_secs(60 * 24 * 60 * 60);
        for f in [
            "batch-20260901-010101.log",
            "plan-20260901-010101.log",
            "Sample Series S01E001.log",
            "analysis/x.json",
        ] {
            age(&d.join(f), old);
        }
        assert_eq!(trim_run_logs(d, KEEP_RUN_LOGS), 2);
        assert!(d.join("batch-20261101-010101.log").is_file(), "recent");
        assert!(d.join("Sample Series S01E001.log").is_file(), "episode log");
        assert!(d.join("analysis/x.json").is_file(), "cache");
        assert_eq!(trim_run_logs(&d.join("missing"), KEEP_RUN_LOGS), 0);
    }

    #[test]
    fn a_large_app_log_is_rotated() {
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("app.log");
        std::fs::write(&log, "small").unwrap();
        rotate(&log, 100);
        assert!(log.is_file());
        std::fs::write(&log, vec![b'x'; 200]).unwrap();
        rotate(&log, 100);
        assert!(!log.exists() && dir.path().join("app.log.1").is_file());
    }
}
