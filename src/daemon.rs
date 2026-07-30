//! `start`/`stop` process lifecycle for the relay daemon (`relay::run`).
//!
//! Deliberately simple: re-exec ourselves with a hidden subcommand, redirect
//! stdio to a log file, record the child pid. `stop` just signals that pid.

use anyhow::{Context, Result, bail};
use std::process::{Command, Stdio};

/// Hidden subcommand the daemonized child runs (see `main.rs`).
pub const RELAY_SERVER_ARG: &str = "__relay-server";

fn read_pid() -> Option<u32> {
    let path = crate::paths::pid_path().ok()?;
    let text = std::fs::read_to_string(path).ok()?;
    text.trim().parse().ok()
}

fn is_alive(pid: u32) -> bool {
    // Signal 0: no-op existence check (see kill(2)).
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn start() -> Result<()> {
    if let Some(pid) = read_pid()
        && is_alive(pid)
    {
        bail!("multiglass is already running (pid {pid}); `multiglass stop` first");
    }

    let exe = std::env::current_exe().context("locating multiglass's own binary")?;
    let log_path = crate::paths::log_path()?;
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .with_context(|| format!("opening {}", log_path.display()))?;
    let log_err = log.try_clone().context("cloning log file handle")?;

    let child = Command::new(exe)
        .arg(RELAY_SERVER_ARG)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log))
        .stderr(Stdio::from(log_err))
        .spawn()
        .context("spawning the relay daemon")?;

    std::fs::write(crate::paths::pid_path()?, child.id().to_string()).context("writing pidfile")?;
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
    } else {
        Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .status()
            .context("sending SIGTERM")?;
        println!("multiglass: stopped (pid {pid})");
    }
    let _ = std::fs::remove_file(crate::paths::pid_path()?);
    Ok(())
}
