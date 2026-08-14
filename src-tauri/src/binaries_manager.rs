//! Orchestration half of the managed-binaries mechanism (the pure half lives
//! in `binaries.rs`): resolve the latest build per platform, download to
//! `temp/` staging, verify against the published checksums, extract, make the
//! staged file runnable BEFORE it reaches `bin/` (chmod + de-quarantine), then
//! publish with a same-volume rename. One install/check at a time; a failed
//! check writes nothing (the honest-state rule).
//!
//! Endpoint shapes verified live at build time:
//!   macOS arm64  https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffmpeg.zip
//!                → 307 to /download/macos/arm64/<epoch>_<version>/ffmpeg.zip,
//!                with a `<url>.sha256` sidecar (`<hex>  ffmpeg.zip`).
//!   Windows x64  GitHub latest release of BtbN/FFmpeg-Builds: the
//!                `ffmpeg-master-latest-win64-gpl.zip` asset plus a
//!                `checksums.sha256` asset; the release name is the version.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};
use ureq::ResponseExt;

use crate::binaries::{self, BinaryFacts, BinaryStatus};
use crate::{logging, nanoid, storage};

// Subpath names are owned by the one resolver module (storage-path
// conventions); re-exported here so existing call sites keep their imports.
pub use crate::paths::{BIN_DIR_NAME, DEPENDENCIES_FILE_NAME, TEMP_DIR_NAME};

const MARTIN_REDIRECT_URL: &str =
    "https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffmpeg.zip";
const BTBN_LATEST_API: &str = "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/latest";

pub fn ffmpeg_file_name() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

pub fn ffmpeg_path(root: &Path) -> PathBuf {
    root.join(BIN_DIR_NAME).join(ffmpeg_file_name())
}

/// Wipes and recreates the staging dir — crash debris in `temp/` is worthless
/// by definition. Called once at launch.
pub fn reset_temp_dir(root: &Path) {
    let temp = root.join(TEMP_DIR_NAME);
    let _ = std::fs::remove_dir_all(&temp);
    let _ = std::fs::create_dir_all(&temp);
}

// --- The facts store: its own file, self-healing (missing OR corrupt →
// fresh defaults; the opposite of the config store's quarantine, because
// every field is re-derivable by a check). ---

pub fn load_facts(root: &Path) -> BinaryFacts {
    let file = root.join(DEPENDENCIES_FILE_NAME);
    let Ok(bytes) = std::fs::read(&file) else {
        return BinaryFacts::default();
    };
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| serde_json::from_value(v.get("ffmpeg")?.clone()).ok())
        .unwrap_or_default()
}

pub fn save_facts(root: &Path, facts: &BinaryFacts) -> Result<(), String> {
    // records: dependencies.json rides write_atomic's backup hook. The facts
    // are re-derivable, so recording is not REQUIRED — but tiny self-healing
    // text in the store is harmless, and a separate unrecorded write path
    // would cost more than it saves (accounted in storage.rs's table).
    let value = serde_json::json!({ "ffmpeg": facts });
    let mut text = serde_json::to_string_pretty(&value).map_err(|e| e.to_string())?;
    text.push('\n');
    storage::write_atomic(&root.join(DEPENDENCIES_FILE_NAME), text.as_bytes())
}

// --- Resolution ---

pub struct Resolved {
    pub version: String,
    pub download_url: String,
    pub sums_url: String,
    pub sums_asset: String,
}

fn agent(timeout_secs: u64) -> ureq::Agent {
    ureq::Agent::new_with_config(
        ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_secs(timeout_secs)))
            .save_redirect_history(true)
            .build(),
    )
}

fn assert_https(url: &str) -> Result<(), String> {
    if url.starts_with("https://") {
        Ok(())
    } else {
        Err(format!("refusing non-https download url: {url}"))
    }
}

