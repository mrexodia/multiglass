use anyhow::{Context, Result};
use std::path::PathBuf;

#[cfg(unix)]
fn state_dir_root() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".config"))
}

#[cfg(windows)]
fn state_dir_root() -> Result<PathBuf> {
    // APPDATA is the native per-user roaming config directory and, unlike
    // HOME, is present in regular cmd.exe and PowerShell environments.
    let appdata = std::env::var_os("APPDATA").context("APPDATA is not set")?;
    Ok(PathBuf::from(appdata))
}

fn state_dir() -> Result<PathBuf> {
    let dir = state_dir_root()?.join("multiglass");
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    Ok(dir)
}

pub fn config_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("config.json"))
}

pub fn pid_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("relay.pid"))
}

pub fn log_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("relay.log"))
}

pub fn sessions_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("local-sessions.json"))
}

pub fn active_session_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("active-session"))
}
