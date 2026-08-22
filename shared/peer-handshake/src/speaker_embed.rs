//! Speaker embeddings: vectorize a voice instead of collapsing it to
//! a scalar similarity. A speaker is a REGION in feature space, not a
//! point on a tuned dial — so identity becomes a distance/membership
//! question over a fixed-length vector, not a threshold on a blended
//! pitch+timbre number (the v0.1.496 weakness).
//!
//! This is the pure, dependency-free core (embed + distance + margin).
//! Rewiring identify() and changing speaker storage are deliberately
//! deferred (they calcify format/representation) — see
//! docs/design/speaker_embeddings.md.
//!
//! The embedding here is HAND-POOLED MFCC statistics (mean + std per
//! coefficient). It's the bridge to an eventual LEARNED embedding: the
//! interface — embed(...) -> Vec<f32> + cosine_distance — stays fixed,
//! so a trained on-device model can replace the producer later without
//! disturbing the identity/clustering layer.

/// MFCC coefficient count (matches phonetic::N_MFCC). Kept local so
/// this module is dependency-free and standalone-testable.
pub const N_MFCC: usize = 13;
/// Embedding dimensionality: mean (13) + std-dev (13) per coef.
pub const EMBED_DIM: usize = N_MFCC * 2;

/// Build a speaker embedding from an MFCC matrix (row-major,
/// n_frames × N_MFCC). Pools statistics that characterize the VOICE
/// across phonemes:
///   - mean of each coef (spectral envelope — WHO, roughly)
///   - std-dev of each coef (how much the voice MOVES — the seed of
///     singer / speed-talker / rhythm dynamics: a held note has low
///     variance in some coefs; rapid speech has a different variance
///     signature)
/// Returns an L2-normalized EMBED_DIM vector so cosine distance is
/// well-behaved. Empty/short input → a zero vector (callers treat a
/// zero embedding as "no usable signal").
pub fn embed_from_mfcc(mfcc: &[f32], n_frames: usize) -> Vec<f32> {
    if n_frames == 0 || mfcc.len() < n_frames * N_MFCC {
        return vec![0.0; EMBED_DIM];
    }
    let mut mean = vec![0.0f32; N_MFCC];
    for f in 0..n_frames {
        let base = f * N_MFCC;
        for c in 0..N_MFCC {
            mean[c] += mfcc[base + c];
        }
    }
    let inv_n = 1.0 / n_frames as f32;
    for c in 0..N_MFCC { mean[c] *= inv_n; }

    let mut var = vec![0.0f32; N_MFCC];
    for f in 0..n_frames {
        let base = f * N_MFCC;
        for c in 0..N_MFCC {
            let d = mfcc[base + c] - mean[c];
            var[c] += d * d;
        }
    }
    // Population std-dev (n_frames in the denominator). For n_frames==1
    // var is 0 → std 0, which is correct (a single frame has no spread).
    let mut emb = vec![0.0f32; EMBED_DIM];
    for c in 0..N_MFCC {
        emb[c] = mean[c];
        emb[N_MFCC + c] = (var[c] * inv_n).sqrt();
    }
    l2_normalize(&mut emb);
    emb
}

/// L2-normalize in place. A zero vector stays zero (no NaN). A
/// non-finite vector (NaN/inf from corrupt MFCC input) is zeroed —
/// v0.1.589: the `norm > 1e-12` check below is bypassed by a NaN norm
/// (NaN comparisons are false), which would leave NaN components in the
/// embedding and poison every downstream cosine distance. Zeroing makes
/// a garbage embedding behave as "no information" (distance 1.0 to
/// everything) rather than NaN.
fn l2_normalize(v: &mut [f32]) {
    let mut sum_sq = 0.0f32;
    for &x in v.iter() { sum_sq += x * x; }
    if !sum_sq.is_finite() {
        for x in v.iter_mut() { *x = 0.0; }
        return;
    }
    let norm = sum_sq.sqrt();
    if norm > 1e-12 {
        let inv = 1.0 / norm;
        for x in v.iter_mut() { *x *= inv; }
    }
}

