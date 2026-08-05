//! Fetching a manifest and its artifacts from a remote, for both kinds.
//!
//! The two kinds converge deliberately: a `github` remote and a `path` remote
//! carry the identical file set, so once the manifest bytes are in hand the
//! rest of the updater does not know or care which one it came from. Only
//! artifact resolution differs — a name in a release's asset list, or a name
//! next to the manifest on disk.

use std::path::{Path, PathBuf};
use std::time::Duration;

use zenmon_core::error::{Result, ZenmonError};
use zenmon_core::remotes::RemoteSpec;

use super::github::{self, Asset, TOKEN_ENV};
use super::manifest::{ReleaseArtifact, ReleaseManifest, MANIFEST_NAME};

const HTTP_TIMEOUT: Duration = Duration::from_secs(120);

/// Where a manifest came from, kept only so artifact URLs can be resolved the
/// same way the manifest was.
#[derive(Debug)]
pub enum ManifestSource {
    Github {
        repo: String,
        tag: String,
        assets: Vec<Asset>,
    },
    Directory(PathBuf),
}

impl ManifestSource {
    /// Human-readable origin, for messages.
    pub fn describe(&self) -> String {
        match self {
            ManifestSource::Github { repo, tag, .. } => format!("{repo} {tag}"),
            ManifestSource::Directory(dir) => dir.display().to_string(),
        }
    }
}

fn http_client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent(concat!("zenmon/", env!("CARGO_PKG_VERSION")))
        .timeout(HTTP_TIMEOUT)
        .build()
        .map_err(|err| ZenmonError::internal(format!("could not create an HTTP client: {err}")))
}

fn with_token(request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    match std::env::var(TOKEN_ENV) {
        Ok(token) if !token.trim().is_empty() => request.bearer_auth(token.trim()),
        _ => request,
    }
}

async fn get_bytes(
    client: &reqwest::Client,
    url: &str,
    accept: &str,
    repo: &str,
) -> Result<Vec<u8>> {
    let response = with_token(client.get(url).header(reqwest::header::ACCEPT, accept))
        .send()
        .await
        .map_err(|err| ZenmonError::connection(format!("could not reach {url}: {err}")))?;

    let status = response.status();
    if !status.is_success() {
        return Err(ZenmonError::connection(github::describe_status(
            status, repo,
        )));
    }
    response
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|err| ZenmonError::connection(format!("download from {url} failed: {err}")))
}

/// Resolves a remote to the newest release's manifest.
pub async fn fetch_manifest(spec: &RemoteSpec) -> Result<(ReleaseManifest, ManifestSource)> {
    match spec {
        RemoteSpec::Github { repo } => fetch_github_manifest(repo).await,
        RemoteSpec::Path { path } => fetch_directory_manifest(Path::new(path)),
    }
}

async fn fetch_github_manifest(repo: &str) -> Result<(ReleaseManifest, ManifestSource)> {
    let client = http_client()?;
    let list_url = format!("https://api.github.com/repos/{repo}/releases?per_page=100");
    let body = get_bytes(&client, &list_url, "application/vnd.github+json", repo).await?;
    let releases = github::parse_releases(&body, repo)?;

    let release = github::select_latest(&releases).ok_or_else(|| {
        ZenmonError::not_found(format!(
            "{repo} has no published release tagged v<version>; \
             releases are cut by pushing a tag (see .github/workflows/release.yml)"
        ))
    })?;

    // A release without a manifest was not built by the release workflow. Say
    // that rather than reporting a missing file, which reads like corruption.
    let asset = github::find_asset(release, MANIFEST_NAME).ok_or_else(|| {
        ZenmonError::not_found(format!(
            "release {} of {repo} has no {MANIFEST_NAME}; it was not produced by the \
             release workflow and cannot be installed",
            release.tag_name
        ))
    })?;

    let bytes = get_bytes(&client, &asset.url, "application/octet-stream", repo).await?;
    let manifest = ReleaseManifest::parse(&bytes, &format!("{repo} {}", release.tag_name))?;

    Ok((
        manifest,
        ManifestSource::Github {
            repo: repo.to_owned(),
            tag: release.tag_name.clone(),
            assets: release.assets.clone(),
        },
    ))
}

fn fetch_directory_manifest(dir: &Path) -> Result<(ReleaseManifest, ManifestSource)> {
    let path = dir.join(MANIFEST_NAME);
    let bytes = std::fs::read(&path).map_err(|err| match err.kind() {
        // The USB-stick case: the directory is a perfectly good remote that
        // simply is not mounted right now.
        std::io::ErrorKind::NotFound => ZenmonError::not_found(format!(
            "no {MANIFEST_NAME} in {}; is the directory present and does it hold a release?",
            dir.display()
        )),
        _ => ZenmonError::internal(format!("could not read {}: {err}", path.display())),
    })?;

    let manifest = ReleaseManifest::parse(&bytes, &path.display().to_string())?;
    Ok((manifest, ManifestSource::Directory(dir.to_path_buf())))
}

