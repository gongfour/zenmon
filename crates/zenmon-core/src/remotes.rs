//! Named release-remote registry — where `zenmon update` looks for releases.
//!
//! **zenmon application internals — not part of the public API.** This module
//! encodes zenmon's own update behaviour and is `pub` only because
//! `zenmon-cli` and the tray app are separate crates. See the crate-level docs.
//!
//! A *remote* is a place that holds release manifests and their artifacts:
//! either a GitHub repository whose releases carry a `zenmon.json`, or a
//! filesystem/UNC directory holding the same file set. The registry maps short
//! names to those places and remembers which one is the default, so
//! `zenmon update` can take a `--remote <name>` (or nothing at all).
//!
//! It lives in the per-user config directory, deliberately outside any zenmon
//! checkout: which builds a machine installs from is a property of the machine,
//! not of a source tree that may not even be present.
//!
//! # Why `path` exists alongside `github`
//!
//! `github` is the distribution channel. `path` is what makes a locally built
//! binary installable — publish into a folder, then update from it — and it is
//! the same mechanism an air-gapped machine uses with a USB stick. Both read
//! the identical file layout, so a folder can be copied between them.

use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::{Result, ZenmonError};

/// The repository consulted when the registry is empty. A freshly installed
/// zenmon must be able to update itself before the user has configured
/// anything; requiring `remote add` first would make the common path the
/// hardest one.
pub const BUILTIN_REPO: &str = "gongfour/zenmon";

/// Name reported for the built-in remote. Never stored in the file — it exists
/// only so messages can say where a resolution came from.
pub const BUILTIN_NAME: &str = "(built-in)";

/// Overrides the registry location, for tests and for running against a
/// throwaway registry without touching the real one.
pub const REMOTES_FILE_ENV: &str = "ZENMON_REMOTES";

/// The `kind` values this build can act on.
///
/// Used only to tell "you need a newer zenmon" apart from "this entry is
/// malformed" — see [`RemoteEntry`].
const KNOWN_KINDS: &[&str] = &["github", "path"];

/// Where a remote's releases live.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RemoteSpec {
    /// A GitHub repository, `owner/name`. Releases tagged `v<semver>` carrying
    /// a `zenmon.json` asset.
    Github { repo: String },
    /// A directory holding the same file set a GitHub release does. Local
    /// disk, a mounted USB stick, or a UNC share.
    Path { path: String },
}

impl RemoteSpec {
    pub fn kind(&self) -> &'static str {
        match self {
            RemoteSpec::Github { .. } => "github",
            RemoteSpec::Path { .. } => "path",
        }
    }

    /// The location, without the kind — `list` prints the two in columns.
    pub fn location(&self) -> &str {
        match self {
            RemoteSpec::Github { repo } => repo,
            RemoteSpec::Path { path } => path,
        }
    }

    /// A github remote pointing at [`BUILTIN_REPO`].
    pub fn builtin() -> Self {
        RemoteSpec::Github {
            repo: BUILTIN_REPO.to_owned(),
        }
    }
}

impl fmt::Display for RemoteSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} {}", self.kind(), self.location())
    }
}

/// One entry in the registry: either a remote this build understands, or one
/// it does not.
///
/// A bare `RemoteSpec` cannot be stored, because serde fails the *whole file*
/// when it meets an unknown `kind` — and this file also holds the default-remote
/// pointer and every other entry. A zenmon that predates a future kind would
/// then break `remote list`, `remote add` and `update` for remotes it does
/// understand perfectly well, and the user's only clue would be a parse error.
///
/// So an unrecognised entry is parked here instead. It round-trips verbatim (an
/// older zenmon writing a new remote cannot silently drop one it did not
/// understand), it stays visible in `remote list`, and it fails only when
/// something actually asks to *use* it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum RemoteEntry {
    Known(RemoteSpec),
    Unknown(UnknownRemote),
}

/// An entry whose `kind` this build cannot act on, kept byte-for-byte.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct UnknownRemote {
    pub kind: String,
    #[serde(flatten)]
    pub rest: BTreeMap<String, toml::Value>,
}

impl RemoteEntry {
    /// The spec, when this build understands the entry.
    pub fn known(&self) -> Option<&RemoteSpec> {
        match self {
            RemoteEntry::Known(spec) => Some(spec),
            RemoteEntry::Unknown(_) => None,
        }
    }

