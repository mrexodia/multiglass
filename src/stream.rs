//! `multiglass stream` — wraps the current tab exactly like `shellglass push`
//! does today, except it pushes to the local relay (`multiglass start`)
//! instead of a real hub. The key is throwaway and local-only: nothing here
//! ever leaves 127.0.0.1.
//!
//! Every tab is meant to run this unconditionally, relay running or not: the
//! shell itself starts immediately regardless of relay reachability, and
//! registering + streaming happen entirely in the background, connecting (and
//! reconnecting) whenever the relay is there. Showing the terminal is the
//! point; streaming is a bonus the relay may or may not be around for.

use crate::identity;
use anyhow::{Context, Result};
use shellglass::api::{Presentation, PushOptions, push};
use shellglass::proto::session_id;
use shellglass::pty;
use std::time::Duration;

/// Backoff between registration attempts while waiting for the local relay to
/// come up — mirrors shellglass's own reconnect cadence for consistency.
const RELAY_WAIT_BACKOFF: Duration = Duration::from_millis(500);

fn local_key() -> Result<String> {
    use std::io::Read as _;
    let mut file = std::fs::File::open("/dev/urandom").context("opening /dev/urandom")?;
    let mut buf = [0u8; 32];
    file.read_exact(&mut buf).context("reading /dev/urandom")?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}

pub async fn run(slug: Option<String>, no_switch: bool, command: Vec<String>) -> Result<()> {
    let slug = identity::attach_slug(slug)?;
    // Exported into the wrapped shell's environment (portable_pty snapshots
    // the current process env when it builds the child command), so a bare
    // `multiglass switch` run from inside this very session finds itself —
    // no iTerm2, no manual slug, no argument needed.
    unsafe {
        std::env::set_var(identity::SLUG_ENV, &slug);
    }
    let key = local_key()?;
    let id = session_id(&key);
    let base = crate::config::Config::load()?.local_base();
    let command = if command.is_empty() {
        vec![default_shell()]
    } else {
        command
    };
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    // Tell the relay about this session in the background: retried forever
    // against a relay that isn't up (yet), entirely detached from the
    // terminal below — a relay that's down, rejects us, or never shows up
    // must never block or touch the shell the user is actually using.
    tokio::spawn(register_with_relay(
        base.clone(),
        id,
        slug.clone(),
        cwd,
        command.join(" "),
        no_switch,
    ));

    let mut options = PushOptions::new(base, key);
    // Start the shell now, not after the hub (here, the local relay) accepts
    // a connection — the terminal itself doesn't care whether anything's
    // streaming it. `pty::start`'s `passive` similarly keeps hub connectivity
    // from ever pausing/clearing the real terminal once it's live.
    options.eager_start = true;
    let presentation = Presentation::load(None)?;
    push(move || pty::start(&command, false, true), presentation, options).await
}

/// Register this session with the relay and, unless `no_switch`, switch it
/// live — retried forever against a relay that isn't up (yet). Runs detached
/// from the terminal, so any failure here (relay down, rejects us, etc.) just
/// means this tab never shows up as a `multiglass` session — never something
/// the wrapped shell's own terminal shows or is affected by.
async fn register_with_relay(
    base: String,
    id: String,
    slug: String,
    cwd: String,
    command: String,
    no_switch: bool,
) {
    let client = reqwest::Client::new();
    let body = serde_json::json!({ "id": id, "slug": slug, "cwd": cwd, "command": command });
    let resp = loop {
        match client
            .post(format!("{base}/multiglass/register"))
            .json(&body)
            .send()
            .await
        {
            Ok(resp) => break resp,
            Err(e) if e.is_connect() => tokio::time::sleep(RELAY_WAIT_BACKOFF).await,
            Err(_) => return, // non-transient transport failure: give up quietly
        }
    };
    if !resp.status().is_success() {
        return; // relay reachable but rejected us: give up quietly
    }
    if !no_switch {
        let _ = crate::call_switch(&base, &slug).await;
    }
}
