//! Process control for long-running pipeline processes: every child of a job goes into one
//! group that is killed together, with no console window and below-normal priority. On
//! Windows that's a Job Object with kill-on-close, so nothing keeps running even if the app
//! crashes. On Unix each child leads its own process group (killed with `killpg`) and runs at
//! nice 10; after a crash the pipes break, so the chain still stops on its own.

use std::ffi::OsString;
use std::path::Path;

use tokio::process::{Child, Command};

/// The nice value of child processes on Unix (Windows: below-normal priority).
#[cfg(unix)]
pub(crate) const BACKGROUND_NICE: i32 = 10;

/// A tokio `Command` with the flags every pipeline child gets.
pub fn command(exe: &Path, args: &[OsString]) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(args).kill_on_drop(true);
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;
        cmd.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
    }
    #[cfg(unix)]
    {
        // Its own process group, so `ProcessGroup::kill` also reaches anything it starts (and a
        // Ctrl-C in the terminal goes to the app, which cancels the job, not to the children)
        cmd.process_group(0);
        // SAFETY: setpriority is async-signal-safe and touches no memory of the parent.
        unsafe {
            cmd.pre_exec(|| {
                libc::setpriority(libc::PRIO_PROCESS, 0, BACKGROUND_NICE);
                Ok(())
            });
        }
    }
    cmd
}

/// The processes of one job.
pub struct ProcessGroup {
    #[cfg(windows)]
    job: windows::Job,
    /// Process group ids (each child leads its own group).
    #[cfg(unix)]
    groups: std::sync::Mutex<Vec<i32>>,
}

impl ProcessGroup {
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {
            #[cfg(windows)]
            job: windows::Job::new()?,
            #[cfg(unix)]
            groups: std::sync::Mutex::default(),
        })
    }

    /// Adds a freshly spawned child (and the processes it starts later).
    pub fn add(&self, child: &Child) -> std::io::Result<()> {
        #[cfg(windows)]
        if let Some(handle) = child.raw_handle() {
            self.job.assign(handle)?;
        }
        #[cfg(unix)]
        if let Some(pid) = child.id() {
            self.groups
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .push(pid as i32);
        }
        #[cfg(not(any(windows, unix)))]
        let _ = child;
        Ok(())
    }

    /// Kills every process in the group. On Unix the children must not have been waited for
    /// yet (until then their ids can't be reused), which is how the pipeline calls this.
    pub fn kill(&self) {
        #[cfg(windows)]
        self.job.terminate();
        #[cfg(unix)]
        for &pgid in self.groups.lock().unwrap_or_else(|e| e.into_inner()).iter() {
            // SAFETY: a plain syscall; a group that already exited just returns ESRCH.
            unsafe { libc::killpg(pgid, libc::SIGKILL) };
        }
    }
}

/// Keeps the computer from sleeping while a batch runs (the display may still turn off).
/// Dropping it lets the computer sleep again.
pub struct KeepAwake {
    /// Windows: a thread holding `ES_SYSTEM_REQUIRED`; it ends when the sender drops.
    #[cfg(windows)]
    _stop: std::sync::mpsc::Sender<()>,
    /// macOS: `caffeinate -i -w <app pid>`, which also ends on its own if the app dies.
    #[cfg(target_os = "macos")]
    caffeinate: std::process::Child,
}

impl KeepAwake {
    /// `None` when the OS gives no way to do it (or it failed): batches still run.
    pub fn start() -> Option<Self> {
        #[cfg(windows)]
        {
            use windows_sys::Win32::System::Power::{
                ES_CONTINUOUS, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
            };
            // The request belongs to the thread that makes it, so it gets a thread of its own
            let (stop, wait) = std::sync::mpsc::channel::<()>();
            std::thread::Builder::new()
                .name("keep-awake".into())
                .spawn(move || {
                    // SAFETY: plain Win32 calls with valid flags.
                    unsafe { SetThreadExecutionState(ES_CONTINUOUS | ES_SYSTEM_REQUIRED) };
                    let _ = wait.recv();
                    unsafe { SetThreadExecutionState(ES_CONTINUOUS) };
                })
                .ok()?;
            Some(Self { _stop: stop })
        }
        #[cfg(target_os = "macos")]
        {
            let caffeinate = std::process::Command::new("/usr/bin/caffeinate")
                .args(["-i", "-w", &std::process::id().to_string()])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .ok()?;
            Some(Self { caffeinate })
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        None
    }
}

#[cfg(target_os = "macos")]
impl Drop for KeepAwake {
    fn drop(&mut self) {
        let _ = self.caffeinate.kill();
        let _ = self.caffeinate.wait();
    }
}

/// Suspends every thread of a process (HandBrake pause: it has no pump to stop reading).
pub fn suspend(pid: u32) -> std::io::Result<()> {
    #[cfg(windows)]
    return windows::for_each_thread(pid, true);
    #[cfg(unix)]
    return signal(pid, "STOP");
}

/// Resumes a process suspended with [`suspend`].
pub fn resume(pid: u32) -> std::io::Result<()> {
    #[cfg(windows)]
    return windows::for_each_thread(pid, false);
    #[cfg(unix)]
    return signal(pid, "CONT");
}

#[cfg(unix)]
fn signal(pid: u32, sig: &str) -> std::io::Result<()> {
    let status = std::process::Command::new("kill")
        .arg(format!("-{sig}"))
        .arg(pid.to_string())
        .status()?;
    if status.success() {
        Ok(())
    } else {
        Err(std::io::Error::other(format!("kill -{sig} {pid} failed")))
    }
}

#[cfg(windows)]
mod windows {
    use std::os::windows::io::RawHandle;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject, TerminateJobObject,
    };

