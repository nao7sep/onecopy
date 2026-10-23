//! Registry and orchestration for managed dependencies (`binaries.rs` owns
//! pure status decisions; `binaries_acquisition.rs` owns acquisition): N
//! entries of three kinds — binaries
//! (resolved live per platform), native runtimes, and model files (the latter
//! two are pinned artifacts) — each
//! downloading to `temp/` staging, verifying, then publishing with a
//! same-volume rename. One install/check at a time per entry, with different
//! entries allowed in parallel; a failed check writes nothing.
//!
//! Model entries pin their canonical artifact IN CODE: URL, sha256 (taken
//! from the upstream's own LFS metadata), and byte size. A model's "version"
//! is the pin's short digest, so an app update that re-pins shows
//! update-available exactly like a new ffmpeg build does — and a model check
//! needs no network at all, because the app itself is the source of "latest".
//!
//! The INSTALLED version of every entry is read from the artifact, never from
//! the facts store (managed-runtime-dependencies-conventions): a fact about a
//! file kept away from that file drifts the moment an install does not write
//! the record, and a present artifact with no recorded version reads as
//! "installed (not checked)" forever — never offering the update that exists.
//! Three reads, one rule (whatever the artifact itself can be made to say):
//!   ffmpeg, macOS    run it and parse its banner. martin-riedl builds a
//!                    numbered upstream release and the binary names that same
//!                    release, so one namespace covers both sides.
//!   ffmpeg, Windows  read `bin/ffmpeg.json`, written beside the binary at
//!                    install. The binary reports no version comparable with
//!                    upstream, so the sidecar records the resolved
//!                    `autobuild-…` tag itself and a later check compares
//!                    tags directly.
//!   a model          read a verified-install identity beside the model. File
//!                    size establishes usable presence, while the identity
//!                    binds the verified digest to its published file version.
//!                    Missing version evidence stays installed-unchecked; an older
//!                    digest remains update-available even at the same size.
//!
//! Endpoint shapes verified live at build time:
//!   macOS arm64  https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffmpeg.zip
//!                → 307 to /download/macos/arm64/<epoch>_<version>/ffmpeg.zip,
//!                with a `<url>.sha256` sidecar (`<hex>  ffmpeg.zip`).
//!   Windows x64  The newest `autobuild-…` release of BtbN/FFmpeg-Builds,
//!                resolved fresh from the releases list on every install and
//!                check (never the rolling `latest`, whose fixed-named
//!                assets are silently replaced by a new build every day):
//!                its immutable win64 GPL asset plus that same release's
//!                `checksums.sha256`; the release TAG is the version.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::binaries::{self, BinaryFacts, BinaryStatus};
use crate::binaries_acquisition as acquisition;
use crate::{formats, logging, nanoid, storage, subprocess};

// Subpath names are owned by the one resolver module (storage-path
// conventions); re-exported here so existing call sites keep their imports.
pub use crate::paths::{BIN_DIR_NAME, DEPENDENCIES_FILE_NAME, MODELS_DIR_NAME, TEMP_DIR_NAME};

#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DependencyKind {
    Binary,
    Runtime,
    Model,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DependencyPlatform {
    Any,
    Windows,
}

/// One typed snapshot of an entry's acquisition. Fixed phases use one unit;
/// streamed phases use bytes and retain an unknown total only when the server
/// did not announce one. The frontend owns wording and history presentation.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum InstallPhase {
    Resolve,
    Download,
    Verify,
    Install,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallProgress {
    pub phase: InstallPhase,
    pub done: u64,
    pub total: Option<u64>,
    pub next_phase: Option<InstallPhase>,
}

impl InstallProgress {
    fn fixed(phase: InstallPhase, done: u64, next_phase: Option<InstallPhase>) -> Self {
        Self {
            phase,
            done,
            total: Some(1),
            next_phase,
        }
    }

    fn bytes(
        phase: InstallPhase,
        done: u64,
        total: Option<u64>,
        next_phase: InstallPhase,
    ) -> Self {
        Self {
            phase,
            done,
            total,
            next_phase: Some(next_phase),
        }
    }
}

/// One managed dependency. `pinned` is Some for opaque artifacts selected by
/// the app; None for binaries that resolve live.
pub struct DependencySpec {
    pub id: &'static str,
    pub label: &'static str,
    pub kind: DependencyKind,
    /// Missing this dependency prevents a built-in core presentation path,
    /// rather than only withholding optional enrichment.
    pub required_for_core: bool,
    pub file_name: &'static str,
    pub platform: DependencyPlatform,
    pub pinned: Option<PinnedArtifact>,
}

pub struct PinnedArtifact {
    pub url: &'static str,
    /// The upstream's own release number, for an upstream that numbers its
    /// releases; the interface names it beside the tool. None for an artifact
    /// pinned by commit, whose age `released` already says.
    pub upstream_version: Option<&'static str>,
    /// Digest and exact size of the downloaded object.
    pub sha256: &'static str,
    pub bytes: u64,
    /// Some upstreams publish the accepted file inside an archive. The archive
    /// and the one extracted file are independently pinned; no path or DLL
    /// search is trusted at runtime.
    pub extracted: Option<ExtractedArtifact>,
    /// When this artifact was PUBLISHED upstream (probed from the upstream's
    /// own commit history 2026-08-17) — the only honest answer to "how old is
    /// this model?", since a content hash carries no date. Note for the ONNX
    /// zoo entries: the file's current path was created by a repo-wide
    /// restructure in Dec 2023, so these dates come from the pre-restructure
    /// path, where the artifact actually first appeared.
    pub released: &'static str,
}

pub struct ExtractedArtifact {
    pub archive_entry: &'static str,
    pub sha256: &'static str,
    pub bytes: u64,
}

// The pinned ONNX Runtime release, written once: the download URL and the
// English label are built from it, and the interface reads it from
// `upstream_version`.
macro_rules! onnxruntime_version {
    () => {
        "1.28.0"
    };
}

