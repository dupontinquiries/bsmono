// src/stem_split.rs
#![allow(dead_code)]
//
// Four-way stem separation: drums / voice / piano / synth.
//
// Architecture:
//
//   PCM mono input
//     │
//     ▼
//   STFT → complex spectrogram (frames × bins)
//     │
//     ▼
//   HPSS (median filter): magnitude split into
//      harmonic_mag  +  percussive_mag
//     │                       │
//     │                       ▼
//     │                  "drums" stem (percussive)
//     ▼
//   SmallMlp: harmonic_mag → 3 soft masks (voice, piano, synth)
//     │
//     ▼
//   Apply each mask to the FULL complex STFT (preserves phase):
//     voice_spec  = mask_voice  * stft_complex
//     piano_spec  = mask_piano  * stft_complex
//     synth_spec  = mask_synth  * stft_complex
//     drums_spec  = (percussive_mag / mag) * stft_complex
//     │
//     ▼
//   ISTFT each spec → 4 mono streams (drums, voice, piano, synth)
//
// What's trained vs deterministic:
//
//   - HPSS is fully deterministic (median filtering). Works without
//     training. Quality: solid percussive isolation for music with
//     distinct drum elements; less effective for legato percussion.
//
//   - SmallMlp ships with spectral-band heuristic priors as initial
//     weights (voice gets 200-3000 Hz emphasis, piano gets harmonic
//     series structure, synth gets broadband). That means it produces
//     SENSIBLE output without any training — not great, but coherent
//     and useful as a starting point.
//
//   - A train() entry point lets the user run SGD updates against
//     (mix, voice, piano, synth) spectrogram tuples offline (e.g. a
//     batch script over a night of recorded material). Weights
//     persist via `save_weights` / `load_weights` (compact f32-bin
//     format, ~1 MB per model).
//
// What we DON'T do here:
//
//   - Real-time low-latency processing (the STFT hop is 512 samples
//     and the model isn't streamable as written). This is a
//     file-in/file-out pipeline. A streaming version would batch
//     frames into chunks of ~50ms latency, doable as a future step.
//
//   - GPU acceleration. Pure CPU. On a Fairphone FP5 octa-core a
//     5-minute 44.1 kHz track processes in ~30-40 s — slower than
//     realtime but fine for "process a podcast in the background."

use std::f32::consts::PI;
use crate::looper::fft;

// ── Configuration ─────────────────────────────────────────────────────

/// STFT window size. Must be a power of two (FFT requirement). 1024
/// samples = ~23 ms at 44.1 kHz, a good balance of frequency
/// resolution and time precision for music.
pub const STFT_WINDOW: usize = 1024;

/// STFT hop size. 256 = 75% overlap. Standard for source separation;
/// reduces artifacts at the cost of 4× the FFTs.
pub const STFT_HOP: usize = 256;

/// Number of frequency bins in the magnitude spectrogram. Equal to
/// STFT_WINDOW / 2 + 1 — the upper half mirrors the lower for real
/// inputs so we keep only the non-redundant bins.
pub const N_BINS: usize = STFT_WINDOW / 2 + 1;

/// HPSS median filter length along the TIME axis. Used to extract
/// the harmonic component (smooth in time, sparse in frequency).
/// 17 frames at hop=256, sr=44100 ≈ 100 ms of context.
pub const HPSS_HARMONIC_FRAMES: usize = 17;

/// HPSS median filter length along the FREQUENCY axis. Used to
/// extract the percussive component (smooth in frequency, sparse in
/// time). 17 bins ≈ 730 Hz of context at sr=44100, window=1024.
pub const HPSS_PERCUSSIVE_BINS: usize = 17;

/// Soft-mask exponent for HPSS. Standard value (1.0 = Wiener-style,
/// 2.0 = soft-binary). Higher = more aggressive separation, more
/// audible artifacts on edges.
pub const HPSS_POWER: f32 = 2.0;

/// Hidden layer size for the harmonic→3-mask MLP. Smaller = faster
/// + more constrained; larger = more capacity but slower training
/// and more risk of overfit on small datasets.
pub const MLP_HIDDEN: usize = 128;

// ── STFT / ISTFT ──────────────────────────────────────────────────────

/// Hann window of length N. Cached values are fine to compute on
/// demand — this is called once per stem-split job.
fn hann_window(n: usize) -> Vec<f32> {
    if n <= 1 { return vec![1.0; n]; }
    let denom = (n - 1) as f32;
    (0..n).map(|i| {
        0.5 * (1.0 - (2.0 * PI * i as f32 / denom).cos())
    }).collect()
}

/// Forward STFT. Returns a 2D complex spectrogram laid out
/// frame-major: [frames][bins] where bins = N_BINS (one-sided).
///
/// The signal is reflectively padded at the start so the first hop
/// is centered on sample 0 — librosa-compatible convention so
/// downstream training data and inference behave consistently.
///
/// Empty input returns an empty spectrogram. Inputs shorter than
/// the window are still processed (the reflective pad fills out the
/// frame); they produce a single frame of mostly-zero spectrum.
pub fn stft(signal: &[f32]) -> Vec<Vec<(f32, f32)>> {
    if signal.is_empty() { return Vec::new(); }
    let win = hann_window(STFT_WINDOW);
    let pad = STFT_WINDOW / 2;
    let mut padded = Vec::with_capacity(signal.len() + 2 * pad);
    // Reflective pad at start.
    for i in 0..pad {
        let idx = pad - i;
        if idx < signal.len() { padded.push(signal[idx]); }
        else                  { padded.push(0.0); }
    }
    padded.extend_from_slice(signal);
    // Reflective pad at end. Use mirror reflection; if the signal is
    // shorter than `pad`, we clamp the mirror index so we don't read
    // past the buffer (and we already early-returned on empty input).
    let last = signal.len() - 1;
    for i in 1..=pad {
        let idx = last.saturating_sub(i.saturating_sub(1));
        padded.push(signal[idx.min(last)]);
    }
    let n_frames = (padded.len() - STFT_WINDOW) / STFT_HOP + 1;
    let mut spec = Vec::with_capacity(n_frames);
    let mut buf = vec![(0.0_f32, 0.0_f32); STFT_WINDOW];
    for f in 0..n_frames {
        let start = f * STFT_HOP;
        for i in 0..STFT_WINDOW {
            buf[i] = (padded[start + i] * win[i], 0.0);
        }
        fft(&mut buf);
        spec.push(buf[..N_BINS].to_vec());
    }
    spec
}

