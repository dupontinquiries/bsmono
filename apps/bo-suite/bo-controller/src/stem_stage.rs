// src/stem_stage.rs
#![allow(dead_code)]
//
// Virtual stereo staging for 4 mono stems.
//
// Goals:
//   - Place each stem at a distinct spatial position so listeners can
//     hear them as separate elements without headphone-only HRTF tricks.
//   - Keep latency low — the output is a single stereo pass over each
//     stem with at most a few-ms delay line.
//   - No external deps: classical panning + Haas (precedence) effect
//     + an early-reflections approximation for depth.
//
// Placement defaults:
//
//      L 60°        L 30°        center        R 30°        R 60°
//        │           │             │             │           │
//      synth       piano         voice         drums       (free)
//        │           │             │             │
//      Haas-spread  10ms early   slight depth  centered
//                   reflection    via stereo    + tiny
//                                 spreader      pre-delay
//
// Why these placements:
//   - Voice dead center: vocal-forward mixes feel natural and let the
//     listener focus on lyrics/dialogue.
//   - Drums center-but-front: anchoring the rhythm without crowding
//     the vocal frequencies.
//   - Piano off-axis left: classical positioning (most concert piano
//     placements have the keyboard on the left as you face the stage).
//   - Synth wide via Haas: pads and leads sound bigger with wide
//     spread; the Haas trick (delay one channel 5–25 ms) creates
//     phantom width without phase cancellation.
//
// All placements are caller-configurable via `StageConfig`.

use std::f32::consts::PI;

/// Sample-rate-aware delay line in milliseconds. Used for Haas and
/// early-reflection placement. Returns a `Vec<f32>` of the input
/// delayed by `delay_ms`; pads the head with zeros.
fn delay_ms(signal: &[f32], delay_ms: f32, sample_rate: u32) -> Vec<f32> {
    let d_samples = (delay_ms * sample_rate as f32 / 1000.0) as usize;
    let mut out = vec![0.0_f32; d_samples];
    out.extend_from_slice(signal);
    out
}

/// Equal-power pan: takes mono input and produces (L, R) at the
/// requested azimuth. `pan` ∈ [-1.0, 1.0] (-1 = full left, +1 = full
/// right, 0 = center). Equal-power keeps the perceived loudness
/// constant across the pan field.
fn equal_power_pan(samples: &[f32], pan: f32) -> (Vec<f32>, Vec<f32>) {
    let pan = pan.clamp(-1.0, 1.0);
    // Map pan [-1,1] → angle [0, π/2].
    let theta = (pan + 1.0) * PI / 4.0;
    let l_gain = theta.cos();
    let r_gain = theta.sin();
    let l: Vec<f32> = samples.iter().map(|s| s * l_gain).collect();
    let r: Vec<f32> = samples.iter().map(|s| s * r_gain).collect();
    (l, r)
}

/// Mix two mono streams into one with per-stream gains. Used to layer
/// the direct signal with its early reflection.
fn mix(a: &[f32], a_gain: f32, b: &[f32], b_gain: f32) -> Vec<f32> {
    let n = a.len().max(b.len());
    let mut out = vec![0.0_f32; n];
    for i in 0..n {
        let av = a.get(i).copied().unwrap_or(0.0);
        let bv = b.get(i).copied().unwrap_or(0.0);
        out[i] = av * a_gain + bv * b_gain;
    }
    out
}

/// Per-stem placement parameters.
#[derive(Debug, Clone, Copy)]
pub struct StemPlacement {
    /// Equal-power pan position [-1.0, 1.0]. -1 = hard left.
    pub pan: f32,
    /// Per-channel delay (ms) used for Haas-style widening. Positive
    /// = right channel arrives later; negative = left channel later.
    /// Values in [-25.0, 25.0] are safe; larger creates audible
    /// echo rather than spatial cue.
    pub haas_ms: f32,
    /// Early-reflection delay (ms). 0.0 disables the reflection.
    /// 5-30 ms gives subtle depth; >50 ms sounds like distinct echo.
    pub early_ms: f32,
    /// Early-reflection gain. 0.2 = audible but distinctly secondary.
    pub early_gain: f32,
    /// Overall gain for this stem in the final mix [0.0, 2.0].
    pub gain: f32,
}

