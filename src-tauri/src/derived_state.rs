//! Durable result receipts for optional media analysis, plus the narrow
//! recovery policy that may reset those reconstructible results. This is not
//! a job queue: absence means pending, the coordinator remains the only
//! dispatcher, and only fixed, safe classes can be retried from Issues.

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::Serialize;

use crate::preview::CachePaths;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WorkClass {
    Previews,
    Snapshots,
    Similarity,
    Faces,
    VideoTranscripts,
    AudioTranscripts,
}

impl WorkClass {
    pub(crate) const ALL: [Self; 6] = [
        Self::Previews,
        Self::Snapshots,
        Self::Similarity,
        Self::Faces,
        Self::VideoTranscripts,
        Self::AudioTranscripts,
    ];

    pub(crate) fn id(self) -> &'static str {
        match self {
            Self::Previews => "previews",
            Self::Snapshots => "snapshots",
            Self::Similarity => "similarity",
            Self::Faces => "faces",
            Self::VideoTranscripts => "video-transcripts",
            Self::AudioTranscripts => "audio-transcripts",
        }
    }

    pub(crate) fn bit(self) -> u8 {
        1 << (self as u8)
    }

    pub(crate) fn parse(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|class| class.id() == id)
    }

    pub(crate) fn is_transcription(self) -> bool {
        matches!(self, Self::VideoTranscripts | Self::AudioTranscripts)
    }

    pub(crate) fn supported_on_platform(self) -> bool {
        match self {
            Self::Faces => crate::platform_support::FACE_SCORING,
            Self::VideoTranscripts | Self::AudioTranscripts => {
                crate::platform_support::TRANSCRIPTION
            }
            Self::Previews | Self::Snapshots | Self::Similarity => true,
        }
    }

    pub(crate) fn content_kind(self) -> Option<&'static str> {
        match self {
            Self::VideoTranscripts => Some("video"),
            Self::AudioTranscripts => Some("audio"),
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
pub struct WorkCapabilities {
    pub ffmpeg: bool,
    pub video_snapshots_enabled: bool,
    pub similarity_enabled: bool,
    pub face_scoring_supported: bool,
    pub face_enabled: bool,
    pub face_models: bool,
    pub transcription_supported: bool,
    pub transcription_model: bool,
    pub video_transcription_enabled: bool,
    pub audio_transcription_enabled: bool,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct WorkDebt {
    pub runnable: u64,
    pub blocked: u64,
    pub failed: u64,
    pub reason: Option<&'static str>,
    pub disabled: bool,
    pub unavailable: bool,
}

pub(crate) struct WorkDebts([WorkDebt; 6]);

impl WorkDebts {
    pub(crate) fn get(&self, class: WorkClass) -> WorkDebt {
        self.0[class as usize]
    }
}

struct DebtCounts {
    image_previews: u64,
    video_previews: u64,
    waiting_images: u64,
    preview_failures: u64,
    snapshots: u64,
    snapshot_failures: u64,
    faces: u64,
    face_failures: u64,
    video_transcripts: u64,
    video_transcript_failures: u64,
    audio_transcripts: u64,
    audio_transcript_failures: u64,
}

fn work_debt_sql(ffmpeg: bool) -> String {
    let (image_pending, _) = preview_pending_predicates(ffmpeg);
    let video_pending = video_preview_pending_predicate();
    let preview_ready = preview_available_predicate("c");
    format!(
        "SELECT
           COALESCE(SUM(l.kind = 'image' AND {image_pending}), 0),
           COALESCE(SUM(l.kind = 'video' AND {video_pending}), 0),
           COALESCE(SUM(l.kind = 'image' AND c.derived_at_utc = '{NEEDS_FFMPEG}'), 0),
           COALESCE(SUM(l.kind IN ('image', 'video') AND c.derived_at_utc = '{FAILED}'), 0),
           COALESCE(SUM(l.kind = 'video' AND c.strip_frames IS NULL
                        AND c.duration_ms IS NOT NULL
                        AND {preview_ready}), 0),
           COALESCE(SUM(l.kind = 'video' AND c.strip_frames < 0), 0),
           COALESCE(SUM(l.kind = 'image' AND r.face_state IS NULL
                        AND {preview_ready}), 0),
           COALESCE(SUM(r.face_state = '{FAILED}'), 0),
           COALESCE(SUM(c.kind = 'video' AND c.duration_ms IS NOT NULL
                        AND r.transcript_state IS NULL), 0),
           COALESCE(SUM(c.kind = 'video' AND r.transcript_state = '{FAILED}'), 0),
           COALESCE(SUM(c.kind = 'audio' AND r.transcript_state IS NULL), 0),
           COALESCE(SUM(c.kind = 'audio' AND r.transcript_state = '{FAILED}'), 0)
         FROM logical_contents l
         JOIN contents c ON c.hash = l.content_hash
         LEFT JOIN analysis_receipts r ON r.content_hash = l.content_hash"
    )
}

/// Durable output debt for every fixed class. This is the sole owner of the
/// physical sentinel/receipt encoding; runtime and UI projection consume one
/// coherent aggregate over the maintained live-item projection rather than
/// repeatedly probing physical paths or inventing a second queue.
pub(crate) fn work_debts(
    conn: &Connection,
    capabilities: WorkCapabilities,
) -> Result<WorkDebts, String> {
    let sql = work_debt_sql(capabilities.ffmpeg);
    let counts = conn
        .query_row(&sql, [], |row| {
            let count = |column| row.get::<_, i64>(column).map(|value| value.max(0) as u64);
            Ok(DebtCounts {
                image_previews: count(0)?,
                video_previews: count(1)?,
                waiting_images: count(2)?,
                preview_failures: count(3)?,
                snapshots: count(4)?,
                snapshot_failures: count(5)?,
                faces: count(6)?,
                face_failures: count(7)?,
                video_transcripts: count(8)?,
                video_transcript_failures: count(9)?,
                audio_transcripts: count(10)?,
                audio_transcript_failures: count(11)?,
            })
        })
        .map_err(|error| error.to_string())?;

    let previews = if capabilities.ffmpeg {
        WorkDebt {
            runnable: counts.image_previews + counts.video_previews,
            failed: counts.preview_failures,
            ..WorkDebt::default()
        }
    } else {
        let blocked = counts.video_previews + counts.waiting_images;
        WorkDebt {
            runnable: counts.image_previews,
            blocked,
            failed: counts.preview_failures,
            reason: (blocked > 0).then_some("Waiting for ffmpeg"),
            ..WorkDebt::default()
        }
    };
    let snapshots = if !capabilities.video_snapshots_enabled {
        WorkDebt {
            disabled: true,
            reason: Some("Turn on video snapshots in Settings"),
            ..WorkDebt::default()
        }
    } else if capabilities.ffmpeg {
        WorkDebt {
            runnable: counts.snapshots,
            failed: counts.snapshot_failures,
            ..WorkDebt::default()
        }
    } else {
        WorkDebt {
            blocked: counts.snapshots,
            failed: counts.snapshot_failures,
            reason: Some("Waiting for ffmpeg"),
            unavailable: true,
            ..WorkDebt::default()
        }
    };
    let similarity = if capabilities.similarity_enabled {
        WorkDebt {
            runnable: crate::similarity::dirty_bucket_count(conn)?,
            ..WorkDebt::default()
        }
    } else {
        WorkDebt {
            disabled: true,
            reason: Some("Turn on similar-photo analysis in Settings"),
            ..WorkDebt::default()
        }
    };
    let faces = if !capabilities.face_scoring_supported {
        WorkDebt {
            blocked: counts.faces,
            failed: counts.face_failures,
            reason: Some(crate::platform_support::MAC_ONLY_REASON),
            unavailable: true,
            ..WorkDebt::default()
        }
    } else if !capabilities.face_enabled {
        WorkDebt {
            disabled: true,
            reason: Some("Turn on face scoring in Settings"),
            ..WorkDebt::default()
        }
    } else if capabilities.face_models {
        WorkDebt {
            runnable: counts.faces,
            failed: counts.face_failures,
            ..WorkDebt::default()
        }
    } else {
        WorkDebt {
            blocked: counts.faces,
            failed: counts.face_failures,
            reason: Some("Waiting for face models"),
            unavailable: true,
            ..WorkDebt::default()
        }
    };
    let transcript_debt = |enabled: bool, runnable: u64, failed: u64, setting: &'static str| {
        if !capabilities.transcription_supported {
            WorkDebt {
                blocked: runnable,
                failed,
                reason: Some(crate::platform_support::MAC_ONLY_REASON),
                unavailable: true,
                ..WorkDebt::default()
            }
        } else if !enabled {
            WorkDebt {
                disabled: true,
                reason: Some(setting),
                ..WorkDebt::default()
            }
        } else if capabilities.ffmpeg && capabilities.transcription_model {
            WorkDebt {
                runnable,
                failed,
                ..WorkDebt::default()
            }
        } else {
            WorkDebt {
                blocked: runnable,
                failed,
                reason: Some(transcript_unavailable_reason(capabilities)),
                unavailable: true,
                ..WorkDebt::default()
            }
        }
    };
    let video_transcripts = transcript_debt(
        capabilities.video_transcription_enabled,
        counts.video_transcripts,
        counts.video_transcript_failures,
        "Turn on video transcription in Settings",
    );
    let audio_transcripts = transcript_debt(
        capabilities.audio_transcription_enabled,
        counts.audio_transcripts,
        counts.audio_transcript_failures,
        "Turn on audio transcription in Settings",
    );
    Ok(WorkDebts([
        previews,
        snapshots,
        similarity,
        faces,
        video_transcripts,
        audio_transcripts,
    ]))
}

// EXCEPTION (tests-folder convention): this pins the private aggregate SQL
// owned here, so the shipped projection and its structural assertion cannot
// drift into different queries.
#[cfg(test)]
mod debt_query_tests {
    use super::*;

    #[test]
    fn debt_snapshot_scans_one_logical_projection_and_never_probes_paths() {
        let dir = tempfile::Builder::new()
            .prefix("onecopy-debt-plan-")
            .tempdir()
            .unwrap();
        let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
        let mut statement = conn
            .prepare(&format!("EXPLAIN QUERY PLAN {}", work_debt_sql(true)))
            .unwrap();
        let details: Vec<String> = statement
            .query_map([], |row| row.get(3))
            .unwrap()
            .map(Result::unwrap)
            .collect();

        assert_eq!(
            details
                .iter()
                .filter(|line| line.starts_with("SCAN "))
                .count(),
            1,
            "debt projection must aggregate one source scan: {details:?}"
        );
        assert!(
            details.iter().any(|line| line.contains("logical_contents")),
            "debt projection lost the maintained live-item truth: {details:?}"
        );
        assert!(
            details.iter().all(|line| !line.contains("paths")),
            "debt projection regressed to physical-path probes: {details:?}"
        );
    }
}

pub const FACE_ERROR: &str = "face-score-error";
pub const PREVIEW_ERROR: &str = "decode-error";
pub const TRANSCRIPT_ERROR: &str = "transcription-error";
pub const VIDEO_POSTER_ERROR: &str = "video-poster-error";
pub const VIDEO_STRIP_ERROR: &str = "video-strip-error";
pub const RESOURCE_ISSUE_PREFIX: &str = "resource-limit-";

pub const READY: &str = "ready";
pub const READY_TEXT: &str = "ready-text";
pub const READY_EMPTY: &str = "ready-empty";
pub const FAILED: &str = "failed";
pub const NEEDS_FFMPEG: &str = "needs-ffmpeg";
pub const DERIVE_VERSION: i64 = 3;
const STRIP_FAILED: i64 = -1;
pub const SNAPSHOT_CANDIDATE_PAGE_SIZE: usize = 32;
pub const FACE_CANDIDATE_PAGE_SIZE: usize = 32;
pub const TRANSCRIPT_CANDIDATE_PAGE_SIZE: usize = 64;

fn transcript_unavailable_reason(capabilities: WorkCapabilities) -> &'static str {
    if !capabilities.transcription_supported {
        crate::platform_support::MAC_ONLY_REASON
    } else if capabilities.ffmpeg && !capabilities.transcription_model {
        "Waiting for transcription model"
    } else {
        "Waiting for ffmpeg"
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ItemWorkState {
    pub state: &'static str,
    pub has_value: bool,
    pub reason: Option<&'static str>,
    pub done: Option<u64>,
    pub total: Option<u64>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ItemWorkStates {
    pub preview: Option<ItemWorkState>,
    pub snapshots: Option<ItemWorkState>,
    pub similarity: Option<ItemWorkState>,
    pub faces: Option<ItemWorkState>,
    pub transcripts: Option<ItemWorkState>,
}

pub(crate) struct ItemWorkFacts<'a> {
    pub kind: &'a str,
    pub derived_at: Option<&'a str>,
    pub derived_version: i64,
    pub strip_frames: Option<i64>,
    pub duration_ms: Option<i64>,
    pub similar_group_id: Option<i64>,
    pub face_state: Option<&'a str>,
    pub face_score: Option<f64>,
    pub transcript_state: Option<&'a str>,
}

fn item_state(state: &'static str, has_value: bool, reason: Option<&'static str>) -> ItemWorkState {
    ItemWorkState {
        state,
        has_value,
        reason,
        done: None,
        total: None,
    }
}

/// One backend-authored per-item projection over existing receipts and
/// capabilities. It creates no state: runtime overlays the active item later.
pub(crate) fn item_work_states(
    facts: ItemWorkFacts<'_>,
    capabilities: WorkCapabilities,
    similarity_dirty: bool,
) -> ItemWorkStates {
    let media = matches!(facts.kind, "image" | "video");
    let preview = media.then(|| match facts.derived_at {
        Some(FAILED) => item_state("failed", false, Some("Preview generation failed")),
        Some(NEEDS_FFMPEG) if !capabilities.ffmpeg => {
            item_state("unavailable", false, Some("Waiting for ffmpeg"))
        }
        None if facts.kind == "video" && !capabilities.ffmpeg => {
            item_state("unavailable", false, Some("Waiting for ffmpeg"))
        }
        None | Some(NEEDS_FFMPEG) => item_state("pending", false, None),
        Some(_) if facts.derived_version < DERIVE_VERSION => item_state("pending", true, None),
        Some(_) => item_state("ready", true, None),
    });
    let preview_ready = preview.as_ref().is_some_and(|state| state.state == "ready");
    let preview_failed = preview.as_ref().is_some_and(|state| state.state == "failed");

    let snapshots = (facts.kind == "video").then(|| {
        if facts.strip_frames == Some(STRIP_FAILED) {
            item_state("failed", false, Some("Video snapshot generation failed"))
        } else if let Some(count) = facts.strip_frames {
            item_state("ready", count > 0, None)
        } else if !capabilities.video_snapshots_enabled {
            item_state("disabled", false, Some("Video snapshots are off"))
        } else if !capabilities.ffmpeg {
            item_state("unavailable", false, Some("Waiting for ffmpeg"))
        } else if preview_failed {
            item_state("blocked", false, Some("Preview generation failed"))
        } else if !preview_ready || facts.duration_ms.is_none() {
            item_state("waiting", false, Some("Waiting for the video poster"))
        } else {
            item_state("pending", false, None)
        }
    });

    let similarity = (facts.kind == "image").then(|| {
        let has_value = facts.similar_group_id.is_some();
        if !capabilities.similarity_enabled {
            item_state("disabled", has_value, Some("Similar-photo analysis is off"))
        } else if preview_failed {
            item_state("blocked", has_value, Some("Preview generation failed"))
        } else if !preview_ready {
            item_state("waiting", has_value, Some("Waiting for the preview"))
        } else if similarity_dirty {
            item_state("pending", has_value, None)
        } else {
            item_state("ready", has_value, None)
        }
    });

    let faces = (facts.kind == "image").then(|| {
        if facts.face_state == Some(FAILED) {
            item_state("failed", false, Some("Face scoring failed"))
        } else if facts.face_state == Some(READY) {
            item_state(
                "ready",
                facts.face_score.is_some_and(|score| score > 0.0),
                None,
            )
        } else if !capabilities.face_scoring_supported {
            item_state(
                "unavailable",
                false,
                Some(crate::platform_support::MAC_ONLY_REASON),
            )
        } else if !capabilities.face_enabled {
            item_state("disabled", false, Some("Face scoring is off"))
        } else if !capabilities.face_models {
            item_state("unavailable", false, Some("Waiting for face models"))
        } else if preview_failed {
            item_state("blocked", false, Some("Preview generation failed"))
        } else if !preview_ready {
            item_state("waiting", false, Some("Waiting for the preview"))
        } else {
            item_state("pending", false, None)
        }
    });

    let transcripts = matches!(facts.kind, "video" | "audio").then(|| {
        let enabled = if facts.kind == "video" {
            capabilities.video_transcription_enabled
        } else {
            capabilities.audio_transcription_enabled
        };
        if facts.transcript_state == Some(FAILED) {
            item_state("failed", false, Some("Transcription failed"))
        } else if facts.transcript_state == Some(READY_TEXT) {
            item_state("ready", true, None)
        } else if facts.transcript_state == Some(READY_EMPTY) {
            item_state("ready", false, None)
        } else if !capabilities.transcription_supported {
            item_state(
                "unavailable",
                false,
                Some(crate::platform_support::MAC_ONLY_REASON),
            )
        } else if !enabled {
            item_state("disabled", false, Some("Automatic transcription is off"))
        } else if !capabilities.ffmpeg || !capabilities.transcription_model {
            item_state(
                "unavailable",
                false,
                Some(transcript_unavailable_reason(capabilities)),
            )
        } else if facts.kind == "video" && preview_failed {
            item_state("blocked", false, Some("Video poster generation failed"))
        } else if facts.kind == "video" && facts.duration_ms.is_none() {
            item_state("waiting", false, Some("Waiting for the video poster"))
        } else {
            item_state("pending", false, None)
        }
    });

    ItemWorkStates {
        preview,
        snapshots,
        similarity,
        faces,
        transcripts,
    }
}

fn priority_predicate(class: WorkClass, capabilities: WorkCapabilities) -> String {
    let preview_ready = preview_available_predicate("c");
    match class {
        WorkClass::Previews => {
            let (image, video) = preview_pending_predicates(capabilities.ffmpeg);
            format!("((l.kind = 'image' AND {image}) OR (l.kind = 'video' AND {video}))")
        }
        WorkClass::Snapshots if capabilities.video_snapshots_enabled && capabilities.ffmpeg => {
            format!(
                "l.kind = 'video' AND c.strip_frames IS NULL AND c.duration_ms IS NOT NULL \
             AND {preview_ready}"
            )
        }
        WorkClass::Faces
            if capabilities.face_scoring_supported
                && capabilities.face_enabled
                && capabilities.face_models =>
        {
            format!("l.kind = 'image' AND r.face_state IS NULL AND {preview_ready}")
        }
        WorkClass::VideoTranscripts
            if capabilities.transcription_supported
                && capabilities.video_transcription_enabled
                && capabilities.ffmpeg
                && capabilities.transcription_model =>
        {
            "c.kind = 'video' AND c.duration_ms IS NOT NULL AND r.transcript_state IS NULL"
                .to_string()
        }
        WorkClass::AudioTranscripts
            if capabilities.transcription_supported
                && capabilities.audio_transcription_enabled
                && capabilities.ffmpeg
                && capabilities.transcription_model =>
        {
            "c.kind = 'audio' AND r.transcript_state IS NULL".to_string()
        }
        WorkClass::Similarity if capabilities.similarity_enabled => "l.kind = 'image'".to_string(),
        WorkClass::Similarity
        | WorkClass::Snapshots
        | WorkClass::Faces
        | WorkClass::VideoTranscripts
        | WorkClass::AudioTranscripts => "0".to_string(),
    }
}

/// Bounded selected/viewport/section candidates for one fixed class. This is
/// ordering policy over existing output debt, never a persisted queue.
pub(crate) fn priority_candidates(
    conn: &Connection,
    class: WorkClass,
    capabilities: WorkCapabilities,
    selected: Option<&str>,
    visible: &[String],
    section: Option<(&str, Option<i64>, Option<i64>)>,
    limit: usize,
) -> Result<Vec<String>, String> {
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut hinted = Vec::new();
    let mut seen = std::collections::HashSet::new();
    if let Some(hash) = selected {
        if seen.insert(hash) {
            hinted.push(hash);
        }
    }
    for hash in visible {
        if seen.insert(hash.as_str()) {
            hinted.push(hash);
        }
    }
    let predicate = priority_predicate(class, capabilities);
    let mut hashes = Vec::new();
    if !hinted.is_empty() {
        let values = (1..=hinted.len())
            .map(|index| format!("(?{index}, {})", index - 1))
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "WITH hinted(hash, priority) AS (VALUES {values}) \
             SELECT h.hash FROM hinted h \
             JOIN logical_contents l ON l.content_hash = h.hash \
             JOIN contents c ON c.hash = l.content_hash \
             LEFT JOIN analysis_receipts r ON r.content_hash = c.hash \
             WHERE {predicate} ORDER BY h.priority LIMIT {limit}"
        );
        let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
        hashes = statement
            .query_map(params_from_iter(hinted), |row| row.get(0))
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())?;
    }
    let Some((kind, start_ms, end_ms)) = section else {
        return Ok(hashes);
    };
    if hashes.len() >= limit || !matches!(kind, "image" | "video" | "other") {
        return Ok(hashes);
    }
    let time_clause = if start_ms.is_some() {
        "AND l.resolved_utc_ms >= ?2 AND l.resolved_utc_ms < ?3"
    } else {
        "AND l.resolved_utc_ms IS NULL"
    };
    let sql = format!(
        "SELECT l.content_hash FROM logical_contents l \
         JOIN contents c ON c.hash = l.content_hash \
         LEFT JOIN analysis_receipts r ON r.content_hash = c.hash \
         WHERE l.kind = ?1 {time_clause} AND {predicate} \
         ORDER BY l.resolved_utc_ms, l.representative_path_id LIMIT {limit}"
    );
    let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
    let section_hashes: Vec<String> = match (start_ms, end_ms) {
        (Some(start), Some(end)) => statement
            .query_map(params![kind, start, end], |row| row.get(0))
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())?,
        _ => statement
            .query_map([kind], |row| row.get(0))
            .map_err(|error| error.to_string())?
            .collect::<rusqlite::Result<Vec<_>>>()
            .map_err(|error| error.to_string())?,
    };
    let mut seen: std::collections::HashSet<String> = hashes.iter().cloned().collect();
    for hash in section_hashes {
        if seen.insert(hash.clone()) {
            hashes.push(hash);
            if hashes.len() == limit {
                break;
            }
        }
    }
    Ok(hashes)
}

