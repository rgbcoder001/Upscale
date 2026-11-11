//! Writing finished files to the output folder when it may be a network share. A share that
//! drops for a moment (Windows error 59, "an unexpected network error occurred") fails a single
//! write, so a whole day's encode shouldn't depend on one. These helpers copy with a fresh start
//! after each failure and wait a little longer every time.

use std::future::Future;
use std::io::{self, Read, Write};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use crate::pipeline::Control;

/// Waits before each new attempt at writing to the output folder: a share that dropped is
/// usually back within a minute or two. Five attempts over about eight minutes.
pub const OUTPUT_RETRY_WAITS: [Duration; 4] = [
    Duration::from_secs(15),
    Duration::from_secs(60),
    Duration::from_secs(120),
    Duration::from_secs(300),
];

const BUFFER: usize = 8 << 20;

fn cancelled() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "cancelled")
}

/// Copies `from` to `to`, replacing it. Checks for a cancel between blocks, flushes `to` to the
/// disk (or the share) and fails unless it ends up exactly as long as `from`.
pub fn copy_file(from: &Path, to: &Path, control: &Control) -> io::Result<u64> {
    let mut reader = std::fs::File::open(from)?;
    let want = reader.metadata()?.len();
    let mut writer = std::fs::File::create(to)?;
    let mut buf = vec![0u8; BUFFER];
    let mut copied = 0u64;
    loop {
        if control.is_cancelled() {
            return Err(cancelled());
        }
        let n = match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        writer.write_all(&buf[..n])?;
        copied += n as u64;
    }
    writer.sync_all()?;
    let written = std::fs::metadata(to)?.len();
    if copied != want || written != want {
        return Err(io::Error::other(format!(
            "copied {written} of {want} bytes"
        )));
    }
    Ok(copied)
}

/// Sleeps for `d`; `Err` (Interrupted) as soon as the job is cancelled.
pub async fn sleep_or_cancel(d: Duration, control: &Control) -> io::Result<()> {
    tokio::select! {
        _ = tokio::time::sleep(d) => Ok(()),
        _ = control.wait_cancelled() => Err(cancelled()),
    }
}

/// Runs `attempt` until it succeeds, waiting `waits[i]` after the i-th failure and calling
/// `on_retry` before each wait. Returns the last error when the waits run out, or at once when
/// the job is cancelled.
pub async fn with_retries<T, Fut>(
    waits: &[Duration],
    control: &Control,
    mut on_retry: impl FnMut(&io::Error, Duration),
    mut attempt: impl FnMut() -> Fut,
) -> io::Result<T>
where
    Fut: Future<Output = io::Result<T>>,
{
    let mut waits = waits.iter();
    loop {
        let e = match attempt().await {
            Ok(v) => return Ok(v),
            Err(e) => e,
        };
        if control.is_cancelled() {
            return Err(cancelled());
        }
        let Some(&wait) = waits.next() else {
            return Err(e);
        };
        on_retry(&e, wait);
        sleep_or_cancel(wait, control).await?;
    }
}

/// [`copy_file`] off the async threads, started over after each failure (see [`with_retries`]).
/// A failed copy leaves no file at `to`.
pub async fn copy_retrying(
    from: &Path,
    to: &Path,
    control: &Arc<Control>,
    waits: &[Duration],
    on_retry: impl FnMut(&io::Error, Duration),
) -> io::Result<u64> {
    let result = with_retries(waits, control, on_retry, || {
        let (from, to, control) = (from.to_path_buf(), to.to_path_buf(), control.clone());
        async move {
            tokio::task::spawn_blocking(move || copy_file(&from, &to, &control))
                .await
                .map_err(io::Error::other)?
        }
    })
    .await;
    if result.is_err() {
        let _ = std::fs::remove_file(to);
    }
    result
}

