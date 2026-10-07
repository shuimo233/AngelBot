//! Own one child process and, on Windows, its descendant process tree.

use std::io;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus};

#[cfg(windows)]
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
#[cfg(windows)]
use std::os::windows::process::CommandExt;
#[cfg(windows)]
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::CREATE_NO_WINDOW;

/// The job handle stays alive even if the root process exits before its children.
/// Dropping the final handle terminates every process still in the job.
pub(crate) struct ProcessTree {
    child: Child,
    #[cfg(windows)]
    job: OwnedHandle,
}

impl ProcessTree {
    pub(crate) fn spawn(command: &mut Command) -> Result<Self, String> {
        #[cfg(windows)]
        {
            let job = create_kill_on_close_job()?;
            // AngelBot is a GUI app in release builds. A console child
            // (including cmd.exe for npx.cmd) must not open a separate window.
            command.creation_flags(CREATE_NO_WINDOW);
            let mut child = command.spawn().map_err(|error| error.to_string())?;

            // A newly spawned process can run before this call. Keep the gap as
            // small as possible and never accept an unassigned child.
            if unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) } == 0
            {
                let assignment_error = io::Error::last_os_error();
                let cleanup_error = match child.kill() {
                    Ok(()) => child.wait().err(),
                    Err(kill_error) => match child.try_wait() {
                        Ok(Some(_)) => None,
                        _ => Some(kill_error),
                    },
                };
                return Err(match cleanup_error {
                    Some(cleanup_error) => format!(
                        "Failed to assign child process to job: {assignment_error}; \
                         failed to reap unassigned process: {cleanup_error}"
                    ),
                    None => format!("Failed to assign child process to job: {assignment_error}"),
                });
            }

            Ok(Self { child, job })
        }

        #[cfg(not(windows))]
        {
            Ok(Self {
                child: command.spawn().map_err(|error| error.to_string())?,
            })
        }
    }

    pub(crate) fn take_stdin(&mut self) -> Option<ChildStdin> {
        self.child.stdin.take()
    }

    pub(crate) fn take_stdout(&mut self) -> Option<ChildStdout> {
        self.child.stdout.take()
    }

    pub(crate) fn take_stderr(&mut self) -> Option<ChildStderr> {
        self.child.stderr.take()
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.child.try_wait()
    }

    pub(crate) fn terminate_and_wait(&mut self) -> io::Result<()> {
        #[cfg(windows)]
        {
            // Killing only Child misses npx.cmd/cmd.exe or Node grandchildren.
            if unsafe { TerminateJobObject(self.job.as_raw_handle(), 1) } == 0 {
                return Err(io::Error::last_os_error());
            }
            self.child.wait()?;
            Ok(())
        }

        #[cfg(not(windows))]
        {
            match self.child.kill() {
                Ok(()) => {
                    self.child.wait()?;
                    Ok(())
                }
                Err(error) => match self.child.try_wait()? {
                    Some(_) => Ok(()),
                    None => Err(error),
                },
            }
        }
    }
}

#[cfg(windows)]
fn create_kill_on_close_job() -> Result<OwnedHandle, String> {
    let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if raw.is_null() {
        return Err(format!(
            "Failed to create child process job: {}",
            io::Error::last_os_error()
        ));
    }
    let job = unsafe { OwnedHandle::from_raw_handle(raw) };
    let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    if unsafe {
        SetInformationJobObject(
            job.as_raw_handle(),
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            std::mem::size_of_val(&limits) as u32,
        )
    } == 0
    {
        return Err(format!(
            "Failed to configure child process job: {}",
            io::Error::last_os_error()
        ));
    }
    Ok(job)
}

#[cfg(all(test, windows))]
mod tests {
    use super::ProcessTree;
    use std::fs;
    use std::io::{self, Write};
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
    };

    fn spawn_fixture() -> (ProcessTree, OwnedHandle, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("spawn-grandchild.cjs");
        let pid_file = dir.path().join("grandchild.pid");
        fs::write(
            &script,
            r#"
const fs = require('fs');
const { spawn } = require('child_process');
process.stdin.once('data', () => {
  const child = spawn(process.execPath, ['-e', 'setInterval(() => {}, 1000)'], {
    detached: true,
    stdio: 'ignore',
    windowsHide: true
  });
  child.unref();
  fs.writeFileSync(process.argv[2], String(child.pid) + '\n');
  setInterval(() => {}, 1000);
});
"#,
        )
        .unwrap();
        let mut command = Command::new("node");
        command
            .arg(script)
            .arg(&pid_file)
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let mut tree = ProcessTree::spawn(&mut command).unwrap();
        let mut stdin = tree.take_stdin().unwrap();
        stdin.write_all(b"go\n").unwrap();
        drop(stdin);

        let deadline = Instant::now() + Duration::from_secs(5);
        let pid = loop {
            if let Ok(pid) = fs::read_to_string(&pid_file) {
                if let Some(complete) = pid.strip_suffix('\n') {
                    break complete.parse::<u32>().unwrap();
                }
            }
            assert!(Instant::now() < deadline, "grandchild PID was not reported");
            thread::sleep(Duration::from_millis(10));
        };
        let raw = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        assert!(
            !raw.is_null(),
            "could not open grandchild process {pid}: {}",
            io::Error::last_os_error()
        );
        let grandchild = unsafe { OwnedHandle::from_raw_handle(raw) };
        assert_eq!(
            unsafe { WaitForSingleObject(grandchild.as_raw_handle(), 0) },
            WAIT_TIMEOUT,
            "grandchild exited before cleanup"
        );
        (tree, grandchild, dir)
    }

    fn kill_only_root(tree: &mut ProcessTree, grandchild: &OwnedHandle) {
        tree.child.kill().unwrap();
        tree.child.wait().unwrap();
        assert!(tree.try_wait().unwrap().is_some());
        assert_eq!(
            unsafe { WaitForSingleObject(grandchild.as_raw_handle(), 0) },
            WAIT_TIMEOUT,
            "root exit must leave the grandchild alive until job cleanup"
        );
    }

    #[test]
    fn terminating_job_kills_grandchild() {
        let (mut tree, grandchild, _dir) = spawn_fixture();
        tree.terminate_and_wait().unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(grandchild.as_raw_handle(), 5_000) },
            WAIT_OBJECT_0
        );
    }

    #[test]
    fn dropping_job_after_root_exit_kills_grandchild() {
        let (mut tree, grandchild, _dir) = spawn_fixture();
        kill_only_root(&mut tree, &grandchild);
        drop(tree);
        assert_eq!(
            unsafe { WaitForSingleObject(grandchild.as_raw_handle(), 5_000) },
            WAIT_OBJECT_0
        );
    }

    #[test]
    fn terminating_job_after_root_exit_kills_grandchild() {
        let (mut tree, grandchild, _dir) = spawn_fixture();
        kill_only_root(&mut tree, &grandchild);
        tree.terminate_and_wait().unwrap();
        assert_eq!(
            unsafe { WaitForSingleObject(grandchild.as_raw_handle(), 5_000) },
            WAIT_OBJECT_0
        );
    }
}
