//! Persisted config: the upstream hub URL + this person's session key,
//! copy-pasted once from their `/me` page (`multiglass config <url> <key>`)
//! rather than any OAuth dance — they already have to install this CLI, so a
//! browser-gated page handing them a string to paste is no weaker than the
//! existing `shellglass push --key` instructions it replaces.

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::Write as _;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt as _;

const DEFAULT_LOCAL_PORT: u16 = 47890;

fn default_local_port() -> u16 {
    DEFAULT_LOCAL_PORT
}

#[derive(Serialize, Deserialize)]
pub struct Config {
    pub upstream_url: String,
    pub upstream_key: String,
    /// 127.0.0.1-only, so a fixed default is fine — but configurable per
    /// machine in case that port is already taken. `default` so a config
    /// written before this field existed still loads.
    #[serde(default = "default_local_port")]
    pub local_port: u16,
}

impl Config {
    pub fn local_base(&self) -> String {
        format!("http://127.0.0.1:{}", self.local_port)
    }

    pub fn load() -> Result<Self> {
        let path = crate::paths::config_path()?;
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => bail!(concat!(
                "multiglass is not configured yet.",
                "\n\nConfigure the upstream hub first:",
                "\n  multiglass config <url> <shellglass-key>"
            )),
            Err(error) => {
                return Err(error).with_context(|| format!("reading {}", path.display()));
            }
        };
        serde_json::from_slice(&bytes)
            .with_context(|| format!("parsing configuration at {}", path.display()))
    }

    pub fn save(&self) -> Result<()> {
        let path = crate::paths::config_path()?;
        let body = serde_json::to_vec_pretty(self)?;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .with_context(|| format!("writing {}", path.display()))?;
        file.write_all(&body)?;
        // The key is a bearer credential — same posture as the push key it wraps.
        // On Windows the file inherits the user's ACL from %APPDATA%. Unix needs
        // an explicit mode because the process umask may allow group/world access.
        #[cfg(unix)]
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        Ok(())
    }
}

pub fn login(key: String, upstream_url: String, local_port: Option<u16>) -> Result<()> {
    let cfg = Config {
        upstream_url,
        upstream_key: key,
        local_port: local_port.unwrap_or(DEFAULT_LOCAL_PORT),
    };
    cfg.save()?;
    println!(
        "multiglass: config saved to {}",
        crate::paths::config_path()?.display()
    );
    if crate::daemon::is_running() {
        println!(
            "multiglass: the running relay is still using the previous settings; \
             run `multiglass stop` and `multiglass start` to apply this config"
        );
    }
    Ok(())
}