    /// The `kind` string as it appears in the file.
    pub fn kind(&self) -> &str {
        match self {
            RemoteEntry::Known(spec) => spec.kind(),
            RemoteEntry::Unknown(unknown) => &unknown.kind,
        }
    }

    /// What to print for the location column. An unknown entry has no field
    /// this build can name, so it says so rather than inventing one.
    pub fn location(&self) -> &str {
        match self {
            RemoteEntry::Known(spec) => spec.location(),
            RemoteEntry::Unknown(_) => "(not understood by this zenmon)",
        }
    }

    /// Why this entry cannot be used, phrased for whichever case it is.
    ///
    /// A `kind` this build lists as known but still failed to parse is a
    /// malformed entry, not an old binary — telling someone to upgrade there
    /// would send them down the wrong path.
    fn unusable_reason(&self, name: &str, path: &Path) -> String {
        let kind = self.kind();
        if KNOWN_KINDS.contains(&kind) {
            format!(
                "remote {name:?} has kind {kind:?} but its settings are incomplete or malformed; \
                 fix it in {} or re-add it with `zenmon remote add {name} ...`",
                path.display()
            )
        } else {
            format!(
                "remote {name:?} has kind {kind:?}, which this zenmon does not understand; \
                 it was probably added by a newer version — upgrade zenmon, or pick another \
                 remote with --remote"
            )
        }
    }
}

/// Which of the three resolution paths produced a remote. Callers report this
/// so "it used the built-in default" is never a silent assumption.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteSource {
    /// Named explicitly, e.g. `--remote usb`.
    Requested,
    /// The registry's configured default.
    Default,
    /// Nothing configured; [`BUILTIN_REPO`].
    Builtin,
}

/// A remote ready to use, with the name and origin to report.
#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedRemote {
    pub name: String,
    pub spec: RemoteSpec,
    pub source: RemoteSource,
}

/// The registry file's contents.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RemotesConfig {
    /// Name of the remote used when none is given. Kept as a name rather than
    /// a flag on the entry so exactly one can hold it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub remotes: BTreeMap<String, RemoteEntry>,
}

impl RemotesConfig {
    pub fn is_empty(&self) -> bool {
        self.remotes.is_empty()
    }

    /// Whether `name` is the configured default.
    pub fn is_default(&self, name: &str) -> bool {
        self.default.as_deref() == Some(name)
    }

    /// Registers a remote, replacing any entry of the same name.
    ///
    /// The first remote added becomes the default even without `make_default`:
    /// a registry with entries but no default would make plain `zenmon update`
    /// fail for someone who has configured exactly one place to update from.
    pub fn add(&mut self, name: &str, spec: RemoteSpec, make_default: bool) -> Result<()> {
        validate_remote_name(name)?;
        validate_spec(&spec)?;
        let first = self.remotes.is_empty();
        self.remotes
            .insert(name.to_owned(), RemoteEntry::Known(spec));
        if make_default || first {
            self.default = Some(name.to_owned());
        }
        Ok(())
    }

    /// Removes a remote. Clearing the default along with it is deliberate: a
    /// dangling default would resolve to "not found" on every later `update`.
    pub fn remove(&mut self, name: &str) -> Result<()> {
        if self.remotes.remove(name).is_none() {
            return Err(unknown_remote(name, &self.names()));
        }
        if self.is_default(name) {
            // Promote the sole survivor rather than leaving nothing selected —
            // with one remote left there is no ambiguity about what to pick.
            self.default = match self.remotes.len() {
                1 => self.remotes.keys().next().cloned(),
                _ => None,
            };
        }
        Ok(())
    }

    pub fn set_default(&mut self, name: &str) -> Result<()> {
        if !self.remotes.contains_key(name) {
            return Err(unknown_remote(name, &self.names()));
        }
        self.default = Some(name.to_owned());
        Ok(())
    }

    pub fn names(&self) -> Vec<String> {
        self.remotes.keys().cloned().collect()
    }

