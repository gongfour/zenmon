use color_eyre::Result;
use zenoh::Session;

pub async fn run(_session: Session, _tick_rate_ms: u64) -> Result<()> {
    tracing::info!("TUI not yet implemented");
    Ok(())
}
