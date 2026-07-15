use crate::config::ZenmonConfig;
use color_eyre::eyre::eyre;
use color_eyre::Result;
use zenoh::Session;

/// Open a Zenoh session from ZenmonConfig.
pub async fn open_session(config: &ZenmonConfig) -> Result<Session> {
    let zenoh_config = config.to_zenoh_config()?;
    let session = zenoh::open(zenoh_config).await.map_err(|e| eyre!(e))?;
    tracing::info!(zid = %session.zid(), "Zenoh session opened");
    Ok(session)
}
