//! Process control for long-running pipeline processes: every child of a job goes into one
//! group that is killed together (a Windows Job Object with kill-on-close, so nothing keeps
//! running even if the app crashes), with no console window and below-normal priority.

use std::ffi::OsString;
use std::path::Path;

use tokio::process::{Child, Command};

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
    cmd
}

/// The processes of one job.
pub struct ProcessGroup {
    #[cfg(windows)]
    job: windows::Job,
}

impl ProcessGroup {
    pub fn new() -> std::io::Result<Self> {
        Ok(Self {
            #[cfg(windows)]
            job: windows::Job::new()?,
        })
    }

    /// Adds a freshly spawned child (and the processes it starts later).
    pub fn add(&self, child: &Child) -> std::io::Result<()> {
        #[cfg(windows)]
        if let Some(handle) = child.raw_handle() {
            self.job.assign(handle)?;
        }
        #[cfg(not(windows))]
        let _ = child;
        Ok(())
    }

    /// Kills every process in the group.
    pub fn kill(&self) {
        #[cfg(windows)]
        self.job.terminate();
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
        #[cfg(not(windows))]
        child.start_kill().unwrap();
        let status = tokio::time::timeout(std::time::Duration::from_secs(10), child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(!status.success());
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
