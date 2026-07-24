use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::error::ZenmonError;

use serde::Serialize;
use serde_json::Value;

const DEFAULT_ENDPOINT: &str = "tcp/localhost:7447";
const DEFAULT_SCOUT_PORT: u16 = 7446;

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
/// `ZENMON_CONNECT_TIMEOUT` environment variable so neither can smuggle in a
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

/// Connection configuration used by the core and TUI.
#[derive(Debug, Clone)]
pub struct ZenmonConfig {
    /// Effective endpoint shown to the user. When `endpoint_override` is None,
    /// the complete endpoint list from `config_file` remains authoritative.
    pub endpoint: String,
    pub mode: ConnectMode,
    pub namespace: Option<String>,
    pub config_file: Option<PathBuf>,
    /// When set, overrides Zenoh's multicast scouting port.
    pub scout_port: Option<u16>,
    /// Connect deadline. In client mode this maps to Zenoh's
    /// `connect/timeout_ms` (client already has `exit_on_failure=true`), so a
    /// client that can't reach its router within the deadline fails to open.
    /// Peer mode is left untouched (zero routers is healthy for a peer).
    pub connect_timeout: Option<Duration>,
    endpoint_override: Option<String>,
    mode_override: Option<ConnectMode>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectMode {
    Peer,
    Client,
}

impl ConnectMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Peer => "peer",
            Self::Client => "client",
        }
    }

    fn parse(value: &str) -> Result<Self, ZenmonError> {
        match value.to_ascii_lowercase().as_str() {
            "peer" => Ok(Self::Peer),
            "client" => Ok(Self::Client),
            _ => Err(ZenmonError::invalid_input(format!(
                "invalid connection mode '{}': expected 'peer' or 'client'",
                value
            ))),
        }
    }
}

impl fmt::Display for ConnectMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfigSource {
    Default,
    File,
    Env,
    Cli,
}

impl fmt::Display for ConfigSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = match self {
            Self::Default => "default",
            Self::File => "file",
            Self::Env => "env",
            Self::Cli => "cli",
        };
        f.write_str(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResolvedValue<T> {
    pub value: T,
    pub source: ConfigSource,
}

impl<T> ResolvedValue<T> {
    fn new(value: T, source: ConfigSource) -> Self {
        Self { value, source }
    }
}

/// Safe, allow-listed view returned by `config show --effective`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EffectiveConfig {
    pub endpoint: ResolvedValue<String>,
    pub mode: ResolvedValue<ConnectMode>,
    pub namespace: ResolvedValue<Option<String>>,
    pub config_file: ResolvedValue<Option<PathBuf>>,
    pub scout_port: ResolvedValue<u16>,
    /// Rendered connect deadline (e.g. `"5s"`), or `None` when unset. Kept as a
    /// humantime string so the effective view serializes cleanly for both the
    /// text table and `--json`.
    pub connect_timeout: ResolvedValue<Option<String>>,
}

#[derive(Debug, Clone)]
pub struct ResolvedConfig {
    pub config: ZenmonConfig,
    pub effective: EffectiveConfig,
}

/// Explicit command-line values. `None` means the flag was not supplied.
#[derive(Debug, Clone, Default)]
pub struct ConfigOverrides {
    pub endpoint: Option<String>,
    pub mode: Option<String>,
    pub namespace: Option<String>,
    pub config_file: Option<PathBuf>,
    pub scout_port: Option<u16>,
    /// Already parsed and range-validated by the CLI `--connect-timeout` parser.
    pub connect_timeout: Option<Duration>,
}

/// Environment values separated from process access to keep resolution testable.
#[derive(Debug, Clone, Default)]
pub struct ConfigEnvironment {
    pub endpoint: Option<String>,
    pub mode: Option<String>,
    pub namespace: Option<String>,
    pub config_file: Option<PathBuf>,
    pub scout_port: Option<String>,
    pub connect_timeout: Option<String>,
}

impl ConfigEnvironment {
    pub fn from_process() -> Self {
        Self {
            endpoint: std::env::var("ZENMON_ENDPOINT").ok(),
            mode: std::env::var("ZENMON_MODE").ok(),
            namespace: std::env::var("ZENMON_NAMESPACE").ok(),
            config_file: std::env::var_os("ZENMON_CONFIG").map(PathBuf::from),
            scout_port: std::env::var("ZENMON_SCOUT_PORT").ok(),
            connect_timeout: std::env::var("ZENMON_CONNECT_TIMEOUT").ok(),
        }
    }
}

