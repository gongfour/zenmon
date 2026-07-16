use color_eyre::eyre::eyre;
use std::path::PathBuf;
use std::time::Duration;

use crate::error::ZemonError;

/// Smallest connect timeout we accept. Zenoh's `connect/timeout_ms` is
/// millisecond-granular, so a sub-millisecond value (e.g. `1ns`) would round
/// down to `0` — "give up immediately" / disabled — which is never what the
/// caller asked for. Reject it instead of silently changing the request.
pub const MIN_CONNECT_TIMEOUT_MS: u128 = 1;

/// Largest connect timeout we accept (~49.7 days). Beyond this the value is
/// meaningless as a connect deadline and risks overflowing Zenoh's millisecond
/// field.
pub const MAX_CONNECT_TIMEOUT_MS: u128 = u32::MAX as u128;

/// Validate a connect timeout against Zenoh's millisecond granularity and range.
///
/// Applied identically to the `--connect-timeout` flag and the
/// `ZEMON_CONNECT_TIMEOUT` environment variable so neither can smuggle in a
/// value that silently becomes `0ms` or overflows.
pub fn validate_connect_timeout(timeout: Duration) -> Result<Duration, String> {
    let ms = timeout.as_millis();
    if ms < MIN_CONNECT_TIMEOUT_MS {
        return Err(format!(
            "connect timeout must be at least {}ms, got {:?}: Zenoh's connect timeout is \
             millisecond-granular so smaller values would silently become 0ms",
            MIN_CONNECT_TIMEOUT_MS, timeout
        ));
    }
    if ms > MAX_CONNECT_TIMEOUT_MS {
        return Err(format!(
            "connect timeout must be at most {}ms (~49 days), got {}ms",
            MAX_CONNECT_TIMEOUT_MS, ms
        ));
    }
    Ok(timeout)
}

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
    ///
    /// A malformed or out-of-range `ZEMON_CONNECT_TIMEOUT` is a hard error
    /// rather than a silently ignored value, so a caller who set it never gets
    /// the default deadline without knowing.
    pub fn from_env() -> Result<Self, ZemonError> {
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
            let parsed = humantime::parse_duration(&ct).map_err(|e| {
                ZemonError::invalid_input(format!(
                    "invalid ZEMON_CONNECT_TIMEOUT '{}': {} (try e.g. 5s, 100ms)",
                    ct, e
                ))
            })?;
            let validated = validate_connect_timeout(parsed).map_err(|msg| {
                ZemonError::invalid_input(format!("invalid ZEMON_CONNECT_TIMEOUT '{}': {}", ct, msg))
            })?;
            cfg.connect_timeout = Some(validated);
        }

        Ok(cfg)
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

    #[test]
    fn validate_connect_timeout_accepts_in_range() {
        assert_eq!(
            validate_connect_timeout(Duration::from_millis(1)).unwrap(),
            Duration::from_millis(1)
        );
        assert_eq!(
            validate_connect_timeout(Duration::from_secs(5)).unwrap(),
            Duration::from_secs(5)
        );
    }

    #[test]
    fn validate_connect_timeout_rejects_sub_millisecond() {
        // Non-zero but rounds to 0ms under Zenoh's millisecond granularity.
        assert!(validate_connect_timeout(Duration::from_nanos(1)).is_err());
        assert!(validate_connect_timeout(Duration::from_micros(999)).is_err());
    }

    #[test]
    fn validate_connect_timeout_rejects_zero() {
        assert!(validate_connect_timeout(Duration::ZERO).is_err());
    }

    #[test]
    fn validate_connect_timeout_rejects_beyond_max() {
        let too_big = Duration::from_millis((MAX_CONNECT_TIMEOUT_MS + 1) as u64);
        assert!(validate_connect_timeout(too_big).is_err());
        // The max itself is accepted.
        assert!(validate_connect_timeout(Duration::from_millis(MAX_CONNECT_TIMEOUT_MS as u64)).is_ok());
    }
}
