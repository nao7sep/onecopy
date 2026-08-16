//! Image embeddings for cross-device similarity (Design: Similar-shot
//! grouping) — a SigLIP 2 large vision tower run through ONNX Runtime,
//! linked in like the whisper engine; only the MODEL is provisioned, by the
//! managed registry. dHash sees gradient layouts, so two devices' renderings
//! of one scene rarely match it; the embedding is the signal that does.
//!
//! Preprocessing follows the model's own published contract exactly
//! (preprocessor_config.json, probed 2026-08-17): resize DIRECTLY to
//! 384×384 — SigLIP squares the image rather than cropping it, so nothing at
//! the edges is thrown away — RGB, scale 1/255, then normalize with mean and
//! std 0.5. This differs from CLIP's shortest-edge-plus-centre-crop and its
//! own mean/std, which is why the contract is read per model rather than
//! carried over. The output embedding is L2-normalized here so similarity is
//! a plain dot product.

use std::path::Path;

use image::DynamicImage;

/// The code accepts whatever embedding width the model declares rather than
/// pinning the number, so an artifact swap cannot silently truncate; the
/// sanity bound only rejects the absurd.
pub const MAX_EMBEDDING_DIM: usize = 4096;

/// The published input size and normalization for the pinned tower.
const INPUT_EDGE: u32 = 384;
const NORM_MEAN: f32 = 0.5;
const NORM_STD: f32 = 0.5;

/// One session reused across a whole pass — model load costs seconds, one
/// image costs tens of milliseconds.
pub struct Embedder {
    session: ort::session::Session,
    input_name: String,
    output_name: String,
}

impl Embedder {
    pub fn load(model: &Path) -> Result<Embedder, String> {
        let session = ort::session::Session::builder()
            .map_err(|e| e.to_string())?
            .commit_from_file(model)
            .map_err(|e| format!("embedding model load failed: {e}"))?;
        // Discover the IO by name at load, once, with the honest error if the
        // artifact ever stops looking like a CLIP vision encoder. The pooled
        // embedding output is preferred by name; otherwise the one output
        // that is not the hidden-state sequence.
        let input_name = session
            .inputs()
            .first()
            .map(|i| i.name().to_string())
            .ok_or("model declares no inputs")?;
        let output_names: Vec<String> =
            session.outputs().iter().map(|o| o.name().to_string()).collect();
        let output_name = output_names
            .iter()
            .find(|n| n.contains("pool") || n.contains("embed"))
            .or_else(|| output_names.iter().find(|n| !n.contains("hidden")))
            .cloned()
            .ok_or_else(|| format!("no usable output among {output_names:?}"))?;
        crate::logging::debug(
            "embedding model loaded",
            serde_json::json!({ "input": input_name, "output": output_name, "outputs": output_names }),
        );
        Ok(Embedder { session, input_name, output_name })
    }

    /// The published preprocessing, then the encoder, then L2 normalization.
    pub fn embed(&mut self, img: &DynamicImage) -> Result<Vec<f32>, String> {
        // A DIRECT square resize, per the contract — no centre crop, so a
        // subject at the edge of a wide frame still reaches the encoder.
        let square = img
            .resize_exact(INPUT_EDGE, INPUT_EDGE, image::imageops::FilterType::Triangle)
            .to_rgb8();

        let edge = INPUT_EDGE as usize;
        let mut tensor = ndarray::Array4::<f32>::zeros((1, 3, edge, edge));
        for (x, y, pixel) in square.enumerate_pixels() {
            for channel in 0..3 {
                tensor[[0, channel, y as usize, x as usize]] =
                    (pixel[channel] as f32 / 255.0 - NORM_MEAN) / NORM_STD;
            }
        }

        let input_name = self.input_name.clone();
        let output_name = self.output_name.clone();
        let outputs = self
            .session
            .run(ort::inputs![input_name.as_str() => ort::value::Tensor::from_array(tensor).map_err(|e| e.to_string())?])
            .map_err(|e| format!("embedding inference failed: {e}"))?;
        let pooled = outputs
            .get(output_name.as_str())
            .ok_or("embedding output vanished mid-session")?;
        let (_, data) = pooled
            .try_extract_tensor::<f32>()
            .map_err(|e| e.to_string())?;
        let mut vector: Vec<f32> = data.to_vec();
        if vector.is_empty() || vector.len() > MAX_EMBEDDING_DIM {
            return Err(format!("unexpected embedding size {}", vector.len()));
        }
        let norm = vector.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for value in &mut vector {
                *value /= norm;
            }
        }
        Ok(vector)
    }
}

