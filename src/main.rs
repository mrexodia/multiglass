mod config;
mod daemon;
mod identity;
mod paths;
mod relay;
mod stream;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    name = "multiglass",
    about = "Stream terminal sessions through one local relay to the hub"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Store your upstream hub + session key.
    Config {
        upstream: String,
        key: String,
        /// Local port for the relay's hub + control API, if the default (47890) is taken.
        #[arg(long)]
        port: Option<u16>,
    },
    /// Start the local relay in the background.
    Start,
    /// Stop the local relay.
    Stop,
    /// Wrap this tab's shell and push it to the local relay.
    Stream {
        /// Name this session yourself instead of auto-capturing/generating an
        /// id — lets you `multiglass switch <slug>` it from anywhere.
        slug: Option<String>,
        /// Don't switch the upstream hub to this session immediately —
        /// register and mirror locally, but leave whatever's currently live
        /// alone until you `multiglass switch` it yourself.
        #[arg(long)]
        no_switch: bool,
        /// Interactive command to mirror; put it last, after `--`. Defaults to
        /// $SHELL on Unix or %COMSPEC% on Windows.
        #[arg(
            last = true,
            num_args = 1..,
            allow_hyphen_values = true,
            value_name = "CMD"
        )]
        command: Vec<String>,
    },
    /// Tell the relay to upstream this session now.
    Switch {
        /// Which session to switch to. Defaults to $MULTIGLASS_SLUG (set by
        /// `stream` in the shell you're running this from), then to
        /// $ITERM_SESSION_ID — pass this explicitly to switch to a tab you're
        /// not currently inside (e.g. from automation).
        slug: Option<String>,
    },
    /// Show where we're streaming to and what's locally attached.
    Status,
    #[command(hide = true, name = "__relay-server")]
    RelayServer,
}

#[derive(serde::Deserialize)]
struct StatusEntry {
    slug: String,
    cwd: Option<String>,
    command: Option<String>,
    attached: bool,
    active: bool,
}

async fn status() -> Result<()> {
    let cfg = config::Config::load()?;
    let base = cfg.local_base();
    println!("upstream: {}", cfg.upstream_url);
    println!("relay:    {base}");
    println!();

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("{base}/multiglass/status"))
        .send()
        .await
        .with_context(|| {
            format!("calling the local relay at {base} — is `multiglass start` running?")
        })?;
    if !resp.status().is_success() {
        anyhow::bail!(
            "status failed: {} {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
    }
    let entries: Vec<StatusEntry> = resp.json().await.context("parsing status response")?;
    if entries.is_empty() {
        println!("no sessions attached");
        return Ok(());
    }
    for e in entries {
        let marker = if e.active {
            "*"
        } else if e.attached {
            " "
        } else {
            "?" // registered but no pusher connected (e.g. stream exited)
        };
        println!(
            "{marker} {:<20} {:<15} {}",
            e.slug,
            e.command.as_deref().unwrap_or("-"),
            e.cwd.as_deref().unwrap_or("-"),
        );
    }
    Ok(())
}

/// Call the relay's explicit switch endpoint.
async fn call_switch(base: &str, slug: &str) -> Result<()> {
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base}/multiglass/switch"))
        .json(&serde_json::json!({ "slug": slug }))
        .send()
        .await
        .with_context(|| {
            format!("calling the local relay at {base} — is `multiglass start` running?")
        })?;
    if !resp.status().is_success() {
        anyhow::bail!(
            "switch failed: {} {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
    }
    Ok(())
}

async fn switch(slug: Option<String>) -> Result<()> {
    let slug = identity::resolve_slug(slug)?;
    let base = config::Config::load()?.local_base();
    call_switch(&base, &slug).await
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Config {
            key,
            upstream,
            port,
        } => config::login(key, upstream, port),
        Command::Start => daemon::start(),
        Command::Stop => daemon::stop(),
        Command::Stream {
            slug,
            no_switch,
            command,
        } => stream::run(slug, no_switch, command).await,
        Command::Switch { slug } => switch(slug).await,
        Command::Status => status().await,
        Command::RelayServer => relay::run().await,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stream_command_after_separator_is_not_consumed_as_slug() {
        let cli = Cli::try_parse_from(["multiglass", "stream", "--", "powershell.exe", "-NoLogo"])
            .unwrap();

        let Command::Stream { slug, command, .. } = cli.command else {
            panic!("expected stream command");
        };
        assert_eq!(slug, None);
        assert_eq!(command, ["powershell.exe", "-NoLogo"]);
    }

    #[test]
    fn stream_still_accepts_a_positional_slug_before_command() {
        let cli =
            Cli::try_parse_from(["multiglass", "stream", "work", "--", "cmd.exe", "/d"]).unwrap();

        let Command::Stream { slug, command, .. } = cli.command else {
            panic!("expected stream command");
        };
        assert_eq!(slug.as_deref(), Some("work"));
        assert_eq!(command, ["cmd.exe", "/d"]);
    }
}
