//! Managed external binaries (ffmpeg now; whisper later), per the
//! managed-runtime-dependencies conventions.
//!
//! This module holds the PURE half: release-version parsing, checksum-file
//! parsing, and the four-state status derivation over persisted facts. The
//! network/orchestration half (download with idle watchdog, staged install
//! with integrity verify, per-binary locking) builds on these.
//!
//! On-disk layout (the conventions' standard tree):
//!   `<root>/bin/ffmpeg[.exe]`        the installed executable
//!   `<root>/temp/…`                  download staging, wiped at launch
//!   `<root>/dependencies.json`       recorded facts, its own store
//!
//! Facts persist only what cannot be re-derived: `installedVersion`,
//! `latestKnownVersion`, `lastCheckedAtUtc`. Presence is re-scanned from disk;
//! a FAILED check writes nothing (the honest-state rule), so "up to date"
//! always means a check actually succeeded.

use serde::{Deserialize, Serialize};

/// Persisted facts for one managed binary (`dependencies.json` value shape).
#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct BinaryFacts {
    pub installed_version: Option<String>,
    pub latest_known_version: Option<String>,
    pub last_checked_at_utc: Option<String>,
}

/// The four display states, derived — never persisted.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BinaryStatus {
    NotInstalled,
    UpdateAvailable,
    UpToDate,
    InstalledUnchecked,
}

/// Derives the display status from presence-on-disk plus the persisted facts.
pub fn derive_status(present: bool, facts: &BinaryFacts) -> BinaryStatus {
    if !present {
        return BinaryStatus::NotInstalled;
    }
    match (&facts.installed_version, &facts.latest_known_version) {
        (Some(installed), Some(latest)) if installed != latest => BinaryStatus::UpdateAvailable,
        (Some(_), Some(_)) => BinaryStatus::UpToDate,
        _ => BinaryStatus::InstalledUnchecked,
    }
}

/// Parses a martin-riedl.de macOS build URL's version. The download redirects
/// into a path whose final segments look like `.../<epoch>_<version>/ffmpeg.zip`;
/// the version is everything after the first underscore of that folder segment
/// (probed live against the real endpoint before this was encoded).
pub fn parse_martin_build_version(url_path: &str) -> Option<String> {
    for segment in url_path.split('/').rev() {
        if let Some((epoch, version)) = segment.split_once('_') {
            if !epoch.is_empty()
                && epoch.bytes().all(|b| b.is_ascii_digit())
                && !version.is_empty()
            {
                return Some(version.to_string());
            }
        }
    }
    None
}

/// Parses a `SHA256SUMS`-style file: lines of `<64-hex>  [*]<filename>`.
/// Returns the hex digest for `asset_name`, or None when absent.
pub fn parse_sums(sums_text: &str, asset_name: &str) -> Option<String> {
    for line in sums_text.lines() {
        let line = line.trim();
        let mut parts = line.splitn(2, char::is_whitespace);
        let (Some(digest), Some(rest)) = (parts.next(), parts.next()) else {
            continue;
        };
        if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
            continue;
        }
        let name = rest.trim().trim_start_matches('*');
        if name == asset_name {
            return Some(digest.to_ascii_lowercase());
        }
    }
    None
}

/// The BtbN Windows asset name the registry pins (master rolling builds).
pub const BTBN_WIN64_ASSET: &str = "ffmpeg-master-latest-win64-gpl.zip";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_derivation_covers_all_four_states() {
        let facts = |installed: Option<&str>, latest: Option<&str>| BinaryFacts {
            installed_version: installed.map(String::from),
            latest_known_version: latest.map(String::from),
            last_checked_at_utc: None,
        };
        assert_eq!(
            derive_status(false, &facts(Some("7.1"), Some("7.1"))),
            BinaryStatus::NotInstalled
        );
        assert_eq!(
            derive_status(true, &facts(Some("7.0"), Some("7.1"))),
            BinaryStatus::UpdateAvailable
        );
        assert_eq!(
            derive_status(true, &facts(Some("7.1"), Some("7.1"))),
            BinaryStatus::UpToDate
        );
        assert_eq!(
            derive_status(true, &facts(Some("7.1"), None)),
            BinaryStatus::InstalledUnchecked
        );
        assert_eq!(
            derive_status(true, &facts(None, None)),
            BinaryStatus::InstalledUnchecked
        );
    }

    #[test]
    fn martin_version_parses_the_epoch_version_segment() {
        assert_eq!(
            parse_martin_build_version("/download/release/1719302400_7.0.1/ffmpeg.zip"),
            Some("7.0.1".to_string())
        );
        assert_eq!(
            parse_martin_build_version("/x/1719302400_7.0.1-tessus/ffmpeg.zip"),
            Some("7.0.1-tessus".to_string())
        );
        assert_eq!(parse_martin_build_version("/plain/ffmpeg.zip"), None);
        assert_eq!(parse_martin_build_version("/notepoch_x/ffmpeg.zip"), None);
    }

    #[test]
    fn sums_parsing_matches_exact_names_with_optional_star() {
        let sums = "\
0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef  ffmpeg-master-latest-win64-gpl.zip
fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210 *other.zip
not-a-digest  whatever.zip";
        assert_eq!(
            parse_sums(sums, BTBN_WIN64_ASSET).as_deref(),
            Some("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        );
        assert_eq!(
            parse_sums(sums, "other.zip").as_deref(),
            Some("fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210")
        );
        assert_eq!(parse_sums(sums, "missing.zip"), None);
        assert_eq!(parse_sums(sums, "whatever.zip"), None);
    }
}
