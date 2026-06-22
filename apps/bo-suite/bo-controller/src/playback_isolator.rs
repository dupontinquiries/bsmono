// src/playback_isolator.rs
#![allow(dead_code)]
//
// Predictive playback isolator (echo cancellation with lookahead).
//
// The problem: a speaker plays a known signal x[n] (the reference).
// A mic captures y[n] = h(x)[n] + r[n], where h is the speaker→mic
// acoustic transfer function and r[n] is everything else — room
// reflections we don't care to model, voice, ambient noise. We want
// to recover r[n] so the controller can react to "is the room quiet
// vs. is someone talking" without being confused by the music it
// just played.
//
// Why predictive: most echo-cancellation pipelines run sample-by-
// sample or block-by-block on already-captured mic samples. We know
// the reference up to AT LEAST 100 samples into the future (we sent
// it to the audio output buffer, the speaker hasn't played it yet).
// A predictor that uses that lookahead can output a residual stream
// that's already-compensated by the time the mic catches up, with
// no extra processing latency past the network's own forward-pass
// cost. The user-visible effect: r[n] is available with effectively
// zero algorithmic delay relative to the mic stream.
//
// Architecture: a small MLP. Input:
//   - 1024 samples of reference history (spectral context)
//   - 100 samples of reference lookahead (what's about to play)
//   - 256 samples of mic history (the contamination we're trying to
//     predict — gives the network feedback on its own past mistakes)
// Output:
//   - 100 samples of estimated echo at the mic for the lookahead
//     window. Subtract these from the synced mic samples to get the
//     residual.
//
// Parameter count: 1380×64 + 64 + 64×100 + 100 = 94,884 ≈ 370 KB f32.
// Forward pass: ~95k multiply-accumulates per block; at 480 blocks/s
// (100 samples @ 48 kHz) that's 45M MACs/s — easy on any phone CPU.
//
// Online learning: the MSE between predicted echo and observed mic is
// the loss. We do one SGD step per block. The filter therefore tracks
// changes in the room (someone walked between speaker and mic, the
// speaker volume changed, etc.) over a timescale of seconds.
//
// What this is NOT:
//   - A trained-from-scratch model — random init means the first ~10
//     blocks produce mostly garbage residuals. Pre-training on a
//     room-recorded reference/mic pair would close that gap; the
//     architecture is stable for the user to drop trained weights in
//     later via `load_weights`.
//   - A nonlinear-echo handler — small speakers can clip and produce
//     harmonics not present in the reference. An MLP can model this
//     in principle but ours only sees a few tens of recent samples;
//     for severe nonlinear echo a frequency-domain stage is the
//     standard remedy. Out of scope here.
//   - A double-talk detector — when the user speaks WHILE the speaker
//     is playing, the residual is what we want, but the training step
//     gets noisy gradient because the "target" mic signal includes
//     the user's voice (which is NOT predictable from the reference).
//     A real system would freeze training during double-talk. We
//     don't — instead we run with a small learning rate so user voice
//     contributes only marginal drift to the filter.

use std::collections::VecDeque;
use std::f32::consts::PI;

/// History of the reference signal fed to the network. Longer
/// context = better spectral modeling but more compute. 1024 ≈
/// 21 ms at 48 kHz, enough to cover the longest reasonable in-room
/// echo path (a 4 m room round-trip is ~24 ms, so 21 ms catches the
/// direct path and the first reflection).
pub const LOOKBACK_REF: usize = 1024;

/// How many samples of reference lookahead the network considers.
/// This is also the block size — we emit residual in chunks of
/// LOOKAHEAD samples per call to `process_block`. 100 samples at
/// 48 kHz is ~2 ms, the user's specified target.
pub const LOOKAHEAD: usize = 100;

/// Mic history feeding the network. Shorter than reference because
/// the mic side mostly informs "did my previous prediction err in
/// some direction?" — recent context is enough.
pub const LOOKBACK_MIC: usize = 256;

/// Hidden layer width.
pub const HIDDEN: usize = 64;

const INPUT_DIM: usize = LOOKBACK_REF + LOOKAHEAD + LOOKBACK_MIC;  // 1380
const OUTPUT_DIM: usize = LOOKAHEAD;

