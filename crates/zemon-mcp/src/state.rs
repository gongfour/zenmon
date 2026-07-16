//! Server-wide state: resolved config + one shared, lazily-opened Zenoh session
//! reused across tool calls. A dropped session is reopened on the next call.

use std::sync::Arc;
use tokio::sync::Mutex;
use zemon_core::config::ZemonConfig;
use zemon_core::error::ZemonError;

pub struct ServerState {
    pub config: ZemonConfig,
    session: Mutex<Option<Arc<zenoh::Session>>>,
}

impl ServerState {
    pub fn new(config: ZemonConfig) -> Self {
        Self {
            config,
            session: Mutex::new(None),
        }
    }

    /// Return the shared session, opening it on first use. Callers that observe
    /// a connection error should call `invalidate_session` so the next call
    /// reopens.
    pub async fn session(&self) -> Result<Arc<zenoh::Session>, ZemonError> {
        let mut guard = self.session.lock().await;
        if let Some(s) = guard.as_ref() {
            return Ok(s.clone());
        }
        let session = zemon_core::session::open_session(&self.config).await?;
        let session = Arc::new(session);
        *guard = Some(session.clone());
        Ok(session)
    }

    /// Drop the cached session so the next `session()` reopens it.
    pub async fn invalidate_session(&self) {
        *self.session.lock().await = None;
    }
}
