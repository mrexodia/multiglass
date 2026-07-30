//! Resolves "which locally-attached session is this" without requiring
//! iTerm2: `stream` captures or generates an identity and exports it into the
//! shell it wraps (`$MULTIGLASS_SLUG`), so anything run inside that shell —
//! including a bare `multiglass switch` — finds it automatically, no matter
//! what terminal you're in. iTerm2's `$ITERM_SESSION_ID` is used
//! opportunistically when present (purely so repeated `stream` invocations in
//! the same tab reuse one identity instead of piling up registrations); an
//! explicit argument always wins, for driving a tab you're not inside (the
//! iTerm2 focus-watcher automation).

use anyhow::{Result, bail};

pub const SLUG_ENV: &str = "MULTIGLASS_SLUG";

fn iterm_slug() -> Option<String> {
    let raw = std::env::var("ITERM_SESSION_ID").ok()?;
    Some(raw.rsplit(':').next().unwrap_or(&raw).to_string())
}

fn random_slug() -> Result<String> {
    use std::io::Read as _;
    let mut file = std::fs::File::open("/dev/urandom")?;
    let mut buf = [0u8; 8];
    file.read_exact(&mut buf)?;
    Ok(buf.iter().map(|b| format!("{b:02x}")).collect())
}

/// The ambient identity of the shell we're running in, if any: `$MULTIGLASS_SLUG`
/// (set by an enclosing `stream`) or, failing that, iTerm2's session GUID.
fn ambient_slug() -> Option<String> {
    std::env::var(SLUG_ENV).ok().or_else(iterm_slug)
}

/// The identity `stream` should register under: an explicit slug you named
/// yourself (`multiglass stream myslug`) wins outright; otherwise this reuses
/// `$MULTIGLASS_SLUG` if already set (a nested stream), else iTerm2's session
/// GUID if present, else mints a fresh one — so this works the same outside
/// iTerm2, in tmux, over plain SSH, wherever.
pub fn attach_slug(explicit: Option<String>) -> Result<String> {
    if let Some(slug) = explicit {
        return Ok(slug);
    }
    if let Some(slug) = ambient_slug() {
        return Ok(slug);
    }
    random_slug()
}

/// Resolve which session a command like `switch` should target: an explicit
/// argument (driving a tab you're not inside) beats the identity of the
/// shell you're actually typing in.
pub fn resolve_slug(explicit: Option<String>) -> Result<String> {
    if let Some(slug) = explicit {
        return Ok(slug);
    }
    if let Some(slug) = ambient_slug() {
        return Ok(slug);
    }
    bail!(
        "can't tell which session this is — pass a slug explicitly, or run this from inside `multiglass stream`"
    )
}
