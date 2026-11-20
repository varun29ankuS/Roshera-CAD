//! Child processes die with the api-server.
//!
//! ## The defect this forecloses
//!
//! Every agent session runs its MCP server as a child process
//! (`node roshera-mcp/dist/index.js`, spawned by goose). goose kills those
//! children when a session or connection ends, but only while the
//! api-server is alive to do it. When the server process itself is killed
//! (Task Manager, `taskkill /F`, the OS low-memory reaper, a crash),
//! Windows does NOT kill its children: they are re-parented to nothing and
//! keep running, holding memory, until the machine is rebooted or someone
//! finds them by hand.
//!
//! ## The mechanism (Windows)
//!
//! At startup, before anything is spawned, the server creates a Job Object
//! with `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE` and assigns itself to it.
//! Every process it spawns afterwards inherits the job, and the job does
//! not permit breakaway. Nothing spawns with `CREATE_BREAKAWAY_FROM_JOB`:
//! goose starts each MCP server through rmcp's `TokioChildProcess` (a
//! `process-wrap` command with no `JobObject` wrapper), setting only
//! `CREATE_NO_WINDOW` (`vendor/goose/crates/goose/src/subprocess.rs`); this
//! crate's own spawns set no creation flags at all. The server holds the only
//! handle to the job, created non-inheritable so no child can keep it
//! open, and never closes it. When the server process ends, however it
//! ends, the kernel closes that handle, the job's last handle is gone, and
//! the kernel terminates every process still in the job.
//!
//! Windows 8 and later support nested jobs, so a server launched from a
//! terminal, IDE or service host that already put it in a job can still
//! be assigned to its own. If assignment is refused (an older OS, or a
//! parent job whose limits forbid it), the server logs a warning and boots
//! anyway: losing this guarantee is not a reason to refuse to serve.
//!
//! ## Other platforms
//!
//! Nothing yet. Linux would use `prctl(PR_SET_PDEATHSIG)` in each child
//! (between fork and exec), which needs the spawn sites' cooperation; that
//! is out of scope here, so [`bind_child_processes_to_server_lifetime`]
//! reports [`ChildLifetimeBinding::Unsupported`].

/// What [`bind_child_processes_to_server_lifetime`] achieved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ChildLifetimeBinding {
    /// This process is in a kill-on-close job only it holds a handle to:
    /// every child it spawns from now on dies when it does.
    Bound,
    /// An earlier call already bound this process; nothing was changed.
    AlreadyBound,
    /// The job could not be created, configured or joined. Children
    /// spawned from now on may outlive the server.
    NotBound {
        /// The step that failed and the OS error code.
        reason: String,
    },
    /// No mechanism is implemented on this platform.
    #[cfg_attr(windows, allow(dead_code))] // constructed only off Windows
    Unsupported,
}

/// Binds every child process this process spawns from now on to this
/// process's lifetime (see the module docs). Call it once, at startup,
/// before anything is spawned. Never fails the caller: the outcome is
/// logged and returned.
pub(crate) fn bind_child_processes_to_server_lifetime() -> ChildLifetimeBinding {
    let outcome = platform::bind();
    match &outcome {
        ChildLifetimeBinding::Bound => tracing::info!(
            target: "child_process_lifetime",
            "child processes are bound to the server's lifetime (kill-on-close job object)"
        ),
        ChildLifetimeBinding::AlreadyBound => {}
        ChildLifetimeBinding::NotBound { reason } => tracing::warn!(
            target: "child_process_lifetime",
            %reason,
            "could not bind child processes to the server's lifetime; agent MCP \
             server processes may outlive a killed server"
        ),
        ChildLifetimeBinding::Unsupported => tracing::info!(
            target: "child_process_lifetime",
            "child-process lifetime binding is not implemented on this platform"
        ),
    }
    outcome
}

#[cfg(windows)]
mod platform {
    use super::ChildLifetimeBinding;
    use std::sync::{Mutex, OnceLock};
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;

