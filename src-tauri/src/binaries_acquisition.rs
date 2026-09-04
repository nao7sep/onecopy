//! Acquisition edge for managed dependencies: HTTPS resolution and bounded,
//! cancellable transfer; integrity hashing; archive extraction; and durable,
//! same-volume publication. Registry, facts, claims, and install orchestration
//! remain in `binaries_manager`.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::binaries;

const MARTIN_REDIRECT_URL: &str =
    "https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffmpeg.zip";
const BTBN_LATEST_API: &str = "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/latest";
const METADATA_TIMEOUT: Duration = Duration::from_secs(60);
const DOWNLOAD_IDLE_TIMEOUT: Duration = Duration::from_secs(120);
const DOWNLOAD_CANCEL_POLL: Duration = Duration::from_millis(50);
const METADATA_MAX_BYTES: u64 = 8 * 1024 * 1024;
const UNPINNED_MAX_DOWNLOAD_BYTES: u64 = 4 * 1024 * 1024 * 1024;
const FFMPEG_MAX_EXTRACTED_BYTES: u64 = 1024 * 1024 * 1024;
const MIN_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const MAX_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
const UNPINNED_DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(6 * 60 * 60);
const MIN_TOLERATED_BYTES_PER_SEC: u64 = 32 * 1024;
const POST_DOWNLOAD_BUDGET: Duration = Duration::from_secs(30 * 60);
const CHECK_OPERATION_TIMEOUT: Duration = Duration::from_secs(5 * 60);
pub(crate) const CANCELLED_ERROR: &str = "dependency operation cancelled";

pub(crate) struct OperationDeadline {
    started: std::time::Instant,
    timeout: Duration,
}

impl OperationDeadline {
    pub(crate) fn for_install(expected_bytes: Option<u64>) -> Self {
        Self {
            started: std::time::Instant::now(),
            timeout: download_whole_timeout(expected_bytes).saturating_add(POST_DOWNLOAD_BUDGET),
        }
    }

    pub(crate) fn for_check() -> Self {
        Self {
            started: std::time::Instant::now(),
            timeout: CHECK_OPERATION_TIMEOUT,
        }
    }

    pub(crate) fn check(&self, cancelled: &AtomicBool) -> Result<(), String> {
        check_cancelled(cancelled)?;
        if self.started.elapsed() >= self.timeout {
            Err(format!(
                "dependency operation timed out after {} seconds",
                self.timeout.as_secs()
            ))
        } else {
            Ok(())
        }
    }

    fn remaining(&self, cancelled: &AtomicBool) -> Result<Duration, String> {
        self.check(cancelled)?;
        Ok(self.timeout.saturating_sub(self.started.elapsed()))
    }
}

pub(crate) struct Resolved {
    pub version: String,
    pub download_url: String,
    pub sums_url: String,
    pub sums_asset: String,
}

fn network_runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        // Reqwest's connector uses tokio::net; time alone leaves no reactor and
        // panics on the first real request.
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}

fn block_on_network<T>(
    future: impl std::future::Future<Output = Result<T, String>>,
) -> Result<T, String> {
    let runtime = network_runtime()?;
    let result = runtime.block_on(future);
    // Dropping a runtime can wait indefinitely for a blocking DNS worker.
    // A cancelled/expired user operation must return promptly even if the OS
    // resolver has not; unfinished blocking work is allowed to finish detached.
    runtime.shutdown_timeout(Duration::from_millis(100));
    result
}

fn metadata_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .read_timeout(Duration::from_secs(30))
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| e.to_string())
}

fn download_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(60))
        // Per-read idleness catches a stalled peer; download_whole_timeout is
        // the separate finite wall-clock bound for a slow but active peer.
        .read_timeout(DOWNLOAD_IDLE_TIMEOUT)
        .https_only(true)
        .redirect(reqwest::redirect::Policy::limited(10))
        .build()
        .map_err(|e| e.to_string())
}

