//! Registry and orchestration for managed dependencies (`binaries.rs` owns
//! pure status decisions; `binaries_acquisition.rs` owns acquisition): N
//! entries of two kinds — binaries
//! (resolved live per platform) and model files (pinned artifacts) — each
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
//!                    install. BtbN ships rolling master builds (`N-119123-g…`)
//!                    under a release named by build time — two namespaces, so
//!                    probing would report a phantom update forever.
//!   a model          read a verified-install identity beside the model. File
//!                    size establishes usable presence, while the identity
//!                    records which digest was verified before publication.
//!                    A missing identity stays installed-unchecked; an older
//!                    digest remains update-available even at the same size.
//!
//! Endpoint shapes verified live at build time:
//!   macOS arm64  https://ffmpeg.martin-riedl.de/redirect/latest/macos/arm64/release/ffmpeg.zip
//!                → 307 to /download/macos/arm64/<epoch>_<version>/ffmpeg.zip,
//!                with a `<url>.sha256` sidecar (`<hex>  ffmpeg.zip`).
//!   Windows x64  GitHub latest release of BtbN/FFmpeg-Builds: the
//!                `ffmpeg-master-latest-win64-gpl.zip` asset plus a
//!                `checksums.sha256` asset; the release name is the version.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::binaries::{self, BinaryFacts, BinaryStatus};
use crate::binaries_acquisition as acquisition;
use crate::{logging, nanoid, storage, subprocess};