/// Cosine distance between two embeddings: 0.0 (identical direction)
/// to 2.0 (opposite). For L2-normalized inputs this is 1 - dot.
/// Mismatched lengths or a zero vector → 1.0 (maximally "no
/// information", neither a match nor an anti-match).
pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() { return 1.0; }
    let mut dot = 0.0f32;
    let mut na = 0.0f32;
    let mut nb = 0.0f32;
    for i in 0..a.len() {
        dot += a[i] * b[i];
        na += a[i] * a[i];
        nb += b[i] * b[i];
    }
    if na < 1e-12 || nb < 1e-12 { return 1.0; }
    // v0.1.589: a NaN/inf component makes na/nb non-finite, and the
    // `< 1e-12` guard above does NOT catch it (NaN comparisons are
    // always false, inf passes). Without this, a corrupt embedding
    // (from glitched audio or a stale on-disk vector) yields a NaN
    // distance that poisons speaker identification — the same voice
    // "floor" class fixed in from_audio/similarity. Treat non-finite
    // accumulators as maximally-distant (1.0 = no match).
    if !na.is_finite() || !nb.is_finite() || !dot.is_finite() {
        return 1.0;
    }
    let cos = dot / (na.sqrt() * nb.sqrt());
    // cos can still be non-finite only if the inputs were (handled
    // above); clamp also defends the rare rounding case. NaN-safe:
    // re-check finiteness because clamp() passes NaN through.
    if !cos.is_finite() {
        return 1.0;
    }
    (1.0 - cos).clamp(0.0, 2.0)
}

/// Nearest candidate to `query` by cosine distance, plus the
/// ambiguity MARGIN (distance to 2nd-nearest minus distance to
/// nearest). A large margin = confident; a small margin = the query
/// sits between two candidates (ambiguous → caller should refuse to
/// guess, same principle as v0.1.496's identify guard, now in vector
/// space). Returns None for an empty candidate set.
///
/// Returns (best_index, best_distance, margin). If there's only one
/// candidate, margin is f32::INFINITY (nothing to be ambiguous with).
pub fn nearest_with_margin(query: &[f32], candidates: &[Vec<f32>])
    -> Option<(usize, f32, f32)>
{
    if candidates.is_empty() { return None; }
    let mut best_i = 0usize;
    let mut best_d = f32::INFINITY;
    let mut second_d = f32::INFINITY;
    for (i, c) in candidates.iter().enumerate() {
        let d = cosine_distance(query, c);
        if d < best_d {
            second_d = best_d;
            best_d = d;
            best_i = i;
        } else if d < second_d {
            second_d = d;
        }
    }
    let margin = if second_d.is_finite() { second_d - best_d } else { f32::INFINITY };
    Some((best_i, best_d, margin))
}

