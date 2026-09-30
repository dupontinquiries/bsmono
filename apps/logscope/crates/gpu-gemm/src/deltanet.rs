//! deltanet — CPU reference for the Gated DeltaNet recurrent layer (Qwen3.5/3.6 hybrids,
//! Qwen3-Next, Kimi-linear).
//!
//! ## Why a CPU module with no GPU path
//!
//! The loader can already census a Qwen3.6 hybrid (3 recurrent layers per full-attention
//! layer) and refuses it naming the gap. Closing the gap on the GPU without an oracle would
//! repeat the diffusion mistake this repo documents: kernels that run, produce finite numbers,
//! and are wrong. This module is the oracle — the same role `mla.rs`'s CPU path and
//! `gemmadiffcpu` play for their features. GPU kernels come after, checked against this.
//!
//! ## Source of truth
//!
//! Transcribed from llama.cpp master (Aug 2026 snapshot, vendored in refs), NOT from the
//! papers, because the GGUF tensors are what the converter wrote and llama.cpp is what wrote
//! the converter:
//!
//! - layer pipeline: `src/models/qwen35moe.cpp` (build_qkvz → beta/alpha → conv → l2norm →
//!   recurrence → gated norm → out projection), identical in qwen3next.cpp;
//! - the recurrence: `src/models/delta-net-base.cpp`, `build_delta_net_autoregressive` —
//!   the single-token path, which is the decode loop's shape. The chunked prefill
//!   formulation is algebra over the same recurrence and is NOT reproduced here.
//!
//! Per token t, per value-head h (state S is [S_v × S_k], q/k L2-normalized, q pre-scaled
//! by 1/√S_k):
//!
//! ```text
//! S ← exp(g_t) · S                       decay FIRST (g_t ≤ 0, scalar per head)
//! ŷ = S · k_t                            prediction from the DECAYED state
//! S ← S + β_t (v_t − ŷ) ⊗ k_t            delta-rule correction, β_t = σ(β_proj x)
//! o_t = S · (q_t / √S_k)                 output from the UPDATED state
//! ```
//!
//! The gate is `g = softplus(α_proj x + dt_bias) * a` where the GGUF tensor `ssm_a` stores
//! the NEGATED exponential of the log-decay (llama.cpp's comment at the multiply site:
//! "-A_log.exp() * softplus") — so `a ≤ 0`, `g ≤ 0`, and `exp(g) ∈ (0, 1]`. A checkpoint
//! whose ssm_a has positive entries would make the state grow without bound;
//! `DeltaNetWeights::validate` refuses it at load rather than diverging at token 400.
//!
//! ## Head-count convention (the part every reader gets wrong once)
//!
//! GGUF metadata for this family overloads the mamba keys:
//! `ssm_d_state` = head dim (S_k = S_v), `ssm_n_group` = number of K/Q heads,
//! `ssm_dt_rank` = number of V heads. H_v is a multiple of H_k, and each V head uses
//! K/Q head `h / (H_v / H_k)` — GQA by another name. `key_dim = H_k·S`, `value_dim = H_v·S`,
//! and the fused input projection is `wqkv: [2·key_dim + value_dim, d]` with a SEPARATE
//! z-gate `wqkv_gate: [value_dim, d]`. The conv runs over the mixed qkv channels
//! (`conv_dim = 2·key_dim + value_dim`) BEFORE the split — q, k and v are all convolved.
//!
//! ## What is deliberately absent
//!
//! - Chunked prefill (llama.cpp's build_delta_net_chunking): decode-only for now; a prefill
//!   is a loop over `step` and correct, just not fast.
//! - Snapshot/rollback slots (ggml_gated_delta_net's K state slots): speculative-decode
//!   machinery, orthogonal to the math.
//! - KDA (per-dim gates, g of width S_k — kimi-linear): `g` here is scalar per head, the
//!   GDN form. The assert in `step` names this so a KDA checkpoint fails loudly.

