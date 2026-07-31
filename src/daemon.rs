//! `start`/`stop` process lifecycle for the relay daemon (`relay::run`).
//!
//! Deliberately simple: re-exec ourselves with a hidden subcommand, redirect
//! stdio to a log file, record the child pid, and stop that process on demand.

use anyhow::{Context, Result, bail};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

/// Hidden subcommand the daemonized child runs (see `main.rs`).
pub const RELAY_SERVER_ARG: &str = "__relay-server";

/// Fatal setup errors (bad key/protocol, occupied port, invalid presentation)
/// return almost immediately. Observe the child briefly so `start` can report
/// those errors instead of claiming that an already-dead daemon started.
const STARTUP_OBSERVE_TIMEOUT: Duration = Duration::from_secs(2);
const STARTUP_POLL_INTERVAL: Duration = Duration::from_millis(25);
const MAX_STARTUP_ERROR_CHARS: usize = 8 * 1024;

fn read_pid() -> Option<u32> {
    let path = crate::paths::pid_path().ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    text.trim().parse().ok()
}

#[cfg(unix)]
fn is_alive(pid: u32) -> bool {
    // Signal 0: no-op existence check (see kill(2)).
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(windows)]
mod windows_process {
    use anyhow::{Context, Result};
    use std::io;
    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_INVALID_PARAMETER, HANDLE, STILL_ACTIVE, WAIT_FAILED, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
        TerminateProcess, WaitForSingleObject,
    };

    // Standard process right required by WaitForSingleObject. windows-sys does
    // not expose the generic SYNCHRONIZE value in the Threading module.
    const SYNCHRONIZE: u32 = 0x0010_0000;

    struct ProcessHandle(HANDLE);

    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            // SAFETY: the handle came from a successful OpenProcess call and
            // this owner closes it exactly once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    fn open(pid: u32, access: u32) -> io::Result<ProcessHandle> {
        // SAFETY: OpenProcess has no pointer arguments and the returned handle
        // is wrapped immediately for cleanup.
        let handle = unsafe { OpenProcess(access, 0, pid) };
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(ProcessHandle(handle))
        }
    }

    fn exit_code(handle: &ProcessHandle) -> io::Result<u32> {
        let mut code = 0;
        // SAFETY: `code` is writable and `handle` remains valid for the call.
        if unsafe { GetExitCodeProcess(handle.0, &mut code) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(code)
        }
    }

    pub fn is_alive(pid: u32) -> bool {
        open(pid, PROCESS_QUERY_LIMITED_INFORMATION)
            .and_then(|handle| exit_code(&handle))
            .map(|code| code == STILL_ACTIVE as u32)
            .unwrap_or(false)
    }

    /// Returns false if the process exited between the liveness check and the
    /// attempt to terminate it.
    pub fn terminate(pid: u32) -> Result<bool> {
        let handle = match open(
            pid,
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | SYNCHRONIZE,
        ) {
            Ok(handle) => handle,
            Err(error) if error.raw_os_error() == Some(ERROR_INVALID_PARAMETER as i32) => {
                return Ok(false);
            }
            Err(error) => {
                return Err(error).with_context(|| format!("opening relay process {pid}"));
            }
        };
        if exit_code(&handle).context("checking relay process state")? != STILL_ACTIVE as u32 {
            return Ok(false);
        }

        // A detached Windows process cannot receive Unix's SIGTERM. End the
        // relay directly; all of its persistent state lives outside the process.
        // SAFETY: `handle` is valid and was opened with PROCESS_TERMINATE.
        if unsafe { TerminateProcess(handle.0, 0) } == 0 {
            return Err(io::Error::last_os_error()).context("terminating relay process");
        }
        // Avoid returning while the old process can still hold the local port.
        // SAFETY: `handle` remains valid for the duration of the wait.
        match unsafe { WaitForSingleObject(handle.0, 5_000) } {
            WAIT_OBJECT_0 => Ok(true),
            WAIT_TIMEOUT => anyhow::bail!("timed out waiting for relay process {pid} to stop"),
            WAIT_FAILED => {
                Err(io::Error::last_os_error()).context("waiting for relay process to stop")
            }
            result => anyhow::bail!("unexpected result {result} while waiting for relay process"),
        }
    }
}

#[cfg(windows)]
fn is_alive(pid: u32) -> bool {
    windows_process::is_alive(pid)
}