async fn wait_for_cancel(cancelled: &AtomicBool) {
    while !cancelled.load(Ordering::Relaxed) {
        tokio::time::sleep(DOWNLOAD_CANCEL_POLL).await;
    }
}

async fn cancellable_with_timeout<T>(
    future: impl std::future::Future<Output = Result<T, String>>,
    cancelled: &AtomicBool,
    timeout: Duration,
    timeout_label: &str,
) -> Result<T, String> {
    check_cancelled(cancelled)?;
    tokio::select! {
        biased;
        _ = wait_for_cancel(cancelled) => Err(CANCELLED_ERROR.to_string()),
        result = future => result,
        _ = tokio::time::sleep(timeout) => Err(format!("{timeout_label} timed out after {} seconds", timeout.as_secs())),
    }
}

fn download_whole_timeout(expected_bytes: Option<u64>) -> Duration {
    let Some(bytes) = expected_bytes else {
        return UNPINNED_DOWNLOAD_TIMEOUT;
    };
    // Permit a sustained 32 KiB/s plus ten minutes for connection, TLS and
    // final disk flush. The 1.6 GB Whisper pin therefore gets about fourteen
    // hours, while every operation still has a real finite end.
    let transfer_seconds = bytes.div_ceil(MIN_TOLERATED_BYTES_PER_SEC);
    Duration::from_secs(transfer_seconds.saturating_add(10 * 60))
        .clamp(MIN_DOWNLOAD_TIMEOUT, MAX_DOWNLOAD_TIMEOUT)
}

fn download_ceiling(expected_bytes: Option<u64>) -> u64 {
    expected_bytes.unwrap_or(UNPINNED_MAX_DOWNLOAD_BYTES)
}

fn ensure_download_within_ceiling(
    received: u64,
    expected_bytes: Option<u64>,
) -> Result<(), String> {
    let ceiling = download_ceiling(expected_bytes);
    if received > ceiling {
        Err(format!("download exceeded the {ceiling}-byte safety limit"))
    } else {
        Ok(())
    }
}

fn ensure_ffmpeg_extract_within_ceiling(extracted: u64) -> Result<(), String> {
    if extracted > FFMPEG_MAX_EXTRACTED_BYTES {
        Err(format!(
            "ffmpeg executable exceeded the {FFMPEG_MAX_EXTRACTED_BYTES}-byte extraction limit"
        ))
    } else {
        Ok(())
    }
}

fn assert_https(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|_| format!("invalid download url: {url}"))?;
    if parsed.scheme() == "https" {
        Ok(())
    } else {
        Err(format!("refusing non-https download url: {url}"))
    }
}

#[cfg(feature = "ai-test-support")]
fn require_online_acquisition() -> Result<(), String> {
    offline_acquisition_guard(std::env::var_os("ONECOPY_AI_OFFLINE").as_deref())
}

#[cfg(feature = "ai-test-support")]
fn offline_acquisition_guard(value: Option<&std::ffi::OsStr>) -> Result<(), String> {
    if value == Some(std::ffi::OsStr::new("1")) {
        Err("managed dependency network access is disabled in offline AI execution".to_string())
    } else {
        Ok(())
    }
}

#[cfg(not(feature = "ai-test-support"))]
fn require_online_acquisition() -> Result<(), String> {
    Ok(())
}