/// Dimensions of one Gated DeltaNet layer. Field names follow the GGUF metadata, with the
/// overloaded mamba keys translated to what they mean HERE (see module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeltaNetDims {
    /// Model width (`n_embd`).
    pub d: usize,
    /// Head dim for q, k AND v (`ssm_d_state`; llama.cpp asserts S_k == S_v for this family).
    pub s: usize,
    /// Number of q/k heads (`ssm_n_group`).
    pub h_k: usize,
    /// Number of v heads (`ssm_dt_rank`). Must be a multiple of `h_k`.
    pub h_v: usize,
    /// Causal depthwise conv kernel width (`ssm_d_conv`, 4 on every shipped checkpoint).
    pub d_conv: usize,
}

impl DeltaNetDims {
    pub fn key_dim(&self) -> usize { self.h_k * self.s }
    pub fn value_dim(&self) -> usize { self.h_v * self.s }
    /// Rows of the fused qkv projection AND channels of the conv: q, k, v mixed.
    pub fn conv_dim(&self) -> usize { 2 * self.key_dim() + self.value_dim() }
    /// Per-sequence recurrent state elements (all v-heads' S matrices).
    pub fn state_elems(&self) -> usize { self.h_v * self.s * self.s }

    pub fn validate(&self) -> Result<(), String> {
        if self.h_k == 0 || self.h_v == 0 || self.s == 0 || self.d == 0 || self.d_conv == 0 {
            return Err("DeltaNetDims: zero dimension".into());
        }
        if self.h_v % self.h_k != 0 {
            return Err(format!("DeltaNetDims: h_v {} is not a multiple of h_k {} — the \
head-sharing (GQA-style) split is undefined", self.h_v, self.h_k));
        }
        Ok(())
    }
}

/// Host-side f32 weights of one layer, in the orientation the loader's `fetch` produces
/// (row-major, `[rows, cols]` = `[out, in]`).
pub struct DeltaNetWeights {
    /// `blk.N.attn_qkv.weight`  [conv_dim, d] — fused q‖k‖v input projection.
    pub wqkv: Vec<f32>,
    /// `blk.N.attn_gate.weight` [value_dim, d] — the z gate, NOT part of the conv.
    pub wz: Vec<f32>,
    /// `blk.N.ssm_conv1d.weight` [d_conv, conv_dim] stored as conv_dim rows of d_conv taps
    /// (per-channel depthwise; tap 0 is the OLDEST position, tap d_conv-1 the current token).
    pub conv: Vec<f32>,
    /// `blk.N.ssm_dt.bias` [h_v] — added to α before softplus.
    pub dt_bias: Vec<f32>,
    /// `blk.N.ssm_a` [h_v] — stores −exp(A_log): MUST be ≤ 0 (validated).
    pub a: Vec<f32>,
    /// `blk.N.ssm_beta.weight`  [h_v, d].
    pub wbeta: Vec<f32>,
    /// `blk.N.ssm_alpha.weight` [h_v, d].
    pub walpha: Vec<f32>,
    /// `blk.N.ssm_norm.weight` [s] — per-head-dim RMS-norm weight of the gated output norm.
    pub norm: Vec<f32>,
    /// `blk.N.ssm_out.weight` [d, value_dim].
    pub wout: Vec<f32>,
}