/// Resolves the latest ffmpeg build for this platform. Errors on platforms
/// with no managed build (Linux, Intel macs) — manual install applies there.
pub fn resolve_latest() -> Result<Resolved, String> {
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        // The GET follows the 307; the final URI carries <epoch>_<version>.
        // The body is dropped unread — resolution needs only the redirect.
        let response = agent(60)
            .get(MARTIN_REDIRECT_URL)
            .call()
            .map_err(|e| e.to_string())?;
        let final_uri = response.get_uri().to_string();
        let version = binaries::parse_martin_build_version(response.get_uri().path())
            .ok_or_else(|| format!("no <epoch>_<version> segment in {final_uri}"))?;
        Ok(Resolved {
            version,
            sums_url: format!("{final_uri}.sha256"),
            sums_asset: "ffmpeg.zip".to_string(),
            download_url: final_uri,
        })
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        let mut response = agent(60)
            .get(BTBN_LATEST_API)
            .header("User-Agent", "onecopy")
            .call()
            .map_err(|e| e.to_string())?;
        let body = response
            .body_mut()
            .read_to_string()
            .map_err(|e| e.to_string())?;
        let release: serde_json::Value =
            serde_json::from_str(&body).map_err(|e| e.to_string())?;
        let version = release
            .get("name")
            .and_then(|n| n.as_str())
            .unwrap_or("latest")
            .to_string();
        let asset_url = |name: &str| -> Option<String> {
            release.get("assets")?.as_array()?.iter().find_map(|a| {
                (a.get("name")?.as_str()? == name)
                    .then(|| a.get("browser_download_url")?.as_str().map(String::from))?
            })
        };
        Ok(Resolved {
            version,
            download_url: asset_url(binaries::BTBN_WIN64_ASSET)
                .ok_or_else(|| format!("release has no {}", binaries::BTBN_WIN64_ASSET))?,
            sums_url: asset_url("checksums.sha256")
                .ok_or_else(|| "release has no checksums.sha256".to_string())?,
            sums_asset: binaries::BTBN_WIN64_ASSET.to_string(),
        })
    } else {
        Err("no managed ffmpeg build for this platform; install ffmpeg manually".to_string())
    }
}

// --- Download / verify / extract / publish ---

// not recorded: download staging — binary bytes into temp/, verified then
// published by rename; outside the managed-text backup path by design.
fn download_to(url: &str, dest: &Path, mut on_progress: impl FnMut(u64)) -> Result<u64, String> {
    assert_https(url)?;
    let mut response = agent(900).get(url).call().map_err(|e| e.to_string())?;
    let mut reader = response.body_mut().as_reader();
    let mut file = std::fs::File::create(dest).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        total += n as u64;
        on_progress(total);
    }
    file.sync_all().map_err(|e| e.to_string())?;
    Ok(total)
}

fn fetch_text(url: &str) -> Result<String, String> {
    assert_https(url)?;
    agent(60)
        .get(url)
        .header("User-Agent", "onecopy")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Pulls the ffmpeg entry out of the archive: an exact-basename match, unique
/// across the archive (ambiguity is an error — a multi-match archive means the
/// upstream layout changed, and guessing which binary to install is not safe).
fn extract_ffmpeg(archive_path: &Path, staged: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let wanted = ffmpeg_file_name();
    let matches: Vec<String> = archive
        .file_names()
        .filter(|name| {
            name.rsplit(['/', '\\']).next() == Some(wanted) && !name.ends_with('/')
        })
        .map(String::from)
        .collect();
    let inner = match matches.as_slice() {
        [one] => one.clone(),
        [] => return Err(format!("archive holds no {wanted}")),
        many => return Err(format!("archive holds {} candidates for {wanted}", many.len())),
    };
    let mut entry = archive.by_name(&inner).map_err(|e| e.to_string())?;
    // not recorded: staged binary extraction (temp/), published by rename below.
    let mut out = std::fs::File::create(staged).map_err(|e| e.to_string())?;
    std::io::copy(&mut entry, &mut out).map_err(|e| e.to_string())?;
    out.sync_all().map_err(|e| e.to_string())?;
    Ok(())
}

/// Everything that must be true of the binary BEFORE it reaches `bin/`:
/// native architecture (macOS), executable bit, quarantine attribute
/// stripped (macOS best-effort).
fn make_runnable(staged: &Path) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // The conventions' arch gate applied to bytes the app didn't build:
        // an x86_64-only download must fail HERE, not as Rosetta jank later.
        let header = {
            use std::io::Read;
            let mut file = std::fs::File::open(staged).map_err(|e| e.to_string())?;
            let mut buf = [0u8; 4096];
            let n = file.read(&mut buf).map_err(|e| e.to_string())?;
            buf[..n].to_vec()
        };
        if !macho_has_arm64(&header) {
            return Err("downloaded binary carries no native arm64 slice".to_string());
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("xattr")
            .args(["-d", "com.apple.quarantine"])
            .arg(staged)
            .status();
    }
    Ok(())
}