/// Moves `from` to `to` from a plain thread (the disc ripper): a rename when they're on the
/// same drive, else a copy to `<to>.partial`, renamed once complete, then `from` is deleted.
/// A failed copy starts over after each of `waits`, calling `on_retry` first. `to` must not
/// exist. A failure leaves `from` where it was.
pub fn move_file_blocking(
    from: &Path,
    to: &Path,
    waits: &[Duration],
    mut on_retry: impl FnMut(&io::Error, Duration),
) -> io::Result<()> {
    if to.exists() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        ));
    }
    if let Some(dir) = to.parent() {
        std::fs::create_dir_all(dir)?;
    }
    if std::fs::rename(from, to).is_ok() {
        return Ok(());
    }
    let mut partial = to.as_os_str().to_owned();
    partial.push(".partial");
    let partial = std::path::PathBuf::from(partial);
    let control = Control::new();
    let mut waits = waits.iter();
    loop {
        match copy_file(from, &partial, &control) {
            Ok(_) => break,
            Err(e) => {
                let _ = std::fs::remove_file(&partial);
                let Some(&wait) = waits.next() else {
                    return Err(e);
                };
                on_retry(&e, wait);
                std::thread::sleep(wait);
            }
        }
    }
    if let Err(e) = std::fs::rename(&partial, to) {
        let _ = std::fs::remove_file(&partial);
        return Err(e);
    }
    let _ = std::fs::remove_file(from);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[test]
    fn copies_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.mkv"), dir.path().join("b.mkv"));
        let data: Vec<u8> = (0..(BUFFER + 1234)).map(|i| (i % 251) as u8).collect();
        std::fs::write(&a, &data).unwrap();
        std::fs::write(
            &b,
            b"an older, longer file that should be replaced entirely",
        )
        .unwrap();
        let n = copy_file(&a, &b, &Control::new()).unwrap();
        assert_eq!(n, data.len() as u64);
        assert_eq!(std::fs::read(&b).unwrap(), data);
    }

    #[test]
    fn a_cancelled_job_stops_the_copy() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a.mkv"), dir.path().join("b.mkv"));
        std::fs::write(&a, b"data").unwrap();
        let control = Control::new();
        control.cancel();
        let e = copy_file(&a, &b, &control).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Interrupted);
    }

    #[tokio::test]
    async fn retries_until_it_works() {
        let control = Control::new();
        let tries = Cell::new(0);
        let mut retried = Vec::new();
        let out = with_retries(
            &[Duration::ZERO; 4],
            &control,
            |e, _| retried.push(e.to_string()),
            || {
                tries.set(tries.get() + 1);
                let n = tries.get();
                async move {
                    if n < 3 {
                        Err(io::Error::other(format!("network error {n}")))
                    } else {
                        Ok(n)
                    }
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(out, 3);
        assert_eq!(retried, ["network error 1", "network error 2"]);
    }

    #[tokio::test]
    async fn gives_up_with_the_last_error() {
        let control = Control::new();
        let tries = Cell::new(0);
        let e = with_retries(
            &[Duration::ZERO; 2],
            &control,
            |_, _| {},
            || {
                tries.set(tries.get() + 1);
                let n = tries.get();
                async move { Err::<(), _>(io::Error::other(format!("network error {n}"))) }
            },
        )
        .await
        .unwrap_err();
        assert_eq!(tries.get(), 3);
        assert_eq!(e.to_string(), "network error 3");
    }

    #[tokio::test]
    async fn a_cancel_during_the_wait_stops_retrying() {
        let control = Control::new();
        let c = control.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            c.cancel();
        });
        let tries = Cell::new(0);
        let e = with_retries(
            &[Duration::from_secs(600)],
            &control,
            |_, _| {},
            || {
                tries.set(tries.get() + 1);
                async { Err::<(), _>(io::Error::other("network error")) }
            },
        )
        .await
        .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::Interrupted);
        assert_eq!(tries.get(), 1);
    }

    #[test]
    fn moves_into_a_new_folder_and_never_over_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("rip.mkv");
        std::fs::write(&from, b"raw rip").unwrap();
        let to = dir
            .path()
            .join("Show")
            .join("Season 1")
            .join("Show S01E01.mkv");
        move_file_blocking(&from, &to, &[], |_, _| {}).unwrap();
        assert_eq!(std::fs::read(&to).unwrap(), b"raw rip");
        assert!(!from.exists());

        std::fs::write(&from, b"another rip").unwrap();
        let e = move_file_blocking(&from, &to, &[], |_, _| {}).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AlreadyExists);
        assert!(from.exists(), "kept when the move fails");
        assert_eq!(std::fs::read(&to).unwrap(), b"raw rip");
    }

    #[tokio::test]
    async fn a_failed_copy_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let to = dir.path().join("out.partial.mkv");
        let e = copy_retrying(
            &dir.path().join("missing.mkv"),
            &to,
            &Control::new(),
            &[Duration::ZERO],
            |_, _| {},
        )
        .await
        .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::NotFound);
        assert!(!to.exists());
    }
}
