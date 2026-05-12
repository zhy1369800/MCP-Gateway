use crate::error::AppError;

#[cfg(target_os = "windows")]
mod imp {
    use std::mem::zeroed;
    use std::ptr::null;
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, OpenProcess, PROCESS_SET_QUOTA, PROCESS_TERMINATE,
    };

    use crate::error::AppError;

    struct JobHandle(HANDLE);

    unsafe impl Send for JobHandle {}
    unsafe impl Sync for JobHandle {}

    impl Drop for JobHandle {
        fn drop(&mut self) {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    static GATEWAY_JOB: OnceLock<JobHandle> = OnceLock::new();

    pub fn enable_gateway_process_job() -> Result<(), AppError> {
        let job = ensure_gateway_job()?;
        let assigned = unsafe { AssignProcessToJobObject(job.0, GetCurrentProcess()) };
        if assigned == 0 {
            // The gateway may already run inside a supervisor job. Keep our job
            // handle anyway; child processes can still be assigned after spawn.
            return Err(last_os_error(
                "failed to assign gateway process to Windows job",
            ));
        }
        Ok(())
    }

    pub fn assign_child_to_gateway_job(pid: u32) -> Result<(), AppError> {
        let job = ensure_gateway_job()?;
        let process = unsafe { OpenProcess(PROCESS_SET_QUOTA | PROCESS_TERMINATE, 0, pid) };
        if process.is_null() {
            return Err(last_os_error(
                "failed to open child process for Windows job",
            ));
        }

        let assigned = unsafe { AssignProcessToJobObject(job.0, process) };
        let close_result = unsafe { CloseHandle(process) };
        if assigned == 0 {
            return Err(last_os_error(
                "failed to assign child process to Windows job",
            ));
        }
        if close_result == 0 {
            return Err(last_os_error("failed to close child process handle"));
        }
        Ok(())
    }

    fn ensure_gateway_job() -> Result<&'static JobHandle, AppError> {
        if let Some(job) = GATEWAY_JOB.get() {
            return Ok(job);
        }

        let job = create_gateway_job()?;
        let _ = GATEWAY_JOB.set(job);
        GATEWAY_JOB
            .get()
            .ok_or_else(|| AppError::Internal("failed to initialize Windows job".to_string()))
    }

    fn create_gateway_job() -> Result<JobHandle, AppError> {
        let handle = unsafe { CreateJobObjectW(null(), null()) };
        if handle.is_null() {
            return Err(last_os_error("failed to create Windows job"));
        }

        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { zeroed() };
        info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = unsafe {
            SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                &info as *const _ as *const _,
                std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
            )
        };

        if ok == 0 {
            let error = last_os_error("failed to configure Windows job");
            unsafe {
                CloseHandle(handle);
            }
            return Err(error);
        }

        Ok(JobHandle(handle))
    }

    fn last_os_error(context: &str) -> AppError {
        AppError::Internal(format!("{context}: {}", std::io::Error::last_os_error()))
    }
}

#[cfg(not(target_os = "windows"))]
mod imp {
    use crate::error::AppError;

    pub fn enable_gateway_process_job() -> Result<(), AppError> {
        Ok(())
    }

    pub fn assign_child_to_gateway_job(_pid: u32) -> Result<(), AppError> {
        Ok(())
    }
}

pub fn enable_gateway_process_job() -> Result<(), AppError> {
    imp::enable_gateway_process_job()
}

pub fn assign_child_to_gateway_job(pid: u32) -> Result<(), AppError> {
    imp::assign_child_to_gateway_job(pid)
}
