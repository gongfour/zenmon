//! Server-wide state: the resolved config and the shared Zenoh session.
//! (Session lifecycle is fleshed out in Task 2.)

use zemon_core::config::ZemonConfig;

pub struct ServerState {
    pub config: ZemonConfig,
}

impl ServerState {
    pub fn new(config: ZemonConfig) -> Self {
        Self { config }
    }
}