#[derive(Debug, Default)]
struct FileValues {
    mode: Option<ConnectMode>,
    namespace: Option<String>,
    /// Raw `connect/endpoints` JSON. Endpoints may be an array or an object
    /// keyed by mode, so the mode-dependent choice is deferred until the final
    /// mode (after env/CLI overrides) is known.
    endpoints: Option<Value>,
    /// Raw `scouting/multicast/address` JSON, likewise mode-dependent.
    scout_address: Option<Value>,
}

impl Default for ZenmonConfig {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_ENDPOINT.to_string(),
            mode: ConnectMode::Client,
            namespace: None,
            config_file: None,
            scout_port: None,
            connect_timeout: None,
            endpoint_override: Some(DEFAULT_ENDPOINT.to_string()),
            mode_override: Some(ConnectMode::Client),
        }
    }
}

impl ZenmonConfig {
    /// Build a Zenoh Config while preserving file-owned values that were not
    /// explicitly overridden by environment variables or CLI flags.
    pub fn to_zenoh_config(&self) -> Result<zenoh::Config, ZenmonError> {
        let mut config = match &self.config_file {
            Some(path) => zenoh::Config::from_file(path).map_err(invalid_config)?,
            None => zenoh::Config::default(),
        };

        if let Some(mode) = self.mode_override {
            config
                .insert_json5("mode", &format!("\"{}\"", mode.as_str()))
                .map_err(invalid_config)?;
        }

        if let Some(endpoint) = &self.endpoint_override {
            let endpoint_json = serde_json::to_string(&[endpoint]).map_err(invalid_config)?;
            config
                .insert_json5("connect/endpoints", &endpoint_json)
                .map_err(invalid_config)?;
        }

        if let Some(ns) = &self.namespace {
            let namespace_json = serde_json::to_string(ns).map_err(invalid_config)?;
            config
                .insert_json5("namespace", &namespace_json)
                .map_err(invalid_config)?;
        }

        if let Some(port) = self.scout_port {
            config
                .insert_json5("scouting/multicast/enabled", "true")
                .map_err(invalid_config)?;
            config
                .insert_json5(
                    "scouting/multicast/address",
                    &format!("\"224.0.0.224:{}\"", port),
                )
                .map_err(invalid_config)?;
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
                    .map_err(invalid_config)?;
                config
                    .insert_json5("connect/exit_on_failure", "true")
                    .map_err(invalid_config)?;
            }
        }

        Ok(config)
    }

    /// Update the mode interactively and make it an explicit runtime override.
    pub fn set_mode(&mut self, mode: ConnectMode) {
        self.mode = mode;
        self.mode_override = Some(mode);
    }
}

/// Every failure inside configuration resolution is a bad input (bad file, bad
/// env var, bad flag), never a network or internal fault, so they all carry
/// [`crate::error::ErrorKind::InvalidInput`].
fn invalid_config(e: impl fmt::Display) -> ZenmonError {
    ZenmonError::invalid_input(e.to_string())
}

pub fn resolve_config(overrides: ConfigOverrides) -> Result<ResolvedConfig, ZenmonError> {
    resolve_config_with_env(overrides, ConfigEnvironment::from_process())
}

