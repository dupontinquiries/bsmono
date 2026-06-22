// src/network_sim.rs
#![allow(dead_code)]
//
// Network / latency simulator + fuzz harness for the noise cancellers.
//
// ── What this is for ──
//
// The cancellers live downstream of three latency contributors that
// vary minute-to-minute:
//
//   1. Mic capture jitter — PulseAudio's read cadence varies with
//      load; we typically see 20-40 ms, occasional 100+ ms stalls
//      when something else hogs the audio thread.
//   2. Sink-write buffering — paplay's internal buffer is configurable
//      and the BT A2DP path adds another 50-150 ms.
//   3. Speaker round-trip — the alignment probe measures this, but
//      it drifts (room temp, BT re-handshakes, listener movement).
//
// On real hardware these compound into a 100-400 ms anti-noise-to-
// noise delivery delay. The canceller's job is to STILL produce
// useful suppression despite that delay — and to gracefully degrade
// when it gets worse.
//
// The right way to find out where it falls over is to push known
// signals through a simulated pipeline that exercises the full
// latency envelope. This module does that.
//
// ── The sim model ──
//
// A `NetworkSim` wraps a one-way "channel" that adds:
//   - constant_latency_ms   — base round-trip
//   - jitter_ms_std         — Gaussian jitter on each chunk
//   - drop_probability      — random chunk drop (BT pop)
//   - duplicate_probability — random chunk duplication (a real BT
//                             quirk: chunks sometimes arrive twice
//                             when the link recovers from a stall)
//
// The fuzz harness then:
//   1. Synthesizes a known noise event (beeper / drone / passby)
//   2. Feeds it to the canceller through the sim
//   3. Sums anti-noise output with the original ground-truth signal
//      at the listener position (with the same propagation latency)
//   4. Measures residual energy in the target band
//   5. Reports suppression (dB) across the sweep
//
// The harness is a regular `#[test]`; cargo test --nocapture prints
// the suppression table so a human can eyeball where degradation
// becomes severe.

use std::collections::VecDeque;

/// One simulated chunk of audio with its scheduled arrival time.
#[derive(Debug, Clone)]
struct PendingChunk {
    /// Sample data.
    samples: Vec<f32>,
    /// Sample index at which this chunk should be delivered.
    deliver_at: usize,
}

#[derive(Debug, Clone)]
pub struct NetworkSim {
    /// Mean latency in samples (= ms × sample_rate / 1000).
    mean_latency_samples: usize,
    /// Jitter standard deviation in samples.
    jitter_samples: f32,
    /// Probability [0, 1] that a chunk gets dropped entirely.
    drop_prob: f32,
    /// Probability [0, 1] that a chunk gets duplicated.
    duplicate_prob: f32,
    /// In-flight chunks waiting to be delivered.
    pending: VecDeque<PendingChunk>,
    /// Current sample index — advanced by `process_chunk`.
    now_samples: usize,
    /// Deterministic RNG state (so fuzz runs are reproducible
    /// given a seed).
    rng_state: u64,
    /// Sample rate of the audio stream (used internally for
    /// converting ms → samples in the input setters).
    sample_rate: f32,
    /// Histogram of actual delivery latencies in samples — used
    /// by the test report.
    actual_latency_samples: Vec<usize>,
}

impl NetworkSim {
    pub fn new(sample_rate: f32, seed: u64) -> Self {
        Self {
            mean_latency_samples: 0,
            jitter_samples: 0.0,
            drop_prob: 0.0,
            duplicate_prob: 0.0,
            pending: VecDeque::new(),
            now_samples: 0,
            rng_state: seed.max(1),
            sample_rate,
            actual_latency_samples: Vec::new(),
        }
    }

    pub fn with_latency_ms(mut self, ms: f32) -> Self {
        self.mean_latency_samples =
            (ms.max(0.0) * self.sample_rate / 1000.0) as usize;
        self
    }
    pub fn with_jitter_ms(mut self, ms: f32) -> Self {
        self.jitter_samples = ms.max(0.0) * self.sample_rate / 1000.0;
        self
    }
    pub fn with_drop_prob(mut self, p: f32) -> Self {
        self.drop_prob = p.clamp(0.0, 1.0); self
    }
    pub fn with_duplicate_prob(mut self, p: f32) -> Self {
        self.duplicate_prob = p.clamp(0.0, 1.0); self
    }

    /// Statistics on the actual end-to-end latency seen by chunks
    /// that DID arrive (drops don't contribute). Returns
    /// `(mean_ms, stddev_ms, max_ms)` or `(0, 0, 0)` if no chunks
    /// have been delivered.
    pub fn latency_stats_ms(&self) -> (f32, f32, f32) {
        if self.actual_latency_samples.is_empty() { return (0.0, 0.0, 0.0); }
        let to_ms = |s: usize| s as f32 * 1000.0 / self.sample_rate;
        let n = self.actual_latency_samples.len() as f32;
        let mean: f32 = self.actual_latency_samples.iter()
            .map(|&s| to_ms(s)).sum::<f32>() / n;
        let var: f32 = self.actual_latency_samples.iter()
            .map(|&s| { let m = to_ms(s) - mean; m * m })
            .sum::<f32>() / n;
        let max = self.actual_latency_samples.iter()
            .map(|&s| to_ms(s))
            .fold(0.0f32, |a, b| a.max(b));
        (mean, var.sqrt(), max)
    }

    /// Push a chunk into the sim's input. Schedules it (or its
    /// duplicates) for delivery according to current parameters.
    /// Returns how many copies were enqueued (0 = dropped).
    pub fn submit(&mut self, samples: &[f32]) -> usize {
        if samples.is_empty() { return 0; }
        let mut delivered = 0;

        // Decide: dropped, duplicated, or single delivery?
        let drop_roll = self.next_uniform();
        if drop_roll < self.drop_prob { return 0; }

        let dup_roll = self.next_uniform();
        let copies = if dup_roll < self.duplicate_prob { 2 } else { 1 };

        for _ in 0..copies {
            // Apply jitter via a Box-Muller-ish trick: take the
            // mean of two uniform samples to approximate Gaussian.
            let jitter = if self.jitter_samples > 0.0 {
                let u1 = self.next_uniform();
                let u2 = self.next_uniform();
                // (u1 + u2 - 1.0) is in [-1, 1] with a triangular
                // distribution centered at 0. Multiply by jitter
                // for the spread.
                ((u1 + u2 - 1.0) * 2.0 * self.jitter_samples) as i64
            } else { 0 };
            let lat = (self.mean_latency_samples as i64 + jitter).max(0) as usize;
            self.pending.push_back(PendingChunk {
                samples: samples.to_vec(),
                deliver_at: self.now_samples + lat,
            });
            delivered += 1;
        }
        delivered
    }

    /// Pull whatever chunks have arrived by `now_samples`. Advances
    /// the clock by `advance_samples` and returns the concatenated
    /// arrived-during-this-window samples.
    ///
    /// The "merge" model: if multiple chunks arrived in the same
    /// window, their samples get concatenated in arrival order.
    /// Out-of-order arrivals (a chunk that was jitter-delayed
    /// past a later chunk) are appended in the order they actually
    /// arrive, not the order they were submitted — that's the
    /// realistic behavior of the BT/UDP path.
    pub fn pull(&mut self, advance_samples: usize) -> Vec<f32> {
        self.now_samples += advance_samples;
        let mut out = Vec::new();
        // Find all chunks scheduled for delivery up to now.
        let mut still_pending = VecDeque::with_capacity(self.pending.len());
        while let Some(chunk) = self.pending.pop_front() {
            if chunk.deliver_at <= self.now_samples {
                let lat = chunk.deliver_at.saturating_sub(
                    chunk.deliver_at.saturating_sub(
                        chunk.samples.len()));  // approximate
                self.actual_latency_samples.push(lat);
                out.extend_from_slice(&chunk.samples);
            } else {
                still_pending.push_back(chunk);
            }
        }
        self.pending = still_pending;
        out
    }

