//! Claims on episodes in the output folder, so two machines sharing one output folder (e.g. a
//! network share) never encode the same episode at once. A job holds
//! `<output>/.claims/<name>.claim` (created exclusively, which SMB honours) while it runs,
//! refreshes its modification time every few minutes, and removes it when it ends. Another
//! machine skips the episode while the claim is fresh; a claim not refreshed for
//! [`STALE_AFTER`] (its machine crashed or lost the share) is taken over. A claim left by this
//! machine (e.g. before a crash) is always taken over.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

/// A claim not refreshed for this long is treated as abandoned.
pub const STALE_AFTER: Duration = Duration::from_secs(30 * 60);
/// How often a running job refreshes its claim.
pub const REFRESH_EVERY: Duration = Duration::from_secs(2 * 60);

/// What a claim file holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    pub host: String,
    pub pid: u32,
    pub started: String,
}

/// A held claim; dropping it removes the file.
#[derive(Debug)]
pub struct Claim {
    path: PathBuf,
    refresher: Option<tokio::task::JoinHandle<()>>,
}

/// The claim file for output `base`.
pub fn claim_path(out_dir: &Path, base: &str) -> PathBuf {
    out_dir.join(".claims").join(format!("{base}.claim"))
}

/// The start of a job's skip reason when another machine holds its claim
/// (`being encoded on MAC-MINI`).
pub const BUSY_PREFIX: &str = "being encoded on ";

/// The other machine holding a fresh claim on `base`, if any (nothing is changed).
pub fn held_elsewhere(out_dir: &Path, base: &str, host: &str) -> Option<Holder> {
    let path = claim_path(out_dir, base);
    let age = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .ok()
        .map(|t| SystemTime::now().duration_since(t).unwrap_or_default())?;
    read_holder(&path).filter(|h| h.host != host && age < STALE_AFTER)
}

/// This computer's name, as other machines see it in a claim.
pub fn host_name() -> String {
    #[cfg(windows)]
    let name = std::env::var("COMPUTERNAME").ok();
    #[cfg(unix)]
    let name = {
        let mut buf = [0u8; 256];
        // SAFETY: the buffer is valid for its length; gethostname NUL-terminates within it.
        let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
        ok.then(|| {
            let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
            String::from_utf8_lossy(&buf[..end]).into_owned()
        })
    };
    #[cfg(not(any(windows, unix)))]
    let name: Option<String> = None;
    name.filter(|n| !n.is_empty())
        .unwrap_or_else(|| "unknown".into())
}

impl Claim {
    /// Claims `base` for `host`. `Ok(Err(holder))` when another machine holds a fresh claim.
    pub fn acquire(
        out_dir: &Path,
        base: &str,
        host: &str,
    ) -> std::io::Result<Result<Self, Holder>> {
        let path = claim_path(out_dir, base);
        std::fs::create_dir_all(path.parent().expect("claims folder"))?;
        let me = Holder {
            host: host.into(),
            pid: std::process::id(),
            started: chrono::Local::now().to_rfc3339(),
        };
        // Twice: once more after removing a stale or own claim
        for _ in 0..2 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => {
                    serde_json::to_writer_pretty(file, &me)?;
                    return Ok(Ok(Self {
                        path,
                        refresher: None,
                    }));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    if let Some(h) = held_elsewhere(out_dir, base, host) {
                        return Ok(Err(h));
                    }
                    // Abandoned, unreadable, or left by this machine
                    match std::fs::remove_file(&path) {
                        Ok(()) => {}
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e),
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Err(std::io::Error::other(format!(
            "couldn't claim {}",
            path.display()
        )))
    }

    /// Refreshes the claim every [`REFRESH_EVERY`] until it's dropped (needs a tokio runtime).
    pub fn keep_fresh(&mut self) {
        let path = self.path.clone();
        self.refresher = Some(tokio::spawn(async move {
            loop {
                tokio::time::sleep(REFRESH_EVERY).await;
                let _ = touch(&path);
            }
        }));
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        if let Some(r) = self.refresher.take() {
            r.abort();
        }
        let _ = std::fs::remove_file(&self.path);
    }
}

fn read_holder(path: &Path) -> Option<Holder> {
    serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
}

fn touch(path: &Path) -> std::io::Result<()> {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)?
        .set_modified(SystemTime::now())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn age(path: &Path, by: Duration) {
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - by)
            .unwrap();
    }

    #[test]
    fn one_machine_at_a_time() {
        let out = tempfile::tempdir().unwrap();
        let pc = Claim::acquire(out.path(), "ep1", "PC").unwrap().unwrap();
        assert!(pc.path().is_file());
        let busy = Claim::acquire(out.path(), "ep1", "MAC")
            .unwrap()
            .unwrap_err();
        assert_eq!(busy.host, "PC");
        assert_eq!(
            held_elsewhere(out.path(), "ep1", "MAC").map(|h| h.host),
            Some("PC".into())
        );
        assert_eq!(
            held_elsewhere(out.path(), "ep1", "PC"),
            None,
            "not by itself"
        );
        assert_eq!(held_elsewhere(out.path(), "ep9", "MAC"), None);
        assert!(
            Claim::acquire(out.path(), "ep2", "MAC").unwrap().is_ok(),
            "other episodes are free"
        );
        drop(pc);
        assert!(!claim_path(out.path(), "ep1").exists(), "released on drop");
        assert!(Claim::acquire(out.path(), "ep1", "MAC").unwrap().is_ok());
    }

    #[test]
    fn stale_and_own_claims_are_taken_over() {
        let out = tempfile::tempdir().unwrap();
        let path = claim_path(out.path(), "ep1");
        // A machine that crashed: its claim stays behind
        std::mem::forget(Claim::acquire(out.path(), "ep1", "PC").unwrap().unwrap());
        assert!(Claim::acquire(out.path(), "ep1", "MAC").unwrap().is_err());
        age(&path, STALE_AFTER + Duration::from_secs(60));
        let mac = Claim::acquire(out.path(), "ep1", "MAC").unwrap().unwrap();
        assert_eq!(read_holder(&path).unwrap().host, "MAC");
        std::mem::forget(mac);
        // The same machine after a crash takes its own claim back at once
        assert!(Claim::acquire(out.path(), "ep1", "MAC").unwrap().is_ok());
    }

    #[test]
    fn an_unreadable_claim_is_replaced() {
        let out = tempfile::tempdir().unwrap();
        let path = claim_path(out.path(), "ep1");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "half-written").unwrap();
        assert!(Claim::acquire(out.path(), "ep1", "MAC").unwrap().is_ok());
        assert!(!host_name().is_empty());
    }

    #[tokio::test]
    async fn refreshing_keeps_the_claim_fresh() {
        let out = tempfile::tempdir().unwrap();
        let mut c = Claim::acquire(out.path(), "ep1", "PC").unwrap().unwrap();
        age(c.path(), Duration::from_secs(600));
        touch(c.path()).unwrap();
        let fresh = std::fs::metadata(c.path()).unwrap().modified().unwrap();
        assert!(SystemTime::now().duration_since(fresh).unwrap() < Duration::from_secs(60));
        c.keep_fresh();
        drop(c);
        assert!(!claim_path(out.path(), "ep1").exists());
    }
}
