// src/room_geometry.rs
#![allow(dead_code)]
//
// 3D acoustic geometry for listener-position-aware ANC.
//
// ── The conventional ANC model and its limit ──
//
// The canceller's existing alignment probe measures one round-trip
// number: "from anti-noise emission to mic capture, how many
// samples elapse?" That number characterizes ONE point in space —
// the microphone — and cancellation is optimized for THAT point.
//
// In practice the listener is rarely at the microphone. The phone
// might be on the nightstand while the user's head is 80 cm away
// on the pillow. At 343 m/s, 80 cm = 2.3 ms = 110 samples of
// timing offset. Cancellation that's perfect at the mic is
// arbitrarily wrong at the pillow.
//
// And in the multi-listener case (two people in the room, a baby
// crib), there's no single optimum — different positions need
// different anti-noise timings.
//
// ── What this module does ──
//
// It models the acoustic geometry as 3D points + propagation
// delays. Given:
//   - one or more SPEAKERS at known positions
//   - one or more LISTENER positions the user cares about
//   - the MIC position (where we observe what's actually happening)
//   - one or more NOISE SOURCES (window, vent, door)
//
// it computes the per-speaker delay assignments that best minimize
// residual at the listener positions, in a least-squares sense.
//
// For a single speaker + single listener: closed-form solution
// (just compensate the geometry offset).
//
// For multiple speakers / listeners: real least-squares with the
// number of variables = number of speakers and the number of
// constraints = number of (listener, frequency) pairs. Solved by
// gradient descent for any non-trivial setup; an iterative
// gradient is cheap because we only need a few hundred iterations
// to converge and the problem is small (≤8 speakers, ≤4 listeners,
// ~5 target frequencies).
//
// ── Note on "perceived position" ──
//
// The user's framing — "minimize the wave pocket... to some sort
// of perceived user report based on where the microphones are
// reported to be, and where maybe the center of the room is and
// where the window is" — captures the key insight: the user
// REPORTS where they want quiet. Not the mic, not the speakers,
// but a USER-SPECIFIED position (or set of positions).
//
// The "basic interface" they mentioned would be the QML pages
// for entering these positions. We model them here; UI comes in
// a separate pass.

use std::f32::consts::PI;

/// Speed of sound at 20°C, 50% humidity, sea level. Variation
/// across realistic indoor conditions is ~1%; for the precision
/// we care about (sample-resolution cancellation at 48 kHz means
/// 21 μs per sample = 7 mm of path), a fixed value is fine.
pub const SPEED_OF_SOUND_M_S: f32 = 343.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self { Self { x, y, z } }
    pub fn dist(self, other: Vec3) -> f32 {
        let dx = self.x - other.x;
        let dy = self.y - other.y;
        let dz = self.z - other.z;
        (dx * dx + dy * dy + dz * dz).sqrt()
    }
}

/// Propagation delay from point a to point b, in samples at
/// `sample_rate`. Negative? No — by physics distances are positive.
pub fn delay_samples(a: Vec3, b: Vec3, sample_rate: f32) -> f32 {
    a.dist(b) / SPEED_OF_SOUND_M_S * sample_rate
}

/// A speaker emits anti-noise with an additional `extra_delay_samples`
/// of processing latency (BT buffer, sink-write buffer, etc.). The
/// time from "anti-noise emission decision" to "wave arrives at
/// point p" is `extra_delay_samples + dist(speaker_pos, p)/c·sr`.
#[derive(Debug, Clone)]
pub struct Speaker {
    pub position: Vec3,
    /// Channel latency (samples). Typically the speaker_calibration
    /// store's best_estimate_round_trip / 2 (one-way).
    pub extra_delay_samples: f32,
    /// Label for UI ("Smokey", "BS Stage", "FP5 internal").
    pub label: String,
}

