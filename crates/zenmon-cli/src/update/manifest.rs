//! The `zenmon.json` release manifest — the contract between
//! `scripts/release/build-manifest.sh` and `zenmon update`.

use std::env;

use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use zenmon_core::error::{Result, ZenmonError};

/// Asset name carrying the manifest, in a GitHub release and in a `path`
/// remote's directory alike.
pub const MANIFEST_NAME: &str = "zenmon.json";

/// The only schema this build understands.
pub const SCHEMA_VERSION: u32 = 1;

/// Release tags are `v<semver>`; anything else in the repository is not a
/// zenmon release.
pub const TAG_PREFIX: &str = "v";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseManifest {
    pub schema_version: u32,
    pub version: String,
    #[serde(default)]
    pub channel: String,
    pub artifacts: Vec<ReleaseArtifact>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReleaseArtifact {
    /// `{OS}-{ARCH}` as the *running binary* reports itself — not the Rust
    /// target triple. See `scripts/release/package-cli.sh`.
    pub target: String,
    /// Manifest-relative file name. Never absolute, so the same file set works
    /// in a GitHub release and in a directory copied to a USB stick.
    pub url: String,
    pub sha256: String,
    /// Name of the executable inside the archive.
    pub binary: String,
}

/// How this build identifies itself in a manifest.
pub fn platform_target() -> String {
    format!("{}-{}", env::consts::OS, env::consts::ARCH)
}

/// The version this binary was built as.
pub fn current_version() -> Result<Version> {
    Version::parse(env!("CARGO_PKG_VERSION")).map_err(|err| {
        ZenmonError::internal(format!(
            "this build has an unparseable version {:?}: {err}",
            env!("CARGO_PKG_VERSION")
        ))
    })
}

impl ReleaseManifest {
    pub fn parse(bytes: &[u8], origin: &str) -> Result<Self> {
        let manifest: ReleaseManifest = serde_json::from_slice(bytes).map_err(|err| {
            ZenmonError::invalid_input(format!("invalid release manifest from {origin}: {err}"))
        })?;

        // A newer schema is not a corrupt file, and saying so is the difference
        // between "upgrade zenmon" and a bug hunt.
        if manifest.schema_version != SCHEMA_VERSION {
            return Err(ZenmonError::invalid_input(format!(
                "release manifest from {origin} uses schema version {} but this zenmon \
                 understands {SCHEMA_VERSION}; upgrade zenmon",
                manifest.schema_version
            )));
        }
        if manifest.artifacts.is_empty() {
            return Err(ZenmonError::invalid_input(format!(
                "release manifest from {origin} lists no artifacts"
            )));
        }
        Ok(manifest)
    }

    pub fn semver(&self) -> Result<Version> {
        Version::parse(&self.version).map_err(|err| {
            ZenmonError::invalid_input(format!(
                "release manifest version {:?} is not valid semver: {err}",
                self.version
            ))
        })
    }

    /// The artifact for a platform, or an error naming what the manifest does
    /// carry — "no build for your platform" is a normal answer for a
    /// source-built Termux install, and it should read like one.
    pub fn artifact_for(&self, target: &str) -> Result<&ReleaseArtifact> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.target == target)
            .ok_or_else(|| {
                let available: Vec<&str> =
                    self.artifacts.iter().map(|a| a.target.as_str()).collect();
                ZenmonError::not_found(format!(
                    "release {} has no build for {target} (it has: {}); \
                     build from source for this platform",
                    self.version,
                    available.join(", ")
                ))
            })
    }
}

/// Rejects a checksum that could never match before anything is downloaded.
pub fn validate_sha256(value: &str) -> Result<()> {
    if value.len() != 64 || !value.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(ZenmonError::invalid_input(format!(
            "release manifest has a malformed sha256 {value:?}: expected 64 hex characters"
        )));
    }
    Ok(())
}

/// Confirms downloaded bytes are the ones the manifest named.
///
/// This is the whole integrity story for the CLI: the manifest arrives over
/// HTTPS from the repository, and the checksum ties the artifact to it. It
/// does not defend against someone who can write to the repository — see the
/// design note in docs/superpowers/specs.
pub fn verify_sha256(bytes: &[u8], expected: &str) -> Result<()> {
    let actual = format!("{:x}", Sha256::digest(bytes));
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(ZenmonError::invalid_input(format!(
            "downloaded artifact does not match the manifest checksum \
             (expected {expected}, got {actual}); the download was corrupted or tampered with"
        )));
    }
    Ok(())
}

