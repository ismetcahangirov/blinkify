//! Making sure no `ffmpeg.exe` outlives Blinkify.
//!
//! On Windows, killing a parent does not kill its children. An `ffmpeg.exe`
//! left running after the editor exits — cleanly or not — keeps its input file
//! open, and the user experiences that as "Blinkify locked my file" long after
//! Blinkify is gone.
//!
//! Every sidecar process is therefore placed in one Windows **job object**
//! created with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`. Blinkify holds the only
//! handle to it and never closes it; when the Blinkify process ends for any
//! reason — a normal exit, a panic, Task Manager — the kernel closes the handle
//! and terminates every process in the job. No cleanup code has to run, which
//! is the point: an unclean shutdown is exactly when cleanup code does not.

use std::process::Child;

/// Put `child` in the kill-on-close job.
///
/// Returns whether it was adopted. A failure is not fatal — the process still
/// runs, and the orchestrator still kills it on cancel and on shutdown — but it
/// loses the guarantee for an unclean exit.
#[cfg(windows)]
pub(crate) fn adopt(child: &Child) -> bool {
    use std::os::windows::io::AsRawHandle;
    use std::sync::{Mutex, OnceLock, PoisonError};

    use win32job::{ExtendedLimitInfo, Job};

    static JOB: OnceLock<Option<Mutex<Job>>> = OnceLock::new();
    let job = JOB.get_or_init(|| {
        let mut limits = ExtendedLimitInfo::new();
        limits.limit_kill_on_job_close();
        Job::create_with_limit_info(&limits).ok().map(Mutex::new)
    });
    job.as_ref().is_some_and(|job| {
        job.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .assign_process(child.as_raw_handle() as isize)
            .is_ok()
    })
}

#[cfg(not(windows))]
pub(crate) fn adopt(_child: &Child) -> bool {
    false
}