/// Default online learning rate. Small enough that user voice
/// (which manifests as transient residual energy) doesn't push the
/// filter far from the steady-state echo path during double-talk.
pub const DEFAULT_LR: f32 = 1e-4;

/// Default echo delay (samples). Caller should override via
/// `set_echo_delay` after a calibration pass; this default is the
/// "no reasonable estimate yet" fallback that effectively turns
/// the canceller into a feedforward filter without time alignment.
pub const DEFAULT_ECHO_DELAY: usize = 480;  // 10 ms @ 48 kHz

/// Predictive AEC. Owns the MLP weights, the reference and mic
/// ring buffers, and the echo-delay estimate.
pub struct PredictiveAec {
    // Layer 1: input → hidden, ReLU.
    w1: Vec<Vec<f32>>,    // [HIDDEN][INPUT_DIM]
    b1: Vec<f32>,         // [HIDDEN]
    // Layer 2: hidden → output, linear (echo can be any real number).
    w2: Vec<Vec<f32>>,    // [OUTPUT_DIM][HIDDEN]
    b2: Vec<f32>,         // [OUTPUT_DIM]

    /// Online learning rate (SGD step size).
    lr: f32,

    /// Reference samples we've seen. Front is oldest, back is newest.
    /// We need at least LOOKBACK_REF + LOOKAHEAD samples to do
    /// inference; until then `process_block` produces zeros and skips
    /// training.
    ref_buf: VecDeque<f32>,

    /// Mic samples we've seen. Same convention.
    mic_buf: VecDeque<f32>,

    /// Echo delay in samples — how long after we send reference[n]
    /// does the mic see its contribution. Used by callers to align
    /// reference and mic before calling `process_block`. Stored here
    /// for inspection / persistence; not used directly by forward().
    echo_delay: usize,

    /// Block count — used by tests + diagnostics.
    blocks_processed: u64,
}

impl PredictiveAec {
    /// Construct with Xavier-initialized weights and the default
    /// learning rate. The network starts producing mostly-zero
    /// echo estimates and adapts online over the first several
    /// seconds of paired reference/mic data.
    pub fn new() -> Self {
        Self::with_seed(0xC0FFEE_u64)
    }

    /// Same as `new` but with a caller-controlled PRNG seed —
    /// tests want determinism.
    pub fn with_seed(seed: u64) -> Self {
        let mut rng = SplitMix64::new(seed);
        // Xavier-ish init: σ ≈ √(2 / fan_in). Helps the ReLUs not
        // saturate on first pass with INPUT_DIM=1380 inputs.
        let sigma1 = (2.0 / INPUT_DIM as f32).sqrt();
        let sigma2 = (2.0 / HIDDEN as f32).sqrt();
        let mut w1 = vec![vec![0.0_f32; INPUT_DIM]; HIDDEN];
        for row in w1.iter_mut() {
            for v in row.iter_mut() { *v = rng.gauss() * sigma1; }
        }
        let b1 = vec![0.0_f32; HIDDEN];
        let mut w2 = vec![vec![0.0_f32; HIDDEN]; OUTPUT_DIM];
        for row in w2.iter_mut() {
            for v in row.iter_mut() { *v = rng.gauss() * sigma2; }
        }
        let b2 = vec![0.0_f32; OUTPUT_DIM];
        Self {
            w1, b1, w2, b2,
            lr: DEFAULT_LR,
            ref_buf: VecDeque::with_capacity(LOOKBACK_REF + LOOKAHEAD + 64),
            mic_buf: VecDeque::with_capacity(LOOKBACK_MIC + LOOKAHEAD + 64),
            echo_delay: DEFAULT_ECHO_DELAY,
            blocks_processed: 0,
        }
    }

    pub fn set_learning_rate(&mut self, lr: f32) {
        // Clamp negative or NaN to a safe default rather than letting
        // it poison the weights silently.
        self.lr = if lr.is_finite() && lr > 0.0 {
            lr.min(0.1)  // upper cap; anything above 0.1 diverges fast
        } else {
            DEFAULT_LR
        };
    }

    pub fn learning_rate(&self) -> f32 { self.lr }