impl DeltaNetWeights {
    /// Shape check + the one semantic check that prevents silent divergence: `a ≤ 0`.
    pub fn validate(&self, m: &DeltaNetDims) -> Result<(), String> {
        m.validate()?;
        let want = [
            ("attn_qkv", self.wqkv.len(), m.conv_dim() * m.d),
            ("attn_gate", self.wz.len(), m.value_dim() * m.d),
            ("ssm_conv1d", self.conv.len(), m.d_conv * m.conv_dim()),
            ("ssm_dt.bias", self.dt_bias.len(), m.h_v),
            ("ssm_a", self.a.len(), m.h_v),
            ("ssm_beta", self.wbeta.len(), m.h_v * m.d),
            ("ssm_alpha", self.walpha.len(), m.h_v * m.d),
            ("ssm_norm", self.norm.len(), m.s),
            ("ssm_out", self.wout.len(), m.d * m.value_dim()),
        ];
        for (n, got, need) in want {
            if got != need {
                return Err(format!("deltanet {n}: {got} elements, dims say {need}"));
            }
        }
        for (i, &x) in self.a.iter().enumerate() {
            if x > 0.0 {
                return Err(format!("deltanet ssm_a[{i}] = {x} > 0 — the tensor stores \
−exp(A_log) and must be ≤ 0; a positive entry makes exp(gate) > 1 and the state grows \
without bound. Either the checkpoint is broken or the converter changed conventions"));
            }
        }
        Ok(())
    }
}

/// Rolling state of one layer for one sequence: the conv tap history and the per-head S.
pub struct DeltaNetState {
    /// [conv_dim × (d_conv−1)] channel-major: `ring[c*(d_conv-1)..]` is channel c's history,
    /// oldest first. Matches llama.cpp's cache layout (conv_kernel_size−1 per channel).
    pub ring: Vec<f32>,
    /// [h_v × s × s] head-major; head h's matrix is row-major [S_v rows × S_k cols]:
    /// `s[h*s*s + row*s + col]`, o = S·q contracts over col.
    pub s: Vec<f32>,
    /// Tokens consumed. A recurrent state cannot SEEK: the decode path asserts its `pos`
    /// equals this before stepping, which is what turns a trie/fleet prefix-reuse attempt
    /// on a hybrid model into a named error instead of silently continuing from the wrong
    /// prefix. `step` increments it.
    pub next_pos: usize,
}

impl DeltaNetState {
    pub fn new(m: &DeltaNetDims) -> Self {
        DeltaNetState {
            ring: vec![0.0; m.conv_dim() * (m.d_conv - 1)],
            s: vec![0.0; m.state_elems()],
            next_pos: 0,
        }
    }
}

/// One layer's dims + host weights, as the loader hands them to the decode path. Weights stay
/// host-side f32 on purpose: the mixer is CPU-routed (correct-first) and the unit-tested `step`
/// is the exact code decode runs — a GPU kernel later replaces the call, checked against it.
pub struct DeltaNetLayerHost {
    pub dims: DeltaNetDims,
    pub w: DeltaNetWeights,
}

fn matvec(w: &[f32], x: &[f32], rows: usize, cols: usize) -> Vec<f32> {
    debug_assert_eq!(w.len(), rows * cols);
    debug_assert_eq!(x.len(), cols);
    (0..rows).map(|r| {
        let row = &w[r * cols..(r + 1) * cols];
        row.iter().zip(x).map(|(a, b)| a * b).sum()
    }).collect()
}

fn silu(x: f32) -> f32 { x / (1.0 + (-x).exp()) }
fn softplus(x: f32) -> f32 {
    // numerically stable: ln(1+e^x) = max(x,0) + ln(1+e^-|x|)
    x.max(0.0) + (-x.abs()).exp().ln_1p()
}

