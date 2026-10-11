//! Facts about an installed `zenmon` binary that the CLI's `update` and the
//! tray's "Install CLI" must agree on: where cargo puts binaries, whether two
//! spellings name one directory, and what version a binary reports.
//!
//! **zenmon application internals — not part of the public API.** Both used to
//! carry their own copies, and they drifted: the tray compared lowercased
//! strings while the CLI canonicalized, so the two could disagree on whether
//! a symlinked `~/.cargo/bin` was cargo's.

use std::path::{Path, PathBuf};

use semver::Version;

/// `$CARGO_HOME/bin`, else `~/.cargo/bin`.
pub fn cargo_bin_dir() -> Option<PathBuf> {
    if let Some(home) = std::env::var_os("CARGO_HOME") {
        return Some(PathBuf::from(home).join("bin"));
    }
    directories::BaseDirs::new().map(|d| d.home_dir().join(".cargo").join("bin"))
}

/// Whether two paths name the same directory.
///
/// Canonicalizes when both exist, so a symlinked or `..`-containing spelling
/// still matches. Otherwise compares text with trailing separators ignored —
/// and case ignored on Windows — which is how PATH entries behave there.
pub fn same_dir(left: &Path, right: &Path) -> bool {
    if let (Ok(l), Ok(r)) = (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        return l == r;
    }
    let norm = |p: &Path| {
        let s = p.to_string_lossy();
        let s = s.trim_end_matches(['\\', '/']);
        if cfg!(windows) {
            s.to_lowercase()
        } else {
            s.to_string()
        }
    };
    norm(left) == norm(right)
}

/// The first semver in a `--version` line, tolerating anything around it and
/// a leading `v`. An exact-string match breaks the moment the output carries
/// anything extra, such as a build timestamp.
pub fn parse_version_output(output: &str) -> Option<Version> {
    output
        .split_whitespace()
        .find_map(|token| Version::parse(token.trim_start_matches('v')).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_version_from_output_with_extra_text_around_it() {
        assert_eq!(
            parse_version_output("zenmon 0.1.0"),
            Some(Version::new(0, 1, 0))
        );
        assert_eq!(
            parse_version_output("zenmon 0.2.1 (built 2026-08-05T10:00:00Z)"),
            Some(Version::new(0, 2, 1))
        );
        assert_eq!(
            parse_version_output("zenmon v1.4.0\n"),
            Some(Version::new(1, 4, 0))
        );
        assert_eq!(
            parse_version_output("zenmon 0.3.0-rc.1")
                .unwrap()
                .to_string(),
            "0.3.0-rc.1"
        );
        assert_eq!(parse_version_output("no version here"), None);
        assert_eq!(parse_version_output(""), None);
    }

    #[test]
    fn same_dir_ignores_a_trailing_separator_on_missing_paths() {
        let a = Path::new("/no/such/zenmon/dir");
        assert!(same_dir(a, Path::new("/no/such/zenmon/dir/")));
        assert!(!same_dir(a, Path::new("/no/such/zenmon/other")));
    }

    #[test]
    fn same_dir_sees_through_a_dotdot_spelling() {
        let base = std::env::temp_dir().join(format!("zenmon-samedir-{}", std::process::id()));
        let sub = base.join("bin");
        std::fs::create_dir_all(&sub).unwrap();
        assert!(same_dir(&sub, &sub.join("..").join("bin")));
        let _ = std::fs::remove_dir_all(&base);
    }
}