/// Downloads (or reads) one artifact's bytes. The caller verifies the checksum.
pub async fn fetch_artifact(
    source: &ManifestSource,
    artifact: &ReleaseArtifact,
) -> Result<Vec<u8>> {
    reject_non_relative_url(&artifact.url)?;

    match source {
        ManifestSource::Github { repo, tag, assets } => {
            let asset = assets
                .iter()
                .find(|asset| asset.name == artifact.url)
                .ok_or_else(|| {
                    ZenmonError::not_found(format!(
                        "release {tag} of {repo} lists {} in its manifest but has no such asset",
                        artifact.url
                    ))
                })?;
            let client = http_client()?;
            get_bytes(&client, &asset.url, "application/octet-stream", repo).await
        }
        ManifestSource::Directory(dir) => {
            let path = dir.join(&artifact.url);
            std::fs::read(&path).map_err(|err| match err.kind() {
                std::io::ErrorKind::NotFound => ZenmonError::not_found(format!(
                    "{} names {} but the file is not there",
                    dir.join(MANIFEST_NAME).display(),
                    artifact.url
                )),
                _ => ZenmonError::internal(format!("could not read {}: {err}", path.display())),
            })
        }
    }
}

/// Artifact URLs are manifest-relative file names by contract. Enforcing that
/// keeps a manifest from pointing the updater at an arbitrary host, and keeps
/// a `path` remote from reading outside its own directory.
fn reject_non_relative_url(url: &str) -> Result<()> {
    let looks_absolute = url.contains("://")
        || url.starts_with('/')
        || url.starts_with('\\')
        || url.contains("..")
        || Path::new(url).components().count() != 1;

    if looks_absolute {
        return Err(ZenmonError::invalid_input(format!(
            "release manifest artifact url {url:?} is not a plain file name; \
             manifests must reference artifacts sitting next to them"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn artifact(url: &str) -> ReleaseArtifact {
        ReleaseArtifact {
            target: "linux-x86_64".to_owned(),
            url: url.to_owned(),
            sha256: "a".repeat(64),
            binary: "zenmon".to_owned(),
        }
    }

    #[test]
    fn accepts_a_plain_file_name() {
        assert!(reject_non_relative_url("zenmon-0.1.0-x86_64-unknown-linux-gnu.tar.gz").is_ok());
        assert!(reject_non_relative_url("zenmon.zip").is_ok());
    }

    /// A manifest that could name an absolute URL would let whoever writes it
    /// redirect the download anywhere, and a `path` remote could be walked out
    /// of with `..`.
    #[test]
    fn rejects_anything_that_escapes_the_manifest_directory() {
        for bad in [
            "https://evil.example.com/zenmon.tar.gz",
            "http://example.com/x",
            "/etc/passwd",
            "\\\\nas\\share\\x.zip",
            "../../../etc/passwd",
            "sub/dir/zenmon.zip",
            "..",
        ] {
            assert!(
                reject_non_relative_url(bad).is_err(),
                "{bad:?} should be rejected"
            );
        }
    }

    #[tokio::test]
    async fn a_missing_directory_remote_says_it_may_not_be_mounted() {
        let dir = std::env::temp_dir().join("zenmon-update-absent-remote");
        let _ = std::fs::remove_dir_all(&dir);

        let err = fetch_directory_manifest(&dir).unwrap_err();
        let message = err.to_string();
        assert!(message.contains("zenmon.json"), "{message}");
        assert!(message.contains("directory present"), "{message}");
    }

    #[tokio::test]
    async fn reads_a_manifest_and_artifact_from_a_directory_remote() {
        let dir = std::env::temp_dir().join("zenmon-update-dir-remote");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join(MANIFEST_NAME),
            format!(
                r#"{{"schemaVersion":1,"version":"0.2.0","channel":"stable","artifacts":[
                    {{"target":"linux-x86_64","url":"a.tar.gz","sha256":"{}","binary":"zenmon"}}]}}"#,
                "a".repeat(64)
            ),
        )
        .unwrap();
        std::fs::write(dir.join("a.tar.gz"), b"payload").unwrap();

        let (manifest, source) = fetch_directory_manifest(&dir).unwrap();
        assert_eq!(manifest.version, "0.2.0");

        let bytes = fetch_artifact(&source, &artifact("a.tar.gz"))
            .await
            .unwrap();
        assert_eq!(bytes, b"payload");

        let err = fetch_artifact(&source, &artifact("missing.tar.gz"))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("not there"), "{err}");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
