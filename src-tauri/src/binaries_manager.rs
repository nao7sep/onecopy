//! Orchestration half of the managed-dependencies mechanism (the pure half
//! lives in `binaries.rs`): a REGISTRY of N entries of two kinds — binaries
//! (resolved live per platform) and model files (pinned artifacts) — each
//! downloading to `temp/` staging, verifying, then publishing with a
//! same-volume rename. One install/check at a time across the registry; a
//! failed check writes nothing (the honest-state rule).
//!
//! Model entries pin their canonical artifact IN CODE: URL, sha256 (taken
//! from the upstream's own LFS metadata), and byte size. A model's "version"
//! is the pin's short digest, so an app update that re-pins shows
//! update-available exactly like a new ffmpeg build does — and a model check
//! needs no network at all, because the app itself is the source of "latest".
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

use sha2::{Digest, Sha256};
use ureq::ResponseExt;

use crate::binaries::{self, BinaryFacts, BinaryStatus};
use crate::{logging, nanoid, storage};

// Subpath names are owned by the one resolver module (storage-path
// conventions); re-exported here so existing call sites keep their imports.
pub use crate::paths::{BIN_DIR_NAME, DEPENDENCIES_FILE_NAME, MODELS_DIR_NAME, TEMP_DIR_NAME};

const MARTIN_REDIRECT_URL: &str =
    "https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffmpeg.zip";
const BTBN_LATEST_API: &str = "https://api.github.com/repos/BtbN/FFmpeg-Builds/releases/latest";

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyKind {
    Binary,
    Model,
}

/// One managed dependency. `pinned` is Some for model entries — the canonical
/// artifact the app provisions; None for binaries, which resolve live.
pub struct DependencySpec {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: DependencyKind,
    pub file_name: &'static str,
    pub pinned: Option<PinnedModel>,
}

pub struct PinnedModel {
    pub url: &'static str,
    /// From the upstream repository's own LFS metadata, not our download.
    pub sha256: &'static str,
    pub bytes: u64,
    /// When this artifact was PUBLISHED upstream (probed from the upstream's
    /// own commit history 2026-08-17) — the only honest answer to "how old is
    /// this model?", since a content hash carries no date. Note for the ONNX
    /// zoo entries: the file's current path was created by a repo-wide
    /// restructure in Dec 2023, so these dates come from the pre-restructure
    /// path, where the artifact actually first appeared.
    pub released: &'static str,
}

/// The registry. Order is display order in Managed tools. Adding an entry
/// here is the WHOLE registration — facts, status, download, and the UI row
/// all derive from it.
pub const DEPENDENCIES: &[DependencySpec] = &[
    DependencySpec {
        id: "ffmpeg",
        label: "ffmpeg",
        kind: DependencyKind::Binary,
        file_name: "", // platform-resolved by ffmpeg_file_name()
        pinned: None,
    },
    DependencySpec {
        id: "clip-vit-b32",
        label: "Similarity model (CLIP ViT-B/32)",
        kind: DependencyKind::Model,
        file_name: "clip-vit-b32-vision.onnx",
        pinned: Some(PinnedModel {
            // Qdrant's MIT-licensed ONNX export of openai/clip-vit-base-patch32;
            // sha256 from the repository's LFS pointer (probed 2026-08-16).
            url: "https://huggingface.co/Qdrant/clip-ViT-B-32-vision/resolve/main/model.onnx",
            sha256: "c68d3d9a200ddd2a8c8a5510b576d4c94d1ae383bf8b36dd8c084f94e1fb4d63",
            bytes: 351_686_194,
            released: "2024-04-30",
        }),
    },
    DependencySpec {
        id: "whisper-large-v3-turbo",
        label: "Transcription model (Whisper large-v3-turbo)",
        kind: DependencyKind::Model,
        file_name: "ggml-large-v3-turbo.bin",
        pinned: Some(PinnedModel {
            // Canonical whisper.cpp model repository; sha256 from its LFS
            // pointer (probed live 2026-08-16).
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin",
            sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
            bytes: 1_624_555_275,
            released: "2024-10-01",
        }),
    },
    DependencySpec {
        id: "ultraface-rfb320",
        label: "Face detector (Ultraface RFB-320)",
        kind: DependencyKind::Model,
        file_name: "ultraface-rfb320.onnx",
        pinned: Some(PinnedModel {
            // Official ONNX model zoo (repo Apache-2.0; the Ultraface upstream
            // is MIT); sha256 computed from the downloaded artifact 2026-08-16.
            url: "https://github.com/onnx/models/raw/main/validated/vision/body_analysis/ultraface/models/version-RFB-320.onnx",
            sha256: "34cd7e60aeff28744c657de7a3dc64e872d506741de66987f3426f2b79f88017",
            bytes: 1_270_727,
            released: "2020-12-17",
        }),
    },
    DependencySpec {
        id: "emotion-ferplus",
        label: "Expression model (Emotion FER+)",
        kind: DependencyKind::Model,
        file_name: "emotion-ferplus-8.onnx",
        pinned: Some(PinnedModel {
            // Official ONNX model zoo, MIT per its model card; sha256 computed
            // from the downloaded artifact 2026-08-16. Face scoring needs BOTH
            // this and the detector — either alone stays inert.
            url: "https://github.com/onnx/models/raw/main/validated/vision/body_analysis/emotion_ferplus/model/emotion-ferplus-8.onnx",
            sha256: "a2a2ba6a335a3b29c21acb6272f962bd3d47f84952aaffa03b60986e04efa61c",
            bytes: 35_040_571,
            released: "2020-03-18",
        }),
    },
];