    pub fn set_echo_delay(&mut self, samples: usize) {
        self.echo_delay = samples;
    }
    pub fn echo_delay(&self) -> usize { self.echo_delay }

    pub fn blocks_processed(&self) -> u64 { self.blocks_processed }

    /// Push a new chunk of reference and mic samples (must be the
    /// SAME length and the caller must have time-aligned them per
    /// `echo_delay`), get back the residual block. Internally:
    ///
    ///   1. Append the new samples to the ring buffers.
    ///   2. If buffers aren't full enough yet, return mic_block
    ///      unchanged (passthrough until we can predict).
    ///   3. Build the input vector from the current buffer state.
    ///   4. Forward pass → echo estimate.
    ///   5. residual[i] = mic_block[i] - echo_estimate[i].
    ///   6. One SGD step with mic_block as target.
    ///
    /// Returns a Vec the same length as `mic_block`. The caller
    /// owns it — typically writes it into the downstream consumer.
    pub fn process_block(
        &mut self, ref_block: &[f32], mic_block: &[f32],
    ) -> Vec<f32> {
        // Mic and reference blocks must agree in length so subtraction
        // is well-defined. If they don't, we passthrough mic — better
        // than panicking deep in an audio thread.
        if ref_block.len() != mic_block.len() {
            eprintln!(
                "playback_isolator: block length mismatch (ref={}, \
                 mic={}); passing mic through unmodified",
                ref_block.len(), mic_block.len(),
            );
            return mic_block.to_vec();
        }
        if ref_block.is_empty() { return Vec::new(); }

        self.append_buffers(ref_block, mic_block);

        // Passthrough until we've buffered enough history. The mic
        // contribution to the residual is the mic itself in this
        // case — no echo estimate available.
        if self.ref_buf.len() < LOOKBACK_REF + LOOKAHEAD
            || self.mic_buf.len() < LOOKBACK_MIC + LOOKAHEAD
        {
            return mic_block.to_vec();
        }

        // Build input feature vector.
        let input = self.build_input();
        // Forward pass.
        let (hidden, echo_est) = self.forward(&input);
        // Residual = mic - echo_estimate (over the most recent
        // LOOKAHEAD samples of mic_buf, which is what mic_block
        // delivered).
        let mic_recent: Vec<f32> = self.mic_buf.iter()
            .rev().take(LOOKAHEAD).rev().copied().collect();
        let residual: Vec<f32> = mic_recent.iter().zip(echo_est.iter())
            .map(|(m, e)| m - e).collect();

        // Online SGD step.
        self.sgd_step(&input, &hidden, &echo_est, &mic_recent);

        self.blocks_processed = self.blocks_processed.wrapping_add(1);
        residual
    }

    /// Run a forward pass without updating weights. Useful for
    /// inference-only paths (e.g. after weights are frozen) and for
    /// tests that want to inspect prediction without affecting state.
    pub fn predict_only(&self, ref_block: &[f32]) -> Vec<f32> {
        if ref_block.len() != LOOKAHEAD { return Vec::new(); }
        if self.ref_buf.len() < LOOKBACK_REF
            || self.mic_buf.len() < LOOKBACK_MIC
        {
            return vec![0.0; LOOKAHEAD];
        }
        // Build a synthetic input: existing buffer state but with
        // ref_block as the lookahead window.
        let mut input = vec![0.0_f32; INPUT_DIM];
        let mut p = 0;
        for v in self.ref_buf.iter()
            .rev().take(LOOKBACK_REF).rev() {
            input[p] = *v; p += 1;
        }
        for v in ref_block { input[p] = *v; p += 1; }
        for v in self.mic_buf.iter()
            .rev().take(LOOKBACK_MIC).rev() {
            input[p] = *v; p += 1;
        }
        let (_, echo_est) = self.forward(&input);
        echo_est
    }

