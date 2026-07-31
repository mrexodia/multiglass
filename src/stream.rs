//! `multiglass stream` — wraps the current tab exactly like `shellglass push`
//! does today, except it pushes to the local relay (`multiglass start`)
//! instead of a real hub. The key is throwaway and local-only: nothing here
//! ever leaves 127.0.0.1.
//!
//! Every tab is meant to run this unconditionally, relay running or not: the
//! shell itself starts immediately regardless of relay reachability, while
//! registration waits in parallel. The push connection starts only after the
//! relay has accepted its local key, avoiding a register-vs-push authentication
//! race. Showing the terminal is the point; streaming is a bonus.

use crate::identity;
use anyhow::Result;
use shellglass::api::{Presentation, PushOptions, push};
use shellglass::proto::session_id;
use shellglass::pty;
use std::time::Duration;

/// Backoff between registration attempts while waiting for the local relay to
/// come up — mirrors shellglass's own reconnect cadence for consistency.
const RELAY_WAIT_BACKOFF: Duration = Duration::from_millis(500);

fn local_key() -> Result<String> {
    let mut buf = [0u8; 32];
    getrandom::fill(&mut buf)
        .map_err(|e| anyhow::anyhow!("generating a local session key: {e}"))?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

#[cfg(unix)]
fn default_shell() -> String {
    std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".into())
}

#[cfg(windows)]
fn default_shell() -> String {
    // COMSPEC is always a native Windows path. SHELL is often absent, or may
    // be an MSYS path that CreateProcess/ConPTY cannot resolve.
    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into())
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

    let presentation = Presentation::load(None)?;
    // Start the shell before waiting for the relay. Holding a SourceSession is
    // enough to keep the terminal bridge alive; its newest-frame watch channel
    // safely replaces intermediate frames until the pusher consumes it.
    let source = pty::start(&command, false, true)?;
    let mut source_frames = source.frames.clone();

    // Registration and the shell run concurrently, but the local `/push` is
    // deliberately not attempted until registration succeeds. Previously both
    // requests raced whenever a relay started, and `/push` could win and receive
    // a fatal 403 for its not-yet-known throwaway key.
    tokio::select! {
        _ = register_with_relay(
            base.clone(),
            id,
            slug.clone(),
            cwd,
            command.join(" "),
            no_switch,
        ) => {}
        _ = async {
            while source_frames.changed().await.is_ok() {}
        } => return Ok(()),
    }

    let mut options = PushOptions::new(base, key);
    // The source is already running, so have `push` take ownership immediately
    // rather than invoking its normal connect-before-source gate.
    options.eager_start = true;
    push(move || Ok(source), presentation, options).await
}

/// Register this session with the relay and, unless `no_switch`, switch it
/// live. Retried forever without affecting the already-running terminal: local
/// transport failures and a relay that's still starting are both transient from
/// the stream's point of view.
async fn register_with_relay(
    base: String,
    id: String,
    slug: String,
    cwd: String,
    command: String,
    no_switch: bool,
) {
    let client = reqwest::Client::new();
    let body = serde_json::json!({
        "id": id,
        "slug": slug,
        "cwd": cwd,
        "command": command,
        "activate": !no_switch,
    });
    loop {
        let registered = client
            .post(format!("{base}/multiglass/register"))
            .json(&body)
            .send()
            .await
            .is_ok_and(|resp| resp.status().is_success());
        if registered {
            break;
        }
        tokio::time::sleep(RELAY_WAIT_BACKOFF).await;
    }
}