pub fn spec_of(id: &str) -> Option<&'static DependencySpec> {
    DEPENDENCIES.iter().find(|d| d.id == id)
}

/// A model pin's user-facing version: the digest's short prefix — changes
/// exactly when the app re-pins, which is the only way a model updates.
fn pin_version(pinned: &PinnedModel) -> String {
    pinned.sha256[..12].to_string()
}

/// The installed location for any entry.
pub fn installed_path(root: &Path, spec: &DependencySpec) -> PathBuf {
    match spec.kind {
        DependencyKind::Binary => root.join(BIN_DIR_NAME).join(ffmpeg_file_name()),
        DependencyKind::Model => root.join(MODELS_DIR_NAME).join(spec.file_name),
    }
}

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

fn load_facts_map(root: &Path) -> serde_json::Map<String, serde_json::Value> {
    let file = root.join(DEPENDENCIES_FILE_NAME);
    let Ok(bytes) = std::fs::read(&file) else {
        return serde_json::Map::new();
    };
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

/// One entry's facts out of the shared map — the historical `{"ffmpeg": …}`
/// shape generalized in place, so an existing file needs no migration.
pub fn load_facts_for(root: &Path, id: &str) -> BinaryFacts {
    load_facts_map(root)
        .get(id)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

/// Read-modify-write of ONE entry's facts; every other entry's are untouched.
/// Serialized by its OWN lock: installs run in parallel per-id (2026-08-17),
/// so the RMW can no longer lean on a global operation claim.
static FACTS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

pub fn save_facts_for(root: &Path, id: &str, facts: &BinaryFacts) -> Result<(), String> {
    let _lock = FACTS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // records: dependencies.json rides write_atomic's backup hook. The facts
    // are re-derivable, so recording is not REQUIRED — but tiny self-healing
    // text in the store is harmless, and a separate unrecorded write path
    // would cost more than it saves (accounted in storage.rs's table).
    let mut map = load_facts_map(root);
    map.insert(id.to_string(), serde_json::to_value(facts).map_err(|e| e.to_string())?);
    let mut text = serde_json::to_string_pretty(&serde_json::Value::Object(map))
        .map_err(|e| e.to_string())?;
    text.push('\n');
    storage::write_atomic(&root.join(DEPENDENCIES_FILE_NAME), text.as_bytes())
}

/// Back-compat readers used by the scan settings and launch check.
pub fn load_facts(root: &Path) -> BinaryFacts {
    load_facts_for(root, "ffmpeg")
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

/// In-flight operations by entry id. PER-ID, deliberately (developer,
/// 2026-08-17): several dependencies may download AT ONCE — only a second
/// operation on the SAME entry is refused. The facts-file RMW no longer
/// rides this claim; it has its own lock below.
static IN_FLIGHT: std::sync::LazyLock<std::sync::Mutex<std::collections::HashSet<String>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashSet::new()));

struct BusyGuard {
    id: String,
}
impl Drop for BusyGuard {
    fn drop(&mut self) {
        IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner()).remove(&self.id);
    }
}

fn claim(id: &str) -> Result<BusyGuard, String> {
    let mut in_flight = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
    if !in_flight.insert(id.to_string()) {
        return Err(format!("{id} is already being worked on"));
    }
    Ok(BusyGuard { id: id.to_string() })
}

/// The full install/update for any registry entry. Binaries: resolve →
/// download → verify → extract → make runnable → publish. Models: pinned
/// download → sha256 verify → publish. Both stage in `temp/` and land with a
/// same-volume rename; both record facts only on success.
pub fn install_entry(
    root: &Path,
    id: &str,
    mut on_progress: impl FnMut(&str, String),
) -> Result<BinaryFacts, String> {
    let spec = spec_of(id).ok_or_else(|| format!("unknown dependency: {id}"))?;
    match spec.kind {
        DependencyKind::Binary => install_or_update(root, on_progress),
        DependencyKind::Model => {
            let _guard = claim(id)?;
            let pinned = spec.pinned.as_ref().ok_or("model entry carries no pin")?;
            let temp = root.join(TEMP_DIR_NAME);
            std::fs::create_dir_all(&temp).map_err(|e| e.to_string())?;
            let partial = temp.join(format!("{id}-{}.partial", nanoid::generate()));
            let result = (|| -> Result<BinaryFacts, String> {
                on_progress(
                    "download",
                    format!("{} ({} MB)", spec.label, pinned.bytes / 1_048_576),
                );
                download_to(pinned.url, &partial, |done| {
                    on_progress(
                        "download",
                        format!("{} / {} MB", done / 1_048_576, pinned.bytes / 1_048_576),
                    );
                })?;
                on_progress("verify", "checking integrity".to_string());
                let actual = file_sha256(&partial)?;
                if actual != pinned.sha256 {
                    return Err(format!(
                        "checksum mismatch for {id}: expected {}, got {actual}",
                        pinned.sha256
                    ));
                }
                let target = installed_path(root, spec);
                std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
                // Replace-in-place over any previous model (same volume).
                // not recorded: the model file is a re-downloadable artifact.
                std::fs::rename(&partial, &target).map_err(|e| e.to_string())?;
                let version = pin_version(pinned);
                let facts = BinaryFacts {
                    installed_version: Some(version.clone()),
                    latest_known_version: Some(version),
                    last_checked_at_utc: Some(logging::now_iso_millis()),
                };
                save_facts_for(root, id, &facts)?;
                logging::info(
                    "model installed",
                    serde_json::json!({ "id": id, "path": target.to_string_lossy() }),
                );
                Ok(facts)
            })();
            let _ = std::fs::remove_file(&partial);
            result
        }
    }
}