impl StemPlacement {
    /// Voice: dead center, narrow stereo spread for slight depth.
    pub fn voice_default() -> Self {
        Self { pan: 0.0, haas_ms: 0.0, early_ms: 8.0, early_gain: 0.10, gain: 1.0 }
    }
    /// Drums: center-but-front, very slight pre-delay on the right
    /// for body. Don't let drums spread wide — that loses the kick.
    pub fn drums_default() -> Self {
        Self { pan: 0.0, haas_ms: 2.0, early_ms: 0.0, early_gain: 0.0, gain: 1.0 }
    }
    /// Piano: left ~30° with a clear early reflection from the right.
    /// Mimics a stage placement of the instrument off to one side.
    pub fn piano_default() -> Self {
        Self { pan: -0.4, haas_ms: -3.0, early_ms: 12.0, early_gain: 0.15, gain: 0.95 }
    }
    /// Synth: wide via Haas (right channel delayed 15 ms) and pulled
    /// further off-axis. Synth pads feel BIG with this treatment.
    pub fn synth_default() -> Self {
        Self { pan: 0.3, haas_ms: 15.0, early_ms: 18.0, early_gain: 0.12, gain: 0.85 }
    }
}

/// Full configuration for staging all four stems.
#[derive(Debug, Clone)]
pub struct StageConfig {
    pub voice: StemPlacement,
    pub drums: StemPlacement,
    pub piano: StemPlacement,
    pub synth: StemPlacement,
    pub sample_rate: u32,
}

impl StageConfig {
    pub fn default_for(sample_rate: u32) -> Self {
        Self {
            voice: StemPlacement::voice_default(),
            drums: StemPlacement::drums_default(),
            piano: StemPlacement::piano_default(),
            synth: StemPlacement::synth_default(),
            sample_rate,
        }
    }
}

/// Apply per-stem placement to one mono stream, returning a stereo
/// (L, R) pair. Internal helper for `stage`.
fn place(mono: &[f32], p: &StemPlacement, sample_rate: u32) -> (Vec<f32>, Vec<f32>) {
    // Apply per-stem gain BEFORE spatial processing (so reflections
    // scale too).
    let gained: Vec<f32> = mono.iter().map(|s| s * p.gain).collect();
    let (mut l, mut r) = equal_power_pan(&gained, p.pan);

    // Haas widening: delay one channel relative to the other.
    if p.haas_ms != 0.0 {
        if p.haas_ms > 0.0 {
            // Right channel arrives later → push perceptual image LEFT.
            r = delay_ms(&r, p.haas_ms.abs(), sample_rate);
        } else {
            l = delay_ms(&l, p.haas_ms.abs(), sample_rate);
        }
    }

    // Early reflection (single tap) mixed into the OPPOSITE channel.
    // This is the cheap-and-cheerful way to suggest a room without a
    // full convolution reverb.
    if p.early_ms > 0.0 && p.early_gain > 0.0 {
        let refl = delay_ms(&gained, p.early_ms, sample_rate);
        if p.pan <= 0.0 {
            // Stem is on the left; reflection bounces to the right.
            r = mix(&r, 1.0, &refl, p.early_gain);
        } else {
            l = mix(&l, 1.0, &refl, p.early_gain);
        }
    }
    (l, r)
}

/// Combine four mono stems into a final stereo mix per the config.
/// Output channels are the same length as the longest staged stream
/// (which may exceed input length by haas + early delays). Zero-pads
/// short streams so they align in time.
pub fn stage(
    voice: &[f32], drums: &[f32], piano: &[f32], synth: &[f32],
    cfg: &StageConfig,
) -> (Vec<f32>, Vec<f32>) {
    let (vl, vr) = place(voice, &cfg.voice, cfg.sample_rate);
    let (dl, dr) = place(drums, &cfg.drums, cfg.sample_rate);
    let (pl, pr) = place(piano, &cfg.piano, cfg.sample_rate);
    let (sl, sr) = place(synth, &cfg.synth, cfg.sample_rate);
    let total = vl.len().max(dl.len()).max(pl.len()).max(sl.len());
    let mut left  = vec![0.0_f32; total];
    let mut right = vec![0.0_f32; total];
    for i in 0..total {
        left[i]  = vl.get(i).copied().unwrap_or(0.0)
                 + dl.get(i).copied().unwrap_or(0.0)
                 + pl.get(i).copied().unwrap_or(0.0)
                 + sl.get(i).copied().unwrap_or(0.0);
        right[i] = vr.get(i).copied().unwrap_or(0.0)
                 + dr.get(i).copied().unwrap_or(0.0)
                 + pr.get(i).copied().unwrap_or(0.0)
                 + sr.get(i).copied().unwrap_or(0.0);
    }
    (left, right)
}