/// A noise source — typically a window or HVAC vent. Its position
/// determines when noise arrives at each listener; we use these
/// delays to figure out how to time the anti-noise.
#[derive(Debug, Clone)]
pub struct NoiseSource {
    pub position: Vec3,
    pub label: String,
}

/// A user-defined listener position. We try to minimize residual
/// here.
#[derive(Debug, Clone)]
pub struct Listener {
    pub position: Vec3,
    pub label: String,
    /// Relative weight if multiple listeners. 1.0 = standard. A
    /// baby crib might be weighted 3.0 to prioritize it.
    pub weight: f32,
}

#[derive(Debug, Clone, Default)]
pub struct RoomGeometry {
    pub mic: Option<Vec3>,
    pub speakers: Vec<Speaker>,
    pub listeners: Vec<Listener>,
    pub noise_sources: Vec<NoiseSource>,
}

impl RoomGeometry {
    pub fn new() -> Self { Self::default() }

    /// Returns the time-of-arrival (in samples) of a noise from
    /// `noise_source_idx` at every listener position.
    pub fn noise_arrival_times(&self, noise_source_idx: usize, sr: f32) -> Vec<f32> {
        let Some(src) = self.noise_sources.get(noise_source_idx) else {
            return Vec::new();
        };
        self.listeners.iter()
            .map(|l| delay_samples(src.position, l.position, sr))
            .collect()
    }

    /// Time-of-arrival of speaker `speaker_idx`'s output at every
    /// listener position, INCLUDING the speaker's channel latency.
    pub fn anti_arrival_times(&self, speaker_idx: usize, sr: f32) -> Vec<f32> {
        let Some(spk) = self.speakers.get(speaker_idx) else {
            return Vec::new();
        };
        self.listeners.iter()
            .map(|l| spk.extra_delay_samples
                + delay_samples(spk.position, l.position, sr))
            .collect()
    }

    /// For a single-speaker / single-listener setup: how many
    /// extra samples should the canceller delay anti-noise so that
    /// it arrives at the listener simultaneously with noise from
    /// the indicated source?
    ///
    /// Returns Some(samples_to_add) if positive (delay anti-noise),
    /// or Some(0.0) if anti is already arriving late (can't speed
    /// up). None if geometry is incomplete.
    pub fn single_listener_delay_correction(
        &self, noise_idx: usize, speaker_idx: usize, sr: f32,
    ) -> Option<f32> {
        let listener = self.listeners.first()?;
        let src = self.noise_sources.get(noise_idx)?;
        let spk = self.speakers.get(speaker_idx)?;
        let noise_dt = delay_samples(src.position, listener.position, sr);
        let anti_dt = spk.extra_delay_samples
            + delay_samples(spk.position, listener.position, sr);
        // We want anti to arrive at the SAME TIME as noise. If anti
        // is already EARLIER (smaller dt), we need to ADD delay.
        // If anti is later, we can't make it earlier — return 0
        // and accept the misalignment.
        Some((noise_dt - anti_dt).max(0.0))
    }

