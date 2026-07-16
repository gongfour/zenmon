//! Server-wide state: resolved config + one shared, lazily-opened Zenoh session
//! reused across tool calls.
//!
//! v1 scope: the session is opened on first use and reused. `invalidate_session`
//! clears the cache so the next call reopens. The cache is invalidated when a
//! call fails with an explicit `ErrorKind::Connection` error, but in v1 no
//! session handler actually produces a `connection`-kind error AFTER a
//! successful open — the core query paths surface post-open failures as
//! `internal` (see `handlers.rs`, which flattens them via `ZemonError::internal`
//! / `.map_err(ZemonError::from)`), so this eviction path does not currently
//! trigger. A connection that dies mid-session is therefore not detected or
//! recovered from automatically; restarting the server always reopens cleanly.
//! Mapping post-open errors to `connection`-kind is left as a deliberate
//! follow-up rather than done here.

use std::sync::Arc;
use tokio::sync::Mutex;
use zemon_core::config::{EffectiveConfig, ZemonConfig};
use zemon_core::error::ZemonError;

pub struct ServerState {
    pub config: ZemonConfig,
    /// The effective, allow-listed view of the config the session was actually
    /// built from (CLI flags + env + config-file, as resolved at startup).
    /// `config_show` serializes this directly rather than re-resolving, so it
    /// always agrees with `info` about what the server is bound to.
    pub effective: EffectiveConfig,
    session: Mutex<Option<Arc<zenoh::Session>>>,
}

impl ServerState {
    pub fn new(config: ZemonConfig, effective: EffectiveConfig) -> Self {
        Self {
            config,
            effective,
            session: Mutex::new(None),
        }
    }

    /// Return the shared session, opening it on first use. Callers that observe
    /// an explicit connection-kind error should call `invalidate_session` so the
    /// next call reopens (see the module note on mid-session drops).
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
