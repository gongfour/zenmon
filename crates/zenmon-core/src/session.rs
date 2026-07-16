use crate::config::ZenmonConfig;
use crate::error::ZenmonError;
use zenoh::Session;

/// Open a Zenoh session from ZenmonConfig.
pub async fn open_session(config: &ZenmonConfig) -> Result<Session, ZenmonError> {
    let zenoh_config = config
        .to_zenoh_config()
        .map_err(|e| ZenmonError::invalid_input(format!("invalid Zenoh config: {}", e)))?;
    let session = zenoh::open(zenoh_config)
        .await
        .map_err(|e| ZenmonError::connection(format!("failed to open Zenoh session: {}", e)))?;
    tracing::info!(zid = %session.zid(), "Zenoh session opened");
    Ok(session)
}