    /// Estimate per-speaker delay corrections that minimize squared
    /// residual at all listener positions, summed across all noise
    /// sources, at a single target frequency.
    ///
    /// Returns a Vec of `samples_to_add` per speaker. The total
    /// anti-noise emission time per speaker is then
    /// `speaker.extra_delay_samples + result[i]`.
    ///
    /// Math: at frequency f, the residual at listener L is
    ///
    ///   r(L) = noise(L) + Σ_speakers anti_k(L) · exp(-i·2π·f·anti_dt_k)
    ///
    /// where anti_dt_k = spk.extra_delay + |spk - L|/c·sr + δ_k
    /// and δ_k is the delay correction we're choosing.
    ///
    /// We minimize Σ_L w_L · |r(L)|² over (δ_1, ..., δ_K).
    ///
    /// For one speaker + one listener this collapses to the
    /// closed-form `single_listener_delay_correction`. For more,
    /// gradient descent on the squared error function.
    pub fn optimize_delays(
        &self,
        noise_idx: usize,
        target_freq_hz: f32,
        sr: f32,
        n_iters: usize,
    ) -> Vec<f32> {
        let n_speakers = self.speakers.len();
        if n_speakers == 0 || self.listeners.is_empty() { return Vec::new(); }
        let Some(src) = self.noise_sources.get(noise_idx) else {
            return vec![0.0; n_speakers];
        };
        // Precompute geometry — per-listener noise arrival, per-
        // (speaker, listener) base anti-noise arrival.
        let noise_dts: Vec<f32> = self.listeners.iter()
            .map(|l| delay_samples(src.position, l.position, sr))
            .collect();
        let base_anti_dts: Vec<Vec<f32>> = self.speakers.iter().map(|spk| {
            self.listeners.iter().map(|l| {
                spk.extra_delay_samples
                    + delay_samples(spk.position, l.position, sr)
            }).collect()
        }).collect();
        let weights: Vec<f32> = self.listeners.iter().map(|l| l.weight).collect();

        let omega = 2.0 * PI * target_freq_hz / sr;
        // For destructive interference we want each speaker's
        // contribution to be 180° out of phase with the noise at
        // every listener. Closed-form for single speaker: shift δ
        // so phase of anti equals phase of noise + π.
        //
        // For multiple speakers we use gradient descent. Initial
        // guess: each speaker matches the single-speaker optimum
        // for the closest listener.
        let mut deltas: Vec<f32> = self.speakers.iter().enumerate().map(|(i, _)| {
            // Find closest listener for this speaker.
            let (best_l, _) = base_anti_dts[i].iter().enumerate()
                .min_by(|a, b| a.1.partial_cmp(b.1).unwrap())
                .unwrap();
            // Want: noise_dt[best_l] = anti_arrival[i][best_l] (mod 2π/ω)
            (noise_dts[best_l] - base_anti_dts[i][best_l]).max(0.0)
        }).collect();

        // Gradient descent. Step size chosen empirically — too
        // large oscillates, too small fails to converge in n_iters.
        let mut step = 1.0;
        let mut prev_loss = f32::INFINITY;
        for _ in 0..n_iters {
            let loss = self.residual_loss(
                &deltas, &noise_dts, &base_anti_dts, &weights, omega);
            // Adaptive step: shrink when loss increases.
            if loss > prev_loss * 1.001 { step *= 0.7; }
            prev_loss = loss;
            // Numerical gradient (central difference).
            const H: f32 = 0.5;  // samples
            for i in 0..n_speakers {
                deltas[i] += H;
                let lp = self.residual_loss(
                    &deltas, &noise_dts, &base_anti_dts, &weights, omega);
                deltas[i] -= 2.0 * H;
                let lm = self.residual_loss(
                    &deltas, &noise_dts, &base_anti_dts, &weights, omega);
                deltas[i] += H;
                let grad = (lp - lm) / (2.0 * H);
                deltas[i] = (deltas[i] - step * grad).max(0.0);
            }
        }
        deltas
    }

