//! `zenmon update` — check for, and install, newer releases.

#[cfg(feature = "self-update")]
mod github;
#[cfg(feature = "self-update")]
mod install;
#[cfg(feature = "self-update")]
mod manifest;
#[cfg(feature = "self-update")]
mod source;

#[cfg(not(feature = "self-update"))]
pub async fn run(_command: crate::cli::UpdateCommand, _json: bool) -> zenmon_core::Result<()> {
    // The subcommand still exists in builds without the feature. "Unrecognized
    // subcommand" would send someone looking for a typo; this says what
    // happened and what to do instead.
    Err(zenmon_core::ZenmonError::invalid_input(
        "this zenmon was built without the self-updater (--no-default-features). \
         Release binaries are published for windows/linux/macos only; on other platforms \
         update by rebuilding from source.",
    ))
}

#[cfg(feature = "self-update")]
pub use imp::run;

#[cfg(feature = "self-update")]
mod imp {
    use zenmon_core::error::Result;
    use zenmon_core::remotes::{self, RemoteSource, ResolvedRemote};

    use super::install::{self, Installation};
    use super::manifest::{self as mf, ReleaseManifest};
    use super::source::{self, ManifestSource};
    use crate::cli::UpdateCommand;

    pub async fn run(command: UpdateCommand, json: bool) -> Result<()> {
        match command {
            UpdateCommand::Check { remote } => check(remote.as_deref(), json).await,
            UpdateCommand::Apply { remote, reinstall } => {
                apply(remote.as_deref(), reinstall, json).await
            }
        }
    }

    /// Resolves `--remote` (or the default, or the built-in) into a spec.
    fn resolve(requested: Option<&str>) -> Result<ResolvedRemote> {
        let path = remotes::config_path()?;
        let registry = remotes::load_from(&path)?;
        registry.resolve(requested, &path)
    }

    fn source_label(source: RemoteSource) -> &'static str {
        match source {
            RemoteSource::Requested => "requested",
            RemoteSource::Default => "default",
            RemoteSource::Builtin => "built-in",
        }
    }

    fn remote_json(remote: &ResolvedRemote) -> serde_json::Value {
        serde_json::json!({
            "name": remote.name,
            "kind": remote.spec.kind(),
            "location": remote.spec.location(),
            "source": source_label(remote.source),
        })
    }

    async fn resolve_release(
        requested: Option<&str>,
    ) -> Result<(ResolvedRemote, ReleaseManifest, ManifestSource)> {
        let remote = resolve(requested)?;
        let (manifest, origin) = source::fetch_manifest(&remote.spec).await?;
        Ok((remote, manifest, origin))
    }

    async fn check(requested: Option<&str>, json: bool) -> Result<()> {
        let (remote, manifest, origin) = resolve_release(requested).await?;
        let available = manifest.semver()?;
        let current = mf::current_version()?;

        // Reported even when there is nothing to install: knowing whether this
        // release covers your platform is the answer to a different question
        // than whether a newer version exists.
        let target = mf::platform_target();
        let supported = manifest.artifact_for(&target).is_ok();

        if json {
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "current": current.to_string(),
                    "available": available.to_string(),
                    "update_available": available > current,
                    "channel": manifest.channel,
                    "target": target,
                    "target_supported": supported,
                    "remote": remote_json(&remote),
                    "origin": origin.describe(),
                }))?
            );
            return Ok(());
        }

        match available.cmp(&current) {
            std::cmp::Ordering::Greater => {
                println!(
                    "update available: {current} -> {available}  ({})",
                    origin.describe()
                );
                println!("run `zenmon update apply` to install it");
            }
            std::cmp::Ordering::Equal => {
                println!("zenmon {current} is up to date ({})", origin.describe());
            }
            // Not an error: a locally built binary, or a `path` remote holding
            // an older release on purpose.
            std::cmp::Ordering::Less => {
                println!(
                    "installed zenmon {current} is newer than {available} at {}",
                    origin.describe()
                );
                println!("`zenmon update apply --reinstall` would install the older one");
            }
        }
        if !supported {
            println!("note: this release has no build for {target}");
        }
        Ok(())
    }

    async fn apply(requested: Option<&str>, reinstall: bool, json: bool) -> Result<()> {
        let (remote, manifest, origin) = resolve_release(requested).await?;
        let available = manifest.semver()?;
        let current = mf::current_version()?;

        if !mf::should_apply(&available, &current, reinstall) {
            if json {
                println!(
                    "{}",
                    serde_json::to_string(&serde_json::json!({
                        "ok": true,
                        "installed": false,
                        "reason": if available == current { "up_to_date" } else { "not_newer" },
                        "current": current.to_string(),
                        "available": available.to_string(),
                        "remote": remote_json(&remote),
                    }))?
                );
            } else {
                println!(
                    "nothing to do: installed {current}, available {available} \
                     ({}) — pass --reinstall to install it anyway",
                    origin.describe()
                );
            }
            return Ok(());
        }

        // Both of these fail without touching the network, so a cargo-managed
        // install or an unsupported platform is reported before a download
        // rather than after one.
        let installation: Installation = install::current_installation()?;
        let target = mf::platform_target();
        let artifact = manifest.artifact_for(&target)?.clone();
        mf::validate_sha256(&artifact.sha256)?;

        let bytes = source::fetch_artifact(&origin, &artifact).await?;
        mf::verify_sha256(&bytes, &artifact.sha256)?;
        let binary = install::extract_binary(&bytes, &artifact.url, &artifact.binary)?;
        let outcome = install::stage_and_swap(&installation, &binary, &available)?;

        if json {
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "ok": true,
                    "installed": true,
                    "from": current.to_string(),
                    "to": available.to_string(),
                    "path": installation.exe.display().to_string(),
                    "retained_old_binaries": outcome.retained,
                    "remote": remote_json(&remote),
                }))?
            );
        } else {
            println!(
                "installed zenmon {available} (was {current}) at {}",
                installation.exe.display()
            );
            if outcome.retained > 0 {
                println!(
                    "{} older zenmon binaries are still being executed by running processes \
                     and were left in place; they keep the old code until restarted, and a \
                     later update removes the files.",
                    outcome.retained
                );
            }
        }
        Ok(())
    }
}
