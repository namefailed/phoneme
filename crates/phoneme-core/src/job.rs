//! Windows Job Object wrapper for kill-on-close process lifetimes.
//!
//! A [`KillOnCloseJob`] is the OS-level safety net behind the shutdown chain:
//! every process assigned to it is terminated by the kernel the moment the
//! job's last handle closes — which happens automatically when the process
//! holding that handle exits, however it exits (clean quit, panic, or Task
//! Manager's End task). The daemon holds one job for its spawned children
//! (whisper-server(s), a Phoneme-launched Ollama); the tray optionally holds
//! one for the daemon itself, so end-processing the tray takes the whole tree
//! down.
//!
//! Membership is decided at spawn time: Windows has no way to remove a
//! process from a kill-on-close job later, so "don't tie the daemon to the
//! tray" (`interface.quit_stops_daemon = false`) must be known before the
//! daemon is spawned. Children of a job member join the job automatically,
//! which is what makes the tray → daemon → children chain transitive.

use std::io;
use std::os::windows::io::RawHandle;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};

/// An anonymous Job Object configured with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`.
///
/// Dropping it (or the owning process dying) closes the job handle, and the
/// kernel terminates every process still assigned. Keep it alive for as long
/// as the assigned processes should be allowed to run.
#[derive(Debug)]
pub struct KillOnCloseJob {
    handle: HANDLE,
}

// SAFETY: the job handle is a kernel object reference; Windows allows it to be
// used from any thread, and the only operations exposed (`assign_raw` and the
// closing drop) are individually thread-safe kernel calls.
unsafe impl Send for KillOnCloseJob {}
unsafe impl Sync for KillOnCloseJob {}

impl KillOnCloseJob {
    /// Create the job and set the kill-on-close limit.
    ///
    /// Errors carry the underlying OS error (e.g. policy restrictions); the
    /// callers treat a failure as "no safety net" and continue without one
    /// rather than refusing to run.
    pub fn new() -> io::Result<Self> {
        // SAFETY: CreateJobObjectW with null attributes/name creates a fresh
        // anonymous job; a null return signals failure.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Self { handle };

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        // SAFETY: `info` is a properly-initialized JOBOBJECT_EXTENDED_LIMIT_
        // INFORMATION and the size argument matches it exactly.
        let ok = unsafe {
            SetInformationJobObject(
                job.handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const std::ffi::c_void,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };
        if ok == 0 {
            // `job` drops here and closes the half-configured handle.
            return Err(io::Error::last_os_error());
        }
        Ok(job)
    }

    /// Assign a process (by its raw process handle) to this job.
    ///
    /// Use the handle of a `std::process::Child` (`AsRawHandle`) or a
    /// `tokio::process::Child` (`raw_handle()`). Fails if the handle lacks
    /// `PROCESS_SET_QUOTA | PROCESS_TERMINATE` rights or, pre-Windows 8, if
    /// the process is already in an incompatible job. Callers log and carry
    /// on — a missing safety net must never abort a spawn that succeeded.
    pub fn assign_raw(&self, process: RawHandle) -> io::Result<()> {
        // SAFETY: both handles are valid for the duration of the call; the
        // kernel validates rights and job compatibility.
        let ok = unsafe { AssignProcessToJobObject(self.handle, process as HANDLE) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for KillOnCloseJob {
    fn drop(&mut self) {
        // SAFETY: `handle` was returned by CreateJobObjectW and is closed
        // exactly once. Closing the last handle fires the kill-on-close limit.
        unsafe { CloseHandle(self.handle) };
    }
}

#[cfg(test)]
mod tests {
    use super::KillOnCloseJob;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::time::{Duration, Instant};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    /// The end-to-end OS contract: a child assigned to the job dies when the
    /// job's last handle closes. This is the exact mechanism that reaps the
    /// daemon's children after an unclean daemon death.
    #[test]
    fn closing_the_job_kills_an_assigned_child() {
        let job = KillOnCloseJob::new().expect("create job object");

        // A child that would otherwise outlive this test by far.
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "ping -n 60 127.0.0.1 >NUL"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("spawn long-running cmd child");

        job.assign_raw(child.as_raw_handle())
            .expect("assign child to job");
        assert!(
            child.try_wait().expect("try_wait").is_none(),
            "child should still be running right after assignment"
        );

        drop(job); // last handle closes → kernel terminates the member

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if child.try_wait().expect("try_wait").is_some() {
                return; // reaped — the kill-on-close limit fired
            }
            if Instant::now() >= deadline {
                let _ = child.kill(); // don't leak the child on failure
                panic!("child survived the job close — kill-on-close did not fire");
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// A process that already exited can't be assigned — the call errors
    /// instead of succeeding silently, so callers can log the real cause.
    #[test]
    fn assigning_a_dead_process_errors() {
        let job = KillOnCloseJob::new().expect("create job object");
        let mut child = std::process::Command::new("cmd")
            .args(["/c", "exit 0"])
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("spawn short-lived cmd child");
        child.wait().expect("child exits");
        assert!(job.assign_raw(child.as_raw_handle()).is_err());
    }
}