    fn append_buffers(&mut self, ref_block: &[f32], mic_block: &[f32]) {
        for v in ref_block { self.ref_buf.push_back(*v); }
        for v in mic_block { self.mic_buf.push_back(*v); }
        // Keep buffer sizes bounded so VecDeque doesn't grow forever.
        let max_ref = LOOKBACK_REF + LOOKAHEAD + 16;
        let max_mic = LOOKBACK_MIC + LOOKAHEAD + 16;
        while self.ref_buf.len() > max_ref { self.ref_buf.pop_front(); }
        while self.mic_buf.len() > max_mic { self.mic_buf.pop_front(); }
    }

    fn build_input(&self) -> Vec<f32> {
        let mut input = vec![0.0_f32; INPUT_DIM];
        let mut p = 0;
        // Reference lookback: the oldest LOOKBACK_REF samples
        // available, which sit at positions [len-LOOKBACK_REF-LOOKAHEAD
        // .. len-LOOKAHEAD).
        let ref_len = self.ref_buf.len();
        let lookback_start = ref_len.saturating_sub(LOOKBACK_REF + LOOKAHEAD);
        for i in 0..LOOKBACK_REF {
            input[p] = *self.ref_buf.get(lookback_start + i).unwrap_or(&0.0);
            p += 1;
        }
        // Reference lookahead: the most recent LOOKAHEAD samples.
        let lookahead_start = ref_len.saturating_sub(LOOKAHEAD);
        for i in 0..LOOKAHEAD {
            input[p] = *self.ref_buf.get(lookahead_start + i).unwrap_or(&0.0);
            p += 1;
        }
        // Mic lookback: the LOOKBACK_MIC samples that arrived BEFORE
        // the most recent LOOKAHEAD samples.
        let mic_len = self.mic_buf.len();
        let mic_start = mic_len.saturating_sub(LOOKBACK_MIC + LOOKAHEAD);
        for i in 0..LOOKBACK_MIC {
            input[p] = *self.mic_buf.get(mic_start + i).unwrap_or(&0.0);
            p += 1;
        }
        debug_assert_eq!(p, INPUT_DIM);
        input
    }

    /// Forward pass. Returns (hidden_pre_relu_post_activation,
    /// output) so the backward pass can reuse hidden activations.
    fn forward(&self, input: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let mut hidden = vec![0.0_f32; HIDDEN];
        for j in 0..HIDDEN {
            let mut acc = self.b1[j];
            let row = &self.w1[j];
            for i in 0..INPUT_DIM { acc += row[i] * input[i]; }
            hidden[j] = relu(acc);
        }
        let mut output = vec![0.0_f32; OUTPUT_DIM];
        for k in 0..OUTPUT_DIM {
            let mut acc = self.b2[k];
            let row = &self.w2[k];
            for j in 0..HIDDEN { acc += row[j] * hidden[j]; }
            output[k] = acc;
        }
        (hidden, output)
    }

    /// SGD step minimizing ½ Σ (target_k - output_k)².
    /// ∂L/∂output_k = output_k - target_k = -residual_k.
    /// ∂L/∂W2[k][j] = (output_k - target_k) * hidden_j.
    /// ∂L/∂hidden_j = Σ_k (output_k - target_k) * W2[k][j].
    /// ∂L/∂W1[j][i] = ∂L/∂hidden_j * relu'(pre_j) * input_i.
    /// Bias gradients follow similarly with input_i replaced by 1.
    ///
    /// Gradient clipping: each weight's update magnitude is capped
    /// at `lr * 1.0` to keep a single huge residual (e.g. someone
    /// clapping next to the mic) from blowing up the weights. This
    /// is the cheapest form of stability protection and works fine
    /// here because the loss has bounded curvature.
    fn sgd_step(
        &mut self,
        input: &[f32],
        hidden: &[f32],
        output: &[f32],
        target: &[f32],
    ) {
        // Output-side gradients: d_out[k] = output[k] - target[k].
        let mut d_out = vec![0.0_f32; OUTPUT_DIM];
        for k in 0..OUTPUT_DIM {
            d_out[k] = output[k] - target[k];
        }
        // Stability: if any d_out is non-finite, abort the step
        // rather than spreading NaN through the weights.
        if d_out.iter().any(|v| !v.is_finite()) { return; }

        // Hidden-side gradients: d_hidden[j] = Σ_k d_out[k] *
        // W2[k][j], gated by ReLU' (= 1 where hidden>0 else 0).
        let mut d_hidden = vec![0.0_f32; HIDDEN];
        for j in 0..HIDDEN {
            if hidden[j] <= 0.0 { continue; }  // ReLU' = 0
            let mut acc = 0.0_f32;
            for k in 0..OUTPUT_DIM { acc += d_out[k] * self.w2[k][j]; }
            d_hidden[j] = acc;
        }

        let lr = self.lr;
        // Update W2 + b2.
        for k in 0..OUTPUT_DIM {
            let dk = d_out[k];
            self.b2[k] -= lr * dk;
            let row = &mut self.w2[k];
            for j in 0..HIDDEN {
                let g = dk * hidden[j];
                // Clip per-element gradient magnitude.
                let g = g.clamp(-1.0, 1.0);
                row[j] -= lr * g;
            }
        }
        // Update W1 + b1.
        for j in 0..HIDDEN {
            let dj = d_hidden[j];
            if dj == 0.0 { continue; }
            self.b1[j] -= lr * dj;
            let row = &mut self.w1[j];
            for i in 0..INPUT_DIM {
                let g = dj * input[i];
                let g = g.clamp(-1.0, 1.0);
                row[i] -= lr * g;
            }
        }
    }

