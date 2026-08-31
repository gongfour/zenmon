//! Saved TUI views.
//!
//! A profile is what you were *looking at* — the filter, which branches were
//! open, which field each key was plotting — so that "the layout I use when a
//! forklift stalls" survives closing the terminal.
//!
//! ## Why this is not in the tray's config file
//!
//! [`crate::remotes`] already drew this line and wrote down why: the tray keeps
//! its own settings under `zenmon-tray`, and only facts *both binaries must
//! agree on* go under `zenmon`. A saved view is neither — it is the CLI's own
//! state, and the tray has no use for it.
//!
//! Sharing the tray's file would also mean two separately-installed binaries
//! writing one document. The tray deserializes into a typed struct, so it drops
//! fields it does not know on its next save; a version skew in either direction
//! silently eats data. A separate file has no such failure mode.
//!
//! Connection settings are deliberately absent. A profile that named an
//! endpoint could not act on it — the TUI can reconnect on a mode or scout-port
//! change but not on an endpoint — so storing one would be a field that looks
//! like it does something.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, ZenmonError};

/// Overrides the profiles file, for tests and for anyone keeping views beside a
/// project rather than in their home directory.
pub const PROFILES_FILE_ENV: &str = "ZENMON_TUI_PROFILES";

/// One saved view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuiProfile {
    pub name: String,
    /// The master-pane filter.
    #[serde(default)]
    pub filter: String,
    /// Branch paths that were open.
    #[serde(default)]
    pub expanded: Vec<String>,
    /// Over-threshold branches that were listed in full.
    #[serde(default)]
    pub unfolded: Vec<String>,
    /// Key expression → JSON pointer being plotted.
    #[serde(default)]
    pub plot_fields: BTreeMap<String, String>,
    /// Whether the payload diff was on. Defaults to true, matching the app, so
    /// a profile written before this existed does not silently turn it off.
    #[serde(default = "default_true")]
    pub diff_enabled: bool,
}

fn default_true() -> bool {
    true
}

/// Every saved view, newest write wins per name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuiProfiles {
    #[serde(default, rename = "profile")]
    pub profiles: Vec<TuiProfile>,
}

impl TuiProfiles {
    pub fn get(&self, name: &str) -> Option<&TuiProfile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// Insert or replace by name, keeping the list sorted so the picker order
    /// does not depend on save order.
    pub fn upsert(&mut self, profile: TuiProfile) {
        match self.profiles.iter_mut().find(|p| p.name == profile.name) {
            Some(existing) => *existing = profile,
            None => self.profiles.push(profile),
        }
        self.profiles.sort_by(|a, b| a.name.cmp(&b.name));
    }

    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.profiles.len();
        self.profiles.retain(|p| p.name != name);
        self.profiles.len() != before
    }

    pub fn names(&self) -> Vec<&str> {
        self.profiles.iter().map(|p| p.name.as_str()).collect()
    }
}

/// Where views are kept: `zenmon`'s config dir, beside `remotes.toml`.
pub fn config_path() -> Result<PathBuf> {
    if let Some(raw) = std::env::var_os(PROFILES_FILE_ENV) {
        let path = PathBuf::from(raw);
        if path.as_os_str().is_empty() {
            return Err(ZenmonError::invalid_input(format!(
                "{PROFILES_FILE_ENV} is set but empty"
            )));
        }
        return Ok(path);
    }
    let dirs = directories::ProjectDirs::from("", "", "zenmon").ok_or_else(|| {
        ZenmonError::internal(
            "could not determine a config directory for this platform; \
             set ZENMON_TUI_PROFILES to choose the profiles file explicitly",
        )
    })?;
    Ok(dirs.config_dir().join("tui-profiles.toml"))
}

/// Load saved views, or an empty set when the file does not exist yet.
///
/// A corrupt file is a hard error rather than an empty set, for the same reason
/// as the remotes registry: silently reporting "no saved views" when the file
/// plainly contains some would send the user looking in the wrong place, and
/// the next save would overwrite what was there.
pub fn load_from(path: &Path) -> Result<TuiProfiles> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(TuiProfiles::default()),
        Err(err) => {
            return Err(ZenmonError::internal(format!(
                "failed to read {}: {err}",
                path.display()
            )))
        }
    };
    toml::from_str(&text).map_err(|err| {
        ZenmonError::invalid_input(format!("{} is not valid: {err}", path.display()))
    })
}

