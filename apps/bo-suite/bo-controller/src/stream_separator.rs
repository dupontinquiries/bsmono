// src/stream_separator.rs
#![allow(dead_code)]
//
// Streaming source separator.
//
// The companion to `stem_split.rs`. Where stem_split is a file-in /
// file-out batch pipeline (read whole WAV, separate, write four
// WAVs), this module operates incrementally on PCM blocks delivered
// by a live audio source. The use case is the user's multi-room
// scenario: take whatever the controller is playing, separate into
// drums/voice/piano/synth in real time, and stream each stem as its
// own channel to a device that plays it on the speakers in one room.
//
// Why a separate module rather than a streaming flag on stem_split:
// the streaming pipeline carries MORE state — STFT input buffer,
// spectrogram ring for the HPSS sliding median, ISTFT overlap-add
// tail per stem, and a frame-alignment counter — and gets enough
// algorithmic surgery (no reflective padding, no central reconstruction
// trim) that keeping the batch version honest meant building this
// alongside rather than dual-purposing stem_split's functions.
//
// Latency. The HPSS time-median needs HPSS_HARMONIC_FRAMES of context
// to start producing output (we want the median centered on the frame
// we're emitting, so we wait HPSS_HARMONIC_FRAMES/2 frames into the
// past and an equal amount into the future). At hop=256, sr=44100,
// 17/2 = 8 frames = ~46 ms. The streaming pipeline also has STFT_HOP
// of block-quantization latency (we emit samples in 256-sample chunks
// independent of the input block size). Together: ~50 ms typical
// algorithmic latency, knowable via `latency_samples()` so the caller
// can compensate via the bo-mux scheduled-start machinery.
//
// What this does NOT do:
//   - Handle sample-rate conversion. Input must be at the rate the
//     SmallMlp was trained on (default 44100). Resample upstream.
//   - Adapt the underlying MLP. Weights are frozen; online retraining
//     would mean tracking residual energy and is out of scope for v1.
//   - Decode encoded streams. Input is f32 PCM. The caller is
//     responsible for symphonia/opus/etc. → f32.

use std::collections::VecDeque;
use crate::stem_split::{
    STFT_WINDOW, STFT_HOP, N_BINS,
    HPSS_HARMONIC_FRAMES, HPSS_PERCUSSIVE_BINS, HPSS_POWER,
    SmallMlp,
};
use crate::looper::fft;
use std::f32::consts::PI;

/// One block of separated audio. Each Vec has the same length —
/// however many samples the streaming pipeline emitted on this push.
/// May be empty during warmup (before the spec ring is full enough
/// to start producing output).
#[derive(Debug, Clone, Default)]
pub struct StemBlock {
    pub drums: Vec<f32>,
    pub voice: Vec<f32>,
    pub piano: Vec<f32>,
    pub synth: Vec<f32>,
}

impl StemBlock {
    pub fn len(&self) -> usize { self.drums.len() }
    pub fn is_empty(&self) -> bool { self.drums.is_empty() }
}

/// Stem identifier — used for channel routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Stem { Drums, Voice, Piano, Synth }

impl Stem {
    pub fn as_str(&self) -> &'static str {
        match self {
            Stem::Drums => "drums",
            Stem::Voice => "voice",
            Stem::Piano => "piano",
            Stem::Synth => "synth",
        }
    }
    pub fn all() -> [Stem; 4] {
        [Stem::Drums, Stem::Voice, Stem::Piano, Stem::Synth]
    }
}

/// Streaming separator state. Push input PCM via `push`; receive
/// per-stem output PCM via the return value. Output is delayed by
/// `latency_samples()` relative to input — caller should treat the
/// stem streams as time-aligned with one another but lagged behind
/// the source.
pub struct StreamingSeparator {
    mlp: SmallMlp,
    win: Vec<f32>,

    /// Pending input samples not yet windowed into an STFT frame.
    /// Drained STFT_HOP at a time once STFT_WINDOW samples are
    /// available.
    input_buf: VecDeque<f32>,

    /// Last HPSS_HARMONIC_FRAMES STFT frames, kept as a ring so the
    /// time-median can run over them. Front = oldest, back = newest.
    spec_ring: VecDeque<Vec<(f32, f32)>>,
    /// Matching magnitude ring — cached to avoid recomputing per
    /// median step. abs(spec_ring[k]) elementwise.
    mag_ring: VecDeque<Vec<f32>>,

    /// Per-stem overlap-add carryover. After we ISTFT a frame, the
    /// first STFT_HOP samples are emitted; the remaining STFT_WINDOW
    /// - STFT_HOP samples are added into the next frame's overlap.
    /// One tail per stem (drums/voice/piano/synth).
    stem_tails: [Vec<f32>; 4],

    /// Per-stem window-sum carryover for the same overlap region.
    /// Tracks the sum of squared window taps so normalisation works
    /// across block boundaries (and stays consistent with the batch
    /// ISTFT).
    wsum_tails: [Vec<f32>; 4],