/// Resolves the latest ffmpeg build for this platform. Errors on platforms
/// with no managed build (Linux, Intel macs) — manual install applies there.
pub(crate) fn resolve_latest(
    cancelled: &AtomicBool,
    deadline: &OperationDeadline,
) -> Result<Resolved, String> {
    deadline.check(cancelled)?;
    if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
        // The GET follows the 307; the final URI carries <epoch>_<version>.
        // The body is dropped unread — resolution needs only the redirect.
        let (final_url, _) = fetch_metadata(MARTIN_REDIRECT_URL, false, cancelled, deadline)?;
        let final_uri = final_url.to_string();
        let version = binaries::parse_martin_build_version(final_url.path())
            .map(|v| binaries::normalize_version(&v))
            .ok_or_else(|| format!("no <epoch>_<version> segment in {final_uri}"))?;
        Ok(Resolved {
            version,
            sums_url: format!("{final_uri}.sha256"),
            sums_asset: "ffmpeg.zip".to_string(),
            download_url: final_uri,
        })
    } else if cfg!(all(target_os = "windows", target_arch = "x86_64")) {
        let (_, body) = fetch_metadata(BTBN_LATEST_API, true, cancelled, deadline)?;
        let body = String::from_utf8(body).map_err(|e| e.to_string())?;
        let release: serde_json::Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        // The release NAME, not the tag: BtbN's tag is the constant `latest`, a
        // rolling pointer that would compare equal to itself forever and never
        // offer an update. The name carries the build moment and does change.
        let version = release
            .get("name")
            .and_then(|name| name.as_str())
            .filter(|name| !name.trim().is_empty())
            .map(binaries::normalize_version)
            .ok_or_else(|| "release has no name".to_string())?;
        let asset_url = |name: &str| -> Option<String> {
            release.get("assets")?.as_array()?.iter().find_map(|a| {
                (a.get("name")?.as_str()? == name)
                    .then(|| a.get("browser_download_url")?.as_str().map(String::from))?
            })
        };
        let resolved = Resolved {
            version,
            download_url: asset_url(binaries::BTBN_WIN64_ASSET)
                .ok_or_else(|| format!("release has no {}", binaries::BTBN_WIN64_ASSET))?,
            sums_url: asset_url("checksums.sha256")
                .ok_or_else(|| "release has no checksums.sha256".to_string())?,
            sums_asset: binaries::BTBN_WIN64_ASSET.to_string(),
        };
        assert_https(&resolved.download_url)?;
        assert_https(&resolved.sums_url)?;
        Ok(resolved)
    } else {
        Err("no managed ffmpeg build for this platform; install ffmpeg manually".to_string())
    }
}

// not recorded: download staging — binary bytes into temp/, verified then
// published by rename; outside the managed-text backup path by design.
pub(crate) fn download_to(
    url: &str,
    dest: &Path,
    cancelled: &AtomicBool,
    deadline: &OperationDeadline,
    expected_bytes: Option<u64>,
    mut on_progress: impl FnMut(u64, Option<u64>),
) -> Result<u64, String> {
    require_online_acquisition()?;
    assert_https(url)?;
    check_cancelled(cancelled)?;
    let ceiling = download_ceiling(expected_bytes);
    let timeout = download_whole_timeout(expected_bytes).min(deadline.remaining(cancelled)?);
    block_on_network(cancellable_with_timeout(
        async {
            let mut response = download_client()?
                .get(url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|e| e.to_string())?;
            assert_https(response.url().as_str())?;
            let announced_bytes = expected_bytes.or(response.content_length());
            if let Some(length) = response.content_length() {
                if length > ceiling {
                    return Err(format!(
                        "download is {length} bytes, above the {ceiling}-byte safety limit"
                    ));
                }
                if let Some(expected) = expected_bytes {
                    if length != expected {
                        return Err(format!(
                            "download length mismatch: expected {expected} bytes, server announced {length}"
                        ));
                    }
                }
            }
            on_progress(0, announced_bytes);
            // Tokio's filesystem adapter keeps this current-thread runtime free
            // to poll the cancellation and whole-deadline branches while the OS
            // performs writes and the durable flush on its blocking pool.
            let mut file = tokio::fs::File::create(dest)
                .await
                .map_err(|e| e.to_string())?;
            let mut total = 0u64;
            let mut reported = 0u64;
            loop {
                check_cancelled(cancelled)?;
                let Some(bytes) = response.chunk().await.map_err(|e| e.to_string())? else {
                    break;
                };
                total += bytes.len() as u64;
                ensure_download_within_ceiling(total, expected_bytes)?;
                file.write_all(&bytes).await.map_err(|e| e.to_string())?;
                if total.saturating_sub(reported) >= 1024 * 1024 {
                    on_progress(total, announced_bytes);
                    reported = total;
                }
            }
            if let Some(expected) = expected_bytes {
                if total != expected {
                    return Err(format!(
                        "download length mismatch: expected {expected} bytes, received {total}"
                    ));
                }
            }
            if expected_bytes.is_none()
                && announced_bytes.is_some_and(|announced| announced != total)
            {
                return Err(format!(
                    "download length mismatch: server announced {} bytes, received {total}",
                    announced_bytes.unwrap_or_default()
                ));
            }
            if total != reported || announced_bytes.is_none() {
                // A server may omit Content-Length. The completed snapshot can
                // still close the phase with the actual stable byte total.
                on_progress(total, Some(announced_bytes.unwrap_or(total)));
            }
            file.sync_all().await.map_err(|e| e.to_string())?;
            Ok(total)
        },
        cancelled,
        timeout,
        "dependency download",
    ))
}