/// The registry. Order is display order in Managed tools. Adding an entry
/// here is the WHOLE registration — facts, status, download, and the UI row
/// all derive from it.
pub const DEPENDENCIES: &[DependencySpec] = &[
    DependencySpec {
        id: "ffmpeg",
        label: "ffmpeg",
        kind: DependencyKind::Binary,
        required_for_core: true,
        file_name: "", // platform-resolved by ffmpeg_file_name()
        platform: DependencyPlatform::Any,
        pinned: None,
    },
    DependencySpec {
        id: "onnxruntime-win-x64",
        label: concat!("Face-scoring runtime (ONNX Runtime ", onnxruntime_version!(), ")"),
        kind: DependencyKind::Runtime,
        required_for_core: false,
        file_name: "onnxruntime.dll",
        platform: DependencyPlatform::Windows,
        pinned: Some(PinnedArtifact {
            // Official Microsoft NuGet package. Pin both the package and the
            // exact x64 CPU runtime extracted from it so System32 and PATH can
            // never choose a different ONNX Runtime build.
            url: concat!(
                "https://www.nuget.org/api/v2/package/Microsoft.ML.OnnxRuntime/",
                onnxruntime_version!()
            ),
            upstream_version: Some(onnxruntime_version!()),
            sha256: "769d1d3ea8ab6cd69f737c9dd4d4462aa4ad0ccfa106eaf506efc40d7bead5db",
            bytes: 139_145_017,
            extracted: Some(ExtractedArtifact {
                archive_entry: "runtimes/win-x64/native/onnxruntime.dll",
                sha256: "18370c375f07357fa5874344a9d9ac17e6b6fe1eb18b1dd209d79483b4470257",
                bytes: 15_809_848,
            }),
            released: "2026-07-25",
        }),
    },
    DependencySpec {
        id: "whisper-large-v3-turbo",
        label: "Transcription model (Whisper large-v3-turbo)",
        kind: DependencyKind::Model,
        required_for_core: false,
        file_name: "ggml-large-v3-turbo.bin",
        platform: DependencyPlatform::Any,
        pinned: Some(PinnedArtifact {
            // Canonical whisper.cpp model repository, fixed to the verified
            // commit that added this file; sha256 matches its Xet metadata.
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/98aa99a0a9db05ae2342309f5096248665f7cba3/ggml-large-v3-turbo.bin",
            upstream_version: None,
            sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
            bytes: 1_624_555_275,
            extracted: None,
            released: "2024-10-01",
        }),
    },
    DependencySpec {
        id: "ultraface-rfb640",
        label: "Face detector (UltraFace RFB-640)",
        kind: DependencyKind::Model,
        required_for_core: false,
        file_name: "ultraface-rfb640.onnx",
        platform: DependencyPlatform::Any,
        pinned: Some(PinnedArtifact {
            // Official ONNX model zoo (repo Apache-2.0; the Ultraface upstream
            // is MIT), fixed to the file's immutable commit; sha256 verified
            // against the bytes at that revision 2026-08-22.
            // The 640 variant over the 320: same family and licence, double the
            // input resolution, so a face that is small in the frame — the
            // common case in family photos — is still found.
            url: "https://media.githubusercontent.com/media/onnx/models/4c46cd00fbdb7cd30b6c1c17ab54f2e1f4f7b177/validated/vision/body_analysis/ultraface/models/version-RFB-640.onnx",
            upstream_version: None,
            sha256: "8f4c659275977e7a3bfbfa339a9c769ad793df50f9c0baa8c14b11baa1646430",
            bytes: 1_588_012,
            extracted: None,
            released: "2020-12-17",
        }),
    },
    DependencySpec {
        id: "hsemotion-enet-b2",
        label: "Expression model (HSEmotion EfficientNet-B2)",
        kind: DependencyKind::Model,
        required_for_core: false,
        file_name: "hsemotion-enet-b2-8.onnx",
        platform: DependencyPlatform::Any,
        pinned: Some(PinnedArtifact {
            // HSEmotion's AffectNet-trained EfficientNet-B2, from the project's
            // current home (sb-ai-lab/EmotiEffLib, Apache-2.0 — the old
            // av-savchenko repo redirects here); sha256 computed from the
            // immutable file commit and verified against its bytes 2026-08-22.
            // Replaces FER+, which was trained
            // on 2016-era FER data and is five years older. Face scoring needs
            // BOTH this and the detector — either alone stays inert.
            url: "https://raw.githubusercontent.com/sb-ai-lab/EmotiEffLib/af833487321c3efdcb1768a91a6c656a1986fdf6/models/affectnet_emotions/onnx/enet_b2_8.onnx",
            upstream_version: None,
            sha256: "180a9d4845b59393de4511598a0d1d34b705034691ea32959ce5009db7cf52b7",
            bytes: 30_779_724,
            extracted: None,
            released: "2022-11-09",
        }),
    },
];

pub fn spec_of(id: &str) -> Option<&'static DependencySpec> {
    DEPENDENCIES
        .iter()
        .find(|d| d.id == id && available_on_current_platform(d))
}

fn available_on_current_platform(spec: &DependencySpec) -> bool {
    match spec.platform {
        DependencyPlatform::Any => true,
        DependencyPlatform::Windows => cfg!(windows),
    }
}

/// A model pin's user-facing version: the digest's short prefix — changes
/// exactly when the app re-pins, which is the only way a model updates.
fn pin_version(pinned: &PinnedArtifact) -> String {
    pinned.sha256[..12].to_string()
}