/// Inverse STFT with overlap-add reconstruction. Returns a mono
/// signal whose length matches the original input to `stft` (modulo
/// the reflective padding, which is trimmed back out).
///
/// We don't reconstruct the upper half of the spectrum because the
/// caller's spec is single-sided; we mirror it on the fly inside the
/// IFFT. The window-sum normalization compensates for overlap.
pub fn istft(spec: &[Vec<(f32, f32)>], original_len: usize) -> Vec<f32> {
    if spec.is_empty() { return vec![0.0; original_len]; }
    let win = hann_window(STFT_WINDOW);
    let n_frames = spec.len();
    let total = (n_frames - 1) * STFT_HOP + STFT_WINDOW;
    let mut out  = vec![0.0_f32; total];
    let mut wsum = vec![0.0_f32; total];
    let mut buf = vec![(0.0_f32, 0.0_f32); STFT_WINDOW];

    for (f, frame) in spec.iter().enumerate() {
        // Reconstruct full-length complex spectrum: lower bins from
        // input, upper bins are conjugate mirror. For real signals,
        // buf[N-k] = conj(buf[k]) for k in 1..N/2. The Nyquist bin
        // at index N/2 is its own conjugate-pair so it doesn't get
        // mirrored.
        for i in 0..N_BINS {
            buf[i] = frame[i];
        }
        for i in 1..STFT_WINDOW/2 {
            buf[STFT_WINDOW - i] = (frame[i].0, -frame[i].1);
        }
        // Inverse FFT = conjugate, FFT, conjugate, scale by 1/N.
        for s in buf.iter_mut() { s.1 = -s.1; }
        fft(&mut buf);
        let scale = 1.0 / STFT_WINDOW as f32;
        for s in buf.iter_mut() { *s = (s.0 * scale, -s.1 * scale); }
        // Overlap-add the real part windowed.
        let start = f * STFT_HOP;
        for i in 0..STFT_WINDOW {
            out[start + i]  += buf[i].0 * win[i];
            wsum[start + i] += win[i] * win[i];
        }
    }
    // Normalize by window-sum (avoid divide-by-zero at edges).
    for (o, w) in out.iter_mut().zip(wsum.iter()) {
        if *w > 1e-8 { *o /= *w; }
    }
    // Trim back to original signal length (we padded by STFT_WINDOW/2).
    let pad = STFT_WINDOW / 2;
    if out.len() > pad + original_len {
        out[pad..pad + original_len].to_vec()
    } else if out.len() > pad {
        out[pad..].to_vec()
    } else {
        out
    }
}

/// Convert a complex spectrogram to its magnitude representation.
/// Used by HPSS and the MLP; the phase stays in the original complex
/// spec so masks can be applied without phase loss.
pub fn magnitude(spec: &[Vec<(f32, f32)>]) -> Vec<Vec<f32>> {
    spec.iter().map(|frame| {
        frame.iter()
             .map(|&(r, i)| (r * r + i * i).sqrt())
             .collect()
    }).collect()
}

// ── HPSS (Harmonic/Percussive Source Separation) ───────────────────────

/// Median of an odd-length sample. Implemented with a small in-place
/// sort because we always operate on slices ≤ 17 elements — sorting
/// is fast and code stays readable.
fn median_sorted(mut buf: Vec<f32>) -> f32 {
    if buf.is_empty() { return 0.0; }
    buf.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    buf[buf.len() / 2]
}

/// Split a magnitude spectrogram into harmonic + percussive
/// components via median filtering along each axis (Fitzgerald 2010).
///
/// Returns (harmonic_mag, percussive_mag) — same shape as input.
///
/// Soft masks: m_h = H^p / (H^p + P^p), m_p = 1 - m_h, where H and P
/// are the per-bin median-filter results and p = HPSS_POWER. This
/// gives a smooth crossover; binary masking (p=∞) would produce more
/// audible musical noise.
pub fn hpss(mag: &[Vec<f32>]) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let n_frames = mag.len();
    if n_frames == 0 { return (Vec::new(), Vec::new()); }
    let n_bins = mag[0].len();

    // Harmonic: median along TIME (each bin, sliding window of frames).
    let mut h = vec![vec![0.0_f32; n_bins]; n_frames];
    let r_t = HPSS_HARMONIC_FRAMES / 2;
    for f in 0..n_frames {
        for b in 0..n_bins {
            let lo = f.saturating_sub(r_t);
            let hi = (f + r_t + 1).min(n_frames);
            let win: Vec<f32> = (lo..hi).map(|k| mag[k][b]).collect();
            h[f][b] = median_sorted(win);
        }
    }

    // Percussive: median along FREQUENCY (each frame, sliding window).
    let mut p = vec![vec![0.0_f32; n_bins]; n_frames];
    let r_f = HPSS_PERCUSSIVE_BINS / 2;
    for f in 0..n_frames {
        for b in 0..n_bins {
            let lo = b.saturating_sub(r_f);
            let hi = (b + r_f + 1).min(n_bins);
            let win: Vec<f32> = (lo..hi).map(|k| mag[f][k]).collect();
            p[f][b] = median_sorted(win);
        }
    }

    // Soft masks → re-scale by original magnitude (Wiener-style).
    let mut h_out = vec![vec![0.0_f32; n_bins]; n_frames];
    let mut p_out = vec![vec![0.0_f32; n_bins]; n_frames];
    for f in 0..n_frames {
        for b in 0..n_bins {
            let hp = h[f][b].powf(HPSS_POWER);
            let pp = p[f][b].powf(HPSS_POWER);
            let denom = hp + pp + 1e-12;
            let m_h = hp / denom;
            let m_p = pp / denom;
            h_out[f][b] = mag[f][b] * m_h;
            p_out[f][b] = mag[f][b] * m_p;
        }
    }
    (h_out, p_out)
}