fn fetch_metadata(
    url: &str,
    read_body: bool,
    cancelled: &AtomicBool,
    deadline: &OperationDeadline,
) -> Result<(reqwest::Url, Vec<u8>), String> {
    require_online_acquisition()?;
    assert_https(url)?;
    check_cancelled(cancelled)?;
    block_on_network(cancellable_with_timeout(
        async {
            let mut response = metadata_client()?
                .get(url)
                .header("User-Agent", "onecopy")
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .map_err(|e| e.to_string())?;
            let final_url = response.url().clone();
            assert_https(final_url.as_str())?;
            if !read_body {
                return Ok((final_url, Vec::new()));
            }
            if response
                .content_length()
                .is_some_and(|length| length > METADATA_MAX_BYTES)
            {
                return Err(format!(
                    "metadata response exceeds {METADATA_MAX_BYTES} bytes"
                ));
            }
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|e| e.to_string())? {
                if body.len().saturating_add(chunk.len()) > METADATA_MAX_BYTES as usize {
                    return Err(format!(
                        "metadata response exceeds {METADATA_MAX_BYTES} bytes"
                    ));
                }
                body.extend_from_slice(&chunk);
            }
            Ok((final_url, body))
        },
        cancelled,
        METADATA_TIMEOUT.min(deadline.remaining(cancelled)?),
        "dependency metadata request",
    ))
}

pub(crate) fn fetch_text(
    url: &str,
    cancelled: &AtomicBool,
    deadline: &OperationDeadline,
) -> Result<String, String> {
    let (_, body) = fetch_metadata(url, true, cancelled, deadline)?;
    String::from_utf8(body).map_err(|e| e.to_string())
}

pub(crate) struct RemoveFilesOnDrop(Vec<PathBuf>);

impl RemoveFilesOnDrop {
    pub(crate) fn new(paths: Vec<PathBuf>) -> Self {
        Self(paths)
    }
}

impl Drop for RemoveFilesOnDrop {
    fn drop(&mut self) {
        for path in &self.0 {
            if let Err(error) = std::fs::remove_file(path) {
                if error.kind() != std::io::ErrorKind::NotFound {
                    crate::logging::warn(
                        "dependency staging cleanup failed",
                        serde_json::json!({
                            "file": path.to_string_lossy(),
                            "error": { "message": error.to_string() }
                        }),
                    );
                }
            }
        }
    }
}

pub(crate) fn file_sha256(
    path: &Path,
    cancelled: &AtomicBool,
    deadline: &OperationDeadline,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let total = file.metadata().map_err(|e| e.to_string())?.len();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    let mut done = 0u64;
    on_progress(done, total);
    loop {
        deadline.check(cancelled)?;
        let n = file.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        done += n as u64;
        on_progress(done, total);
    }
    Ok(hex::encode(hasher.finalize()))
}