/// Batch cosine distance: one query against N centroids packed into a
/// single contiguous, row-major `centroids` buffer (N × dim). Writes
/// N distances into `out`.
///
/// ── GPU-acceleration seam (Vulkan/Mesa) ──────────────────────────
/// This is deliberately shaped as a GPU COMPUTE KERNEL would be:
///   - flat contiguous input (no Vec<Vec>, no pointer chasing),
///   - one independent output per work-item (centroid i → out[i]),
///   - NO data-dependent branches or early-exit in the inner loop
///     (GPUs suffer from branch divergence; this has none),
///   - pure arithmetic reduction (dot + two norms) per row.
/// As written it is cache-friendly and auto-vectorizes on CPU (fast
/// now, no dependency). When the corpus grows enough to justify it,
/// the SAME operation maps 1:1 onto a Vulkan compute shader: upload
/// query + centroids, dispatch one work-item per centroid, read back
/// `out`. The multi-speaker identity cost then becomes a single
/// batched GEMV-style dispatch instead of N CPU comparisons.
///
/// WHY NOT the DTW path (the current matcher): DTW is a sequential
/// dynamic-programming recurrence with a data-dependent
/// early-termination cutoff — the worst case for a GPU (no
/// vectorization across the recurrence, heavy branch divergence). It
/// stays threaded on CPU. The EMBEDDING distance below is the part
/// worth accelerating, which is exactly why the v0.1.498 vector
/// representation is the right direction for scaling multi-speaker.
///
/// Assumes all rows are `dim` long; if `centroids.len()` isn't a
/// multiple of `dim`, the trailing partial row is ignored. `out` is
/// resized to the number of complete rows.
pub fn batch_cosine_distance(query: &[f32], centroids: &[f32], dim: usize, out: &mut Vec<f32>) {
    out.clear();
    if dim == 0 || query.len() != dim { return; }
    let n = centroids.len() / dim;
    out.reserve(n);
    // Precompute the query norm once (loop-invariant).
    let mut qn = 0.0f32;
    for i in 0..dim { qn += query[i] * query[i]; }
    let qn = qn.sqrt();
    for row in 0..n {
        let base = row * dim;
        let c = &centroids[base..base + dim];
        // Per-work-item reduction — branchless inner loop.
        let mut dot = 0.0f32;
        let mut cn = 0.0f32;
        for i in 0..dim {
            dot += query[i] * c[i];
            cn += c[i] * c[i];
        }
        let denom = qn * cn.sqrt();
        // Single guarded division at the END (not in the hot loop).
        let d = if denom > 1e-12 { (1.0 - dot / denom).clamp(0.0, 2.0) } else { 1.0 };
        out.push(d);
    }
}

