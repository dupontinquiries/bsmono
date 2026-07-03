//! Face embedding extraction (DEFERRED).
//!
//! **Status (V32.32):** Deferred. This module was scaffolded for a
//! prior generic-face-recognition direction (ArcFace-style 512-dim
//! embeddings, identity matching across contacts). That direction
//! has been superseded by the personal-bandwidth approach — see
//! `docs/FACE_BANDWIDTH.md` and the crate-level `lib.rs` for the
//! active design.
//!
//! The module is preserved so that **if** Phase 3 (on-device neural
//! model) ever needs embedding extraction, the API surface is
//! already declared. New code should NOT consume from this module
//! — use `library` and `reference` instead.
//!
//! ---
//!
//! Original module documentation follows:
//!
//! An "embedding" is a fixed-dim feature vector that uniquely
//! identifies a face — two photos of the same person produce
//! similar embeddings (small Euclidean distance); two different
//! people produce dissimilar embeddings. Modern face recognition
//! systems use 128-dim (FaceNet) or 512-dim (ArcFace) vectors.
//!
//! ## Current state: stub
//!
//! `Embedder::embed` returns a deterministic zero vector. The API is
//! real; the inference is not. See `lib.rs` rationale for why we
//! haven't committed to a specific model + runtime yet.
//!
//! ## Wiring the real model
//!
//! When ready:
//!   1. Pick a model. ArcFace (insightface), MobileFaceNet, or
//!      adaface are the obvious candidates. MobileFaceNet is smallest
//!      (~5MB) and least accurate; ArcFace ~100MB and most accurate.
//!   2. Commit to a runtime. tract-onnx is in the dep tree already
//!      via the detector — using the same lets us share the runtime
//!      and avoid two ONNX backends.
//!   3. Implement `Embedder::from_onnx(path)` to actually load.
//!   4. Implement `Embedder::embed(crop)` to:
//!      a. Resize crop to model's expected input (typically 112x112)
//!      b. Normalize to model's expected range (typically [-1, 1])
//!      c. Run inference
//!      d. L2-normalize output (standard for face embeddings)
//!   5. Bump `EMBEDDING_MODEL_VERSION` so stored profiles get
//!      invalidated.

use crate::FaceError;

/// Dimension of the face embedding vector. 512 matches ArcFace; can
/// be changed later if we pick a different model but doing so breaks
/// previously-stored profiles (see EMBEDDING_MODEL_VERSION).
pub const EMBEDDING_DIM: usize = 512;

/// Bump this when:
///   - The model architecture changes (different dim, different
///     normalization, different input preprocessing)
///   - The runtime changes (tract → onnxruntime → onnx2c, etc.)
///   - Anything that would make a stored embedding produce different
///     match results than a freshly-computed one
///
/// Stored profiles include this version; on load, profiles with a
/// mismatched version are invalidated and discarded.
pub const EMBEDDING_MODEL_VERSION: u32 = 0;  // 0 = stub

/// A 512-element L2-normalized face feature vector.
///
/// L2-normalized means the values sum-of-squares to ~1.0, which
/// makes cosine similarity equivalent to dot product (cheaper to
/// compute). All real face embedders produce this. The stub
/// produces all-zeros which has the special property that ALL
/// distances are zero — useful for testing the storage layer
/// without false positives in match logic.
///
/// Storage note: `vec` is `Vec<f32>` rather than `[f32; EMBEDDING_DIM]`
/// for serde compatibility — serde can't auto-derive Serialize/
/// Deserialize for arrays larger than 32. Vec adds one heap
/// allocation per embedding (negligible for a feature that runs
/// occasionally) and brings the bonus of letting future model
/// changes use a different dimension without breaking the type.
/// `EMBEDDING_DIM` is still the canonical length; `Embedder::embed`
/// always returns a Vec of exactly that size.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct FaceEmbedding {
    /// The feature vector. Length must equal `EMBEDDING_DIM`.
    pub vec: Vec<f32>,
    /// Model version that produced this embedding. Stored alongside
    /// so we can invalidate when the model changes.
    pub model_version: u32,
}

impl FaceEmbedding {
    /// All-zeros embedding. Used by the stub. Distance to anything
    /// else is also zero, so the matcher will refuse to match (sane
    /// default: stub returns no matches, not random matches).
    pub fn zero() -> Self {
        Self { vec: vec![0.0; EMBEDDING_DIM], model_version: EMBEDDING_MODEL_VERSION }
    }