    /// Squared residual (sum over listeners, weighted) for a
    /// configuration of speaker delays.
    fn residual_loss(
        &self,
        deltas: &[f32],
        noise_dts: &[f32],
        base_anti_dts: &[Vec<f32>],
        weights: &[f32],
        omega: f32,
    ) -> f32 {
        let mut total = 0.0;
        for (l_idx, &w) in weights.iter().enumerate() {
            // Phasor for the noise at this listener.
            let noise_phase = -omega * noise_dts[l_idx];
            let noise_re = noise_phase.cos();
            let noise_im = noise_phase.sin();
            // Sum each speaker's contribution.
            let mut anti_re = 0.0;
            let mut anti_im = 0.0;
            for (s_idx, &delta) in deltas.iter().enumerate() {
                let arrival = base_anti_dts[s_idx][l_idx] + delta;
                let phase = -omega * arrival;
                // Unit amplitude per speaker — we're optimizing
                // phase relationship, not amplitude scaling.
                // Sign-invert (cancellation polarity).
                anti_re -= phase.cos();
                anti_im -= phase.sin();
            }
            // Distance-attenuation would scale per-speaker amplitudes
            // by 1/r — but for delay-optimization at one freq the
            // amplitude is a fixed factor we don't tune, so we
            // leave it as 1 here.
            let total_re = noise_re + anti_re;
            let total_im = noise_im + anti_im;
            total += w * (total_re * total_re + total_im * total_im);
        }
        total
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sr() -> f32 { 48_000.0 }

    #[test]
    fn dist_zero_for_identical_points() {
        let p = Vec3::new(1.0, 2.0, 3.0);
        assert!(p.dist(p) < 1e-6);
    }

    #[test]
    fn dist_matches_pythagorean() {
        let a = Vec3::new(0.0, 0.0, 0.0);
        let b = Vec3::new(3.0, 4.0, 0.0);
        assert!((a.dist(b) - 5.0).abs() < 1e-5);
    }

    #[test]
    fn delay_samples_at_1m_is_about_140() {
        // 1 m / 343 m/s × 48000 samples/s = 139.94 samples.
        let a = Vec3::new(0.0, 0.0, 0.0);
        let b = Vec3::new(1.0, 0.0, 0.0);
        let d = delay_samples(a, b, sr());
        assert!((d - 140.0).abs() < 1.0,
            "1m delay should be ~140 samples, got {}", d);
    }

    #[test]
    fn single_listener_correction_handles_simple_geometry() {
        // Geometry:
        //   window at (-2, 0, 0) — noise source
        //   speaker at (1, 0, 0) — closer to listener than noise
        //   listener at (0, 0, 0) — center
        // Noise travels 2m to listener. Anti travels 1m to listener.
        // Anti is closer (arrives earlier) — we need to add delay
        // to compensate.
        let mut g = RoomGeometry::new();
        g.noise_sources.push(NoiseSource {
            position: Vec3::new(-2.0, 0.0, 0.0),
            label: "window".into(),
        });
        g.speakers.push(Speaker {
            position: Vec3::new(1.0, 0.0, 0.0),
            extra_delay_samples: 0.0,
            label: "spk".into(),
        });
        g.listeners.push(Listener {
            position: Vec3::new(0.0, 0.0, 0.0),
            label: "user".into(),
            weight: 1.0,
        });
        let delta = g.single_listener_delay_correction(0, 0, sr()).unwrap();
        // Noise: 2m / 343 × 48000 = 280 samples.
        // Anti:  1m / 343 × 48000 = 140 samples.
        // Correction: 280 - 140 = 140 samples.
        assert!((delta - 140.0).abs() < 1.0,
            "expected ~140 samples correction, got {}", delta);
    }

    #[test]
    fn single_listener_returns_zero_when_anti_already_late() {
        // Anti speaker is FARTHER than noise — already late.
        let mut g = RoomGeometry::new();
        g.noise_sources.push(NoiseSource {
            position: Vec3::new(1.0, 0.0, 0.0),  // close to listener
            label: "window".into(),
        });
        g.speakers.push(Speaker {
            position: Vec3::new(5.0, 0.0, 0.0),  // far from listener
            extra_delay_samples: 1000.0,  // BT buffer
            label: "spk".into(),
        });
        g.listeners.push(Listener {
            position: Vec3::new(0.0, 0.0, 0.0),
            label: "user".into(),
            weight: 1.0,
        });
        let delta = g.single_listener_delay_correction(0, 0, sr()).unwrap();
        assert_eq!(delta, 0.0,
            "should clamp to 0 when anti can't arrive on time");
    }

    #[test]
    fn optimize_delays_finds_single_speaker_optimum() {
        // Same geometry as above. Optimization should converge
        // close to the closed-form result.
        let mut g = RoomGeometry::new();
        g.noise_sources.push(NoiseSource {
            position: Vec3::new(-2.0, 0.0, 0.0), label: "w".into() });
        g.speakers.push(Speaker {
            position: Vec3::new(1.0, 0.0, 0.0),
            extra_delay_samples: 0.0, label: "spk".into() });
        g.listeners.push(Listener {
            position: Vec3::new(0.0, 0.0, 0.0),
            label: "user".into(), weight: 1.0 });
        let closed = g.single_listener_delay_correction(0, 0, sr()).unwrap();
        let optimized = g.optimize_delays(0, 200.0, sr(), 200);
        assert_eq!(optimized.len(), 1);
        // The closed-form value is the time-domain optimum; the
        // optimized value is the phase-frequency-domain optimum at
        // 200 Hz. They should agree modulo one period (2π/ω samples).
        // Period at 200 Hz, 48k sr: 48000/200 = 240 samples. So
        // optimized should be (closed mod 240) +/- N*240.
        let period = sr() / 200.0;
        let diff_mod = ((optimized[0] - closed).rem_euclid(period));
        // Within ~10% of period of either 0 or full period.
        let agreement = diff_mod.min(period - diff_mod);
        assert!(agreement < period * 0.15,
            "optimizer should approach closed-form within 15% of period; \
             closed={:.1}, optimized={:.1}, diff_mod={:.1}",
            closed, optimized[0], diff_mod);
    }

    #[test]
    fn optimize_delays_two_speaker_two_listener() {
        // Two speakers, two listeners. The optimizer should find
        // SOME delay configuration; we mostly check it doesn't
        // crash and produces finite outputs.
        let mut g = RoomGeometry::new();
        g.noise_sources.push(NoiseSource {
            position: Vec3::new(0.0, -3.0, 0.0), label: "window".into() });
        g.speakers.push(Speaker {
            position: Vec3::new(-1.0, 1.0, 0.0),
            extra_delay_samples: 100.0, label: "L".into() });
        g.speakers.push(Speaker {
            position: Vec3::new(1.0, 1.0, 0.0),
            extra_delay_samples: 100.0, label: "R".into() });
        g.listeners.push(Listener {
            position: Vec3::new(-0.5, 0.0, 0.0),
            label: "you".into(), weight: 1.0 });
        g.listeners.push(Listener {
            position: Vec3::new(0.5, 0.0, 0.0),
            label: "partner".into(), weight: 1.0 });
        let deltas = g.optimize_delays(0, 200.0, sr(), 300);
        assert_eq!(deltas.len(), 2);
        for d in &deltas {
            assert!(d.is_finite(), "deltas finite");
            assert!(*d >= 0.0, "deltas non-negative");
            // Shouldn't be wildly larger than typical room geometry
            // delays (a few hundred samples).
            assert!(*d < 2000.0, "delta {} unreasonable", d);
        }
    }

    #[test]
    fn optimize_returns_empty_for_empty_geometry() {
        let g = RoomGeometry::new();
        let r = g.optimize_delays(0, 200.0, sr(), 10);
        assert!(r.is_empty());
    }

    #[test]
    fn optimize_handles_missing_noise_source_gracefully() {
        let mut g = RoomGeometry::new();
        g.speakers.push(Speaker {
            position: Vec3::new(0.0, 0.0, 0.0),
            extra_delay_samples: 0.0, label: "s".into() });
        g.listeners.push(Listener {
            position: Vec3::new(1.0, 0.0, 0.0),
            label: "l".into(), weight: 1.0 });
        // No noise_sources — index 0 doesn't exist.
        let r = g.optimize_delays(0, 200.0, sr(), 10);
        // Should return zero-deltas rather than panic.
        assert_eq!(r, vec![0.0]);
    }
}