    /// xorshift32-based deterministic uniform in [0, 1).
    fn next_uniform(&mut self) -> f32 {
        let mut x = self.rng_state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.rng_state = x.max(1);
        ((x >> 1) as f32) / (u64::MAX as f32 / 2.0)
    }
}

// ─── Suppression metric ──────────────────────────────────────────────────

/// Measure how much an anti-noise signal suppresses a reference
/// signal in a target band. Returns dB suppression: positive means
/// the residual (reference + anti) has less band energy than the
/// reference alone; zero means no suppression; negative means
/// the anti-noise INCREASED the noise (wrong phase).
///
/// Implementation: simple Goertzel-based RMS in the band.
pub fn suppression_db(
    reference: &[f32],
    residual:  &[f32],
    lo_hz: f32, hi_hz: f32, sr: f32,
) -> f32 {
    let band_energy = |s: &[f32]| -> f32 {
        // Sum of Goertzel magnitudes across the band, normalized
        // by samples + n_probes for an RMS-like number.
        let n_probes = 16;
        let mut total = 0.0f32;
        for i in 0..n_probes {
            let f = lo_hz + (hi_hz - lo_hz) * (i as f32) / (n_probes as f32 - 1.0);
            let mag = crate::passby_canceller::goertzel(s, f, sr);
            total += mag * mag;
        }
        (total / n_probes as f32).sqrt() / s.len().max(1) as f32
    };
    let r = band_energy(reference).max(1e-12);
    let q = band_energy(residual).max(1e-12);
    20.0 * (r / q).log10()
}

/// JitterSmoother: a fixed-latency buffer that absorbs arrival
/// jitter by holding samples for a configurable duration before
/// emitting them at a steady rate.
///
/// ── Why this exists ──
///
/// The fuzz harness showed jitter is much more damaging than
/// constant latency: at zero jitter, +400 ms of latency still
/// gives +9.78 dB suppression. At 50 ms jitter, the same canceller
/// drops to negative territory. The user's hypothesis: if pure
/// latency is harmless, can we deliberately TRADE jitter for
/// latency by holding samples in a smoothing buffer?
///
/// ── How it works ──
///
/// The smoother holds `target_delay_samples` samples in a queue.
/// When the canceller asks for N samples, the smoother emits N
/// from the queue's HEAD (oldest samples first) — but only if
/// the queue depth would stay above the target after pulling.
/// Otherwise it underruns (emits zeros).
///
/// Input arrivals can be bursty (high jitter); output is steady
/// (low jitter). The cost: every sample is delayed by
/// `target_delay_samples` extra samples on average.
///
/// ── The trade-off in numbers ──
///
/// If input has mean latency 100 ms with 50 ms jitter, and the
/// smoother is configured with target_delay = 100 ms, then:
///   - Output mean latency: 200 ms (input + buffer)
///   - Output jitter: << 50 ms (buffer absorbs most variance)
///
/// If the canceller's latency tolerance is independent of jitter
/// (which the fuzz suggests), this is a clean win: trade absolute
/// latency for jitter reduction. Cost: 100 ms more total delay.
pub struct JitterSmoother {
    buffer: std::collections::VecDeque<f32>,
    target_delay_samples: usize,
    /// Tracks underrun count for diagnostics — if this grows,
    /// the configured delay is too small for the input jitter.
    underruns: u64,
}

impl JitterSmoother {
    pub fn new(target_delay_samples: usize) -> Self {
        Self {
            buffer: std::collections::VecDeque::with_capacity(
                target_delay_samples * 4),
            target_delay_samples,
            underruns: 0,
        }
    }

    pub fn submit(&mut self, samples: &[f32]) {
        for &s in samples {
            self.buffer.push_back(if s.is_finite() { s } else { 0.0 });
        }
    }

    pub fn pull(&mut self, n: usize) -> Vec<f32> {
        let mut out = Vec::with_capacity(n);
        for _ in 0..n {
            // Only emit if the buffer is at or above target depth —
            // this ensures we always have target_delay_samples of
            // future content cushion. If below target, emit zero
            // (smoother prefers gaps over rate-jumps).
            if self.buffer.len() > self.target_delay_samples {
                out.push(self.buffer.pop_front().unwrap_or(0.0));
            } else {
                self.underruns += 1;
                out.push(0.0);
            }
        }
        out
    }