/// Pulls the ffmpeg entry out of the archive: an exact-basename match, unique
/// across the archive (ambiguity is an error — a multi-match archive means the
/// upstream layout changed, and guessing which binary to install is not safe).
pub(crate) fn extract_ffmpeg(
    archive_path: &Path,
    staged: &Path,
    wanted: &str,
    cancelled: &AtomicBool,
    deadline: &OperationDeadline,
) -> Result<(), String> {
    let file = std::fs::File::open(archive_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let matches: Vec<String> = archive
        .file_names()
        .filter(|name| name.rsplit(['/', '\\']).next() == Some(wanted) && !name.ends_with('/'))
        .map(String::from)
        .collect();
    let inner = match matches.as_slice() {
        [one] => one.clone(),
        [] => return Err(format!("archive holds no {wanted}")),
        many => {
            return Err(format!(
                "archive holds {} candidates for {wanted}",
                many.len()
            ))
        }
    };
    let mut entry = archive.by_name(&inner).map_err(|e| e.to_string())?;
    ensure_ffmpeg_extract_within_ceiling(entry.size())?;
    // not recorded: staged binary extraction (temp/), published by rename below.
    let mut out = std::fs::File::create(staged).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut extracted = 0u64;
    loop {
        deadline.check(cancelled)?;
        let n = entry.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        extracted = extracted
            .checked_add(n as u64)
            .ok_or_else(|| "ffmpeg executable size overflow".to_string())?;
        ensure_ffmpeg_extract_within_ceiling(extracted)?;
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
    out.sync_all().map_err(|e| e.to_string())?;
    Ok(())
}

/// Extracts one explicitly pinned file from a ZIP-compatible package. The
/// entry name and uncompressed size are exact, and output goes to one staging
/// file rather than honoring any archive path.
pub(crate) fn extract_pinned_zip_entry(
    archive_path: &Path,
    staged: &Path,
    wanted: &str,
    expected_bytes: u64,
    cancelled: &AtomicBool,
    deadline: &OperationDeadline,
) -> Result<(), String> {
    let file = std::fs::File::open(archive_path).map_err(|e| e.to_string())?;
    let mut archive = zip::ZipArchive::new(file).map_err(|e| e.to_string())?;
    let mut entry = archive
        .by_name(wanted)
        .map_err(|_| format!("archive holds no exact entry {wanted}"))?;
    if entry.is_dir() || entry.size() != expected_bytes {
        return Err(format!(
            "archive entry {wanted} has size {}, expected {expected_bytes}",
            entry.size()
        ));
    }
    let mut out = std::fs::File::create(staged).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut extracted = 0u64;
    loop {
        deadline.check(cancelled)?;
        let n = entry.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        extracted = extracted
            .checked_add(n as u64)
            .ok_or_else(|| format!("archive entry {wanted} size overflow"))?;
        if extracted > expected_bytes {
            return Err(format!("archive entry {wanted} exceeds its pinned size"));
        }
        out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
    }
    if extracted != expected_bytes {
        return Err(format!(
            "archive entry {wanted} extracted {extracted} bytes, expected {expected_bytes}"
        ));
    }
    out.sync_all().map_err(|e| e.to_string())?;
    deadline.check(cancelled)
}

/// Publishes a verified same-volume staged artifact, replacing an older one.
/// `std::fs::rename` already has replace semantics on Unix; Windows requires
/// MoveFileExW for the same atomic update behavior.
#[cfg(not(windows))]
pub(crate) fn publish_staged(staged: &Path, target: &Path) -> Result<(), String> {
    std::fs::rename(staged, target).map_err(|e| e.to_string())?;
    // The staged contents were synced before publication. Persist the rename's
    // directory entry too where the platform permits opening a directory; this
    // mirrors storage::write_atomic and is deliberately best-effort.
    if let Some(parent) = target.parent() {
        match std::fs::File::open(parent) {
            Ok(directory) => crate::fs_recovery::sync_all(
                &directory,
                parent,
                "managed dependency publication sync",
            ),
            Err(error) => crate::logging::warn(
                "managed dependency directory could not be opened for sync",
                serde_json::json!({
                    "path": parent,
                    "error": { "message": error.to_string() },
                }),
            ),
        }
    }
    Ok(())
}

#[cfg(windows)]
pub(crate) fn publish_staged(staged: &Path, target: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let staged = crate::winpath::for_fs(staged);
    let target = crate::winpath::for_fs(target);
    let from: Vec<u16> = staged.as_os_str().encode_wide().chain(Some(0)).collect();
    let to: Vec<u16> = target.as_os_str().encode_wide().chain(Some(0)).collect();
    let moved = unsafe {
        MoveFileExW(
            from.as_ptr(),
            to.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if moved == 0 {
        Err(std::io::Error::last_os_error().to_string())
    } else {
        Ok(())
    }
}

/// Everything that must be true of the binary before publication: native
/// architecture (macOS), executable bit, and quarantine removal (best-effort).
#[cfg(unix)]
pub(crate) fn make_runnable(
    staged: &Path,
    cancelled: &AtomicBool,
    deadline: &OperationDeadline,
) -> Result<(), String> {
    deadline.check(cancelled)?;
    #[cfg(target_os = "macos")]
    {
        let header = {
            let mut file = std::fs::File::open(staged).map_err(|e| e.to_string())?;
            let mut buf = [0u8; 4096];
            let n = file.read(&mut buf).map_err(|e| e.to_string())?;
            buf[..n].to_vec()
        };
        if !macho_has_arm64(&header) {
            return Err("downloaded binary carries no native arm64 slice".to_string());
        }
    }
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(staged, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    #[cfg(target_os = "macos")]
    {
        let mut command = std::process::Command::new("xattr");
        command.args(["-d", "com.apple.quarantine"]).arg(staged);
        if let Err(error) = crate::subprocess::run_bounded_idle(
            command,
            &|| deadline.check(cancelled).is_err(),
            Duration::from_secs(10),
        ) {
            crate::logging::warn(
                "managed dependency quarantine cleanup failed",
                serde_json::json!({
                    "path": staged,
                    "error": { "message": error },
                }),
            );
        }
    }
    deadline.check(cancelled)
}

#[cfg(not(unix))]
pub(crate) fn make_runnable(
    _staged: &Path,
    cancelled: &AtomicBool,
    deadline: &OperationDeadline,
) -> Result<(), String> {
    deadline.check(cancelled)
}

#[cfg(target_os = "macos")]
fn macho_has_arm64(header: &[u8]) -> bool {
    const CPU_ARM64: u32 = 0x0100_000C;
    if header.len() < 8 {
        return false;
    }
    let magic_be = u32::from_be_bytes([header[0], header[1], header[2], header[3]]);
    if u32::from_le_bytes([header[0], header[1], header[2], header[3]]) == 0xFEED_FACF {
        return u32::from_le_bytes([header[4], header[5], header[6], header[7]]) == CPU_ARM64;
    }
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

pub(crate) fn check_cancelled(cancelled: &AtomicBool) -> Result<(), String> {
    if cancelled.load(Ordering::Relaxed) {
        Err(CANCELLED_ERROR.to_string())
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[test]
    fn managed_networks_refuse_plain_http() {
        assert!(assert_https("https://example.test/artifact").is_ok());
        assert!(assert_https("http://example.test/artifact")
            .unwrap_err()
            .contains("refusing non-https"));
    }

    #[cfg(feature = "ai-test-support")]
    // This proof stays beside the private acquisition edge so it can exercise
    // the exact guard used by both metadata and artifact network requests.
    #[test]
    fn offline_ai_execution_blocks_the_network_edge() {
        let previous = std::env::var_os("ONECOPY_AI_OFFLINE");
        std::env::set_var("ONECOPY_AI_OFFLINE", "1");
        let blocked = require_online_acquisition();
        if let Some(value) = previous {
            std::env::set_var("ONECOPY_AI_OFFLINE", value);
        } else {
            std::env::remove_var("ONECOPY_AI_OFFLINE");
        }

        let error = blocked.unwrap_err();
        assert!(error.contains("network access is disabled"), "{error}");
    }

    #[test]
    fn network_runtime_has_io_and_async_file_drivers() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-acquisition-runtime-")
            .tempdir()
            .unwrap();
        let durable = dir.path().join("durable.partial");
        network_runtime().unwrap().block_on(async {
            let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
                .await
                .unwrap();
            let address = listener.local_addr().unwrap();
            let (client, server) =
                tokio::join!(tokio::net::TcpStream::connect(address), listener.accept(),);
            assert!(client.is_ok());
            assert!(server.is_ok());

            let mut file = tokio::fs::File::create(&durable).await.unwrap();
            file.write_all(b"durable bytes").await.unwrap();
            file.sync_all().await.unwrap();
        });
        assert_eq!(std::fs::read(durable).unwrap(), b"durable bytes");
    }

    #[test]
    fn network_waits_are_promptly_cancellable_and_whole_bounded() {
        let runtime = network_runtime().unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let trigger = cancelled.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(25));
            trigger.store(true, Ordering::Relaxed);
        });
        let started = std::time::Instant::now();
        let error = runtime
            .block_on(cancellable_with_timeout(
                std::future::pending::<Result<(), String>>(),
                &cancelled,
                Duration::from_secs(5),
                "test request",
            ))
            .unwrap_err();
        assert_eq!(error, CANCELLED_ERROR);
        assert!(started.elapsed() < Duration::from_secs(1));

        let not_cancelled = AtomicBool::new(false);
        let error = runtime
            .block_on(cancellable_with_timeout(
                std::future::pending::<Result<(), String>>(),
                &not_cancelled,
                Duration::from_millis(25),
                "test request",
            ))
            .unwrap_err();
        assert!(error.contains("timed out"));
    }

    #[test]
    fn pinned_downloads_have_scaled_deadlines_and_exact_byte_ceilings() {
        let bytes = 1_624_555_275;
        let timeout = download_whole_timeout(Some(bytes));
        assert!(timeout > Duration::from_secs(12 * 60 * 60));
        assert!(timeout <= MAX_DOWNLOAD_TIMEOUT);
        assert!(ensure_download_within_ceiling(bytes, Some(bytes)).is_ok());
        assert!(ensure_download_within_ceiling(bytes + 1, Some(bytes)).is_err());
        assert!(ensure_download_within_ceiling(UNPINNED_MAX_DOWNLOAD_BYTES + 1, None).is_err());

        let operation = OperationDeadline::for_install(Some(bytes));
        assert!(operation.timeout > timeout);
    }

    #[test]
    fn ffmpeg_extraction_has_an_exact_uncompressed_byte_ceiling() {
        assert!(ensure_ffmpeg_extract_within_ceiling(FFMPEG_MAX_EXTRACTED_BYTES).is_ok());
        assert!(ensure_ffmpeg_extract_within_ceiling(FFMPEG_MAX_EXTRACTED_BYTES + 1).is_err());
    }

    #[test]
    fn staged_cleanup_runs_during_unwinding() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-acquisition-unwind-")
            .tempdir()
            .unwrap();
        let staged = dir.path().join("artifact.partial");
        std::fs::write(&staged, b"partial").unwrap();
        let _ = std::panic::catch_unwind({
            let staged = staged.clone();
            move || {
                let _cleanup = RemoveFilesOnDrop::new(vec![staged]);
                panic!("simulated worker panic");
            }
        });
        assert!(!staged.exists());
    }

    #[test]
    fn hashing_and_extraction_stop_when_cancelled() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-acquisition-cancel-")
            .tempdir()
            .unwrap();
        let file = dir.path().join("artifact.bin");
        std::fs::write(&file, vec![7u8; 2 * 1024 * 1024]).unwrap();
        let cancelled = AtomicBool::new(true);
        let deadline = OperationDeadline::for_install(Some(2 * 1024 * 1024));
        assert_eq!(
            file_sha256(&file, &cancelled, &deadline, |_, _| {}).unwrap_err(),
            CANCELLED_ERROR
        );

        let archive_path = dir.path().join("ffmpeg.zip");
        let archive_file = std::fs::File::create(&archive_path).unwrap();
        let mut archive = zip::ZipWriter::new(archive_file);
        archive
            .start_file("ffmpeg.exe", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(b"binary bytes").unwrap();
        archive.finish().unwrap();

        let staged = dir.path().join("ffmpeg.staged");
        assert_eq!(
            extract_ffmpeg(&archive_path, &staged, "ffmpeg.exe", &cancelled, &deadline,)
                .unwrap_err(),
            CANCELLED_ERROR
        );
    }

    #[test]
    fn hashing_reports_stable_byte_progress_through_completion() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-acquisition-hash-progress-")
            .tempdir()
            .unwrap();
        let file = dir.path().join("artifact.bin");
        let bytes = 2 * 1024 * 1024 + 17;
        std::fs::write(&file, vec![7u8; bytes]).unwrap();
        let cancelled = AtomicBool::new(false);
        let deadline = OperationDeadline::for_install(Some(bytes as u64));
        let mut snapshots = Vec::new();

        file_sha256(&file, &cancelled, &deadline, |done, total| {
            snapshots.push((done, total));
        })
        .unwrap();

        assert_eq!(snapshots.first(), Some(&(0, bytes as u64)));
        assert_eq!(snapshots.last(), Some(&(bytes as u64, bytes as u64)));
        assert!(snapshots.windows(2).all(|pair| pair[0].0 <= pair[1].0));
    }

    #[test]
    fn cumulative_deadline_expires_local_work_too() {
        let deadline = OperationDeadline {
            started: std::time::Instant::now() - Duration::from_secs(2),
            timeout: Duration::from_secs(1),
        };
        let cancelled = AtomicBool::new(false);
        assert!(deadline
            .check(&cancelled)
            .unwrap_err()
            .contains("operation timed out"));
    }

    #[test]
    fn publishing_replaces_an_existing_artifact() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-acquisition-publish-")
            .tempdir()
            .unwrap();
        let staged = dir.path().join("artifact.staged");
        let target = dir.path().join("artifact.bin");
        std::fs::write(&target, b"old").unwrap();
        std::fs::write(&staged, b"new").unwrap();

        publish_staged(&staged, &target).unwrap();

        assert_eq!(std::fs::read(&target).unwrap(), b"new");
        assert!(!staged.exists());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macho_arm64_detection_covers_thin_fat_and_foreign() {
        let mut thin_arm = vec![0xCF, 0xFA, 0xED, 0xFE];
        thin_arm.extend_from_slice(&0x0100_000Cu32.to_le_bytes());
        assert!(macho_has_arm64(&thin_arm));

        let mut thin_x86 = vec![0xCF, 0xFA, 0xED, 0xFE];
        thin_x86.extend_from_slice(&0x0100_0007u32.to_le_bytes());
        assert!(!macho_has_arm64(&thin_x86));

        let mut fat = Vec::new();
        fat.extend_from_slice(&0xCAFE_BABEu32.to_be_bytes());
        fat.extend_from_slice(&2u32.to_be_bytes());
        fat.extend_from_slice(&0x0100_0007u32.to_be_bytes());
        fat.extend_from_slice(&[0u8; 16]);
        fat.extend_from_slice(&0x0100_000Cu32.to_be_bytes());
        fat.extend_from_slice(&[0u8; 16]);
        assert!(macho_has_arm64(&fat));

        let mut fat_x86 = Vec::new();
        fat_x86.extend_from_slice(&0xCAFE_BABEu32.to_be_bytes());
        fat_x86.extend_from_slice(&1u32.to_be_bytes());
        fat_x86.extend_from_slice(&0x0100_0007u32.to_be_bytes());
        fat_x86.extend_from_slice(&[0u8; 16]);
        assert!(!macho_has_arm64(&fat_x86));
        assert!(!macho_has_arm64(b"#!/bin/sh\n"));
        assert!(!macho_has_arm64(&[]));
    }
}