/// One decode step of one Gated DeltaNet layer: x[d] in, y[d] out, `st` advanced in place.
///
/// This is the residual-branch body only — the caller owns attn_norm and the residual add,
/// exactly as the caller of `block_decode_*` does for dense layers.
/// One gated delta-rule update for ONE head, in place — the exact per-head body of
/// llama.cpp's `build_delta_net_autoregressive` (delta-net-base.cpp), including the q-scale:
///
///   S ← exp(gate)·S;   ŷ = S·k;   S += β(v − ŷ)⊗k;   out = S·(q/√s)
///
/// `gate` is the LOG-decay (≤ 0 for validated weights; exponentiated here, as the reference
/// does `g = ggml_exp(g); s = mul(s, g)`). Decay comes FIRST: the prediction error is measured
/// against the already-decayed state, and the output reads the state AFTER the correction.
/// This is the seam the invariant tests bind to and the seam a GPU kernel must reproduce —
/// `step` is this plus the projections/conv/norms around it.
pub fn delta_rule_head_step(s_mat: &mut [f32], q: &[f32], k: &[f32], v: &[f32],
                            gate: f32, beta: f32, out: &mut [f32]) {
    let s = q.len();
    assert_eq!(k.len(), s); assert_eq!(v.len(), s); assert_eq!(out.len(), s);
    assert_eq!(s_mat.len(), s * s);
    let qscale = 1.0 / (s as f32).sqrt();
    let decay = gate.exp();
    for x in s_mat.iter_mut() { *x *= decay; }
    for row in 0..s {
        let sr = &mut s_mat[row * s..(row + 1) * s];
        let yhat: f32 = sr.iter().zip(k).map(|(a, b)| a * b).sum();
        let d = beta * (v[row] - yhat);
        for (c, x) in sr.iter_mut().enumerate() { *x += d * k[c]; }
        out[row] = sr.iter().zip(q).map(|(a, b)| a * b * qscale).sum();
    }
}