/// The installed location for any entry.
pub fn installed_path(root: &Path, spec: &DependencySpec) -> PathBuf {
    match spec.kind {
        DependencyKind::Binary => root.join(BIN_DIR_NAME).join(ffmpeg_file_name()),
        DependencyKind::Runtime => root.join(BIN_DIR_NAME).join(spec.file_name),
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
    // not recorded: temp/ contains disposable dependency download staging.
    let temp = root.join(TEMP_DIR_NAME);
    if let Err(error) = std::fs::remove_dir_all(&temp) {
        if error.kind() != std::io::ErrorKind::NotFound {
            logging::warn(
                "dependency staging reset failed",
                serde_json::json!({
                    "directory": crate::native_path::encode(&temp),
                    "error": { "message": error.to_string() }
                }),
            );
        }
    }
    if let Err(error) = std::fs::create_dir_all(&temp) {
        logging::warn(
            "dependency staging recreation failed",
            serde_json::json!({
                "directory": crate::native_path::encode(&temp),
                "error": { "message": error.to_string() }
            }),
        );
    }
}

// --- The facts store: its own file, self-healing (missing OR corrupt →
// fresh defaults; the opposite of the config store's quarantine, because
// every field is re-derivable by a check). A file a newer OneCopy wrote
// reads as empty and is not saved over. ---

fn load_facts_map(root: &Path) -> serde_json::Map<String, serde_json::Value> {
    match storage::read_json_file(&root.join(DEPENDENCIES_FILE_NAME), formats::DEPENDENCIES) {
        Ok(storage::JsonFile::Document(serde_json::Value::Object(map))) => map,
        Ok(storage::JsonFile::Newer(newer)) => {
            storage::warn_newer(&newer);
            serde_json::Map::new()
        }
        _ => serde_json::Map::new(),
    }
}

/// One entry's facts out of the shared map, keyed by the dependency's id.
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
    save_entry(
        root,
        id,
        serde_json::to_value(facts).map_err(|e| e.to_string())?,
    )
}

/// The launch-check throttle timestamp rides in the same facts store, as a
/// top-level string beside the per-binary entries. It is a volatile network
/// fact like `lastCheckedAtUtc`: re-derivable by simply checking again.
pub const MANAGED_TOOL_UPDATE_ATTEMPT_KEY: &str = "managedToolUpdateLastAttemptAtUtc";

/// The recorded attempt timestamp under `key`, when it is a string.
pub fn load_check_attempt(root: &Path, key: &str) -> Option<String> {
    load_facts_map(root)
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
}

pub fn save_check_attempt(root: &Path, key: &str, at_utc: &str) -> Result<(), String> {
    save_entry(root, key, serde_json::Value::String(at_utc.to_string()))
}

fn save_entry(root: &Path, key: &str, value: serde_json::Value) -> Result<(), String> {
    let _lock = FACTS_LOCK.lock().unwrap_or_else(|p| p.into_inner());
    // not recorded: dependencies.json contains re-derivable dependency and
    // update facts, not user-authored data.
    let mut map = load_facts_map(root);
    map.insert(key.to_string(), value);
    storage::write_json_file(
        &root.join(DEPENDENCIES_FILE_NAME),
        &serde_json::Value::Object(map),
        formats::DEPENDENCIES,
        false,
    )
}

// --- The installed version, read from the artifact ---

/// `bin/ffmpeg.json` — the version sidecar beside `bin/ffmpeg[.exe]`. Stem
/// plus the role extension, never a suffix dot-appended to the full filename
/// (so it is `ffmpeg.json`, not `ffmpeg.exe.json`), per the derived-filename
/// grammar.
pub fn version_sidecar_path(root: &Path) -> PathBuf {
    let name = ffmpeg_file_name();
    let stem = name.strip_suffix(".exe").unwrap_or(name);
    root.join(BIN_DIR_NAME).join(format!("{stem}.json"))
}

/// Whether this platform's ffmpeg can report a version comparable with what
/// its source calls "latest" (see the module header).
fn ffmpeg_reports_its_own_version() -> bool {
    !cfg!(windows)
}

/// Records a just-published binary's version beside it. Called only where the
/// binary cannot report a comparable version, and only AFTER the binary itself
/// has landed: a crash between the two leaves a present binary reading
/// version-unknown, which offers a re-acquire — where writing the sidecar
/// first would leave the OLD binary wearing the NEW version's label and
/// reading as up to date.
fn write_version_sidecar(root: &Path, version: &str, file_version: crate::file_identity::FileVersion) -> Result<(), String> {
    if !file_version.matches_path(&ffmpeg_path(root)).map_err(|error| error.to_string())? {
        return Err("ffmpeg changed before its installed version could be recorded".to_string());
    }
    let payload = serde_json::json!({
        "version": version,
        "fileVersion": file_version,
        "installedAt": logging::now_iso_millis(),
    });
    // not recorded: a sidecar colocated in the binary-bearing bin/ directory,
    // describing the re-fetchable binary it sits beside — meaningless without that
    // binary (itself excluded as a re-fetchable binary) and rewritten by the next
    // install (data-backup conventions).
    storage::write_json_file(
        &version_sidecar_path(root),
        &payload,
        formats::FFMPEG_VERSION_SIDECAR,
        false,
    )
}

