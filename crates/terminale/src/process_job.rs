//! Confine the process tree to a Windows Job Object so the ConPTY console
//! host (`OpenConsole.exe`) can never outlive `terminale`.
//!
//! # The problem
//!
//! Each terminal tab/pane runs its shell through a pseudo-console (ConPTY).
//! Windows backs every pseudo-console with an `OpenConsole.exe` host process.
//! On a **clean** close [`crate::Session`]'s `Drop` impl reaps the shell and
//! calls `ClosePseudoConsole`, which lets the host exit. But on a **hard**
//! exit — a force-kill (`taskkill /F`), a panic that aborts, a GPU-driver
//! crash, a power event — `Drop` never runs, so the host is orphaned. An
//! orphaned `OpenConsole.exe` whose pipe peer has vanished busy-loops at
//! ~100 % of one core, and they accumulate one per killed instance.
//!
//! # The fix
//!
//! At startup we create a Job Object with
//! `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and assign **our own process** to it.
//! Child processes inherit job membership, so every shell and every
//! `OpenConsole.exe` we spawn afterwards joins the same job. The flag tells
//! the kernel to terminate every process still in the job the moment the last
//! handle to the job closes — and when `terminale` exits for *any* reason the
//! OS closes all its handles, including the one we hold here. The console
//! hosts therefore die with us no matter how we go down.
//!
//! The job handle is intentionally kept open for the entire process lifetime
//! (parked in a `OnceLock`). Closing it while we are still a member would trip
//! the kill-on-close limit and terminate us too, so we never close it
//! explicitly; process teardown does it for us.
//!
//! This is the same pattern Chromium and `node-pty` use to keep helper
//! processes from leaking. It is a no-op on non-Windows targets.

#[cfg(target_os = "windows")]
mod imp {
    use std::sync::OnceLock;

    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_BREAKAWAY_OK, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    /// Holds the job handle open for the whole process lifetime. Stored as a
    /// raw pointer width (`isize`) because `HANDLE` is not `Send`/`Sync`; we
    /// only ever store it and never touch it again, so this is sound.
    static JOB: OnceLock<isize> = OnceLock::new();

    pub fn confine_to_job() {
        // Idempotent: only the first call sets up the job.
        if JOB.get().is_some() {
            return;
        }

        // SAFETY: `CreateJobObjectW(NULL, NULL)` creates an unnamed,
        // default-security job; it returns NULL on failure, which we check.
        let job: HANDLE = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            tracing::warn!(
                "process_job: CreateJobObjectW failed; ConPTY hosts may outlive a crash"
            );
            return;
        }

        // Configure kill-on-close: terminate all job members when the last
        // handle to the job (ours) is closed — i.e. when this process dies.
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION =
            // SAFETY: an all-zero JOBOBJECT_EXTENDED_LIMIT_INFORMATION is a
            // valid "no limits" baseline; we then OR in the one flag we want.
            unsafe { std::mem::zeroed() };
        // KILL_ON_JOB_CLOSE: reap every member when our handle closes (process
        // exit). BREAKAWAY_OK: let a child that *explicitly* asks
        // (CREATE_BREAKAWAY_FROM_JOB) leave the job — used by the MSI updater,
        // which must outlive us to finish installing. Shells and ConPTY hosts
        // never ask, so they stay confined.
        info.BasicLimitInformation.LimitFlags =
            JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE | JOB_OBJECT_LIMIT_BREAKAWAY_OK;

        let ok = unsafe {
            // SAFETY: `job` is a live handle; `&info` is a correctly-typed,
            // correctly-sized struct matching `JobObjectExtendedLimitInformation`.
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::addr_of!(info).cast(),
                u32::try_from(std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
                    .unwrap_or(u32::MAX),
            )
        };
        if ok == 0 {
            tracing::warn!("process_job: SetInformationJobObject failed; not confining");
            // SAFETY: closing the job we just created and will not use. The
            // process is not yet a member, so this cannot kill us.
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(job);
            }
            return;
        }

        // Assign ourselves to the job. On Windows 8+ a process already in a
        // job can still join a nested one; if the existing job forbids it this
        // fails and we fall back to the old behaviour (logged, non-fatal).
        let assigned = unsafe {
            // SAFETY: `GetCurrentProcess` returns a pseudo-handle valid for
            // this call; `job` is the live handle from above.
            AssignProcessToJobObject(job, GetCurrentProcess())
        };
        if assigned == 0 {
            tracing::warn!(
                "process_job: AssignProcessToJobObject failed (already in a \
                 non-nestable job?); ConPTY hosts may outlive a crash"
            );
            unsafe {
                windows_sys::Win32::Foundation::CloseHandle(job);
            }
            return;
        }

        // Park the handle for the process lifetime — never closed explicitly.
        let _ = JOB.set(job as isize);
        tracing::info!("process_job: confined to kill-on-close job object");
    }
}

/// Confine this process to a kill-on-close Job Object so child processes
/// (shells and their ConPTY `OpenConsole.exe` hosts) are terminated by the OS
/// whenever this process exits — including hard exits where `Drop` never runs.
///
/// Call once, as early as possible in `main`. Idempotent. No-op off Windows.
pub fn confine_to_job() {
    #[cfg(target_os = "windows")]
    imp::confine_to_job();
}
