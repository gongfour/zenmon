use crate::config::ZemonConfig;
use crate::error::ZemonError;
use zenoh::Session;

/// Open a Zenoh session from ZemonConfig.
pub async fn open_session(config: &ZemonConfig) -> Result<Session, ZemonError> {
    let zenoh_config = config
        .to_zenoh_config()
        .map_err(|e| ZemonError::invalid_input(format!("invalid Zenoh config: {}", e)))?;
    let session = zenoh::open(zenoh_config)
        .await
        .map_err(|e| ZemonError::connection(format!("failed to open Zenoh session: {}", e)))?;
    tracing::info!(zid = %session.zid(), "Zenoh session opened");
    Ok(session)
}