fn read_version_sidecar(root: &Path) -> Option<String> {
    let value = match storage::read_json_file(&version_sidecar_path(root), formats::FFMPEG_VERSION_SIDECAR) {
        Ok(storage::JsonFile::Document(value)) => value,
        Ok(storage::JsonFile::Newer(newer)) => {
            storage::warn_newer(&newer);
            return None;
        }
        _ => return None,
    };
    let expected: crate::file_identity::FileVersion = serde_json::from_value(value.get("fileVersion")?.clone()).ok()?;
    if !expected.matches_path(&ffmpeg_path(root)).ok()? { return None; }
    let version = value.get("version")?.as_str()?.trim();
    (!version.is_empty()).then(|| binaries::normalize_version(version))
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelIdentity {
    sha256: String,
    bytes: u64,
    #[serde(default)]
    file_version: Option<crate::file_identity::FileVersion>,
}

fn model_identity_path(root: &Path, spec: &DependencySpec) -> PathBuf {
    installed_path(root, spec).with_extension("json")
}

fn read_model_identity(root: &Path, spec: &DependencySpec) -> Option<String> {
    verified_model_digest(&installed_path(root, spec)).map(|digest| digest[..12].to_string())
}

/// The full verified digest belongs only to the published file version recorded
/// at installation. Presence alone never assigns a model the current build's pin.
pub(crate) fn verified_model_digest(path: &Path) -> Option<String> {
    let document = match storage::read_json_file(&path.with_extension("json"), formats::MODEL_IDENTITY) {
        Ok(storage::JsonFile::Document(document)) => document,
        Ok(storage::JsonFile::Newer(newer)) => {
            storage::warn_newer(&newer);
            return None;
        }
        _ => return None,
    };
    let identity: ModelIdentity = serde_json::from_value(document).ok()?;
    if identity.sha256.len() != 64
        || !identity.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    let expected = identity.file_version?;
    let (mut file, _) = crate::file_identity::open_regular_nofollow(path).ok()?;
    if file.metadata().ok()?.len() != identity.bytes
        || crate::file_identity::FileVersion::of(&mut file).ok()? != expected
        || !expected.matches_path(path).ok()?
    {
        return None;
    }
    Some(identity.sha256.to_ascii_lowercase())
}

fn write_model_identity(
    root: &Path,
    spec: &DependencySpec,
    pinned: &PinnedArtifact,
) -> Result<(), String> {
    let path = installed_path(root, spec);
    let (mut file, _) = crate::file_identity::open_regular_nofollow(&path)
        .map_err(|error| error.to_string())?;
    let file_version = crate::file_identity::FileVersion::of(&mut file)
        .map_err(|error| error.to_string())?;
    let identity = ModelIdentity {
        sha256: pinned.sha256.to_string(),
        bytes: installed_bytes(pinned),
        file_version: Some(file_version),
    };
    // not recorded: identity for a re-downloadable model, colocated with and
    // meaningless without that model. It is invalidated before replacement.
    storage::write_json_file(
        &model_identity_path(root, spec),
        &serde_json::to_value(&identity).map_err(|e| e.to_string())?,
        formats::MODEL_IDENTITY,
        false,
    )
}

fn installed_bytes(pinned: &PinnedArtifact) -> u64 {
    pinned
        .extracted
        .as_ref()
        .map(|extracted| extracted.bytes)
        .unwrap_or(pinned.bytes)
}

fn invalidate_model_identity(root: &Path, spec: &DependencySpec) -> Result<(), String> {
    let path = model_identity_path(root, spec);
    storage::refuse_newer(&path, formats::MODEL_IDENTITY)?;
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

const FFMPEG_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Presence remains distinct from a version that this machine can actually
/// read. Failed installed probes remain unknown and retryable.
fn probe_ffmpeg_version(root: &Path) -> Option<String> {
    probe_ffmpeg_at(&ffmpeg_path(root), &|| false, None).ok()
}

fn probe_ffmpeg_at(
    path: &Path,
    cancelled: &dyn Fn() -> bool,
    owner: Option<Arc<dyn Send + Sync>>,
) -> Result<String, String> {
    let timeout = FFMPEG_PROBE_TIMEOUT;
    let started = std::time::Instant::now();
    let mut command = std::process::Command::new(path);
    command.arg("-version");
    let result = subprocess::run_bounded_idle_retaining(command,
        &|| cancelled() || started.elapsed() >= timeout, timeout, owner);
    if started.elapsed() >= timeout {
        return Err("ffmpeg version probe timed out".to_string());
    }
    let run = result?;
    if !run.status_ok {
        return Err(format!("ffmpeg version probe failed: {}", run.stderr_text()));
    }
    binaries::parse_ffmpeg_version(&String::from_utf8_lossy(&run.stdout))
        .ok_or_else(|| "ffmpeg did not report a recognizable version".to_string())
}

struct ObservedFfmpegVersion {
    path: PathBuf,
    file_version: crate::file_identity::FileVersion,
    version: String,
}

/// Retain successful artifact-bound observations across status reads. A failed
/// probe is retryable at the next explicit status refresh, not cached for the
/// launch. Rendering consumes the application store and never probes here.
static INSTALLED_FFMPEG_VERSION: std::sync::LazyLock<
    std::sync::Mutex<Option<ObservedFfmpegVersion>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

fn binary_file_version(path: &Path) -> Result<crate::file_identity::FileVersion, String> {
    let (mut file, _) = crate::file_identity::open_regular_nofollow(path).map_err(|error| error.to_string())?;
    crate::file_identity::FileVersion::of(&mut file).map_err(|error| error.to_string())
}

fn forget_installed_ffmpeg_version() {
    *INSTALLED_FFMPEG_VERSION
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = None;
}

fn installed_ffmpeg_version(root: &Path) -> Option<String> {
    installed_ffmpeg_version_with(root, &INSTALLED_FFMPEG_VERSION, |root| {
        if ffmpeg_reports_its_own_version() { probe_ffmpeg_version(root) }
        else { read_version_sidecar(root) }
    })
}

fn installed_ffmpeg_version_with(
    root: &Path,
    observations: &std::sync::Mutex<Option<ObservedFfmpegVersion>>,
    read: impl FnOnce(&Path) -> Option<String>,
) -> Option<String> {
    let path = ffmpeg_path(root);
    let file_version = binary_file_version(&path).ok()?;
    let mut cached = observations.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(known) = cached.as_ref().filter(|known| known.path == path && known.file_version == file_version) {
        return Some(known.version.clone());
    }
    *cached = None;
    let version = read(root)?;
    // A replacement during the probe cannot attach the old answer to new bytes.
    if !file_version.matches_path(&path).ok()? { return None; }
    *cached = Some(ObservedFfmpegVersion { path, file_version, version: version.clone() });
    Some(version)
}

// --- Resolution ---

pub fn is_cancelled_error(error: &str) -> bool {
    error == acquisition::CANCELLED_ERROR
}

/// In-flight operations by entry id. PER-ID, deliberately (developer,
/// 2026-08-17): several dependencies may download AT ONCE — only a second
/// operation on the SAME entry is refused. The facts-file RMW no longer
/// rides this claim; it has its own lock below.
struct ActiveOperation {
    operation_id: String,
    cancelled: Arc<AtomicBool>,
}

static IN_FLIGHT: std::sync::LazyLock<
    (
        std::sync::Mutex<std::collections::HashMap<String, ActiveOperation>>,
        std::sync::Condvar,
    ),
> = std::sync::LazyLock::new(|| {
    (
        std::sync::Mutex::new(std::collections::HashMap::new()),
        std::sync::Condvar::new(),
    )
});

struct BusyGuard {
    id: String,
    operation_id: String,
    cancelled: Arc<AtomicBool>,
    deadline: acquisition::OperationDeadline,
}
// Shared with blocking acquisition workers so logical timeout cannot release
// this entry while a previous OS call is still running.
struct EntryLease {
    id: String,
    operation_id: String,
}

impl Drop for EntryLease {
    fn drop(&mut self) {
        let (operations, settled) = &*IN_FLIGHT;
        let mut in_flight = operations.lock().unwrap_or_else(|p| p.into_inner());
        if in_flight
            .get(&self.id)
            .is_some_and(|active| active.operation_id == self.operation_id)
        {
            in_flight.remove(&self.id);
            settled.notify_all();
        }
    }
}

fn claim(
    id: &str,
    operation_id: &str,
    mut deadline: acquisition::OperationDeadline,
) -> Result<BusyGuard, String> {
    if operation_id.trim().is_empty() {
        return Err("dependency operation id is empty".to_string());
    }
    if crate::app_lifecycle::shutting_down() {
        return Err("managed dependencies are shutting down".to_string());
    }
    let (operations, _) = &*IN_FLIGHT;
    let mut in_flight = operations.lock().unwrap_or_else(|p| p.into_inner());
    if crate::app_lifecycle::shutting_down() {
        return Err("managed dependencies are shutting down".to_string());
    }
    if in_flight.contains_key(id) {
        return Err(format!("{id} is already being worked on"));
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    in_flight.insert(
        id.to_string(),
        ActiveOperation {
            operation_id: operation_id.to_string(),
            cancelled: cancelled.clone(),
        },
    );
    deadline.retain_owner(Arc::new(EntryLease {
        id: id.to_string(),
        operation_id: operation_id.to_string(),
    }));
    Ok(BusyGuard {
        id: id.to_string(),
        operation_id: operation_id.to_string(),
        cancelled,
        deadline,
    })
}

/// Requests cancellation of the current operation for one registry entry.
/// Network futures race this flag directly, so cancellation does not wait for
/// a connect, TLS handshake, metadata response, or stalled body read.
pub fn cancel_entry(id: &str, operation_id: &str) -> bool {
    let (operations, _) = &*IN_FLIGHT;
    let in_flight = operations.lock().unwrap_or_else(|p| p.into_inner());
    let Some(active) = in_flight.get(id) else {
        return false;
    };
    if active.operation_id != operation_id {
        return false;
    }
    active.cancelled.store(true, Ordering::Relaxed);
    true
}

/// Normal application exit cancels every managed-dependency operation and
/// then joins this registry before allowing process shutdown. Each acquisition
/// owns bounded external waits and observes the same cancellation flag.
pub fn begin_shutdown() {
    let (operations, _) = &*IN_FLIGHT;
    let in_flight = operations.lock().unwrap_or_else(|p| p.into_inner());
    for active in in_flight.values() {
        active.cancelled.store(true, Ordering::Relaxed);
    }
}

pub fn wait_for_idle() {
    let (operations, settled) = &*IN_FLIGHT;
    let mut in_flight = operations.lock().unwrap_or_else(|p| p.into_inner());
    while !in_flight.is_empty() {
        in_flight = settled
            .wait(in_flight)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

/// A claim acquired at command admission, before queueing the blocking worker,
/// so cancellation can reach an operation whose worker has not started.
pub struct StartedInstall(BusyGuard);

pub fn begin_install(id: &str, operation_id: &str) -> Result<StartedInstall, String> {
    let spec = spec_of(id).ok_or_else(|| format!("unknown dependency: {id}"))?;
    let expected = spec.pinned.as_ref().map(|pin| pin.bytes);
    claim(
        id,
        operation_id,
        acquisition::OperationDeadline::for_install(expected),
    )
    .map(StartedInstall)
}

/// Confirms a local file's digest against the one it was supposed to arrive
/// with. This is pure local-file verification — it never opens a network
/// connection — so a regression here (comparing the wrong digest, or
/// accepting a mismatch) is reachable directly by a test with a local
/// fixture, with no server standing in for a real download.
fn verify_checksum(
    path: &Path,
    expected_sha256: &str,
    cancelled: &AtomicBool,
    deadline: &acquisition::OperationDeadline,
    mut on_progress: impl FnMut(u64, u64),
    mismatch_message: impl FnOnce(&str) -> String,
) -> Result<(), String> {
    let actual = acquisition::file_sha256(path, cancelled, deadline, &mut on_progress)?;
    if actual != expected_sha256 {
        return Err(mismatch_message(&actual));
    }
    Ok(())
}

/// Confirms a downloaded pinned artifact against its recorded digest, and,
/// when the pin names an archive entry, extracts and confirms that entry too.
/// Both checks are `verify_checksum` and `acquisition::extract_pinned_zip_entry`
/// over local files the caller already produced, so the whole boundary is
/// exercised by tests without a network seam. Returns the path callers should
/// publish from: `staged` when an entry was extracted, `downloaded` otherwise.
/// A mismatch leaves nothing published.
fn verify_pinned_artifact(
    id: &str,
    downloaded: &Path,
    staged: &Path,
    pinned: &PinnedArtifact,
    cancelled: &Arc<AtomicBool>,
    deadline: &acquisition::OperationDeadline,
    mut on_verify_progress: impl FnMut(u64, u64),
) -> Result<PathBuf, String> {
    verify_checksum(
        downloaded,
        pinned.sha256,
        cancelled,
        deadline,
        &mut on_verify_progress,
        |actual| format!("checksum mismatch for {id}: expected {}, got {actual}", pinned.sha256),
    )?;
    let Some(extracted) = pinned.extracted.as_ref() else {
        return Ok(downloaded.to_path_buf());
    };
    acquisition::extract_pinned_zip_entry(
        downloaded,
        staged,
        extracted.archive_entry,
        extracted.bytes,
        cancelled,
        deadline,
    )?;
    verify_checksum(
        staged,
        extracted.sha256,
        cancelled,
        deadline,
        &mut on_verify_progress,
        |actual| {
            format!(
                "checksum mismatch for extracted {id}: expected {}, got {actual}",
                extracted.sha256
            )
        },
    )?;
    Ok(staged.to_path_buf())
}

pub fn install_entry_started(
    root: &Path,
    started: StartedInstall,
    mut on_progress: impl FnMut(InstallProgress),
) -> Result<BinaryFacts, String> {
    let id = started.0.id.clone();
    let spec = spec_of(&id).ok_or_else(|| format!("unknown dependency: {id}"))?;
    match spec.kind {
        DependencyKind::Binary => install_ffmpeg_started(root, started.0, on_progress),
        DependencyKind::Runtime | DependencyKind::Model => {
            let guard = started.0;
            let pinned = spec.pinned.as_ref().ok_or("pinned entry carries no pin")?;
            let temp = root.join(TEMP_DIR_NAME);
            std::fs::create_dir_all(&temp).map_err(|e| e.to_string())?;
            let partial = temp.join(format!("{id}-{}.partial", nanoid::generate()?));
            let staged = temp.join(format!("{id}-{}.staged", nanoid::generate()?));
            let cleanup = Arc::new(
                acquisition::RemoveFilesOnDrop::new(vec![partial.clone(), staged.clone()]));
            let result = (|| -> Result<BinaryFacts, String> {
                on_progress(InstallProgress::bytes(
                    InstallPhase::Download,
                    0,
                    Some(pinned.bytes),
                    InstallPhase::Verify,
                ));
                acquisition::download_to(
                    pinned.url,
                    &partial,
                    &guard.cancelled,
                    &guard.deadline,
                    Some(pinned.bytes),
                    cleanup.clone(),
                    |done, total| {
                        on_progress(InstallProgress::bytes(
                            InstallPhase::Download,
                            done,
                            total,
                            InstallPhase::Verify,
                        ));
                    },
                )?;
                let publish_source = verify_pinned_artifact(
                    &id,
                    &partial,
                    &staged,
                    pinned,
                    &guard.cancelled,
                    &guard.deadline,
                    |done, total| {
                        on_progress(InstallProgress::bytes(
                            InstallPhase::Verify,
                            done,
                            Some(total),
                            InstallPhase::Install,
                        ));
                    },
                )?;
                let publish_source = publish_source.as_path();
                let target = installed_path(root, spec);
                let parent = target
                    .parent()
                    .ok_or_else(|| format!("installed path has no parent: {}", crate::native_path::label(&target)))?;
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                acquisition::check_cancelled(&guard.cancelled)?;
                on_progress(InstallProgress::fixed(
                    InstallPhase::Install,
                    0,
                    None,
                ));
                // An old identity must never survive a replacement attempt:
                // if publication or the new identity write fails, state stays
                // installed-unchecked instead of assigning either digest.
                invalidate_model_identity(root, spec)?;
                guard.deadline.check(&guard.cancelled)?;
                // Replace-in-place over any previous model (same volume).
                // not recorded: the model file is a re-downloadable artifact.
                acquisition::publish_staged(publish_source, &target)?;
                write_model_identity(root, spec, pinned)?;
                on_progress(InstallProgress::fixed(
                    InstallPhase::Install,
                    1,
                    None,
                ));
                // The facts store remains untouched: latest is the pin compiled
                // into this app, while installed identity belongs beside the
                // verified artifact rather than in network facts.
                let facts = BinaryFacts {
                    latest_known_version: Some(pin_version(pinned)),
                    last_checked_at_utc: None,
                };
                logging::info(
                    "pinned dependency installed",
                    serde_json::json!({ "id": id, "path": crate::native_path::encode(&target) }),
                );
                Ok(facts)
            })();
            result
        }
    }
}

/// The checksum-verified, platform-prepared stage must run before replacing
/// the working tool. Its cleanup follows the physical probe owner on timeout.
fn activate_ffmpeg(
    root: &Path,
    staged: &Path,
    version: &str,
    cancelled: &AtomicBool,
    deadline: &acquisition::OperationDeadline,
    cleanup: Arc<acquisition::RemoveFilesOnDrop>,
) -> Result<(), String> {
    deadline.check(cancelled)?;
    let staged_version = binary_file_version(staged)?;
    let probe = probe_ffmpeg_at(staged, &|| deadline.check(cancelled).is_err(),
        Some(Arc::new((cleanup.clone(), deadline.physical_owner()))));
    deadline.check(cancelled)?;
    let reported = probe?;
    if ffmpeg_reports_its_own_version() && reported != binaries::normalize_version(version) {
        return Err(format!("ffmpeg reported version {reported}, but the selected build is {version}"));
    }
    staged_version.require_path(staged).map_err(|error| error.to_string())?;
    let target = ffmpeg_path(root);
    let parent = target.parent().ok_or_else(|| "installed ffmpeg path has no parent".to_string())?;
    std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    deadline.check(cancelled)?;
    // not recorded: a verified, re-downloadable managed executable.
    acquisition::publish_staged(staged, &target)?;
    forget_installed_ffmpeg_version();
    if !ffmpeg_reports_its_own_version() {
        write_version_sidecar(root, version, staged_version)?;
        forget_installed_ffmpeg_version();
    }
    Ok(())
}

fn install_ffmpeg_started(
    root: &Path,
    guard: BusyGuard,
    mut on_progress: impl FnMut(InstallProgress),
) -> Result<BinaryFacts, String> {
    let temp = root.join(TEMP_DIR_NAME);
    std::fs::create_dir_all(&temp).map_err(|e| e.to_string())?;

    guard.deadline.check(&guard.cancelled)?;
    on_progress(InstallProgress::fixed(
        InstallPhase::Resolve,
        0,
        Some(InstallPhase::Download),
    ));
    let resolved = acquisition::resolve_latest(&guard.cancelled, &guard.deadline)?;
    // The digest belongs to the build just resolved. A rolling release can be
    // republished at any moment, so fetching it after a long download could
    // pair these bytes with another build's digest and fail spuriously, or
    // record the older build's name for newer bytes.
    guard.deadline.check(&guard.cancelled)?;
    let expected = binaries::parse_sums(
        &acquisition::fetch_text(&resolved.sums_url, &guard.cancelled, &guard.deadline)?,
        &resolved.sums_asset,
    )
    .ok_or_else(|| format!("{} not in the checksum file", resolved.sums_asset))?;
    on_progress(InstallProgress::fixed(
        InstallPhase::Resolve,
        1,
        Some(InstallPhase::Download),
    ));

    let partial = temp.join(format!("ffmpeg-{}.partial", nanoid::generate()?));
    let staged = temp.join(format!("ffmpeg-{}-{}", nanoid::generate()?, ffmpeg_file_name()));
    let cleanup = Arc::new(acquisition::RemoveFilesOnDrop::new(vec![partial.clone(), staged.clone()]));
    let result = (|| -> Result<BinaryFacts, String> {
        guard.deadline.check(&guard.cancelled)?;
        acquisition::download_to(
            &resolved.download_url,
            &partial,
            &guard.cancelled,
            &guard.deadline,
            None,
            cleanup.clone(),
            |done, total| {
                on_progress(InstallProgress::bytes(
                    InstallPhase::Download,
                    done,
                    total,
                    InstallPhase::Verify,
                ));
            },
        )?;
        guard.deadline.check(&guard.cancelled)?;
        verify_checksum(
            &partial,
            &expected,
            &guard.cancelled,
            &guard.deadline,
            |done, total| {
                on_progress(InstallProgress::bytes(
                    InstallPhase::Verify,
                    done,
                    Some(total),
                    InstallPhase::Install,
                ));
            },
            |actual| {
                format!(
                    "checksum mismatch for {}: expected {expected}, got {actual}",
                    resolved.sums_asset
                )
            },
        )?;

        on_progress(InstallProgress::fixed(
            InstallPhase::Install,
            0,
            None,
        ));
        acquisition::extract_ffmpeg(
            &partial,
            &staged,
            ffmpeg_file_name(),
            &guard.cancelled,
            &guard.deadline,
        )?;
        acquisition::make_runnable(&staged, &guard.cancelled, &guard.deadline)?;

        activate_ffmpeg(root, &staged, &resolved.version, &guard.cancelled, &guard.deadline, cleanup.clone())?;

        // Only the upstream fact is persisted. What is now installed is read back
        // from the binary itself.
        let facts = BinaryFacts {
            latest_known_version: Some(resolved.version.clone()),
            last_checked_at_utc: Some(logging::now_iso_millis()),
        };
        save_facts_for(root, "ffmpeg", &facts)?;
        on_progress(InstallProgress::fixed(
            InstallPhase::Install,
            1,
            None,
        ));
        logging::info(
            "ffmpeg installed",
            serde_json::json!({ "version": resolved.version, "path": crate::native_path::encode(&ffmpeg_path(root)) }),
        );
        Ok(facts)
    })();
    result
}

/// Version check only. Success updates `latestKnownVersion` + the check stamp;
/// failure writes NOTHING so stale knowledge is never dressed up as fresh.
/// BINARIES ONLY: a model's "latest" is the pin compiled into this build, so
/// there is nothing to ask and nothing to stamp — `state_of` derives it and
/// a re-pin shipped in an app update shows up on its own.
pub fn check_entry(root: &Path, id: &str) -> Result<BinaryFacts, String> {
    let operation_id = nanoid::generate()?;
    check_entry_with_operation(root, id, &operation_id)
}

/// Checks one managed tool's latest version. A check that fails returns before
/// anything is saved, so the tool keeps its last successful facts.
pub fn check_entry_with_operation(
    root: &Path,
    id: &str,
    operation_id: &str,
) -> Result<BinaryFacts, String> {
    let spec = spec_of(id).ok_or_else(|| format!("unknown dependency: {id}"))?;
    let guard = claim(
        id,
        operation_id,
        acquisition::OperationDeadline::for_check(),
    )?;
    let mut facts = load_facts_for(root, id);
    match spec.kind {
        DependencyKind::Binary => {
            let resolved = acquisition::resolve_latest(&guard.cancelled, &guard.deadline)?;
            facts.latest_known_version = Some(resolved.version);
        }
        // Refused rather than faked: a pinned artifact has no upstream to ask. Its
        // selected version belongs to this app build, and `state_of` derives it.
        DependencyKind::Runtime | DependencyKind::Model => {
            return Err(format!(
                "{id} is selected by this app build — there is no update to check for"
            ));
        }
    }
    facts.last_checked_at_utc = Some(logging::now_iso_millis());
    save_facts_for(root, id, &facts)?;
    Ok(facts)
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DependencyState {
    pub id: String,
    pub label: String,
    pub kind: DependencyKind,
    pub status: BinaryStatus,
    /// Read from the artifact on every status — the binary's own banner, the
    /// sidecar beside it, or a model's verified-install identity. None on a
    /// present entry means the version could not be read: not absent, and never
    /// dressed up as up to date.
    pub installed_version: Option<String>,
    pub facts: BinaryFacts,
    pub path: String,
    /// True when absence blocks a core presentation path. The frontend uses
    /// this only to distinguish missing-status emphasis; failures have their
    /// own stronger presentation.
    pub required_for_core: bool,
    /// True when this entry's "latest" is DISCOVERABLE — a binary resolved
    /// live from upstream. A model's latest is selected by the app build, so
    /// there is nothing to look up and nothing to check.
    pub checkable: bool,
    /// A pinned artifact's upstream publication date — how old this model
    /// actually is. None for binaries, whose live version is the answer.
    pub released: Option<String>,
    /// The pinned artifact's upstream release number, where it has one.
    pub upstream_version: Option<String>,
    /// Pinned transfer size, not the extracted runtime's installed size.
    /// Live-resolved binaries remain unknown until acquisition resolves them.
    pub download_bytes: Option<u64>,
}

/// One entry's live state; presence re-scanned from disk, never persisted.
/// A model's presence check is size-exact — a truncated download that somehow
/// reached the models dir reads not-installed. Identity is separate: a full-
/// sized file without a verified-install identity is installed-unchecked.
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
        DependencyKind::Runtime | DependencyKind::Model => {
            let expected = spec.pinned.as_ref().map(installed_bytes);
            std::fs::metadata(&path)
                .map(|m| m.is_file() && Some(m.len()) == expected)
                .unwrap_or(false)
        }
    };
    // Presence and identity stay separate: exact size rejects truncation, while
    // the colocated verified-install record says which digest was published.
    // Missing or malformed identity remains honestly unreadable.
    let installed_version = if !present {
        None
    } else {
        match spec.pinned.as_ref() {
            Some(_) => read_model_identity(root, spec),
            None => installed_ffmpeg_version(root),
        }
    };
    DependencyState {
        id: spec.id.to_string(),
        label: spec.label.to_string(),
        kind: spec.kind,
        status: binaries::derive_status(present, installed_version.as_deref(), &facts),
        path: crate::native_path::encode(&path).into_owned(),
        installed_version,
        facts,
        required_for_core: spec.required_for_core,
        checkable: matches!(spec.kind, DependencyKind::Binary),
        released: spec.pinned.as_ref().map(|p| p.released.to_string()),
        upstream_version: spec
            .pinned
            .as_ref()
            .and_then(|p| p.upstream_version)
            .map(str::to_string),
        download_bytes: spec.pinned.as_ref().map(|p| p.bytes),
    }
}

/// Every registry entry's state, in display order.
pub fn states(root: &Path) -> Vec<DependencyState> {
    DEPENDENCIES
        .iter()
        .filter(|spec| available_on_current_platform(spec))
        .map(|spec| state_of(root, spec))
        .collect()
}

#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises the private
// model-identity sidecar (`ModelIdentity`, `write_model_identity`);
// promoting it would widen the crate's API only for this test.
#[path = "../tests/unit/binaries_manager.rs"]
mod tests;

/// The terminal answer of one install: the command response is the single
/// authoritative boundary, carrying a fresh artifact-derived row.
#[derive(serde::Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum InstallResult {
    Installed {
        #[serde(rename = "operationId")]
        operation_id: String,
        state: DependencyState,
    },
    Cancelled {
        #[serde(rename = "operationId")]
        operation_id: String,
        state: DependencyState,
    },
    Failed {
        #[serde(rename = "operationId")]
        operation_id: String,
        state: DependencyState,
        error: String,
    },
}

/// Installs or updates one entry on the calling (blocking) thread. Progress
/// is an operation-correlated `binaries://progress` event under the shared
/// progress throttle; the outcome clears or raises the entry's install Issue
/// and wakes derived work when a tool may have become usable.
pub fn install_reported(
    app: &tauri::AppHandle,
    root: &Path,
    started: StartedInstall,
) -> Result<InstallResult, String> {
    let id = started.0.id.clone();
    let operation_id = started.0.operation_id.clone();
    let id = id.as_str();
    let operation_id = operation_id.as_str();
    let progress_event_failed = std::cell::Cell::new(false);
    let mut throttle = crate::progress_throttle::ProgressThrottle::<InstallPhase>::default();
    let emit = |progress: InstallProgress| {
        if crate::app_lifecycle::shutting_down() {
            return;
        }
        let completed = progress.total.is_some_and(|total| progress.done >= total);
        if !throttle.admit(progress.phase, completed, std::time::Instant::now()) {
            return;
        }
        if let Err(error) = crate::failure_runtime::emit_checked(
            app,
            "binaries://progress",
            serde_json::json!({
                "id": id,
                "operationId": operation_id,
                "phase": progress.phase,
                "done": progress.done,
                "total": progress.total,
                "nextPhase": progress.next_phase,
            }),
        ) {
            if !progress_event_failed.replace(true) {
                let _ = crate::failure_runtime::report(app, "event-delivery-failed", Some(id), &error);
            }
        }
    };
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        install_entry_started(root, started, emit)
    }));
    let spec = spec_of(id).ok_or_else(|| format!("unknown dependency: {id}"))?;
    let state = state_of(root, spec);
    let operation_id = operation_id.to_string();
    Ok(match outcome {
        Ok(Ok(_facts)) => {
            crate::failure_runtime::clear("dependency-install-failed", Some(id));
            crate::derived_work::wake();
            InstallResult::Installed { operation_id, state }
        }
        Ok(Err(error)) if is_cancelled_error(&error) => {
            logging::info(
                "dependency install cancelled",
                serde_json::json!({ "id": id, "operationId": operation_id }),
            );
            InstallResult::Cancelled { operation_id, state }
        }
        Ok(Err(error)) => install_failed(app, id, operation_id, state, error),
        Err(payload) => install_failed(
            app,
            id,
            operation_id,
            state,
            crate::failure_runtime::panic_message(payload),
        ),
    })
}

