//! Face scoring for group ordering (Design: Rating) — two small managed ONNX
//! models run through the same linked ONNX Runtime:
//! Ultraface RFB-640 finds faces, HSEmotion reads the expression, and the
//! combined score orders a group's face-bearing members ahead of sharpness.
//! Advisory only, never auto-deletes, exactly like sharpness.
//!
//! The score, in one sentence: the best face's detection confidence weighted
//! by how much it smiles — `conf × (0.5 + 0.5 × P(happiness))`, the maximum
//! over detected faces, so a photo where someone clearly smiles beats the
//! frame where the same person blurs or grimaces. No cleanly-licensed model
//! carries an eyes-open signal, so the smile weight is the honest v1 of the
//! Design's "eyes-open/smiling".
//!
//! Both models' input contracts are read from their upstreams, never assumed
//! (probed 2026-08-17). They disagree on everything that matters: the
//! detector takes 640×480 with a (x−127)/128 scaling, while the expression
//! model takes a 260×260 RGB crop with ImageNet normalization and returns
//! EIGHT AffectNet logits whose happiness sits at index 4 — where the FER+
//! model it replaces put happiness at index 1. Assuming an order carried
//! over would have scored a different emotion entirely, silently.
//!
//! Storage contract: `analysis_receipts.face_state` distinguishes pending,
//! ready, and failed; `contents.face_score` holds the ready value (0 means no
//! face, positive means a face was found). Ordering treats NULL and 0.0
//! identically, which keeps a model-less install ordering exactly as today.

use std::path::Path;

use image::DynamicImage;

/// Detections below this confidence are noise, not faces (the Ultraface
/// paper's own evaluation threshold).
pub const MIN_FACE_CONFIDENCE: f32 = 0.7;

/// The detector's published input size (RFB-640).
const DETECT_WIDTH: u32 = 640;
const DETECT_HEIGHT: u32 = 480;

/// The expression model's published input size (EfficientNet-B2) and the
/// ImageNet normalization it was trained with.
const EXPRESSION_EDGE: u32 = 260;
const IMAGENET_MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const IMAGENET_STD: [f32; 3] = [0.229, 0.224, 0.225];

/// Index of "Happiness" in the model's AffectNet class order
/// (Anger, Contempt, Disgust, Fear, HAPPINESS, Neutral, Sadness, Surprise).
const HAPPINESS_CLASS: usize = 4;
const EXPRESSION_CLASSES: usize = 8;
/// Greedy NMS overlap bound — boxes overlapping more than this are one face.
pub const NMS_MAX_IOU: f32 = 0.3;

/// One detected face: confidence plus its RELATIVE corner box on the scored
/// image (the detector works in normalized coordinates).
#[derive(Debug, Clone, Copy)]
pub struct Face {
    pub confidence: f32,
    pub x1: f32,
    pub y1: f32,
    pub x2: f32,
    pub y2: f32,
}

/// Both sessions loaded once per pass — model load costs real time, one
/// image costs milliseconds.
pub struct FaceScorer {
    detector: ort::session::Session,
    det_input: String,
    det_scores: String,
    det_boxes: String,
    emotion: ort::session::Session,
    emo_input: String,
    emo_output: String,
}

impl FaceScorer {
    pub fn load(detector_model: &Path, emotion_model: &Path) -> Result<FaceScorer, String> {
        crate::resource_limits::require_available(
            crate::resource_limits::FACE_REQUIRED_AVAILABLE,
            "Face scoring",
        )?;
        let detector = ort::session::Session::builder()
            .map_err(|e| e.to_string())?
            .commit_from_file(detector_model)
            .map_err(|e| format!("face detector load failed: {e}"))?;
        let det_input = detector
            .inputs()
            .first()
            .map(|i| i.name().to_string())
            .ok_or("detector declares no inputs")?;
        // Discovered by name at load so an artifact swap cannot silently hand
        // boxes to the score reader.
        let det_names: Vec<String> =
            detector.outputs().iter().map(|o| o.name().to_string()).collect();
        let det_scores = det_names
            .iter()
            .find(|n| n.contains("score") || n.contains("conf"))
            .cloned()
            .ok_or_else(|| format!("no score output among {det_names:?}"))?;
        let det_boxes = det_names
            .iter()
            .find(|n| n.contains("box"))
            .cloned()
            .ok_or_else(|| format!("no box output among {det_names:?}"))?;

        let emotion = ort::session::Session::builder()
            .map_err(|e| e.to_string())?
            .commit_from_file(emotion_model)
            .map_err(|e| format!("expression model load failed: {e}"))?;
        let emo_input = emotion
            .inputs()
            .first()
            .map(|i| i.name().to_string())
            .ok_or("expression model declares no inputs")?;
        let emo_output = emotion
            .outputs()
            .first()
            .map(|o| o.name().to_string())
            .ok_or("expression model declares no outputs")?;

        crate::logging::debug(
            "face models loaded",
            serde_json::json!({
                "detector": { "input": det_input, "scores": det_scores, "boxes": det_boxes },
                "emotion": { "input": emo_input, "output": emo_output },
            }),
        );
        Ok(FaceScorer { detector, det_input, det_scores, det_boxes, emotion, emo_input, emo_output })
    }