pub fn resolve_config_with_env(
    overrides: ConfigOverrides,
    env: ConfigEnvironment,
) -> Result<ResolvedConfig, ZenmonError> {
    let config_file = overrides
        .config_file
        .clone()
        .or_else(|| env.config_file.clone());
    let config_file_source = if overrides.config_file.is_some() {
        ConfigSource::Cli
    } else if env.config_file.is_some() {
        ConfigSource::Env
    } else {
        ConfigSource::Default
    };

    let file_values = match &config_file {
        Some(path) => read_file_values(path)?,
        None => FileValues::default(),
    };

    // Resolve mode FIRST (default -> file -> env -> cli). File endpoint and
    // scout-port values can be keyed by mode, so their selection must use the
    // final mode, not the file's own mode — otherwise `show --effective` reports
    // a mode-dependent value that disagrees with what Zenoh actually uses.
    let mut mode = ResolvedValue::new(ConnectMode::Client, ConfigSource::Default);
    let mut mode_override = Some(ConnectMode::Client);
    if let Some(value) = file_values.mode {
        mode = ResolvedValue::new(value, ConfigSource::File);
        mode_override = None;
    }
    if let Some(value) = env.mode {
        let value = ConnectMode::parse(&value)?;
        mode_override = Some(value);
        mode = ResolvedValue::new(value, ConfigSource::Env);
    }
    if let Some(value) = overrides.mode {
        let value = ConnectMode::parse(&value)?;
        mode_override = Some(value);
        mode = ResolvedValue::new(value, ConfigSource::Cli);
    }
    let final_mode = mode.value;

    // Now select the mode-dependent file values against the final mode.
    let file_endpoint = file_endpoint_for_mode(&file_values.endpoints, final_mode);
    let file_scout_port = file_scout_port_for_mode(&file_values.scout_address, final_mode);

    let mut endpoint = ResolvedValue::new(DEFAULT_ENDPOINT.to_string(), ConfigSource::Default);
    let mut endpoint_override = Some(DEFAULT_ENDPOINT.to_string());
    let mut namespace = ResolvedValue::new(None, ConfigSource::Default);
    let mut scout_port = ResolvedValue::new(DEFAULT_SCOUT_PORT, ConfigSource::Default);
    let mut runtime_scout_port = None;

    if let Some(value) = file_endpoint {
        endpoint = ResolvedValue::new(value, ConfigSource::File);
        endpoint_override = None;
    }
    if let Some(value) = file_values.namespace {
        namespace = ResolvedValue::new(Some(value), ConfigSource::File);
    }
    if let Some(value) = file_scout_port {
        scout_port = ResolvedValue::new(value, ConfigSource::File);
    }

    if let Some(value) = env.endpoint {
        endpoint_override = Some(value.clone());
        endpoint = ResolvedValue::new(value, ConfigSource::Env);
    }
    if let Some(value) = env.namespace {
        namespace = ResolvedValue::new(Some(value), ConfigSource::Env);
    }
    if let Some(value) = env.scout_port {
        let value = parse_scout_port(&value, "ZENMON_SCOUT_PORT")?;
        runtime_scout_port = Some(value);
        scout_port = ResolvedValue::new(value, ConfigSource::Env);
    }

    if let Some(value) = overrides.endpoint {
        endpoint_override = Some(value.clone());
        endpoint = ResolvedValue::new(value, ConfigSource::Cli);
    }
    if let Some(value) = overrides.namespace {
        namespace = ResolvedValue::new(Some(value), ConfigSource::Cli);
    }
    if let Some(value) = overrides.scout_port {
        runtime_scout_port = Some(value);
        scout_port = ResolvedValue::new(value, ConfigSource::Cli);
    }

    // connect_timeout: default (unset) -> env (ZENMON_CONNECT_TIMEOUT) -> cli.
    // The CLI value arrives already parsed and range-validated; the env value is
    // parsed and validated here so an invalid one is a hard error, never a
    // silently ignored default.
    let mut connect_timeout = ResolvedValue::new(None, ConfigSource::Default);
    if let Some(value) = env.connect_timeout {
        let parsed = humantime::parse_duration(&value).map_err(|e| {
            ZenmonError::invalid_input(format!(
                "invalid ZENMON_CONNECT_TIMEOUT '{}': {} (try e.g. 5s, 100ms)",
                value, e
            ))
        })?;
        let validated = validate_connect_timeout(parsed).map_err(|msg| {
            ZenmonError::invalid_input(format!(
                "invalid ZENMON_CONNECT_TIMEOUT '{}': {}",
                value, msg
            ))
        })?;
        connect_timeout = ResolvedValue::new(Some(validated), ConfigSource::Env);
    }
    if let Some(value) = overrides.connect_timeout {
        connect_timeout = ResolvedValue::new(Some(value), ConfigSource::Cli);
    }

    let config = ZenmonConfig {
        endpoint: endpoint.value.clone(),
        mode: mode.value,
        namespace: namespace.value.clone(),
        config_file: config_file.clone(),
        scout_port: runtime_scout_port,
        connect_timeout: connect_timeout.value,
        endpoint_override,
        mode_override,
    };

    // Validate the merged configuration without opening a network session.
    config.to_zenoh_config()?;

    let connect_timeout_effective = ResolvedValue::new(
        connect_timeout
            .value
            .map(|d| humantime::format_duration(d).to_string()),
        connect_timeout.source,
    );

    Ok(ResolvedConfig {
        config,
        effective: EffectiveConfig {
            endpoint,
            mode,
            namespace,
            config_file: ResolvedValue::new(config_file, config_file_source),
            scout_port,
            connect_timeout: connect_timeout_effective,
        },
    })
}