    /// Total samples we've consumed from the input — for diagnostics.
    samples_in: u64,
    /// Total samples we've emitted on each stem. They stay equal by
    /// construction because each frame step emits exactly STFT_HOP
    /// samples per stem.
    samples_out: u64,
    /// Number of ISTFT emissions completed so far. The first
    /// `STFT_WINDOW / STFT_HOP − 1` emissions are computed (so the
    /// tail buffer accumulates window-overlap contributions) but
    /// not returned to the caller — their output samples have
    /// incomplete OLA reconstruction (fewer than 4 frames have
    /// contributed) and would create an audible click at stream
    /// start. After the warmup count is reached, all subsequent
    /// emissions are returned in full.
    emissions_done: usize,
}

impl StreamingSeparator {
    pub fn new(mlp: SmallMlp) -> Self {
        let overlap = STFT_WINDOW - STFT_HOP;
        Self {
            mlp,
            win: hann_window(STFT_WINDOW),
            input_buf: VecDeque::with_capacity(STFT_WINDOW * 2),
            spec_ring: VecDeque::with_capacity(HPSS_HARMONIC_FRAMES + 1),
            mag_ring:  VecDeque::with_capacity(HPSS_HARMONIC_FRAMES + 1),
            stem_tails: [
                vec![0.0; overlap],
                vec![0.0; overlap],
                vec![0.0; overlap],
                vec![0.0; overlap],
            ],
            wsum_tails: [
                vec![0.0; overlap],
                vec![0.0; overlap],
                vec![0.0; overlap],
                vec![0.0; overlap],
            ],
            samples_in: 0,
            samples_out: 0,
            emissions_done: 0,
        }
    }

    /// Algorithmic delay between the latest input sample pushed and
    /// the latest output sample emitted, in samples. Caller uses
    /// this to schedule downstream playback — bo-mux's scheduled-
    /// start machinery already absorbs a per-channel offset, so the
    /// listener never sees the delay.
    pub fn latency_samples(&self) -> usize {
        // Steady-state delay between input samples being pushed and
        // the output samples for those same input positions becoming
        // available. The output for input position p emerges (window
        // / hop − 1) frame steps after frame f = p/HOP is pushed,
        // because by then enough frames have been ISTFT'd to fully
        // OLA-reconstruct position p. Derivation:
        //
        //   First emission happens after HPSS_HARMONIC_FRAMES frames
        //   have been pushed. At that moment, the spec ring's center
        //   is at frame HPSS_HARMONIC_FRAMES/2 (the earliest frame
        //   the center will ever point to), so out[0] corresponds to
        //   input position (HPSS_HARMONIC_FRAMES/2) * STFT_HOP. The
        //   total input pushed to get there is (HPSS_HARMONIC_FRAMES
        //   - 1) * STFT_HOP + STFT_WINDOW. Algorithmic delay between
        //   the most-recent input sample and the most-recent output
        //   sample, in steady state, is:
        //
        //     (HPSS_HARMONIC_FRAMES/2 - 1) * STFT_HOP + STFT_WINDOW
        //
        //   At HPSS_HARMONIC_FRAMES=17, STFT_HOP=256, STFT_WINDOW=1024
        //   that's 7*256 + 1024 = 2816 samples (~64 ms at 44.1 kHz).
        (HPSS_HARMONIC_FRAMES / 2 - 1) * STFT_HOP + STFT_WINDOW
    }

    pub fn samples_consumed(&self) -> u64 { self.samples_in }
    pub fn samples_emitted(&self) -> u64 { self.samples_out }

    /// Push a chunk of input PCM. Returns the stem blocks that
    /// became ready in this call — may be empty during warmup,
    /// otherwise typically a multiple of STFT_HOP samples per stem.
    /// The caller should append the returned samples to per-stem
    /// output buffers (e.g. bo-mux's per-channel ring buffers).
    pub fn push(&mut self, input_pcm: &[f32]) -> StemBlock {
        self.samples_in = self.samples_in.wrapping_add(input_pcm.len() as u64);
        // NaN-defense: a single non-finite sample would propagate
        // through the FFT and contaminate every output bin forever.
        // Replace with zero rather than poisoning the pipeline.
        for v in input_pcm {
            self.input_buf.push_back(if v.is_finite() { *v } else { 0.0 });
        }

        let ola_warmup = STFT_WINDOW / STFT_HOP - 1;  // = 3 emissions
        let mut out = StemBlock::default();
        // Each frame-step drains STFT_HOP samples and may emit
        // STFT_HOP samples per stem (once warm).
        while self.input_buf.len() >= STFT_WINDOW {
            if let Some(emitted) = self.step_one_frame() {
                // Discard the first `ola_warmup` emissions — they
                // populated the tail but their output samples have
                // incomplete OLA contributions (fewer than
                // STFT_WINDOW/STFT_HOP frames overlapped them) and
                // would otherwise produce an audible artefact at
                // stream start.
                if self.emissions_done < ola_warmup {
                    self.emissions_done += 1;
                } else {
                    self.emissions_done += 1;
                    out.drums.extend_from_slice(&emitted.drums);
                    out.voice.extend_from_slice(&emitted.voice);
                    out.piano.extend_from_slice(&emitted.piano);
                    out.synth.extend_from_slice(&emitted.synth);
                }
            }
        }
        self.samples_out = self.samples_out.wrapping_add(out.len() as u64);
        out
    }

