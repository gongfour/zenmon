pub mod app;
pub mod event;
pub mod views;

use app::{App, ConnectionState};
use color_eyre::Result;
use dotori_core::config::DotoriConfig;
use dotori_core::types::ZenohMessage;
use event::EventHandler;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::Mutex;
use zenoh::Session;

pub async fn run(config: DotoriConfig, tick_rate_ms: u64) -> Result<()> {
    let endpoint = config.endpoint.clone();
    let mut app = App::new(endpoint);

    let session: Arc<Mutex<Option<Session>>> = Arc::new(Mutex::new(None));
    let (zenoh_tx, zenoh_rx) = mpsc::unbounded_channel::<ZenohMessage>();

    // Channel for background reconnection results
    let (conn_tx, mut conn_rx) = mpsc::unbounded_channel::<ConnectResult>();

    // Try initial connection in background (non-blocking)
    spawn_connect(config.clone(), conn_tx.clone());

    let mut terminal = ratatui::init();
    let mut events = EventHandler::new(tick_rate_ms, zenoh_rx);

    let result = run_loop(
        &mut terminal,
        &mut app,
        &mut events,
        &session,
        &config,
        &zenoh_tx,
        &conn_tx,
        &mut conn_rx,
    )
    .await;

    ratatui::restore();

    if let Some(s) = session.lock().await.take() {
        let _ = s.close().await;
    }

    result
}

enum ConnectResult {
    Connected(Session),
    Failed(String),
}

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

async fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    events: &mut EventHandler,
    session: &Arc<Mutex<Option<Session>>>,
    config: &DotoriConfig,
    zenoh_tx: &mpsc::UnboundedSender<ZenohMessage>,
    conn_tx: &mpsc::UnboundedSender<ConnectResult>,
    conn_rx: &mut mpsc::UnboundedReceiver<ConnectResult>,
) -> Result<()> {
    let mut refresh_interval = tokio::time::interval(std::time::Duration::from_secs(5));
    let mut reconnect_pending = true; // initial connect is in flight

    loop {
        terminal.draw(|frame| app.render(frame))?;

        // Execute pending query if connected
        if let Some(key_expr) = app.pending_query.take() {
            if let Some(s) = session.lock().await.as_ref() {
                match dotori_core::query::get(
                    s,
                    &key_expr,
                    None,
                    std::time::Duration::from_secs(5),
                )
                .await
                {
                    Ok(results) => app.query_results = results,
                    Err(e) => tracing::warn!(error = %e, "Query failed"),
                }
            }
        }

        tokio::select! {
            event = events.next() => {
                app.handle_event(event?);
            }
            // Handle background connection result (non-blocking)
            Some(result) = conn_rx.recv() => {
                reconnect_pending = false;
                match result {
                    ConnectResult::Connected(s) => {
                        app.connection_state = ConnectionState::Connected(format!("{}", s.zid()));
                        app.nodes = dotori_core::registry::list_nodes(&s).await.unwrap_or_default();
                        let _ = dotori_core::subscriber::subscribe(&s, "**", zenoh_tx.clone()).await;
                        *session.lock().await = Some(s);
                    }
                    ConnectResult::Failed(reason) => {
                        app.connection_state = ConnectionState::Disconnected(reason);
                    }
                }
            }
            _ = refresh_interval.tick() => {
                if !app.is_connected() && !reconnect_pending {
                    // Spawn reconnect in background — doesn't block the event loop
                    app.connection_state = ConnectionState::Connecting;
                    reconnect_pending = true;
                    spawn_connect(config.clone(), conn_tx.clone());
                } else if let Some(s) = session.lock().await.as_ref() {
                    if let Ok(nodes) = dotori_core::registry::list_nodes(s).await {
                        app.nodes = nodes;
                    }
                }
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}