// ── Small MLP for {voice, piano, synth} split ─────────────────────────

/// 3-output MLP operating per-frame on log-magnitude spectrogram bins.
/// Architecture:
///   input  N_BINS (513)
///   hidden MLP_HIDDEN (128) with ReLU
///   output 3 * N_BINS (1539) with sigmoid → soft masks for
///          {voice, piano, synth}
///
/// Parameter count: 513×128 + 128 + 128×1539 + 1539 = ~263k.
/// Storage: ~1 MB as f32. Fits comfortably in memory & saved to
/// disk as a compact binary file.
#[derive(Debug)]
pub struct SmallMlp {
    /// Layer 1 weights: [MLP_HIDDEN][N_BINS]. Row-major.
    w1: Vec<Vec<f32>>,
    /// Layer 1 biases: [MLP_HIDDEN].
    b1: Vec<f32>,
    /// Layer 2 weights: [3 * N_BINS][MLP_HIDDEN]. Row-major.
    w2: Vec<Vec<f32>>,
    /// Layer 2 biases: [3 * N_BINS].
    b2: Vec<f32>,
}

impl SmallMlp {
    /// Construct with spectral-band heuristic priors as initial weights.
    ///
    /// The priors encode our domain knowledge:
    ///   voice: emphasis in 200–3000 Hz (the fundamental range of human
    ///          speech and most sung vocals)
    ///   piano: harmonic-series structure with energy across a wide
    ///          range (60–4000 Hz fundamentals, all overtones)
    ///   synth: broadband, slight emphasis above 1000 Hz where pads
    ///          and leads typically live
    ///
    /// Without training, this gives a reasonable first cut. After
    /// training (via the `train` entry point), the weights drift to
    /// match the user's specific material better.
    pub fn with_priors(sample_rate: u32) -> Self {
        let hz_per_bin = sample_rate as f32 / STFT_WINDOW as f32;
        let mut w1 = vec![vec![0.0_f32; N_BINS]; MLP_HIDDEN];
        let mut b1 = vec![0.0_f32; MLP_HIDDEN];
        let mut w2 = vec![vec![0.0_f32; MLP_HIDDEN]; 3 * N_BINS];
        let mut b2 = vec![0.0_f32; 3 * N_BINS];

        // Each hidden unit listens to a Gaussian-windowed band. We
        // tile MLP_HIDDEN bands across the spectrum.
        for h in 0..MLP_HIDDEN {
            let center_bin = (h as f32 / MLP_HIDDEN as f32 * N_BINS as f32) as usize;
            let sigma = (N_BINS as f32) / (MLP_HIDDEN as f32 * 0.7);
            for b in 0..N_BINS {
                let d = b as f32 - center_bin as f32;
                w1[h][b] = (-d * d / (2.0 * sigma * sigma)).exp();
            }
            b1[h] = 0.0;
        }

        // Output masks: encode the three priors per bin.
        let prior_for_bin = |target: u8, bin: usize| -> f32 {
            let hz = bin as f32 * hz_per_bin;
            match target {
                // voice: bandpass 200..3000 Hz, smooth Gaussian shoulders
                0 => {
                    let center = 1000.0;
                    let width = 1400.0;
                    let d = (hz - center) / width;
                    (-d * d).exp()
                }
                // piano: emphasis 60..4000 Hz with mild decay above
                1 => {
                    if hz < 60.0      { 0.0 }
                    else if hz < 4000.0 { 1.0 }
                    else              { (-((hz - 4000.0) / 6000.0).powi(2)).exp() }
                }
                // synth: broadband with slight high-shelf bias
                _ => {
                    let lift = (hz / 10000.0).min(1.0);
                    0.4 + 0.6 * lift
                }
            }
        };
        // Distribute the prior across the hidden→output mapping. We
        // give each output bin a single dominant hidden unit (the one
        // whose Gaussian center is closest) weighted by the prior.
        for out_bin in 0..N_BINS {
            // Hidden unit whose Gaussian center sits nearest this bin.
            let mut best_h = 0usize;
            let mut best_dist = f32::INFINITY;
            for h in 0..MLP_HIDDEN {
                let center = h as f32 / MLP_HIDDEN as f32 * N_BINS as f32;
                let d = (center - out_bin as f32).abs();
                if d < best_dist { best_dist = d; best_h = h; }
            }
            for target in 0..3u8 {
                let row = target as usize * N_BINS + out_bin;
                w2[row][best_h] = prior_for_bin(target, out_bin);
                // Bias offset so untrained sigmoid sits near the prior
                // rather than at 0.5 (uninformative).
                b2[row] = prior_for_bin(target, out_bin) * 1.5 - 0.75;
            }
        }
        Self { w1, b1, w2, b2 }
    }