    /// Save weights as a compact f32-bin blob with a magic header.
    /// Layout:
    ///   magic     4 bytes "AECP"
    ///   version   u32 LE
    ///   shapes    u32 × 4 (lookback_ref, lookahead, lookback_mic, hidden)
    ///   w1, b1, w2, b2  all row-major f32 LE
    pub fn save_to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"AECP");
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&(LOOKBACK_REF as u32).to_le_bytes());
        out.extend_from_slice(&(LOOKAHEAD as u32).to_le_bytes());
        out.extend_from_slice(&(LOOKBACK_MIC as u32).to_le_bytes());
        out.extend_from_slice(&(HIDDEN as u32).to_le_bytes());
        for row in &self.w1 { for v in row { out.extend_from_slice(&v.to_le_bytes()); } }
        for v in &self.b1 { out.extend_from_slice(&v.to_le_bytes()); }
        for row in &self.w2 { for v in row { out.extend_from_slice(&v.to_le_bytes()); } }
        for v in &self.b2 { out.extend_from_slice(&v.to_le_bytes()); }
        out
    }

    /// Deserialize. Returns None on any malformed input (bad magic,
    /// wrong version, wrong shape, truncated). Never panics —
    /// user-supplied weight files are untrusted input.
    pub fn load_from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 24 || &data[0..4] != b"AECP" { return None; }
        let r32 = |i: usize| -> u32 {
            u32::from_le_bytes([data[i], data[i+1], data[i+2], data[i+3]])
        };
        if r32(4) != 1 { return None; }
        if r32(8) as usize != LOOKBACK_REF
            || r32(12) as usize != LOOKAHEAD
            || r32(16) as usize != LOOKBACK_MIC
            || r32(20) as usize != HIDDEN
        { return None; }
        let mut p = 24;
        let read_f32 = |bytes: &[u8], p: usize| -> Option<f32> {
            if p + 4 > bytes.len() { return None; }
            Some(f32::from_le_bytes([bytes[p], bytes[p+1], bytes[p+2], bytes[p+3]]))
        };
        let mut me = Self::new();
        for row in me.w1.iter_mut() {
            for v in row.iter_mut() { *v = read_f32(data, p)?; p += 4; }
        }
        for v in me.b1.iter_mut() { *v = read_f32(data, p)?; p += 4; }
        for row in me.w2.iter_mut() {
            for v in row.iter_mut() { *v = read_f32(data, p)?; p += 4; }
        }
        for v in me.b2.iter_mut() { *v = read_f32(data, p)?; p += 4; }
        Some(me)
    }
}

impl Default for PredictiveAec {
    fn default() -> Self { Self::new() }
}