/// Cosine over L2-normalized vectors is the dot product; mismatched or empty
/// inputs read as no-similarity rather than an error (degenerate rows must
/// never poison a rebuild).
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// The BLOB codec for `contents.embedding`: f32 little-endian, nothing else.
pub fn to_blob(vector: &[f32]) -> Vec<u8> {
    vector.iter().flat_map(|v| v.to_le_bytes()).collect()
}

pub fn from_blob(blob: &[u8]) -> Option<Vec<f32>> {
    if blob.is_empty() || blob.len() % 4 != 0 {
        return None;
    }
    Some(
        blob.chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect(),
    )
}

/// Leader clustering in cosine space — bounded by construction (every member
/// within the threshold of its LEADER), so embedding similarity can never
/// chain the way raw union-find let dhash chain. Indices with no embedding
/// never cluster. Deterministic: leaders arise in index order.
pub fn embedding_clusters(
    embeddings: &[Option<Vec<f32>>],
    min_cosine: f32,
) -> Vec<Vec<usize>> {
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for (i, embedding) in embeddings.iter().enumerate() {
        let Some(embedding) = embedding else { continue };
        let found = clusters.iter_mut().find(|cluster| {
            embeddings[cluster[0]]
                .as_ref()
                .map(|leader| cosine(leader, embedding) >= min_cosine)
                .unwrap_or(false)
        });
        match found {
            Some(cluster) => cluster.push(i),
            None => clusters.push(vec![i]),
        }
    }
    clusters.retain(|cluster| cluster.len() >= 2);
    clusters
}

#[derive(Default, Debug)]
pub struct EmbedStats {
    pub embedded: u64,
    pub failed: u64,
}

/// The embed pass over the index: images with a derived preview and no
/// embedding yet, read FROM THE CACHE (a few-hundred-KB decode — the original
/// is never touched), embedded serially through one session, the BLOB stored
/// on contents. Model absent → an empty pass, silently: dHash-only is the
/// designed fallback. Cancellable between items like every pipeline stage.
pub fn embed_images_pending(
    conn: &rusqlite::Connection,
    cache: &crate::preview::CachePaths,
    model: Option<&Path>,
    mut on_progress: impl FnMut(u64, u64),
) -> Result<EmbedStats, String> {
    let mut stats = EmbedStats::default();
    let Some(model) = model else {
        return Ok(stats);
    };
    let pending: Vec<String> = {
        let mut stmt = conn
            .prepare(
                "SELECT hash FROM contents \
                 WHERE kind = 'image' AND embedding IS NULL \
                   AND derived_at_utc IS NOT NULL \
                   AND derived_at_utc NOT IN ('failed', ?1) \
                   AND EXISTS (SELECT 1 FROM paths p \
                               WHERE p.content_hash = contents.hash AND p.missing = 0)",
            )
            .map_err(|e| e.to_string())?;
        let rows = stmt
            .query_map([crate::preview::NEEDS_FFMPEG], |r| r.get::<_, String>(0))
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        rows
    };
    if pending.is_empty() {
        return Ok(stats);
    }

    let mut embedder = Embedder::load(model)?;
    let total = pending.len() as u64;
    for hash in pending {
        if crate::scanner::cancelled() {
            return Err(crate::scanner::CANCELLED.to_string());
        }
        let preview = cache.preview(&hash);
        let outcome = std::fs::read(&preview)
            .map_err(|e| e.to_string())
            .and_then(|bytes| image::load_from_memory(&bytes).map_err(|e| e.to_string()))
            .and_then(|img| embedder.embed(&img));
        match outcome {
            Ok(vector) => {
                conn.execute(
                    "UPDATE contents SET embedding = ?2 WHERE hash = ?1",
                    rusqlite::params![hash, to_blob(&vector)],
                )
                .map_err(|e| e.to_string())?;
                stats.embedded += 1;
            }
            Err(err) => {
                // A missing or undecodable preview is unusual but not a user
                // problem — the row stays pending and the log carries it.
                crate::logging::warn(
                    "embedding failed",
                    serde_json::json!({ "hash": hash, "error": { "message": err } }),
                );
                stats.failed += 1;
            }
        }
        on_progress(stats.embedded + stats.failed, total);
    }
    Ok(stats)
}
