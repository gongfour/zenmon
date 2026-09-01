pub mod app;
pub mod diff;
pub mod event;
pub mod hangul;
pub mod history;
pub mod plot;
pub mod tree;
pub mod views;

use app::{App, ConnectionState, QueryStatus};
use color_eyre::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use event::{AppEvent, EventHandler};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use zenmon_core::config::ZenmonConfig;
use zenmon_core::types::ZenohMessage;
use zenoh::Session;

pub async fn run(
    mut config: ZenmonConfig,
    refresh: Duration,
    contract: Option<zenmon_core::contract::Contract>,
    allow_publish: bool,
) -> Result<()> {
    let endpoint = config.endpoint.clone();
    let mut app = App::new(endpoint);
    app.contract = contract;
    app.allow_publish = allow_publish;
    // A broken profiles file should not stop the dashboard from starting; it
    // is reported and the session runs without saved views.
    match zenmon_core::profile::load() {
        Ok(profiles) => app.profiles = profiles,
        Err(e) => app.set_error_toast(format!("Saved views unavailable: {e}")),
    }
    app.scout_port_current = config.scout_port;
    app.current_mode = config.mode;

    let session: Arc<Mutex<Option<Session>>> = Arc::new(Mutex::new(None));
    let (zenoh_tx, zenoh_rx) = mpsc::unbounded_channel::<ZenohMessage>();

    let (conn_tx, mut conn_rx) = mpsc::unbounded_channel::<ConnectResult>();
    let (query_tx, mut query_rx) = mpsc::unbounded_channel::<QueryResult>();

    spawn_connect(config.clone(), conn_tx.clone());

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture);
        original_hook(info);
    }));

    let mut events = EventHandler::new(refresh, zenoh_rx);

    let result = run_loop(
        &mut terminal,
        &mut app,
        &mut events,
        &session,
        &mut config,
        &zenoh_tx,
        &conn_tx,
        &mut conn_rx,
        &query_tx,
        &mut query_rx,
    )
    .await;

    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen, DisableMouseCapture)?;

    if let Some(s) = session.lock().await.take() {
        let _ = s.close().await;
    }

    result
}

enum ConnectResult {
    Connected(Session),
    Failed(String),
}

enum QueryResult {
    Ok(Vec<ZenohMessage>),
    Err(String),
}

/// Minimum spacing between full repaints (~15 fps). The event loop coalesces
/// whatever arrives inside one interval into a single frame.
const REDRAW_INTERVAL_MS: u64 = 66;
const MAX_PENDING_EVENTS_PER_BATCH: usize = 512;

/// Whether the event loop should paint this iteration.
///
/// Draining a batch already collapses a burst of messages into one frame, but a
/// stream arriving slower than a repaint takes still bought one full repaint per
/// message. Spacing frames by [`REDRAW_INTERVAL_MS`] bounds that, at the cost of
/// showing a value up to one frame late.
///
/// `immediate` opts out of the spacing: a keypress must never wait on the frame
/// budget, or the UI feels like it is lagging behind the keyboard.
fn should_draw(
    needs_redraw: bool,
    has_toast: bool,
    since_last_draw: Duration,
    immediate: bool,
) -> bool {
    if !needs_redraw && !has_toast {
        return false;
    }
    immediate || since_last_draw >= Duration::from_millis(REDRAW_INTERVAL_MS)
}

fn spawn_connect(config: ZenmonConfig, tx: mpsc::UnboundedSender<ConnectResult>) {
    tokio::spawn(async move {
        match zenmon_core::session::open_session(&config).await {
            Ok(s) => {
                let _ = tx.send(ConnectResult::Connected(s));
            }
            Err(e) => {
                let reason = format!("{}", e).chars().take(60).collect::<String>();
                let _ = tx.send(ConnectResult::Failed(reason));
            }
        }
    });
}

