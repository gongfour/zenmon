//! Profile/settings persistence: `%APPDATA%\zenmon-tray\config\config.toml`.
//!
//! Capture output defaults live under the local (non-roaming) data dir, since
//! capture segments are large binary files that shouldn't ride profile sync.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use zenmon_core::config::{resolve_config_with_env, ConfigEnvironment, ConfigOverrides};
use zenmon_core::{ZenmonConfig, ZenmonError};

const SCHEMA_VERSION: u32 = 1;
const DEFAULT_PROFILE_NAME: &str = "default";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub app: AppSettings,
    #[serde(default)]
    pub profiles: Vec<Profile>,
}

fn default_schema_version() -> u32 {
    SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSettings {
    pub selected_profile: String,
    #[serde(default)]
    pub resume_capture_on_launch: bool,
    #[serde(default)]
    pub last_capture_running: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub endpoint: String,
    /// "client" or "peer" — passed through verbatim to zenmon-core, which
    /// validates it.
    pub mode: String,
    #[serde(default)]
    pub namespace: Option<String>,
    pub key_expr: String,
    pub output_dir: PathBuf,
    pub rotate_size_bytes: u64,
    pub rotate_interval_secs: u64,
    pub max_total_size_bytes: u64,
    pub max_age_secs: u64,
}

impl Profile {
    pub fn rotate_interval(&self) -> Duration {
        Duration::from_secs(self.rotate_interval_secs)
    }

    pub fn max_age(&self) -> Duration {
        Duration::from_secs(self.max_age_secs)
    }

    /// Bridge into zenmon-core. `ZenmonConfig` can't be built as a struct
    /// literal outside zenmon-core (some fields are private), so this is the
    /// only sanctioned path in.
    pub fn to_zenmon_config(&self) -> Result<ZenmonConfig, ZenmonError> {
        let overrides = ConfigOverrides {
            endpoint: Some(self.endpoint.clone()),
            mode: Some(self.mode.clone()),
            namespace: self.namespace.clone(),
            config_file: None,
            scout_port: None,
            connect_timeout: None,
        };
        let resolved = resolve_config_with_env(overrides, ConfigEnvironment::from_process())?;
        Ok(resolved.config)
    }

    pub fn seed(name: &str, data_local_dir: &Path) -> Self {
        Self {
            name: name.to_string(),
            endpoint: "tcp/localhost:7447".to_string(),
            mode: "client".to_string(),
            namespace: None,
            key_expr: "**".to_string(),
            output_dir: data_local_dir.join("captures").join(name),
            rotate_size_bytes: 64 * 1024 * 1024,
            rotate_interval_secs: 3600,
            max_total_size_bytes: 1024 * 1024 * 1024,
            max_age_secs: 7 * 24 * 3600,
        }
    }
}

impl AppConfig {
    /// First-run default: one seeded profile, nothing auto-started.
    fn first_run(data_local_dir: &Path) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            app: AppSettings {
                selected_profile: DEFAULT_PROFILE_NAME.to_string(),
                resume_capture_on_launch: false,
                last_capture_running: false,
            },
            profiles: vec![Profile::seed(DEFAULT_PROFILE_NAME, data_local_dir)],
        }
    }

    pub fn selected_profile(&self) -> Option<&Profile> {
        self.profiles
            .iter()
            .find(|p| p.name == self.app.selected_profile)
    }
}

/// Resolved on-disk locations. `None` if the OS gave us no home directory to
/// anchor against (e.g. some sandboxed/service contexts).
#[derive(Debug, Clone)]
pub struct Paths {
    pub config_file: PathBuf,
    pub data_local_dir: PathBuf,
}

pub fn resolve_paths() -> Option<Paths> {
    let dirs = directories::ProjectDirs::from("", "", "zenmon-tray")?;
    Some(Paths {
        config_file: dirs.config_dir().join("config.toml"),
        data_local_dir: dirs.data_local_dir().to_path_buf(),
    })
}

/// Loads the config, or seeds first-run defaults if the file is missing or
/// unreadable. Never fails outright — a corrupt config shouldn't block the
/// tray icon from appearing.
pub fn load(paths: &Paths) -> AppConfig {
    match std::fs::read_to_string(&paths.config_file) {
        Ok(text) => match toml::from_str::<AppConfig>(&text) {
            Ok(config) => config,
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    path = %paths.config_file.display(),
                    "corrupt config, using first-run defaults"
                );
                AppConfig::first_run(&paths.data_local_dir)
            }
        },
        Err(_) => AppConfig::first_run(&paths.data_local_dir),
    }
}

/// Atomic write: `config.toml.tmp` then rename over `config.toml`, so a
/// killed process never leaves a half-written file behind.
pub fn save(paths: &Paths, config: &AppConfig) -> std::io::Result<()> {
    let dir = paths
        .config_file
        .parent()
        .expect("config_file always has a parent");
    std::fs::create_dir_all(dir)?;

    let text = toml::to_string_pretty(config)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?;

    let tmp = paths.config_file.with_extension("toml.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, &paths.config_file)?;
    Ok(())
}
