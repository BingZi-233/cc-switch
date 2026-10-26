use super::*;

/// Own only this runtime's process tree. Windows job handles are closed by the
/// OS even if the main application crashes; Unix children also watch pipe EOF.
pub(super) struct ProcessTree {
    #[cfg(unix)]
    pid: i32,
    #[cfg(windows)]
    job: isize,
}

impl ProcessTree {
    pub(super) fn attach(child: &Child) -> Result<Self> {
        #[cfg(unix)]
        {
            Ok(Self {
                pid: child.id().ok_or("Plugin process exited")? as i32,
            })
        }
        #[cfg(windows)]
        unsafe {
            use windows_sys::Win32::{Foundation::CloseHandle, System::JobObjects::*};
            let process = child.raw_handle().ok_or("Plugin process exited")?;
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return Err("Could not create plugin process job".into());
            }
            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            if SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                &limits as *const _ as _,
                std::mem::size_of_val(&limits) as u32,
            ) == 0
                || AssignProcessToJobObject(job, process as _) == 0
            {
                CloseHandle(job);
                return Err("Could not isolate plugin process job".into());
            }
            Ok(Self { job: job as isize })
        }
    }

    pub(super) fn kill(&self) {
        #[cfg(unix)]
        unsafe {
            libc::kill(-self.pid, libc::SIGKILL);
        }
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::JobObjects::TerminateJobObject(self.job as _, 1);
        }
    }
}

impl Drop for ProcessTree {
    fn drop(&mut self) {
        self.kill();
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::Foundation::CloseHandle(self.job as _);
        }
    }
}