fn install_failed(
    app: &tauri::AppHandle,
    id: &str,
    operation_id: String,
    state: DependencyState,
    error: String,
) -> InstallResult {
    logging::warn(
        "dependency install failed",
        serde_json::json!({
            "id": id,
            "operationId": operation_id,
            "error": { "message": error.clone() }
        }),
    );
    let _ = crate::failure_runtime::report(app, "dependency-install-failed", Some(id), &error);
    if state.status != BinaryStatus::NotInstalled {
        crate::derived_work::wake();
    }
    InstallResult::Failed {
        operation_id,
        state,
        error,
    }
}

#[derive(serde::Serialize)]
#[serde(tag = "outcome", rename_all = "kebab-case")]
pub enum CheckOutcome {
    Completed { states: Vec<DependencyState> },
    Cancelled,
}

impl CheckOutcome {
    pub fn log_fields(&self) -> serde_json::Value {
        match self {
            Self::Completed { states } => {
                serde_json::json!({ "outcome": "completed", "entries": states.len() })
            }
            Self::Cancelled => serde_json::json!({ "outcome": "cancelled" }),
        }
    }
}

/// A version check for one entry; never installs, and a failure writes
/// nothing.
pub fn check_reported(root: &Path, id: &str, operation_id: &str) -> Result<CheckOutcome, String> {
    match check_entry_with_operation(root, id, operation_id) {
        Ok(_) => Ok(CheckOutcome::Completed { states: states(root) }),
        Err(error) if error == acquisition::CANCELLED_ERROR => Ok(CheckOutcome::Cancelled),
        Err(error) => Err(error),
    }
}
