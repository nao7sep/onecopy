//! Durable results of optional media analysis, plus the narrow recovery
//! policy that reopens failed ones. This is not a job queue: absence means
//! pending, the coordinator remains the only dispatcher, and only fixed, safe
//! classes can be retried from Issues.

use rusqlite::{params, params_from_iter, Connection, OptionalExtension};
use serde::Serialize;
use serde_json::json;

use crate::transcription::Transcript;

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

    pub(crate) fn content_kind(self) -> Option<&'static str> {
        match self {
            Self::VideoTranscripts => Some("video"),
            Self::AudioTranscripts => Some("audio"),
            _ => None,
        }
    }

    /// The transcription class for a content kind, the inverse of
    /// [`Self::content_kind`].
    pub(crate) fn transcription_for_kind(kind: &str) -> Option<Self> {
        [Self::VideoTranscripts, Self::AudioTranscripts]
            .into_iter()
            .find(|class| class.content_kind() == Some(kind))
    }
}

/// Whether a class may run under the current settings and tools, and when
/// not, the reason code every surface shows (Background Work, item states,
/// the coordinator's own gate).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClassGate {
    Runnable,
    /// Turned off in Settings.
    Disabled(&'static str),
    /// On, but a tool or model is missing or the saved acceleration is not
    /// offered here.
    Unavailable(&'static str),
}

/// What an item needs before a class can produce its output. The SQL form
/// selects candidates and counts debt; the fact form projects one item's
/// state, so the two cannot disagree about when work may start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Prerequisite {
    None,
    /// The item's preview is ready.
    Preview,
    /// The video's poster is ready and its duration is known.
    PosterAndDuration,
    /// The video's duration is known, from metadata or its poster, or its
    /// poster is ready: a playable container that reports no duration still
    /// has a transcribable soundtrack.
    DurationOrPoster,
}

impl Prerequisite {
    /// SQL over contents alias `c`.
    fn sql(self) -> String {
        let ready = preview_available_predicate("c");
        match self {
            Self::None => "1".to_string(),
            Self::Preview => ready,
            Self::PosterAndDuration => format!("(c.duration_ms IS NOT NULL AND {ready})"),
            Self::DurationOrPoster => format!("(c.duration_ms IS NOT NULL OR {ready})"),
        }
    }

    pub(crate) fn holds(self, preview_ready: bool, duration_known: bool) -> bool {
        match self {
            Self::None => true,
            Self::Preview => preview_ready,
            Self::PosterAndDuration => preview_ready && duration_known,
            Self::DurationOrPoster => duration_known || preview_ready,
        }
    }

    /// The item state while the prerequisite does not hold.
    fn unmet(self, preview_failed: bool, has_value: bool) -> ItemWorkState {
        let (blocked, waiting) = match self {
            Self::Preview => ("Preview generation failed", "Waiting for the preview"),
            _ => ("Video poster generation failed", "Waiting for the video poster"),
        };
        if preview_failed {
            item_state("blocked", has_value, Some(blocked))
        } else {
            item_state("waiting", has_value, Some(waiting))
        }
    }
}

impl WorkClass {
    /// The one per-class gate.
    pub(crate) fn gate(self, capabilities: WorkCapabilities) -> ClassGate {
        let transcription = |enabled: bool, setting: &'static str| {
            if !enabled {
                ClassGate::Disabled(setting)
            } else if !capabilities.transcription_runnable() {
                ClassGate::Unavailable(transcript_unavailable_reason(capabilities))
            } else {
                ClassGate::Runnable
            }
        };
        match self {
            // Previews always run; formats that need ffmpeg wait for it per
            // item (see `preview_pending_predicates`).
            Self::Previews => ClassGate::Runnable,
            Self::Snapshots if !capabilities.video_snapshots_enabled => {
                ClassGate::Disabled("enable-video-snapshots")
            }
            Self::Snapshots if !capabilities.ffmpeg => ClassGate::Unavailable("waiting-for-ffmpeg"),
            Self::Snapshots => ClassGate::Runnable,
            Self::Similarity if !capabilities.similarity_enabled => {
                ClassGate::Disabled("enable-similarity")
            }
            Self::Similarity => ClassGate::Runnable,
            Self::Faces if !capabilities.face_enabled => ClassGate::Disabled("enable-face-scoring"),
            Self::Faces if !capabilities.faces_runnable() => {
                ClassGate::Unavailable(face_unavailable_reason(capabilities))
            }
            Self::Faces => ClassGate::Runnable,
            Self::VideoTranscripts => transcription(
                capabilities.video_transcription_enabled,
                "enable-video-transcription",
            ),
            Self::AudioTranscripts => transcription(
                capabilities.audio_transcription_enabled,
                "enable-audio-transcription",
            ),
        }
    }

    pub(crate) fn prerequisite(self) -> Prerequisite {
        match self {
            Self::Previews | Self::AudioTranscripts => Prerequisite::None,
            Self::Similarity | Self::Faces => Prerequisite::Preview,
            Self::Snapshots => Prerequisite::PosterAndDuration,
            Self::VideoTranscripts => Prerequisite::DurationOrPoster,
        }
    }

    /// SQL (aliases `l` review_contents, `c` contents) selecting items whose
    /// output this class still owes and whose prerequisite holds, whatever
    /// the gate says. Previews and similarity have their own debt units
    /// (`preview_pending_predicates`, dirty buckets) and are not listed here.
    pub(crate) fn owed_sql(self) -> Option<String> {
        let ready = self.prerequisite().sql();
        let faces = face_state_sql("c");
        let transcript = transcript_state_sql("c");
        match self {
            Self::Snapshots => Some(format!("(l.kind = 'video' AND c.strip_frames IS NULL AND {ready})")),
            Self::Faces => Some(format!("(l.kind = 'image' AND {faces} IS NULL AND {ready})")),
            Self::VideoTranscripts => Some(format!(
                "(c.kind = 'video' AND {ready} AND {transcript} IS NULL)"
            )),
            Self::AudioTranscripts => Some(format!(
                "(c.kind = 'audio' AND {ready} AND {transcript} IS NULL)"
            )),
            Self::Previews | Self::Similarity => None,
        }
    }

    /// SQL (same aliases) selecting items whose output for this class failed.
    fn failed_sql(self) -> Option<String> {
        let faces = face_state_sql("c");
        let transcript = transcript_state_sql("c");
        match self {
            Self::Previews => Some(format!("(l.kind IN ('image', 'video') AND c.derive_outcome = '{FAILED}')")),
            Self::Snapshots => Some("(l.kind = 'video' AND c.strip_frames < 0)".to_string()),
            Self::Faces => Some(format!("(l.kind = 'image' AND {faces} = '{FAILED}')")),
            Self::VideoTranscripts => Some(format!("(c.kind = 'video' AND {transcript} = '{FAILED}')")),
            Self::AudioTranscripts => Some(format!("(c.kind = 'audio' AND {transcript} = '{FAILED}')")),
            Self::Similarity => None,
        }
    }
}