    pub struct Job(HANDLE);

    // The handle is only used through thread-safe Win32 calls.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    impl Job {
        pub fn new() -> std::io::Result<Self> {
            // SAFETY: plain Win32 calls; the handle is checked and owned by `Job`.
            unsafe {
                let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
                if handle.is_null() {
                    return Err(std::io::Error::last_os_error());
                }
                let job = Self(handle);
                let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
                info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
                let ok = SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                );
                if ok == 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(job)
            }
        }

        pub fn assign(&self, process: RawHandle) -> std::io::Result<()> {
            // SAFETY: `process` is a live process handle owned by the tokio Child.
            if unsafe { AssignProcessToJobObject(self.0, process as HANDLE) } == 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        }

        pub fn terminate(&self) {
            // SAFETY: the handle is valid until drop.
            unsafe { TerminateJobObject(self.0, 1) };
        }
    }

    /// Suspends or resumes every thread of `pid`.
    pub fn for_each_thread(pid: u32, suspend: bool) -> std::io::Result<()> {
        use windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE;
        use windows_sys::Win32::System::Diagnostics::ToolHelp::{
            CreateToolhelp32Snapshot, TH32CS_SNAPTHREAD, THREADENTRY32, Thread32First, Thread32Next,
        };
        use windows_sys::Win32::System::Threading::{
            OpenThread, ResumeThread, SuspendThread, THREAD_SUSPEND_RESUME,
        };

        // SAFETY: a thread snapshot walked with a correctly sized entry; every handle opened
        // here is closed here.
        unsafe {
            let snap = CreateToolhelp32Snapshot(TH32CS_SNAPTHREAD, 0);
            if snap == INVALID_HANDLE_VALUE {
                return Err(std::io::Error::last_os_error());
            }
            let mut entry: THREADENTRY32 = std::mem::zeroed();
            entry.dwSize = std::mem::size_of::<THREADENTRY32>() as u32;
            let mut ok = Thread32First(snap, &mut entry);
            while ok != 0 {
                if entry.th32OwnerProcessID == pid {
                    let thread = OpenThread(THREAD_SUSPEND_RESUME, 0, entry.th32ThreadID);
                    if !thread.is_null() {
                        if suspend {
                            SuspendThread(thread);
                        } else {
                            ResumeThread(thread);
                        }
                        CloseHandle(thread);
                    }
                }
                ok = Thread32Next(snap, &mut entry);
            }
            CloseHandle(snap);
        }
        Ok(())
    }

    impl Drop for Job {
        fn drop(&mut self) {
            // Closing the last handle kills every process still in the job.
            // SAFETY: owned handle, closed once.
            unsafe { CloseHandle(self.0) };
        }
    }
}