    /// Resolves a `--remote` argument into something usable.
    ///
    /// `path` is only used to point at the file in error messages.
    pub fn resolve(&self, requested: Option<&str>, path: &Path) -> Result<ResolvedRemote> {
        if let Some(name) = requested {
            let entry = self
                .remotes
                .get(name)
                .ok_or_else(|| unknown_remote(name, &self.names()))?;
            return self.resolve_entry(name, entry, RemoteSource::Requested, path);
        }

        if let Some(name) = self.default.as_deref() {
            // A default naming a removed remote should not silently fall
            // through to the built-in: the user configured something, and
            // quietly updating from somewhere else is the wrong repair.
            let entry = self.remotes.get(name).ok_or_else(|| {
                ZenmonError::not_found(format!(
                    "default remote {name:?} is not in the registry ({}); \
                     set another with `zenmon remote default <name>`",
                    path.display()
                ))
            })?;
            return self.resolve_entry(name, entry, RemoteSource::Default, path);
        }

        if self.remotes.is_empty() {
            return Ok(ResolvedRemote {
                name: BUILTIN_NAME.to_owned(),
                spec: RemoteSpec::builtin(),
                source: RemoteSource::Builtin,
            });
        }

        Err(ZenmonError::invalid_input(format!(
            "no default remote is set and none was given; pass --remote <name> \
             or run `zenmon remote default <name>` (available: {})",
            self.names().join(", ")
        )))
    }

    fn resolve_entry(
        &self,
        name: &str,
        entry: &RemoteEntry,
        source: RemoteSource,
        path: &Path,
    ) -> Result<ResolvedRemote> {
        match entry.known() {
            Some(spec) => Ok(ResolvedRemote {
                name: name.to_owned(),
                spec: spec.clone(),
                source,
            }),
            None => Err(ZenmonError::invalid_input(
                entry.unusable_reason(name, path),
            )),
        }
    }
}