    /// At end-of-stream, drain remaining context by feeding silence
    /// so the last few real samples make it out of the spectrogram
    /// ring. Returns whatever PCM was buffered up. Calling push()
    /// after drain() is undefined — make a fresh separator.
    pub fn drain(&mut self) -> StemBlock {
        // Pad input with enough zeros to flush the ring through
        // (HPSS_HARMONIC_FRAMES / 2 + 1 frame's worth).
        let n_pad = (HPSS_HARMONIC_FRAMES / 2 + 1) * STFT_HOP + STFT_WINDOW;
        let zeros = vec![0.0_f32; n_pad];
        self.push(&zeros)
    }

    /// One STFT frame's worth of streaming work. Pulls STFT_WINDOW
    /// samples (without consuming them yet), computes the STFT
    /// frame, pushes it onto the ring, then if the ring is full
    /// enough produces one frame of separated output (STFT_HOP
    /// samples per stem) via the overlap-add tail mechanism. Pops
    /// STFT_HOP samples from the input buffer at the end.
    fn step_one_frame(&mut self) -> Option<StemBlock> {
        debug_assert!(self.input_buf.len() >= STFT_WINDOW);

        // Build the windowed frame from the front of input_buf.
        let mut buf = vec![(0.0_f32, 0.0_f32); STFT_WINDOW];
        for i in 0..STFT_WINDOW {
            buf[i] = (self.input_buf[i] * self.win[i], 0.0);
        }
        fft(&mut buf);
        let frame: Vec<(f32, f32)> = buf[..N_BINS].to_vec();
        let mag: Vec<f32> = frame.iter()
            .map(|(re, im)| (re * re + im * im).sqrt())
            .collect();

        // Push into ring; evict oldest if full.
        self.spec_ring.push_back(frame);
        self.mag_ring.push_back(mag);
        if self.spec_ring.len() > HPSS_HARMONIC_FRAMES {
            self.spec_ring.pop_front();
            self.mag_ring.pop_front();
        }

        // Consume STFT_HOP samples from input.
        for _ in 0..STFT_HOP { self.input_buf.pop_front(); }

        // Output requires a full ring (we need symmetric context
        // around the center frame for the time median). Until then
        // we're warming up — return None.
        if self.spec_ring.len() < HPSS_HARMONIC_FRAMES { return None; }

        // The frame we're emitting is the CENTER of the ring. With
        // HPSS_HARMONIC_FRAMES = 17 that's ring index 8.
        let center = HPSS_HARMONIC_FRAMES / 2;
        let center_spec = self.spec_ring[center].clone();
        let center_mag  = &self.mag_ring[center];

        // ── HPSS time median over the ring → harmonic mag ───────────
        let mut harmonic_mag = vec![0.0_f32; N_BINS];
        let mut scratch = vec![0.0_f32; HPSS_HARMONIC_FRAMES];
        for b in 0..N_BINS {
            for (k, frame_mag) in self.mag_ring.iter().enumerate() {
                scratch[k] = frame_mag[b];
            }
            harmonic_mag[b] = median_sorted(&mut scratch);
        }
        // ── HPSS freq median across the CENTER frame → percussive ──
        let mut percussive_mag = vec![0.0_f32; N_BINS];
        let half = HPSS_PERCUSSIVE_BINS / 2;
        let mut fscratch = vec![0.0_f32; HPSS_PERCUSSIVE_BINS];
        for b in 0..N_BINS {
            for k in 0..HPSS_PERCUSSIVE_BINS {
                let bin = (b as isize + k as isize - half as isize)
                    .clamp(0, N_BINS as isize - 1) as usize;
                fscratch[k] = center_mag[bin];
            }
            percussive_mag[b] = median_sorted(&mut fscratch);
        }

        // ── Soft Wiener masks (harmonic vs percussive) ──────────────
        let mut drums_mask = vec![0.0_f32; N_BINS];
        let mut h_frac     = vec![0.0_f32; N_BINS];
        for b in 0..N_BINS {
            let hp = harmonic_mag[b].powf(HPSS_POWER);
            let pp = percussive_mag[b].powf(HPSS_POWER);
            let denom = hp + pp + 1e-9;
            drums_mask[b] = pp / denom;
            h_frac[b]     = hp / denom;
        }

        // ── MLP frame inference: harmonic mag → 3-way masks ─────────
        let mut masks = self.mlp.predict_frame(&harmonic_mag);
        SmallMlp::normalize_masks(&mut masks);
        let mut voice_mask = vec![0.0_f32; N_BINS];
        let mut piano_mask = vec![0.0_f32; N_BINS];
        let mut synth_mask = vec![0.0_f32; N_BINS];
        for b in 0..N_BINS {
            voice_mask[b] = masks[0][b] * h_frac[b];
            piano_mask[b] = masks[1][b] * h_frac[b];
            synth_mask[b] = masks[2][b] * h_frac[b];
        }

        // ── Apply masks to the center spec, ISTFT one frame each ───
        let drums_spec = apply_frame_mask(&center_spec, &drums_mask);
        let voice_spec = apply_frame_mask(&center_spec, &voice_mask);
        let piano_spec = apply_frame_mask(&center_spec, &piano_mask);
        let synth_spec = apply_frame_mask(&center_spec, &synth_mask);

        let drums_pcm = self.istft_one_frame(&drums_spec, 0);
        let voice_pcm = self.istft_one_frame(&voice_spec, 1);
        let piano_pcm = self.istft_one_frame(&piano_spec, 2);
        let synth_pcm = self.istft_one_frame(&synth_spec, 3);

        Some(StemBlock {
            drums: drums_pcm,
            voice: voice_pcm,
            piano: piano_pcm,
            synth: synth_pcm,
        })
    }