fn parse_scout_port(value: &str, source: &str) -> Result<u16, ZenmonError> {
    value.parse::<u16>().map_err(|_| {
        ZenmonError::invalid_input(format!("invalid scout port '{}' from {}", value, source))
    })
}

fn read_file_values(path: &Path) -> Result<FileValues, ZenmonError> {
    let config = zenoh::Config::from_file(path).map_err(invalid_config)?;
    let value = serde_json::to_value(&config).map_err(invalid_config)?;
    let mode = value
        .get("mode")
        .and_then(Value::as_str)
        .map(ConnectMode::parse)
        .transpose()?;
    let namespace = value
        .get("namespace")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned);

    // Keep mode-dependent values raw; they are resolved against the *final*
    // mode in `resolve_config_with_env`, not the file's own mode.
    let endpoints = value.pointer("/connect/endpoints").cloned();
    let scout_address = value.pointer("/scouting/multicast/address").cloned();

    Ok(FileValues {
        mode,
        namespace,
        endpoints,
        scout_address,
    })
}

/// Select the file-provided endpoint for `mode` from the raw `connect/endpoints`.
fn file_endpoint_for_mode(endpoints: &Option<Value>, mode: ConnectMode) -> Option<String> {
    endpoints
        .as_ref()
        .and_then(|v| first_mode_dependent_string(v, mode))
}

/// Select the file-provided scout port for `mode` from the raw multicast address.
fn file_scout_port_for_mode(address: &Option<Value>, mode: ConnectMode) -> Option<u16> {
    address
        .as_ref()
        .and_then(|v| mode_dependent_string(v, mode))
        .and_then(|address| address.rsplit_once(':'))
        .and_then(|(_, port)| port.parse::<u16>().ok())
}

fn first_mode_dependent_string(value: &Value, mode: ConnectMode) -> Option<String> {
    match value {
        Value::Array(values) => values
            .first()
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        Value::Object(values) => values
            .get(mode.as_str())
            .and_then(|v| first_mode_dependent_string(v, mode)),
        Value::String(value) => Some(value.clone()),
        _ => None,
    }
}