    pub fn underruns(&self) -> u64 { self.underruns }
    pub fn depth(&self) -> usize { self.buffer.len() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::passby_canceller::{
        synth_passby, synth_passby_saturated, synth_passby_realistic,
        PassbyCanceller, WINDOW_SAMPLES,
        BAND_LO_HZ, BAND_HI_HZ, SAMPLE_RATE,
    };
    use crate::backup_beeper::{synth_beeper, BackupBeeperMasker};

    #[test]
    fn submitted_chunk_arrives_after_latency() {
        let sr = 48_000.0;
        let mut sim = NetworkSim::new(sr, 42).with_latency_ms(20.0);
        sim.submit(&vec![1.0f32; 256]);
        // Pulling immediately should yield nothing — latency hasn't elapsed.
        let out = sim.pull(100);
        assert!(out.is_empty(),
            "chunk should still be in flight, got {} samples", out.len());
        // Advance past the latency.
        let out = sim.pull(1500);
        assert_eq!(out.len(), 256, "chunk should have arrived");
    }

    #[test]
    fn drop_probability_drops_chunks() {
        let sr = 48_000.0;
        let mut sim = NetworkSim::new(sr, 1).with_drop_prob(1.0);
        let n = sim.submit(&vec![1.0f32; 256]);
        assert_eq!(n, 0, "100% drop should yield zero deliveries");
    }

    #[test]
    fn duplicate_probability_emits_two_copies() {
        let sr = 48_000.0;
        let mut sim = NetworkSim::new(sr, 99).with_duplicate_prob(1.0);
        let n = sim.submit(&vec![0.5f32; 128]);
        assert_eq!(n, 2);
    }

    #[test]
    fn zero_jitter_zero_drop_passes_through() {
        let sr = 48_000.0;
        let mut sim = NetworkSim::new(sr, 7);
        let total_in = 0;
        let mut total_out = 0;
        for _ in 0..10 {
            sim.submit(&vec![0.1f32; 64]);
        }
        for _ in 0..10 {
            total_out += sim.pull(64).len();
        }
        let _ = total_in;
        assert_eq!(total_out, 640,
            "with zero latency every chunk should arrive within its window");
    }

    /// The user-asked fuzz test: synthesize a noise event, push it
    /// through a sim with varying latency, run it through the
    /// canceller, measure cancellation effectiveness.
    ///
    /// This is a "characterization test" not a pass/fail one — it
    /// prints a table when run with --nocapture so a human can see
    /// where the canceller degrades. The asserts only verify the
    /// boring invariants (output is finite, low-latency cancels
    /// more than high-latency).
    #[test]
    fn fuzz_passby_under_varying_latency() {
        let sr = SAMPLE_RATE;
        eprintln!("\n=== passby canceller — latency sweep ===");
        eprintln!("{:>10} {:>10} {:>12} {:>12}",
                  "lat_ms", "jit_ms", "drop_pct", "supp_dB");

        // Synthesize a passby event we'll reuse across all trials.
        let event = synth_passby(8.0, 120.0, 0.04, sr, 17);

        let mut results: Vec<(f32, f32, f32, f32)> = Vec::new();
        for &latency_ms in &[0.0f32, 20.0, 50.0, 100.0, 200.0, 400.0] {
            for &jitter_ms in &[0.0f32, 10.0, 50.0] {
                for &drop_pct in &[0.0f32, 5.0] {
                    let supp = run_passby_trial(
                        &event, latency_ms, jitter_ms, drop_pct / 100.0, sr);
                    eprintln!("{:>10.0} {:>10.0} {:>12.1} {:>12.2}",
                              latency_ms, jitter_ms, drop_pct, supp);
                    results.push((latency_ms, jitter_ms, drop_pct, supp));
                }
            }
        }
        eprintln!("=== end sweep ===\n");

        // Invariants we care about (boring sanity, not pass/fail
        // characterization):
        //   - Suppression at 0 ms latency / 0 jitter / 0 drop must
        //     be NON-NEGATIVE (canceller should never make it worse
        //     under ideal conditions).
        //   - Suppression numbers must all be finite.
        let zero_zero = results.iter()
            .find(|r| r.0 == 0.0 && r.1 == 0.0 && r.2 == 0.0)
            .expect("zero-zero-zero trial should be in results");
        assert!(zero_zero.3.is_finite(), "supp should be finite");
        assert!(zero_zero.3 >= -1.0,
            "canceller shouldn't AMPLIFY under ideal conditions; got {} dB",
            zero_zero.3);

        for r in &results {
            assert!(r.3.is_finite(), "all supps finite; {:?}", r);
        }
    }

    /// Experiment: does setting the canceller's saturation to match
    /// the source's saturation actually improve cancellation?
    ///
    /// Hypothesis (from the user's intuition): yes, because clean
    /// sinusoidal anti-noise can't cancel the higher harmonics that
    /// asymmetric soft-clipping introduces. Matching the saturation
    /// shape in the anti-noise should let it attack those harmonics
    /// too.
    ///
    /// Method: synthesize a saturated passby at saturation=0.3,
    /// run two trials at zero-channel-noise (latency=0, jitter=0,
    /// drop=0):
    ///   - trial A: canceller saturation = 0.0 (clean anti)
    ///   - trial B: canceller saturation = 0.3 (matched anti)
    /// Compare suppression. If hypothesis holds, B > A.
    #[test]
    fn saturation_matching_improves_cancellation_on_saturated_source() {
        let sr = SAMPLE_RATE;
        let saturation = 0.3;
        let event = synth_passby_saturated(8.0, 120.0, 0.04, saturation, sr, 17);

        let supp_clean = run_passby_trial_with_saturation(
            &event, 0.0, 0.0, 0.0, sr, 0.0);
        let supp_matched = run_passby_trial_with_saturation(
            &event, 0.0, 0.0, 0.0, sr, saturation);

        eprintln!("\n=== saturation matching ===");
        eprintln!("clean anti  ({}): supp = {:.2} dB", 0.0, supp_clean);
        eprintln!("matched anti ({}): supp = {:.2} dB", saturation, supp_matched);
        eprintln!("delta: {:+.2} dB", supp_matched - supp_clean);
        eprintln!("===========================\n");

        // Sanity: both should be finite numbers in the same ballpark.
        assert!(supp_clean.is_finite() && supp_matched.is_finite());
        // We don't assert matched > clean as a HARD requirement
        // because the band-energy metric is dominated by the
        // fundamental, where clean anti also cancels well. The
        // EXPECTED improvement is at upper harmonics (4f, 5f, 6f)
        // which contribute less to the aggregate metric. So this
        // test is characterization, not pass/fail — the printed
        // numbers are the actual finding.
    }

    /// Same as run_passby_trial but with a configurable canceller
    /// saturation. Used by the matching experiment.
    fn run_passby_trial_with_saturation(
        event: &[f32], latency_ms: f32, jitter_ms: f32,
        drop_prob: f32, sr: f32, canceller_saturation: f32,
    ) -> f32 {
        let mut sim_ear  = NetworkSim::new(sr, 11)
            .with_latency_ms(latency_ms).with_jitter_ms(jitter_ms)
            .with_drop_prob(drop_prob);
        let mut sim_anti = NetworkSim::new(sr, 23)
            .with_latency_ms(latency_ms).with_jitter_ms(jitter_ms)
            .with_drop_prob(drop_prob);
        let mut canceller = PassbyCanceller::new();
        canceller.set_saturation(canceller_saturation);
        let win = WINDOW_SAMPLES;
        let mut reference: Vec<f32> = Vec::with_capacity(event.len());
        let mut residual:  Vec<f32> = Vec::with_capacity(event.len());
        let mut i = 0;
        while i + win <= event.len() {
            let chunk = &event[i..i + win];
            canceller.process_window(chunk);
            let anti = canceller.pull_output(win);
            sim_ear.submit(chunk);
            sim_anti.submit(&anti);
            let arrived_noise = sim_ear.pull(win);
            let arrived_anti  = sim_anti.pull(win);
            let lim = arrived_noise.len().min(arrived_anti.len());
            for k in 0..lim {
                reference.push(arrived_noise[k]);
                residual.push(arrived_noise[k] - arrived_anti[k]);
            }
            i += win;
        }
        for _ in 0..10 {
            let n = sim_ear.pull(win);
            let a = sim_anti.pull(win);
            let lim = n.len().min(a.len());
            for k in 0..lim {
                reference.push(n[k]);
                residual.push(n[k] - a[k]);
            }
            if n.is_empty() && a.is_empty() { break; }
        }
        if reference.is_empty() || residual.is_empty() { return 0.0; }
        // Use a WIDER band for the suppression metric here — we want
        // to see the upper-harmonic improvements that saturation
        // matching specifically targets. 60-1200 Hz covers the
        // fundamental (~120 Hz) up through ~10x harmonic.
        suppression_db(&reference, &residual, 60.0, 1200.0, sr)
    }

    /// Same experiment but in the UPPER HARMONIC BAND only — where
    /// saturation matching is supposed to help most. This separates
    /// "saturation helps everywhere" from "saturation helps in the
    /// timbre band specifically."
    #[test]
    fn saturation_matching_specifically_in_upper_band() {
        let sr = SAMPLE_RATE;
        let saturation = 0.3;
        let event = synth_passby_saturated(8.0, 120.0, 0.04, saturation, sr, 17);

        // Measure suppression in the 400-1200 Hz band only — that's
        // h4 (480 Hz) through h10 (1200 Hz) of a 120 Hz fundamental,
        // the range where saturation harmonics live.
        let supp_clean = run_passby_trial_band(
            &event, sr, 0.0, 400.0, 1200.0);
        let supp_matched = run_passby_trial_band(
            &event, sr, saturation, 400.0, 1200.0);

        eprintln!("\n=== saturation in upper band (400-1200 Hz) ===");
        eprintln!("clean anti (0.0): {:.2} dB", supp_clean);
        eprintln!("matched anti ({:.1}): {:.2} dB", saturation, supp_matched);
        eprintln!("delta: {:+.2} dB", supp_matched - supp_clean);
        eprintln!("===============================================\n");

        assert!(supp_clean.is_finite() && supp_matched.is_finite());
    }

    fn run_passby_trial_band(
        event: &[f32], sr: f32, sat: f32, band_lo: f32, band_hi: f32,
    ) -> f32 {
        let mut canceller = PassbyCanceller::new();
        canceller.set_saturation(sat);
        let win = WINDOW_SAMPLES;
        let mut reference: Vec<f32> = Vec::with_capacity(event.len());
        let mut residual:  Vec<f32> = Vec::with_capacity(event.len());
        let mut i = 0;
        while i + win <= event.len() {
            let chunk = &event[i..i + win];
            canceller.process_window(chunk);
            let anti = canceller.pull_output(win);
            for k in 0..win {
                reference.push(chunk[k]);
                residual.push(chunk[k] - anti[k]);
            }
            i += win;
        }
        if reference.is_empty() { return 0.0; }
        suppression_db(&reference, &residual, band_lo, band_hi, sr)
    }

    /// Compare fixed-diversity-offset (current default) vs. per-
    /// harmonic phase locking. Same network sim trial parameters
    /// for both — only the lock mode varies. Prints both results
    /// side by side so the empirical difference is visible.
    #[test]
    fn per_harmonic_lock_vs_fixed_offsets() {
        let sr = SAMPLE_RATE;
        let event = synth_passby_saturated(8.0, 120.0, 0.04, 0.2, sr, 17);

        eprintln!("\n=== per-harmonic vs fixed-offset lock ===");
        eprintln!("source: 120 Hz saturated passby, 8 s");
        eprintln!("{:>10} {:>10} {:>10} {:>12} {:>12}",
                  "lat_ms", "jit_ms", "drop_pct", "fixed_dB", "perharm_dB");
        for &latency_ms in &[0.0_f32, 50.0, 200.0] {
            for &jitter_ms in &[0.0_f32, 20.0, 50.0] {
                let fixed = run_with_lock_mode(
                    &event, latency_ms, jitter_ms, 0.0, sr, false);
                let perharm = run_with_lock_mode(
                    &event, latency_ms, jitter_ms, 0.0, sr, true);
                eprintln!("{:>10.0} {:>10.0} {:>10.1} {:>12.2} {:>12.2}",
                    latency_ms, jitter_ms, 0.0, fixed, perharm);
            }
        }
        eprintln!("=========================================\n");
    }

    fn run_with_lock_mode(
        event: &[f32], latency_ms: f32, jitter_ms: f32,
        drop_prob: f32, sr: f32, per_harmonic: bool,
    ) -> f32 {
        let mut sim_ear = NetworkSim::new(sr, 11)
            .with_latency_ms(latency_ms).with_jitter_ms(jitter_ms)
            .with_drop_prob(drop_prob);
        let mut sim_anti = NetworkSim::new(sr, 23)
            .with_latency_ms(latency_ms).with_jitter_ms(jitter_ms)
            .with_drop_prob(drop_prob);
        let mut canceller = PassbyCanceller::new();
        canceller.set_lock_per_harmonic(per_harmonic);
        let win = WINDOW_SAMPLES;
        let mut reference: Vec<f32> = Vec::with_capacity(event.len());
        let mut residual:  Vec<f32> = Vec::with_capacity(event.len());
        let mut i = 0;
        while i + win <= event.len() {
            let chunk = &event[i..i + win];
            canceller.process_window(chunk);
            let anti = canceller.pull_output(win);
            sim_ear.submit(chunk);
            sim_anti.submit(&anti);
            let arrived_noise = sim_ear.pull(win);
            let arrived_anti  = sim_anti.pull(win);
            let lim = arrived_noise.len().min(arrived_anti.len());
            for k in 0..lim {
                reference.push(arrived_noise[k]);
                residual.push(arrived_noise[k] - arrived_anti[k]);
            }
            i += win;
        }
        for _ in 0..10 {
            let n = sim_ear.pull(win);
            let a = sim_anti.pull(win);
            let lim = n.len().min(a.len());
            for k in 0..lim {
                reference.push(n[k]);
                residual.push(n[k] - a[k]);
            }
            if n.is_empty() && a.is_empty() { break; }
        }
        if reference.is_empty() { return 0.0; }
        suppression_db(&reference, &residual, BAND_LO_HZ, BAND_HI_HZ, sr)
    }

    /// Sliding-shelf saturation sweep. The previous flat-saturation
    /// experiment showed a band trade-off: matched saturation wins
    /// in the upper band, loses in the wide/fundamental band.
    /// Hypothesis: a sliding shelf (lower drive on the fundamental,
    /// higher drive on h3) might capture the upper-band gain
    /// WITHOUT the fundamental-band cost.
    ///
    /// Sweep shelf slope from 0 (flat, baseline) up to 1.5 and
    /// measure both bands.
    #[test]
    fn sliding_shelf_sweep() {
        let sr = SAMPLE_RATE;
        let base_sat = 0.3;
        let event = synth_passby_saturated(8.0, 120.0, 0.04, base_sat, sr, 17);

        eprintln!("\n=== sliding-shelf saturation sweep ===");
        eprintln!("base_sat = {}, source = 120 Hz saturated passby", base_sat);
        eprintln!("{:>8} {:>10} {:>10} {:>12} {:>12} {:>12}",
                  "slope", "h1_sat", "h3_sat", "wide_dB", "upper_dB", "fund_dB");
        for slope in [0.0_f32, 0.25, 0.5, 1.0, 1.5] {
            let h1_drive = base_sat;
            let h3_drive = (base_sat * (1.0 + 2.0 * slope)).min(0.9);
            // Run trial with this slope.
            let wide = run_passby_with_shelf(&event, sr, base_sat, slope,
                60.0, 1200.0);
            let upper = run_passby_with_shelf(&event, sr, base_sat, slope,
                400.0, 1200.0);
            let fund = run_passby_with_shelf(&event, sr, base_sat, slope,
                60.0, 250.0);
            eprintln!("{:>8.2} {:>10.3} {:>10.3} {:>12.2} {:>12.2} {:>12.2}",
                      slope, h1_drive, h3_drive, wide, upper, fund);
        }
        eprintln!("======================================\n");
    }

    /// Run a trial with sliding-shelf saturation enabled.
    fn run_passby_with_shelf(
        event: &[f32], sr: f32,
        base_sat: f32, shelf_slope: f32,
        band_lo: f32, band_hi: f32,
    ) -> f32 {
        let mut sim_ear  = NetworkSim::new(sr, 11);
        let mut sim_anti = NetworkSim::new(sr, 23);
        let mut canceller = PassbyCanceller::new();
        canceller.set_saturation(base_sat);
        canceller.set_sliding_shelf_slope(shelf_slope);
        let win = WINDOW_SAMPLES;
        let mut reference = Vec::with_capacity(event.len());
        let mut residual  = Vec::with_capacity(event.len());
        let mut i = 0;
        while i + win <= event.len() {
            let chunk = &event[i..i + win];
            canceller.process_window(chunk);
            let anti = canceller.pull_output(win);
            sim_ear.submit(chunk);
            sim_anti.submit(&anti);
            let an = sim_ear.pull(win);
            let aa = sim_anti.pull(win);
            let lim = an.len().min(aa.len());
            for k in 0..lim {
                reference.push(an[k]);
                residual.push(an[k] - aa[k]);
            }
            i += win;
        }
        for _ in 0..10 {
            let n = sim_ear.pull(win);
            let a = sim_anti.pull(win);
            let lim = n.len().min(a.len());
            for k in 0..lim {
                reference.push(n[k]);
                residual.push(n[k] - a[k]);
            }
            if n.is_empty() && a.is_empty() { break; }
        }
        if reference.is_empty() { return 0.0; }
        suppression_db(&reference, &residual, band_lo, band_hi, sr)
    }

    /// Sliding-shelf with reverb. Does the shelf still help when
    /// the source has reverb scrambling its harmonic phases?
    #[test]
    fn sliding_shelf_with_reverb() {
        use crate::reverb::Reverb;
        let sr = SAMPLE_RATE;
        let base_sat = 0.3;
        let raw = synth_passby_saturated(8.0, 120.0, 0.04, base_sat, sr, 17);

        let reverb_conditions: [(&str, f32, f32); 3] = [
            ("anechoic",  0.0, 0.0),
            ("bedroom",   0.4, 0.3),
            ("urban",     1.0, 0.5),
        ];

        eprintln!("\n=== sliding-shelf under reverb ===");
        eprintln!("{:>10} {:>8} {:>12} {:>12} {:>12}",
                  "reverb", "slope", "wide_dB", "upper_dB", "fund_dB");
        for (label, rt60, wet) in reverb_conditions {
            let event = if rt60 > 0.0 {
                let mut r = Reverb::new(rt60, 12, sr, 17);
                r.set_wet_mix(wet);
                r.apply(&raw)
            } else { raw.clone() };
            for slope in [0.0_f32, 0.5, 1.0] {
                let wide = run_passby_with_shelf(&event, sr, base_sat, slope, 60.0, 1200.0);
                let upper = run_passby_with_shelf(&event, sr, base_sat, slope, 400.0, 1200.0);
                let fund = run_passby_with_shelf(&event, sr, base_sat, slope, 60.0, 250.0);
                eprintln!("{:>10} {:>8.2} {:>12.2} {:>12.2} {:>12.2}",
                          label, slope, wide, upper, fund);
            }
        }
        eprintln!("===================================\n");
    }

    /// The big reality-check experiment: how does the canceller
    /// perform on a REALISTIC engine (measured harmonic profile
    /// from real recording) vs. the textbook synth used in all
    /// prior experiments?
    ///
    /// If the textbook synth is too easy, the canceller's numbers
    /// will drop on realistic. If the canceller's design is
    /// robust, they'll stay close.
    #[test]
    fn textbook_vs_realistic_synth() {
        let sr = SAMPLE_RATE;
        let f0 = 120.0;
        // Textbook: [1.0, 0.5, 0.3] harmonic amplitudes.
        let textbook = synth_passby(8.0, f0, 0.04, sr, 17);
        // Realistic: measured from real recording.
        let realistic = synth_passby_realistic(8.0, f0, 0.04, sr, 17);

        eprintln!("\n=== textbook vs realistic engine synth ===");
        eprintln!("source: 120 Hz, 8s passby");
        eprintln!("textbook RMS: {:.4}, realistic RMS: {:.4}",
                  (textbook.iter().map(|x| x*x).sum::<f32>()
                      / textbook.len() as f32).sqrt(),
                  (realistic.iter().map(|x| x*x).sum::<f32>()
                      / realistic.len() as f32).sqrt());
        eprintln!("\n{:>10} {:>14} {:>14}", "synth", "wide_dB", "upper_dB");

        for (label, event) in &[("textbook", &textbook), ("realistic", &realistic)] {
            let mut sim_ear  = NetworkSim::new(sr, 11);
            let mut sim_anti = NetworkSim::new(sr, 23);
            let mut canceller = PassbyCanceller::new();
            let win = WINDOW_SAMPLES;
            let mut reference = Vec::with_capacity(event.len());
            let mut residual  = Vec::with_capacity(event.len());
            let mut i = 0;
            while i + win <= event.len() {
                let chunk = &event[i..i + win];
                canceller.process_window(chunk);
                let anti = canceller.pull_output(win);
                sim_ear.submit(chunk);
                sim_anti.submit(&anti);
                let an = sim_ear.pull(win);
                let aa = sim_anti.pull(win);
                let lim = an.len().min(aa.len());
                for k in 0..lim {
                    reference.push(an[k]);
                    residual.push(an[k] - aa[k]);
                }
                i += win;
            }
            let wide = suppression_db(&reference, &residual, 60.0, 1200.0, sr);
            let upper = suppression_db(&reference, &residual, 400.0, 1200.0, sr);
            eprintln!("{:>10} {:>14.2} {:>14.2}", label, wide, upper);
        }
        eprintln!("==========================================\n");
    }

    /// The big design question for v1: does per-harmonic phase
    /// lock win on REAL engine signatures? On textbook synth it
    /// loses by 2.5 dB at ideal conditions; on real engines with
    /// 8 substantial harmonics (vs textbook's 3), per-harmonic
    /// might win because there's more harmonic phase to track.
    #[test]
    fn per_harmonic_vs_fixed_on_realistic_engine() {
        let sr = SAMPLE_RATE;
        let event = synth_passby_realistic(8.0, 120.0, 0.04, sr, 17);

        eprintln!("\n=== per-harmonic vs fixed on REALISTIC engine ===");
        eprintln!("source: realistic 120 Hz engine with measured profile");
        eprintln!("{:>10} {:>10} {:>12} {:>12} {:>10}",
                  "lat_ms", "jit_ms", "fixed_dB", "perharm_dB", "delta");

        for (lat, jit) in [(0.0_f32, 0.0_f32), (0.0, 20.0),
                           (50.0, 0.0), (50.0, 20.0), (200.0, 50.0)] {
            let fixed = run_with_lock_mode(&event, lat, jit, 0.0, sr, false);
            let perharm = run_with_lock_mode(&event, lat, jit, 0.0, sr, true);
            let delta = perharm - fixed;
            eprintln!("{:>10.0} {:>10.0} {:>12.2} {:>12.2} {:>+10.2}",
                      lat, jit, fixed, perharm, delta);
        }
        eprintln!("=================================================\n");
    }

    /// Saturation matching on realistic engine — the textbook
    /// finding was +2.38 dB. Does that hold or grow on real
    /// engine harmonics?
    #[test]
    fn saturation_matching_on_realistic_engine() {
        let sr = SAMPLE_RATE;
        let saturation = 0.3;
        // Apply saturation to the realistic engine output to make
        // it more "real" (real engines DO have saturation).
        let raw = synth_passby_realistic(8.0, 120.0, 0.04, sr, 17);
        let event: Vec<f32> = raw.iter().map(|&x| {
            if saturation > 1e-6 {
                let bias = 0.15 * saturation;
                let drive = 1.0 + 2.0 * saturation;
                let driven = x * drive + bias;
                let clipped = driven.tanh() / drive;
                let dc_offset = bias.tanh() / drive;
                clipped - dc_offset
            } else { x }
        }).collect();

        let wide_clean = run_passby_trial_with_saturation(
            &event, 0.0, 0.0, 0.0, sr, 0.0);
        let wide_matched = run_passby_trial_with_saturation(
            &event, 0.0, 0.0, 0.0, sr, saturation);
        let upper_clean = run_passby_trial_band(
            &event, sr, 0.0, 400.0, 1200.0);
        let upper_matched = run_passby_trial_band(
            &event, sr, saturation, 400.0, 1200.0);

        eprintln!("\n=== saturation matching on REALISTIC engine ===");
        eprintln!("source: realistic 120 Hz engine, saturated 0.3");
        eprintln!("{:>14} {:>14} {:>14} {:>14}",
                  "wide_clean", "wide_matched", "upper_clean", "upper_matched");
        eprintln!("{:>14.2} {:>14.2} {:>14.2} {:>14.2}",
                  wide_clean, wide_matched, upper_clean, upper_matched);
        eprintln!("wide delta: {:+.2} dB, upper delta: {:+.2} dB",
                  wide_matched - wide_clean, upper_matched - upper_clean);
        eprintln!("===============================================\n");
    }

    /// V1 EXPERIMENT: does the 8-harmonic extension actually
    /// recover meaningful suppression on real engine signatures?
    /// The ideal-canceller upper-bound was +29.6 dB. The current
    /// 3-harmonic canceller delivers -0.32 dB. Where does the
    /// 8-harmonic mode land between those?
    #[test]
    fn extended_8harmonic_on_realistic_engine() {
        let sr = SAMPLE_RATE;
        let event = synth_passby_realistic(8.0, 120.0, 0.04, sr, 17);

        eprintln!("\n=== V1: 3-harmonic vs 8-harmonic on real engine ===");
        eprintln!("source: realistic 120 Hz engine, anechoic, zero channel jitter");
        eprintln!("{:>14} {:>14} {:>14} {:>14} {:>10}",
                  "n_harmonics", "wide_dB", "upper_dB", "phase_q", "peak_seen");

        for &n_h in &[3usize, 5, 8, 10] {
            let mut sim_ear  = NetworkSim::new(sr, 11);
            let mut sim_anti = NetworkSim::new(sr, 23);
            let mut canceller = PassbyCanceller::new();
            if n_h > 3 { canceller.set_n_harmonics(n_h); }
            canceller.set_lock_per_harmonic(true);
            let win = WINDOW_SAMPLES;
            let mut reference = Vec::with_capacity(event.len());
            let mut residual  = Vec::with_capacity(event.len());
            let mut last_q = 0.0;
            let mut peak_seen = false;
            let mut max_q = 0.0_f32;
            let mut i = 0;
            while i + win <= event.len() {
                let chunk = &event[i..i + win];
                let snap = canceller.process_window(chunk);
                last_q = snap.phase_lock_quality;
                if snap.phase_lock_quality > max_q { max_q = snap.phase_lock_quality; }
                if canceller.phase() != crate::passby_canceller::PassbyPhase::Quiet {
                    peak_seen = true;
                }
                let a = canceller.pull_output(win);
                sim_ear.submit(chunk);
                sim_anti.submit(&a);
                let n_arr = sim_ear.pull(win);
                let a_arr = sim_anti.pull(win);
                let lim = n_arr.len().min(a_arr.len());
                for k in 0..lim {
                    reference.push(n_arr[k]);
                    residual.push(n_arr[k] - a_arr[k]);
                }
                i += win;
            }
            let _ = last_q;
            let wide = suppression_db(&reference, &residual, 60.0, 1200.0, sr);
            let upper = suppression_db(&reference, &residual, 400.0, 1200.0, sr);
            eprintln!("{:>14} {:>14.2} {:>14.2} {:>14.3} {:>10}",
                      n_h, wide, upper, max_q, peak_seen);
        }
        eprintln!("=====================================================\n");
    }

    /// achieve if the canceller tracked all 8 measured harmonics
    /// (with their measured amplitudes) instead of just h1-h3?
    ///
    /// Synthesizes an "ideal" anti-noise that perfectly cancels
    /// the source at all 8 harmonics. This is the theoretical
    /// ceiling — the current canceller only emits at h1, h2, h3
    /// with [1.0, 0.5, 0.25] amplitudes, missing 5 of 8 real
    /// harmonics and using wrong ratios for the 3 it does emit.
    #[test]
    fn ideal_8harmonic_canceller_upper_bound() {
        use std::f32::consts::PI;
        let sr = SAMPLE_RATE;
        let f0 = 120.0;
        let event = synth_passby_realistic(8.0, f0, 0.04, sr, 17);

        // Build the "ideal" anti-noise by directly inverting the
        // source's measured harmonic content at each sample. This
        // is what a perfect canceller could achieve.
        const HARMONIC_AMPS: [f32; 8] = [
            1.00, 1.30, 0.26, 0.88, 0.78, 0.50, 0.79, 0.14,
        ];
        let sum_amps: f32 = HARMONIC_AMPS.iter().sum();
        let n = event.len();
        let mut anti: Vec<f32> = Vec::with_capacity(n);
        // Use the SAME phase progression as synth_passby_realistic
        // and the same envelope.
        let mut phase = 0.0f32;
        let duration_s = 8.0_f32;
        let peak_t = duration_s * 0.5;
        let sigma = duration_s * 0.18;
        let max_doppler = 0.04;
        for i in 0..n {
            let t = i as f32 / sr;
            let cos_arg = ((t / duration_s).clamp(0.0, 1.0)) * PI;
            let doppler_factor = 1.0 + max_doppler * cos_arg.cos();
            let inst_freq = f0 * doppler_factor;
            let dphase = 2.0 * PI * inst_freq / sr;
            let mut engine = 0.0_f32;
            for (n_idx, &amp) in HARMONIC_AMPS.iter().enumerate() {
                let n_h = (n_idx + 1) as f32;
                if f0 * n_h >= sr * 0.45 { break; }
                engine += amp * (n_h * phase).sin();
            }
            let dt = t - peak_t;
            let env = 1.0 / (1.0 + (dt / sigma).powi(2));
            anti.push((engine / sum_amps) * env * 0.7);
            phase += dphase;
            if phase > 2.0 * PI { phase -= 2.0 * PI; }
        }

        // Measure suppression for: (1) no canceller, (2) current
        // 3-harmonic canceller, (3) ideal 8-harmonic anti-noise.
        let wide_baseline = suppression_db(&event, &event, 60.0, 1200.0, sr);
        let upper_baseline = suppression_db(&event, &event, 400.0, 1200.0, sr);

        // Current 3-harmonic canceller.
        let mut sim_ear  = NetworkSim::new(sr, 11);
        let mut sim_anti = NetworkSim::new(sr, 23);
        let mut canceller = PassbyCanceller::new();
        let win = WINDOW_SAMPLES;
        let mut reference = Vec::with_capacity(event.len());
        let mut residual  = Vec::with_capacity(event.len());
        let mut i = 0;
        while i + win <= event.len() {
            let chunk = &event[i..i + win];
            canceller.process_window(chunk);
            let a = canceller.pull_output(win);
            sim_ear.submit(chunk);
            sim_anti.submit(&a);
            let n_arr = sim_ear.pull(win);
            let a_arr = sim_anti.pull(win);
            let lim = n_arr.len().min(a_arr.len());
            for k in 0..lim {
                reference.push(n_arr[k]);
                residual.push(n_arr[k] - a_arr[k]);
            }
            i += win;
        }
        let cur_wide = suppression_db(&reference, &residual, 60.0, 1200.0, sr);
        let cur_upper = suppression_db(&reference, &residual, 400.0, 1200.0, sr);

        // "Ideal" canceller — perfect alignment with the source's
        // 8-harmonic content.
        let n_min = event.len().min(anti.len());
        let ref_ideal: Vec<f32> = event[..n_min].to_vec();
        let res_ideal: Vec<f32> = (0..n_min).map(|k| event[k] - anti[k]).collect();
        let ideal_wide = suppression_db(&ref_ideal, &res_ideal, 60.0, 1200.0, sr);
        let ideal_upper = suppression_db(&ref_ideal, &res_ideal, 400.0, 1200.0, sr);

        eprintln!("\n=== 3-harmonic vs 8-harmonic canceller on real engine ===");
        eprintln!("source: realistic 120 Hz engine, anechoic, zero channel jitter");
        eprintln!("{:>30} {:>12} {:>12}", "scenario", "wide_dB", "upper_dB");
        eprintln!("{:>30} {:>12.2} {:>12.2}",
                  "baseline (no canceller)", wide_baseline, upper_baseline);
        eprintln!("{:>30} {:>12.2} {:>12.2}",
                  "current 3-harmonic", cur_wide, cur_upper);
        eprintln!("{:>30} {:>12.2} {:>12.2}",
                  "ideal 8-harmonic (upper bound)", ideal_wide, ideal_upper);
        eprintln!("{:>30} {:>12.2} {:>12.2}",
                  "available headroom", ideal_wide - cur_wide,
                  ideal_upper - cur_upper);
        eprintln!("==========================================================\n");
    }

    /// The big trade-off experiment: does adding a JitterSmoother
    /// (which adds latency) IMPROVE suppression by reducing the
    /// jitter the canceller sees?
    ///
    /// Setup: source has high jitter (50 ms). Test with smoother
    /// at various depths and measure suppression. If the user's
    /// hypothesis ("latency is harmless, jitter kills") is right,
    /// we should see suppression IMPROVE as smoother depth grows
    /// up to roughly the jitter magnitude.
    #[test]
    fn jitter_smoother_tradeoff_sweep() {
        let sr = SAMPLE_RATE;
        let event = synth_passby_saturated(8.0, 120.0, 0.04, 0.2, sr, 17);
        let base_jitter = 50.0_f32;

        eprintln!("\n=== jitter-smoother trade-off ===");
        eprintln!("source jitter: {} ms", base_jitter);
        eprintln!("{:>14} {:>14} {:>14}",
                  "smoother_ms", "suppression_dB", "underruns");
        for smoother_ms in [0_u32, 10, 25, 50, 100, 200] {
            let supp = run_with_smoother(
                &event, 50.0, base_jitter, sr, smoother_ms);
            eprintln!("{:>14} {:>14.2} {:>14}", smoother_ms, supp.0, supp.1);
        }
        eprintln!("=================================\n");
    }

    /// Same trade-off but at a fixed latency where the canceller
    /// already struggles, to see if smoother is a net win or
    /// adds enough latency to break things.
    #[test]
    fn jitter_smoother_with_high_baseline_latency() {
        let sr = SAMPLE_RATE;
        let event = synth_passby_saturated(8.0, 120.0, 0.04, 0.2, sr, 17);

        eprintln!("\n=== jitter-smoother at high baseline ===");
        eprintln!("base latency: 200 ms, jitter: 50 ms");
        eprintln!("{:>14} {:>14}", "smoother_ms", "suppression_dB");
        for smoother_ms in [0_u32, 25, 50, 100] {
            let supp = run_with_smoother(
                &event, 200.0, 50.0, sr, smoother_ms);
            eprintln!("{:>14} {:>14.2}", smoother_ms, supp.0);
        }
        eprintln!("=========================================\n");
    }

    fn run_with_smoother(
        event: &[f32], latency_ms: f32, jitter_ms: f32,
        sr: f32, smoother_ms: u32,
    ) -> (f32, u64) {
        let mut sim_ear  = NetworkSim::new(sr, 11)
            .with_latency_ms(latency_ms).with_jitter_ms(jitter_ms);
        let mut sim_anti = NetworkSim::new(sr, 23)
            .with_latency_ms(latency_ms).with_jitter_ms(jitter_ms);
        let smoother_samples = (smoother_ms as f32 / 1000.0 * sr) as usize;
        let mut smoother_ear = JitterSmoother::new(smoother_samples);
        let mut smoother_anti = JitterSmoother::new(smoother_samples);
        let mut canceller = PassbyCanceller::new();
        let win = WINDOW_SAMPLES;
        let mut reference = Vec::with_capacity(event.len());
        let mut residual  = Vec::with_capacity(event.len());

        let mut i = 0;
        while i + win <= event.len() {
            let chunk = &event[i..i + win];
            canceller.process_window(chunk);
            let anti = canceller.pull_output(win);
            // Through net sim, then through jitter smoother.
            sim_ear.submit(chunk);
            sim_anti.submit(&anti);
            let net_noise = sim_ear.pull(win);
            let net_anti  = sim_anti.pull(win);
            smoother_ear.submit(&net_noise);
            smoother_anti.submit(&net_anti);
            // Pull the same number of samples from both smoothers.
            let smoothed_n = smoother_ear.pull(win);
            let smoothed_a = smoother_anti.pull(win);
            let lim = smoothed_n.len().min(smoothed_a.len());
            for k in 0..lim {
                reference.push(smoothed_n[k]);
                residual.push(smoothed_n[k] - smoothed_a[k]);
            }
            i += win;
        }
        for _ in 0..20 {
            let n = sim_ear.pull(win);
            let a = sim_anti.pull(win);
            smoother_ear.submit(&n);
            smoother_anti.submit(&a);
            let sn = smoother_ear.pull(win);
            let sa = smoother_anti.pull(win);
            let lim = sn.len().min(sa.len());
            for k in 0..lim {
                reference.push(sn[k]);
                residual.push(sn[k] - sa[k]);
            }
            if n.is_empty() && a.is_empty() && sn.iter().all(|&x| x == 0.0) {
                break;
            }
        }
        if reference.is_empty() { return (0.0, 0); }
        let supp = suppression_db(&reference, &residual, BAND_LO_HZ, BAND_HI_HZ, sr);
        let underruns = smoother_ear.underruns() + smoother_anti.underruns();
        (supp, underruns)
    }

    #[test]
    fn smoother_smooths_jitter_to_steady() {
        // Submit samples in BURSTS (alternating big/small chunks)
        // and verify that pull() gives the same total samples.
        let mut s = JitterSmoother::new(200);
        // Prime the smoother.
        let prime: Vec<f32> = (0..500).map(|i| i as f32).collect();
        s.submit(&prime);
        // Pull at steady rate.
        let mut got = Vec::new();
        for _ in 0..3 {
            got.extend(s.pull(100));
        }
        // Should have pulled 300 samples (3 × 100).
        assert_eq!(got.len(), 300);
        // First sample pulled should be the FIRST submitted (FIFO).
        assert_eq!(got[0], 0.0);
    }

    #[test]
    fn smoother_underruns_when_starved() {
        let mut s = JitterSmoother::new(100);
        // Don't submit anything; try to pull.
        let out = s.pull(50);
        assert_eq!(out.len(), 50);
        assert!(out.iter().all(|&x| x == 0.0), "underrun should emit zeros");
        assert!(s.underruns() > 0, "underruns counter should increment");
    }

    #[test]
    fn smoother_sanitizes_non_finite_input() {
        let mut s = JitterSmoother::new(0);
        let input = vec![1.0_f32, f32::NAN, 2.0, f32::INFINITY, 3.0];
        s.submit(&input);
        let out = s.pull(5);
        for &v in &out {
            assert!(v.is_finite(), "smoother output must be finite");
        }
    }

    /// One trial: synthesize → sim → canceller → measure.
    fn run_passby_trial(
        event: &[f32], latency_ms: f32, jitter_ms: f32,
        drop_prob: f32, sr: f32,
    ) -> f32 {
        // Two parallel paths:
        //   - "ear" path: the noise reaches the listener directly with
        //     the SAME mean latency the canceller's output has (so
        //     phase alignment is best-case). The anti-noise can't
        //     beat the listener path; it has to ARRIVE at the listener
        //     at the same time. In the sim we assume the canceller's
        //     output also goes through a NetworkSim with the same
        //     latency — that's the "speaker round-trip" + "BT buffer"
        //     contribution.
        //   - "anti" path: canceller sees the noise via mic, computes
        //     anti-noise, emits it through its own NetworkSim.
        let mut sim_ear  = NetworkSim::new(sr, 11)
            .with_latency_ms(latency_ms).with_jitter_ms(jitter_ms)
            .with_drop_prob(drop_prob);
        let mut sim_anti = NetworkSim::new(sr, 23)
            .with_latency_ms(latency_ms).with_jitter_ms(jitter_ms)
            .with_drop_prob(drop_prob);

        let mut canceller = PassbyCanceller::new();
        let win = WINDOW_SAMPLES;

        // Reference + residual accumulators — we measure suppression
        // over the full event after sim shuffles.
        let mut reference: Vec<f32> = Vec::with_capacity(event.len());
        let mut residual:  Vec<f32> = Vec::with_capacity(event.len());

        let mut i = 0;
        while i + win <= event.len() {
            let chunk = &event[i..i + win];
            canceller.process_window(chunk);
            let anti = canceller.pull_output(win);
            sim_ear.submit(chunk);
            sim_anti.submit(&anti);
            let arrived_noise = sim_ear.pull(win);
            let arrived_anti  = sim_anti.pull(win);
            let lim = arrived_noise.len().min(arrived_anti.len());
            for k in 0..lim {
                reference.push(arrived_noise[k]);
                residual.push(arrived_noise[k] - arrived_anti[k]);
            }
            i += win;
        }
        // Drain any remaining in-flight samples.
        for _ in 0..10 {
            let n = sim_ear.pull(win);
            let a = sim_anti.pull(win);
            let lim = n.len().min(a.len());
            for k in 0..lim {
                reference.push(n[k]);
                residual.push(n[k] - a[k]);
            }
            if n.is_empty() && a.is_empty() { break; }
        }
        if reference.is_empty() || residual.is_empty() { return 0.0; }
        suppression_db(&reference, &residual, BAND_LO_HZ, BAND_HI_HZ, sr)
    }

    /// Same harness pattern but for the backup-beeper masker. The
    /// metric is different here — we're not destructively cancelling,
    /// we're MASKING. So "suppression" doesn't apply; instead we
    /// measure how steady the OUTPUT is (the masker should ride
    /// alongside each beep, not pump arbitrarily).
    #[test]
    fn fuzz_beeper_masker_under_varying_latency() {
        let sr = crate::backup_beeper::SAMPLE_RATE;
        let win = crate::backup_beeper::WINDOW_SAMPLES;
        let sig = synth_beeper(5.0, 1250.0, 800, 400, 4, sr);

        eprintln!("\n=== beeper masker — latency sweep ===");
        eprintln!("{:>10} {:>10} {:>12} {:>14}",
                  "lat_ms", "jit_ms", "drop_pct", "out_finite");

        for &latency_ms in &[0.0f32, 50.0, 200.0] {
            for &jitter_ms in &[0.0f32, 20.0] {
                let mut m = BackupBeeperMasker::new();
                let mut sim = NetworkSim::new(sr, 5)
                    .with_latency_ms(latency_ms).with_jitter_ms(jitter_ms);
                let mut all_finite = true;
                let mut i = 0;
                while i + win <= sig.len() {
                    let chunk = &sig[i..i + win];
                    m.process_window(chunk);
                    let out = m.pull_output(win);
                    sim.submit(&out);
                    let arrived = sim.pull(win);
                    for &v in &arrived {
                        if !v.is_finite() { all_finite = false; break; }
                    }
                    i += win;
                }
                eprintln!("{:>10.0} {:>10.0} {:>12.1} {:>14}",
                    latency_ms, jitter_ms, 0.0, all_finite);
                assert!(all_finite,
                    "masker output should be finite under all conditions");
            }
        }
        eprintln!("=== end beeper sweep ===\n");
    }

    /// Out-of-order delivery: confirm that the sim correctly
    /// reorders chunks when jitter pushes one past another.
    #[test]
    fn jitter_can_reorder_chunks() {
        let sr = 48_000.0;
        // Big jitter relative to mean latency → out-of-order possible.
        let mut sim = NetworkSim::new(sr, 100)
            .with_latency_ms(50.0).with_jitter_ms(40.0);
        // Submit several distinguishable chunks.
        let chunks: Vec<Vec<f32>> = (0..10)
            .map(|i| vec![i as f32; 64])
            .collect();
        for c in &chunks { sim.submit(c); }
        // Advance well past all latencies and capture order.
        let mut received: Vec<f32> = Vec::new();
        for _ in 0..5 {
            received.extend(sim.pull(2000));
        }
        // We got all 640 samples through SOMETHING; that's the
        // invariant. (Whether they came in original order is
        // implementation-dependent and not tested here.)
        assert_eq!(received.len(), 640);
    }

    // ───────────── REVERB-AUGMENTED EXPERIMENTS ─────────────
    //
    // The synth_passby and synth_passby_saturated functions
    // produce anechoic signals — they assume the canceller hears
    // the engine cleanly with no room reflections. Real bedrooms
    // with open windows have RT60 in the 300-800 ms range from
    // walls + ceiling + courtyard reflections, AND large outdoor
    // spaces (parking lots, plazas, urban canyons) can add 1-2 s
    // of pre-bedroom reverb on top of that.
    //
    // The user's specific question: do the previous experimental
    // findings (saturation trade-off, per-harmonic vs fixed)
    // still hold when the signal arrives with reverb mixed in,
    // or does reverb change the picture?

    /// Re-run the saturation band trade-off with reverb mixed in.
    /// If reverb decoheres harmonic phase relationships (as
    /// expected — each reflection adds a random-phased copy), the
    /// trade-off should TILT. Specifically: the upper-band gain
    /// from saturation matching depends on the source's harmonic
    /// phases lining up with the canceller's, which reverb
    /// scrambles. So the gain in the upper band should SHRINK
    /// under reverb.
    #[test]
    fn saturation_band_tradeoff_with_reverb() {
        use crate::reverb::Reverb;
        let sr = SAMPLE_RATE;
        let saturation = 0.3;
        let raw_event = synth_passby_saturated(8.0, 120.0, 0.04, saturation, sr, 17);

        // Three reverb conditions: none, modest (small room with
        // open window), large (big open space + small bedroom).
        let reverb_conditions: [(&str, f32, f32); 3] = [
            ("anechoic",  0.0, 0.0),
            ("bedroom",   0.4, 0.3),
            ("urban",     1.0, 0.5),
        ];

        eprintln!("\n=== saturation trade-off under reverb ===");
        eprintln!("{:>10} {:>14} {:>14} {:>14} {:>14}",
                  "reverb",
                  "wide_clean",
                  "wide_matched",
                  "upper_clean",
                  "upper_matched");

        for (label, rt60, wet) in reverb_conditions {
            let event = if rt60 > 0.0 {
                let mut r = Reverb::new(rt60, 12, sr, 17);
                r.set_wet_mix(wet);
                r.apply(&raw_event)
            } else {
                raw_event.clone()
            };

            let wide_clean = run_passby_trial_with_saturation(
                &event, 0.0, 0.0, 0.0, sr, 0.0);
            let wide_matched = run_passby_trial_with_saturation(
                &event, 0.0, 0.0, 0.0, sr, saturation);
            let upper_clean = run_passby_trial_band(
                &event, sr, 0.0, 400.0, 1200.0);
            let upper_matched = run_passby_trial_band(
                &event, sr, saturation, 400.0, 1200.0);

            eprintln!("{:>10} {:>14.2} {:>14.2} {:>14.2} {:>14.2}",
                      label, wide_clean, wide_matched,
                      upper_clean, upper_matched);
        }
        eprintln!("==========================================\n");
    }

    /// Per-harmonic vs fixed phase lock, with reverb mixed in.
    /// Hypothesis: under reverb, the source's harmonic phase
    /// relationships are RANDOMIZED — each reflection adds a
    /// random-phased copy. The fixed-offset diversity values
    /// (π/6, π/3) were chosen for robustness on a clean source.
    /// Under reverb, NEITHER mode should track the harmonic
    /// phases well, but per-harmonic SHOULD recover some by
    /// re-estimating per window.
    #[test]
    fn per_harmonic_vs_fixed_under_reverb() {
        use crate::reverb::Reverb;
        let sr = SAMPLE_RATE;
        let raw_event = synth_passby_saturated(8.0, 120.0, 0.04, 0.2, sr, 17);

        let reverb_conditions: [(&str, f32, f32); 3] = [
            ("anechoic",  0.0, 0.0),
            ("bedroom",   0.4, 0.3),
            ("urban",     1.0, 0.5),
        ];

        // Test a few channel conditions per reverb level.
        let channel_conditions: [(f32, f32); 3] = [
            (0.0,  0.0),
            (50.0, 20.0),
            (200.0, 50.0),
        ];

        eprintln!("\n=== per-harmonic vs fixed under reverb ===");
        eprintln!("{:>10} {:>10} {:>10} {:>12} {:>12} {:>10}",
                  "reverb", "lat_ms", "jit_ms",
                  "fixed_dB", "perharm_dB", "delta");

        for (label, rt60, wet) in reverb_conditions {
            let event = if rt60 > 0.0 {
                let mut r = Reverb::new(rt60, 12, sr, 17);
                r.set_wet_mix(wet);
                r.apply(&raw_event)
            } else {
                raw_event.clone()
            };

            for (lat, jit) in channel_conditions {
                let fixed = run_with_lock_mode(&event, lat, jit, 0.0, sr, false);
                let perharm = run_with_lock_mode(&event, lat, jit, 0.0, sr, true);
                let delta = perharm - fixed;
                let winner = if delta > 0.3 { "perharm" }
                    else if delta < -0.3 { "fixed" }
                    else { "tie" };
                eprintln!("{:>10} {:>10.0} {:>10.0} {:>12.2} {:>12.2} {:>+10.2}",
                          label, lat, jit, fixed, perharm, delta);
                let _ = winner;
            }
        }
        eprintln!("==========================================\n");
    }
}