/// Update a running centroid embedding with a new observation, given
/// the prior sample count. Returns the new (centroid, count). This is
/// how a speaker's region accumulates: each accepted utterance pulls
/// the centroid, re-normalized. Incremental so a confirmed speaker's
/// model refines over time (the v0.1.497 "individual layer").
pub fn update_centroid(centroid: &[f32], count: u32, obs: &[f32]) -> (Vec<f32>, u32) {
    if centroid.len() != obs.len() || centroid.is_empty() {
        // First observation (or mismatch): adopt obs as the centroid.
        let mut c = obs.to_vec();
        l2_normalize(&mut c);
        return (c, count.max(1));
    }
    let n = count.max(1) as f32;
    let mut out = vec![0.0f32; centroid.len()];
    for i in 0..out.len() {
        out[i] = (centroid[i] * n + obs[i]) / (n + 1.0);
    }
    l2_normalize(&mut out);
    (out, count + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build an MFCC matrix where every frame is `frame`.
    fn const_mfcc(frames: usize, frame: [f32; N_MFCC]) -> Vec<f32> {
        let mut v = Vec::with_capacity(frames * N_MFCC);
        for _ in 0..frames { v.extend_from_slice(&frame); }
        v
    }

    #[test]
    fn embed_dim_and_normalized() {
        let m = const_mfcc(10, [1.0, 2.0, 3.0, 0.5, -1.0, 0.0, 0.2, 0.1, -0.3, 0.4, 0.0, 0.1, -0.2]);
        let e = embed_from_mfcc(&m, 10);
        assert_eq!(e.len(), EMBED_DIM);
        // L2 norm ~1 (non-zero input)
        let norm: f32 = e.iter().map(|x| x*x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-4, "embedding must be L2-normalized, got {}", norm);
    }

    #[test]
    fn nonfinite_inputs_never_produce_nan() {
        // v0.1.589: the recurring voice "floor" class, in the embedding
        // path. Corrupt MFCC (NaN/inf) must yield a FINITE embedding,
        // and a corrupt embedding must yield a FINITE cosine distance —
        // never NaN, which would poison speaker identification.
        let mut nan_frame = [1.0f32; N_MFCC];
        nan_frame[3] = f32::NAN;
        let e = embed_from_mfcc(&const_mfcc(8, nan_frame), 8);
        assert!(e.iter().all(|x| x.is_finite()),
            "embedding from NaN MFCC must be finite (zeroed), got {:?}", e);

        let mut inf_frame = [1.0f32; N_MFCC];
        inf_frame[0] = f32::INFINITY;
        let e2 = embed_from_mfcc(&const_mfcc(8, inf_frame), 8);
        assert!(e2.iter().all(|x| x.is_finite()), "inf MFCC must give finite embedding");

        // cosine_distance with a non-finite vector → 1.0 (no match), not NaN.
        let ok = vec![1.0f32, 2.0, 3.0];
        let d_nan = cosine_distance(&vec![f32::NAN, 1.0, 2.0], &ok);
        let d_inf = cosine_distance(&vec![f32::INFINITY, 1.0, 2.0], &ok);
        assert!(d_nan.is_finite() && (0.0..=2.0).contains(&d_nan), "NaN vec → finite dist, got {d_nan}");
        assert!(d_inf.is_finite() && (0.0..=2.0).contains(&d_inf), "inf vec → finite dist, got {d_inf}");
        assert_eq!(d_nan, 1.0, "corrupt embedding should read as maximally distant");
    }

    #[test]
    fn constant_frames_have_zero_std_half() {
        // Identical frames → std-dev half is all zeros (no spread).
        let frame = [1.0, 2.0, 3.0, 0.5, -1.0, 0.0, 0.2, 0.1, -0.3, 0.4, 0.0, 0.1, -0.2];
        let m = const_mfcc(8, frame);
        let e = embed_from_mfcc(&m, 8);
        for c in 0..N_MFCC {
            assert!(e[N_MFCC + c].abs() < 1e-5,
                "constant frames must give zero std for coef {}", c);
        }
    }

    #[test]
    fn identical_voices_distance_zero() {
        let frame = [1.0, 2.0, 3.0, 0.5, -1.0, 0.3, 0.2, 0.1, -0.3, 0.4, 0.0, 0.1, -0.2];
        let a = embed_from_mfcc(&const_mfcc(10, frame), 10);
        let b = embed_from_mfcc(&const_mfcc(10, frame), 10);
        assert!(cosine_distance(&a, &b) < 1e-5);
    }

    #[test]
    fn different_voices_are_far() {
        let voice_a = [1.0, 2.0, 3.0, 0.5, -1.0, 0.3, 0.2, 0.1, -0.3, 0.4, 0.0, 0.1, -0.2];
        // very different spectral envelope
        let voice_b = [1.0, -2.0, 1.0, 2.5, 1.0, -0.3, 0.9, -0.1, 0.3, -0.4, 0.5, -0.1, 0.2];
        let a = embed_from_mfcc(&const_mfcc(10, voice_a), 10);
        let b = embed_from_mfcc(&const_mfcc(10, voice_b), 10);
        let d = cosine_distance(&a, &b);
        assert!(d > 0.1, "different voices should be measurably apart, got {}", d);
    }

    #[test]
    fn dynamics_distinguish_steady_vs_variable() {
        // Same MEAN, different VARIANCE (a held/steady voice vs a
        // moving one) → the std half of the embedding separates them.
        // This is the seed of singer/speed-talker discrimination.
        let base = [1.0, 2.0, 3.0, 0.5, -1.0, 0.3, 0.2, 0.1, -0.3, 0.4, 0.0, 0.1, -0.2];
        let steady = embed_from_mfcc(&const_mfcc(10, base), 10);
        // variable: alternate frames high/low around the same mean
        let mut hi = base; hi[1] += 2.0;
        let mut lo = base; lo[1] -= 2.0;
        let mut varmfcc = Vec::new();
        for i in 0..10 { varmfcc.extend_from_slice(if i % 2 == 0 { &hi } else { &lo }); }
        let variable = embed_from_mfcc(&varmfcc, 10);
        let d = cosine_distance(&steady, &variable);
        assert!(d > 1e-3, "same mean but different dynamics must differ, got {}", d);
    }

    #[test]
    fn nearest_and_margin() {
        let q = vec![1.0, 0.0, 0.0];
        let close = vec![0.99, 0.14, 0.0];   // near q
        let far = vec![0.0, 0.0, 1.0];       // orthogonal
        let cands = vec![far.clone(), close.clone()];
        let (idx, dist, margin) = nearest_with_margin(&q, &cands).unwrap();
        assert_eq!(idx, 1, "the close candidate must win");
        assert!(dist < 0.05);
        assert!(margin > 0.5, "clear winner → large margin, got {}", margin);
    }

    #[test]
    fn ambiguous_query_small_margin() {
        // query equidistant between two candidates → tiny margin
        let q = vec![1.0, 1.0, 0.0];
        let a = vec![1.0, 0.0, 0.0];
        let b = vec![0.0, 1.0, 0.0];
        let (_, _, margin) = nearest_with_margin(&q, &vec![a, b]).unwrap();
        assert!(margin < 0.05, "equidistant → near-zero margin, got {}", margin);
    }

    #[test]
    fn single_candidate_infinite_margin() {
        let q = vec![1.0, 0.0, 0.0];
        let (_, _, margin) = nearest_with_margin(&q, &vec![vec![0.5, 0.5, 0.0]]).unwrap();
        assert!(margin.is_infinite(), "nothing to be ambiguous with → inf margin");
    }

    #[test]
    fn zero_and_mismatch_guards() {
        assert_eq!(cosine_distance(&[], &[]), 1.0);
        assert_eq!(cosine_distance(&[1.0, 0.0], &[1.0]), 1.0);
        assert_eq!(cosine_distance(&[0.0, 0.0], &[1.0, 0.0]), 1.0);
        // empty/short audio → zero embedding
        assert!(embed_from_mfcc(&[], 0).iter().all(|&x| x == 0.0));
        assert!(embed_from_mfcc(&[1.0; 5], 10).iter().all(|&x| x == 0.0)); // undersized
    }

    #[test]
    fn centroid_accumulates_toward_observations() {
        let v1 = vec![1.0, 0.0, 0.0];
        let v2 = vec![0.0, 1.0, 0.0];
        let (c1, n1) = update_centroid(&[], 0, &v1);
        assert_eq!(n1, 1);
        assert!(cosine_distance(&c1, &v1) < 1e-5, "first obs becomes centroid");
        let (c2, n2) = update_centroid(&c1, n1, &v2);
        assert_eq!(n2, 2);
        // centroid now between v1 and v2 — closer to neither extreme
        assert!(cosine_distance(&c2, &v1) > 0.0 && cosine_distance(&c2, &v2) > 0.0);
        assert!(cosine_distance(&c2, &v1) < cosine_distance(&v1, &v2),
            "centroid must sit between the observations");
    }

    #[test]
    fn batch_distance_matches_per_pair_reference() {
        // The GPU-shaped batch kernel MUST produce identical results
        // to the per-pair reference — the correctness oracle a future
        // Vulkan port validates against.
        let dim = 3;
        let q = vec![1.0, 0.5, -0.2];
        let c0 = vec![1.0, 0.5, -0.2];
        let c1 = vec![-1.0, -0.5, 0.2];
        let c2 = vec![0.0, 1.0, 0.0];
        let mut flat = Vec::new();
        for c in [&c0, &c1, &c2] { flat.extend_from_slice(c); }
        let mut out = Vec::new();
        batch_cosine_distance(&q, &flat, dim, &mut out);
        assert_eq!(out.len(), 3);
        assert!((out[0] - cosine_distance(&q, &c0)).abs() < 1e-6);
        assert!((out[1] - cosine_distance(&q, &c1)).abs() < 1e-6);
        assert!((out[2] - cosine_distance(&q, &c2)).abs() < 1e-6);
        assert!(out[0] < 1e-5, "identical centroid → ~0 distance");
    }

    #[test]
    fn batch_distance_guards() {
        let mut out = vec![9.9]; // must be cleared
        batch_cosine_distance(&[1.0], &[1.0], 0, &mut out);
        assert!(out.is_empty());
        batch_cosine_distance(&[1.0, 2.0], &[1.0, 2.0, 3.0], 3, &mut out);
        assert!(out.is_empty());
        let mut out2 = Vec::new();
        batch_cosine_distance(&[1.0, 0.0, 0.0], &[1.0,0.0,0.0, 0.0,1.0,0.0, 0.5], 3, &mut out2);
        assert_eq!(out2.len(), 2, "partial trailing row ignored");
    }
}