fn mode_dependent_string(value: &Value, mode: ConnectMode) -> Option<&str> {
    match value {
        Value::String(value) => Some(value),
        Value::Object(values) => values
            .get(mode.as_str())
            .and_then(|v| mode_dependent_string(v, mode)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn config_file(contents: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("zenmon-config-{nonce}.json5"));
        fs::write(&path, contents).unwrap();
        path
    }

    #[test]
    fn defaults_are_reported_with_default_sources() {
        let resolved =
            resolve_config_with_env(ConfigOverrides::default(), ConfigEnvironment::default())
                .unwrap();

        assert_eq!(resolved.config.endpoint, DEFAULT_ENDPOINT);
        assert_eq!(resolved.config.mode, ConnectMode::Client);
        assert_eq!(resolved.effective.endpoint.source, ConfigSource::Default);
        assert_eq!(resolved.effective.mode.source, ConfigSource::Default);
        assert_eq!(resolved.effective.scout_port.value, DEFAULT_SCOUT_PORT);
        assert_eq!(resolved.effective.connect_timeout.value, None);
        assert_eq!(
            resolved.effective.connect_timeout.source,
            ConfigSource::Default
        );
    }

    #[test]
    fn file_values_are_preserved_and_reported() {
        let path = config_file(
            r#"{
                mode: "peer",
                connect: { endpoints: ["tcp/127.0.0.1:7000", "tcp/127.0.0.1:7001"] },
                namespace: "factory",
                scouting: { multicast: { address: "224.0.0.224:7500" } }
            }"#,
        );
        let overrides = ConfigOverrides {
            config_file: Some(path.clone()),
            ..Default::default()
        };

        let resolved =
            resolve_config_with_env(overrides, ConfigEnvironment::default()).unwrap();

        assert_eq!(resolved.effective.endpoint.value, "tcp/127.0.0.1:7000");
        assert_eq!(resolved.effective.endpoint.source, ConfigSource::File);
        assert_eq!(resolved.effective.mode.value, ConnectMode::Peer);
        assert_eq!(resolved.effective.namespace.value.as_deref(), Some("factory"));
        assert_eq!(resolved.effective.scout_port.value, 7500);

        let zenoh = resolved.config.to_zenoh_config().unwrap();
        let json = serde_json::to_value(zenoh).unwrap();
        assert_eq!(
            json.pointer("/connect/endpoints")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(2)
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn cli_overrides_environment_and_environment_overrides_file() {
        let path = config_file(
            r#"{ mode: "peer", connect: { endpoints: ["tcp/file:7000"] } }"#,
        );
        let env = ConfigEnvironment {
            endpoint: Some("tcp/env:7001".into()),
            mode: Some("client".into()),
            config_file: Some(path.clone()),
            scout_port: Some("7501".into()),
            ..Default::default()
        };
        let overrides = ConfigOverrides {
            endpoint: Some("tcp/cli:7002".into()),
            mode: Some("peer".into()),
            scout_port: Some(7502),
            ..Default::default()
        };

        let resolved = resolve_config_with_env(overrides, env).unwrap();

        assert_eq!(resolved.config.endpoint, "tcp/cli:7002");
        assert_eq!(resolved.effective.endpoint.source, ConfigSource::Cli);
        assert_eq!(resolved.config.mode, ConnectMode::Peer);
        assert_eq!(resolved.effective.mode.source, ConfigSource::Cli);
        assert_eq!(resolved.effective.scout_port.value, 7502);
        assert_eq!(resolved.effective.scout_port.source, ConfigSource::Cli);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn environment_is_not_overwritten_by_implicit_cli_defaults() {
        let env = ConfigEnvironment {
            endpoint: Some("tcp/env:7447".into()),
            mode: Some("peer".into()),
            ..Default::default()
        };

        let resolved =
            resolve_config_with_env(ConfigOverrides::default(), env).unwrap();

        assert_eq!(resolved.config.endpoint, "tcp/env:7447");
        assert_eq!(resolved.effective.endpoint.source, ConfigSource::Env);
        assert_eq!(resolved.config.mode, ConnectMode::Peer);
        assert_eq!(resolved.effective.mode.source, ConfigSource::Env);
    }

    #[test]
    fn invalid_mode_and_scout_port_are_rejected() {
        let bad_mode = ConfigEnvironment {
            mode: Some("router".into()),
            ..Default::default()
        };
        assert!(resolve_config_with_env(ConfigOverrides::default(), bad_mode).is_err());

        let bad_port = ConfigEnvironment {
            scout_port: Some("not-a-port".into()),
            ..Default::default()
        };
        assert!(resolve_config_with_env(ConfigOverrides::default(), bad_port).is_err());
    }

    #[test]
    fn invalid_file_and_endpoint_are_rejected() {
        let path = config_file("{ this is not valid JSON5");
        let file_overrides = ConfigOverrides {
            config_file: Some(path.clone()),
            ..Default::default()
        };
        assert!(
            resolve_config_with_env(file_overrides, ConfigEnvironment::default()).is_err()
        );
        fs::remove_file(path).unwrap();

        let endpoint_overrides = ConfigOverrides {
            endpoint: Some("definitely-not-an-endpoint".into()),
            ..Default::default()
        };
        assert!(
            resolve_config_with_env(endpoint_overrides, ConfigEnvironment::default()).is_err()
        );
    }

    #[test]
    fn mode_keyed_endpoint_follows_final_mode_from_env() {
        // File declares peer mode with mode-keyed endpoints; env overrides the
        // mode to client. The reported endpoint must be the client endpoint that
        // Zenoh will actually select, not the file-mode (peer) endpoint.
        let path = config_file(
            r#"{
                mode: "peer",
                connect: { endpoints: {
                    peer: ["tcp/peer.example:7000"],
                    client: ["tcp/client.example:7001"]
                } }
            }"#,
        );
        let env = ConfigEnvironment {
            mode: Some("client".into()),
            config_file: Some(path.clone()),
            ..Default::default()
        };

        let resolved = resolve_config_with_env(ConfigOverrides::default(), env).unwrap();

        assert_eq!(resolved.effective.mode.value, ConnectMode::Client);
        assert_eq!(resolved.effective.mode.source, ConfigSource::Env);
        assert_eq!(
            resolved.effective.endpoint.value, "tcp/client.example:7001",
            "endpoint must track the final (client) mode, not the file peer mode"
        );
        assert_eq!(resolved.effective.endpoint.source, ConfigSource::File);

        // The runtime config applies the client mode override while preserving
        // the endpoints map, so Zenoh really does select the client endpoint.
        let zenoh = resolved.config.to_zenoh_config().unwrap();
        let json = serde_json::to_value(zenoh).unwrap();
        assert_eq!(json.get("mode").and_then(Value::as_str), Some("client"));
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn mode_keyed_endpoint_follows_final_mode_from_cli() {
        // Same as above but the client mode comes from a CLI flag.
        let path = config_file(
            r#"{
                mode: "peer",
                connect: { endpoints: {
                    peer: ["tcp/peer.example:7000"],
                    client: ["tcp/client.example:7001"]
                } }
            }"#,
        );
        let overrides = ConfigOverrides {
            mode: Some("client".into()),
            config_file: Some(path.clone()),
            ..Default::default()
        };

        let resolved =
            resolve_config_with_env(overrides, ConfigEnvironment::default()).unwrap();

        assert_eq!(resolved.effective.mode.value, ConnectMode::Client);
        assert_eq!(resolved.effective.mode.source, ConfigSource::Cli);
        assert_eq!(resolved.effective.endpoint.value, "tcp/client.example:7001");
        assert_eq!(resolved.effective.endpoint.source, ConfigSource::File);
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn effective_json_is_allow_listed() {
        let resolved =
            resolve_config_with_env(ConfigOverrides::default(), ConfigEnvironment::default())
                .unwrap();
        let json = serde_json::to_value(&resolved.effective).unwrap();

        assert_eq!(json.as_object().unwrap().len(), 6);
        assert!(json.get("endpoint").is_some());
        assert!(json.get("connect_timeout").is_some());
        assert!(json.get("password").is_none());
        assert!(json.get("plugins").is_none());
    }

    #[test]
    fn connect_timeout_resolves_from_env_and_cli() {
        let env = ConfigEnvironment {
            connect_timeout: Some("2s".into()),
            ..Default::default()
        };
        let resolved = resolve_config_with_env(ConfigOverrides::default(), env).unwrap();
        assert_eq!(resolved.config.connect_timeout, Some(Duration::from_secs(2)));
        assert_eq!(
            resolved.effective.connect_timeout.value.as_deref(),
            Some("2s")
        );
        assert_eq!(resolved.effective.connect_timeout.source, ConfigSource::Env);

        // CLI (already parsed) wins over env.
        let env = ConfigEnvironment {
            connect_timeout: Some("2s".into()),
            ..Default::default()
        };
        let overrides = ConfigOverrides {
            connect_timeout: Some(Duration::from_secs(5)),
            ..Default::default()
        };
        let resolved = resolve_config_with_env(overrides, env).unwrap();
        assert_eq!(resolved.config.connect_timeout, Some(Duration::from_secs(5)));
        assert_eq!(resolved.effective.connect_timeout.source, ConfigSource::Cli);
    }

    #[test]
    fn invalid_env_connect_timeout_is_rejected() {
        let bad = ConfigEnvironment {
            connect_timeout: Some("1ns".into()),
            ..Default::default()
        };
        assert!(resolve_config_with_env(ConfigOverrides::default(), bad).is_err());

        let malformed = ConfigEnvironment {
            connect_timeout: Some("nonsense".into()),
            ..Default::default()
        };
        assert!(resolve_config_with_env(ConfigOverrides::default(), malformed).is_err());
    }

    #[test]
    fn connect_timeout_sets_client_timeout_ms() {
        let cfg = ZenmonConfig {
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
        let cfg = ZenmonConfig {
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