    /// The job handle, kept for the life of the process and never closed:
    /// closing the last handle to a kill-on-close job terminates every
    /// process in it, this one included. Stored as an address because a
    /// raw `HANDLE` is not `Send`/`Sync`; it is only ever read back for
    /// the `AlreadyBound` check.
    static JOB: OnceLock<usize> = OnceLock::new();

    /// Serializes binding so two concurrent first calls cannot both
    /// create a job.
    static BINDING: Mutex<()> = Mutex::new(());

    pub(super) fn bind() -> ChildLifetimeBinding {
        let _serialized = BINDING
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if JOB.get().is_some() {
            return ChildLifetimeBinding::AlreadyBound;
        }

        // SAFETY: both pointers may be null. A null security-attributes
        // pointer gives the default descriptor and a NON-inheritable handle,
        // which is required here: an inherited copy held by a child would
        // keep the job open after the server died. A null name creates an
        // unnamed job no other process can open.
        let job: HANDLE = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if job.is_null() {
            return not_bound("CreateJobObjectW");
        }

        // SAFETY: `JOBOBJECT_EXTENDED_LIMIT_INFORMATION` is a plain C struct
        // of integers; all-zero is a valid value meaning "no limits".
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
        // SAFETY: `job` is the valid job handle created above; the pointer
        // and size describe `limits`, a live, correctly typed value for the
        // `JobObjectExtendedLimitInformation` class, read only for the
        // duration of the call.
        let configured = unsafe {
            SetInformationJobObject(
                job,
                JobObjectExtendedLimitInformation,
                std::ptr::from_ref(&limits).cast(),
                size,
            )
        };
        if configured == 0 {
            let outcome = not_bound("SetInformationJobObject");
            close(job);
            return outcome;
        }

        // SAFETY: `job` is valid; `GetCurrentProcess` returns a pseudo
        // handle to this process that needs no closing.
        let assigned = unsafe { AssignProcessToJobObject(job, GetCurrentProcess()) };
        if assigned == 0 {
            // Refused (e.g. a parent job that forbids nesting): this
            // process is not in the job, so closing the handle kills
            // nothing.
            let outcome = not_bound("AssignProcessToJobObject");
            close(job);
            return outcome;
        }

        // Held for the life of the process: see `JOB`.
        let _ = JOB.set(job as usize);
        ChildLifetimeBinding::Bound
    }

    fn not_bound(step: &str) -> ChildLifetimeBinding {
        // SAFETY: reads this thread's last-error value; no preconditions.
        let code = unsafe { GetLastError() };
        ChildLifetimeBinding::NotBound {
            reason: format!("{step} failed (OS error {code})"),
        }
    }