pub(crate) fn preview_pending_predicates(ffmpeg: bool) -> (String, String) {
    let stale = format!(
        "c.derived_version < {DERIVE_VERSION} \
         AND c.derived_at_utc NOT IN ('{FAILED}', '{NEEDS_FFMPEG}')",
    );
    let image = if ffmpeg {
        format!(
            "(c.derived_at_utc IS NULL OR c.derived_at_utc = '{}' OR ({stale}))",
            NEEDS_FFMPEG,
        )
    } else {
        format!("(c.derived_at_utc IS NULL OR ({stale}))")
    };
    let video = if ffmpeg {
        video_preview_pending_predicate()
    } else {
        "0".to_string()
    };
    (image, video)
}

fn video_preview_pending_predicate() -> String {
    format!(
        "(c.derived_at_utc IS NULL OR \
         (c.derived_version < {DERIVE_VERSION} AND c.derived_at_utc != '{FAILED}'))"
    )
}

pub(crate) fn preview_available_predicate(content_alias: &str) -> String {
    format!(
        "({content_alias}.derived_at_utc IS NOT NULL AND \
         {content_alias}.derived_at_utc NOT IN ('{FAILED}', '{NEEDS_FFMPEG}') AND \
         {content_alias}.derived_version >= {DERIVE_VERSION})"
    )
}