/// Interleave (L, R) into a single Vec<f32> in the order L0,R0,L1,R1,…
/// Used when writing the staged result to a stereo WAV.
pub fn interleave(left: &[f32], right: &[f32]) -> Vec<f32> {
    let n = left.len().min(right.len());
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        out.push(left[i]);
        out.push(right[i]);
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equal_power_pan_center_is_equal_gains() {
        let sig = vec![1.0_f32; 16];
        let (l, r) = equal_power_pan(&sig, 0.0);
        let expected = (PI / 4.0).cos();
        for i in 0..16 {
            assert!((l[i] - expected).abs() < 1e-6, "L bin {} = {}", i, l[i]);
            assert!((r[i] - expected).abs() < 1e-6, "R bin {} = {}", i, r[i]);
        }
    }

    #[test]
    fn equal_power_pan_full_left_silences_right() {
        let sig = vec![1.0_f32; 8];
        let (_, r) = equal_power_pan(&sig, -1.0);
        for v in r { assert!(v.abs() < 1e-6); }
    }

    #[test]
    fn equal_power_pan_full_right_silences_left() {
        let sig = vec![1.0_f32; 8];
        let (l, _) = equal_power_pan(&sig, 1.0);
        for v in l { assert!(v.abs() < 1e-6); }
    }

    #[test]
    fn equal_power_pan_preserves_total_power() {
        // L² + R² should equal input² regardless of pan position.
        let sig = vec![0.7_f32; 8];
        for pan in [-1.0, -0.5, 0.0, 0.3, 0.8, 1.0] {
            let (l, r) = equal_power_pan(&sig, pan);
            for i in 0..8 {
                let power = l[i] * l[i] + r[i] * r[i];
                let expected = sig[i] * sig[i];
                assert!((power - expected).abs() < 1e-5,
                    "pan {} bin {} total power {} != input² {}",
                    pan, i, power, expected);
            }
        }
    }

    #[test]
    fn equal_power_pan_clamps_out_of_range() {
        // pan=5.0 should clamp to 1.0 (full right).
        let sig = vec![1.0_f32; 4];
        let (l, _) = equal_power_pan(&sig, 5.0);
        for v in l { assert!(v.abs() < 1e-6); }
    }

    #[test]
    fn delay_ms_pads_front_with_zeros() {
        let sig = vec![1.0_f32; 100];
        let d = delay_ms(&sig, 1.0, 1000); // 1ms at 1kHz = 1 sample
        assert_eq!(d.len(), 101);
        assert_eq!(d[0], 0.0);
        assert_eq!(d[1], 1.0);
    }

    #[test]
    fn delay_ms_zero_is_identity() {
        let sig = vec![1.0_f32, 2.0, 3.0];
        let d = delay_ms(&sig, 0.0, 44100);
        assert_eq!(d, sig);
    }

    #[test]
    fn stage_produces_balanced_stereo() {
        // Equal mono input on all four stems with default placements;
        // result should have nonzero energy in both channels.
        let n = 4410;
        let s: Vec<f32> = (0..n).map(|i| (i as f32 / 100.0).sin() * 0.3).collect();
        let cfg = StageConfig::default_for(44100);
        let (l, r) = stage(&s, &s, &s, &s, &cfg);
        let l_rms: f32 = (l.iter().map(|v| v*v).sum::<f32>() / l.len() as f32).sqrt();
        let r_rms: f32 = (r.iter().map(|v| v*v).sum::<f32>() / r.len() as f32).sqrt();
        assert!(l_rms > 0.05, "L too quiet: {}", l_rms);
        assert!(r_rms > 0.05, "R too quiet: {}", r_rms);
    }

    #[test]
    fn stage_voice_centered_yields_equal_l_r() {
        // With ONLY voice (default = pure center), L and R should be
        // nearly equal RMS (the early reflection adds a tiny bias).
        let n = 4410;
        let s: Vec<f32> = (0..n).map(|i| (i as f32 / 80.0).sin() * 0.3).collect();
        let silence = vec![0.0; n];
        let cfg = StageConfig::default_for(44100);
        let (l, r) = stage(&s, &silence, &silence, &silence, &cfg);
        let l_rms: f32 = (l.iter().map(|v| v*v).sum::<f32>() / l.len() as f32).sqrt();
        let r_rms: f32 = (r.iter().map(|v| v*v).sum::<f32>() / r.len() as f32).sqrt();
        // Voice default has early_ms=8, early_gain=0.10. Since voice's
        // pan is 0 (left side per the `<= 0.0` branch), reflection goes
        // to R. So R is slightly louder. Tolerance accommodates that.
        let ratio = l_rms / r_rms;
        assert!(ratio > 0.85 && ratio < 1.15,
                "voice center L/R ratio {} not near 1.0", ratio);
    }

    #[test]
    fn stage_piano_left_yields_louder_left() {
        let n = 4410;
        let s: Vec<f32> = (0..n).map(|i| (i as f32 / 80.0).sin() * 0.3).collect();
        let silence = vec![0.0; n];
        let cfg = StageConfig::default_for(44100);
        let (l, r) = stage(&silence, &silence, &s, &silence, &cfg);
        let l_rms: f32 = (l.iter().map(|v| v*v).sum::<f32>() / l.len() as f32).sqrt();
        let r_rms: f32 = (r.iter().map(|v| v*v).sum::<f32>() / r.len() as f32).sqrt();
        assert!(l_rms > r_rms * 1.3,
                "piano default pan=-0.4 should make L louder: L={} R={}",
                l_rms, r_rms);
    }

    #[test]
    fn interleave_alternates_samples() {
        let l = vec![1.0_f32, 3.0, 5.0];
        let r = vec![2.0_f32, 4.0, 6.0];
        let inter = interleave(&l, &r);
        assert_eq!(inter, vec![1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn interleave_handles_unequal_lengths() {
        let l = vec![1.0_f32, 2.0, 3.0];
        let r = vec![4.0_f32, 5.0];
        let inter = interleave(&l, &r);
        // Takes min length (2 samples).
        assert_eq!(inter, vec![1.0, 4.0, 2.0, 5.0]);
    }

    #[test]
    fn placement_gain_scales_output() {
        let n = 1000;
        let s = vec![1.0_f32; n];
        let mut p = StemPlacement::voice_default();
        p.gain = 0.5;
        let (l, _r) = place(&s, &p, 44100);
        // Center pan with gain=0.5 → both channels should have
        // peak around 0.5 * cos(π/4) ≈ 0.353.
        let peak = l.iter().cloned().fold(0.0_f32, f32::max);
        assert!((peak - 0.5 * (PI / 4.0).cos()).abs() < 0.05,
                "gain 0.5 peak should be ≈0.353, got {}", peak);
    }

    #[test]
    fn placement_zero_gain_silences() {
        let s = vec![1.0_f32; 100];
        let mut p = StemPlacement::voice_default();
        p.gain = 0.0;
        let (l, r) = place(&s, &p, 44100);
        for v in l.iter().chain(r.iter()) {
            assert!(v.abs() < 1e-6);
        }
    }

    // ── Property/fuzz tests ──────────────────────────────────────────

    fn prng(seed: u64) -> impl FnMut() -> f32 {
        let mut s = seed;
        move || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let x = ((s >> 33) ^ s) as u32;
            (x as f32 / u32::MAX as f32) * 2.0 - 1.0
        }
    }

    #[test]
    fn fuzz_equal_power_pan_preserves_total_power() {
        // For any pan position and any input sample, L² + R² == in².
        // Foundation property — if this breaks, panning is broken.
        for seed in 0..200_u64 {
            let mut rng = prng(0xACE_1F8 ^ seed);
            let pan = rng();
            let in_v = rng();
            let (l, r) = equal_power_pan(&[in_v], pan);
            let power = l[0] * l[0] + r[0] * r[0];
            let expected = in_v * in_v;
            assert!((power - expected).abs() < 1e-5,
                "seed {}: pan={} in={} → L²+R²={} != {}",
                seed, pan, in_v, power, expected);
        }
    }

    #[test]
    fn fuzz_stage_never_panics_on_random_inputs() {
        // Stage must handle empty, short, and long mixtures of
        // arbitrary stems without panicking.
        for seed in 0..30_u64 {
            let mut rng = prng(0xF00D_BEEF ^ seed);
            let len = ((rng() + 1.0) * 0.5 * 4096.0) as usize + 64;
            let make_stream = |rng: &mut dyn FnMut() -> f32, len: usize| -> Vec<f32> {
                (0..len).map(|_| rng() * 0.5).collect()
            };
            let v = make_stream(&mut rng, len);
            let d = make_stream(&mut rng, len);
            let p = make_stream(&mut rng, len);
            let s = make_stream(&mut rng, len);
            let cfg = StageConfig::default_for(44100);
            let (l, r) = stage(&v, &d, &p, &s, &cfg);
            assert!(!l.is_empty());
            assert!(!r.is_empty());
            for &x in l.iter().chain(r.iter()) {
                assert!(x.is_finite(),
                    "seed {} produced non-finite sample {}", seed, x);
            }
        }
    }

    #[test]
    fn fuzz_interleave_length_matches_min_input() {
        for seed in 0..50_u64 {
            let mut rng = prng(0xBEEF_DEAD ^ seed);
            let nl = ((rng() + 1.0) * 0.5 * 100.0) as usize;
            let nr = ((rng() + 1.0) * 0.5 * 100.0) as usize;
            let l: Vec<f32> = (0..nl).map(|_| rng()).collect();
            let r: Vec<f32> = (0..nr).map(|_| rng()).collect();
            let out = interleave(&l, &r);
            assert_eq!(out.len(), 2 * nl.min(nr),
                "seed {}: interleave({},{}) → {} samples",
                seed, nl, nr, out.len());
        }
    }
}
