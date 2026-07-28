//! cpuattn.rs — the rest of a decode step on the CPU: norm, and attention without a score buffer.
//!
//! Two lessons from llama.cpp's CPU path, both of which change the memory traffic rather than the
//! arithmetic — which is the only thing that matters at this bound.
//!
//! ## 1. Online softmax: never materialise the scores
//!
//! The textbook form computes every `q·k`, finds the max, exponentiates, sums, then weights the values.
//! That is three passes over a vector as long as the context and a buffer to hold it — at 8k context,
//! 32 KiB per head that is written, read, read again.
//!
//! The online form carries a running max and a running sum, rescaling the accumulator when a new max
//! appears. One pass, no buffer, and the same result to within float ordering. This is the same
//! recurrence flash-attention uses on the GPU; it matters more on the CPU because the buffer would
//! otherwise evict the K/V rows being streamed.
//!
//! ## 2. K is stored transposed
//!
//! `q·k` for position `p` needs `k[p][0..head_dim]` contiguous. Storing K as `[n_ctx][head_dim]` gives
//! that; storing it as `[head_dim][n_ctx]` — the shape the projection naturally produces — makes every
//! dot a strided gather. llama.cpp transposes on write for exactly this reason: once per token, rather
//! than a stride penalty on every score of every subsequent token.

/// RMS norm: `x * w / sqrt(mean(x^2) + eps)`.
///
/// Two passes over `d` floats, which is nothing beside the matvec that follows — no reason to fuse it
/// and lose the clarity.
pub fn rmsnorm(x: &[f32], w: &[f32], eps: f32, out: &mut [f32]) {
    let n = x.len();
    let mean_sq = x.iter().map(|v| v * v).sum::<f32>() / n as f32;
    let scale = 1.0 / (mean_sq + eps).sqrt();
    for i in 0..n { out[i] = x[i] * scale * w[i]; }
}

/// Single-head attention over a KV cache, in ONE pass and with no score buffer.
///
/// `k` and `v` are `[n_pos][head_dim]` — K already transposed on write, so each position's key is
/// contiguous and the dot below is a straight walk rather than a strided gather.
///
/// The recurrence: keep `m` (running max) and `s` (running sum of exp). On a new max, the accumulator
/// and sum are rescaled by `exp(old_m - new_m)` — which is where the numerical stability comes from,
/// and why this never exponentiates a large positive number.
pub fn attend(q: &[f32], k: &[f32], v: &[f32], n_pos: usize, head_dim: usize, scale: f32,
              out: &mut [f32]) {
    out.iter_mut().for_each(|o| *o = 0.0);
    if n_pos == 0 { return; }
    let (mut m, mut s) = (f32::NEG_INFINITY, 0f32);
    for p in 0..n_pos {
        let ko = p * head_dim;
        if ko + head_dim > k.len() { break; }
        let score: f32 = (0..head_dim).map(|i| q[i] * k[ko + i]).sum::<f32>() * scale;
        if score > m {
            // Rescale what we have to the new maximum. Doing it this way round means the exponent
            // argument is always <= 0, so nothing overflows however large the scores get.
            let corr = (m - score).exp();
            let corr = if m == f32::NEG_INFINITY { 0.0 } else { corr };
            for o in out.iter_mut() { *o *= corr; }
            s = s * corr + 1.0;
            m = score;
            let vo = p * head_dim;
            for i in 0..head_dim { out[i] += v[vo + i]; }
        } else {
            let e = (score - m).exp();
            s += e;
            let vo = p * head_dim;
            for i in 0..head_dim { out[i] += e * v[vo + i]; }
        }
    }
    let inv = 1.0 / s;
    for o in out.iter_mut() { *o *= inv; }
}

/// Write one position's key into the cache in the transposed layout.
///
/// Exists as a named function so the layout is stated once. Getting this wrong makes attention read
/// correct-looking values from the wrong positions — plausible output, no error.
pub fn write_kv(cache: &mut [f32], pos: usize, head_dim: usize, src: &[f32]) {
    let off = pos * head_dim;
    if off + head_dim > cache.len() { return; }
    cache[off..off + head_dim].copy_from_slice(&src[..head_dim]);
}

/// SwiGLU over a PACKED gate/up tensor: `silu(gate) * up`.
///
/// `gu` holds gate rows then up rows — `[2*ff, d]` — which is how llama.cpp and this loader both store
/// it, and why the matvec above produces both halves in one call. Splitting them into two tensors would
/// double the number of matvecs and halve the length of each, which is worse on every axis: more
/// dispatch overhead, shorter runs for the prefetcher, and the same bytes read.
///
/// `silu(x) = x / (1 + e^-x)`. Written out rather than called through a generic activation, because the
/// indirection would cost a call per element in a loop that runs ff times per layer.
pub fn swiglu(gu: &[f32], ff: usize, out: &mut [f32]) {
    for i in 0..ff {
        let g = gu[i];
        out[i] = (g / (1.0 + (-g).exp())) * gu[ff + i];
    }
}