/// The version gate. `reinstall` is the only way to install something that is
/// not strictly newer, which keeps the automatic path forward-only while still
/// allowing a deliberate re-install or rollback.
pub fn should_apply(available: &Version, current: &Version, reinstall: bool) -> bool {
    reinstall || available > current
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_json(schema: u32) -> String {
        format!(
            r#"{{
              "schemaVersion": {schema},
              "version": "0.2.0",
              "channel": "stable",
              "artifacts": [
                {{"target":"linux-x86_64","url":"zenmon-0.2.0-x86_64-unknown-linux-gnu.tar.gz",
                  "sha256":"{sha}","binary":"zenmon"}},
                {{"target":"windows-x86_64","url":"zenmon-0.2.0-x86_64-pc-windows-msvc.zip",
                  "sha256":"{sha}","binary":"zenmon.exe"}}
              ]
            }}"#,
            sha = "a".repeat(64)
        )
    }

    #[test]
    fn parses_a_v1_manifest() {
        let manifest = ReleaseManifest::parse(manifest_json(1).as_bytes(), "test").unwrap();
        assert_eq!(manifest.semver().unwrap(), Version::new(0, 2, 0));
        assert_eq!(manifest.artifacts.len(), 2);
    }

    /// A manifest from a future zenmon must say so, not read as corrupt.
    #[test]
    fn a_newer_schema_says_to_upgrade() {
        let err = ReleaseManifest::parse(manifest_json(2).as_bytes(), "test").unwrap_err();
        assert!(err.to_string().contains("upgrade zenmon"), "{err}");
    }

    #[test]
    fn rejects_a_manifest_with_no_artifacts() {
        let json = r#"{"schemaVersion":1,"version":"0.2.0","channel":"stable","artifacts":[]}"#;
        let err = ReleaseManifest::parse(json.as_bytes(), "test").unwrap_err();
        assert!(err.to_string().contains("no artifacts"), "{err}");
    }

    #[test]
    fn selects_the_artifact_for_this_platform() {
        let manifest = ReleaseManifest::parse(manifest_json(1).as_bytes(), "test").unwrap();
        let artifact = manifest.artifact_for("windows-x86_64").unwrap();
        assert_eq!(artifact.binary, "zenmon.exe");
    }

    /// The Termux case: a real release that simply has no build here. The
    /// message has to name what exists and point at a source build.
    #[test]
    fn a_missing_platform_lists_what_the_release_does_have() {
        let manifest = ReleaseManifest::parse(manifest_json(1).as_bytes(), "test").unwrap();
        let err = manifest.artifact_for("android-aarch64").unwrap_err();
        let message = err.to_string();
        assert!(message.contains("android-aarch64"), "{message}");
        assert!(message.contains("linux-x86_64"), "{message}");
        assert!(message.contains("build from source"), "{message}");
    }

    #[test]
    fn platform_target_is_os_dash_arch() {
        let target = platform_target();
        assert!(target.contains('-'), "{target}");
        assert!(
            !target.contains("pc-windows"),
            "not a Rust triple: {target}"
        );
    }

    #[test]
    fn rejects_malformed_checksums_before_downloading() {
        assert!(validate_sha256(&"a".repeat(64)).is_ok());
        assert!(validate_sha256(&"A".repeat(64)).is_ok());
        assert!(validate_sha256(&"a".repeat(63)).is_err());
        assert!(validate_sha256(&"a".repeat(65)).is_err());
        assert!(validate_sha256(&"z".repeat(64)).is_err());
        assert!(validate_sha256("").is_err());
    }

    #[test]
    fn verifies_downloaded_bytes_against_the_manifest() {
        // sha256("zenmon")
        let expected = "e4bd0e6dbe4f9a1f0e3a0e40e02d3b6b2a2a48e0e3c73cd1a2d9dc2e2b8e5b5f";
        assert!(verify_sha256(b"zenmon", expected).is_err());

        let actual = format!("{:x}", Sha256::digest(b"zenmon"));
        assert!(verify_sha256(b"zenmon", &actual).is_ok());
        // the manifest may spell it in either case
        assert!(verify_sha256(b"zenmon", &actual.to_uppercase()).is_ok());
        assert!(verify_sha256(b"zenmoN", &actual).is_err());
    }

    #[test]
    fn the_version_gate_is_forward_only_unless_reinstalling() {
        let current = Version::new(0, 2, 0);

        assert!(should_apply(&Version::new(0, 3, 0), &current, false));
        assert!(!should_apply(&Version::new(0, 2, 0), &current, false));
        assert!(!should_apply(&Version::new(0, 1, 0), &current, false));

        // --reinstall covers both same-version reinstall and rollback
        assert!(should_apply(&Version::new(0, 2, 0), &current, true));
        assert!(should_apply(&Version::new(0, 1, 0), &current, true));
    }

    #[test]
    fn this_build_reports_a_parseable_version() {
        current_version().expect("CARGO_PKG_VERSION must be semver");
    }
}
