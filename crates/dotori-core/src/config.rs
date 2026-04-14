use color_eyre::eyre::eyre;
use std::path::PathBuf;

/// Connection configuration for a Zenoh session.
#[derive(Debug, Clone)]
pub struct DotoriConfig {
    pub endpoint: String,
    pub mode: ConnectMode,
    pub namespace: Option<String>,
    pub config_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectMode {
    Peer,
    Client,
}

impl Default for DotoriConfig {
    fn default() -> Self {
        Self {
            endpoint: "tcp/localhost:7447".to_string(),
            mode: ConnectMode::Client,
            namespace: None,
            config_file: None,
        }
    }
}

impl DotoriConfig {
    /// Build a Zenoh Config from DotoriConfig.
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

        Ok(config)
    }

    /// Create config from environment variables, falling back to defaults.
    pub fn from_env() -> Self {
        let mut cfg = Self::default();

        if let Ok(endpoint) = std::env::var("DOTORI_ENDPOINT") {
            cfg.endpoint = endpoint;
        }
        if let Ok(mode) = std::env::var("DOTORI_MODE") {
            cfg.mode = match mode.to_lowercase().as_str() {
                "peer" => ConnectMode::Peer,
                _ => ConnectMode::Client,
            };
        }
        if let Ok(ns) = std::env::var("DOTORI_NAMESPACE") {
            cfg.namespace = Some(ns);
        }
        if let Ok(config_path) = std::env::var("DOTORI_CONFIG") {
            cfg.config_file = Some(PathBuf::from(config_path));
        }

        cfg
    }
}