/// Rotary embedding applied in place to one head, NeoX (split-half) pairing.
///
/// The pairing is the thing to get right: NeoX pairs `i` with `i + head_dim/2`, while the interleaved
/// variant pairs `2i` with `2i+1`. Both are "RoPE", both run without error, and the wrong one produces
/// fluent text that degrades with position — which is exactly the failure this project has spent days
/// chasing on the GPU side. `archcfg::rope_interleaved` already records which a model needs.
pub fn rope_neox(x: &mut [f32], head_dim: usize, pos: usize, base: f32) {
    let half = head_dim / 2;
    for i in 0..half {
        let freq = 1.0 / base.powf(2.0 * i as f32 / head_dim as f32);
        let (s, c) = (pos as f32 * freq).sin_cos();
        let (a, b) = (x[i], x[i + half]);
        x[i] = a * c - b * s;
        x[i + half] = a * s + b * c;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The textbook three-pass form, as the definition of correct.
    fn attend_reference(q: &[f32], k: &[f32], v: &[f32], n: usize, hd: usize, scale: f32) -> Vec<f32> {
        let scores: Vec<f32> = (0..n)
            .map(|p| (0..hd).map(|i| q[i] * k[p * hd + i]).sum::<f32>() * scale).collect();
        let m = scores.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        let ex: Vec<f32> = scores.iter().map(|s| (s - m).exp()).collect();
        let sum: f32 = ex.iter().sum();
        let mut out = vec![0f32; hd];
        for p in 0..n {
            for i in 0..hd { out[i] += ex[p] / sum * v[p * hd + i]; }
        }
        out
    }

    #[test]
    fn online_softmax_matches_the_three_pass_form() {
        // The whole point is doing it in one pass with no score buffer; it is only worth anything if it
        // agrees with the form that uses both.
        let (n, hd) = (37usize, 16usize);
        let q: Vec<f32> = (0..hd).map(|i| (i as f32 * 0.3) - 2.0).collect();
        let k: Vec<f32> = (0..n * hd).map(|i| ((i * 7 % 23) as f32) * 0.1 - 1.0).collect();
        let v: Vec<f32> = (0..n * hd).map(|i| ((i * 13 % 19) as f32) * 0.2 - 2.0).collect();
        let want = attend_reference(&q, &k, &v, n, hd, 0.25);
        let mut got = vec![0f32; hd];
        attend(&q, &k, &v, n, hd, 0.25, &mut got);
        for i in 0..hd {
            assert!((want[i] - got[i]).abs() < 1e-5, "lane {i}: {} vs {}", want[i], got[i]);
        }
    }

    #[test]
    fn a_late_maximum_rescales_correctly() {
        // The recurrence's hard case: the largest score arrives last, so everything accumulated before
        // it has to be scaled down. A version that only handled an early max would pass a monotonic
        // test and be wrong on real data.
        let hd = 4usize;
        let q = vec![1.0f32; hd];
        let mut k = vec![0.01f32; 3 * hd];
        for i in 0..hd { k[2 * hd + i] = 10.0; }          // position 2 dominates
        let v: Vec<f32> = (0..3 * hd).map(|i| i as f32).collect();
        let want = attend_reference(&q, &k, &v, 3, hd, 1.0);
        let mut got = vec![0f32; hd];
        attend(&q, &k, &v, 3, hd, 1.0, &mut got);
        for i in 0..hd { assert!((want[i] - got[i]).abs() < 1e-4, "{} vs {}", want[i], got[i]); }
        // and it really is dominated by the last position
        for i in 0..hd { assert!((got[i] - v[2 * hd + i]).abs() < 0.01); }
    }

    #[test]
    fn large_scores_do_not_overflow() {
        // Rescaling toward the running max means the exponent argument is always <= 0. The naive form
        // would exp(500) and produce inf, then NaN through the division.
        let hd = 4usize;
        let q = vec![100.0f32; hd];
        let k = vec![100.0f32; 2 * hd];
        let v: Vec<f32> = (0..2 * hd).map(|i| i as f32).collect();
        let mut got = vec![0f32; hd];
        attend(&q, &k, &v, 2, hd, 1.0, &mut got);
        assert!(got.iter().all(|x| x.is_finite()), "got {got:?}");
    }

    #[test]
    fn an_empty_cache_yields_zeros_rather_than_NaN() {
        // n_pos = 0 makes the sum zero and the reciprocal infinite. A NaN here would poison the whole
        // residual stream, and the first token of every sequence hits this path.
        let mut got = vec![9.0f32; 4];
        attend(&[1.0; 4], &[], &[], 0, 4, 1.0, &mut got);
        assert_eq!(got, vec![0.0; 4]);
    }

    #[test]
    fn rmsnorm_matches_its_definition() {
        let x = vec![1.0f32, -2.0, 3.0, -4.0];
        let w = vec![0.5f32, 1.0, 1.5, 2.0];
        let mut out = vec![0f32; 4];
        rmsnorm(&x, &w, 1e-6, &mut out);
        let ms = x.iter().map(|v| v * v).sum::<f32>() / 4.0;
        let sc = 1.0 / (ms + 1e-6).sqrt();
        for i in 0..4 { assert!((out[i] - x[i] * sc * w[i]).abs() < 1e-6); }
    }

    #[test]
    fn swiglu_reads_gate_and_up_from_the_packed_halves() {
        // Reading the halves the wrong way round produces silu(up)*gate, which is finite, plausible,
        // and wrong — no error anywhere.
        let ff = 3usize;
        let gu = vec![1.0f32, -2.0, 0.5,   10.0, 20.0, 30.0];   // gate, then up
        let mut out = vec![0f32; ff];
        swiglu(&gu, ff, &mut out);
        for i in 0..ff {
            let g = gu[i];
            let want = (g / (1.0 + (-g).exp())) * gu[ff + i];
            assert!((out[i] - want).abs() < 1e-6, "lane {i}: {} vs {want}", out[i]);
        }
        // silu is not the identity, so a swapped implementation would differ visibly
        assert!((out[0] - gu[ff]).abs() > 0.1, "silu(1.0) != 1.0");
    }

    #[test]
    fn rope_pairs_i_with_i_plus_half_not_adjacent_elements() {
        // NeoX pairs i with i+head_dim/2; the interleaved variant pairs 2i with 2i+1. Both are "RoPE",
        // both run without error, and the wrong one gives fluent text that decays with position — the
        // exact failure this project has chased on the GPU side.
        let hd = 4usize;
        let mut x = vec![1.0f32, 0.0, 0.0, 0.0];
        rope_neox(&mut x, hd, 1, 10000.0);
        // element 0 rotates against element 2 (0 + hd/2), so both move and 1/3 stay put
        assert!(x[0] != 1.0 && x[2] != 0.0, "0 and 2 are a pair: {x:?}");
        assert_eq!(x[1], 0.0, "1 is untouched by the 0/2 rotation");
        assert_eq!(x[3], 0.0);
    }

    #[test]
    fn rope_at_position_zero_is_the_identity() {
        // sin(0)=0, cos(0)=1. A version that got the sign or the pairing wrong usually fails here first,
        // and it is the cheapest possible check.
        let hd = 8usize;
        let orig: Vec<f32> = (0..hd).map(|i| i as f32 + 1.0).collect();
        let mut x = orig.clone();
        rope_neox(&mut x, hd, 0, 10000.0);
        for i in 0..hd { assert!((x[i] - orig[i]).abs() < 1e-6, "lane {i}"); }
    }

    #[test]
    fn rope_preserves_the_norm_of_each_pair() {
        // A rotation cannot change a magnitude. This catches a wrong frequency, a swapped sin/cos, and
        // a sign error — none of which the identity test at position 0 would see.
        let hd = 8usize;
        let mut x: Vec<f32> = (0..hd).map(|i| (i as f32) * 0.7 - 2.0).collect();
        let before: Vec<f32> = (0..hd / 2).map(|i| x[i] * x[i] + x[i + hd / 2] * x[i + hd / 2]).collect();
        rope_neox(&mut x, hd, 37, 10000.0);
        for i in 0..hd / 2 {
            let after = x[i] * x[i] + x[i + hd / 2] * x[i + hd / 2];
            assert!((before[i] - after).abs() < 1e-4, "pair {i}: {} vs {after}", before[i]);
        }
    }

    #[test]
    fn write_kv_places_a_position_contiguously() {
        // The layout the attention dot depends on. Getting it wrong makes attention read correct-looking
        // values from the WRONG positions — plausible output, no error, and nothing to grep for.
        let hd = 4usize;
        let mut cache = vec![0f32; 3 * hd];
        write_kv(&mut cache, 1, hd, &[7.0, 8.0, 9.0, 10.0]);
        assert_eq!(&cache[hd..2 * hd], &[7.0, 8.0, 9.0, 10.0]);
        assert!(cache[..hd].iter().all(|v| *v == 0.0), "other positions untouched");
        // and a write past the end is dropped rather than corrupting a neighbour
        write_kv(&mut cache, 99, hd, &[1.0; 4]);
        assert_eq!(cache.len(), 3 * hd);
    }
}