/// True when the Mach-O header carries an arm64 slice: a thin 64-bit binary
/// with CPU_TYPE_ARM64, or a fat binary listing one. A header parse instead
/// of shelling to `lipo`, so the gate needs no developer tooling installed.
fn macho_has_arm64(header: &[u8]) -> bool {
    const CPU_ARM64: u32 = 0x0100_000C;
    if header.len() < 8 {
        return false;
    }
    let magic_be = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    // Thin 64-bit Mach-O, stored little-endian on disk: MH_MAGIC_64.
    if u32::from_le_bytes([header[0], header[1], header[2], header[3]]) == 0xFEED_FACF {
        return u32::from_le_bytes([header[4], header[5], header[6], header[7]]) == CPU_ARM64;
    }
    // Fat/universal binary: big-endian header listing per-arch entries
    // (FAT_MAGIC; FAT_MAGIC_64 entries are 32 bytes instead of 20).
    if magic_be == 0xCAFE_BABE || magic_be == 0xCAFE_BABF {
        let entry_size = if magic_be == 0xCAFE_BABF { 32 } else { 20 };
        let count = u32::from_be_bytes([header[4], header[5], header[6], header[7]]) as usize;
        for i in 0..count.min(64) {
            let at = 8 + i * entry_size;
            if at + 4 > header.len() {
                return false;
            }
            let cputype =
                u32::from_be_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]]);
            if cputype == CPU_ARM64 {
                return true;
            }
        }
    }
    false
}

static BUSY: AtomicBool = AtomicBool::new(false);

struct BusyGuard;
impl Drop for BusyGuard {
    fn drop(&mut self) {
        BUSY.store(false, Ordering::SeqCst);
    }
}

fn claim() -> Result<BusyGuard, String> {
    if BUSY.swap(true, Ordering::SeqCst) {
        return Err("a binaries operation is already running".to_string());
    }
    Ok(BusyGuard)
}

/// The full install/update: resolve → download → verify → extract → make
/// runnable → publish → record facts.
pub fn install_or_update(
    root: &Path,
    mut on_progress: impl FnMut(&str, String),
) -> Result<BinaryFacts, String> {
    let _guard = claim()?;
    let temp = root.join(TEMP_DIR_NAME);
    std::fs::create_dir_all(&temp).map_err(|e| e.to_string())?;

    on_progress("resolve", "finding the latest build".to_string());
    let resolved = resolve_latest()?;

    let partial = temp.join(format!("ffmpeg-{}.partial", nanoid::generate()));
    let result = (|| -> Result<BinaryFacts, String> {
        on_progress("download", format!("v{} from {}", resolved.version, resolved.download_url));
        let bytes = download_to(&resolved.download_url, &partial, |done| {
            on_progress("download", format!("{} MB", done / 1_048_576));
        })?;
        on_progress("verify", format!("{bytes} bytes downloaded"));
        let expected = binaries::parse_sums(&fetch_text(&resolved.sums_url)?, &resolved.sums_asset)
            .ok_or_else(|| format!("{} not in the checksum file", resolved.sums_asset))?;
        let actual = file_sha256(&partial)?;
        if actual != expected {
            return Err(format!(
                "checksum mismatch for {}: expected {expected}, got {actual}",
                resolved.sums_asset
            ));
        }

        on_progress("install", "extracting".to_string());
        let staged = temp.join(format!("ffmpeg-{}.staged", nanoid::generate()));
        extract_ffmpeg(&partial, &staged)?;
        make_runnable(&staged)?;

        let target = ffmpeg_path(root);
        std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
        // Replace-in-place: rename over any previous install (same volume).
        // not recorded: the installed executable is a re-downloadable binary.
        std::fs::rename(&staged, &target).map_err(|e| e.to_string())?;

        let facts = BinaryFacts {
            installed_version: Some(resolved.version.clone()),
            latest_known_version: Some(resolved.version.clone()),
            last_checked_at_utc: Some(logging::now_iso_millis()),
        };
        save_facts(root, &facts)?;
        logging::info(
            "ffmpeg installed",
            serde_json::json!({ "version": resolved.version, "path": target.to_string_lossy() }),
        );
        Ok(facts)
    })();
    let _ = std::fs::remove_file(&partial);
    result
}