// Subpath names are owned by the one resolver module (storage-path
// conventions); re-exported here so existing call sites keep their imports.
pub use crate::paths::{BIN_DIR_NAME, DEPENDENCIES_FILE_NAME, MODELS_DIR_NAME, TEMP_DIR_NAME};

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
        id: "whisper-large-v3-turbo",
        label: "Transcription model (Whisper large-v3-turbo)",
        kind: DependencyKind::Model,
        file_name: "ggml-large-v3-turbo.bin",
        pinned: Some(PinnedModel {
            // Canonical whisper.cpp model repository, fixed to the verified
            // commit that added this file; sha256 matches its Xet metadata.
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/98aa99a0a9db05ae2342309f5096248665f7cba3/ggml-large-v3-turbo.bin",
            sha256: "1fc70f774d38eb169993ac391eea357ef47c88757ef72ee5943879b7e8e2bc69",
            bytes: 1_624_555_275,
            released: "2024-10-01",
        }),
    },
    DependencySpec {
        id: "ultraface-rfb640",
        label: "Face detector (optional — Settings > Score faces)",
        kind: DependencyKind::Model,
        file_name: "ultraface-rfb640.onnx",
        pinned: Some(PinnedModel {
            // Official ONNX model zoo (repo Apache-2.0; the Ultraface upstream
            // is MIT), fixed to the file's immutable commit; sha256 verified
            // against the bytes at that revision 2026-08-22.
            // The 640 variant over the 320: same family and licence, double the
            // input resolution, so a face that is small in the frame — the
            // common case in family photos — is still found.
            url: "https://media.githubusercontent.com/media/onnx/models/4c46cd00fbdb7cd30b6c1c17ab54f2e1f4f7b177/validated/vision/body_analysis/ultraface/models/version-RFB-640.onnx",
            sha256: "8f4c659275977e7a3bfbfa339a9c769ad793df50f9c0baa8c14b11baa1646430",
            bytes: 1_588_012,
            released: "2020-12-17",
        }),
    },
    DependencySpec {
        id: "hsemotion-enet-b2",
        label: "Expression model (optional — Settings > Score faces)",
        kind: DependencyKind::Model,
        file_name: "hsemotion-enet-b2-8.onnx",
        pinned: Some(PinnedModel {
            // HSEmotion's AffectNet-trained EfficientNet-B2, from the project's
            // current home (sb-ai-lab/EmotiEffLib, Apache-2.0 — the old
            // av-savchenko repo redirects here); sha256 computed from the
            // immutable file commit and verified against its bytes 2026-08-22.
            // Replaces FER+, which was trained
            // on 2016-era FER data and is five years older. Face scoring needs
            // BOTH this and the detector — either alone stays inert.
            url: "https://raw.githubusercontent.com/sb-ai-lab/EmotiEffLib/af833487321c3efdcb1768a91a6c656a1986fdf6/models/affectnet_emotions/onnx/enet_b2_8.onnx",
            sha256: "180a9d4845b59393de4511598a0d1d34b705034691ea32959ce5009db7cf52b7",
            bytes: 30_779_724,
            released: "2022-11-09",
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
fn write_version_sidecar(root: &Path, version: &str) -> Result<(), String> {
    let payload = serde_json::json!({
        "version": version,
        "installedAt": logging::now_iso_millis(),
    });
    let mut text = serde_json::to_string_pretty(&payload).map_err(|e| e.to_string())?;
    text.push('\n');
    // not recorded: a sidecar colocated in the binary-bearing bin/ directory,
    // describing the re-fetchable binary it sits beside — meaningless without that
    // binary (itself excluded as a re-fetchable binary) and rewritten by the next
    // install (data-backup conventions).
    storage::write_atomic_unrecorded(&version_sidecar_path(root), text.as_bytes())
}

fn read_version_sidecar(root: &Path) -> Option<String> {
    let bytes = std::fs::read(version_sidecar_path(root)).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let version = value.get("version")?.as_str()?.trim();
    (!version.is_empty()).then(|| binaries::normalize_version(version))
}

#[derive(serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelIdentity {
    sha256: String,
    bytes: u64,
}

fn model_identity_path(root: &Path, spec: &DependencySpec) -> PathBuf {
    installed_path(root, spec).with_extension("json")
}

fn read_model_identity(root: &Path, spec: &DependencySpec, bytes: u64) -> Option<String> {
    let raw = std::fs::read(model_identity_path(root, spec)).ok()?;
    let identity: ModelIdentity = serde_json::from_slice(&raw).ok()?;
    if identity.bytes != bytes
        || identity.sha256.len() != 64
        || !identity.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return None;
    }
    Some(identity.sha256[..12].to_ascii_lowercase())
}

fn write_model_identity(
    root: &Path,
    spec: &DependencySpec,
    pinned: &PinnedModel,
) -> Result<(), String> {
    let identity = ModelIdentity {
        sha256: pinned.sha256.to_string(),
        bytes: pinned.bytes,
    };
    let mut text = serde_json::to_string_pretty(&identity).map_err(|e| e.to_string())?;
    text.push('\n');
    // not recorded: identity for a re-downloadable model, colocated with and
    // meaningless without that model. It is invalidated before replacement.
    storage::write_atomic_unrecorded(&model_identity_path(root, spec), text.as_bytes())
}

fn invalidate_model_identity(root: &Path, spec: &DependencySpec) -> Result<(), String> {
    match std::fs::remove_file(model_identity_path(root, spec)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Runs the installed ffmpeg and reads the version out of its banner. Bounded
/// like every other subprocess in the app, so a wedged binary cannot hang the
/// caller; a failure to run, a non-zero exit, or unrecognized output all yield
/// None — present-but-unreadable, which is not the same as absent.
fn probe_ffmpeg_version(root: &Path) -> Option<String> {
    let mut command = std::process::Command::new(ffmpeg_path(root));
    command.arg("-version");
    // A healthy -version answers in milliseconds; ten seconds covers a cold
    // start off a spun-down disk without letting a wedged binary hold the
    // status read for the media default of 120s.
    let run =
        subprocess::run_bounded_idle(command, &|| false, std::time::Duration::from_secs(10))
            .ok()?;
    if !run.status_ok {
        return None;
    }
    binaries::parse_ffmpeg_version(&String::from_utf8_lossy(&run.stdout))
}

/// The ffmpeg probe is a subprocess spawn, so its answer is held for the
/// process and re-read only after an install replaces the binary — it must
/// never run per render, and `state_of` is called on every status read.
static INSTALLED_FFMPEG_VERSION: std::sync::LazyLock<
    std::sync::Mutex<Option<Option<String>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(None));

fn forget_installed_ffmpeg_version() {
    *INSTALLED_FFMPEG_VERSION
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = None;
}

fn installed_ffmpeg_version(root: &Path) -> Option<String> {
    let mut cached = INSTALLED_FFMPEG_VERSION
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    if let Some(known) = cached.as_ref() {
        return known.clone();
    }
    let read = if ffmpeg_reports_its_own_version() {
        probe_ffmpeg_version(root)
    } else {
        read_version_sidecar(root)
    };
    *cached = Some(read.clone());
    read
}

// --- Resolution ---

pub fn is_cancelled_error(error: &str) -> bool {
    error == acquisition::CANCELLED_ERROR
}

/// In-flight operations by entry id. PER-ID, deliberately (developer,
/// 2026-08-17): several dependencies may download AT ONCE — only a second
/// operation on the SAME entry is refused. The facts-file RMW no longer
/// rides this claim; it has its own lock below.
static IN_FLIGHT: std::sync::LazyLock<
    std::sync::Mutex<std::collections::HashMap<String, Arc<AtomicBool>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(std::collections::HashMap::new()));

struct BusyGuard {
    id: String,
    cancelled: Arc<AtomicBool>,
}
impl Drop for BusyGuard {
    fn drop(&mut self) {
        IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner()).remove(&self.id);
    }
}