/// Moves `path` to the Recycle Bin (Windows) or the Trash (macOS). On a drive without a
/// Recycle Bin (e.g. a network share) Windows deletes the file for good.
pub fn move_to_trash(path: &Path) -> std::io::Result<()> {
    let path = std::path::absolute(path)?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::{SHFILEOPSTRUCTW, SHFileOperationW};
        const FO_DELETE: u32 = 3;
        const FOF_SILENT: u16 = 0x4;
        const FOF_NOCONFIRMATION: u16 = 0x10;
        const FOF_ALLOWUNDO: u16 = 0x40;
        const FOF_NOERRORUI: u16 = 0x400;
        // pFrom is a list of paths that ends with an extra NUL
        let from: Vec<u16> = path.as_os_str().encode_wide().chain([0, 0]).collect();
        // SAFETY: all-zero is a valid SHFILEOPSTRUCTW (null handles and strings)
        let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
        op.wFunc = FO_DELETE;
        op.pFrom = from.as_ptr();
        op.fFlags = FOF_SILENT | FOF_NOCONFIRMATION | FOF_ALLOWUNDO | FOF_NOERRORUI;
        // SAFETY: `op` is initialised and `from` outlives the call.
        let r = unsafe { SHFileOperationW(&mut op) };
        if r != 0 || op.fAnyOperationsAborted != 0 {
            return Err(std::io::Error::other(format!(
                "SHFileOperation error {r:#x}"
            )));
        }
    }
    if cfg!(target_os = "macos") {
        let out = std::process::Command::new("osascript")
            .args([
                "-e",
                "on run argv",
                "-e",
                "tell application \"Finder\" to delete (POSIX file (item 1 of argv) as alias)",
                "-e",
                "end run",
            ])
            .arg(&path)
            .output()?;
        if !out.status.success() {
            return Err(std::io::Error::other(
                String::from_utf8_lossy(&out.stderr).trim().to_string(),
            ));
        }
    } else if !cfg!(windows) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "no trash on this platform",
        ));
    }
    if path.exists() {
        return Err(std::io::Error::other("the file is still there"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn killing_the_group_ends_its_processes() {
        let group = ProcessGroup::new().unwrap();
        #[cfg(windows)]
        let (exe, args) = ("ping", vec!["-n", "30", "127.0.0.1"]);
        #[cfg(not(windows))]
        let (exe, args) = ("sleep", vec!["30"]);
        let exe = which::which(exe).unwrap();
        let args: Vec<OsString> = args.into_iter().map(OsString::from).collect();
        let mut child = command(&exe, &args)
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap();
        group.add(&child).unwrap();
        group.kill();
        let status = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(!status.success());
    }

    /// Runs `sh -c script` as a pipeline child and returns it with its first stdout line.
    #[cfg(unix)]
    async fn sh(script: &str) -> (Child, String) {
        use tokio::io::AsyncBufReadExt;
        let args: Vec<OsString> = ["-c", script].map(OsString::from).into();
        let mut child = command(Path::new("/bin/sh"), &args)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = tokio::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        out.read_line(&mut line).await.unwrap();
        (child, line.trim().to_string())
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn killing_the_group_also_ends_grandchildren() {
        let group = ProcessGroup::new().unwrap();
        let (mut child, grandchild) = sh("sleep 30 & echo $!; wait").await;
        group.add(&child).unwrap();
        group.kill();
        let _ = child.wait().await;
        let alive = || {
            std::process::Command::new("kill")
                .args(["-0", &grandchild])
                .status()
                .is_ok_and(|s| s.success())
        };
        for _ in 0..50 {
            if !alive() {
                return;
            }
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
        panic!("sleep {grandchild} still runs after the group was killed");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn children_run_at_background_priority() {
        let (mut child, nice) = sh("ps -o nice= -p $$").await;
        let _ = child.wait().await;
        assert_eq!(nice, BACKGROUND_NICE.to_string());
    }

    #[cfg(windows)]
    #[tokio::test]
    async fn a_suspended_process_makes_no_progress() {
        use tokio::io::AsyncReadExt;
        // ping prints a line per second while it runs
        let exe = which::which("ping").unwrap();
        let args: Vec<OsString> = ["-n", "30", "127.0.0.1"].map(OsString::from).into();
        let mut child = command(&exe, &args)
            .stdout(std::process::Stdio::piped())
            .spawn()
            .unwrap();
        let mut out = child.stdout.take().unwrap();
        let pid = child.id().unwrap();
        let mut buf = vec![0u8; 4096];
        let _ = out.read(&mut buf).await.unwrap();
        suspend(pid).unwrap();
        // Drain whatever was already buffered, then expect silence while suspended
        let _ =
            tokio::time::timeout(std::time::Duration::from_millis(300), out.read(&mut buf)).await;
        let silent =
            tokio::time::timeout(std::time::Duration::from_millis(2500), out.read(&mut buf)).await;
        assert!(silent.is_err(), "no output while suspended");
        resume(pid).unwrap();
        let more =
            tokio::time::timeout(std::time::Duration::from_secs(5), out.read(&mut buf)).await;
        assert!(
            more.is_ok_and(|n| n.unwrap() > 0),
            "output again after resume"
        );
        child.start_kill().unwrap();
    }
}