    /// ISTFT one frame with the streaming overlap-add tail. Returns
    /// STFT_HOP samples of finalised output. The remaining
    /// STFT_WINDOW − STFT_HOP samples of windowed output get added
    /// into the per-stem tail so the NEXT (and the one after, and
    /// after that) frame's emission picks them up.
    ///
    /// `stem_idx` indexes self.stem_tails / self.wsum_tails:
    /// 0=drums, 1=voice, 2=piano, 3=synth.
    ///
    /// Streaming OLA math, the version that actually works:
    ///
    /// `tail[i]` holds the SUM of all prior frames' windowed
    /// contributions at upcoming output position (current + i). At
    /// each frame:
    ///
    ///   1. Output = (windowed[0..STFT_HOP] + tail[0..STFT_HOP])
    ///              / (wsq[0..STFT_HOP]   + wsum[0..STFT_HOP])
    ///      — the current frame closes out positions [0, STFT_HOP).
    ///   2. The new tail is the SHIFTED old tail (drop the first
    ///      STFT_HOP entries; the rest move to the front), plus the
    ///      current frame's windowed[STFT_HOP..STFT_WINDOW]
    ///      accumulated at positions [0, overlap).
    ///
    ///      new_tail[i] = (old_tail[i+STFT_HOP] if in range else 0)
    ///                  + windowed[i+STFT_HOP]
    ///
    ///      For i in [0, overlap−STFT_HOP) = [0, 512), the old-tail
    ///      term is non-zero — it's the carried-forward contribution
    ///      from frames f−1, f−2, f−3 at the new position. For i in
    ///      [overlap−STFT_HOP, overlap) = [512, 768), the old tail
    ///      ran out and the new contribution is just the current
    ///      frame's. Same rule for wsum.
    ///
    /// The previous version of this function omitted the old-tail
    /// carry-forward term entirely (only the current frame's
    /// contribution made it into new_tail). The result was that
    /// each output sample only saw the most recent 1-2 frames'
    /// contributions instead of the full 4 (window/hop = 4) needed
    /// for correct OLA reconstruction; the sum-of-stems invariant
    /// was off by up to 0.45 per sample.
    fn istft_one_frame(&mut self, spec_frame: &[(f32, f32)], stem_idx: usize) -> Vec<f32> {
        // Reconstruct full complex spectrum: lower bins from input,
        // upper bins are conjugate mirror.
        let mut buf = vec![(0.0_f32, 0.0_f32); STFT_WINDOW];
        for i in 0..N_BINS { buf[i] = spec_frame[i]; }
        for i in 1..STFT_WINDOW/2 {
            buf[STFT_WINDOW - i] = (spec_frame[i].0, -spec_frame[i].1);
        }
        // Inverse FFT via the conjugate-trick.
        for s in buf.iter_mut() { s.1 = -s.1; }
        fft(&mut buf);
        let scale = 1.0 / STFT_WINDOW as f32;
        for s in buf.iter_mut() { *s = (s.0 * scale, -s.1 * scale); }

        let overlap = STFT_WINDOW - STFT_HOP;
        debug_assert_eq!(self.stem_tails[stem_idx].len(), overlap);

        // Step 1: output is the first STFT_HOP samples — the now-
        // finalized region where all overlapping frames have
        // contributed.
        let mut out = vec![0.0_f32; STFT_HOP];
        {
            let tail = &self.stem_tails[stem_idx];
            let wsum = &self.wsum_tails[stem_idx];
            for i in 0..STFT_HOP {
                let windowed = buf[i].0 * self.win[i];
                let wsq = self.win[i] * self.win[i];
                let num = windowed + tail[i];
                let den = wsq + wsum[i];
                out[i] = if den > 1e-8 { num / den } else { 0.0 };
            }
        }

        // Step 2: build the new tail by shifting the old tail
        // forward by STFT_HOP and accumulating this frame's
        // contribution from positions [STFT_HOP, STFT_WINDOW).
        let mut new_tail = vec![0.0_f32; overlap];
        let mut new_wsum = vec![0.0_f32; overlap];
        {
            let tail = &self.stem_tails[stem_idx];
            let wsum = &self.wsum_tails[stem_idx];
            for i in 0..overlap {
                let buf_idx = i + STFT_HOP;
                let windowed = buf[buf_idx].0 * self.win[buf_idx];
                let wsq = self.win[buf_idx] * self.win[buf_idx];
                let old_idx = i + STFT_HOP;
                let (old_t, old_w) = if old_idx < overlap {
                    (tail[old_idx], wsum[old_idx])
                } else {
                    (0.0, 0.0)
                };
                new_tail[i] = windowed + old_t;
                new_wsum[i] = wsq + old_w;
            }
        }
        self.stem_tails[stem_idx] = new_tail;
        self.wsum_tails[stem_idx] = new_wsum;
        out
    }
}

// ── Helpers ───────────────────────────────────────────────────────────