pub(crate) fn image_candidates(
    conn: &Connection,
    ffmpeg: bool,
    limit: Option<usize>,
    only_hash: Option<&str>,
) -> Result<Vec<(String, String)>, String> {
    let (pending, _) = preview_pending_predicates(ffmpeg);
    let mut statement = conn
        .prepare(&format!(
            "SELECT l.content_hash, p.abs_path \
             FROM logical_contents l \
             JOIN contents c ON c.hash = l.content_hash \
             JOIN paths p ON p.id = l.representative_path_id \
             WHERE l.kind = 'image' AND {pending} AND p.missing = 0 \
               AND (?1 IS NULL OR l.content_hash = ?1) \
             ORDER BY l.content_hash LIMIT ?2"
        ))
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(
            params![only_hash, limit.map_or(i64::MAX, |value| value as i64)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

pub(crate) fn video_candidates(
    conn: &Connection,
    ffmpeg: bool,
    limit: Option<usize>,
    only_hash: Option<&str>,
) -> Result<Vec<(String, String)>, String> {
    let (_, pending) = preview_pending_predicates(ffmpeg);
    let mut statement = conn
        .prepare(&format!(
            "SELECT l.content_hash, p.abs_path \
             FROM logical_contents l \
             JOIN contents c ON c.hash = l.content_hash \
             JOIN paths p ON p.id = l.representative_path_id \
             WHERE l.kind = 'video' AND {pending} AND p.missing = 0 \
               AND (?1 IS NULL OR l.content_hash = ?1) \
             ORDER BY l.content_hash LIMIT ?2"
        ))
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(
            params![only_hash, limit.map_or(i64::MAX, |value| value as i64)],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

pub fn strip_candidates(
    conn: &Connection,
    after_hash: Option<&str>,
    limit: usize,
) -> Result<Vec<(String, i64, String)>, String> {
    let preview_ready = preview_available_predicate("c");
    let mut statement = conn
        .prepare(&format!(
            "SELECT c.hash, c.duration_ms, p.abs_path \
             FROM logical_contents l \
             JOIN contents c ON c.hash = l.content_hash \
             JOIN paths p ON p.id = l.representative_path_id \
             WHERE l.kind = 'video' AND c.strip_frames IS NULL \
               AND c.duration_ms IS NOT NULL AND {preview_ready} \
               AND l.content_hash > ?1 AND p.missing = 0 \
             ORDER BY l.content_hash LIMIT ?2"
        ))
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![after_hash.unwrap_or(""), limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

pub fn prioritized_strip_candidates(
    conn: &Connection,
    hashes: &[String],
    limit: usize,
) -> Result<Vec<(String, i64, String)>, String> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let values = (1..=hashes.len())
        .map(|index| format!("(?{index}, {})", index - 1))
        .collect::<Vec<_>>()
        .join(", ");
    let preview_ready = preview_available_predicate("c");
    let sql = format!(
        "WITH hinted(hash, priority) AS (VALUES {values}) \
         SELECT c.hash, c.duration_ms, p.abs_path FROM hinted h \
         JOIN logical_contents l ON l.content_hash = h.hash \
         JOIN contents c ON c.hash = l.content_hash \
         JOIN paths p ON p.id = l.representative_path_id \
         WHERE l.kind = 'video' AND c.strip_frames IS NULL \
           AND c.duration_ms IS NOT NULL AND p.missing = 0 AND {preview_ready} \
         ORDER BY h.priority LIMIT {limit}"
    );
    let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params_from_iter(hashes), |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

pub fn face_candidates(
    conn: &Connection,
    after_hash: Option<&str>,
    limit: usize,
) -> Result<Vec<(String, String)>, String> {
    let preview_ready = preview_available_predicate("c");
    let mut statement = conn
        .prepare(&format!(
            "SELECT c.hash, p.abs_path \
             FROM logical_contents l \
             JOIN contents c ON c.hash = l.content_hash \
             JOIN paths p ON p.id = l.representative_path_id \
             LEFT JOIN analysis_receipts r ON r.content_hash = c.hash \
             WHERE l.kind = 'image' AND r.face_state IS NULL \
               AND {preview_ready} \
               AND l.content_hash > ?1 AND p.missing = 0 \
             ORDER BY l.content_hash LIMIT ?2"
        ))
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params![after_hash.unwrap_or(""), limit as i64], |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

pub fn prioritized_face_candidates(
    conn: &Connection,
    hashes: &[String],
    limit: usize,
) -> Result<Vec<(String, String)>, String> {
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let values = (1..=hashes.len())
        .map(|index| format!("(?{index}, {})", index - 1))
        .collect::<Vec<_>>()
        .join(", ");
    let preview_ready = preview_available_predicate("c");
    let sql = format!(
        "WITH hinted(hash, priority) AS (VALUES {values}) \
         SELECT c.hash, p.abs_path FROM hinted h \
         JOIN logical_contents l ON l.content_hash = h.hash \
         JOIN contents c ON c.hash = l.content_hash \
         JOIN paths p ON p.id = l.representative_path_id \
         LEFT JOIN analysis_receipts r ON r.content_hash = c.hash \
         WHERE l.kind = 'image' AND r.face_state IS NULL \
           AND p.missing = 0 AND {preview_ready} \
         ORDER BY h.priority LIMIT {limit}"
    );
    let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params_from_iter(hashes), |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

pub fn transcript_candidates(
    conn: &Connection,
    kind: &str,
    after_hash: Option<&str>,
    limit: usize,
) -> Result<Vec<(String, String)>, String> {
    if !matches!(kind, "video" | "audio") {
        return Err(format!("unsupported transcription kind: {kind}"));
    }
    let mut statement = conn
        .prepare(
            "SELECT c.hash, p.abs_path \
             FROM logical_contents l \
             JOIN contents c ON c.hash = l.content_hash \
             JOIN paths p ON p.id = l.representative_path_id \
             LEFT JOIN analysis_receipts r ON r.content_hash = c.hash \
             WHERE c.kind = ?1 AND (?1 = 'audio' OR c.duration_ms IS NOT NULL) \
               AND r.transcript_state IS NULL AND p.missing = 0 \
               AND l.content_hash > ?2 \
             ORDER BY l.content_hash LIMIT ?3",
        )
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(
            params![kind, after_hash.unwrap_or(""), limit as i64],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

pub fn prioritized_transcript_candidates(
    conn: &Connection,
    kind: &str,
    hashes: &[String],
    limit: usize,
) -> Result<Vec<(String, String)>, String> {
    if !matches!(kind, "video" | "audio") {
        return Err(format!("unsupported transcription kind: {kind}"));
    }
    if hashes.is_empty() {
        return Ok(Vec::new());
    }
    let values = (1..=hashes.len())
        .map(|index| format!("(?{index}, {})", index - 1))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "WITH hinted(hash, priority) AS (VALUES {values}) \
         SELECT c.hash, p.abs_path FROM hinted h \
         JOIN logical_contents l ON l.content_hash = h.hash \
         JOIN contents c ON c.hash = l.content_hash \
         JOIN paths p ON p.id = l.representative_path_id \
         LEFT JOIN analysis_receipts r ON r.content_hash = c.hash \
         WHERE c.kind = '{kind}' AND ('{kind}' = 'audio' OR c.duration_ms IS NOT NULL) \
           AND r.transcript_state IS NULL AND p.missing = 0 \
         ORDER BY h.priority LIMIT {limit}"
    );
    let mut statement = conn.prepare(&sql).map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(params_from_iter(hashes), |row| {
            Ok((row.get(0)?, row.get(1)?))
        })
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    Ok(rows)
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TranscriptResult {
    pub status: &'static str,
    pub text: Option<String>,
    pub message: Option<String>,
}

pub fn transcript_result(
    conn: &Connection,
    cache: &CachePaths,
    hash: &str,
) -> Result<TranscriptResult, String> {
    let state: Option<String> = conn
        .query_row(
            "SELECT transcript_state FROM analysis_receipts WHERE content_hash = ?1",
            [hash],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .flatten();
    match state.as_deref() {
        Some(READY_TEXT | READY_EMPTY) => match std::fs::read_to_string(cache.transcript(hash)) {
            Ok(text) => Ok(TranscriptResult {
                status: READY,
                text: Some(text),
                message: None,
            }),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                conn.execute(
                    "UPDATE analysis_receipts SET transcript_state = NULL,
                         transcript_updated_at_utc = NULL WHERE content_hash = ?1",
                    [hash],
                )
                .map_err(|error| error.to_string())?;
                Ok(TranscriptResult {
                    status: "pending",
                    text: None,
                    message: None,
                })
            }
            Err(error) => Err(format!("transcript cache read failed: {error}")),
        },
        Some(FAILED) => {
            let message = conn
                .query_row(
                    "SELECT i.message FROM issues i JOIN paths p ON p.abs_path = i.path
                     WHERE i.kind = ?1 AND p.content_hash = ?2 LIMIT 1",
                    params![TRANSCRIPT_ERROR, hash],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|error| error.to_string())?
                .flatten();
            Ok(TranscriptResult {
                status: FAILED,
                text: None,
                message,
            })
        }
        _ => match std::fs::read_to_string(cache.transcript(hash)) {
            Ok(text) => {
                let path: String = conn
                    .query_row(
                        "SELECT abs_path FROM paths
                         WHERE content_hash = ?1 AND missing = 0 LIMIT 1",
                        [hash],
                        |row| row.get(0),
                    )
                    .map_err(|error| error.to_string())?;
                record_transcript_success(conn, hash, &path, !text.trim().is_empty())?;
                Ok(TranscriptResult {
                    status: READY,
                    text: Some(text),
                    message: None,
                })
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(TranscriptResult {
                status: "pending",
                text: None,
                message: None,
            }),
            Err(error) => Err(format!("transcript cache read failed: {error}")),
        },
    }
}

pub fn record_preview_success(
    conn: &Connection,
    hash: &str,
    path: &str,
    width: u32,
    height: u32,
    sharpness: f64,
    phash: u64,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            &format!(
                "UPDATE contents SET width = COALESCE(width, ?2), \
                 height = COALESCE(height, ?3), sharpness = ?4, phash = ?5, \
                 derived_at_utc = ?6, derived_version = {DERIVE_VERSION} \
                 WHERE hash = ?1"
            ),
            params![
                hash,
                width,
                height,
                sharpness,
                phash as i64,
                crate::logging::now_iso_millis()
            ],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::clear_issues(&transaction, path, &[PREVIEW_ERROR])?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_preview_blocked(conn: &Connection, hash: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE contents SET derived_at_utc = ?2 WHERE hash = ?1",
        params![hash, NEEDS_FFMPEG],
    )
    .map(|_| ())
    .map_err(|error| error.to_string())
}

fn record_content_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    issue_kind: &str,
    message: &str,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE contents SET derived_at_utc = ?2 WHERE hash = ?1",
            params![hash, FAILED],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::upsert_issue(&transaction, Some(path), issue_kind, message)?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_preview_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    message: &str,
) -> Result<(), String> {
    record_content_failure(conn, hash, path, PREVIEW_ERROR, message)
}

pub fn record_poster_success(
    conn: &Connection,
    hash: &str,
    path: &str,
    duration_ms: u64,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            &format!(
                "UPDATE contents SET duration_ms = COALESCE(duration_ms, ?2), \
                 derived_at_utc = ?3, derived_version = {DERIVE_VERSION} WHERE hash = ?1"
            ),
            params![hash, duration_ms as i64, crate::logging::now_iso_millis()],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::clear_issues(&transaction, path, &[VIDEO_POSTER_ERROR])?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_poster_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    message: &str,
) -> Result<(), String> {
    record_content_failure(conn, hash, path, VIDEO_POSTER_ERROR, message)
}

pub fn record_strip_success(
    conn: &Connection,
    hash: &str,
    path: &str,
    frame_count: u32,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE contents SET strip_frames = ?2 WHERE hash = ?1",
            params![hash, frame_count as i64],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::clear_issues(&transaction, path, &[VIDEO_STRIP_ERROR])?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_strip_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    message: &str,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE contents SET strip_frames = ?2 WHERE hash = ?1",
            params![hash, STRIP_FAILED],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::upsert_issue(&transaction, Some(path), VIDEO_STRIP_ERROR, message)?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_face_success(
    conn: &Connection,
    hash: &str,
    path: &str,
    score: f64,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE contents SET face_score = ?2 WHERE hash = ?1",
            params![hash, score],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO analysis_receipts
               (content_hash, face_state, face_updated_at_utc)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (content_hash) DO UPDATE SET
               face_state = excluded.face_state,
               face_updated_at_utc = excluded.face_updated_at_utc",
            params![hash, READY, crate::logging::now_iso_millis()],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::clear_issues(&transaction, path, &[FACE_ERROR])?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_face_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    message: &str,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE contents SET face_score = NULL WHERE hash = ?1",
            [hash],
        )
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO analysis_receipts
               (content_hash, face_state, face_updated_at_utc)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (content_hash) DO UPDATE SET
               face_state = excluded.face_state,
               face_updated_at_utc = excluded.face_updated_at_utc",
            params![hash, FAILED, crate::logging::now_iso_millis()],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::upsert_issue(&transaction, Some(path), FACE_ERROR, message)?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_transcript_success(
    conn: &Connection,
    hash: &str,
    path: &str,
    has_text: bool,
) -> Result<(), String> {
    let state = if has_text { READY_TEXT } else { READY_EMPTY };
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO analysis_receipts
               (content_hash, transcript_state, transcript_updated_at_utc)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (content_hash) DO UPDATE SET
               transcript_state = excluded.transcript_state,
               transcript_updated_at_utc = excluded.transcript_updated_at_utc",
            params![hash, state, crate::logging::now_iso_millis()],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::clear_issues(&transaction, path, &[TRANSCRIPT_ERROR])?;
    transaction.commit().map_err(|error| error.to_string())
}

pub fn record_transcript_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    message: &str,
) -> Result<(), String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO analysis_receipts
               (content_hash, transcript_state, transcript_updated_at_utc)
             VALUES (?1, ?2, ?3)
             ON CONFLICT (content_hash) DO UPDATE SET
               transcript_state = excluded.transcript_state,
               transcript_updated_at_utc = excluded.transcript_updated_at_utc",
            params![hash, FAILED, crate::logging::now_iso_millis()],
        )
        .map_err(|error| error.to_string())?;
    crate::index_store::upsert_issue(&transaction, Some(path), TRANSCRIPT_ERROR, message)?;
    transaction.commit().map_err(|error| error.to_string())
}

/// A replacement attempt never invalidates the completed transcript it was
/// meant to supersede. Only the new failure is recorded; the ready receipt and
/// old cache remain current until a later replacement succeeds.
pub fn record_transcript_replacement_failure(
    conn: &Connection,
    path: &str,
    message: &str,
) -> Result<(), String> {
    crate::index_store::upsert_issue(conn, Some(path), TRANSCRIPT_ERROR, message)
}

// EXCEPTION (tests-folder convention): this pins a private database-state
// transition without widening the production storage surface for a test.
#[cfg(test)]
mod transcript_replacement_tests {
    use super::*;

    #[test]
    fn replacement_failure_preserves_completed_receipt_and_records_the_attempt() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::index_store::open(&dir.path().join("index.sqlite3")).unwrap();
        conn.execute_batch(
            "INSERT INTO contents (hash, byte_size, kind) VALUES ('media', 10, 'video');
             INSERT INTO paths (abs_path, dir_path, file_name, kind, content_hash)
             VALUES ('/media.mov', '/', 'media.mov', 'video', 'media');
             INSERT INTO analysis_receipts (content_hash, transcript_state)
             VALUES ('media', 'ready-text');",
        )
        .unwrap();

        record_transcript_replacement_failure(&conn, "/media.mov", "replacement failed").unwrap();

        let receipt: String = conn
            .query_row(
                "SELECT transcript_state FROM analysis_receipts WHERE content_hash = 'media'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        let issue: (String, String) = conn
            .query_row(
                "SELECT kind, message FROM issues WHERE path = '/media.mov'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(receipt, READY_TEXT);
        assert_eq!(
            issue,
            (
                TRANSCRIPT_ERROR.to_string(),
                "replacement failed".to_string()
            )
        );
    }
}

fn content_hash_for_issue(
    conn: &Connection,
    issue_id: i64,
) -> Result<Option<(String, String, String)>, String> {
    conn.query_row(
        "SELECT i.kind, i.path, p.content_hash
         FROM issues i
         JOIN paths p ON p.abs_path = i.path AND p.missing = 0
         WHERE i.id = ?1 AND p.content_hash IS NOT NULL
         LIMIT 1",
        [issue_id],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    )
    .optional()
    .map_err(|error| error.to_string())
}

/// Resets only a reconstructible output named by the current issue. The issue
/// remains visible until that output succeeds and clears it.
pub fn retry_issue(conn: &Connection, issue_id: i64) -> Result<bool, String> {
    let Some((kind, _path, hash)) = content_hash_for_issue(conn, issue_id)? else {
        return Ok(false);
    };
    if (kind == FACE_ERROR && !crate::platform_support::FACE_SCORING)
        || (kind == TRANSCRIPT_ERROR && !crate::platform_support::TRANSCRIPTION)
    {
        return Ok(false);
    }
    let changed = match kind.as_str() {
        PREVIEW_ERROR | VIDEO_POSTER_ERROR => conn.execute(
            "UPDATE contents SET derived_at_utc = NULL
             WHERE hash = ?1 AND derived_at_utc IS NOT NULL",
            [&hash],
        ),
        VIDEO_STRIP_ERROR => conn.execute(
            "UPDATE contents SET strip_frames = NULL
             WHERE hash = ?1 AND strip_frames IS NOT NULL",
            [&hash],
        ),
        FACE_ERROR => conn.execute(
            "UPDATE analysis_receipts SET face_state = NULL,
                 face_updated_at_utc = NULL
             WHERE content_hash = ?1 AND face_state IS NOT NULL",
            [&hash],
        ),
        TRANSCRIPT_ERROR => conn.execute(
            "UPDATE analysis_receipts SET transcript_state = NULL,
                 transcript_updated_at_utc = NULL
             WHERE content_hash = ?1 AND transcript_state IS NOT NULL",
            [&hash],
        ),
        _ => return Ok(false),
    }
    .map_err(|error| error.to_string())?;
    Ok(changed > 0)
}

pub fn retry_all(conn: &Connection) -> Result<u64, String> {
    let transaction = conn
        .unchecked_transaction()
        .map_err(|error| error.to_string())?;
    let mut retried = transaction
        .execute(
            "UPDATE contents SET derived_at_utc = NULL \
             WHERE derived_at_utc IS NOT NULL \
               AND EXISTS (SELECT 1 FROM paths p JOIN issues i ON i.path = p.abs_path \
                           WHERE p.content_hash = contents.hash AND p.missing = 0 \
                             AND i.kind IN (?1, ?2))",
            params![PREVIEW_ERROR, VIDEO_POSTER_ERROR],
        )
        .map_err(|error| error.to_string())? as u64;
    retried += transaction
        .execute(
            "UPDATE contents SET strip_frames = NULL \
             WHERE strip_frames IS NOT NULL \
               AND EXISTS (SELECT 1 FROM paths p JOIN issues i ON i.path = p.abs_path \
                           WHERE p.content_hash = contents.hash AND p.missing = 0 \
                             AND i.kind = ?1)",
            [VIDEO_STRIP_ERROR],
        )
        .map_err(|error| error.to_string())? as u64;
    if crate::platform_support::FACE_SCORING {
        retried += transaction
            .execute(
            "UPDATE analysis_receipts SET face_state = NULL, face_updated_at_utc = NULL \
             WHERE face_state IS NOT NULL \
               AND EXISTS (SELECT 1 FROM paths p JOIN issues i ON i.path = p.abs_path \
                           WHERE p.content_hash = analysis_receipts.content_hash \
                             AND p.missing = 0 AND i.kind = ?1)",
                [FACE_ERROR],
            )
            .map_err(|error| error.to_string())? as u64;
    }
    if crate::platform_support::TRANSCRIPTION {
        retried += transaction
            .execute(
            "UPDATE analysis_receipts \
             SET transcript_state = NULL, transcript_updated_at_utc = NULL \
             WHERE transcript_state IS NOT NULL \
               AND EXISTS (SELECT 1 FROM paths p JOIN issues i ON i.path = p.abs_path \
                           WHERE p.content_hash = analysis_receipts.content_hash \
                             AND p.missing = 0 AND i.kind = ?1)",
                [TRANSCRIPT_ERROR],
            )
            .map_err(|error| error.to_string())? as u64;
    }
    transaction.commit().map_err(|error| error.to_string())?;
    Ok(retried)
}

pub fn issue_recovery(
    conn: &Connection,
    issue_id: i64,
) -> Result<Option<crate::issue_recovery::IssueRecovery>, String> {
    if resource_class_for_issue(conn, issue_id)?.is_some_and(WorkClass::supported_on_platform) {
        return Ok(Some(crate::issue_recovery::IssueRecovery {
            action: "retry",
            label: "Resume",
            status: "available",
        }));
    }
    let Some((kind, _path, hash)) = content_hash_for_issue(conn, issue_id)? else {
        return Ok(None);
    };
    if (kind == FACE_ERROR && !crate::platform_support::FACE_SCORING)
        || (kind == TRANSCRIPT_ERROR && !crate::platform_support::TRANSCRIPTION)
    {
        return Ok(None);
    }
    let queued = match kind.as_str() {
        PREVIEW_ERROR | VIDEO_POSTER_ERROR => conn
            .query_row(
                "SELECT derived_at_utc IS NULL FROM contents WHERE hash = ?1",
                [&hash],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?,
        VIDEO_STRIP_ERROR => conn
            .query_row(
                "SELECT strip_frames IS NULL FROM contents WHERE hash = ?1",
                [&hash],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?,
        FACE_ERROR => conn
            .query_row(
                "SELECT face_state IS NULL FROM analysis_receipts WHERE content_hash = ?1",
                [&hash],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?,
        TRANSCRIPT_ERROR => conn
            .query_row(
                "SELECT transcript_state IS NULL FROM analysis_receipts WHERE content_hash = ?1",
                [&hash],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map_err(|error| error.to_string())?,
        _ => return Ok(None),
    };
    Ok(queued.map(|queued| crate::issue_recovery::IssueRecovery {
        action: "retry",
        label: "Retry",
        status: if queued { "queued" } else { "available" },
    }))
}

pub(crate) fn resource_class_for_issue(
    conn: &Connection,
    issue_id: i64,
) -> Result<Option<WorkClass>, String> {
    let kind: Option<String> = conn
        .query_row("SELECT kind FROM issues WHERE id = ?1", [issue_id], |row| {
            row.get(0)
        })
        .optional()
        .map_err(|error| error.to_string())?;
    Ok(kind
        .as_deref()
        .and_then(|kind| kind.strip_prefix(RESOURCE_ISSUE_PREFIX))
        .and_then(WorkClass::parse))
}

pub(crate) fn take_resource_issue(
    conn: &Connection,
    issue_id: i64,
) -> Result<Option<WorkClass>, String> {
    let class = resource_class_for_issue(conn, issue_id)?
        .filter(|class| class.supported_on_platform());
    if class.is_some() {
        conn.execute("DELETE FROM issues WHERE id = ?1", [issue_id])
            .map_err(|error| error.to_string())?;
    }
    Ok(class)
}

pub(crate) fn take_all_resource_issues(conn: &Connection) -> Result<Vec<WorkClass>, String> {
    let mut statement = conn
        .prepare("SELECT DISTINCT kind FROM issues WHERE kind LIKE 'resource-limit-%'")
        .map_err(|error| error.to_string())?;
    let kinds = statement
        .query_map([], |row| row.get::<_, String>(0))
        .map_err(|error| error.to_string())?
        .collect::<rusqlite::Result<Vec<_>>>()
        .map_err(|error| error.to_string())?;
    let classes = kinds
        .iter()
        .map(|kind| {
            kind.strip_prefix(RESOURCE_ISSUE_PREFIX)
                .and_then(WorkClass::parse)
                .ok_or_else(|| format!("unknown resource issue kind: {kind}"))
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .filter(|class| class.supported_on_platform())
        .collect::<Vec<_>>();
    drop(statement);
    for class in &classes {
        conn.execute(
            "DELETE FROM issues WHERE kind = ?1",
            [format!("{RESOURCE_ISSUE_PREFIX}{}", class.id())],
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(classes)
}
