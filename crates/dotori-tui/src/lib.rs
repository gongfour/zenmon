pub mod app;
pub mod event;
pub mod views;

use app::App;
use color_eyre::eyre::eyre;
use color_eyre::Result;
use dotori_core::types::ZenohMessage;
use event::EventHandler;
use tokio::sync::mpsc;
use zenoh::Session;

pub async fn run(session: Session, tick_rate_ms: u64) -> Result<()> {
    let (zenoh_tx, zenoh_rx) = mpsc::unbounded_channel::<ZenohMessage>();

    let _sub_handle = dotori_core::subscriber::subscribe(&session, "**", zenoh_tx.clone()).await?;

    let connection_info = format!("zid:{}", session.zid());
    let mut app = App::new(connection_info);

    app.topics = dotori_core::discover::discover(&session, "**")
        .await
        .unwrap_or_default();
    app.nodes = dotori_core::registry::list_nodes(&session)
        .await
        .unwrap_or_default();

    let mut terminal = ratatui::init();
    let mut events = EventHandler::new(tick_rate_ms, zenoh_rx);

    let result = run_loop(&mut terminal, &mut app, &mut events, &session).await;

    ratatui::restore();
    session.close().await.map_err(|e| eyre!(e))?;
    result
}

async fn run_loop(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    events: &mut EventHandler,
    session: &Session,
) -> Result<()> {
    let mut refresh_interval = tokio::time::interval(std::time::Duration::from_secs(5));

    loop {
        terminal.draw(|frame| app.render(frame))?;

        if let Some(key_expr) = app.pending_query.take() {
            match dotori_core::query::get(
                session,
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

        tokio::select! {
            event = events.next() => {
                app.handle_event(event?);
            }
            _ = refresh_interval.tick() => {
                if let Ok(topics) = dotori_core::discover::discover(session, "**").await {
                    app.topics = topics;
                }
                if let Ok(nodes) = dotori_core::registry::list_nodes(session).await {
                    app.nodes = nodes;
                }
            }
        }

        if app.should_quit {
            break;
        }
    }
    Ok(())
}
