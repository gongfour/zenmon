//! `zenmon update` — check for, and install, newer releases.

#[cfg(feature = "self-update")]
mod github;
#[cfg(feature = "self-update")]
mod install;
#[cfg(feature = "self-update")]
mod manifest;
#[cfg(feature = "self-update")]
mod source;
#[cfg(feature = "self-update")]
mod tray;

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
    use super::tray;
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

        let tray_install = tray::detect();
        let tray_supported = manifest.tray_artifact_for(&target).is_some();

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
                    "tray": tray_install.as_ref().map(|t| serde_json::json!({
                        "path": t.path.display().to_string(),
                        "version": t.version.as_ref().map(|v| v.to_string()),
                        "target_supported": tray_supported,
                        "update_available": tray_supported && t.version.as_ref()
                            .is_none_or(|v| &available > v),
                    })),
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

        // Only an installed tray is worth a line; on machines without one the
        // whole topic is noise.
        if let Some(tray) = tray_install {
            match (&tray.version, tray_supported) {
                (_, false) => println!(
                    "note: zenmon-tray is installed but this release has no tray build \
                     for {target}"
                ),
                (Some(version), true) if &available > version => println!(
                    "tray update available: {version} -> {available} \
                     (`zenmon update apply` installs both)"
                ),
                (Some(version), true) => println!("zenmon-tray {version} is up to date"),
                (None, true) => println!(
                    "zenmon-tray is installed but its version could not be read; \
                     `zenmon update apply` will reinstall it"
                ),
            }
        }
        Ok(())
    }

    /// One tray update: fetch, verify, and hand to the platform installer.
    async fn apply_tray(
        origin: &ManifestSource,
        installed: &tray::TrayInstall,
        artifact: &mf::ReleaseArtifact,
    ) -> Result<()> {
        mf::validate_sha256(&artifact.sha256)?;
        let bytes = source::fetch_artifact(origin, artifact).await?;
        mf::verify_sha256(&bytes, &artifact.sha256)?;
        tray::install(installed, artifact, &bytes)
    }

    async fn apply(requested: Option<&str>, reinstall: bool, json: bool) -> Result<()> {
        let (remote, manifest, origin) = resolve_release(requested).await?;
        let available = manifest.semver()?;
        let current = mf::current_version()?;
        let target = mf::platform_target();

        let cli_due = mf::should_apply(&available, &current, reinstall);

        // The tray is due when one is installed on this machine, the release
        // ships a tray build for this platform, and the installed one is not
        // newer — or its version is unreadable, which counts as due rather
        // than pinning a broken install on the old version forever.
        let tray_installed = tray::detect();
        let tray_artifact = manifest.tray_artifact_for(&target).cloned();
        let tray_due = tray_installed.is_some()
            && tray_artifact.is_some()
            && tray_installed
                .as_ref()
                .and_then(|t| t.version.as_ref())
                .is_none_or(|v| mf::should_apply(&available, v, reinstall));

        if !cli_due && !tray_due {
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

        // CLI first: it is the update this command exists for, and a tray
        // failure after it leaves a clear state — new CLI, old tray, an error
        // naming both. Success is printed per half as it lands (text mode),
        // so a later tray failure cannot swallow the CLI's outcome.
        let mut cli_outcome: Option<(Installation, install::SwapOutcome)> = None;
        if cli_due {
            // Both of these fail without touching the network, so a
            // cargo-managed install or an unsupported platform is reported
            // before a download rather than after one.
            let installation: Installation = install::current_installation()?;
            let artifact = manifest.artifact_for(&target)?.clone();
            mf::validate_sha256(&artifact.sha256)?;

            let bytes = source::fetch_artifact(&origin, &artifact).await?;
            mf::verify_sha256(&bytes, &artifact.sha256)?;
            let binary = install::extract_binary(&bytes, &artifact.url, &artifact.binary)?;
            let outcome = install::stage_and_swap(&installation, &binary, &available)?;

            if !json {
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
            cli_outcome = Some((installation, outcome));
        }

        let mut tray_updated: Option<tray::TrayInstall> = None;
        if tray_due {
            let installed = tray_installed.as_ref().expect("tray_due implies detection");
            let artifact = tray_artifact
                .as_ref()
                .expect("tray_due implies an artifact");
            if let Err(err) = apply_tray(&origin, installed, artifact).await {
                // The error must carry what already happened: "the update
                // failed" alone would send someone re-running a CLI update
                // that in fact landed.
                return Err(if cli_outcome.is_some() {
                    zenmon_core::ZenmonError::internal(format!(
                        "zenmon {available} was installed, but the zenmon-tray update \
                         failed: {err}"
                    ))
                } else {
                    err
                });
            }
            if !json {
                match &installed.version {
                    Some(version) => println!(
                        "installed zenmon-tray {available} (was {version}) at {}",
                        installed.path.display()
                    ),
                    None => println!(
                        "installed zenmon-tray {available} at {}",
                        installed.path.display()
                    ),
                }
            }
            tray_updated = Some(installed.clone());
        }

        if json {
            println!(
                "{}",
                serde_json::to_string(&serde_json::json!({
                    "ok": true,
                    "installed": cli_outcome.is_some(),
                    "from": current.to_string(),
                    "to": available.to_string(),
                    "path": cli_outcome.as_ref().map(|(i, _)| i.exe.display().to_string()),
                    "retained_old_binaries": cli_outcome.as_ref().map_or(0, |(_, o)| o.retained),
                    "remote": remote_json(&remote),
                    "tray": tray_updated.as_ref().map(|t| serde_json::json!({
                        "installed": true,
                        "from": t.version.as_ref().map(|v| v.to_string()),
                        "to": available.to_string(),
                        "path": t.path.display().to_string(),
                    })),
                }))?
            );
        }
        Ok(())
    }
}
