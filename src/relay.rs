//! The background daemon started by `multiglass start`.
//!
//! Runs two things side by side:
//!   - a local shellglass hub (127.0.0.1-only) that each `multiglass stream`
//!     pushes into, one session per iTerm2 tab;
//!   - exactly ONE upstream `push` connection to the real hub, fed by
//!     whichever local session is currently "switched to" — switching is a
//!     [`shellglass::source::FramePublisher::switch_source`] call, which
//!     forces a full-frame resync, so the upstream connection and its public
//!     view URL never restart when you switch tabs.
//!
//! A tiny control API (`/multiglass/register`, `/multiglass/switch`) is
//! merged onto the same router: `stream` calls the first once at startup (and
//! the second immediately after, unless `--no-switch`), the iTerm2
//! focus-watcher calls the second on every tab switch.

use anyhow::{Context, Result};
use axum::extract::State;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use shellglass::api::{Presentation, PushOptions, push};
use shellglass::hub::{AllowConfig, HubState};
use shellglass::model::{Color, Frame, Grid, StyledCell};
use shellglass::source::{FramePublisher, external_source};
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

/// What `stream` reported about itself at register time — cosmetic, purely
/// for `multiglass status`; the hub itself never needs to know this.
#[derive(Clone, Serialize)]
struct SessionMeta {
    cwd: String,
    command: String,
}

#[derive(Clone)]
struct RelayState {
    hub: HubState,
    publisher: FramePublisher,
    follower: Arc<Mutex<Option<JoinHandle<()>>>>,
    active_slug: Arc<Mutex<Option<String>>>,
    meta: Arc<Mutex<HashMap<String, SessionMeta>>>,
}

fn placeholder_frame(text: &str) -> Frame {
    Frame::Screen(Grid {
        source_epoch: 0,
        cols: text.chars().count() as u16,
        rows: vec![
            text.chars()
                .map(|ch| StyledCell {
                    text: ch.to_string(),
                    ..Default::default()
                })
                .collect(),
        ],
        cursor: None,
        cursor_style: 0,
        default_colors: (Color::Default, Color::Default),
        title: "multiglass".into(),
        links: Default::default(),
        images: Vec::new(),
        image_data: Default::default(),
    })
}

#[derive(Deserialize)]
struct RegisterBody {
    id: String,
    slug: String,
    cwd: String,
    command: String,
}

async fn register(
    State(state): State<RelayState>,
    Json(body): Json<RegisterBody>,
) -> Result<(), (axum::http::StatusCode, String)> {
    use shellglass::hub::AddError;
    // Re-attaching in the same tab reuses the same slug with a fresh id —
    // drop the stale registration first rather than erroring.
    if let Err(AddError::SlugTaken) = state.hub.add_session(&body.id, Some(&body.slug)) {
        state.hub.remove_by_slug(&body.slug);
        state
            .hub
            .add_session(&body.id, Some(&body.slug))
            .map_err(|e| (axum::http::StatusCode::BAD_REQUEST, format!("{e:?}")))?;
    }
    state.meta.lock().await.insert(
        body.slug,
        SessionMeta {
            cwd: body.cwd,
            command: body.command,
        },
    );
    Ok(())
}

#[derive(Deserialize)]
struct SwitchBody {
    slug: String,
}

async fn switch(
    State(state): State<RelayState>,
    Json(body): Json<SwitchBody>,
) -> Result<(), (axum::http::StatusCode, String)> {
    let Some(live) = state.hub.live(&body.slug) else {
        return Err((
            axum::http::StatusCode::NOT_FOUND,
            format!("no attached session for {:?}", body.slug),
        ));
    };

    let mut follower = state.follower.lock().await;
    if let Some(task) = follower.take() {
        task.abort();
    }

    // Resync immediately, then keep following this session's updates until
    // the next switch (or process exit) replaces this task.
    state.publisher.switch_source((*live.frame()).clone());
    let publisher = state.publisher.clone();
    let mut ticks = live.ticks();
    *follower = Some(tokio::spawn(async move {
        while ticks.recv().await.is_ok() {
            publisher.publish((*live.frame()).clone());
        }
    }));
    *state.active_slug.lock().await = Some(body.slug);
    Ok(())
}

#[derive(Serialize)]
struct StatusEntry {
    slug: String,
    cwd: Option<String>,
    command: Option<String>,
    /// A pusher is currently connected for this slug.
    attached: bool,
    /// This is the slug currently being forwarded to the real hub.
    active: bool,
}

async fn status(State(state): State<RelayState>) -> Json<Vec<StatusEntry>> {
    let meta = state.meta.lock().await;
    let active = state.active_slug.lock().await.clone();
    Json(
        state
            .hub
            .list_sessions()
            .into_iter()
            .map(|s| {
                let m = meta.get(&s.slug);
                StatusEntry {
                    active: active.as_deref() == Some(s.slug.as_str()),
                    slug: s.slug,
                    cwd: m.map(|m| m.cwd.clone()),
                    command: m.map(|m| m.command.clone()),
                    attached: s.live,
                }
            })
            .collect(),
    )
}

fn control_router(state: RelayState) -> Router {
    Router::new()
        .route("/multiglass/register", post(register))
        .route("/multiglass/switch", post(switch))
        .route("/multiglass/status", get(status))
        .with_state(state)
}

pub async fn run() -> Result<()> {
    let cfg = crate::config::Config::load()?;
    let local_base = cfg.local_base();

    let hub_state = HubState::new(AllowConfig::default(), local_base.clone());
    let (publisher, source) =
        external_source(placeholder_frame("multiglass: no tab activated yet"));
    let relay_state = RelayState {
        hub: hub_state.clone(),
        publisher: publisher.clone(),
        follower: Arc::new(Mutex::new(None)),
        active_slug: Arc::new(Mutex::new(None)),
        meta: Arc::new(Mutex::new(HashMap::new())),
    };

    let app = shellglass::hub::app_with_cors(hub_state, &[]).merge(control_router(relay_state));
    let addr = format!("127.0.0.1:{}", cfg.local_port);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .with_context(|| {
            format!("binding local hub on {addr} — is `multiglass start` already running?")
        })?;
    println!("multiglass: local hub listening on {local_base}");

    let serve = axum::serve(
        listener,
        app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    );
    let presentation = Presentation::load(None)?;
    let upstream = push(
        move || Ok(source),
        presentation,
        PushOptions::new(cfg.upstream_url, cfg.upstream_key),
    );

    tokio::select! {
        result = serve => { result.context("local hub server exited")?; }
        result = upstream => { result.context("upstream push exited")?; }
        _ = shutdown_signal() => { println!("multiglass: shutting down"); }
    }
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = signal(SignalKind::terminate()).expect("installing SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = ctrl_c.await;
    }
}
