//! Production dependency lookup for OneCopy's AI features.

use std::path::{Path, PathBuf};

use crate::binaries::BinaryStatus;
use crate::binaries_manager::{self, DependencyKind};

const FFMPEG: &str = "ffmpeg";
#[cfg(windows)]
const ONNX_RUNTIME: &str = "onnxruntime-win-x64";
const WHISPER: &str = "whisper-large-v3-turbo";
const ULTRAFACE: &str = "ultraface-rfb640";
const HSEMOTION: &str = "hsemotion-enet-b2";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FaceScoringDependencies {
    pub runtime: Option<PathBuf>,
    pub detector: PathBuf,
    pub emotion: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranscriptionDependencies {
    pub ffmpeg: Option<PathBuf>,
    pub model: Option<PathBuf>,
}

fn production_path(root: &Path, id: &str) -> Option<PathBuf> {
    let spec = binaries_manager::spec_of(id)?;
    let path = binaries_manager::installed_path(root, spec);
    let available = match spec.kind {
        // Ordinary media paths accept an installed ffmpeg file and report an
        // invocation problem only if that file cannot run.
        DependencyKind::Binary => path.is_file(),
        DependencyKind::Runtime | DependencyKind::Model => {
            binaries_manager::state_of(root, spec).status != BinaryStatus::NotInstalled
        }
    };
    available.then_some(path)
}

pub fn production_face_scoring(root: &Path) -> Option<FaceScoringDependencies> {
    let detector = production_path(root, ULTRAFACE)?;
    let emotion = production_path(root, HSEMOTION)?;
    #[cfg(windows)]
    let runtime = Some(production_path(root, ONNX_RUNTIME)?);
    #[cfg(not(windows))]
    let runtime = None;
    Some(FaceScoringDependencies {
        runtime,
        detector,
        emotion,
    })
}

/// The installed managed ffmpeg, the one presence rule every media path
/// reads.
pub fn production_ffmpeg(root: &Path) -> Option<PathBuf> {
    production_path(root, FFMPEG)
}

/// Provenance retained by a loaded model instance. Unknown components are
/// explicit; a compiled download pin never identifies unchecked installed bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModelIdentity {
    pub model: String,
    pub version: String,
}

pub(crate) struct ModelOutcome<T> {
    pub model: ModelIdentity,
    pub result: Result<T, String>,
}

fn model_identity(ids: &[&str], versions: &[Option<String>]) -> ModelIdentity {
    ModelIdentity {
        model: ids.join("+"),
        version: versions.iter().map(|version| version.as_deref().unwrap_or("unknown"))
            .collect::<Vec<_>>().join("+"),
    }
}

pub fn transcription_model() -> ModelIdentity {
    transcription_model_with_version(None)
}

pub(crate) fn transcription_model_with_version(version: Option<String>) -> ModelIdentity {
    model_identity(&[WHISPER], &[version])
}

pub fn face_model() -> ModelIdentity {
    face_model_with_versions(None, None)
}

pub(crate) fn face_model_with_versions(detector: Option<String>, emotion: Option<String>) -> ModelIdentity {
    model_identity(&[ULTRAFACE, HSEMOTION], &[detector, emotion])
}

/// Bind acquisition evidence to the file observed at native session creation.
/// This is not a fresh integrity check: unchanged physical-file/version evidence
/// permits using the acquisition digest. Replacements or unbound legacy records
/// remain usable, but their exact version is unknown. Once captured, later
/// artifact updates cannot relabel this model instance's results.
pub(crate) struct ModelObservation {
    path: PathBuf,
    version: Option<crate::file_identity::FileVersion>,
    digest: Option<String>,
}

impl ModelObservation {
    pub(crate) fn before(path: &Path) -> Self {
        let version = crate::file_identity::open_regular_nofollow(path)
            .and_then(|(mut file, _)| crate::file_identity::FileVersion::of(&mut file)).ok();
        Self {
            path: path.to_path_buf(),
            version,
            digest: binaries_manager::verified_model_digest(path),
        }
    }

    pub(crate) fn after_load(self) -> Option<String> {
        if self.version?.matches_path(&self.path).ok()? { self.digest } else { None }
    }
}

pub fn production_transcription(root: &Path) -> TranscriptionDependencies {
    TranscriptionDependencies {
        ffmpeg: production_ffmpeg(root),
        model: production_path(root, WHISPER),
    }
}

// Model loading is a class dependency failure, never a receipt against a media file.
const MODEL_UNAVAILABLE: &str = "model-unavailable: ";
pub fn model_load_error(error: impl std::fmt::Display) -> String {
    format!("{MODEL_UNAVAILABLE}{error}")
}
pub fn is_model_load_error(error: &str) -> bool {
    error.starts_with(MODEL_UNAVAILABLE)
}
pub fn model_error_detail(error: &str) -> &str {
    error.strip_prefix(MODEL_UNAVAILABLE).unwrap_or(error)
}

#[cfg(test)]
#[path = "../tests/unit/ai_dependencies.rs"]
mod tests;