/// Cross-correlation-based delay estimator. Run this on a quiet
/// section with the reference loud and the mic capturing only echo
/// (no voice/ambient) to estimate `echo_delay` in samples.
///
/// Searches in [min_delay, max_delay] for the lag that maximizes
/// the normalized cross-correlation between reference and mic.
/// Returns the lag in samples, or None if the result isn't
/// confident enough (peak correlation below 0.3 → likely no echo
/// or speaker disconnected).
pub fn estimate_echo_delay(
    reference: &[f32], mic: &[f32],
    min_delay: usize, max_delay: usize,
) -> Option<usize> {
    if reference.is_empty() || mic.is_empty() { return None; }
    let max_delay = max_delay.min(mic.len()).min(reference.len());
    if max_delay <= min_delay { return None; }

    // Energy of reference (used for normalization). Mean-subtract
    // to remove any DC offset that would dominate the correlation.
    let ref_mean: f32 = reference.iter().sum::<f32>() / reference.len() as f32;
    let r0: f32 = reference.iter().map(|v| (v - ref_mean).powi(2)).sum();
    if r0 < 1e-9 { return None; }

    let mic_mean: f32 = mic.iter().sum::<f32>() / mic.len() as f32;
    let m0: f32 = mic.iter().map(|v| (v - mic_mean).powi(2)).sum();
    if m0 < 1e-9 { return None; }

    let denom = (r0 * m0).sqrt();
    let mut best_lag = min_delay;
    let mut best_corr = -1.0_f32;
    for lag in min_delay..max_delay {
        let n = (reference.len() - lag).min(mic.len() - lag);
        if n == 0 { break; }
        let mut acc = 0.0_f32;
        for i in 0..n {
            let r = reference[i] - ref_mean;
            let m = mic[i + lag] - mic_mean;
            acc += r * m;
        }
        let corr = acc / denom;
        if corr > best_corr { best_corr = corr; best_lag = lag; }
    }
    if best_corr < 0.3 { None } else { Some(best_lag) }
}

// ── Helpers ───────────────────────────────────────────────────────

fn relu(x: f32) -> f32 { if x > 0.0 { x } else { 0.0 } }

