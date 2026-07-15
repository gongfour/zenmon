use color_eyre::eyre::eyre;
use std::path::PathBuf;
use std::time::Duration;

/// Connection configuration for a Zenoh session.
#[derive(Debug, Clone)]
pub struct ZemonConfig {
    pub endpoint: String,
    pub mode: ConnectMode,
    pub namespace: Option<String>,
    pub config_file: Option<PathBuf>,
    /// When set, overrides Zenoh's multicast scouting port to
    /// `224.0.0.224:{scout_port}`. Zenoh default is 7446.
    pub scout_port: Option<u16>,
    /// Connect deadline. In client mode this maps to Zenoh's
    /// `connect/timeout_ms` (client already has `exit_on_failure=true`), so a
    /// client that can't reach its router within the deadline fails to open.
    /// Peer mode is left untouched (zero routers is healthy for a peer).
    pub connect_timeout: Option<Duration>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectMode {
    Peer,
    Client,
}

impl Default for ZemonConfig {
    fn default() -> Self {
        Self {
            endpoint: "tcp/localhost:7447".to_string(),
            mode: ConnectMode::Client,
            namespace: None,
            config_file: None,
            scout_port: None,
            connect_timeout: None,
        }
    }
}

impl ZemonConfig {
    /// Build a Zenoh Config from ZemonConfig.
    pub fn to_zenoh_config(&self) -> color_eyre::Result<zenoh::Config> {
        let mut config = match &self.config_file {
            Some(path) => zenoh::Config::from_file(path).map_err(|e| eyre!(e))?,
            None => zenoh::Config::default(),
        };

        let mode_str = match self.mode {
            ConnectMode::Peer => "\"peer\"",
            ConnectMode::Client => "\"client\"",
        };
        config.insert_json5("mode", mode_str).map_err(|e| eyre!(e))?;

        let endpoint_json = format!("[\"{}\"]", self.endpoint);
        config.insert_json5("connect/endpoints", &endpoint_json).map_err(|e| eyre!(e))?;

        if let Some(ns) = &self.namespace {
            config.insert_json5("namespace", &format!("\"{}\"", ns)).map_err(|e| eyre!(e))?;
        }

        if let Some(port) = self.scout_port {
            config
                .insert_json5("scouting/multicast/enabled", "true")
                .map_err(|e| eyre!(e))?;
            config
                .insert_json5(
                    "scouting/multicast/address",
                    &format!("\"224.0.0.224:{}\"", port),
                )
                .map_err(|e| eyre!(e))?;
        }

        // Client connect deadline. Zenoh's client mode already defaults to
        // exit_on_failure=true, so setting connect/timeout_ms gives a real
        // "fail if no router within this window" deadline. Peer mode is left
        // at its defaults because a peer with zero routers is healthy.
        if let Some(timeout) = self.connect_timeout {
            if self.mode == ConnectMode::Client {
                let ms = timeout.as_millis();
                config
                    .insert_json5("connect/timeout_ms", &ms.to_string())
                    .map_err(|e| eyre!(e))?;
                config
                    .insert_json5("connect/exit_on_failure", "true")
                    .map_err(|e| eyre!(e))?;
            }
        }

        Ok(config)
    }

    /// Create config from environment variables, falling back to defaults.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();

        if let Ok(endpoint) = std::env::var("ZEMON_ENDPOINT") {
            cfg.endpoint = endpoint;
        }
        if let Ok(mode) = std::env::var("ZEMON_MODE") {
            cfg.mode = match mode.to_lowercase().as_str() {
                "peer" => ConnectMode::Peer,
                _ => ConnectMode::Client,
            };
        }
        if let Ok(ns) = std::env::var("ZEMON_NAMESPACE") {
            cfg.namespace = Some(ns);
        }
        if let Ok(config_path) = std::env::var("ZEMON_CONFIG") {
            cfg.config_file = Some(PathBuf::from(config_path));
        }
        if let Ok(port) = std::env::var("ZEMON_SCOUT_PORT") {
            if let Ok(p) = port.parse::<u16>() {
                cfg.scout_port = Some(p);
            }
        }
        if let Ok(ct) = std::env::var("ZEMON_CONNECT_TIMEOUT") {
            if let Ok(d) = humantime::parse_duration(&ct) {
                cfg.connect_timeout = Some(d);
            }
        }

        cfg
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_timeout_sets_client_timeout_ms() {
        let cfg = ZemonConfig {
            mode: ConnectMode::Client,
            connect_timeout: Some(Duration::from_secs(5)),
            ..Default::default()
        };
        let zc = cfg.to_zenoh_config().unwrap();
        // 5s == 5000ms.
        assert_eq!(zc.get_json("connect/timeout_ms").unwrap(), "5000");
    }

    #[test]
    fn connect_timeout_ignored_in_peer_mode() {
        let cfg = ZemonConfig {
            mode: ConnectMode::Peer,
            connect_timeout: Some(Duration::from_secs(5)),
            ..Default::default()
        };
        // Builds fine; peer keeps Zenoh's default per-mode timeout object
        // rather than the scalar we set only for clients.
        let zc = cfg.to_zenoh_config().unwrap();
        assert_ne!(zc.get_json("connect/timeout_ms").unwrap(), "5000");
    }

    #[test]
    fn no_connect_timeout_leaves_default() {
        let cfg = ZemonConfig::default();
        let zc = cfg.to_zenoh_config().unwrap();
        assert_ne!(zc.get_json("connect/timeout_ms").unwrap(), "5000");
    }
}
