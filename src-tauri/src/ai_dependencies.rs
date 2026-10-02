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

/// Which model produced a result, and which exact file of it: the managed
/// dependency's id and the digest of its pinned artifact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelIdentity {
    pub model: &'static str,
    pub version: &'static str,
}

fn model_identity(id: &'static str) -> ModelIdentity {
    let version = binaries_manager::spec_of(id)
        .and_then(|spec| spec.pinned.as_ref())
        .map_or("", |pinned| pinned.sha256);
    ModelIdentity { model: id, version }
}

pub fn transcription_model() -> ModelIdentity {
    model_identity(WHISPER)
}

pub fn production_transcription(root: &Path) -> TranscriptionDependencies {
    TranscriptionDependencies {
        ffmpeg: production_ffmpeg(root),
        model: production_path(root, WHISPER),
    }
}