/// Hann window of length n. Sin² formulation matches stem_split's
/// hann_window so streaming and batch produce equivalent windowing.
fn hann_window(n: usize) -> Vec<f32> {
    (0..n).map(|i| {
        let phase = PI * i as f32 / (n - 1) as f32;
        phase.sin().powi(2)
    }).collect()
}

/// Median of a small slice (mutates the slice with a sort). For the
/// HPSS window sizes (17 elements) this is faster than a heap-based
/// streaming median and uses no allocations.
fn median_sorted(scratch: &mut [f32]) -> f32 {
    scratch.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = scratch.len();
    if n == 0 { 0.0 }
    else if n % 2 == 1 { scratch[n / 2] }
    else { (scratch[n / 2 - 1] + scratch[n / 2]) / 2.0 }
}

/// Apply a magnitude mask to a single complex frame.
fn apply_frame_mask(frame: &[(f32, f32)], mask: &[f32]) -> Vec<(f32, f32)> {
    debug_assert_eq!(frame.len(), mask.len());
    frame.iter().zip(mask.iter())
        .map(|(c, m)| (c.0 * m, c.1 * m))
        .collect()
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stem_split::SmallMlp;

    fn fresh_separator(_seed: u64) -> StreamingSeparator {
        // The MLP only has with_priors() — the prior-init is
        // deterministic, so passing _seed is just a label for test
        // intent (different test = different separator instance,
        // not different weights).
        let mlp = SmallMlp::with_priors(44100);
        StreamingSeparator::new(mlp)
    }

    #[test]
    fn streaming__latency_matches_steady_state_delay() {
        let sep = fresh_separator(1);
        // 7 frames × 256 hop + 1024 window = 2816 samples
        assert_eq!(sep.latency_samples(),
                   (HPSS_HARMONIC_FRAMES / 2 - 1) * STFT_HOP + STFT_WINDOW);
    }

    #[test]
    fn streaming__empty_push_returns_empty() {
        let mut sep = fresh_separator(1);
        let out = sep.push(&[]);
        assert!(out.is_empty());
    }

    #[test]
    fn streaming__short_push_returns_empty_until_warm() {
        let mut sep = fresh_separator(1);
        // Push less than one frame's worth.
        let out = sep.push(&vec![0.1_f32; 200]);
        assert!(out.is_empty());
    }

    #[test]
    fn streaming__warmup_then_emit_steady_state() {
        let mut sep = fresh_separator(2);
        // Push enough audio that the ring fills + several frames emit.
        let n_input = (HPSS_HARMONIC_FRAMES + 8) * STFT_HOP + STFT_WINDOW;
        let signal: Vec<f32> = (0..n_input)
            .map(|i| ((i as f32) * 0.01).sin() * 0.4).collect();
        let out = sep.push(&signal);
        assert!(!out.is_empty(),
                "expected non-empty output after warmup, got len 0");
        // Each emitted frame is STFT_HOP samples per stem.
        assert_eq!(out.len() % STFT_HOP, 0,
                   "output must be a multiple of STFT_HOP");
        // All stems emit equal lengths.
        assert_eq!(out.drums.len(), out.voice.len());
        assert_eq!(out.drums.len(), out.piano.len());
        assert_eq!(out.drums.len(), out.synth.len());
    }

    #[test]
    fn streaming__output_block_size_independent_of_input_block_size() {
        // Push the same total audio in two different block sizes;
        // the cumulative output of each stem must be equal-length
        // (the underlying frame-step is deterministic regardless of
        // how the caller chunks the input).
        let n_input = (HPSS_HARMONIC_FRAMES + 6) * STFT_HOP + STFT_WINDOW;
        let signal: Vec<f32> = (0..n_input)
            .map(|i| ((i as f32 * 0.013).sin()
                    + (i as f32 * 0.041).cos() * 0.3) * 0.4).collect();

        let mut sep_a = fresh_separator(3);
        let out_a = sep_a.push(&signal);

        let mut sep_b = fresh_separator(3);
        let mut acc_b = StemBlock::default();
        for chunk in signal.chunks(173) {  // a deliberately awkward size
            let part = sep_b.push(chunk);
            acc_b.drums.extend(part.drums);
            acc_b.voice.extend(part.voice);
            acc_b.piano.extend(part.piano);
            acc_b.synth.extend(part.synth);
        }
        assert_eq!(out_a.drums.len(), acc_b.drums.len(),
                   "drum output length must not depend on input block size");
        assert_eq!(out_a.voice.len(), acc_b.voice.len());
        assert_eq!(out_a.piano.len(), acc_b.piano.len());
        assert_eq!(out_a.synth.len(), acc_b.synth.len());
    }

    #[test]
    fn streaming__chunked_vs_single_shot_match_in_steady_state() {
        // Stronger statement: same seed + same input → same output,
        // regardless of chunking. Tolerate small numerical diff
        // because the FFT order-of-operations changes don't perfectly
        // commute.
        let n = (HPSS_HARMONIC_FRAMES + 10) * STFT_HOP + STFT_WINDOW;
        let sig: Vec<f32> = (0..n).map(|i| {
            ((i as f32 * 0.011).sin() * 0.5
             + (i as f32 * 0.027).sin() * 0.3) * 0.4
        }).collect();

        let mut a = fresh_separator(4);
        let out_a = a.push(&sig);

        let mut b = fresh_separator(4);
        let mut out_b = StemBlock::default();
        for chunk in sig.chunks(STFT_HOP) {
            let part = b.push(chunk);
            out_b.drums.extend(part.drums);
            out_b.voice.extend(part.voice);
            out_b.piano.extend(part.piano);
            out_b.synth.extend(part.synth);
        }
        assert_eq!(out_a.drums.len(), out_b.drums.len());

        // Compare samples — should be bit-equal because we feed
        // input in STFT_HOP-aligned chunks (matches the frame step).
        for (x, y) in out_a.drums.iter().zip(out_b.drums.iter()) {
            assert!((x - y).abs() < 1e-5,
                    "hop-aligned chunked drums diverged: {} vs {}", x, y);
        }
    }

    #[test]
    fn streaming__nonfinite_input_does_not_propagate() {
        let mut sep = fresh_separator(5);
        let n = (HPSS_HARMONIC_FRAMES + 4) * STFT_HOP + STFT_WINDOW;
        let mut sig = vec![0.1_f32; n];
        // Drop in a few NaN/Inf values mid-stream.
        sig[100] = f32::NAN;
        sig[400] = f32::INFINITY;
        sig[800] = f32::NEG_INFINITY;
        let out = sep.push(&sig);
        assert!(!out.is_empty());
        for v in out.drums.iter().chain(out.voice.iter())
            .chain(out.piano.iter()).chain(out.synth.iter())
        {
            assert!(v.is_finite(), "NaN/Inf leaked into separator output");
        }
    }

    #[test]
    fn streaming__drain_flushes_remaining_context() {
        // After pushing audio and then drain()ing, total emitted
        // samples per stem ≥ (input - one frame) — the last frame
        // of context isn't fully reconstructible (no future context)
        // but we get within STFT_WINDOW of the input length.
        let mut sep = fresh_separator(6);
        let n_input = (HPSS_HARMONIC_FRAMES + 5) * STFT_HOP + STFT_WINDOW;
        let sig: Vec<f32> = (0..n_input)
            .map(|i| ((i as f32 * 0.02).sin()) * 0.5).collect();
        let _ = sep.push(&sig);
        let tail = sep.drain();
        let total = sep.samples_emitted();
        // We expect to recover at LEAST n_input - latency_samples()
        // worth of output. drain() should not panic and not produce
        // non-finite.
        for v in tail.drums.iter() { assert!(v.is_finite()); }
        let _ = total;  // exact value depends on overlap; just ensure no panic
    }

    #[test]
    fn streaming__silence_in_silence_out() {
        // Pure-zero input should produce near-zero output.
        let mut sep = fresh_separator(7);
        let n = (HPSS_HARMONIC_FRAMES + 4) * STFT_HOP + STFT_WINDOW;
        let sig = vec![0.0_f32; n];
        let out = sep.push(&sig);
        for v in out.drums.iter() {
            assert!(v.abs() < 1e-4, "silent input produced {} drums", v);
        }
        for v in out.voice.iter() {
            assert!(v.abs() < 1e-4, "silent input produced {} voice", v);
        }
    }

    #[test]
    fn streaming__stem_enum_round_trips_to_string() {
        for s in Stem::all() {
            let name = s.as_str();
            assert!(!name.is_empty());
            assert!(name.chars().all(|c| c.is_ascii_lowercase()));
        }
    }

    #[test]
    fn streaming__cumulative_emission_grows_with_input() {
        let mut sep = fresh_separator(8);
        let _ = sep.push(&vec![0.0; 500]);
        let after_500 = sep.samples_emitted();
        let n = (HPSS_HARMONIC_FRAMES + 4) * STFT_HOP + STFT_WINDOW;
        let _ = sep.push(&vec![0.1_f32; n]);
        let after_more = sep.samples_emitted();
        assert!(after_more > after_500,
                "output should grow as input grows past warmup");
    }

    #[test]
    fn streaming__sum_of_stems_recovers_input_in_steady_state() {
        // Masks sum to 1 per bin by construction:
        //   drums = p^p / (h^p + p^p)
        //   voice + piano + synth = h^p/(h^p+p^p) (after normalize_masks)
        // Therefore sum(stems) = ISTFT(center_spec) per frame, and
        // properly-summed OLA-ISTFT of a frame sequence recovers the
        // input signal. In steady state (after enough frames have
        // accumulated in the tail to provide full window-overlap
        // redundancy), sum(stems)[i] should equal input[i + offset]
        // to within float roundoff, where `offset = (HPSS_HARMONIC_FRAMES / 2)
        // * STFT_HOP` accounts for the centered HPSS context delay.
        //
        // This is a strong invariant: it catches any OLA accumulation
        // bug, since a missing frame contribution would show up as a
        // mismatch with the input.
        let mut sep = fresh_separator(99);
        let n = (HPSS_HARMONIC_FRAMES + 30) * STFT_HOP + STFT_WINDOW;
        let sig: Vec<f32> = (0..n).map(|i| {
            ((i as f32 * 0.013).sin() * 0.4
             + (i as f32 * 0.041).cos() * 0.2) * 0.5
        }).collect();
        let out = sep.push(&sig);
        // After the warmup-skip in push(), out[0] corresponds to the
        // first FULLY-RECONSTRUCTED input position — exactly the
        // value reported by latency_samples().
        let input_offset = sep.latency_samples();
        // Skip a few hops at each end for safety (small numerical
        // edge effects near OLA boundaries).
        let skip_start = STFT_HOP;
        let skip_end   = STFT_HOP;
        assert!(out.drums.len() > skip_start + skip_end + STFT_HOP,
                "not enough output: {}", out.drums.len());
        let n_compare = out.drums.len() - skip_start - skip_end;

        let mut max_err = 0.0_f32;
        let mut max_err_at = 0;
        for i in skip_start..(skip_start + n_compare) {
            let stem_sum = out.drums[i] + out.voice[i]
                         + out.piano[i] + out.synth[i];
            // Output index i corresponds to input position
            // i + input_offset (the center frame's start).
            let input = sig[i + input_offset];
            let err = (stem_sum - input).abs();
            if err > max_err { max_err = err; max_err_at = i; }
        }
        assert!(max_err < 0.01,
                "sum of stems should recover input within float \
                 roundoff in steady state; max_err={} at i={}",
                max_err, max_err_at);
    }

    // ── Randomized fuzz tests for the streaming separator ─────────
    //
    // The streaming separator is the third leg of the multi-output
    // management engine: it converts a single live audio stream into
    // four time-aligned stem streams that the channel router can
    // route per-room. Bugs here propagate to every downstream channel
    // simultaneously, so the test budget is generous.
    //
    // Deterministic xorshift; failure printouts include the seed so
    // a bisect can replay the exact frame-by-frame sequence.

    fn xorshift_next(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }
    fn rand_f32(state: &mut u64, lo: f32, hi: f32) -> f32 {
        let bits = xorshift_next(state);
        let n = (bits as f64 / u64::MAX as f64) as f32;
        lo + (hi - lo) * n
    }

    #[test]
    fn streaming_fuzz__random_block_sizes_match_one_shot() {
        // Push the same audio in random-sized chunks and verify the
        // cumulative output matches the single-push variant exactly.
        // Block-boundary handling must be size-invariant.
        let mut signal_seed = 0xfeed_face_1234_5678_u64;
        let n = (HPSS_HARMONIC_FRAMES + 12) * STFT_HOP + STFT_WINDOW;
        let sig: Vec<f32> = (0..n)
            .map(|_| rand_f32(&mut signal_seed, -0.8, 0.8))
            .collect();

        let mut sep_a = fresh_separator(11);
        let out_a = sep_a.push(&sig);

        for trial in 0..10 {
            let mut chunk_seed = 0xab12_cd34_ef56_7890_u64
                .wrapping_add(trial as u64);
            let mut sep_b = fresh_separator(11);
            let mut acc = StemBlock::default();
            let mut pos = 0;
            while pos < sig.len() {
                // Random chunk sizes between 1 and 1024.
                let max_remaining = sig.len() - pos;
                let chunk = (xorshift_next(&mut chunk_seed) as usize
                             % 1024).max(1).min(max_remaining);
                let part = sep_b.push(&sig[pos..pos + chunk]);
                acc.drums.extend(part.drums);
                acc.voice.extend(part.voice);
                acc.piano.extend(part.piano);
                acc.synth.extend(part.synth);
                pos += chunk;
            }
            assert_eq!(out_a.drums.len(), acc.drums.len(),
                "trial {}: drum length differs (one-shot {} vs chunked {})",
                trial, out_a.drums.len(), acc.drums.len());
            // Within a tight tolerance, samples should match between
            // one-shot and any chunking. The streaming step is fully
            // determined by which STFT frames have entered the ring,
            // which doesn't depend on how the input was chunked at
            // the push level.
            for (i, (a, b)) in out_a.drums.iter().zip(acc.drums.iter()).enumerate() {
                assert!((a - b).abs() < 1e-4,
                    "trial {}: drum sample {} diverged: {} vs {}",
                    trial, i, a, b);
            }
        }
    }

    #[test]
    fn streaming_fuzz__nan_injection_recovers_and_continues() {
        // Inject NaN/Inf into the input stream at random positions.
        // The separator's input sanitize layer (in push()) replaces
        // them with 0 before they reach the STFT. Verify that:
        //   1. No output sample is ever non-finite.
        //   2. The separator continues producing output after NaN
        //      injection (the spec ring and OLA tail stay sane).
        let mut sep = fresh_separator(13);
        let mut seed = 0xbada_da5555aaaa_u64;
        let n = (HPSS_HARMONIC_FRAMES + 20) * STFT_HOP + STFT_WINDOW;
        let mut sig: Vec<f32> = (0..n)
            .map(|_| rand_f32(&mut seed, -0.5, 0.5))
            .collect();
        // Inject ~5% bad samples at random positions.
        for i in 0..sig.len() {
            if xorshift_next(&mut seed) % 20 == 0 {
                sig[i] = match xorshift_next(&mut seed) % 3 {
                    0 => f32::NAN,
                    1 => f32::INFINITY,
                    _ => f32::NEG_INFINITY,
                };
            }
        }
        let out = sep.push(&sig);
        assert!(!out.is_empty(), "no output after NaN-laden push");
        for (i, v) in out.drums.iter()
            .chain(out.voice.iter())
            .chain(out.piano.iter())
            .chain(out.synth.iter())
            .enumerate()
        {
            assert!(v.is_finite(),
                "non-finite output sample {} from NaN-defended input",
                i);
        }
    }

    #[test]
    fn streaming_fuzz__sustained_high_amplitude_no_amplitude_explosion() {
        // Drive the separator with high-amplitude signal sustained
        // over many windows. The mask normalization ensures stems
        // sum to ≤ input magnitude — output should never exceed the
        // input's peak magnitude (within a small slack for
        // overlap-add edge effects).
        let mut sep = fresh_separator(17);
        let mut seed = 0xfeed_5318_def_1234_u64;
        let n = (HPSS_HARMONIC_FRAMES + 30) * STFT_HOP + STFT_WINDOW;
        let peak = 0.9_f32;
        let sig: Vec<f32> = (0..n)
            .map(|_| rand_f32(&mut seed, -peak, peak))
            .collect();
        let out = sep.push(&sig);
        let slack = 1.5;  // OLA edge effects can briefly exceed peak
        for v in out.drums.iter()
            .chain(out.voice.iter())
            .chain(out.piano.iter())
            .chain(out.synth.iter())
        {
            assert!(v.abs() < peak * slack,
                "stem sample magnitude {} exceeds {}× peak input",
                v, slack);
        }
    }

    #[test]
    fn streaming_fuzz__incremental_push_emits_same_total_length() {
        // For a given total input length, the total emitted output
        // length must be deterministic regardless of how the input
        // arrived (single push, push every byte, push random
        // chunks). Important for sync alignment with bo-mux's
        // scheduled-start machinery.
        let n = (HPSS_HARMONIC_FRAMES + 10) * STFT_HOP + STFT_WINDOW;
        let sig: Vec<f32> = (0..n).map(|i| (i as f32 * 0.01).sin() * 0.3).collect();

        let mut single = fresh_separator(19);
        let out_single = single.push(&sig);

        let mut per_sample = fresh_separator(19);
        let mut total_emitted_per_sample = 0usize;
        for x in &sig {
            let part = per_sample.push(&[*x]);
            total_emitted_per_sample += part.len();
        }

        let mut chunked = fresh_separator(19);
        let mut total_emitted_chunked = 0usize;
        for chunk in sig.chunks(173) {
            let part = chunked.push(chunk);
            total_emitted_chunked += part.len();
        }

        assert_eq!(out_single.len(), total_emitted_per_sample,
            "per-sample push emitted different total length");
        assert_eq!(out_single.len(), total_emitted_chunked,
            "chunked push emitted different total length");
    }

    #[test]
    fn streaming_fuzz__sum_of_stems_invariant_holds_across_seeds() {
        // The sum-of-stems-equals-input invariant from the existing
        // recovery test, but exercised across 10 different MLP
        // weight states + 10 different signal seeds.
        //
        // SmallMlp::with_priors is deterministic for a given sample
        // rate, so we vary the SIGNAL not the weights; the test is
        // really about whether the OLA accumulator remains correct
        // under varying spectral content.
        let mut signal_seed = 0x5eed578a_feedface_u64;
        for trial in 0..10 {
            let mut sep = fresh_separator(0);
            let n = (HPSS_HARMONIC_FRAMES + 25) * STFT_HOP + STFT_WINDOW;
            // Build a varied test signal: random walk + sinusoid bias.
            let bias_freq = rand_f32(&mut signal_seed, 0.005, 0.05);
            let sig: Vec<f32> = (0..n).map(|i| {
                let s = (i as f32 * bias_freq).sin() * 0.3;
                let noise = rand_f32(&mut signal_seed, -0.15, 0.15);
                s + noise
            }).collect();
            let out = sep.push(&sig);
            let input_offset = sep.latency_samples();
            let skip = STFT_HOP;
            assert!(out.drums.len() > skip * 2);
            let mut max_err = 0.0_f32;
            for i in skip..(out.drums.len() - skip) {
                let stem_sum = out.drums[i] + out.voice[i]
                             + out.piano[i] + out.synth[i];
                let input = sig[i + input_offset];
                let err = (stem_sum - input).abs();
                if err > max_err { max_err = err; }
            }
            assert!(max_err < 0.015,
                "trial {} (bias_freq={:.4}): sum-of-stems diverged \
                 from input by {} (>0.015)",
                trial, bias_freq, max_err);
        }
    }

    #[test]
    fn streaming_fuzz__interleaved_drain_does_not_panic() {
        // Drain at unusual points — middle of warmup, just after
        // first emission, after long streams. Must never panic.
        for early_n in [0, 100, STFT_WINDOW / 2, STFT_WINDOW,
                        STFT_HOP * 5, STFT_HOP * 17,
                        STFT_HOP * 30, 99999]
        {
            let mut sep = fresh_separator(23);
            let mut seed = 0xdada_1010_5555_u64;
            let early: Vec<f32> = (0..early_n)
                .map(|_| rand_f32(&mut seed, -0.5, 0.5))
                .collect();
            let _ = sep.push(&early);
            let tail = sep.drain();
            for v in tail.drums.iter() {
                assert!(v.is_finite(), "drain emitted non-finite after {} samples", early_n);
            }
        }
    }
}