    /// Closes a job handle this process is NOT a member of.
    fn close(job: HANDLE) {
        // SAFETY: `job` is a valid handle owned by the caller and not used
        // again after this call.
        unsafe {
            CloseHandle(job);
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::ChildLifetimeBinding;

    // Not implemented: on Linux each child would set
    // `prctl(PR_SET_PDEATHSIG, SIGKILL)` between fork and exec, which needs
    // the spawn sites (goose's extension spawn) to cooperate. Out of scope
    // for now.
    pub(super) fn bind() -> ChildLifetimeBinding {
        ChildLifetimeBinding::Unsupported
    }
}

#[cfg(all(test, windows))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    /// Set (to a scratch directory) only in the re-executed helper.
    const HELPER_DIR_ENV: &str = "ROSHERA_CHILD_LIFETIME_HELPER_DIR";
    const HELPER_TEST: &str = "child_process_lifetime::tests::helper_binds_and_spawns_a_grandchild";
    const OUTCOME_FILE: &str = "outcome.txt";
    const GRANDCHILD_PID_FILE: &str = "grandchild.pid";
    const STARTUP_DEADLINE: Duration = Duration::from_secs(60);
    const EXIT_DEADLINE: Duration = Duration::from_secs(15);

    fn process_alive(pid: u32) -> bool {
        let Ok(output) = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH", "/FO", "CSV"])
            .output()
        else {
            return false;
        };
        String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
    }

    /// `taskkill /F` without `/T`: terminates exactly this process
    /// (`TerminateProcess`), never its tree.
    fn kill_process(pid: u32) {
        let _ = Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    /// Kills every PID it holds when dropped, so a failing assertion never
    /// leaves the helper or the grandchild running.
    struct Reap {
        pids: Vec<u32>,
        dir: PathBuf,
    }

    impl Drop for Reap {
        fn drop(&mut self) {
            for pid in &self.pids {
                kill_process(*pid);
            }
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    fn read_when_present(path: &Path, deadline: Instant) -> Option<String> {
        loop {
            if let Ok(text) = std::fs::read_to_string(path) {
                if !text.is_empty() {
                    return Some(text);
                }
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    /// Helper mode: a no-op in a normal test run. Re-executed by
    /// [`killed_server_takes_its_children_with_it`] with [`HELPER_DIR_ENV`]
    /// set, it is the "server": it binds exactly as `main` does, spawns a
    /// long-lived grandchild, records its PID, and waits to be killed.
    #[test]
    fn helper_binds_and_spawns_a_grandchild() {
        let Some(dir) = std::env::var_os(HELPER_DIR_ENV).map(PathBuf::from) else {
            return;
        };
        let outcome = bind_child_processes_to_server_lifetime();
        std::fs::write(dir.join(OUTCOME_FILE), format!("{outcome:?}")).unwrap();
        // Spawned directly (no `cmd` in between), so the PID is the
        // sleeping process itself. ~10 minutes: only a kill ends it early.
        let mut grandchild = Command::new("ping")
            .args(["-n", "600", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let pid_path = dir.join(GRANDCHILD_PID_FILE);
        let partial = dir.join("grandchild.pid.partial");
        std::fs::write(&partial, grandchild.id().to_string()).unwrap();
        std::fs::rename(&partial, &pid_path).unwrap();
        // Blocks until the outer test kills this helper (or ping ends).
        let _ = grandchild.wait();
    }

    /// A server killed outright (`TerminateProcess`, what Task Manager and
    /// `taskkill /F` do) must not leave its children running.
    #[test]
    fn killed_server_takes_its_children_with_it() {
        let dir =
            std::env::temp_dir().join(format!("roshera-child-lifetime-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut reap = Reap {
            pids: Vec::new(),
            dir: dir.clone(),
        };

        let exe = std::env::current_exe().unwrap();
        let mut helper = Command::new(exe)
            .args([HELPER_TEST, "--exact", "--nocapture", "--test-threads=1"])
            .env(HELPER_DIR_ENV, &dir)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let helper_pid = helper.id();
        reap.pids.push(helper_pid);

        let deadline = Instant::now() + STARTUP_DEADLINE;
        let grandchild_pid: u32 = read_when_present(&dir.join(GRANDCHILD_PID_FILE), deadline)
            .expect("the helper never reported its grandchild's PID")
            .trim()
            .parse()
            .unwrap();
        reap.pids.push(grandchild_pid);
        let outcome = std::fs::read_to_string(dir.join(OUTCOME_FILE)).unwrap();
        assert_eq!(
            outcome,
            format!("{:?}", ChildLifetimeBinding::Bound),
            "the production binding must succeed in the helper, or this test \
             proves nothing"
        );
        assert!(
            process_alive(grandchild_pid),
            "grandchild {grandchild_pid} must be running before the kill"
        );
        eprintln!("helper pid {helper_pid}, grandchild pid {grandchild_pid}");

        // TerminateProcess on the helper alone, as Task Manager does.
        helper.kill().unwrap();
        let _ = helper.wait();

        let deadline = Instant::now() + EXIT_DEADLINE;
        while process_alive(grandchild_pid) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
        }
        assert!(
            !process_alive(grandchild_pid),
            "grandchild {grandchild_pid} outlived its killed parent {helper_pid}"
        );
    }
}