fn spawn_scout_task(config: ZenmonConfig, tx: mpsc::UnboundedSender<AppEvent>, timeout: Duration) {
    tokio::spawn(async move {
        let _ = tx.send(AppEvent::ScoutStarted);
        let now = SystemTime::now();
        match zenmon_core::scout::scout(&config, timeout).await {
            Ok(scouts) => {
                let nodes: Vec<_> = scouts.iter().map(|s| s.to_node_info(now)).collect();
                let _ = tx.send(AppEvent::ScoutNodes(nodes));
            }
            Err(e) => {
                tracing::warn!("scout failed: {}", e);
                let _ = tx.send(AppEvent::ScoutNodes(Vec::new()));
            }
        }
    });
}

fn spawn_doctor_task(config: ZenmonConfig, tx: mpsc::UnboundedSender<AppEvent>, timeout: Duration) {
    tokio::spawn(async move {
        let _ = tx.send(AppEvent::DoctorStarted);
        // `doctor::run` opens its own session internally, so no shared session
        // is needed here; mirror the scout/port-scan background-task pattern.
        let report = zenmon_core::doctor::run(&config, timeout).await;
        let _ = tx.send(AppEvent::DoctorReport(report));
    });
}

fn spawn_port_scan_task(config: ZenmonConfig, tx: mpsc::UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        let _ = tx.send(AppEvent::PortScanStarted);
        match zenmon_core::scout::scout_port_range(&config, 7446, 7546, Duration::from_secs(1))
            .await
        {
            Ok(results) => {
                let _ = tx.send(AppEvent::PortScanResults(results));
            }
            Err(e) => {
                tracing::warn!("port scan failed: {}", e);
                let _ = tx.send(AppEvent::PortScanResults(Vec::new()));
            }
        }
    });
}

fn spawn_admin_polling_task(
    session: Arc<Mutex<Option<Session>>>,
    tx: mpsc::UnboundedSender<AppEvent>,
) {
    tokio::spawn(async move {
        let mut ticker = tokio::time::interval(Duration::from_secs(2));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            ticker.tick().await;
            let sess = {
                let guard = session.lock().await;
                guard.as_ref().cloned()
            };
            let Some(sess) = sess else {
                continue;
            };
            match zenmon_core::registry::query_admin_nodes(&sess).await {
                Ok(nodes) => {
                    if tx.send(AppEvent::AdminNodes(nodes)).is_err() {
                        break;
                    }
                }
                Err(e) => {
                    tracing::warn!("admin query failed: {}", e);
                }
            }
        }
    });
}

fn spawn_liveliness_subscriber(session: &Session, tx: mpsc::UnboundedSender<AppEvent>) {
    let (liveliness_tx, mut liveliness_rx) =
        mpsc::unbounded_channel::<zenmon_core::types::LivelinessEvent>();

    let session = session.clone();
    tokio::spawn(async move {
        if let Err(e) =
            zenmon_core::discover::subscribe_liveliness(&session, "**", liveliness_tx).await
        {
            tracing::warn!("liveliness subscribe failed: {}", e);
        }
    });

    tokio::spawn(async move {
        while let Some(event) = liveliness_rx.recv().await {
            if tx.send(AppEvent::Liveliness(event)).is_err() {
                break;
            }
        }
    });
}