#[cfg(unix)]
fn terminate(pid: u32) -> Result<bool> {
    let status = Command::new("kill")
        .args(["-TERM", &pid.to_string()])
        .status()
        .context("sending SIGTERM")?;
    Ok(status.success())
}

#[cfg(windows)]
fn terminate(pid: u32) -> Result<bool> {
    windows_process::terminate(pid)
}

fn startup_log(log_path: &std::path::Path, offset: u64) -> Option<String> {
    let bytes = std::fs::read(log_path).ok()?;
    let start = usize::try_from(offset)
        .unwrap_or(usize::MAX)
        .min(bytes.len());
    let text = String::from_utf8_lossy(&bytes[start..]);
    // The log is normally entirely local/sanitized output. Strip any remaining
    // terminal control characters before echoing it into the caller's console.
    let clean: String = text
        .chars()
        .filter(|ch| !ch.is_control() || matches!(ch, '\n' | '\r' | '\t'))
        .collect();
    let clean = clean.trim();
    if clean.is_empty() {
        return None;
    }
    Some(
        clean
            .chars()
            .rev()
            .take(MAX_STARTUP_ERROR_CHARS)
            .collect::<String>()
            .chars()
            .rev()
            .collect(),
    )
}

fn startup_error(log_path: &std::path::Path, log_offset: u64, status: ExitStatus) -> anyhow::Error {
    let status = status
        .code()
        .map(|code| format!("exit code {code}"))
        .unwrap_or_else(|| "terminated by a signal".into());
    match startup_log(log_path, log_offset) {
        Some(details) => anyhow::anyhow!(
            "relay failed to start ({status}):\n\n{details}\n\nFull log: {}",
            log_path.display()
        ),
        None => anyhow::anyhow!(
            "relay failed to start ({status}); see {}",
            log_path.display()
        ),
    }
}

fn observe_startup(child: &mut Child, log_path: &std::path::Path, log_offset: u64) -> Result<()> {
    let deadline = Instant::now() + STARTUP_OBSERVE_TIMEOUT;
    loop {
        if let Some(status) = child
            .try_wait()
            .context("checking whether the relay started")?
        {
            return Err(startup_error(log_path, log_offset, status));
        }
        if Instant::now() >= deadline {
            return Ok(());
        }
        std::thread::sleep(STARTUP_POLL_INTERVAL);
    }
}

pub fn start() -> Result<()> {
    if let Some(pid) = read_pid()
        && is_alive(pid)
    {
        bail!("multiglass is already running (pid {pid}); `multiglass stop` first");
    }

    // Fail in the foreground with the same actionable message as the other
    // commands instead of spawning a relay that immediately exits into its log.
    crate::config::Config::load()?;

    let exe = std::env::current_exe().context("locating multiglass's own binary")?;
    let log_path = crate::paths::log_path()?;
    let log_offset = std::fs::metadata(&log_path).map(|m| m.len()).unwrap_or(0);
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening {}", log_path.display()))?;
    let log_err = log.try_clone().context("cloning log file handle")?;

    let mut command = Command::new(exe);
    command
        .arg(RELAY_SERVER_ARG)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err));
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        use windows_sys::Win32::System::Threading::{CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS};

        // Do not leave a console window behind when `start` is called from
        // Explorer, cmd.exe, or PowerShell. A new process group also prevents
        // Ctrl+C in the launching console from reaching the relay.
        command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
    }
    let mut child = command.spawn().context("spawning the relay daemon")?;
    let pid_path = crate::paths::pid_path()?;
    if let Err(error) = std::fs::write(&pid_path, child.id().to_string()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error).context("writing pidfile");
    }

    if let Err(error) = observe_startup(&mut child, &log_path, log_offset) {
        // Usually the child has already exited. If observation itself failed,
        // make sure we do not leave an untracked relay behind.
        let _ = child.kill();
        let _ = child.wait();
        let _ = std::fs::remove_file(pid_path);
        return Err(error);
    }
    println!(
        "multiglass: started (pid {}), logging to {}",
        child.id(),
        log_path.display()
    );
    Ok(())
}

pub fn stop() -> Result<()> {
    let Some(pid) = read_pid() else {
        println!("multiglass: not running");
        return Ok(());
    };
    if !is_alive(pid) {
        println!("multiglass: not running (stale pidfile)");
    } else if terminate(pid)? {
        println!("multiglass: stopped (pid {pid})");
    } else {
        println!("multiglass: not running (exited while stopping)");
    }
    let _ = std::fs::remove_file(crate::paths::pid_path()?);
    Ok(())
}