    /// Euclidean (L2) distance between two embeddings. Lower = more
    /// similar. For L2-normalized vectors, distance is in [0.0, 2.0].
    pub fn distance(&self, other: &FaceEmbedding) -> f32 {
        if self.model_version != other.model_version {
            // Incompatible — return infinity so the matcher treats as
            // "no match" rather than producing nonsense.
            return f32::INFINITY;
        }
        if self.vec.len() != other.vec.len() {
            // Different-length vectors are also incomparable. This
            // shouldn't happen in practice (same model_version =>
            // same length) but defensive.
            return f32::INFINITY;
        }
        let mut sq = 0.0f32;
        for i in 0..self.vec.len() {
            let d = self.vec[i] - other.vec[i];
            sq += d * d;
        }
        sq.sqrt()
    }

    /// Cosine similarity between two embeddings. Higher = more
    /// similar. For L2-normalized vectors, in [-1.0, 1.0]; typically
    /// in [0.0, 1.0] for face embeddings (same-person ≥ 0.5 usually).
    pub fn cosine_similarity(&self, other: &FaceEmbedding) -> f32 {
        if self.model_version != other.model_version {
            return 0.0;
        }
        if self.vec.len() != other.vec.len() {
            return 0.0;
        }
        let mut dot = 0.0f32;
        for i in 0..self.vec.len() {
            dot += self.vec[i] * other.vec[i];
        }
        dot
    }

    /// True if every value is zero (e.g. stub output, or uninitialized).
    /// Matchers should refuse to match against zero embeddings.
    pub fn is_zero(&self) -> bool {
        self.vec.iter().all(|&v| v == 0.0)
    }
}

/// Face embedding extractor.
///
/// V32.32: scaffold-only. `from_onnx` accepts a model path for API
/// completeness but doesn't load it; `embed` returns zero vectors.
/// See module docs for the real wire-up plan.
pub struct Embedder {
    #[cfg(feature = "onnx")]
    _model_path: String,
}

impl Embedder {
    /// Construct from a path to an ONNX face-embedding model.
    ///
    /// V32.32: stub — accepts any path string and returns a working
    /// Embedder that produces zero embeddings.
    pub fn from_onnx(_path: &str) -> Result<Self, FaceError> {
        Ok(Self {
            #[cfg(feature = "onnx")]
            _model_path: _path.to_string(),
        })
    }

    /// Compute the embedding of a face crop.
    ///
    /// `rgb` is the tightly-packed RGB888 of a face crop — typically
    /// the output of `Detector::detect` followed by cropping the
    /// bounding box and resizing to the model's expected input size.
    /// The Embedder is responsible for any additional preprocessing
    /// (normalization, alignment) the model expects.
    ///
    /// V32.32: returns a zero embedding regardless of input.
    pub fn embed(
        &self,
        rgb: &[u8],
        width: u32,
        height: u32,
    ) -> Result<FaceEmbedding, FaceError> {
        // Validate the input shape so callers find bugs early even
        // though we don't yet use the pixels.
        let expected = (width as usize)
            .checked_mul(height as usize)
            .and_then(|v| v.checked_mul(3))
            .ok_or(FaceError::InvalidInput("dimensions overflow"))?;
        if rgb.len() < expected {
            return Err(FaceError::InvalidInput(
                "rgb buffer smaller than width*height*3",
            ));
        }
        if width < 16 || height < 16 {
            return Err(FaceError::InvalidInput("face crop smaller than 16x16"));
        }
        Ok(FaceEmbedding::zero())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_embedding_is_zero() {
        let z = FaceEmbedding::zero();
        assert!(z.is_zero());
        assert_eq!(z.distance(&FaceEmbedding::zero()), 0.0);
    }

    #[test]
    fn version_mismatch_yields_infinite_distance() {
        let mut a = FaceEmbedding::zero();
        let mut b = FaceEmbedding::zero();
        a.model_version = 1;
        b.model_version = 2;
        assert_eq!(a.distance(&b), f32::INFINITY);
    }

    #[test]
    fn distance_is_symmetric() {
        let mut a = FaceEmbedding::zero();
        let mut b = FaceEmbedding::zero();
        a.vec[0] = 1.0;
        b.vec[1] = 1.0;
        let d_ab = a.distance(&b);
        let d_ba = b.distance(&a);
        assert!((d_ab - d_ba).abs() < 1e-6);
        // sqrt(2) for two orthogonal unit vectors
        assert!((d_ab - 2.0f32.sqrt()).abs() < 1e-6);
    }

    #[test]
    fn embedder_validates_input_size() {
        let e = Embedder::from_onnx("any-path").unwrap();
        let tiny = vec![0u8; 8 * 8 * 3];
        assert!(matches!(
            e.embed(&tiny, 8, 8),
            Err(FaceError::InvalidInput(_))
        ));
    }
}