fn claim(id: &str) -> Result<BusyGuard, String> {
    let mut in_flight = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
    if in_flight.contains_key(id) {
        return Err(format!("{id} is already being worked on"));
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    in_flight.insert(id.to_string(), cancelled.clone());
    Ok(BusyGuard {
        id: id.to_string(),
        cancelled,
    })
}

/// Requests cancellation of the current operation for one registry entry.
/// Network futures race this flag directly, so cancellation does not wait for
/// a connect, TLS handshake, metadata response, or stalled body read.
pub fn cancel_entry(id: &str) -> bool {
    let in_flight = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
    let Some(cancelled) = in_flight.get(id) else {
        return false;
    };
    cancelled.store(true, Ordering::Relaxed);
    true
}

/// The full install/update for any registry entry. Binaries: resolve →
/// download → verify → extract → make runnable → publish. Models: pinned
/// download → sha256 verify → publish. Both stage in `temp/` and land with a
/// same-volume rename; both record facts only on success.
pub fn install_entry(
    root: &Path,
    id: &str,
    on_progress: impl FnMut(&str, String),
) -> Result<BinaryFacts, String> {
    let started = begin_install(id)?;
    install_entry_started(root, started, on_progress)
}

/// A claim acquired before the detached worker starts, so a Cancel click can
/// never race ahead of the operation's registration on a slow machine.
pub struct StartedInstall(BusyGuard);

pub fn begin_install(id: &str) -> Result<StartedInstall, String> {
    spec_of(id).ok_or_else(|| format!("unknown dependency: {id}"))?;
    claim(id).map(StartedInstall)
}

pub fn install_entry_started(
    root: &Path,
    started: StartedInstall,
    mut on_progress: impl FnMut(&str, String),
) -> Result<BinaryFacts, String> {
    let id = started.0.id.clone();
    let spec = spec_of(&id).ok_or_else(|| format!("unknown dependency: {id}"))?;
    match spec.kind {
        DependencyKind::Binary => install_ffmpeg_started(root, started.0, on_progress),
        DependencyKind::Model => {
            let guard = started.0;
            let pinned = spec.pinned.as_ref().ok_or("model entry carries no pin")?;
            let temp = root.join(TEMP_DIR_NAME);
            std::fs::create_dir_all(&temp).map_err(|e| e.to_string())?;
            let partial = temp.join(format!("{id}-{}.partial", nanoid::generate()));
            let _cleanup = acquisition::RemoveFilesOnDrop::new(vec![partial.clone()]);
            let result = (|| -> Result<BinaryFacts, String> {
                on_progress(
                    "download",
                    format!("{} ({} MB)", spec.label, pinned.bytes / 1_048_576),
                );
                acquisition::download_to(
                    pinned.url,
                    &partial,
                    &guard.cancelled,
                    Some(pinned.bytes),
                    |done| {
                        on_progress(
                            "download",
                            format!("{} / {} MB", done / 1_048_576, pinned.bytes / 1_048_576),
                        );
                    },
                )?;
                on_progress("verify", "checking integrity".to_string());
                let actual = acquisition::file_sha256(&partial, &guard.cancelled)?;
                if actual != pinned.sha256 {
                    return Err(format!(
                        "checksum mismatch for {id}: expected {}, got {actual}",
                        pinned.sha256
                    ));
                }
                let target = installed_path(root, spec);
                std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
                acquisition::check_cancelled(&guard.cancelled)?;
                // An old identity must never survive a replacement attempt:
                // if publication or the new identity write fails, state stays
                // installed-unchecked instead of assigning either digest.
                invalidate_model_identity(root, spec)?;
                // Replace-in-place over any previous model (same volume).
                // not recorded: the model file is a re-downloadable artifact.
                acquisition::publish_staged(&partial, &target)?;
                write_model_identity(root, spec, pinned)?;
                // The facts store remains untouched: latest is the pin compiled
                // into this app, while installed identity belongs beside the
                // verified artifact rather than in network facts.
                let facts = BinaryFacts {
                    latest_known_version: Some(pin_version(pinned)),
                    last_checked_at_utc: None,
                };
                logging::info(
                    "model installed",
                    serde_json::json!({ "id": id, "path": target.to_string_lossy() }),
                );
                Ok(facts)
            })();
            result
        }
    }
}

fn install_ffmpeg_started(
    root: &Path,
    guard: BusyGuard,
    mut on_progress: impl FnMut(&str, String),
) -> Result<BinaryFacts, String> {
    let temp = root.join(TEMP_DIR_NAME);
    std::fs::create_dir_all(&temp).map_err(|e| e.to_string())?;

    acquisition::check_cancelled(&guard.cancelled)?;
    on_progress("resolve", "finding the latest build".to_string());
    let resolved = acquisition::resolve_latest(&guard.cancelled)?;

    let partial = temp.join(format!("ffmpeg-{}.partial", nanoid::generate()));
    let staged = temp.join(format!("ffmpeg-{}.staged", nanoid::generate()));
    let _cleanup = acquisition::RemoveFilesOnDrop::new(vec![partial.clone(), staged.clone()]);
    let result = (|| -> Result<BinaryFacts, String> {
        acquisition::check_cancelled(&guard.cancelled)?;
        on_progress(
            "download",
            format!("v{} from {}", resolved.version, resolved.download_url),
        );
        let bytes = acquisition::download_to(
            &resolved.download_url,
            &partial,
            &guard.cancelled,
            None,
            |done| {
                on_progress("download", format!("{} MB", done / 1_048_576));
            },
        )?;
        acquisition::check_cancelled(&guard.cancelled)?;
        on_progress("verify", format!("{bytes} bytes downloaded"));
        let expected = binaries::parse_sums(
            &acquisition::fetch_text(&resolved.sums_url, &guard.cancelled)?,
            &resolved.sums_asset,
        )
        .ok_or_else(|| format!("{} not in the checksum file", resolved.sums_asset))?;
        acquisition::check_cancelled(&guard.cancelled)?;
        let actual = acquisition::file_sha256(&partial, &guard.cancelled)?;
        if actual != expected {
            return Err(format!(
                "checksum mismatch for {}: expected {expected}, got {actual}",
                resolved.sums_asset
            ));
        }

        on_progress("install", "extracting".to_string());
        acquisition::extract_ffmpeg(&partial, &staged, ffmpeg_file_name(), &guard.cancelled)?;
        acquisition::make_runnable(&staged, &guard.cancelled)?;

        let target = ffmpeg_path(root);
        std::fs::create_dir_all(target.parent().unwrap()).map_err(|e| e.to_string())?;
        acquisition::check_cancelled(&guard.cancelled)?;
        // Replace-in-place: rename over any previous install (same volume).
        // not recorded: the installed executable is a re-downloadable binary.
        acquisition::publish_staged(&staged, &target)?;

        // The binary has landed — drop the cached read FIRST, so even a failed
        // sidecar write below cannot leave the pre-install answer cached; a
        // fresh read sees whatever is actually on disk.
        forget_installed_ffmpeg_version();
        // Where the binary cannot report a comparable version, record the
        // resolved one beside it — AFTER the publish, so a failure here leaves
        // a present binary reading version-unknown (which offers a re-acquire)
        // rather than an old binary wearing the new version's label.
        if !ffmpeg_reports_its_own_version() {
            write_version_sidecar(root, &resolved.version)?;
            // The write changed what a read would see; drop again in case a
            // concurrent status read cached between the two.
            forget_installed_ffmpeg_version();
        }

        // Only the upstream fact is persisted. What is now installed is read back
        // from the binary itself.
        let facts = BinaryFacts {
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
    result
}

/// Version check only. Success updates `latestKnownVersion` + the check stamp;
/// failure writes NOTHING so stale knowledge is never dressed up as fresh.
/// BINARIES ONLY: a model's "latest" is the pin compiled into this build, so
/// there is nothing to ask and nothing to stamp — `state_of` derives it and
/// a re-pin shipped in an app update shows up on its own.
pub fn check_entry(root: &Path, id: &str) -> Result<BinaryFacts, String> {
    let spec = spec_of(id).ok_or_else(|| format!("unknown dependency: {id}"))?;
    let guard = claim(id)?;
    let mut facts = load_facts_for(root, id);
    match spec.kind {
        DependencyKind::Binary => {
            let resolved = acquisition::resolve_latest(&guard.cancelled)?;
            facts.latest_known_version = Some(resolved.version);
        }
        // Refused rather than faked: a model has no upstream to ask. Its
        // selected version belongs to this app build, and `state_of` derives it.
        DependencyKind::Model => {
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
    /// True when this entry's "latest" is DISCOVERABLE — a binary resolved
    /// live from upstream. A model's latest is selected by the app build, so
    /// there is nothing to look up and nothing to check.
    pub checkable: bool,
    /// A pinned artifact's upstream publication date — how old this model
    /// actually is. None for binaries, whose live version is the answer.
    pub released: Option<String>,
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
        DependencyKind::Model => {
            let expected = spec.pinned.as_ref().map(|p| p.bytes);
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
            Some(_) => std::fs::metadata(&path)
                .ok()
                .and_then(|metadata| read_model_identity(root, spec, metadata.len())),
            None => installed_ffmpeg_version(root),
        }
    };
    DependencyState {
        id: spec.id.to_string(),
        label: spec.label.to_string(),
        kind: spec.kind,
        status: binaries::derive_status(present, installed_version.as_deref(), &facts),
        path: path.to_string_lossy().to_string(),
        installed_version,
        facts,
        checkable: matches!(spec.kind, DependencyKind::Binary),
        released: spec.pinned.as_ref().map(|p| p.released.to_string()),
    }
}

/// Every registry entry's state, in display order.
pub fn states(root: &Path) -> Vec<DependencyState> {
    DEPENDENCIES.iter().map(|spec| state_of(root, spec)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_is_visible_to_chunked_work_and_released_with_the_claim() {
        let id = "whisper-large-v3-turbo";
        let started = begin_install(id).unwrap();
        assert!(cancel_entry(id));
        assert!(is_cancelled_error(
            &acquisition::check_cancelled(&started.0.cancelled).unwrap_err()
        ));
        drop(started);
        assert!(!cancel_entry(id));
    }

    #[test]
    fn installed_model_status_needs_no_persisted_facts() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-binmgr-model-state-")
            .tempdir()
            .unwrap();
        let spec = spec_of("ultraface-rfb640").unwrap();
        let pinned = spec.pinned.as_ref().unwrap();
        let target = installed_path(dir.path(), spec);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::File::create(&target)
            .unwrap()
            .set_len(pinned.bytes)
            .unwrap();
        write_model_identity(dir.path(), spec, pinned).unwrap();
        let expected_version = pin_version(pinned);

        let model = state_of(dir.path(), spec);

        assert_eq!(model.status, BinaryStatus::UpToDate);
        assert_eq!(
            model.installed_version.as_deref(),
            Some(expected_version.as_str())
        );
        assert_eq!(model.facts.last_checked_at_utc, None);
        assert!(!dir.path().join(DEPENDENCIES_FILE_NAME).exists());
        assert!(model_identity_path(dir.path(), spec).is_file());
    }

    #[test]
    fn a_same_size_older_model_remains_update_available() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-binmgr-old-model-")
            .tempdir()
            .unwrap();
        let spec = spec_of("ultraface-rfb640").unwrap();
        let pinned = spec.pinned.as_ref().unwrap();
        let target = installed_path(dir.path(), spec);
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::File::create(&target)
            .unwrap()
            .set_len(pinned.bytes)
            .unwrap();
        let older = ModelIdentity {
            sha256: "a".repeat(64),
            bytes: pinned.bytes,
        };
        std::fs::write(
            model_identity_path(dir.path(), spec),
            serde_json::to_vec(&older).unwrap(),
        )
        .unwrap();

        let model = state_of(dir.path(), spec);

        assert_eq!(model.status, BinaryStatus::UpdateAvailable);
        assert_eq!(model.installed_version.as_deref(), Some("aaaaaaaaaaaa"));
        assert_eq!(
            model.facts.latest_known_version.as_deref(),
            Some(pin_version(pinned).as_str())
        );
    }

    #[test]
    fn facts_store_self_heals_on_missing_and_corrupt() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-binmgr-")
            .tempdir()
            .unwrap();
        // Missing → defaults.
        assert_eq!(load_facts_for(dir.path(), "ffmpeg"), BinaryFacts::default());
        // Corrupt → defaults, no quarantine (re-derivable facts).
        std::fs::write(dir.path().join(DEPENDENCIES_FILE_NAME), b"{ not json").unwrap();
        assert_eq!(load_facts_for(dir.path(), "ffmpeg"), BinaryFacts::default());
    }

    #[test]
    #[serial_test::serial(backup_store)]
    fn facts_round_trip_through_the_store() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-binmgr-rt-")
            .tempdir()
            .unwrap();
        let facts = BinaryFacts {
            latest_known_version: Some("9.1".into()),
            last_checked_at_utc: Some("2026-08-08T12:00:00.000Z".into()),
        };
        save_facts_for(dir.path(), "ffmpeg", &facts).unwrap();
        assert_eq!(load_facts_for(dir.path(), "ffmpeg"), facts);
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
        let facts = install_entry(dir.path(), "ffmpeg", |phase, detail| {
            eprintln!("[{phase}] {detail}");
        })
        .expect("live install should succeed");
        assert!(facts.latest_known_version.is_some());

        let path = ffmpeg_path(dir.path());
        assert!(path.is_file());
        let output = std::process::Command::new(&path)
            .arg("-version")
            .output()
            .expect("installed ffmpeg should run");
        assert!(output.status.success());
        let banner = String::from_utf8_lossy(&output.stdout);
        assert!(banner.starts_with("ffmpeg version"), "banner: {banner}");

        // The installed version is READ BACK off the binary just published, and
        // it is the version the resolve named — the whole point of the design.
        let state = state_of(dir.path(), spec_of("ffmpeg").unwrap());
        assert_eq!(
            state.installed_version.as_deref(),
            facts.latest_known_version.as_deref(),
        );
        assert_eq!(state.status, BinaryStatus::UpToDate);
    }
}