pub fn load() -> Result<TuiProfiles> {
    load_from(&config_path()?)
}

pub fn save_to(path: &Path, profiles: &TuiProfiles) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| {
                ZenmonError::internal(format!("failed to create {}: {err}", parent.display()))
            })?;
        }
    }
    let text = toml::to_string_pretty(profiles)
        .map_err(|err| ZenmonError::internal(format!("failed to serialize profiles: {err}")))?;

    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|err| {
        ZenmonError::internal(format!("failed to write {}: {err}", tmp.display()))
    })?;
    // Rename replaces an existing destination on both Unix and Windows, so a
    // killed process never leaves a half-written file behind.
    std::fs::rename(&tmp, path).map_err(|err| {
        let _ = std::fs::remove_file(&tmp);
        ZenmonError::internal(format!("failed to replace {}: {err}", path.display()))
    })
}

pub fn save(profiles: &TuiProfiles) -> Result<()> {
    save_to(&config_path()?, profiles)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile(name: &str) -> TuiProfile {
        TuiProfile {
            name: name.into(),
            filter: "agv".into(),
            expanded: vec!["agv".into(), "agv/f001".into()],
            unfolded: vec!["agv".into()],
            plot_fields: [("agv/f001/battery".to_string(), "/percent".to_string())]
                .into_iter()
                .collect(),
            diff_enabled: true,
        }
    }

    fn tmp_path(name: &str) -> PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!(
            "zenmon-profiles-{}-{}.toml",
            name,
            std::process::id()
        ));
        p
    }

    #[test]
    fn a_missing_file_is_an_empty_set_not_an_error() {
        let path = tmp_path("missing");
        let _ = std::fs::remove_file(&path);
        assert_eq!(load_from(&path).unwrap(), TuiProfiles::default());
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_a_silent_empty_set() {
        // Reporting "no saved views" over a file that has some would send the
        // user hunting, and the next save would overwrite them.
        let path = tmp_path("corrupt");
        std::fs::write(&path, "this is not toml {{{").unwrap();
        let err = load_from(&path).unwrap_err().to_string();
        assert!(err.contains("not valid"), "{err}");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_profile_round_trips_through_the_file() {
        let path = tmp_path("roundtrip");
        let mut set = TuiProfiles::default();
        set.upsert(profile("stall"));
        save_to(&path, &set).unwrap();

        let back = load_from(&path).unwrap();
        assert_eq!(back, set);
        assert_eq!(back.get("stall").unwrap().filter, "agv");
        assert_eq!(
            back.get("stall").unwrap().plot_fields["agv/f001/battery"],
            "/percent"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn saving_the_same_name_replaces_rather_than_duplicates() {
        let mut set = TuiProfiles::default();
        set.upsert(profile("a"));
        let mut updated = profile("a");
        updated.filter = "srv".into();
        set.upsert(updated);
        assert_eq!(set.profiles.len(), 1);
        assert_eq!(set.get("a").unwrap().filter, "srv");
    }

    #[test]
    fn names_come_out_sorted_regardless_of_save_order() {
        let mut set = TuiProfiles::default();
        set.upsert(profile("zulu"));
        set.upsert(profile("alpha"));
        set.upsert(profile("mike"));
        assert_eq!(set.names(), vec!["alpha", "mike", "zulu"]);
    }

    #[test]
    fn remove_reports_whether_anything_went() {
        let mut set = TuiProfiles::default();
        set.upsert(profile("a"));
        assert!(set.remove("a"));
        assert!(!set.remove("a"));
    }

    #[test]
    fn a_profile_written_before_diff_existed_keeps_the_diff_on() {
        // `#[serde(default)]` on a bool would be false, silently turning off a
        // feature the app has on by default.
        let parsed: TuiProfiles = toml::from_str(
            r#"
[[profile]]
name = "old"
filter = "agv"
"#,
        )
        .unwrap();
        assert!(parsed.get("old").unwrap().diff_enabled);
    }

    #[test]
    fn an_empty_env_override_is_rejected_rather_than_silently_ignored() {
        std::env::set_var(PROFILES_FILE_ENV, "");
        let err = config_path().unwrap_err().to_string();
        std::env::remove_var(PROFILES_FILE_ENV);
        assert!(err.contains(PROFILES_FILE_ENV), "{err}");
    }
}