/// The analysis classes whose failures are records (`records::analysis_events`).
const FACES_CLASS: &str = "faces";
const TRANSCRIPTS_CLASS: &str = "transcripts";

/// SQL over a contents alias: whether this launch's latest analysis event
/// for the content and class is a failure by the current model. A failure is
/// not retried in the launch that recorded it until a recheck reopens it.
fn failed_now_sql(alias: &str, class: &str, model: &crate::ai_dependencies::ModelIdentity) -> String {
    let session = crate::records::session_sql();
    format!(
        "EXISTS (SELECT 1 FROM records.analysis_events e \
         WHERE e.id = (SELECT MAX(x.id) FROM records.analysis_events x \
           WHERE x.content_hash = {alias}.hash AND x.class = '{class}' AND x.session_id IS {session}) \
         AND e.event = '{FAILED}' AND e.model = {} AND e.model_version = {})",
        crate::records::sql_text(Some(&model.model)),
        crate::records::sql_text(Some(&model.version)),
    )
}

/// SQL over a contents alias: `ready` once a face check exists, `failed` per
/// [`failed_now_sql`], else NULL (pending). A completed result counts as
/// ready whatever model produced it, so a newer model never redoes it.
pub fn face_state_sql(alias: &str) -> String {
    format!(
        "(CASE WHEN EXISTS (SELECT 1 FROM face_checks k WHERE k.content_hash = {alias}.hash) THEN '{READY}' \
         WHEN {} THEN '{FAILED}' END)",
        failed_now_sql(alias, FACES_CLASS, &crate::ai_dependencies::face_model())
    )
}

/// SQL over a contents alias: the content's face score, NULL when unchecked.
pub fn face_score_sql(alias: &str) -> String {
    format!("(SELECT fs.score FROM face_scores fs WHERE fs.content_hash = {alias}.hash)")
}

/// SQL over a contents alias: `ready-text` or `ready-empty` once a transcript
/// exists, `failed` per [`failed_now_sql`], else NULL (pending).
pub fn transcript_state_sql(alias: &str) -> String {
    format!(
        "(CASE (SELECT TRIM(t.text) <> '' FROM transcripts t WHERE t.content_hash = {alias}.hash) \
         WHEN 1 THEN '{READY_TEXT}' WHEN 0 THEN '{READY_EMPTY}' \
         ELSE CASE WHEN {} THEN '{FAILED}' END END)",
        failed_now_sql(alias, TRANSCRIPTS_CLASS, &crate::ai_dependencies::transcription_model())
    )
}

#[derive(Clone, Copy)]
pub struct WorkCapabilities {
    pub ffmpeg: bool,
    pub video_snapshots_enabled: bool,
    pub similarity_enabled: bool,
    pub face_enabled: bool,
    pub face_models: bool,
    pub transcription_model: bool,
    /// Whether each engine's saved acceleration is one this binary offers.
    /// An unsupported value is a configuration failure of that engine only.
    pub transcription_acceleration: bool,
    pub face_acceleration: bool,
    pub video_transcription_enabled: bool,
    pub audio_transcription_enabled: bool,
}

impl WorkCapabilities {
    fn faces_runnable(self) -> bool {
        self.face_models && self.face_acceleration
    }

    fn transcription_runnable(self) -> bool {
        self.ffmpeg && self.transcription_model && self.transcription_acceleration
    }
}

pub const UNSUPPORTED_ACCELERATION: &str = "unsupported-acceleration";

// A saved backend this binary does not offer is named first: installing a
// missing tool would not make that engine runnable.
fn face_unavailable_reason(capabilities: WorkCapabilities) -> &'static str {
    if !capabilities.face_acceleration {
        UNSUPPORTED_ACCELERATION
    } else {
        "waiting-for-face-models"
    }
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

/// The classes whose owed and failed outputs are counted row by row.
const COUNTED: [WorkClass; 4] = [
    WorkClass::Snapshots,
    WorkClass::Faces,
    WorkClass::VideoTranscripts,
    WorkClass::AudioTranscripts,
];