/// The ffmpeg install/update (the registry's one binary entry).
pub fn install_or_update(
    root: &Path,
    mut on_progress: impl FnMut(&str, String),
) -> Result<BinaryFacts, String> {
    let _guard = claim("ffmpeg")?;
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
        save_facts_for(root, "ffmpeg", &facts)?;
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
/// BINARIES ONLY: a model's "latest" is the pin compiled into this build, so
/// there is nothing to ask and nothing to stamp — `state_of` derives it and
/// a re-pin shipped in an app update shows up on its own.
pub fn check_entry(root: &Path, id: &str) -> Result<BinaryFacts, String> {
    let spec = spec_of(id).ok_or_else(|| format!("unknown dependency: {id}"))?;
    let _guard = claim(id)?;
    let mut facts = load_facts_for(root, id);
    match spec.kind {
        DependencyKind::Binary => {
            let resolved = resolve_latest()?;
            facts.latest_known_version = Some(resolved.version);
        }
        // Refused rather than faked: a model has no upstream to ask. Its
        // version ships with the app, and `state_of` already derives it.
        DependencyKind::Model => {
            return Err(format!(
                "{id} ships with the app — there is no update to check for"
            ));
        }
    }
    facts.last_checked_at_utc = Some(logging::now_iso_millis());
    save_facts_for(root, id, &facts)?;
    Ok(facts)
}

/// Back-compat: the ffmpeg check.
pub fn check_for_updates(root: &Path) -> Result<BinaryFacts, String> {
    check_entry(root, "ffmpeg")
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyState {
    pub id: String,
    pub label: String,
    pub kind: DependencyKind,
    pub status: BinaryStatus,
    pub facts: BinaryFacts,
    pub path: String,
    /// True when this entry's "latest" is DISCOVERABLE — a binary resolved
    /// live from upstream. A model's latest is a constant compiled into the
    /// app, so there is nothing to look up and nothing to check.
    pub checkable: bool,
    /// A pinned artifact's upstream publication date — how old this model
    /// actually is. None for binaries, whose live version is the answer.
    pub released: Option<String>,
}

/// One entry's live state; presence re-scanned from disk, never persisted.
/// A model's presence check is size-exact — a truncated download that somehow
/// reached the models dir must read not-installed, not installed-broken.
pub fn state_of(root: &Path, spec: &DependencySpec) -> DependencyState {
    let path = installed_path(root, spec);
    let mut facts = load_facts_for(root, spec.id);
    // A model's "latest" is the pin in THIS app build — a constant, not a
    // lookup. Deriving it here means a re-pinned model surfaces as
    // update-available the moment the app updates, with no check to run and
    // no timestamp to record. (Recording a "checked at" for a model would
    // claim a lookup that never happened.)
    if let Some(pinned) = spec.pinned.as_ref() {
        facts.latest_known_version = Some(pin_version(pinned));
    }
    let present = match spec.kind {
        DependencyKind::Binary => binaries::is_usable_binary(&path),
        DependencyKind::Model => {
            let expected = spec.pinned.as_ref().map(|p| p.bytes);
            std::fs::metadata(&path)
                .map(|m| m.is_file() && Some(m.len()) == expected)
                .unwrap_or(false)
        }
    };
    DependencyState {
        id: spec.id.to_string(),
        label: spec.label.to_string(),
        kind: spec.kind,
        status: binaries::derive_status(present, &facts),
        path: path.to_string_lossy().to_string(),
        facts,
        checkable: matches!(spec.kind, DependencyKind::Binary),
        released: spec.pinned.as_ref().map(|p| p.released.to_string()),
    }
}

/// Every registry entry's state, in display order.
pub fn states(root: &Path) -> Vec<DependencyState> {
    DEPENDENCIES.iter().map(|spec| state_of(root, spec)).collect()
}

/// Back-compat: the ffmpeg state (the scan settings and chip read it).
pub fn state(root: &Path) -> DependencyState {
    state_of(root, spec_of("ffmpeg").expect("ffmpeg is registered"))
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
        save_facts_for(dir.path(), "ffmpeg", &facts).unwrap();
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
