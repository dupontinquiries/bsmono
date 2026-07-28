//! cpuops.rs — the scalar ops around the matvec, with the numerics llama.cpp gets right.
//!
//! These are small enough to look obvious and each has a failure mode that only appears on real data.
//! Written against the same reference the GPU kernels are: whatever `gguf`-loaded weights and the
//! shader path produce.

/// Softmax in place, max-subtracted.
///
/// **The max subtraction is not optional.** `exp(x)` overflows to +inf above ~88 in f32, and attention
/// logits routinely exceed that on long contexts — the sum then becomes inf, every probability becomes
/// NaN, and the whole row is lost. Subtracting the max makes the largest term exactly `exp(0) = 1` and
/// changes nothing mathematically, since `softmax(x) == softmax(x - c)` for any constant.
///
/// llama.cpp does this in `ggml_vec_soft_max_f32`; it is the single most common omission in a
/// hand-written attention.
pub fn softmax(x: &mut [f32]) {
    if x.is_empty() { return; }
    let max = x.iter().fold(f32::NEG_INFINITY, |m, v| m.max(*v));
    if !max.is_finite() {
        // An all -inf row (a fully masked position) would otherwise produce 0/0. Uniform is the
        // conventional answer and keeps the row summing to 1 rather than to NaN.
        let u = 1.0 / x.len() as f32;
        for v in x.iter_mut() { *v = u; }
        return;
    }
    let mut sum = 0f32;
    for v in x.iter_mut() { *v = (*v - max).exp(); sum += *v; }
    let inv = 1.0 / sum;
    for v in x.iter_mut() { *v *= inv; }
}

/// RMS norm: `x * w / sqrt(mean(x^2) + eps)`.
///
/// The sum of squares accumulates in f32 across the whole row. For d=8192 with activations of order 1
/// that is fine; the reason to say so is that accumulating in f16 — tempting when the weights are f16 —
/// would lose the tail of the sum and shift the norm.
pub fn rmsnorm(x: &[f32], w: &[f32], eps: f32, out: &mut [f32]) {
    let n = x.len();
    let ss: f32 = x.iter().map(|v| v * v).sum();
    let scale = 1.0 / (ss / n as f32 + eps).sqrt();
    for i in 0..n { out[i] = x[i] * scale * w[i]; }
}

/// SiLU: `x * sigmoid(x)`, the activation in every SwiGLU FFN here.
///
/// Computed as `x / (1 + exp(-x))`. For large negative `x`, `exp(-x)` overflows to inf and the result
/// is `x / inf = -0.0` — which is the correct limit, so no guard is needed. For large positive `x`,
/// `exp(-x)` underflows to 0 and the result is `x`, also correct. The naive form is safe here precisely
/// because both overflow directions land on the right answer.
#[inline]
pub fn silu(x: f32) -> f32 { x / (1.0 + (-x).exp()) }

/// Packed SwiGLU: `down_input[i] = silu(gate[i]) * up[i]`.
///
/// `gu` holds gate then up contiguously, which is how the packed `ffn_gate_up` tensor is laid out — so
/// this consumes it without an unpacking pass.
pub fn swiglu(gu: &[f32], ff: usize, out: &mut [f32]) {
    for i in 0..ff { out[i] = silu(gu[i]) * gu[ff + i]; }
}