fn work_debt_sql(ffmpeg: bool) -> String {
    let (image_pending, _) = preview_pending_predicates(ffmpeg);
    let (_, video_pending) = preview_pending_predicates(true);
    let mut columns = vec![
        format!("COALESCE(SUM(l.kind = 'image' AND {image_pending}), 0)"),
        format!("COALESCE(SUM(l.kind = 'video' AND {video_pending}), 0)"),
        format!("COALESCE(SUM(l.kind = 'image' AND c.derive_outcome = '{NEEDS_FFMPEG}'), 0)"),
        format!(
            "COALESCE(SUM({}), 0)",
            WorkClass::Previews.failed_sql().expect("previews record failures")
        ),
    ];
    for class in COUNTED {
        columns.push(format!(
            "COALESCE(SUM({}), 0)",
            class.owed_sql().expect("counted classes owe rows")
        ));
        columns.push(format!(
            "COALESCE(SUM({}), 0)",
            class.failed_sql().expect("counted classes record failures")
        ));
    }
    format!(
        "SELECT {}
         FROM review_contents l
         JOIN contents c ON c.hash = l.content_hash",
        columns.join(",\n           ")
    )
}

/// One gated class's debt from its owed and failed counts.
fn gated_debt(gate: ClassGate, owed: u64, failed: u64) -> WorkDebt {
    match gate {
        ClassGate::Disabled(reason) => WorkDebt {
            disabled: true,
            reason: Some(reason),
            ..WorkDebt::default()
        },
        ClassGate::Unavailable(reason) => WorkDebt {
            blocked: owed,
            failed,
            reason: Some(reason),
            unavailable: true,
            ..WorkDebt::default()
        },
        ClassGate::Runnable => WorkDebt {
            runnable: owed,
            failed,
            ..WorkDebt::default()
        },
    }
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
    let counts: Vec<u64> = conn
        .query_row(&sql, [], |row| {
            (0..4 + 2 * COUNTED.len())
                .map(|column| row.get::<_, i64>(column).map(|value| value.max(0) as u64))
                .collect()
        })
        .map_err(|error| error.to_string())?;
    let (image_previews, video_previews, waiting_images, preview_failures) =
        (counts[0], counts[1], counts[2], counts[3]);

    let previews = if capabilities.ffmpeg {
        WorkDebt {
            runnable: image_previews + video_previews,
            failed: preview_failures,
            ..WorkDebt::default()
        }
    } else {
        let blocked = video_previews + waiting_images;
        WorkDebt {
            runnable: image_previews,
            blocked,
            failed: preview_failures,
            reason: (blocked > 0).then_some("waiting-for-ffmpeg"),
            ..WorkDebt::default()
        }
    };
    let similarity = match WorkClass::Similarity.gate(capabilities) {
        ClassGate::Runnable => WorkDebt {
            runnable: crate::similarity::dirty_bucket_count(conn)?,
            ..WorkDebt::default()
        },
        gate => gated_debt(gate, 0, 0),
    };
    let mut debts = [WorkDebt::default(); 6];
    debts[WorkClass::Previews as usize] = previews;
    debts[WorkClass::Similarity as usize] = similarity;
    for (index, class) in COUNTED.into_iter().enumerate() {
        debts[class as usize] = gated_debt(
            class.gate(capabilities),
            counts[4 + 2 * index],
            counts[5 + 2 * index],
        );
    }
    Ok(WorkDebts(debts))
}

// EXCEPTION (tests-folder convention): this pins the private aggregate SQL
// owned here, so the shipped projection and its structural assertion cannot
// drift into different queries.
#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises the private
// `work_debt_sql` query builder; promoting it would widen the crate's API
// only for this test.
#[path = "../tests/unit/derived_state/debt_query_tests.rs"]
mod debt_query_tests;

#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises the private per-class
// gate, prerequisite and owed SQL together, so their SQL and item-state
// forms are proven to agree; promoting them would widen the crate's API only
// for this test.
#[path = "../tests/unit/derived_state/class_rule_tests.rs"]
mod class_rule_tests;

pub const FACE_ERROR: &str = "face-score-error";
pub const PREVIEW_ERROR: &str = "decode-error";
pub const TRANSCRIPT_ERROR: &str = "transcription-error";
pub const VIDEO_POSTER_ERROR: &str = "video-poster-error";
pub const VIDEO_STRIP_ERROR: &str = "video-strip-error";

pub const READY: &str = "ready";
pub const READY_TEXT: &str = "ready-text";
pub const READY_EMPTY: &str = "ready-empty";
pub const FAILED: &str = "failed";
pub const NEEDS_FFMPEG: &str = "needs-ffmpeg";
pub use crate::formats::DERIVE_VERSION;
const STRIP_FAILED: i64 = -1;
pub const SNAPSHOT_CANDIDATE_PAGE_SIZE: usize = 32;
pub const FACE_CANDIDATE_PAGE_SIZE: usize = 32;
pub const TRANSCRIPT_CANDIDATE_PAGE_SIZE: usize = 64;

