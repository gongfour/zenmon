//! The slice of the GitHub releases API `zenmon update` needs.

use semver::Version;
use serde::Deserialize;
use zenmon_core::error::{Result, ZenmonError};

use super::manifest::TAG_PREFIX;

/// Environment variable holding a bearer token.
///
/// The repository is public, so this is normally unset. It matters for two
/// cases: a private fork, and shared-IP networks where the 60-requests-per-hour
/// anonymous limit is reached by someone else on the same address.
pub const TOKEN_ENV: &str = "ZENMON_UPDATE_TOKEN";

#[derive(Debug, Deserialize)]
pub struct Release {
    pub tag_name: String,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Asset {
    pub name: String,
    /// The API URL. Used rather than `browser_download_url` because it accepts
    /// the bearer token, so the same code path serves a private fork.
    pub url: String,
}

/// The newest release, by semver, among tags shaped `v<semver>`.
///
/// Drafts and prereleases are skipped: this is the stable channel, and picking
/// up a `v0.3.0-rc1` because it sorts highest is not what a plain
/// `zenmon update` should do. Tags that are not zenmon releases (or not
/// semver) are ignored rather than being an error — a repository is allowed to
/// have other tags.
pub fn select_latest(releases: &[Release]) -> Option<&Release> {
    releases
        .iter()
        .filter(|release| !release.draft && !release.prerelease)
        .filter_map(|release| {
            let version = Version::parse(release.tag_name.strip_prefix(TAG_PREFIX)?).ok()?;
            Some((version, release))
        })
        .max_by(|(left, _), (right, _)| left.cmp(right))
        .map(|(_, release)| release)
}

pub fn find_asset<'a>(release: &'a Release, name: &str) -> Option<&'a Asset> {
    release.assets.iter().find(|asset| asset.name == name)
}

/// Turns a transport/status failure into advice, because the two likely causes
/// — rate limiting and a private repository — both look like a bare 403 and
/// have completely different fixes.
pub fn describe_status(status: reqwest::StatusCode, repo: &str) -> String {
    let authenticated = std::env::var_os(TOKEN_ENV).is_some_and(|v| !v.is_empty());
    match status.as_u16() {
        403 | 429 if !authenticated => format!(
            "GitHub refused the request for {repo} ({status}). Unauthenticated requests are \
             limited to 60 per hour per IP address, which a shared or NAT'd network can reach \
             without you doing anything. Set {TOKEN_ENV} to a token, or use a `path` remote."
        ),
        403 | 429 => format!(
            "GitHub refused the request for {repo} ({status}). The token in {TOKEN_ENV} may be \
             expired or lack read access to the repository."
        ),
        404 => format!(
            "GitHub has no repository {repo} that this request can see ({status}). Check the \
             name, or set {TOKEN_ENV} if it is private."
        ),
        _ => format!("GitHub request for {repo} failed: {status}"),
    }
}

pub fn parse_releases(bytes: &[u8], repo: &str) -> Result<Vec<Release>> {
    serde_json::from_slice(bytes).map_err(|err| {
        ZenmonError::internal(format!("could not read the release list for {repo}: {err}"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str, draft: bool, prerelease: bool) -> Release {
        Release {
            tag_name: tag.to_owned(),
            draft,
            prerelease,
            assets: vec![Asset {
                name: "zenmon.json".to_owned(),
                url: format!("https://api.github.com/{tag}/zenmon.json"),
            }],
        }
    }

    #[test]
    fn picks_the_highest_semver_not_the_first_listed() {
        let releases = vec![
            release("v0.2.0", false, false),
            release("v0.10.0", false, false),
            release("v0.9.0", false, false),
        ];
        // lexically "v0.9.0" > "v0.10.0"; semver disagrees, and semver wins
        assert_eq!(select_latest(&releases).unwrap().tag_name, "v0.10.0");
    }

    #[test]
    fn skips_drafts_and_prereleases() {
        let releases = vec![
            release("v0.2.0", false, false),
            release("v0.3.0", true, false),
            release("v0.4.0", false, true),
        ];
        assert_eq!(select_latest(&releases).unwrap().tag_name, "v0.2.0");
    }

    /// A repository may carry tags that are not zenmon releases; those are not
    /// an error, they simply are not candidates.
    #[test]
    fn ignores_tags_that_are_not_v_semver() {
        let releases = vec![
            release("nightly", false, false),
            release("v-not-semver", false, false),
            release("0.5.0", false, false), // no `v` prefix
            release("v0.1.0", false, false),
        ];
        assert_eq!(select_latest(&releases).unwrap().tag_name, "v0.1.0");
    }

    #[test]
    fn no_matching_release_is_none_rather_than_a_panic() {
        assert!(select_latest(&[]).is_none());
        assert!(select_latest(&[release("nightly", false, false)]).is_none());
    }

    #[test]
    fn finds_the_manifest_asset_by_exact_name() {
        let release = release("v0.1.0", false, false);
        assert!(find_asset(&release, "zenmon.json").is_some());
        assert!(find_asset(&release, "zenmon.json.sig").is_none());
        assert!(find_asset(&release, "ZENMON.JSON").is_none());
    }

    #[test]
    fn parses_the_release_list_shape_github_returns() {
        let json = br#"[
          {"tag_name":"v0.1.0","draft":false,"prerelease":false,
           "assets":[{"name":"zenmon.json","url":"https://api.github.com/a/1",
                      "browser_download_url":"https://github.com/x"}]},
          {"tag_name":"v0.2.0"}
        ]"#;
        let releases = parse_releases(json, "gongfour/zenmon").unwrap();
        assert_eq!(releases.len(), 2);
        // absent booleans and assets default rather than failing the parse
        assert!(!releases[1].draft);
        assert!(releases[1].assets.is_empty());
    }

    /// Rate limiting is the failure a public repository actually hits, and a
    /// bare "403" sends people looking in the wrong place.
    #[test]
    fn a_403_without_a_token_explains_the_rate_limit() {
        let message = describe_status(reqwest::StatusCode::FORBIDDEN, "gongfour/zenmon");
        assert!(message.contains("60 per hour"), "{message}");
        assert!(message.contains(TOKEN_ENV), "{message}");
    }

    #[test]
    fn a_404_suggests_the_repository_may_be_private() {
        let message = describe_status(reqwest::StatusCode::NOT_FOUND, "gongfour/zenmon");
        assert!(message.contains("private"), "{message}");
    }
}