fn unknown_remote(name: &str, available: &[String]) -> ZenmonError {
    if available.is_empty() {
        ZenmonError::not_found(format!(
            "no remote named {name:?}; the registry is empty — add one with \
             `zenmon remote add {name} --github <owner/repo>`"
        ))
    } else {
        ZenmonError::not_found(format!(
            "no remote named {name:?} (available: {})",
            available.join(", ")
        ))
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

/// Remote names appear on the command line and as TOML table keys, so they are
/// held to a conservative charset rather than whatever TOML would quote.
pub fn validate_remote_name(name: &str) -> Result<()> {
    if name.is_empty() {
        return Err(ZenmonError::invalid_input("remote name cannot be empty"));
    }
    if name.len() > 64 {
        return Err(ZenmonError::invalid_input(
            "remote name cannot be longer than 64 characters",
        ));
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    {
        return Err(ZenmonError::invalid_input(format!(
            "invalid remote name {name:?}: use ASCII letters, digits, '-', '_' or '.'"
        )));
    }
    Ok(())
}

fn validate_spec(spec: &RemoteSpec) -> Result<()> {
    match spec {
        RemoteSpec::Github { repo } => validate_github_repo(repo),
        RemoteSpec::Path { path } => validate_path(path),
    }
}

/// `owner/name`, as GitHub itself spells it. Catching a bare `zenmon` or a full
/// `https://github.com/...` here beats surfacing it as a 404 during an update.
pub fn validate_github_repo(repo: &str) -> Result<()> {
    let mut parts = repo.split('/');
    let owner = parts.next().unwrap_or_default();
    let name = parts.next().unwrap_or_default();
    let extra = parts.next();

    if owner.is_empty() || name.is_empty() || extra.is_some() {
        return Err(ZenmonError::invalid_input(format!(
            "invalid GitHub repository {repo:?}: expected owner/name (e.g. {BUILTIN_REPO})"
        )));
    }
    if repo.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ZenmonError::invalid_input(format!(
            "invalid GitHub repository {repo:?}: contains whitespace or control characters"
        )));
    }
    Ok(())
}

/// Existence is not checked: a USB stick or network share is routinely absent
/// when the remote is registered and present when it is used. `update` reports
/// a missing directory at the point it actually needs it.
pub fn validate_path(path: &str) -> Result<()> {
    if path.trim().is_empty() {
        return Err(ZenmonError::invalid_input(
            "remote path cannot be empty".to_owned(),
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Persistence
// ---------------------------------------------------------------------------

/// The registry file. `ZENMON_REMOTES` overrides it wholesale.
///
/// Note this is `zenmon`, not `zenmon-tray`: the tray keeps its capture
/// settings in its own directory, but the list of places to update from is one
/// machine-level fact both binaries must agree on.
pub fn config_path() -> Result<PathBuf> {
    if let Some(raw) = std::env::var_os(REMOTES_FILE_ENV) {
        let path = PathBuf::from(raw);
        if path.as_os_str().is_empty() {
            return Err(ZenmonError::invalid_input(format!(
                "{REMOTES_FILE_ENV} is set but empty"
            )));
        }
        return Ok(path);
    }
    let dirs = directories::ProjectDirs::from("", "", "zenmon").ok_or_else(|| {
        ZenmonError::internal(
            "could not determine a config directory for this platform; \
             set ZENMON_REMOTES to choose the registry file explicitly",
        )
    })?;
    Ok(dirs.config_dir().join("remotes.toml"))
}

/// Loads the registry, or an empty one when the file does not exist yet.
///
/// A *corrupt* file is a hard error, unlike the tray's own config which falls
/// back to defaults. There the goal is that the tray icon always appears; here
/// a silent empty registry would make `remote list` deny remotes the user can
/// see in the file, and would send `update` to the built-in repository instead
/// of the one they configured.
pub fn load_from(path: &Path) -> Result<RemotesConfig> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(RemotesConfig::default())
        }
        Err(err) => {
            return Err(ZenmonError::internal(format!(
                "failed to read {}: {err}",
                path.display()
            )))
        }
    };
    toml::from_str(&text).map_err(|err| {
        ZenmonError::invalid_input(format!("invalid remote registry {}: {err}", path.display()))
    })
}

pub fn load() -> Result<RemotesConfig> {
    load_from(&config_path()?)
}

/// Writes the registry, creating the config directory on first use.
///
/// Serialised to a temporary file and renamed into place: a crash or a full
/// disk partway through a direct write would leave a truncated TOML that the
/// loader then rejects, locking the user out of every remote they had.
pub fn save_to(path: &Path, config: &RemotesConfig) -> Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent).map_err(|err| {
                ZenmonError::internal(format!("failed to create {}: {err}", parent.display()))
            })?;
        }
    }
    let text = toml::to_string_pretty(config)
        .map_err(|err| ZenmonError::internal(format!("failed to serialize remotes: {err}")))?;

    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, text).map_err(|err| {
        ZenmonError::internal(format!("failed to write {}: {err}", tmp.display()))
    })?;
    // std::fs::rename replaces an existing destination on both Unix and
    // Windows, so this is a single atomic swap rather than remove-then-move.
    std::fs::rename(&tmp, path).map_err(|err| {
        let _ = std::fs::remove_file(&tmp);
        ZenmonError::internal(format!("failed to replace {}: {err}", path.display()))
    })
}