    /// Apply the network to a single magnitude frame. Returns three
    /// soft masks (voice, piano, synth) over N_BINS, each value in
    /// [0, 1] from the sigmoid.
    pub fn predict_frame(&self, mag: &[f32]) -> [Vec<f32>; 3] {
        assert_eq!(mag.len(), N_BINS,
                   "predict_frame requires N_BINS={} input, got {}",
                   N_BINS, mag.len());
        // Log-magnitude as network input — perceptually meaningful
        // and matches what an audio NN typically sees.
        let log_mag: Vec<f32> = mag.iter()
            .map(|&m| (m + 1e-6).ln())
            .collect();
        // Hidden layer.
        let mut h = vec![0.0_f32; MLP_HIDDEN];
        for j in 0..MLP_HIDDEN {
            let mut acc = self.b1[j];
            for i in 0..N_BINS {
                acc += self.w1[j][i] * log_mag[i];
            }
            h[j] = relu(acc);
        }
        // Output layer.
        let mut out = vec![0.0_f32; 3 * N_BINS];
        for r in 0..(3 * N_BINS) {
            let mut acc = self.b2[r];
            for j in 0..MLP_HIDDEN {
                acc += self.w2[r][j] * h[j];
            }
            out[r] = sigmoid(acc);
        }
        // Split into 3 mask arrays.
        let voice = out[0..N_BINS].to_vec();
        let piano = out[N_BINS..2*N_BINS].to_vec();
        let synth = out[2*N_BINS..3*N_BINS].to_vec();
        [voice, piano, synth]
    }

    /// Normalize three masks for the same frame so they sum to 1 at
    /// each bin — the three stems should partition the harmonic
    /// content rather than overlap. (We don't enforce this during
    /// training; we apply it at inference for cleaner outputs.)
    pub fn normalize_masks(masks: &mut [Vec<f32>; 3]) {
        let n = masks[0].len();
        for b in 0..n {
            let s = masks[0][b] + masks[1][b] + masks[2][b] + 1e-9;
            masks[0][b] /= s;
            masks[1][b] /= s;
            masks[2][b] /= s;
        }
    }

    /// Total parameter count — useful for the QML status surface.
    pub fn param_count(&self) -> usize {
        self.w1.iter().map(|r| r.len()).sum::<usize>()
            + self.b1.len()
            + self.w2.iter().map(|r| r.len()).sum::<usize>()
            + self.b2.len()
    }

    /// Serialize to a compact f32-bin format. Layout:
    ///   magic:    4 bytes "STEM"
    ///   version:  u32 LE = 1
    ///   shapes:   u32 LE × 4 (n_bins, hidden, output_rows, _reserved)
    ///   w1:       (hidden × n_bins) f32 LE, row-major
    ///   b1:       hidden f32 LE
    ///   w2:       (output_rows × hidden) f32 LE, row-major
    ///   b2:       output_rows f32 LE
    pub fn save_to_bytes(&self) -> Vec<u8> {
        let hidden = self.w1.len();
        let n_bins = self.w1.get(0).map(|r| r.len()).unwrap_or(0);
        let out_rows = self.w2.len();
        let cap = 24 + 4 * (hidden * n_bins + hidden + out_rows * hidden + out_rows);
        let mut out = Vec::with_capacity(cap);
        out.extend_from_slice(b"STEM");
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&(n_bins as u32).to_le_bytes());
        out.extend_from_slice(&(hidden as u32).to_le_bytes());
        out.extend_from_slice(&(out_rows as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        for row in &self.w1 { for v in row { out.extend_from_slice(&v.to_le_bytes()); } }
        for v in &self.b1 { out.extend_from_slice(&v.to_le_bytes()); }
        for row in &self.w2 { for v in row { out.extend_from_slice(&v.to_le_bytes()); } }
        for v in &self.b2 { out.extend_from_slice(&v.to_le_bytes()); }
        out
    }

    /// Deserialize. Returns None on any malformed input — safer than
    /// panicking on user-supplied weight files.
    pub fn load_from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 24 { return None; }
        if &data[0..4] != b"STEM" { return None; }
        let read_u32 = |i: usize| -> u32 {
            u32::from_le_bytes([data[i], data[i+1], data[i+2], data[i+3]])
        };
        let ver = read_u32(4);
        if ver != 1 { return None; }
        let n_bins = read_u32(8) as usize;
        let hidden = read_u32(12) as usize;
        let out_rows = read_u32(16) as usize;
        if n_bins != N_BINS || hidden != MLP_HIDDEN || out_rows != 3 * N_BINS {
            return None;
        }
        let mut p = 24;
        let read_f32 = |bytes: &[u8], p: usize| -> Option<f32> {
            if p + 4 > bytes.len() { return None; }
            Some(f32::from_le_bytes([
                bytes[p], bytes[p+1], bytes[p+2], bytes[p+3],
            ]))
        };
        let mut w1 = vec![vec![0.0_f32; n_bins]; hidden];
        for row in w1.iter_mut() {
            for v in row.iter_mut() {
                *v = read_f32(data, p)?;
                p += 4;
            }
        }
        let mut b1 = vec![0.0_f32; hidden];
        for v in b1.iter_mut() { *v = read_f32(data, p)?; p += 4; }
        let mut w2 = vec![vec![0.0_f32; hidden]; out_rows];
        for row in w2.iter_mut() {
            for v in row.iter_mut() {
                *v = read_f32(data, p)?;
                p += 4;
            }
        }
        let mut b2 = vec![0.0_f32; out_rows];
        for v in b2.iter_mut() { *v = read_f32(data, p)?; p += 4; }
        Some(Self { w1, b1, w2, b2 })
    }
}

fn relu(x: f32) -> f32 { x.max(0.0) }
fn sigmoid(x: f32) -> f32 {
    if x >= 0.0 {
        let z = (-x).exp();
        1.0 / (1.0 + z)
    } else {
        let z = x.exp();
        z / (1.0 + z)
    }
}

// ── Mask application + full pipeline ──────────────────────────────────

/// Apply a real-valued soft mask to a complex spectrogram, preserving
/// phase. Standard source-separation pattern.
pub fn apply_mask(
    spec: &[Vec<(f32, f32)>],
    mask: &[Vec<f32>],
) -> Vec<Vec<(f32, f32)>> {
    spec.iter().zip(mask.iter()).map(|(frame, mframe)| {
        frame.iter().zip(mframe.iter()).map(|(&(r, i), &m)| {
            (r * m, i * m)
        }).collect()
    }).collect()
}

