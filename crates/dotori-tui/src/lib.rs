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

    // Try to connect, but don't fail if we can't
    let session: Arc<Mutex<Option<Session>>> = Arc::new(Mutex::new(None));

    match dotori_core::session::open_session(&config).await {
        Ok(s) => {
            app.connection_state =
                ConnectionState::Connected(format!("{}", s.zid()));
            app.topics = dotori_core::discover::discover(&s, "**")
                .await
                .unwrap_or_default();
            app.nodes = dotori_core::registry::list_nodes(&s)
                .await
                .unwrap_or_default();
            *session.lock().await = Some(s);
        }
        Err(e) => {
            let reason = format!("{}", e).chars().take(60).collect::<String>();
            app.connection_state = ConnectionState::Disconnected(reason);
        }
    }

    // Set up Zenoh message channel
    let (zenoh_tx, zenoh_rx) = mpsc::unbounded_channel::<ZenohMessage>();

    // Start subscriber if connected
    if let Some(s) = session.lock().await.as_ref() {
        let _ = dotori_core::subscriber::subscribe(s, "**", zenoh_tx.clone()).await;
    }

    let mut terminal = ratatui::init();
    let mut events = EventHandler::new(tick_rate_ms, zenoh_rx);

    let result = run_loop(&mut terminal, &mut app, &mut events, &session, &config, &zenoh_tx).await;

    ratatui::restore();

    if let Some(s) = session.lock().await.take() {
        let _ = s.close().await;
    }

    result
}

async fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    events: &mut EventHandler,
    session: &Arc<Mutex<Option<Session>>>,
    config: &DotoriConfig,
    zenoh_tx: &mpsc::UnboundedSender<ZenohMessage>,
) -> Result<()> {
    let mut refresh_interval = tokio::time::interval(std::time::Duration::from_secs(5));

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
            _ = refresh_interval.tick() => {
                // If disconnected, try to reconnect
                if !app.is_connected() {
                    app.connection_state = ConnectionState::Connecting;
                    match dotori_core::session::open_session(config).await {
                        Ok(s) => {
                            app.connection_state = ConnectionState::Connected(format!("{}", s.zid()));
                            app.topics = dotori_core::discover::discover(&s, "**").await.unwrap_or_default();
                            app.nodes = dotori_core::registry::list_nodes(&s).await.unwrap_or_default();
                            let _ = dotori_core::subscriber::subscribe(&s, "**", zenoh_tx.clone()).await;
                            *session.lock().await = Some(s);
                        }
                        Err(e) => {
                            let reason = format!("{}", e).chars().take(60).collect::<String>();
                            app.connection_state = ConnectionState::Disconnected(reason);
                        }
                    }
                } else if let Some(s) = session.lock().await.as_ref() {
                    // Refresh topics and nodes
                    if let Ok(topics) = dotori_core::discover::discover(s, "**").await {
                        app.topics = topics;
                    }
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