/// SplitMix64 PRNG with Box-Muller Gaussian draw. Tiny inline so
/// we don't add a dep for one initializer.
struct SplitMix64 { state: u64 }
impl SplitMix64 {
    fn new(seed: u64) -> Self { Self { state: seed.wrapping_add(0x9E37_79B9_7F4A_7C15) } }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn next_unit(&mut self) -> f32 {
        // Use top 24 bits → uniform in [0, 1)
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }
    /// Box-Muller Gaussian: σ=1, μ=0.
    fn gauss(&mut self) -> f32 {
        let u1 = self.next_unit().max(1e-9);
        let u2 = self.next_unit();
        (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
    }
}

// ── Tests ─────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(freq: f32, n: usize, sr: f32, amp: f32) -> Vec<f32> {
        (0..n).map(|i| (2.0 * PI * freq * i as f32 / sr).sin() * amp).collect()
    }

    #[test]
    fn forward_pass_shape() {
        let aec = PredictiveAec::with_seed(1);
        let input = vec![0.1_f32; INPUT_DIM];
        let (hidden, out) = aec.forward(&input);
        assert_eq!(hidden.len(), HIDDEN);
        assert_eq!(out.len(), OUTPUT_DIM);
        for v in out { assert!(v.is_finite()); }
    }

    #[test]
    fn forward_pass_deterministic_for_same_seed() {
        let a = PredictiveAec::with_seed(42);
        let b = PredictiveAec::with_seed(42);
        let input = vec![0.3_f32; INPUT_DIM];
        let (_, out_a) = a.forward(&input);
        let (_, out_b) = b.forward(&input);
        for (x, y) in out_a.iter().zip(out_b.iter()) {
            assert_eq!(x, y, "same seed → same forward output");
        }
    }

    #[test]
    fn empty_blocks_return_empty() {
        let mut aec = PredictiveAec::with_seed(7);
        let out = aec.process_block(&[], &[]);
        assert!(out.is_empty());
    }

    #[test]
    fn mismatched_block_lengths_passthrough_mic() {
        let mut aec = PredictiveAec::with_seed(7);
        let r = vec![0.5_f32; 50];
        let m = vec![0.2_f32; 100];
        let out = aec.process_block(&r, &m);
        // Passthrough = mic_block unchanged.
        assert_eq!(out, m);
    }

    #[test]
    fn early_blocks_passthrough_until_buffers_fill() {
        let mut aec = PredictiveAec::with_seed(7);
        let ref_block = vec![0.5_f32; LOOKAHEAD];
        let mic_block = vec![0.3_f32; LOOKAHEAD];
        // First block: buffers were empty, can't predict.
        let out = aec.process_block(&ref_block, &mic_block);
        assert_eq!(out, mic_block, "first block should be passthrough");
    }

    #[test]
    fn process_eventually_produces_nonpassthrough_residual() {
        // After enough blocks to fill the buffers, process_block
        // must produce a residual that differs from the mic input
        // (because the network's prediction is nonzero).
        let mut aec = PredictiveAec::with_seed(11);
        let n_blocks = (LOOKBACK_REF + LOOKAHEAD) / LOOKAHEAD + 2;
        let mut last_diff = 0.0_f32;
        for _ in 0..n_blocks {
            let r = tone(440.0, LOOKAHEAD, 48000.0, 0.5);
            let m = tone(440.0, LOOKAHEAD, 48000.0, 0.45); // close to echo
            let out = aec.process_block(&r, &m);
            assert_eq!(out.len(), LOOKAHEAD);
            for v in &out { assert!(v.is_finite()); }
            let diff: f32 = out.iter().zip(m.iter())
                .map(|(o, mi)| (o - mi).abs()).sum();
            last_diff = diff;
        }
        // After buffers filled, the residual should differ from
        // passthrough — non-zero prediction was subtracted.
        assert!(last_diff > 0.0,
                "expected non-passthrough residual after warmup");
    }

    #[test]
    fn online_training_reduces_residual_on_constant_echo() {
        // The cleanest learnable case: mic = constant_gain * reference.
        // After many blocks the AEC should learn an effective scalar
        // and the residual energy should drop dramatically.
        let mut aec = PredictiveAec::with_seed(2);
        aec.set_learning_rate(5e-3);
        // White-noise reference (no easy spectral structure to abuse).
        let mut rng = SplitMix64::new(0xDEADBEEF);
        let gain = 0.4_f32;

        let mut energy_first: Option<f32> = None;
        let mut energy_last: f32 = 0.0;
        for blk in 0..120 {
            let r: Vec<f32> = (0..LOOKAHEAD).map(|_| rng.gauss() * 0.5).collect();
            let m: Vec<f32> = r.iter().map(|x| x * gain).collect();
            let out = aec.process_block(&r, &m);
            let e: f32 = out.iter().map(|v| v * v).sum();
            if blk == (LOOKBACK_REF + LOOKAHEAD) / LOOKAHEAD + 2 {
                energy_first = Some(e);  // first post-warmup block
            }
            energy_last = e;
        }
        if let Some(ef) = energy_first {
            assert!(energy_last < ef * 0.8,
                    "expected residual energy to drop after training: \
                     first={} last={}", ef, energy_last);
        }
    }

    #[test]
    fn set_learning_rate_clamps_invalid() {
        let mut aec = PredictiveAec::with_seed(1);
        aec.set_learning_rate(f32::NAN);
        assert_eq!(aec.learning_rate(), DEFAULT_LR);
        aec.set_learning_rate(-1.0);
        assert_eq!(aec.learning_rate(), DEFAULT_LR);
        aec.set_learning_rate(0.5);
        // 0.5 > cap of 0.1, should clamp.
        assert!(aec.learning_rate() <= 0.1);
        aec.set_learning_rate(1e-3);
        assert_eq!(aec.learning_rate(), 1e-3);
    }

    #[test]
    fn save_load_roundtrip() {
        let mut aec = PredictiveAec::with_seed(99);
        // Train a step so the weights aren't all init values.
        for _ in 0..5 {
            let r = vec![0.5_f32; LOOKAHEAD];
            let m = vec![0.2_f32; LOOKAHEAD];
            aec.process_block(&r, &m);
        }
        let bytes = aec.save_to_bytes();
        let loaded = PredictiveAec::load_from_bytes(&bytes).unwrap();
        // Spot-check weights match.
        assert_eq!(aec.w1[0][0], loaded.w1[0][0]);
        assert_eq!(aec.w2[10][5], loaded.w2[10][5]);
        assert_eq!(aec.b1[7], loaded.b1[7]);
        assert_eq!(aec.b2[50], loaded.b2[50]);
    }

    #[test]
    fn load_rejects_bad_magic() {
        let mut bytes = PredictiveAec::with_seed(1).save_to_bytes();
        bytes[0] = b'X';
        assert!(PredictiveAec::load_from_bytes(&bytes).is_none());
    }

    #[test]
    fn load_rejects_wrong_version() {
        let mut bytes = PredictiveAec::with_seed(1).save_to_bytes();
        bytes[4..8].copy_from_slice(&99u32.to_le_bytes());
        assert!(PredictiveAec::load_from_bytes(&bytes).is_none());
    }

    #[test]
    fn load_rejects_truncated() {
        let bytes = PredictiveAec::with_seed(1).save_to_bytes();
        assert!(PredictiveAec::load_from_bytes(&bytes[..100]).is_none());
    }

    #[test]
    fn estimate_echo_delay_finds_known_lag() {
        // Build a reference tone, and a "mic" that's the same tone
        // delayed by 200 samples plus some uncorrelated noise.
        let n = 4000;
        let r = tone(800.0, n, 48000.0, 0.5);
        let delay = 200_usize;
        let mut m = vec![0.0_f32; n];
        for i in delay..n { m[i] = r[i - delay] * 0.4; }
        // Add noise.
        let mut rng = SplitMix64::new(7);
        for v in m.iter_mut() { *v += rng.gauss() * 0.02; }
        let est = estimate_echo_delay(&r, &m, 0, 1000).unwrap();
        // Should land within a few samples of the true delay.
        let err = (est as i32 - delay as i32).abs();
        assert!(err <= 5, "estimated delay {} too far from true {}",
                est, delay);
    }

    #[test]
    fn estimate_echo_delay_returns_none_for_uncorrelated_signals() {
        let mut rng = SplitMix64::new(1);
        let n = 2000;
        let r: Vec<f32> = (0..n).map(|_| rng.gauss()).collect();
        // Different seed → independent noise.
        let mut rng2 = SplitMix64::new(99999);
        let m: Vec<f32> = (0..n).map(|_| rng2.gauss()).collect();
        assert!(estimate_echo_delay(&r, &m, 0, 500).is_none(),
                "uncorrelated signals should return None");
    }

    #[test]
    fn estimate_echo_delay_handles_empty() {
        assert!(estimate_echo_delay(&[], &[], 0, 100).is_none());
        assert!(estimate_echo_delay(&[0.1; 100], &[], 0, 100).is_none());
    }

    #[test]
    fn process_block_never_produces_nan() {
        // Random reference and mic, including some pathological
        // inputs, should never produce non-finite residual.
        let mut aec = PredictiveAec::with_seed(1);
        let mut rng = SplitMix64::new(42);
        for _ in 0..50 {
            let r: Vec<f32> = (0..LOOKAHEAD).map(|_| rng.gauss() * 2.0).collect();
            let m: Vec<f32> = (0..LOOKAHEAD).map(|_| rng.gauss() * 2.0).collect();
            let out = aec.process_block(&r, &m);
            for v in &out { assert!(v.is_finite(), "non-finite output"); }
        }
    }

    #[test]
    fn predict_only_does_not_mutate_state() {
        let mut aec = PredictiveAec::with_seed(1);
        // Warm buffers.
        for _ in 0..((LOOKBACK_REF + LOOKAHEAD) / LOOKAHEAD + 2) {
            let r = vec![0.1_f32; LOOKAHEAD];
            let m = vec![0.05_f32; LOOKAHEAD];
            aec.process_block(&r, &m);
        }
        let blocks_before = aec.blocks_processed();
        let w_before = aec.w1[0][0];
        let r = vec![0.3_f32; LOOKAHEAD];
        let _ = aec.predict_only(&r);
        assert_eq!(aec.blocks_processed(), blocks_before,
                   "predict_only must not bump block counter");
        assert_eq!(aec.w1[0][0], w_before,
                   "predict_only must not change weights");
    }
}