fn transcript_unavailable_reason(capabilities: WorkCapabilities) -> &'static str {
    if !capabilities.transcription_acceleration {
        UNSUPPORTED_ACCELERATION
    } else if capabilities.ffmpeg && !capabilities.transcription_model {
        "waiting-for-transcription-model"
    } else {
        "waiting-for-ffmpeg"
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
    pub derive_outcome: Option<&'a str>,
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

/// One backend-authored per-item projection over existing results and
/// capabilities. It creates no state: runtime overlays the active item later.
pub(crate) fn item_work_states(
    facts: ItemWorkFacts<'_>,
    capabilities: WorkCapabilities,
    similarity_dirty: bool,
) -> ItemWorkStates {
    let media = matches!(facts.kind, "image" | "video");
    let waiting_for_ffmpeg = || item_state("unavailable", false, Some("waiting-for-ffmpeg"));
    let preview = media.then(|| match (facts.derive_outcome, facts.derived_at) {
        (Some(FAILED), _) => item_state("failed", false, Some("Preview generation failed")),
        (Some(_), _) if !capabilities.ffmpeg => waiting_for_ffmpeg(),
        (None, None) if facts.kind == "video" && !capabilities.ffmpeg => waiting_for_ffmpeg(),
        (Some(_), _) | (None, None) => item_state("pending", false, None),
        (None, Some(_)) if facts.derived_version < DERIVE_VERSION => item_state("pending", true, None),
        (None, Some(_)) => item_state("ready", true, None),
    });
    let preview_ready = preview.as_ref().is_some_and(|state| state.state == "ready");
    let preview_failed = preview.as_ref().is_some_and(|state| state.state == "failed");

    let duration_known = facts.duration_ms.is_some();
    // Receipts first; then the class gate; then the class prerequisite.
    let gated = |class: WorkClass, has_value: bool| match class.gate(capabilities) {
        ClassGate::Disabled(reason) => item_state("disabled", has_value, Some(reason)),
        ClassGate::Unavailable(reason) => item_state("unavailable", has_value, Some(reason)),
        ClassGate::Runnable if !class.prerequisite().holds(preview_ready, duration_known) => {
            class.prerequisite().unmet(preview_failed, has_value)
        }
        ClassGate::Runnable => item_state("pending", has_value, None),
    };

    let snapshots = (facts.kind == "video").then(|| {
        if facts.strip_frames == Some(STRIP_FAILED) {
            item_state("failed", false, Some("Video snapshot generation failed"))
        } else if let Some(count) = facts.strip_frames {
            item_state("ready", count > 0, None)
        } else {
            gated(WorkClass::Snapshots, false)
        }
    });

    let similarity = (facts.kind == "image").then(|| {
        let has_value = facts.similar_group_id.is_some();
        match gated(WorkClass::Similarity, has_value) {
            pending if pending.state == "pending" && !similarity_dirty => {
                item_state("ready", has_value, None)
            }
            state => state,
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
        } else {
            gated(WorkClass::Faces, false)
        }
    });

    let transcripts = WorkClass::transcription_for_kind(facts.kind).map(|class| {
        if facts.transcript_state == Some(FAILED) {
            item_state("failed", false, Some("Transcription failed"))
        } else if facts.transcript_state == Some(READY_TEXT) {
            item_state("ready", true, None)
        } else if facts.transcript_state == Some(READY_EMPTY) {
            item_state("ready", false, None)
        } else {
            gated(class, false)
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
    match class {
        WorkClass::Previews => {
            let (image, video) = preview_pending_predicates(capabilities.ffmpeg);
            format!("((l.kind = 'image' AND {image}) OR (l.kind = 'video' AND {video}))")
        }
        _ if class.gate(capabilities) != ClassGate::Runnable => "0".to_string(),
        WorkClass::Similarity => format!(
            "l.kind = 'image' AND {}",
            crate::similarity::pending_bucket_predicate("l")
        ),
        _ => class.owed_sql().expect("gated classes owe rows"),
    }
}

pub(crate) fn pending_work_predicate(capabilities: WorkCapabilities, classes: impl Iterator<Item = WorkClass>) -> String {
    let predicates = classes.map(|class| format!("({})", priority_predicate(class, capabilities))).collect::<Vec<_>>();
    if predicates.is_empty() { "0".to_string() } else { predicates.join(" OR ") }
}

/// Bounded selected/viewport/section candidates for one fixed class. This is
/// ordering policy over existing output debt, never a persisted queue.
pub(crate) fn priority_candidates(
    conn: &Connection,
    class: WorkClass,
    capabilities: WorkCapabilities,
    selected: Option<&str>,
    visible: &[String],
    section: Option<(crate::queries::SectionKind, Option<i64>, Option<i64>)>,
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
             JOIN review_contents l ON l.content_hash = h.hash \
             JOIN contents c ON c.hash = l.content_hash \
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
    if hashes.len() >= limit {
        return Ok(hashes);
    }
    let kind = kind.as_str();
    let time_clause = if start_ms.is_some() {
        "AND l.resolved_utc_ms >= ?2 AND l.resolved_utc_ms < ?3"
    } else {
        "AND l.resolved_utc_ms IS NULL"
    };
    let sql = format!(
        "SELECT l.content_hash FROM review_contents l \
         JOIN contents c ON c.hash = l.content_hash \
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

/// SQL over the `c` alias: the latest attempt left no outcome holding the
/// item back, and no current-version output exists yet.
fn preview_owed() -> String {
    format!(
        "(c.derive_outcome IS NULL AND \
         (c.derived_at_utc IS NULL OR c.derived_version < {DERIVE_VERSION}))"
    )
}

pub(crate) fn preview_pending_predicates(ffmpeg: bool) -> (String, String) {
    let owed = preview_owed();
    if ffmpeg {
        let waiting = format!("({owed} OR c.derive_outcome = '{NEEDS_FFMPEG}')");
        (waiting.clone(), waiting)
    } else {
        (owed, "0".to_string())
    }
}

pub(crate) fn preview_available_predicate(content_alias: &str) -> String {
    format!(
        "({content_alias}.derived_at_utc IS NOT NULL AND \
         {content_alias}.derive_outcome IS NULL AND \
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
             FROM review_contents l \
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
             FROM review_contents l \
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
    let owed = WorkClass::Snapshots.owed_sql().expect("snapshots owe rows");
    let mut statement = conn
        .prepare(&format!(
            "SELECT c.hash, c.duration_ms, p.abs_path \
             FROM review_contents l \
             JOIN contents c ON c.hash = l.content_hash \
             JOIN paths p ON p.id = l.representative_path_id \
             WHERE {owed} \
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
    let owed = WorkClass::Snapshots.owed_sql().expect("snapshots owe rows");
    let sql = format!(
        "WITH hinted(hash, priority) AS (VALUES {values}) \
         SELECT c.hash, c.duration_ms, p.abs_path FROM hinted h \
         JOIN review_contents l ON l.content_hash = h.hash \
         JOIN contents c ON c.hash = l.content_hash \
         JOIN paths p ON p.id = l.representative_path_id \
         WHERE {owed} AND p.missing = 0 \
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
    let owed = WorkClass::Faces.owed_sql().expect("faces owe rows");
    let mut statement = conn
        .prepare(&format!(
            "SELECT c.hash, p.abs_path \
             FROM review_contents l \
             JOIN contents c ON c.hash = l.content_hash \
             JOIN paths p ON p.id = l.representative_path_id \
             WHERE {owed} \
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
    let owed = WorkClass::Faces.owed_sql().expect("faces owe rows");
    let sql = format!(
        "WITH hinted(hash, priority) AS (VALUES {values}) \
         SELECT c.hash, p.abs_path FROM hinted h \
         JOIN review_contents l ON l.content_hash = h.hash \
         JOIN contents c ON c.hash = l.content_hash \
         JOIN paths p ON p.id = l.representative_path_id \
         WHERE {owed} AND p.missing = 0 \
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

fn transcript_owed(kind: &str) -> Result<String, String> {
    WorkClass::transcription_for_kind(kind)
        .and_then(WorkClass::owed_sql)
        .ok_or_else(|| format!("unsupported transcription kind: {kind}"))
}

pub fn transcript_candidates(
    conn: &Connection,
    kind: &str,
    after_hash: Option<&str>,
    limit: usize,
) -> Result<Vec<(String, String)>, String> {
    let owed = transcript_owed(kind)?;
    let mut statement = conn
        .prepare(&format!(
            "SELECT c.hash, p.abs_path \
             FROM review_contents l \
             JOIN contents c ON c.hash = l.content_hash \
             JOIN paths p ON p.id = l.representative_path_id \
             WHERE {owed} AND p.missing = 0 \
               AND l.content_hash > ?1 \
             ORDER BY l.content_hash LIMIT ?2"
        ))
        .map_err(|error| error.to_string())?;
    let rows = statement
        .query_map(
            params![after_hash.unwrap_or(""), limit as i64],
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
    let owed = transcript_owed(kind)?;
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
         JOIN review_contents l ON l.content_hash = h.hash \
         JOIN contents c ON c.hash = l.content_hash \
         JOIN paths p ON p.id = l.representative_path_id \
         WHERE {owed} AND p.missing = 0 \
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

pub fn transcript_result(conn: &Connection, hash: &str) -> Result<TranscriptResult, String> {
    if let Some(transcript) = stored_transcript(conn, hash)? {
        return Ok(TranscriptResult {
            status: READY,
            text: Some(crate::transcription::render(&transcript.segments)),
            message: None,
        });
    }
    let failed: bool = conn
        .query_row(
            &format!(
                "SELECT {} FROM contents c WHERE c.hash = ?1",
                failed_now_sql("c", TRANSCRIPTS_CLASS, &crate::ai_dependencies::transcription_model())
            ),
            [hash],
            |row| row.get(0),
        )
        .optional()
        .map_err(|error| error.to_string())?
        .unwrap_or(false);
    if !failed {
        return Ok(TranscriptResult {
            status: "pending",
            text: None,
            message: None,
        });
    }
    let message = conn
        .query_row(
            "SELECT i.message FROM active_issues i JOIN paths p ON p.abs_path = i.path
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

fn stored_transcript(conn: &Connection, hash: &str) -> Result<Option<Transcript>, String> {
    let row: Option<(Option<String>, String)> = conn
        .query_row(
            "SELECT language, segments FROM transcripts WHERE content_hash = ?1",
            [hash],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|error| error.to_string())?;
    row.map(|(language, segments)| {
        Ok(Transcript {
            language,
            segments: serde_json::from_str(&segments)
                .map_err(|error| format!("stored transcript is unreadable: {error}"))?,
        })
    })
    .transpose()
}

/// Returns whether this success actually resolved a live Issue, so the
/// derived-work coordinator can tell the Issues surface changed even when the
/// batch as a whole had no failure (C-M3).
pub fn record_preview_success(
    conn: &Connection,
    hash: &str,
    path: &str,
    width: u32,
    height: u32,
    sharpness: f64,
    phash: u64,
) -> Result<bool, String> {
    let transaction = crate::sqlite::write_transaction(conn)
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            &format!(
                "UPDATE contents SET width = COALESCE(width, ?2), \
                 height = COALESCE(height, ?3), sharpness = ?4, phash = ?5, \
                 derived_at_utc = ?6, derive_outcome = NULL, \
                 derived_version = {DERIVE_VERSION} WHERE hash = ?1"
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
    let issues_changed = crate::index_store::clear_issues(&transaction, path, &[PREVIEW_ERROR])?;
    crate::records::commit(transaction).map_err(|error| error.to_string())?;
    Ok(issues_changed)
}

pub fn record_preview_blocked(conn: &Connection, hash: &str) -> Result<(), String> {
    conn.execute(
        "UPDATE contents SET derive_outcome = ?2 WHERE hash = ?1",
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
) -> Result<bool, String> {
    logged_if_unrecorded(path, issue_kind, message, || {
        let transaction = crate::sqlite::write_transaction(conn)
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE contents SET derive_outcome = ?2 WHERE hash = ?1",
                params![hash, FAILED],
            )
            .map_err(|error| error.to_string())?;
        let issues_changed = record_derived_issue(&transaction, path, issue_kind, message)?;
        crate::records::commit(transaction).map_err(|error| error.to_string())?;
        Ok(issues_changed)
    })
}

/// The catalogue key naming a derived-work Issue's sentence for `issue_kind`,
/// so it follows the interface language; the raw diagnostic (an ffmpeg or
/// decode error) stays in `message`, as recorded, after it (R5.5 D-L12,
/// D-L13).
fn derived_issue_message_key(issue_kind: &str) -> &'static str {
    match issue_kind {
        PREVIEW_ERROR => "notice.previewFailed",
        VIDEO_POSTER_ERROR => "notice.videoPosterFailed",
        VIDEO_STRIP_ERROR => "notice.videoStripFailed",
        FACE_ERROR => "notice.faceScoreFailed",
        TRANSCRIPT_ERROR => "notice.transcriptFailed",
        _ => "notice.derivedPrepFailed",
    }
}

/// The Issue row (with any analysis row beside it) is the normal record of a
/// derived-work failure, so nothing is logged when it is written. When the
/// record cannot be written or committed, one error line keeps the original
/// diagnostic together with the records error (logging conventions: failure
/// evidence survives its record's failure).
fn logged_if_unrecorded(
    path: &str,
    issue_kind: &str,
    diagnostic: &str,
    record: impl FnOnce() -> Result<bool, String>,
) -> Result<bool, String> {
    let recorded = record();
    if let Err(record_error) = &recorded {
        crate::logging::error(
            "derived media failure could not be recorded",
            json!({
                "kind": issue_kind,
                "path": path,
                "error": { "message": diagnostic },
                "recordError": { "message": record_error },
            }),
        );
    }
    recorded
}

/// Returns whether this call actually opened a new live Issue (C-M3).
fn record_derived_issue(
    conn: &Connection,
    path: &str,
    issue_kind: &str,
    diagnostic: &str,
) -> Result<bool, String> {
    crate::index_store::upsert_issue_with_descriptor(
        conn,
        Some(path),
        issue_kind,
        Some(derived_issue_message_key(issue_kind)),
        None,
        diagnostic,
    )
}

pub fn record_preview_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    message: &str,
) -> Result<bool, String> {
    record_content_failure(conn, hash, path, PREVIEW_ERROR, message)
}

/// A poster for a video whose container reports no duration still
/// succeeds; the video simply has no scene snapshots, since none can be
/// placed along an unknown timeline.
pub fn record_poster_success(
    conn: &Connection,
    hash: &str,
    path: &str,
    duration_ms: Option<u64>,
) -> Result<bool, String> {
    let transaction = crate::sqlite::write_transaction(conn)
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            &format!(
                "UPDATE contents SET duration_ms = COALESCE(duration_ms, ?2), \
                 strip_frames = CASE WHEN ?2 IS NULL AND duration_ms IS NULL \
                     THEN COALESCE(strip_frames, 0) ELSE strip_frames END, \
                 derived_at_utc = ?3, derive_outcome = NULL, \
                 derived_version = {DERIVE_VERSION} WHERE hash = ?1"
            ),
            params![
                hash,
                duration_ms.map(|value| value as i64),
                crate::logging::now_iso_millis()
            ],
        )
        .map_err(|error| error.to_string())?;
    let issues_changed =
        crate::index_store::clear_issues(&transaction, path, &[VIDEO_POSTER_ERROR])?;
    crate::records::commit(transaction).map_err(|error| error.to_string())?;
    Ok(issues_changed)
}

pub fn record_poster_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    message: &str,
) -> Result<bool, String> {
    record_content_failure(conn, hash, path, VIDEO_POSTER_ERROR, message)
}

pub fn record_strip_success(
    conn: &Connection,
    hash: &str,
    path: &str,
    frame_count: u32,
) -> Result<bool, String> {
    let transaction = crate::sqlite::write_transaction(conn)
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "UPDATE contents SET strip_frames = ?2 WHERE hash = ?1",
            params![hash, frame_count as i64],
        )
        .map_err(|error| error.to_string())?;
    let issues_changed =
        crate::index_store::clear_issues(&transaction, path, &[VIDEO_STRIP_ERROR])?;
    crate::records::commit(transaction).map_err(|error| error.to_string())?;
    Ok(issues_changed)
}

pub fn record_strip_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    message: &str,
) -> Result<bool, String> {
    logged_if_unrecorded(path, VIDEO_STRIP_ERROR, message, || {
        let transaction = crate::sqlite::write_transaction(conn)
            .map_err(|error| error.to_string())?;
        transaction
            .execute(
                "UPDATE contents SET strip_frames = ?2 WHERE hash = ?1",
                params![hash, STRIP_FAILED],
            )
            .map_err(|error| error.to_string())?;
        let issues_changed =
            record_derived_issue(&transaction, path, VIDEO_STRIP_ERROR, message)?;
        crate::records::commit(transaction).map_err(|error| error.to_string())?;
        Ok(issues_changed)
    })
}

pub fn record_face_success(
    conn: &Connection,
    hash: &str,
    path: &str,
    found: &[crate::face::FoundFace],
    model: &crate::ai_dependencies::ModelIdentity,
) -> Result<bool, String> {
    let transaction = crate::sqlite::write_transaction(conn)
        .map_err(|error| error.to_string())?;
    transaction
        .execute("DELETE FROM faces WHERE content_hash = ?1", [hash])
        .map_err(|error| error.to_string())?;
    for found in found {
        let expression = found.expression.map(|probabilities| probabilities.map(f64::from));
        let probability = |index: usize| expression.map(|probabilities| probabilities[index]);
        transaction
            .execute(
                "INSERT INTO faces
                   (content_hash, x1, y1, x2, y2, confidence, anger, contempt, disgust, fear,
                    happiness, neutral, sadness, surprise, model, model_version)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16)",
                params![
                    hash,
                    found.face.x1,
                    found.face.y1,
                    found.face.x2,
                    found.face.y2,
                    found.face.confidence,
                    probability(0),
                    probability(1),
                    probability(2),
                    probability(3),
                    probability(4),
                    probability(5),
                    probability(6),
                    probability(7),
                    model.model,
                    model.version,
                ],
            )
            .map_err(|error| error.to_string())?;
    }
    transaction
        .execute(
            "INSERT INTO face_checks (content_hash, model, model_version, face_count, checked_at_utc)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (content_hash) DO UPDATE SET
               model = excluded.model, model_version = excluded.model_version,
               face_count = excluded.face_count, checked_at_utc = excluded.checked_at_utc",
            params![
                hash,
                model.model,
                model.version,
                found.len() as i64,
                crate::logging::now_iso_millis()
            ],
        )
        .map_err(|error| error.to_string())?;
    let issues_changed = crate::index_store::clear_issues(&transaction, path, &[FACE_ERROR])?;
    crate::records::commit(transaction).map_err(|error| error.to_string())?;
    Ok(issues_changed)
}

/// Records a failed analysis as a record beside the Issue it raises.
fn record_analysis_failure(
    transaction: &Connection,
    hash: &str,
    class: &str,
    model: &crate::ai_dependencies::ModelIdentity,
    path: &str,
    message: &str,
) -> Result<(), String> {
    transaction
        .execute(
            &format!(
                "INSERT INTO records.analysis_events
                   (session_id, time_utc, content_hash, class, event, model, model_version, path, message)
                 VALUES ({}, ?1, ?2, ?3, '{FAILED}', ?4, ?5, ?6, ?7)",
                crate::records::session_sql()
            ),
            params![
                crate::logging::now_iso_millis(),
                hash,
                class,
                model.model,
                model.version,
                path,
                message
            ],
        )
        .map(|_| crate::records::wrote(transaction))
        .map_err(|error| error.to_string())
}

pub fn record_face_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    message: &str,
) -> Result<bool, String> {
    logged_if_unrecorded(path, FACE_ERROR, message, || {
        let transaction = crate::sqlite::write_transaction(conn)
            .map_err(|error| error.to_string())?;
        record_analysis_failure(
            &transaction,
            hash,
            FACES_CLASS,
            &crate::ai_dependencies::face_model(),
            path,
            message,
        )?;
        let issues_changed = record_derived_issue(&transaction, path, FACE_ERROR, message)?;
        crate::records::commit(transaction).map_err(|error| error.to_string())?;
        Ok(issues_changed)
    })
}

pub fn record_transcript_success(
    conn: &Connection,
    hash: &str,
    path: &str,
    transcript: &Transcript,
    model: crate::ai_dependencies::ModelIdentity,
) -> Result<bool, String> {
    let transaction = crate::sqlite::write_transaction(conn)
        .map_err(|error| error.to_string())?;
    transaction
        .execute(
            "INSERT INTO transcripts
               (content_hash, model, model_version, language, text, segments, created_at_utc)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT (content_hash) DO UPDATE SET
               model = excluded.model, model_version = excluded.model_version,
               language = excluded.language, text = excluded.text,
               segments = excluded.segments, created_at_utc = excluded.created_at_utc",
            params![
                hash,
                model.model,
                model.version,
                transcript.language,
                transcript.text(),
                serde_json::to_string(&transcript.segments).map_err(|error| error.to_string())?,
                crate::logging::now_iso_millis(),
            ],
        )
        .map_err(|error| error.to_string())?;
    let issues_changed = crate::index_store::clear_issues(&transaction, path, &[TRANSCRIPT_ERROR])?;
    crate::records::commit(transaction).map_err(|error| error.to_string())?;
    Ok(issues_changed)
}

pub fn record_transcript_failure(
    conn: &Connection,
    hash: &str,
    path: &str,
    message: &str,
) -> Result<bool, String> {
    logged_if_unrecorded(path, TRANSCRIPT_ERROR, message, || {
        let transaction = crate::sqlite::write_transaction(conn)
            .map_err(|error| error.to_string())?;
        record_analysis_failure(
            &transaction,
            hash,
            TRANSCRIPTS_CLASS,
            &crate::ai_dependencies::transcription_model(),
            path,
            message,
        )?;
        let issues_changed =
            record_derived_issue(&transaction, path, TRANSCRIPT_ERROR, message)?;
        crate::records::commit(transaction).map_err(|error| error.to_string())?;
        Ok(issues_changed)
    })
}

/// A replacement attempt never invalidates the completed transcript it was
/// meant to supersede. Only the new Issue is raised; the old transcript
/// remains current until a later replacement succeeds.
pub fn record_transcript_replacement_failure(
    conn: &Connection,
    path: &str,
    message: &str,
) -> Result<bool, String> {
    logged_if_unrecorded(path, TRANSCRIPT_ERROR, message, || {
        record_derived_issue(conn, path, TRANSCRIPT_ERROR, message)
    })
}

// EXCEPTION (tests-folder convention): this pins a private database-state
// transition without widening the production storage surface for a test.
#[cfg(test)]
// EXCEPTION to tests-folder conventions: exercises the private
// `derived_issue_message_key`; promoting it would widen the crate's API
// only for this test.
#[path = "../tests/unit/derived_state/transcript_replacement_tests.rs"]
mod transcript_replacement_tests;

/// An explicit attempt boundary, not a query side effect or an Issues action.
pub enum FailedOutputScope {
    Library,
    Section {
        kind: crate::queries::SectionKind,
        bounds: Option<(i64, i64)>,
    },
}

/// Reopens only failed outputs. Completed values, waiting prerequisites,
/// feature policy, and diagnostic records are deliberately untouched.
pub fn reset_failed_outputs(
    conn: &Connection,
    scope: FailedOutputScope,
) -> Result<u64, String> {
    let transaction = crate::sqlite::write_transaction(conn).map_err(|error| error.to_string())?;
    let count = reset_failed_outputs_in_transaction(&transaction, scope)?;
    crate::records::commit(transaction).map_err(|error| error.to_string())?;
    Ok(count)
}

pub(crate) fn reset_failed_outputs_in_transaction(
    conn: &Connection,
    scope: FailedOutputScope,
) -> Result<u64, String> {
    let (membership, values) = match scope {
        FailedOutputScope::Library => (String::new(), Vec::new()),
        FailedOutputScope::Section { kind, bounds } => {
            let mut values = vec![rusqlite::types::Value::Text(kind.as_str().to_string())];
            let dates = match bounds {
                Some((start, end)) if start < end => {
                    values.extend([start.into(), end.into()]);
                    "AND resolved_utc_ms >= ?2 AND resolved_utc_ms < ?3"
                }
                Some(_) => return Err("section date range must be increasing".to_string()),
                None => "AND resolved_utc_ms IS NULL",
            };
            (
                format!(" IN (SELECT content_hash FROM review_contents WHERE kind = ?1 {dates})"),
                values,
            )
        }
    };
    let mut count = 0;
    for (table, key, reset, failed) in [
        (
            "contents",
            "hash",
            "derive_outcome = NULL",
            "derive_outcome = 'failed'",
        ),
        (
            "contents",
            "hash",
            "strip_frames = NULL",
            "strip_frames = -1",
        ),
    ] {
        let filter = if membership.is_empty() {
            String::new()
        } else {
            format!(" AND {key}{membership}")
        };
        count += conn
            .execute(
                &format!("UPDATE {table} SET {reset} WHERE {failed}{filter}"),
                params_from_iter(values.iter()),
            )
            .map_err(|error| error.to_string())? as u64;
    }
    // A failure stays a record; the user's new attempt is recorded after it.
    for (class, state) in [
        (FACES_CLASS, face_state_sql("c")),
        (TRANSCRIPTS_CLASS, transcript_state_sql("c")),
    ] {
        let filter = if membership.is_empty() {
            String::new()
        } else {
            format!(" AND c.hash{membership}")
        };
        let reopened = conn
            .execute(
                &format!(
                    "INSERT INTO records.analysis_events (session_id, time_utc, content_hash, class, event)
                     SELECT {}, {}, c.hash, '{class}', 'reopened' FROM contents c
                     WHERE {state} = '{FAILED}'{filter}",
                    crate::records::session_sql(),
                    crate::records::sql_text(Some(&crate::logging::now_iso_millis())),
                ),
                params_from_iter(values.iter()),
            )
            .map_err(|error| error.to_string())?;
        if reopened > 0 {
            crate::records::wrote(conn);
        }
        count += reopened as u64;
    }
    Ok(count)
}

/// Reopens every analysis that failed in this launch. A rebuild is such a
/// boundary: the contents it clears come back to be attempted again.
pub(crate) fn reopen_session_analysis_failures(conn: &Connection) -> Result<(), String> {
    let session = crate::records::session_sql();
    conn.execute(
        &format!(
            "INSERT INTO records.analysis_events (session_id, time_utc, content_hash, class, event)
             SELECT {session}, ?1, e.content_hash, e.class, 'reopened' FROM records.analysis_events e
             WHERE e.event = '{FAILED}' AND e.id IN (
               SELECT MAX(x.id) FROM records.analysis_events x
               WHERE x.session_id IS {session} GROUP BY x.content_hash, x.class)"
        ),
        [crate::logging::now_iso_millis()],
    )
    .map(|reopened| if reopened > 0 { crate::records::wrote(conn) })
    .map_err(|error| error.to_string())
}

pub(crate) fn preview_failed(conn: &Connection, hash: &str) -> Result<bool, String> {
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM contents WHERE hash = ?1 AND derive_outcome = ?2)",
        params![hash, FAILED],
        |row| row.get(0),
    )
    .map_err(|error| error.to_string())
}
