use anyhow::{Context, Result};
use std::path::PathBuf;

fn state_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME").context("HOME is not set")?;
    let dir = PathBuf::from(home).join(".config").join("multiglass");
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
