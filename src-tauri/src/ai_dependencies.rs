//! One dependency boundary for OneCopy's production AI features and its
//! explicit real-artifact preparation. Production keeps accepting the same
//! installed artifacts it accepts today; prepared execution requires every
//! artifact to match the exact identity selected by this build.

use std::path::{Path, PathBuf};

#[cfg(feature = "ai-test-support")]
use sha2::{Digest, Sha256};
#[cfg(feature = "ai-test-support")]
use std::io::Read;

use crate::binaries::BinaryStatus;
#[cfg(feature = "ai-test-support")]
use crate::binaries_manager::DependencySpec;
use crate::binaries_manager::{self, DependencyKind};

const FFMPEG: &str = "ffmpeg";
#[cfg(windows)]
const ONNX_RUNTIME: &str = "onnxruntime-win-x64";
const WHISPER: &str = "whisper-large-v3-turbo";
const ULTRAFACE: &str = "ultraface-rfb640";
const HSEMOTION: &str = "hsemotion-enet-b2";

#[cfg(feature = "ai-test-support")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Requirement {
    FaceScoring,
    Transcription,
}

#[cfg(feature = "ai-test-support")]
impl Requirement {
    pub fn id(self) -> &'static str {
        match self {
            Self::FaceScoring => "face-scoring",
            Self::Transcription => "transcription",
        }
    }

    fn dependency_ids(self) -> &'static [&'static str] {
        match self {
            Self::FaceScoring => FACE_SCORING_DEPENDENCIES,
            Self::Transcription => TRANSCRIPTION_DEPENDENCIES,
        }
    }
}

#[cfg(feature = "ai-test-support")]
impl std::str::FromStr for Requirement {
    type Err = String;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        match value {
            "face-scoring" => Ok(Self::FaceScoring),
            "transcription" => Ok(Self::Transcription),
            _ => Err(format!("unknown AI dependency requirement: {value}")),
        }
    }
}

#[cfg(all(feature = "ai-test-support", windows))]
const FACE_SCORING_DEPENDENCIES: &[&str] = &[ONNX_RUNTIME, ULTRAFACE, HSEMOTION];
#[cfg(all(feature = "ai-test-support", not(windows)))]
const FACE_SCORING_DEPENDENCIES: &[&str] = &[ULTRAFACE, HSEMOTION];
#[cfg(feature = "ai-test-support")]
const TRANSCRIPTION_DEPENDENCIES: &[&str] = &[FFMPEG, WHISPER];

#[cfg(feature = "ai-test-support")]
pub fn dependency_ids(requirements: &[Requirement]) -> Vec<&'static str> {
    let mut ids = Vec::new();
    for requirement in requirements {
        for id in requirement.dependency_ids() {
            if !ids.contains(id) {
                ids.push(*id);
            }
        }
    }
    ids
}

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
        // Keep the existing production lookup: ordinary media paths have
        // historically accepted an installed ffmpeg file and report an
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

pub fn production_transcription(root: &Path) -> TranscriptionDependencies {
    TranscriptionDependencies {
        ffmpeg: production_path(root, FFMPEG),
        model: production_path(root, WHISPER),
    }
}

#[cfg(feature = "ai-test-support")]
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ArtifactReadiness {
    NotInstalled,
    InstalledUnchecked,
    UpdateAvailable,
    IdentityMismatch,
    Current,
}

#[cfg(feature = "ai-test-support")]
impl ArtifactReadiness {
    fn from_status(status: BinaryStatus) -> Self {
        match status {
            BinaryStatus::NotInstalled => Self::NotInstalled,
            BinaryStatus::InstalledUnchecked => Self::InstalledUnchecked,
            BinaryStatus::UpdateAvailable => Self::UpdateAvailable,
            BinaryStatus::UpToDate => Self::Current,
        }
    }

    fn is_current(self) -> bool {
        self == Self::Current
    }
}

#[cfg(feature = "ai-test-support")]
impl std::fmt::Display for ArtifactReadiness {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::NotInstalled => "not-installed",
            Self::InstalledUnchecked => "installed-unchecked",
            Self::UpdateAvailable => "update-available",
            Self::IdentityMismatch => "identity-mismatch",
            Self::Current => "current",
        })
    }
}