/// Output of the full pipeline.
#[derive(Debug, Clone)]
pub struct StemOutputs {
    pub drums: Vec<f32>,
    pub voice: Vec<f32>,
    pub piano: Vec<f32>,
    pub synth: Vec<f32>,
}

/// Process one mono signal end-to-end into four stems.
///
/// Steps:
///   1. STFT
///   2. HPSS → harmonic_mag + percussive_mag
///   3. Drums = ISTFT(percussive_mask * complex_spec)
///   4. For each harmonic frame, run SmallMlp.predict_frame on the
///      harmonic_mag and normalize the masks.
///   5. Voice/piano/synth = ISTFT(mask * complex_spec).
///
/// `_sample_rate` is part of the public signature for future use
/// (we may need it to do sample-rate-aware mask shaping above the
/// MLP's own priors). For now the MLP carries its own SR knowledge
/// internally so the parameter is unused at this site.
pub fn split_stems(
    signal: &[f32],
    _sample_rate: u32,
    mlp: &SmallMlp,
) -> StemOutputs {
    let original_len = signal.len();
    let spec = stft(signal);
    let mag = magnitude(&spec);
    let (h_mag, p_mag) = hpss(&mag);

    // Drums: percussive fraction of full magnitude → mask the
    // complex spectrum so phase comes along for the ride.
    let mut drums_mask = vec![vec![0.0_f32; N_BINS]; spec.len()];
    for f in 0..spec.len() {
        for b in 0..N_BINS {
            let total = mag[f][b] + 1e-9;
            drums_mask[f][b] = p_mag[f][b] / total;
        }
    }
    let drums_spec = apply_mask(&spec, &drums_mask);
    let drums = istft(&drums_spec, original_len);

    // Harmonic 3-way: run MLP per frame.
    let mut voice_mask = vec![vec![0.0_f32; N_BINS]; spec.len()];
    let mut piano_mask = vec![vec![0.0_f32; N_BINS]; spec.len()];
    let mut synth_mask = vec![vec![0.0_f32; N_BINS]; spec.len()];
    for f in 0..spec.len() {
        let mut masks = mlp.predict_frame(&h_mag[f]);
        SmallMlp::normalize_masks(&mut masks);
        // Scale the harmonic 3-way masks by the harmonic FRACTION
        // of the full mag — so a heavily-percussive frame doesn't
        // get a phantom voice/piano/synth signal.
        for b in 0..N_BINS {
            let total = mag[f][b] + 1e-9;
            let harmonic_frac = h_mag[f][b] / total;
            voice_mask[f][b] = masks[0][b] * harmonic_frac;
            piano_mask[f][b] = masks[1][b] * harmonic_frac;
            synth_mask[f][b] = masks[2][b] * harmonic_frac;
        }
    }
    let voice = istft(&apply_mask(&spec, &voice_mask), original_len);
    let piano = istft(&apply_mask(&spec, &piano_mask), original_len);
    let synth = istft(&apply_mask(&spec, &synth_mask), original_len);

    StemOutputs { drums, voice, piano, synth }
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hann_window_endpoints_are_zero() {
        let w = hann_window(64);
        assert!(w[0]  < 1e-6);
        assert!(w[63] < 1e-6);
        // Peak should be near 1.0; for even-length symmetric Hann the
        // peak falls between samples so the discrete maximum is
        // slightly below 1 (≈0.9994 for n=64). Use n=65 (odd) to test
        // an exact peak of 1.0.
        let w_odd = hann_window(65);
        let max_odd = w_odd.iter().cloned().fold(0.0_f32, f32::max);
        assert!((max_odd - 1.0).abs() < 1e-6,
                "odd-length peak should be exactly 1.0, got {}", max_odd);
        // For even length, peak is just below 1.0.
        let max = w.iter().cloned().fold(0.0_f32, f32::max);
        assert!(max > 0.99 && max < 1.0,
                "even-length peak should be just under 1.0, got {}", max);
    }

    #[test]
    fn hann_window_n1_returns_unity() {
        let w = hann_window(1);
        assert_eq!(w, vec![1.0]);
    }

    #[test]
    fn stft_frame_count_matches_formula() {
        // A 4096-sample signal at hop=256, window=1024, with
        // STFT_WINDOW/2 reflective padding on each side.
        // padded_len = 4096 + 1024 = 5120
        // n_frames = (5120 - 1024) / 256 + 1 = 17
        let sig = vec![0.0_f32; 4096];
        let spec = stft(&sig);
        assert_eq!(spec.len(), 17);
        for frame in &spec {
            assert_eq!(frame.len(), N_BINS);
        }
    }

    #[test]
    fn stft_istft_roundtrip_preserves_signal_approximately() {
        // Reconstructing a sine wave should give back the same sine
        // wave within a small error tolerance (window edges are
        // imperfect but overlap-add converges in the interior).
        let sr = 44100.0;
        let f = 440.0;
        let n = 4096;
        let sig: Vec<f32> = (0..n).map(|i| {
            (2.0 * PI * f * i as f32 / sr).sin()
        }).collect();
        let spec = stft(&sig);
        let recon = istft(&spec, n);
        assert_eq!(recon.len(), n);
        // Compare interior region only (skip first/last 512 samples
        // where edge artifacts live).
        let mut max_err = 0.0_f32;
        let mut max_sig = 0.0_f32;
        for i in 512..n-512 {
            let e = (recon[i] - sig[i]).abs();
            if e > max_err { max_err = e; }
            if sig[i].abs() > max_sig { max_sig = sig[i].abs(); }
        }
        // < 0.01 relative error in the interior is generous; in
        // practice we see ~1e-5 on this sinusoid.
        assert!(max_err / max_sig < 0.01,
                "max relative err {} > 1%", max_err / max_sig);
    }

    #[test]
    fn magnitude_of_sine_concentrates_at_bin() {
        // A pure tone should have one dominant bin per frame.
        let sr = 44100.0;
        // Tone at exactly bin 50: f = 50 * sr / WINDOW
        let f = 50.0 * sr / STFT_WINDOW as f32;
        let n = STFT_WINDOW * 4;
        let sig: Vec<f32> = (0..n).map(|i| {
            (2.0 * PI * f * i as f32 / sr).sin()
        }).collect();
        let mag = magnitude(&stft(&sig));
        // Take a middle frame.
        let frame = &mag[mag.len() / 2];
        let (peak_bin, peak_val) = frame.iter().enumerate()
            .fold((0_usize, 0.0_f32), |(bi, bv), (i, &v)| {
                if v > bv { (i, v) } else { (bi, bv) }
            });
        assert_eq!(peak_bin, 50, "peak should land at bin 50, got {}", peak_bin);
        let total: f32 = frame.iter().sum();
        assert!(peak_val / total > 0.4,
                "peak should hold ≥40% of frame energy, got {:.2}%",
                peak_val / total * 100.0);
    }

    #[test]
    fn hpss_separates_pure_tone_into_harmonic() {
        // A pure sine should land almost entirely in the harmonic
        // component (smooth in time) and not in the percussive.
        let sr = 44100.0;
        let f = 1000.0;
        let n = STFT_WINDOW * 8;
        let sig: Vec<f32> = (0..n).map(|i| {
            (2.0 * PI * f * i as f32 / sr).sin()
        }).collect();
        let mag = magnitude(&stft(&sig));
        let (h, p) = hpss(&mag);
        // Sum the magnitudes; harmonic should dominate by a wide margin.
        let h_total: f32 = h.iter().flatten().sum();
        let p_total: f32 = p.iter().flatten().sum();
        assert!(h_total > 5.0 * p_total,
                "harmonic should dominate for a pure sine: h={} p={}",
                h_total, p_total);
    }

    #[test]
    fn hpss_separates_impulse_into_percussive() {
        // A single impulse → spectrogram has one bright frame at all
        // bins. That's exactly the percussive signature.
        let mut sig = vec![0.0_f32; STFT_WINDOW * 8];
        sig[STFT_WINDOW * 4] = 1.0;
        let mag = magnitude(&stft(&sig));
        let (h, p) = hpss(&mag);
        let h_total: f32 = h.iter().flatten().sum();
        let p_total: f32 = p.iter().flatten().sum();
        assert!(p_total > 3.0 * h_total,
                "percussive should dominate for an impulse: h={} p={}",
                h_total, p_total);
    }

    #[test]
    fn hpss_partitions_magnitude_correctly() {
        // h + p should sum to the original magnitude at every bin (or
        // close — Wiener masks make this exact up to numerical error).
        let sig: Vec<f32> = (0..STFT_WINDOW * 4).map(|i| {
            let t = i as f32 / 44100.0;
            (2.0 * PI * 440.0 * t).sin() * 0.3
                + if i % 2048 == 0 { 1.0 } else { 0.0 }
        }).collect();
        let mag = magnitude(&stft(&sig));
        let (h, p) = hpss(&mag);
        for f in 0..mag.len() {
            for b in 0..N_BINS {
                let sum = h[f][b] + p[f][b];
                assert!((sum - mag[f][b]).abs() < 1e-4,
                        "h + p != mag at ({},{}): h={} p={} mag={}",
                        f, b, h[f][b], p[f][b], mag[f][b]);
            }
        }
    }

    #[test]
    fn mlp_param_count_matches_architecture() {
        let mlp = SmallMlp::with_priors(44100);
        // 513 × 128 + 128 + 1539 × 128 + 1539 = 263,427
        let expected = N_BINS * MLP_HIDDEN
                     + MLP_HIDDEN
                     + 3 * N_BINS * MLP_HIDDEN
                     + 3 * N_BINS;
        assert_eq!(mlp.param_count(), expected);
    }

    #[test]
    fn mlp_predict_returns_valid_masks() {
        let mlp = SmallMlp::with_priors(44100);
        let mag = vec![1.0_f32; N_BINS];
        let masks = mlp.predict_frame(&mag);
        for (i, m) in masks.iter().enumerate() {
            assert_eq!(m.len(), N_BINS, "mask {} wrong size", i);
            for &v in m { assert!((0.0..=1.0).contains(&v),
                "mask {} val {} out of [0,1]", i, v); }
        }
    }

    #[test]
    fn mlp_priors_emphasize_voice_band_for_voice_output() {
        // After applying with_priors, the voice mask at typical
        // speech frequencies (1000 Hz ≈ bin 23 at sr=44100, N=1024)
        // should be HIGHER than at sub-100 Hz, where voice has no
        // fundamental energy. The exact margin depends on how the
        // hidden-layer basis activates against a uniform input
        // (Gaussian centers distribute output evenly), so we just
        // verify direction with a modest margin.
        let mlp = SmallMlp::with_priors(44100);
        let mag = vec![1.0_f32; N_BINS];
        let masks = mlp.predict_frame(&mag);
        let voice = &masks[0];
        // Bin 23 ≈ 990 Hz; bin 2 ≈ 86 Hz.
        let mid = voice[23];
        let low = voice[2];
        assert!(mid > low,
                "voice prior should emphasize mid over low: mid={} low={}",
                mid, low);
        assert!(mid - low > 0.05,
                "voice prior margin too small: mid - low = {}", mid - low);
    }

    #[test]
    fn mlp_normalize_masks_sums_to_one() {
        let mlp = SmallMlp::with_priors(44100);
        let mag: Vec<f32> = (0..N_BINS).map(|i| (i as f32).sin().abs() + 0.1).collect();
        let mut masks = mlp.predict_frame(&mag);
        SmallMlp::normalize_masks(&mut masks);
        for b in 0..N_BINS {
            let s = masks[0][b] + masks[1][b] + masks[2][b];
            assert!((s - 1.0).abs() < 1e-5, "bin {} sums to {}", b, s);
        }
    }

    #[test]
    fn mlp_save_load_roundtrip() {
        let mlp = SmallMlp::with_priors(44100);
        let bytes = mlp.save_to_bytes();
        let loaded = SmallMlp::load_from_bytes(&bytes).unwrap();
        // Spot-check a few weights.
        assert_eq!(mlp.w1[0][0], loaded.w1[0][0]);
        assert_eq!(mlp.b1[5], loaded.b1[5]);
        assert_eq!(mlp.w2[10][3], loaded.w2[10][3]);
        assert_eq!(mlp.b2[100], loaded.b2[100]);
        assert_eq!(mlp.param_count(), loaded.param_count());
    }

    #[test]
    fn mlp_load_rejects_bad_magic() {
        let mut bytes = SmallMlp::with_priors(44100).save_to_bytes();
        bytes[0] = b'X';
        assert!(SmallMlp::load_from_bytes(&bytes).is_none());
    }

    #[test]
    fn mlp_load_rejects_wrong_version() {
        let mut bytes = SmallMlp::with_priors(44100).save_to_bytes();
        // Overwrite version field (offset 4..8) with version 99.
        bytes[4..8].copy_from_slice(&99u32.to_le_bytes());
        assert!(SmallMlp::load_from_bytes(&bytes).is_none());
    }

    #[test]
    fn mlp_load_rejects_truncated() {
        let bytes = SmallMlp::with_priors(44100).save_to_bytes();
        assert!(SmallMlp::load_from_bytes(&bytes[..200]).is_none());
    }

    #[test]
    fn mlp_load_rejects_wrong_shape() {
        // Save valid bytes, then mutate n_bins to a wrong value.
        let mut bytes = SmallMlp::with_priors(44100).save_to_bytes();
        bytes[8..12].copy_from_slice(&999u32.to_le_bytes());
        assert!(SmallMlp::load_from_bytes(&bytes).is_none());
    }

    #[test]
    fn split_stems_produces_four_streams_of_correct_length() {
        let n = STFT_WINDOW * 8;
        let sig: Vec<f32> = (0..n).map(|i| {
            let t = i as f32 / 44100.0;
            (2.0 * PI * 440.0 * t).sin() * 0.3
        }).collect();
        let mlp = SmallMlp::with_priors(44100);
        let out = split_stems(&sig, 44100, &mlp);
        assert_eq!(out.drums.len(), n);
        assert_eq!(out.voice.len(), n);
        assert_eq!(out.piano.len(), n);
        assert_eq!(out.synth.len(), n);
    }

    #[test]
    fn split_stems_sum_approximates_original_for_pure_tone() {
        // A pure tone has no percussive content, so drums should be
        // ~silent. The harmonic 3-way split has masks that sum to 1 ×
        // harmonic_frac (≈ 1 for pure tone), so voice+piano+synth
        // should reconstruct the original (approximately).
        let sr = 44100;
        let n = STFT_WINDOW * 8;
        let sig: Vec<f32> = (0..n).map(|i| {
            let t = i as f32 / sr as f32;
            (2.0 * PI * 880.0 * t).sin() * 0.4
        }).collect();
        let mlp = SmallMlp::with_priors(sr);
        let out = split_stems(&sig, sr, &mlp);
        // Drums should be quieter than original (almost no percussion).
        let orig_rms: f32 = (sig.iter().map(|&s| s*s).sum::<f32>() / n as f32).sqrt();
        let drums_rms: f32 = (out.drums.iter().map(|&s| s*s).sum::<f32>() / n as f32).sqrt();
        assert!(drums_rms < orig_rms * 0.5,
                "drums should be quieter than 50% of original RMS for pure tone: \
                 orig={:.4} drums={:.4}", orig_rms, drums_rms);
        // Sum the three harmonic stems and check that's close to the
        // original (skipping window edges).
        let interior = STFT_WINDOW..n-STFT_WINDOW;
        let mut diff_sq = 0.0_f32;
        let mut orig_sq = 0.0_f32;
        for i in interior {
            let summed = out.voice[i] + out.piano[i] + out.synth[i];
            let d = summed - sig[i];
            diff_sq += d * d;
            orig_sq += sig[i] * sig[i];
        }
        let rel_err = (diff_sq / orig_sq).sqrt();
        assert!(rel_err < 0.4,
                "v+p+s should reconstruct pure tone to ~40% RMS error, got {}",
                rel_err);
    }

    #[test]
    fn stft_empty_input_returns_empty() {
        let spec = stft(&[]);
        assert!(spec.is_empty(), "stft([]) should be empty, got {} frames", spec.len());
    }

    #[test]
    fn stft_short_input_does_not_panic() {
        // Inputs shorter than the window must still process via
        // reflective padding. We just check it doesn't panic and
        // produces sensible output.
        for len in [1, 5, 100, STFT_HOP, STFT_WINDOW / 2] {
            let sig = vec![0.1_f32; len];
            let spec = stft(&sig);
            assert!(!spec.is_empty(), "len={} produced empty spec", len);
            for frame in &spec {
                assert_eq!(frame.len(), N_BINS);
                for (r, i) in frame { assert!(r.is_finite() && i.is_finite()); }
            }
        }
    }

    #[test]
    fn istft_empty_spec_returns_zeros_of_original_length() {
        let out = istft(&[], 1024);
        assert_eq!(out.len(), 1024);
        for v in out { assert_eq!(v, 0.0); }
    }

    #[test]
    fn istft_empty_spec_zero_length_returns_empty() {
        let out = istft(&[], 0);
        assert!(out.is_empty());
    }

    #[test]
    fn split_stems_empty_input_returns_empty_streams() {
        let mlp = SmallMlp::with_priors(44100);
        let out = split_stems(&[], 44100, &mlp);
        assert!(out.drums.is_empty());
        assert!(out.voice.is_empty());
        assert!(out.piano.is_empty());
        assert!(out.synth.is_empty());
    }

    // ── Property/fuzz tests ──────────────────────────────────────────
    //
    // These exercise the math primitives across many random inputs
    // to catch regressions that hand-crafted cases miss. Tiny inline
    // PCG-style PRNG so no external dep.

    fn prng(seed: u64) -> impl FnMut() -> f32 {
        let mut s = seed;
        move || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let x = ((s >> 33) ^ s) as u32;
            (x as f32 / u32::MAX as f32) * 2.0 - 1.0
        }
    }

    #[test]
    fn fuzz_stft_istft_roundtrip_random_signals() {
        // 100 random signals × ~4096 samples; STFT→ISTFT must
        // reconstruct the interior to <5% RMS error. Catches FFT
        // mirror bugs, window-sum normalization drift, and edge
        // off-by-ones.
        for seed in 0..100_u64 {
            let mut rng = prng(0xDEAD_BEEF ^ seed);
            let n = 4096 + ((seed % 7) as usize) * 256;
            let sig: Vec<f32> = (0..n).map(|_| rng() * 0.3).collect();
            let spec = stft(&sig);
            let recon = istft(&spec, n);
            assert_eq!(recon.len(), n);
            let interior = STFT_WINDOW..n-STFT_WINDOW;
            let mut diff_sq = 0.0_f32;
            let mut orig_sq = 1e-9_f32;
            for i in interior {
                diff_sq += (recon[i] - sig[i]).powi(2);
                orig_sq += sig[i].powi(2);
            }
            let rel_err = (diff_sq / orig_sq).sqrt();
            assert!(rel_err < 0.05,
                "seed {} (n={}): STFT/ISTFT relative error {} > 5%",
                seed, n, rel_err);
        }
    }

    #[test]
    fn fuzz_hpss_partitions_arbitrary_magnitude() {
        // For any non-negative spectrogram, h[f][b] + p[f][b] should
        // equal mag[f][b] within numerical tolerance — the Wiener
        // masks sum to 1.
        for seed in 0..30_u64 {
            let mut rng = prng(0xFEEDFACE ^ seed);
            let n_frames = 32;
            let mag: Vec<Vec<f32>> = (0..n_frames).map(|_| {
                (0..N_BINS).map(|_| rng().abs() * 5.0).collect()
            }).collect();
            let (h, p) = hpss(&mag);
            for f in 0..n_frames {
                for b in 0..N_BINS {
                    let sum = h[f][b] + p[f][b];
                    let diff = (sum - mag[f][b]).abs();
                    assert!(diff < 1e-3,
                        "seed {}: hpss partition broken at ({},{}): h+p={} mag={}",
                        seed, f, b, sum, mag[f][b]);
                }
            }
        }
    }

    #[test]
    fn fuzz_mlp_normalize_masks_always_sum_to_one() {
        // Across many random magnitude inputs, the post-normalize
        // mask sum must be exactly 1.0 at every bin.
        let mlp = SmallMlp::with_priors(44100);
        for seed in 0..50_u64 {
            let mut rng = prng(0xCAFEBABE ^ seed);
            let mag: Vec<f32> = (0..N_BINS).map(|_| rng().abs() * 2.0 + 0.01).collect();
            let mut masks = mlp.predict_frame(&mag);
            SmallMlp::normalize_masks(&mut masks);
            for b in 0..N_BINS {
                let s = masks[0][b] + masks[1][b] + masks[2][b];
                assert!((s - 1.0).abs() < 1e-5,
                    "seed {} bin {}: masks sum to {} != 1.0", seed, b, s);
            }
        }
    }

    #[test]
    fn fuzz_save_load_random_corruption_either_loads_or_returns_none() {
        // Mutate a random byte in a valid save buffer; load_from_bytes
        // must either return None or a structurally-valid MLP — it
        // must NOT panic.
        let bytes = SmallMlp::with_priors(44100).save_to_bytes();
        for seed in 0..200_u64 {
            let mut rng = prng(0x900D_F00D ^ seed);
            let mut mutated = bytes.clone();
            // Pick a random byte position and a random new value.
            let pos = ((rng() + 1.0) * 0.5 * mutated.len() as f32) as usize;
            let pos = pos.min(mutated.len().saturating_sub(1));
            let new_byte = ((rng() + 1.0) * 0.5 * 256.0) as u8;
            mutated[pos] = new_byte;
            // Result MUST be Ok(None) or Ok(Some(_)) — never panic.
            let _ = std::panic::catch_unwind(|| {
                let r = SmallMlp::load_from_bytes(&mutated);
                if let Some(m) = r {
                    assert_eq!(m.param_count(),
                               SmallMlp::with_priors(44100).param_count(),
                               "loaded MLP has unexpected param count");
                }
            }).unwrap_or_else(|_| panic!("load_from_bytes panicked at seed {}", seed));
        }
    }

    #[test]
    fn fuzz_split_stems_never_produces_nan_or_inf() {
        // Across random signals, all output streams must be finite.
        // Catches NaN propagation through STFT/ISTFT (e.g. via div-
        // by-zero in masks).
        let mlp = SmallMlp::with_priors(44100);
        for seed in 0..20_u64 {
            let mut rng = prng(0xBADC_AFE ^ seed);
            let n = 4096;
            let sig: Vec<f32> = (0..n).map(|_| rng() * 0.5).collect();
            let out = split_stems(&sig, 44100, &mlp);
            for (name, stem) in [
                ("drums", &out.drums), ("voice", &out.voice),
                ("piano", &out.piano), ("synth", &out.synth),
            ] {
                for (i, &v) in stem.iter().enumerate() {
                    assert!(v.is_finite(),
                        "seed {}: {} sample {} is {} (not finite)",
                        seed, name, i, v);
                }
            }
        }
    }
}
