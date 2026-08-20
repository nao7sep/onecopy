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
//!   `<root>/bin/ffmpeg.json`         its version sidecar, where the binary
//!                                    cannot report a comparable version
//!   `<root>/temp/…`                  download staging, wiped at launch
//!   `<root>/dependencies.json`       recorded facts, its own store
//!
//! Facts persist only what cannot be re-derived, and both survivors are
//! NETWORK facts with no on-disk source: `latestKnownVersion` and
//! `lastCheckedAtUtc`. Presence is re-scanned from disk, and so is the
//! INSTALLED version — read from the artifact itself, never persisted beside
//! a copy of it. `installedVersion` used to live in the facts store, one file
//! away from the thing it described, where any install that failed to write
//! the record stranded a present artifact as permanently unversioned; the
//! derivation can only read that as "installed (not checked)", so the update
//! that exists is never offered. A FAILED check still writes nothing (the
//! honest-state rule), so "up to date" always means a check actually
//! succeeded.

use serde::{Deserialize, Serialize};

/// Persisted facts for one managed binary (`dependencies.json` value shape).
///
/// An older file's `installedVersion` is simply not read here, so it drops on
/// the next write (the app is pre-release; no migration code).
#[derive(Serialize, Deserialize, Clone, Default, Debug, PartialEq, Eq)]
#[serde(rename_all = "camelCase", default)]
pub struct BinaryFacts {
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

/// Whether a managed binary at `path` is actually usable.
///
/// `is_file()` alone is not enough: a zero-byte placeholder, or a file whose
/// executable bit was lost (an unzip without permissions, a copy across a
/// filesystem that drops the mode), reports installed and then fails at the
/// first invocation — with the UI insisting the tool is up to date.
pub fn is_usable_binary(path: &std::path::Path) -> bool {
    let Ok(meta) = std::fs::metadata(path) else {
        return false;
    };
    if !meta.is_file() || meta.len() == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        return meta.permissions().mode() & 0o111 != 0;
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Derives the display status from usable-presence, the version read from the
/// artifact, and the persisted latest.
///
/// `installed` is None when a present artifact's version could not be read —
/// the binary would not run, or its sidecar is missing. That is NOT the same
/// as absent and must never read as up to date: there is nothing to compare,
/// so the row holds at InstalledUnchecked and the surface offers the
/// re-acquire that fixes it.
pub fn derive_status(
    present: bool,
    installed: Option<&str>,
    facts: &BinaryFacts,
) -> BinaryStatus {
    if !present {
        return BinaryStatus::NotInstalled;
    }
    match (installed, facts.latest_known_version.as_deref()) {
        (Some(installed), Some(latest)) if installed != latest => BinaryStatus::UpdateAvailable,
        (Some(_), Some(_)) => BinaryStatus::UpToDate,
        _ => BinaryStatus::InstalledUnchecked,
    }
}

/// Strips vendor noise so the installed and latest versions are compared on
/// the same form (the convention's "normalize before comparing"):
/// martin-riedl appends `-https://www.martin-riedl.de` to ffmpeg's version,
/// and release tags carry a leading `v`. Applied to BOTH sides, since the two
/// now come from different sources and only agree once normalized.
pub fn normalize_version(raw: &str) -> String {
    let mut value = raw.trim();
    if let Some(at) = value.find("-http") {
        let (head, tail) = value.split_at(at);
        if tail.starts_with("-http://") || tail.starts_with("-https://") {
            value = head;
        }
    }
    value = value.strip_prefix('v').unwrap_or(value);
    value.trim().to_string()
}

/// Reads the version out of ffmpeg's own banner, whose first line is
/// `ffmpeg version 8.1.2-https://www.martin-riedl.de Copyright (c) …`. The
/// normalized result is the upstream release that the martin-riedl build id
/// also names, so the two compare directly. Output that does not match yields
/// None rather than becoming a version.
pub fn parse_ffmpeg_version(stdout: &str) -> Option<String> {
    let first = stdout.lines().find(|line| !line.trim().is_empty())?.trim();
    let rest = first.strip_prefix("ffmpeg version ")?;
    let token = rest.split_whitespace().next()?;
    let version = normalize_version(token);
    (!version.is_empty()).then_some(version)
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