/// Rotary embedding applied in place to one head's query or key vector.
///
/// NEOX/split-half pairing: element `i` pairs with `i + half`, not with `i + 1`. Getting this wrong
/// produces a model that is subtly incoherent rather than obviously broken — every token is plausible
/// and the long-range structure is gone — which is why the loader logs which pairing it chose.
pub fn rope_neox(v: &mut [f32], pos: usize, base: f32) {
    let half = v.len() / 2;
    for i in 0..half {
        let freq = 1.0 / base.powf(2.0 * i as f32 / v.len() as f32);
        let (s, c) = (pos as f32 * freq).sin_cos();
        let (a, b) = (v[i], v[i + half]);
        v[i] = a * c - b * s;
        v[i + half] = a * s + b * c;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn softmax_survives_logits_that_would_overflow_exp() {
        // THE failure this exists to prevent. exp(x) overflows above ~88 in f32, and attention logits
        // exceed that on long contexts — without max subtraction the sum becomes inf and every
        // probability becomes NaN, losing the entire row.
        let mut x = vec![900.0f32, 899.0, 100.0];
        softmax(&mut x);
        assert!(x.iter().all(|v| v.is_finite()), "no NaN from large logits: {x:?}");
        assert!((x.iter().sum::<f32>() - 1.0).abs() < 1e-5);
        assert!(x[0] > x[1] && x[1] > x[2], "and the ordering survives");
    }

    #[test]
    fn softmax_is_shift_invariant() {
        // softmax(x) == softmax(x - c). If it were not, the max subtraction would be changing the
        // answer rather than only the arithmetic.
        let base = vec![1.0f32, 2.0, 3.0, -1.0];
        let mut a = base.clone();
        let mut b: Vec<f32> = base.iter().map(|v| v + 50.0).collect();
        softmax(&mut a); softmax(&mut b);
        for (x, y) in a.iter().zip(&b) { assert!((x - y).abs() < 1e-6, "{a:?} vs {b:?}"); }
    }

    #[test]
    fn a_fully_masked_row_is_uniform_not_NaN() {
        // All -inf happens on a fully masked attention position. exp gives all zeros, the sum is zero,
        // and the division is 0/0 — a NaN that then propagates through the whole head.
        let mut x = vec![f32::NEG_INFINITY; 4];
        softmax(&mut x);
        assert!(x.iter().all(|v| v.is_finite()), "{x:?}");
        assert!((x.iter().sum::<f32>() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn rmsnorm_matches_the_definition() {
        let x = vec![3.0f32, 4.0, 0.0, 0.0];
        let w = vec![1.0f32; 4];
        let mut out = vec![0f32; 4];
        rmsnorm(&x, &w, 0.0, &mut out);
        let rms = (25.0f32 / 4.0).sqrt();
        for i in 0..4 { assert!((out[i] - x[i] / rms).abs() < 1e-5, "{out:?}"); }
    }

    #[test]
    fn rmsnorm_eps_prevents_a_division_by_zero_on_an_all_zero_row() {
        let x = vec![0.0f32; 8];
        let w = vec![1.0f32; 8];
        let mut out = vec![9.0f32; 8];
        rmsnorm(&x, &w, 1e-6, &mut out);
        assert!(out.iter().all(|v| v.is_finite() && *v == 0.0), "{out:?}");
    }

    #[test]
    fn silu_is_correct_at_both_overflow_extremes() {
        // x / (1 + exp(-x)) looks unsafe and is not: large negative x gives x/inf = -0.0, the correct
        // limit; large positive gives x/1 = x, also correct. Worth pinning so nobody "fixes" it into
        // a branchy version that is slower and no more correct.
        assert!(silu(-1000.0).is_finite() && silu(-1000.0).abs() < 1e-30, "{}", silu(-1000.0));
        assert!((silu(1000.0) - 1000.0).abs() < 1e-3);
        assert!((silu(0.0)).abs() < 1e-9);
        assert!((silu(1.0) - 0.7310586).abs() < 1e-6);
    }

    #[test]
    fn swiglu_consumes_the_packed_gate_up_layout() {
        // gate then up contiguously, which is how ffn_gate_up is stored — so it needs no unpacking pass.
        let gu = vec![1.0f32, 2.0, /* up: */ 10.0, 20.0];
        let mut out = vec![0f32; 2];
        swiglu(&gu, 2, &mut out);
        assert!((out[0] - silu(1.0) * 10.0).abs() < 1e-6);
        assert!((out[1] - silu(2.0) * 20.0).abs() < 1e-6);
    }

    #[test]
    fn rope_pairs_i_with_i_plus_HALF_not_i_plus_one() {
        // NEOX/split-half. The wrong pairing produces a model that is subtly incoherent rather than
        // obviously broken — every token plausible, long-range structure gone — which is the hardest
        // kind of wrong to notice, and why the loader logs which pairing it picked.
        let mut v = vec![1.0f32, 0.0, 0.0, 0.0];
        rope_neox(&mut v, 1, 10000.0);
        // element 0 rotates against element 2 (half = 2), so both move and 1/3 stay put
        assert!(v[0] != 1.0 && v[2] != 0.0, "0 and 2 are a pair: {v:?}");
        assert_eq!(v[1], 0.0);
        assert_eq!(v[3], 0.0);
    }

    #[test]
    fn rope_at_position_zero_is_the_identity() {
        // A rotation of zero radians. If this moved anything, every prompt's first token would be
        // wrong in a way that looks like a bad model rather than a bad rope.
        let orig = vec![0.3f32, -0.7, 1.1, 2.0];
        let mut v = orig.clone();
        rope_neox(&mut v, 0, 10000.0);
        for (a, b) in v.iter().zip(&orig) { assert!((a - b).abs() < 1e-6, "{v:?} vs {orig:?}"); }
    }
}