    /// Faces on the image, confidence-thresholded and NMS-deduplicated,
    /// best-first.
    pub fn detect(&mut self, img: &DynamicImage) -> Result<Vec<Face>, String> {
        // Ultraface's published contract: RGB at the model's input size,
        // scaled (x − 127) / 128.
        let resized = img
            .resize_exact(DETECT_WIDTH, DETECT_HEIGHT, image::imageops::FilterType::Triangle)
            .to_rgb8();
        let mut tensor =
            ndarray::Array4::<f32>::zeros((1, 3, DETECT_HEIGHT as usize, DETECT_WIDTH as usize));
        for (x, y, pixel) in resized.enumerate_pixels() {
            for channel in 0..3 {
                tensor[[0, channel, y as usize, x as usize]] =
                    (pixel[channel] as f32 - 127.0) / 128.0;
            }
        }
        let input = self.det_input.clone();
        let outputs = self
            .detector
            .run(ort::inputs![input.as_str() => ort::value::Tensor::from_array(tensor).map_err(|e| e.to_string())?])
            .map_err(|e| format!("face detection failed: {e}"))?;
        let (_, scores) = outputs
            .get(self.det_scores.as_str())
            .ok_or("score output vanished mid-session")?
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;
        let (_, boxes) = outputs
            .get(self.det_boxes.as_str())
            .ok_or("box output vanished mid-session")?
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;

        // scores: [1, N, 2] (background, face); boxes: [1, N, 4] relative
        // corners. The two flats must agree on N or the artifact is not what
        // the pin promises.
        if scores.len() % 2 != 0 || boxes.len() % 4 != 0 || scores.len() / 2 != boxes.len() / 4 {
            return Err(format!(
                "detector output shapes disagree: {} scores vs {} box values",
                scores.len(),
                boxes.len()
            ));
        }
        let candidates: Vec<Face> = (0..scores.len() / 2)
            .filter_map(|i| {
                let confidence = scores[i * 2 + 1];
                (confidence >= MIN_FACE_CONFIDENCE).then(|| Face {
                    confidence,
                    x1: boxes[i * 4],
                    y1: boxes[i * 4 + 1],
                    x2: boxes[i * 4 + 2],
                    y2: boxes[i * 4 + 3],
                })
            })
            .collect();
        Ok(non_max_suppression(candidates))
    }

    /// P(happiness) for one face crop — the expression model's contract:
    /// 260×260 RGB, 1/255 then ImageNet-normalized, eight AffectNet logits
    /// out, happiness at index 4.
    pub fn smile(&mut self, img: &DynamicImage, face: &Face) -> Result<f32, String> {
        let (w, h) = (img.width() as f32, img.height() as f32);
        // A 15% margin each side: FER+ was trained on loose crops, and the
        // detector's boxes hug the face tightly.
        let (bw, bh) = (face.x2 - face.x1, face.y2 - face.y1);
        let x1 = ((face.x1 - bw * 0.15) * w).max(0.0) as u32;
        let y1 = ((face.y1 - bh * 0.15) * h).max(0.0) as u32;
        let x2 = (((face.x2 + bw * 0.15) * w) as u32).min(img.width());
        let y2 = (((face.y2 + bh * 0.15) * h) as u32).min(img.height());
        if x2 <= x1 || y2 <= y1 {
            return Err("degenerate face box".to_string());
        }
        let crop = img
            .crop_imm(x1, y1, x2 - x1, y2 - y1)
            .resize_exact(EXPRESSION_EDGE, EXPRESSION_EDGE, image::imageops::FilterType::Triangle)
            .to_rgb8();
        let edge = EXPRESSION_EDGE as usize;
        let mut tensor = ndarray::Array4::<f32>::zeros((1, 3, edge, edge));
        for (x, y, pixel) in crop.enumerate_pixels() {
            for channel in 0..3 {
                tensor[[0, channel, y as usize, x as usize]] =
                    (pixel[channel] as f32 / 255.0 - IMAGENET_MEAN[channel]) / IMAGENET_STD[channel];
            }
        }
        let input = self.emo_input.clone();
        let outputs = self
            .emotion
            .run(ort::inputs![input.as_str() => ort::value::Tensor::from_array(tensor).map_err(|e| e.to_string())?])
            .map_err(|e| format!("expression inference failed: {e}"))?;
        let (_, logits) = outputs
            .get(self.emo_output.as_str())
            .ok_or("expression output vanished mid-session")?
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;
        let probabilities = softmax(logits);
        if probabilities.len() != EXPRESSION_CLASSES {
            // An artifact that changed shape has almost certainly changed its
            // class ORDER too, and scoring the wrong emotion silently is the
            // failure this refuses to risk.
            return Err(format!(
                "expression model returned {} classes, not {EXPRESSION_CLASSES}",
                probabilities.len()
            ));
        }
        Ok(probabilities[HAPPINESS_CLASS])
    }

