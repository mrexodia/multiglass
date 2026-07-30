//! `multiglass stream` — wraps the current tab exactly like `shellglass push`
//! does today, except it pushes to the local relay (`multiglass start`)
//! instead of a real hub. The key is throwaway and local-only: nothing here
//! ever leaves 127.0.0.1.

use crate::identity;
use anyhow::{Context, Result, bail};
use shellglass::api::{Presentation, PushOptions, push};
use shellglass::proto::session_id;
use shellglass::pty;

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

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/multiglass/register"))
        .json(&serde_json::json!({
            "id": id,
            "slug": slug,
            "cwd": cwd,
            "command": command.join(" "),
        }))
        .send()
        .await
        .with_context(|| {
            format!("registering with the local relay at {base} — is `multiglass start` running?")
        })?;
    if !resp.status().is_success() {
        bail!(
            "local relay rejected registration: {} {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
    }

    // Immediately go live unless told otherwise — the whole point of
    // `stream` is "this tab, now"; `--no-switch` is for wiring up several
    // tabs ahead of time and switching between them later with `switch`.
    if !no_switch {
        crate::call_switch(&base, &slug).await?;
    }

    let presentation = Presentation::load(None)?;
    push(
        move || pty::start(&command, false),
        presentation,
        PushOptions::new(base, key),
    )
    .await
}