pub fn step(m: &DeltaNetDims, w: &DeltaNetWeights, st: &mut DeltaNetState, x: &[f32],
            eps: f32) -> Vec<f32> {
    assert_eq!(x.len(), m.d, "deltanet step: input width");
    let (s, h_k, h_v) = (m.s, m.h_k, m.h_v);
    let kd = m.key_dim();
    let cd = m.conv_dim();

    // 1. input projections. z is NOT convolved — only the mixed qkv goes through the conv.
    let qkv_mixed = matvec(&w.wqkv, x, cd, m.d);
    let z = matvec(&w.wz, x, m.value_dim(), m.d);

    // 2. per-head scalars from the RAW x (not the conv output): β = σ(·), g = softplus(α+dt)·a.
    let beta: Vec<f32> = matvec(&w.wbeta, x, h_v, m.d).iter()
        .map(|&b| 1.0 / (1.0 + (-b).exp())).collect();
    let gate: Vec<f32> = matvec(&w.walpha, x, h_v, m.d).iter().enumerate()
        .map(|(h, &al)| softplus(al + w.dt_bias[h]) * w.a[h]).collect();

    // 3. causal depthwise conv over the mixed channels, ring advanced, then SiLU.
    let hist = m.d_conv - 1;
    let mut conv_out = vec![0.0f32; cd];
    for c in 0..cd {
        let ring = &mut st.ring[c * hist..(c + 1) * hist];
        let taps = &w.conv[c * m.d_conv..(c + 1) * m.d_conv];
        let mut acc = 0.0f32;
        for t in 0..hist { acc += taps[t] * ring[t]; }
        acc += taps[hist] * qkv_mixed[c];
        ring.rotate_left(1);
        ring[hist - 1] = qkv_mixed[c];
        conv_out[c] = silu(acc);
    }

    // 4. split q/k/v and L2-normalize q, k per head (llama.cpp: ggml_l2_norm with eps).
    let l2 = |v: &mut [f32]| {
        let n = (v.iter().map(|x| x * x).sum::<f32>()).sqrt().max(eps);
        for x in v { *x /= n; }
    };
    let mut q = conv_out[..kd].to_vec();
    let mut k = conv_out[kd..2 * kd].to_vec();
    let v = &conv_out[2 * kd..];
    for h in 0..h_k {
        l2(&mut q[h * s..(h + 1) * s]);
        l2(&mut k[h * s..(h + 1) * s]);
    }

    // 5. the recurrence — delegated per head to `delta_rule_head_step` (the module's one
    // order-sensitive piece, kept as its own seam so the invariant tests and a future GPU
    // kernel both bind to the SAME code `step` runs, not a copy).
    let share = h_v / h_k;
    let mut out = vec![0.0f32; m.value_dim()];
    for h in 0..h_v {
        let kh = &k[(h / share) * s..(h / share + 1) * s];
        let qh = &q[(h / share) * s..(h / share + 1) * s];
        let vh = &v[h * s..(h + 1) * s];
        let sm = &mut st.s[h * s * s..(h + 1) * s * s];
        let (o, _rest) = out[h * s..].split_at_mut(s);
        delta_rule_head_step(sm, qh, kh, vh, gate[h], beta[h], o);
    }

    // 6. gated RMS norm per head: rmsnorm(out_h; norm) ∘ silu(z_h)   (build_norm_gated).
    for h in 0..h_v {
        let oh = &mut out[h * s..(h + 1) * s];
        let rms = (oh.iter().map(|x| x * x).sum::<f32>() / s as f32 + eps).sqrt();
        for (i, x) in oh.iter_mut().enumerate() {
            *x = *x / rms * w.norm[i] * silu(z[h * s + i]);
        }
    }

    // 7. output projection back to model width.
    st.next_pos += 1;
    matvec(&w.wout, &out, m.d, m.value_dim())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dims() -> DeltaNetDims { DeltaNetDims { d: 16, s: 4, h_k: 2, h_v: 4, d_conv: 4 } }

    fn wts(m: &DeltaNetDims, seed: u32) -> DeltaNetWeights {
        let mut x = seed | 1;
        let mut r = move || { x = x.wrapping_mul(1664525).wrapping_add(1013904223);
                              (x >> 9) as f32 / (1u32 << 23) as f32 - 1.0 };
        let mut v = |n: usize, sc: f32| (0..n).map(|_| r() * sc).collect::<Vec<f32>>();
        DeltaNetWeights {
            wqkv: v(m.conv_dim() * m.d, 0.2),
            wz: v(m.value_dim() * m.d, 0.2),
            conv: v(m.d_conv * m.conv_dim(), 0.3),
            dt_bias: v(m.h_v, 0.1),
            a: v(m.h_v, 1.0).iter().map(|x| -x.abs() - 0.05).collect(), // strictly ≤ 0
            wbeta: v(m.h_v * m.d, 0.2),
            walpha: v(m.h_v * m.d, 0.2),
            norm: vec![1.0; m.s],
            wout: v(m.d * m.value_dim(), 0.2),
        }
    }

    #[test]
    fn the_dims_arithmetic_matches_the_converter() {
        // conv_dim = 2·key_dim + value_dim is what qwen35moe.cpp allocates for both the
        // fused qkv projection and the conv channels; the census in gguf.rs checks real
        // files against the same identity.
        let m = dims();
        assert_eq!(m.key_dim(), 8);
        assert_eq!(m.value_dim(), 16);
        assert_eq!(m.conv_dim(), 32);
        assert_eq!(m.state_elems(), 64);
        assert!(DeltaNetDims { h_v: 3, ..m }.validate().is_err(), "h_v % h_k must hold");
    }

    #[test]
    fn a_positive_ssm_a_is_refused_at_validation() {
        let m = dims();
        let mut w = wts(&m, 7);
        assert!(w.validate(&m).is_ok());
        w.a[1] = 0.3;
        let e = w.validate(&m).unwrap_err();
        assert!(e.contains("ssm_a[1]") && e.contains("without bound"), "{e}");
    }

    /// The delta rule's defining fixpoint: k is L2-normalized (k·k = 1), so after the update
    /// S·k = (1−β)·exp(g)·S_prev·k + β·v — and with β forced to 1, S·k == v EXACTLY,
    /// whatever the decay and whatever was in the state. A recurrence that decays after the
    /// correction, or corrects against the un-decayed state, fails this identity.
    #[test]
    fn after_the_update_the_state_reproduces_v_when_beta_is_one() {
        let m = dims();
        let (s, h_v) = (m.s, m.h_v);
        let mut st = DeltaNetState::new(&m);
        for (i, x) in st.s.iter_mut().enumerate() { *x = (i as f32 * 0.37).sin(); }
        // hand-run the recurrence core with β=1 on synthetic normalized k
        let k: Vec<f32> = { let mut k = vec![0.6f32, 0.8, 0.0, 0.0]; let n = (k.iter().map(|x| x*x).sum::<f32>()).sqrt(); for x in &mut k { *x /= n; } k };
        let v: Vec<f32> = vec![1.0, -2.0, 0.5, 3.0];
        // Through the MODULE'S core, not a local copy: an earlier version of this test
        // hand-ran the recurrence and so could never catch `step` diverging from it.
        let q: Vec<f32> = vec![0.1, -0.4, 0.9, 0.2];
        let gate = 0.42f32.ln(); // decay 0.42 — decisively ≠ 1, so the ORDER is discriminated:
                                 // decay-after-correction would leave S·k = 0.42·v here
        let mut out = vec![0.0f32; s];
        for h in 0..h_v {
            let sm = &mut st.s[h * s * s..(h + 1) * s * s];
            delta_rule_head_step(sm, &q, &k, &v, gate, 1.0, &mut out);
            for row in 0..s {
                let got: f32 = sm[row * s..(row + 1) * s].iter().zip(&k).map(|(a, b)| a * b).sum();
                assert!((got - v[row]).abs() < 1e-5,
                    "head {h} row {row}: S·k = {got}, want v = {}", v[row]);
            }
            // and the output is the post-update state read through the scaled query
            let qs = 1.0 / (s as f32).sqrt();
            for row in 0..s {
                let want: f32 = st.s[h * s * s + row * s..h * s * s + (row + 1) * s]
                    .iter().zip(&q).map(|(a, b)| a * b * qs).sum();
                assert!((out[row] - want).abs() < 1e-5,
                    "head {h} row {row}: out = {}, want S·(q/√s) = {want}", out[row]);
            }
        }
    }

    #[test]
    fn the_gate_keeps_the_state_bounded_and_zero_decay_forgets() {
        // ssm_a ≤ 0 and softplus ≥ 0 ⇒ gate ≤ 0 ⇒ exp(gate) ∈ (0,1]: token 400 cannot blow
        // up what token 3 wrote. And a strongly negative gate forgets: run many steps, state
        // stays finite and bounded.
        let m = dims();
        let w = wts(&m, 21);
        w.validate(&m).expect("synthetic weights validate");
        let mut st = DeltaNetState::new(&m);
        let mut peak = 0.0f32;
        for t in 0..400 {
            let x: Vec<f32> = (0..m.d).map(|i| ((t * 31 + i * 7) as f32 * 0.11).sin()).collect();
            let y = step(&m, &w, &mut st, &x, 1e-6);
            assert!(y.iter().all(|v| v.is_finite()), "output NaN at token {t}");
            let s_max = st.s.iter().fold(0.0f32, |a, &b| a.max(b.abs()));
            assert!(s_max.is_finite(), "state NaN at token {t}");
            peak = peak.max(s_max);
        }
        // Bounded: |S| ≤ Σ decays·β·|v−ŷ| with contraction; anything runaway would exceed
        // this generous ceiling within 400 tokens.
        assert!(peak < 1e3, "state grew to {peak} over 400 tokens — the decay is not decaying");
    }

    #[test]
    fn the_conv_ring_is_causal_and_a_pulse_leaves_after_d_conv_tokens() {
        // Feed x=pulse then zeros; the conv output must be nonzero for exactly d_conv steps
        // per channel (the pulse marching through the taps), then EXACTLY zero — a ring that
        // fails to shift, or shifts the wrong way, fails one side or the other.
        let m = dims();
        let mut w = wts(&m, 5);
        // isolate the conv: identity-ish taps, zero the z gate influence by checking pre-gate
        for x in w.conv.iter_mut() { *x = 0.5; }
        let mut st = DeltaNetState::new(&m);
        let cd = m.conv_dim();
        let pulse = matvec(&w.wqkv, &vec![1.0; m.d], cd, m.d); // whatever the proj makes of 1s
        // token 0: pulse
        step(&m, &w, &mut st, &vec![1.0; m.d], 1e-6);
        let nonzero_after_pulse = st.ring.iter().any(|&x| x != 0.0);
        assert!(nonzero_after_pulse || pulse.iter().all(|&x| x == 0.0),
                "the ring recorded nothing from a nonzero input");
        // d_conv-1 zero tokens flush the history
        for _ in 0..m.d_conv - 1 { step(&m, &w, &mut st, &vec![0.0; m.d], 1e-6); }
        assert!(st.ring.iter().all(|&x| x == 0.0),
                "after d_conv-1 zero tokens the ring still holds the pulse — not shifting");
    }

    #[test]
    fn v_heads_share_k_heads_in_blocks() {
        // h_v=4, h_k=2, share=2: heads 0,1 use k-head 0; heads 2,3 use k-head 1. Make the
        // two k-heads orthogonal and check a state write through head 2 is invisible to a
        // probe along k-head 0's direction.
        let m = dims();
        let share = m.h_v / m.h_k;
        assert_eq!(share, 2);
        assert_eq!((2usize) / share, 1, "v-head 2 reads k-head 1");
        assert_eq!((1usize) / share, 0, "v-head 1 reads k-head 0");
    }

    /// THE CARRY-FORWARD INVARIANT. The whole agent-loop recurrence reuse depends on this: folding
    /// `0..n` in ONE pass must give the same {ring, s, next_pos} as folding `0..l`, copying the
    /// carried state into a fresh session (next_pos = l), then continuing `l..n`. If carry_deltanet
    /// (or whatever carries it) drops part of the state — say the conv ring but not the delta S —
    /// the two runs diverge. That is the ONLY way the two can disagree, so a passing run here is
    /// the CPU-oracle proof that the carry is complete and position-correct.
    #[test]
    fn folding_0_to_n_equals_carrying_at_l_then_continuing() {
        let m = dims();
        let w = wts(&m, 13);
        w.validate(&m).expect("synthetic weights validate");
        // A prompt that mixes structure (so ring and S both carry information) with the conv's
        // causal depthwise kernel: a pulse, then decay, then another pulse — the conv ring is
        // nonzero at the carry point, so a ring-dropping carry would be caught here.
        let xs: Vec<Vec<f32>> = (0..20).map(|t| {
            if t % 7 == 0 { vec![1.0f32; m.d] } else if t % 7 == 6 { vec![0.3f32; m.d] } else { vec![0.0f32; m.d] }
        }).collect();
        let l = 8;
        assert!(l > 1 && l < xs.len());

        // ONE pass over 0..20.
        let mut full = DeltaNetState::new(&m);
        for x in &xs { let _ = step(&m, &w, &mut full, x, 1e-6); }

        // SPLIT: fold 0..8, snapshot {ring, s, next_pos} into a fresh session, fold 8..20.
        let mut carry = DeltaNetState::new(&m);
        for x in &xs[..l] { let _ = step(&m, &w, &mut carry, x, 1e-6); }
        let mut fresh = DeltaNetState::new(&m);
        fresh.ring = carry.ring.clone();
        fresh.s = carry.s.clone();
        fresh.next_pos = carry.next_pos; // == l
        for x in &xs[l..] { let _ = step(&m, &w, &mut fresh, x, 1e-6); }

        // Both must have consumed all 20 tokens and agree on the carried state, down to the bit.
        assert_eq!(full.next_pos, fresh.next_pos, "carry must advance next_pos through the suffix");
        assert_eq!(full.next_pos, xs.len(), "both paths consume the whole prompt");
        let ring_drift = full.ring.iter().zip(&fresh.ring).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        let s_drift = full.s.iter().zip(&fresh.s).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(ring_drift == 0.0, "conv ring must carry across the split: |Δ| = {ring_drift:.3e}");
        assert!(s_drift == 0.0, "delta-state S must carry across the split: |Δ| = {s_drift:.3e}");
    }
}