#[cfg(feature = "ai-test-support")]
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ArtifactIdentity {
    pub sha256: String,
    pub bytes: u64,
    pub version: Option<String>,
}

#[cfg(feature = "ai-test-support")]
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedArtifact {
    pub id: String,
    pub kind: DependencyKind,
    pub requirements: Vec<Requirement>,
    pub readiness: ArtifactReadiness,
    pub identity: Option<ArtifactIdentity>,
}

#[cfg(feature = "ai-test-support")]
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreparedContext {
    pub requirements: Vec<Requirement>,
    pub artifacts: Vec<PreparedArtifact>,
    pub capabilities: Vec<crate::ai_acceleration::Capability>,
}

#[cfg(feature = "ai-test-support")]
impl PreparedContext {
    pub fn require_current(self) -> Result<Self, String> {
        let unavailable = self
            .artifacts
            .iter()
            .filter(|artifact| !artifact.readiness.is_current())
            .map(|artifact| format!("{} is {}", artifact.id, artifact.readiness))
            .collect::<Vec<_>>();
        if unavailable.is_empty() {
            Ok(self)
        } else {
            Err(format!("preparation required: {}", unavailable.join(", ")))
        }
    }
}

#[cfg(feature = "ai-test-support")]
fn sha256(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|error| {
        format!(
            "could not read prepared artifact {}: {error}",
            path.display()
        )
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|error| {
            format!(
                "could not read prepared artifact {}: {error}",
                path.display()
            )
        })?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(hex::encode(digest.finalize()))
}

#[cfg(feature = "ai-test-support")]
fn requirements_for_artifact(requirements: &[Requirement], id: &str) -> Vec<Requirement> {
    requirements
        .iter()
        .copied()
        .filter(|requirement| requirement.dependency_ids().contains(&id))
        .collect()
}

#[cfg(feature = "ai-test-support")]
fn prepared_artifact(
    root: &Path,
    requirements: &[Requirement],
    spec: &DependencySpec,
) -> Result<PreparedArtifact, String> {
    let state = binaries_manager::state_of(root, spec);
    let mut readiness = ArtifactReadiness::from_status(state.status);
    let identity = if readiness.is_current() {
        let path = binaries_manager::installed_path(root, spec);
        let actual_sha256 = sha256(&path)?;
        let expected_sha256 = spec.pinned.as_ref().map(|pinned| {
            pinned
                .extracted
                .as_ref()
                .map(|extracted| extracted.sha256)
                .unwrap_or(pinned.sha256)
        });
        if expected_sha256.is_some_and(|expected| actual_sha256 != expected) {
            readiness = ArtifactReadiness::IdentityMismatch;
        }
        Some(ArtifactIdentity {
            sha256: actual_sha256,
            bytes: std::fs::metadata(&path)
                .map_err(|error| error.to_string())?
                .len(),
            version: state.installed_version,
        })
    } else {
        None
    };
    Ok(PreparedArtifact {
        id: spec.id.to_string(),
        kind: spec.kind,
        requirements: requirements_for_artifact(requirements, spec.id),
        readiness,
        identity,
    })
}

#[cfg(feature = "ai-test-support")]
pub fn inspect_prepared(
    root: &Path,
    requirements: &[Requirement],
) -> Result<PreparedContext, String> {
    let mut normalized = Vec::new();
    for requirement in requirements {
        if !normalized.contains(requirement) {
            normalized.push(*requirement);
        }
    }
    if normalized.is_empty() {
        return Err("at least one AI dependency requirement is required".to_string());
    }
    let artifacts = dependency_ids(&normalized)
        .into_iter()
        .map(|id| {
            let spec = binaries_manager::spec_of(id)
                .ok_or_else(|| format!("dependency is not available on this platform: {id}"))?;
            prepared_artifact(root, &normalized, spec)
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(PreparedContext {
        requirements: normalized,
        artifacts,
        capabilities: crate::ai_acceleration::capabilities(None)?,
    })
}

#[cfg(feature = "ai-test-support")]
pub fn require_prepared(
    root: &Path,
    requirements: &[Requirement],
) -> Result<PreparedContext, String> {
    inspect_prepared(root, requirements)?.require_current()
}