// Central event-loop coordinator: threading these handles through a struct
// would add indirection without clarifying the wiring, so allow the count.
#[allow(clippy::too_many_arguments)]
async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    events: &mut EventHandler,
    session: &Arc<Mutex<Option<Session>>>,
    config: &mut ZenmonConfig,
    zenoh_tx: &mpsc::UnboundedSender<ZenohMessage>,
    conn_tx: &mpsc::UnboundedSender<ConnectResult>,
    conn_rx: &mut mpsc::UnboundedReceiver<ConnectResult>,
    query_tx: &mpsc::UnboundedSender<QueryResult>,
    query_rx: &mut mpsc::UnboundedReceiver<QueryResult>,
) -> Result<()> {
    let mut refresh_interval = tokio::time::interval(Duration::from_secs(5));
    refresh_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Wakes the loop so a frame deferred by `should_draw` still gets painted
    // once the traffic that deferred it stops arriving.
    let mut redraw_interval = tokio::time::interval(Duration::from_millis(REDRAW_INTERVAL_MS));
    redraw_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut reconnect_pending = true;
    let mut needs_redraw = true;
    // Start one interval in the past so the first frame paints immediately.
    let mut last_draw = std::time::Instant::now() - Duration::from_millis(REDRAW_INTERVAL_MS);
    let mut draw_immediately = false;

    let tx = events.sender();
    // Owns the "**" stream subscription for as long as the session it belongs to.
    let mut stream_subscription: Option<zenmon_core::subscriber::Subscription> = None;
    spawn_admin_polling_task(session.clone(), tx.clone());
    spawn_scout_task(config.clone(), tx.clone(), Duration::from_secs(3));

    loop {
        if let Some(req) = app.pending_query.take() {
            if let Some(s) = session.lock().await.as_ref() {
                app.query_status = QueryStatus::Running;
                let s = s.clone();
                let tx = query_tx.clone();
                // `None` consolidation delivers every reply; the default keeps
                // one per key, which hides all but the fastest queryable when
                // several serve the same expression.
                let consolidation = if req.all_replies {
                    zenmon_core::query::ConsolidationMode::None
                } else {
                    zenmon_core::query::ConsolidationMode::Auto
                };
                tokio::spawn(async move {
                    match zenmon_core::query::get(
                        &s,
                        &req.key_expr,
                        req.payload.as_deref(),
                        Duration::from_secs(5),
                        None,
                        consolidation,
                    )
                    .await
                    {
                        Ok(outcome) => {
                            let _ = tx.send(QueryResult::Ok(outcome.replies));
                        }
                        Err(e) => {
                            let _ = tx.send(QueryResult::Err(format!("{}", e)));
                        }
                    }
                });
            } else {
                app.query_status = QueryStatus::Error("Not connected".to_string());
            }
        }

        if app.pending_scout_request {
            app.pending_scout_request = false;
            spawn_scout_task(config.clone(), tx.clone(), Duration::from_secs(3));
        }

        if app.pending_port_scan_request {
            app.pending_port_scan_request = false;
            spawn_port_scan_task(config.clone(), tx.clone());
        }

        if app.pending_doctor_request {
            app.pending_doctor_request = false;
            spawn_doctor_task(config.clone(), tx.clone(), Duration::from_secs(5));
        }

        // A publish armed in the editor. The session lives out here, so the
        // editor can only ever hand over an intent — it cannot write by itself.
        if let Some((key, payload)) = app.pending_publish.take() {
            let result = match session.lock().await.as_ref() {
                Some(s) => zenmon_core::publish::put(s, &key, payload.into_bytes(), None)
                    .await
                    .map(|()| key.clone())
                    .map_err(|e| format!("{e}")),
                None => Err("not connected".to_string()),
            };
            app.publish_result = Some(result);
            needs_redraw = true;
        }

        if let Some(new_port) = app.pending_reconnect_port.take() {
            config.scout_port = Some(new_port);
            *session.lock().await = None;
            app.connection_state = ConnectionState::Connecting;
            reconnect_pending = true;
            spawn_connect(config.clone(), conn_tx.clone());
            needs_redraw = true;
        }

        if let Some(new_mode) = app.pending_reconnect_mode.take() {
            config.set_mode(new_mode);
            app.current_mode = new_mode;
            app.clear_network_state();
            *session.lock().await = None;
            app.connection_state = ConnectionState::Connecting;
            reconnect_pending = true;
            spawn_connect(config.clone(), conn_tx.clone());
            needs_redraw = true;
        }

        if should_draw(
            needs_redraw,
            app.toast.is_some(),
            last_draw.elapsed(),
            draw_immediately,
        ) {
            terminal.draw(|frame| app.render(frame))?;
            needs_redraw = false;
            draw_immediately = false;
            last_draw = std::time::Instant::now();
        }

        tokio::select! {
            event = events.next() => {
                let event = event?;
                let mut saw_key = matches!(event, AppEvent::Key(_));
                app.handle_event(event);
                saw_key |= drain_pending_events(events, app, MAX_PENDING_EVENTS_PER_BATCH)?;
                needs_redraw = true;
                draw_immediately |= saw_key;
            }
            Some(result) = query_rx.recv() => {
                match result {
                    QueryResult::Ok(results) => {
                        let count = results.len();
                        app.query_results = results;
                        app.query_status = QueryStatus::Done(count);
                    }
                    QueryResult::Err(e) => {
                        app.query_status = QueryStatus::Error(e);
                    }
                }
                needs_redraw = true;
            }
            Some(result) = conn_rx.recv() => {
                reconnect_pending = false;
                match result {
                    ConnectResult::Connected(s) => {
                        let zid = format!("{}", s.zid());
                        app.connection_state = ConnectionState::Connected(zid.clone());
                        app.self_zid = Some(zid);
                        // Clear stale liveliness state before re-subscribing
                        app.liveliness_tokens.clear();
                        app.liveliness_events.clear();
                        app.liveliness_selected = 0;
                        app.liveliness_log_scroll = 0;
                        // Tear the previous stream subscription down explicitly;
                        // holding it keeps the pump alive across loop turns.
                        if let Some(previous) = stream_subscription.take() {
                            previous.stop().await;
                        }
                        stream_subscription =
                            zenmon_core::subscriber::subscribe(&s, "**", zenoh_tx.clone())
                                .await
                                .ok();
                        spawn_liveliness_subscriber(&s, tx.clone());
                        // Run diagnostics on every (re)connect so the header
                        // health dot reflects a fresh doctor report.
                        app.pending_doctor_request = true;
                        *session.lock().await = Some(s);
                    }
                    ConnectResult::Failed(reason) => {
                        app.connection_state = ConnectionState::Disconnected(reason);
                    }
                }
                needs_redraw = true;
            }
            _ = refresh_interval.tick() => {
                if !app.is_connected() && !reconnect_pending {
                    app.connection_state = ConnectionState::Connecting;
                    reconnect_pending = true;
                    spawn_connect(config.clone(), conn_tx.clone());
                    needs_redraw = true;
                }
            }
            _ = redraw_interval.tick() => {}
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}

/// Feed up to `limit` already-queued events into `app` without yielding, so a
/// burst collapses into one frame instead of one frame each.
///
/// Returns whether the batch contained a keypress, which the caller uses to
/// bypass the frame gate.
fn drain_pending_events(events: &mut EventHandler, app: &mut App, limit: usize) -> Result<bool> {
    let mut saw_key = false;
    for _ in 0..limit {
        let Some(event) = events.try_next()? else {
            break;
        };
        saw_key |= matches!(event, AppEvent::Key(_));
        app.handle_event(event);
    }
    Ok(saw_key)
}

#[cfg(test)]
mod tests {
    use super::{should_draw, REDRAW_INTERVAL_MS};
    use std::time::Duration;

    const PERIOD: Duration = Duration::from_millis(REDRAW_INTERVAL_MS);

    #[test]
    fn nothing_to_paint_never_draws() {
        // Even a long-idle loop stays dark when no state changed.
        assert!(!should_draw(false, false, Duration::from_secs(10), false));
        // …and an "immediate" request cannot conjure a frame out of nothing.
        assert!(!should_draw(false, false, Duration::from_secs(10), true));
    }

    #[test]
    fn pending_repaint_waits_out_the_frame_interval() {
        assert!(!should_draw(true, false, PERIOD / 2, false));
        assert!(should_draw(true, false, PERIOD, false));
        assert!(should_draw(true, false, PERIOD * 2, false));
    }

    #[test]
    fn keypress_bypasses_the_frame_interval() {
        // Typing must not wait on the frame budget, however recent the last
        // frame was — this is what keeps the UI feeling attached to the keyboard.
        assert!(should_draw(true, false, Duration::ZERO, true));
    }

    #[test]
    fn a_visible_toast_keeps_the_frame_gate() {
        // A toast redraws on its own so it can expire, but at the frame rate,
        // not once per loop iteration.
        assert!(!should_draw(false, true, PERIOD / 2, false));
        assert!(should_draw(false, true, PERIOD, false));
    }
}