/// Version check only. Success updates `latestKnownVersion` + the check stamp;
/// failure writes NOTHING so stale knowledge is never dressed up as fresh.
pub fn check_for_updates(root: &Path) -> Result<BinaryFacts, String> {
    let _guard = claim()?;
    let resolved = resolve_latest()?;
    let mut facts = load_facts(root);
    facts.latest_known_version = Some(resolved.version);
    facts.last_checked_at_utc = Some(logging::now_iso_millis());
    save_facts(root, &facts)?;
    Ok(facts)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FfmpegState {
    pub status: BinaryStatus,
    pub facts: BinaryFacts,
    pub path: String,
}

/// Presence re-scanned from disk every call; never persisted.
pub fn state(root: &Path) -> FfmpegState {
    let path = ffmpeg_path(root);
    let facts = load_facts(root);
    FfmpegState {
        status: binaries::derive_status(binaries::is_usable_binary(&path), &facts),
        path: path.to_string_lossy().to_string(),
        facts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macho_arm64_detection_covers_thin_fat_and_foreign() {
        // Thin arm64: MH_MAGIC_64 little-endian + CPU_TYPE_ARM64.
        let mut thin_arm = vec![0xCF, 0xFA, 0xED, 0xFE];
        thin_arm.extend_from_slice(&0x0100_000Cu32.to_le_bytes());
        assert!(macho_has_arm64(&thin_arm));

        // Thin x86_64: same magic, CPU_TYPE_X86_64 — rejected.
        let mut thin_x86 = vec![0xCF, 0xFA, 0xED, 0xFE];
        thin_x86.extend_from_slice(&0x0100_0007u32.to_le_bytes());
        assert!(!macho_has_arm64(&thin_x86));

        // Fat binary with x86_64 then arm64 slices — accepted.
        let mut fat = Vec::new();
        fat.extend_from_slice(&0xCAFE_BABEu32.to_be_bytes());
        fat.extend_from_slice(&2u32.to_be_bytes());
        fat.extend_from_slice(&0x0100_0007u32.to_be_bytes()); // x86_64 entry
        fat.extend_from_slice(&[0u8; 16]); // rest of the 20-byte fat_arch
        fat.extend_from_slice(&0x0100_000Cu32.to_be_bytes()); // arm64 entry
        fat.extend_from_slice(&[0u8; 16]);
        assert!(macho_has_arm64(&fat));

        // Fat with only x86_64 — rejected; garbage — rejected.
        let mut fat_x86 = Vec::new();
        fat_x86.extend_from_slice(&0xCAFE_BABEu32.to_be_bytes());
        fat_x86.extend_from_slice(&1u32.to_be_bytes());
        fat_x86.extend_from_slice(&0x0100_0007u32.to_be_bytes());
        fat_x86.extend_from_slice(&[0u8; 16]);
        assert!(!macho_has_arm64(&fat_x86));
        assert!(!macho_has_arm64(b"#!/bin/sh\n"));
        assert!(!macho_has_arm64(&[]));
    }

    #[test]
    fn facts_store_self_heals_on_missing_and_corrupt() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-binmgr-")
            .tempdir()
            .unwrap();
        // Missing → defaults.
        assert_eq!(load_facts(dir.path()), BinaryFacts::default());
        // Corrupt → defaults, no quarantine (re-derivable facts).
        std::fs::write(dir.path().join(DEPENDENCIES_FILE_NAME), b"{ not json").unwrap();
        assert_eq!(load_facts(dir.path()), BinaryFacts::default());
    }

    #[test]
    #[serial_test::serial(backup_store)]
    fn facts_round_trip_through_the_store() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-binmgr-rt-")
            .tempdir()
            .unwrap();
        let facts = BinaryFacts {
            installed_version: Some("9.0".into()),
            latest_known_version: Some("9.1".into()),
            last_checked_at_utc: Some("2026-08-08T12:00:00.000Z".into()),
        };
        save_facts(dir.path(), &facts).unwrap();
        assert_eq!(load_facts(dir.path()), facts);
    }

    #[test]
    fn reset_temp_dir_wipes_and_recreates() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-binmgr-temp-")
            .tempdir()
            .unwrap();
        let temp = dir.path().join(TEMP_DIR_NAME);
        std::fs::create_dir_all(&temp).unwrap();
        std::fs::write(temp.join("debris.partial"), b"x").unwrap();
        reset_temp_dir(dir.path());
        assert!(temp.is_dir());
        assert_eq!(std::fs::read_dir(&temp).unwrap().count(), 0);
    }

    // The live end-to-end: resolves, downloads (~50-80 MB), verifies against
    // the published checksum, extracts, publishes, and runs `ffmpeg -version`.
    // Ignored in the routine suite; run explicitly with
    // `cargo test live_install_ffmpeg -- --ignored --nocapture`.
    #[test]
    #[ignore]
    #[serial_test::serial(backup_store)]
    fn live_install_ffmpeg() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-binmgr-live-")
            .tempdir()
            .unwrap();
        let facts = install_or_update(dir.path(), |phase, detail| {
            eprintln!("[{phase}] {detail}");
        })
        .expect("live install should succeed");
        assert!(facts.installed_version.is_some());

        let path = ffmpeg_path(dir.path());
        assert!(path.is_file());
        let output = std::process::Command::new(&path)
            .arg("-version")
            .output()
            .expect("installed ffmpeg should run");
        assert!(output.status.success());
        let banner = String::from_utf8_lossy(&output.stdout);
        assert!(banner.starts_with("ffmpeg version"), "banner: {banner}");

        assert_eq!(state(dir.path()).status, BinaryStatus::UpToDate);
    }
}
