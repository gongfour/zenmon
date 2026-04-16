pub mod app;
pub mod event;
pub mod views;

use app::{App, ConnectionState, QueryStatus};
use color_eyre::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use dotori_core::config::DotoriConfig;
use dotori_core::types::ZenohMessage;
use event::{AppEvent, EventHandler};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use zenoh::Session;

pub async fn run(mut config: DotoriConfig, tick_rate_ms: u64) -> Result<()> {
    let endpoint = config.endpoint.clone();
    let mut app = App::new(endpoint);
    app.scout_port_current = config.scout_port;

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
        let _ = execute!(
            std::io::stdout(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );
        original_hook(info);
    }));

    let mut events = EventHandler::new(tick_rate_ms, zenoh_rx);

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
    execute!(
        std::io::stdout(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;

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

const REDRAW_INTERVAL_MS: u64 = 66;
const MAX_PENDING_EVENTS_PER_BATCH: usize = 512;

fn spawn_connect(config: DotoriConfig, tx: mpsc::UnboundedSender<ConnectResult>) {
    tokio::spawn(async move {
        match dotori_core::session::open_session(&config).await {
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

fn spawn_scout_task(
    config: DotoriConfig,
    tx: mpsc::UnboundedSender<AppEvent>,
    timeout: Duration,
) {
    tokio::spawn(async move {
        let _ = tx.send(AppEvent::ScoutStarted);
        let now = SystemTime::now();
        match dotori_core::scout::scout(&config, timeout).await {
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

fn spawn_port_scan_task(config: DotoriConfig, tx: mpsc::UnboundedSender<AppEvent>) {
    tokio::spawn(async move {
        let _ = tx.send(AppEvent::PortScanStarted);
        match dotori_core::scout::scout_port_range(
            &config,
            7446,
            7546,
            Duration::from_secs(1),
        )
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
            match dotori_core::registry::query_admin_nodes(&sess).await {
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

fn spawn_liveliness_subscriber(
    session: &Session,
    tx: mpsc::UnboundedSender<AppEvent>,
) {
    let (liveliness_tx, mut liveliness_rx) =
        mpsc::unbounded_channel::<dotori_core::types::LivelinessEvent>();

    let session = session.clone();
    tokio::spawn(async move {
        if let Err(e) =
            dotori_core::discover::subscribe_liveliness(&session, "**", liveliness_tx).await
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

async fn run_loop(
    terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    app: &mut App,
    events: &mut EventHandler,
    session: &Arc<Mutex<Option<Session>>>,
    config: &mut DotoriConfig,
    zenoh_tx: &mpsc::UnboundedSender<ZenohMessage>,
    conn_tx: &mpsc::UnboundedSender<ConnectResult>,
    conn_rx: &mut mpsc::UnboundedReceiver<ConnectResult>,
    query_tx: &mpsc::UnboundedSender<QueryResult>,
    query_rx: &mut mpsc::UnboundedReceiver<QueryResult>,
) -> Result<()> {
    let mut refresh_interval = tokio::time::interval(Duration::from_secs(5));
    refresh_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut redraw_interval = tokio::time::interval(Duration::from_millis(REDRAW_INTERVAL_MS));
    redraw_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut reconnect_pending = true;
    let mut needs_redraw = true;

    let tx = events.sender();
    spawn_admin_polling_task(session.clone(), tx.clone());
    spawn_scout_task(config.clone(), tx.clone(), Duration::from_secs(3));

    loop {
        if let Some(key_expr) = app.pending_query.take() {
            if let Some(s) = session.lock().await.as_ref() {
                app.query_status = QueryStatus::Running;
                let s = s.clone();
                let tx = query_tx.clone();
                let ke = key_expr.clone();
                tokio::spawn(async move {
                    match dotori_core::query::get(&s, &ke, None, Duration::from_secs(5)).await {
                        Ok(results) => {
                            let _ = tx.send(QueryResult::Ok(results));
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

        if let Some(new_port) = app.pending_reconnect_port.take() {
            config.scout_port = Some(new_port);
            *session.lock().await = None;
            app.connection_state = ConnectionState::Connecting;
            reconnect_pending = true;
            spawn_connect(config.clone(), conn_tx.clone());
            needs_redraw = true;
        }

        if needs_redraw || app.toast.is_some() {
            terminal.draw(|frame| app.render(frame))?;
            needs_redraw = false;
        }

        tokio::select! {
            event = events.next() => {
                app.handle_event(event?);
                drain_pending_events(events, app, MAX_PENDING_EVENTS_PER_BATCH)?;
                needs_redraw = true;
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
                        let _ = dotori_core::subscriber::subscribe(&s, "**", zenoh_tx.clone()).await;
                        spawn_liveliness_subscriber(&s, tx.clone());
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

fn drain_pending_events(events: &mut EventHandler, app: &mut App, limit: usize) -> Result<()> {
    for _ in 0..limit {
        let Some(event) = events.try_next()? else {
            break;
        };
        app.handle_event(event);
    }
    Ok(())
}