    /// The composite: 0.0 = no face; otherwise the best face's
    /// `conf × (0.5 + 0.5 × P(happiness))`.
    pub fn score(&mut self, img: &DynamicImage) -> Result<f32, String> {
        let mut best = 0.0_f32;
        for face in self.detect(img)? {
            // A failed crop degrades to the neutral-expression weight rather
            // than sinking the photo: the face is still real.
            let smile = self.smile(img, &face).unwrap_or(0.0);
            best = best.max(face.confidence * (0.5 + 0.5 * smile));
        }
        Ok(best)
    }
}

/// Greedy NMS, best-first: keeps each face and suppresses every remaining
/// candidate overlapping it beyond [`NMS_MAX_IOU`].
pub fn non_max_suppression(mut candidates: Vec<Face>) -> Vec<Face> {
    candidates.sort_by(|a, b| b.confidence.total_cmp(&a.confidence));
    let mut kept: Vec<Face> = Vec::new();
    for candidate in candidates {
        if kept.iter().all(|k| iou(k, &candidate) <= NMS_MAX_IOU) {
            kept.push(candidate);
        }
    }
    kept
}

/// Intersection over union of two relative corner boxes; degenerate boxes
/// read as no overlap.
pub fn iou(a: &Face, b: &Face) -> f32 {
    let ix = (a.x2.min(b.x2) - a.x1.max(b.x1)).max(0.0);
    let iy = (a.y2.min(b.y2) - a.y1.max(b.y1)).max(0.0);
    let intersection = ix * iy;
    let area = |f: &Face| ((f.x2 - f.x1).max(0.0)) * ((f.y2 - f.y1).max(0.0));
    let union = area(a) + area(b) - intersection;
    if union <= 0.0 { 0.0 } else { intersection / union }
}

/// Numerically-stable softmax; an empty slice stays empty.
pub fn softmax(logits: &[f32]) -> Vec<f32> {
    let Some(max) = logits.iter().copied().reduce(f32::max) else {
        return Vec::new();
    };
    let exps: Vec<f32> = logits.iter().map(|v| (v - max).exp()).collect();
    let sum: f32 = exps.iter().sum();
    exps.iter().map(|v| v / sum).collect()
}

#[derive(Default, Debug)]
pub struct FaceStats {
    pub scored: u64,
    pub failed: u64,
    pub attempted: u64,
    pub candidates_found: bool,
    pub last_attempted_hash: Option<String>,
}

/// The face pass over the index — the embed pass's exact shape: images with a
/// derived preview and no score yet, read FROM THE CACHE, scored serially
/// through one session pair. Either model absent → an empty pass, silently:
/// sharpness-only ordering is the designed fallback. Cancellable between
/// items like every pipeline stage.
pub fn face_scores_pending(
    conn: &rusqlite::Connection,
    cache: &crate::preview::CachePaths,
    models: Option<(&Path, &Path)>,
    priority_hashes: &[String],
    mut on_item: impl FnMut(&str),
    mut on_change: impl FnMut(&str),
    mut on_progress: impl FnMut(u64, u64),
    after_hash: Option<&str>,
    stop: &dyn Fn() -> bool,
) -> Result<FaceStats, String> {
    let mut stats = FaceStats::default();
    let Some((detector_model, emotion_model)) = models else {
        return Ok(stats);
    };
    let pending = if priority_hashes.is_empty() {
        crate::derived_state::face_candidates(
            conn,
            after_hash,
            crate::derived_state::FACE_CANDIDATE_PAGE_SIZE,
        )?
    } else {
        crate::derived_state::prioritized_face_candidates(
            conn,
            priority_hashes,
            crate::derived_state::FACE_CANDIDATE_PAGE_SIZE,
        )?
    };
    if pending.is_empty() {
        return Ok(stats);
    }
    stats.candidates_found = true;

    let mut scorer = FaceScorer::load(detector_model, emotion_model)?;
    let total = pending.len() as u64;
    for (hash, path) in pending {
        if crate::scanner::cancelled() {
            return Err(crate::scanner::CANCELLED.to_string());
        }
        // Coordinator politeness: the user's return stops the pass
        // between images; what is scored stays scored.
        if stop() {
            break;
        }
        on_item(&hash);
        stats.attempted += 1;
        stats.last_attempted_hash = Some(hash.clone());
        let preview = cache.preview(&hash);
        let outcome = std::fs::read(&preview)
            .map_err(|e| e.to_string())
            .and_then(|bytes| crate::resource_limits::decode_bytes(&bytes))
            .and_then(|img| scorer.score(&img));
        match outcome {
            Ok(score) => {
                crate::derived_state::record_face_success(conn, &hash, &path, score as f64)?;
                on_change(&hash);
                stats.scored += 1;
            }
            Err(err) => {
                crate::logging::warn(
                    "face scoring failed",
                    serde_json::json!({ "hash": hash, "error": { "message": err.clone() } }),
                );
                crate::derived_state::record_face_failure(conn, &hash, &path, &err)?;
                on_change(&hash);
                stats.failed += 1;
            }
        }
        on_progress(stats.attempted, total);
    }
    Ok(stats)
}