pub fn save(config: &RemotesConfig) -> Result<()> {
    save_to(&config_path()?, config)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn github(repo: &str) -> RemoteSpec {
        RemoteSpec::Github {
            repo: repo.to_owned(),
        }
    }

    fn path_spec(path: &str) -> RemoteSpec {
        RemoteSpec::Path {
            path: path.to_owned(),
        }
    }

    #[test]
    fn round_trips_known_kinds_through_toml() {
        let mut config = RemotesConfig::default();
        config.add("gh", github("gongfour/zenmon"), false).unwrap();
        config
            .add("usb", path_spec("E:/zenmon_releases"), true)
            .unwrap();

        let text = toml::to_string_pretty(&config).unwrap();
        let parsed: RemotesConfig = toml::from_str(&text).unwrap();

        assert_eq!(parsed, config);
        assert_eq!(parsed.default.as_deref(), Some("usb"));
        assert_eq!(
            parsed.remotes["gh"].known(),
            Some(&github("gongfour/zenmon"))
        );
    }

    /// The whole point of `RemoteEntry::Unknown`: a kind from a future zenmon
    /// must not take the rest of the file down with it.
    #[test]
    fn an_unknown_kind_does_not_break_the_rest_of_the_file() {
        let text = r#"
default = "gh"

[remotes.gh]
kind = "github"
repo = "gongfour/zenmon"

[remotes.corp]
kind = "artifactory"
url = "https://artifactory.example.com/zenmon"
token_env = "CORP_TOKEN"
"#;
        let config: RemotesConfig = toml::from_str(text).unwrap();

        assert_eq!(config.remotes.len(), 2);
        assert_eq!(
            config.remotes["gh"].known(),
            Some(&github("gongfour/zenmon"))
        );
        assert!(config.remotes["corp"].known().is_none());
        assert_eq!(config.remotes["corp"].kind(), "artifactory");

        // and the understood remote still resolves
        let resolved = config.resolve(None, Path::new("remotes.toml")).unwrap();
        assert_eq!(resolved.name, "gh");
        assert_eq!(resolved.source, RemoteSource::Default);
    }

    /// Re-saving must not drop the fields of an entry this build cannot parse,
    /// or an older zenmon would silently destroy a newer one's configuration.
    #[test]
    fn an_unknown_kind_round_trips_verbatim() {
        let text = r#"
[remotes.corp]
kind = "artifactory"
url = "https://artifactory.example.com/zenmon"
token_env = "CORP_TOKEN"
"#;
        let config: RemotesConfig = toml::from_str(text).unwrap();
        let rewritten: RemotesConfig = toml::from_str(&toml::to_string_pretty(&config).unwrap())
            .expect("re-serialized registry must parse");

        let entry = &rewritten.remotes["corp"];
        assert_eq!(entry.kind(), "artifactory");
        match entry {
            RemoteEntry::Unknown(unknown) => {
                assert_eq!(
                    unknown.rest["url"].as_str(),
                    Some("https://artifactory.example.com/zenmon")
                );
                assert_eq!(unknown.rest["token_env"].as_str(), Some("CORP_TOKEN"));
            }
            RemoteEntry::Known(_) => panic!("artifactory must not parse as a known kind"),
        }
    }

    #[test]
    fn using_an_unknown_kind_says_to_upgrade_not_that_it_is_malformed() {
        let text = r#"
default = "corp"

[remotes.corp]
kind = "artifactory"
url = "https://example.com"
"#;
        let config: RemotesConfig = toml::from_str(text).unwrap();
        let err = config
            .resolve(None, Path::new("remotes.toml"))
            .expect_err("an unusable default must fail");
        assert!(err.to_string().contains("upgrade zenmon"), "{err}");
    }

    /// A known kind with missing fields is a typo in the file, not an old
    /// binary — the advice has to differ.
    #[test]
    fn a_malformed_known_kind_says_to_fix_the_file() {
        let text = r#"
[remotes.broken]
kind = "github"
"#;
        let config: RemotesConfig = toml::from_str(text).unwrap();
        let err = config
            .resolve(Some("broken"), Path::new("remotes.toml"))
            .expect_err("a malformed entry must fail");
        let message = err.to_string();
        assert!(message.contains("incomplete or malformed"), "{message}");
        assert!(!message.contains("upgrade zenmon"), "{message}");
    }

    #[test]
    fn an_empty_registry_resolves_to_the_builtin_repository() {
        let config = RemotesConfig::default();
        let resolved = config.resolve(None, Path::new("remotes.toml")).unwrap();

        assert_eq!(resolved.source, RemoteSource::Builtin);
        assert_eq!(resolved.spec, github(BUILTIN_REPO));
    }

    /// Once the user has configured remotes, silence must not fall back to the
    /// built-in — that would update from somewhere they did not choose.
    #[test]
    fn a_populated_registry_without_a_default_refuses_to_guess() {
        let mut config = RemotesConfig::default();
        config.add("a", github("gongfour/zenmon"), false).unwrap();
        config.add("b", path_spec("E:/rel"), false).unwrap();
        config.default = None;

        let err = config
            .resolve(None, Path::new("remotes.toml"))
            .expect_err("must not guess between remotes");
        let message = err.to_string();
        assert!(message.contains("no default remote"), "{message}");
        assert!(message.contains('a') && message.contains('b'), "{message}");
    }

    #[test]
    fn the_first_remote_added_becomes_the_default() {
        let mut config = RemotesConfig::default();
        config
            .add("only", github("gongfour/zenmon"), false)
            .unwrap();
        assert_eq!(config.default.as_deref(), Some("only"));
    }

    #[test]
    fn removing_the_default_promotes_a_sole_survivor_and_otherwise_clears_it() {
        let mut config = RemotesConfig::default();
        config.add("a", github("gongfour/zenmon"), true).unwrap();
        config.add("b", path_spec("E:/rel"), false).unwrap();
        config.add("c", path_spec("F:/rel"), false).unwrap();

        config.remove("a").unwrap();
        assert_eq!(config.default, None, "two candidates left: do not guess");

        config.set_default("b").unwrap();
        config.remove("c").unwrap();
        config.remove("b").unwrap();
        assert_eq!(config.default, None);
        assert!(config.is_empty());
    }

    #[test]
    fn removing_the_default_with_one_remote_left_promotes_it() {
        let mut config = RemotesConfig::default();
        config.add("a", github("gongfour/zenmon"), true).unwrap();
        config.add("b", path_spec("E:/rel"), false).unwrap();

        config.remove("a").unwrap();
        assert_eq!(config.default.as_deref(), Some("b"));
    }

    #[test]
    fn a_dangling_default_is_reported_rather_than_falling_back() {
        let mut config = RemotesConfig::default();
        config.add("a", github("gongfour/zenmon"), true).unwrap();
        config.default = Some("gone".to_owned());

        let err = config
            .resolve(None, Path::new("remotes.toml"))
            .expect_err("a dangling default must fail");
        assert!(err.to_string().contains("gone"), "{err}");
    }

    #[test]
    fn removing_or_defaulting_an_absent_remote_lists_what_exists() {
        let mut config = RemotesConfig::default();
        config.add("a", github("gongfour/zenmon"), true).unwrap();

        let err = config.remove("nope").unwrap_err();
        assert!(err.to_string().contains("available: a"), "{err}");
        let err = config.set_default("nope").unwrap_err();
        assert!(err.to_string().contains("available: a"), "{err}");
    }

    #[test]
    fn rejects_repositories_that_are_not_owner_slash_name() {
        for bad in [
            "zenmon",
            "https://github.com/gongfour/zenmon",
            "gongfour/",
            "/zenmon",
            "gongfour/zen mon",
            "",
        ] {
            assert!(
                validate_github_repo(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
        assert!(validate_github_repo("gongfour/zenmon").is_ok());
        assert!(validate_github_repo("Some-Org/zen.mon_2").is_ok());
    }

    #[test]
    fn rejects_names_that_would_need_quoting_in_toml() {
        for bad in ["", "has space", "sla/sh", "quote\"", "\u{1F600}"] {
            assert!(
                validate_remote_name(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
        for good in ["gh", "usb-2", "corp_nas", "v1.0"] {
            assert!(validate_remote_name(good).is_ok(), "{good:?} should be ok");
        }
    }

    #[test]
    fn rejects_a_blank_path() {
        assert!(validate_path("   ").is_err());
        assert!(validate_path("E:/zenmon_releases").is_ok());
    }

    #[test]
    fn a_missing_registry_file_loads_as_empty() {
        let dir = std::env::temp_dir().join("zenmon-remotes-missing-test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("remotes.toml");

        let config = load_from(&path).expect("a missing file is not an error");
        assert!(config.is_empty());
    }

    #[test]
    fn a_corrupt_registry_file_is_an_error_not_an_empty_one() {
        let dir = std::env::temp_dir().join("zenmon-remotes-corrupt-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("remotes.toml");
        std::fs::write(&path, "this is not = = toml").unwrap();

        let err = load_from(&path).expect_err("a corrupt registry must not read as empty");
        assert!(err.to_string().contains("invalid remote registry"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saves_through_a_temporary_file_and_reloads_identically() {
        let dir = std::env::temp_dir().join("zenmon-remotes-save-test");
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("nested").join("remotes.toml");

        let mut config = RemotesConfig::default();
        config.add("gh", github("gongfour/zenmon"), true).unwrap();
        config
            .add("usb", path_spec(r"\\nas\zenmon"), false)
            .unwrap();

        save_to(&path, &config).expect("save must create parent directories");
        assert_eq!(load_from(&path).unwrap(), config);
        assert!(
            !path.with_extension("toml.tmp").exists(),
            "the temporary file must not survive a successful save"
        );

        // and a second save over an existing file replaces it
        config.add("usb", path_spec("E:/other"), false).unwrap();
        save_to(&path, &config).expect("save must overwrite");
        assert_eq!(load_from(&path).unwrap(), config);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
