// src/podcast_diar.rs
#![allow(dead_code)]
//
// Speaker diarization for podcasts: "who spoke when."
//
// Pipeline:
//
//   PCM mono (any sample rate)
//     │
//     ▼  resample to 16 kHz (typical speech rate)
//     │
//     ▼  windowed energy + zero-crossing rate → VAD mask
//     │
//     ▼  MFCC per 25 ms / 10 ms hop frame → 13 coefs / frame
//     │
//     ▼  segment-level pooling: avg MFCC over consecutive
//        voiced windows separated by ≥300 ms silence
//     │
//     ▼  online k-means clustering of segment embeddings
//        — first segment seeds the first cluster
//        — subsequent segments either join nearest cluster
//          (cosine distance < threshold) or start a new one
//        — cluster count caps at MAX_SPEAKERS (default 6)
//
// Output: Vec<DiarSegment { start_s, end_s, speaker_id }>.
//
// What this can and can't do:
//
//   - GOOD AT: distinguishing 2–4 speakers in clean podcast audio
//     where each speaker has a moderately distinct timbre.
//   - WEAK AT: distinguishing speakers with very similar vocal
//     timbre (e.g. siblings); short interjections that don't fill a
//     full segment; heavy background music or overlapping speech.
//   - DOESN'T DO: speaker IDENTIFICATION ("this is Alice") — only
//     diarization ("speaker A, then B, then A again"). Linking the
//     two requires user-supplied reference audio per known speaker.
//
// Compared to a deep-learning approach (X-vectors, ECAPA-TDNN, etc.),
// this MFCC-clustering pipeline is much simpler and runs on a phone
// in ~1/10th realtime, at the cost of 5-15% lower diarization
// accuracy on hard cases. For "label segments in my podcast" use,
// it's plenty.

use crate::looper::compute_mfcc;

/// Working sample rate. Everything internal is at 16 kHz — standard
/// for telephone-bandwidth speech and what most VAD/MFCC pipelines
/// assume. Audio at other rates is resampled before any analysis.
pub const TARGET_SR: u32 = 16_000;

/// MFCC analysis frame size in samples (25 ms at 16 kHz). Standard
/// for speech; matches what looper::compute_mfcc expects.
pub const FRAME_SAMPLES: usize = 400;

/// MFCC frame hop in samples (10 ms at 16 kHz). Also standard.
pub const HOP_SAMPLES: usize = 160;

/// Number of MFCC coefficients per frame (matches looper::MFCC_N).
pub const MFCC_DIM: usize = 13;

/// Minimum continuous voiced duration to consider a segment (ms).
/// Anything shorter is dropped as noise / interjection / breath.
pub const MIN_SEGMENT_MS: u32 = 300;

/// Silence threshold between segments (ms). Voiced regions separated
/// by less than this are merged into one segment; longer gaps start
/// a new segment.
pub const SILENCE_GAP_MS: u32 = 300;

/// Cap on distinct speaker clusters. 6 is enough for almost any
/// podcast format; episodes with more speakers are rare and we'd
/// rather under-segment than spray noise across 20 clusters.
pub const MAX_SPEAKERS: usize = 6;

/// Cosine-distance threshold for joining an existing cluster. Lower
/// = more clusters (each speaker fine-grained); higher = fewer
/// clusters (more grouping). 0.4 is a sweet spot for podcast audio.
pub const CLUSTER_JOIN_THRESHOLD: f32 = 0.4;

/// Voice activity detection threshold, expressed as a multiple of
/// the rolling noise floor. Frames whose RMS exceeds NOISE × this
/// factor are considered voiced.
pub const VAD_RMS_MULT: f32 = 2.5;

/// Minimum zero-crossing rate for voiced frames. Reject hum/static
/// (which has very low ZCR) and very high-frequency noise (which
/// has very high ZCR).
pub const VAD_ZCR_MIN: f32 = 0.01;
pub const VAD_ZCR_MAX: f32 = 0.35;

/// Linear resampler. Cheap and correct enough for 16 kHz-target
/// speech analysis (we don't need a brick-wall lowpass). For audio
/// already at 16 kHz this is a no-op.
pub fn resample_linear(signal: &[f32], src_sr: u32, dst_sr: u32) -> Vec<f32> {
    if src_sr == dst_sr || signal.is_empty() { return signal.to_vec(); }
    let ratio = dst_sr as f32 / src_sr as f32;
    let new_len = (signal.len() as f32 * ratio) as usize;
    let mut out = Vec::with_capacity(new_len);
    for i in 0..new_len {
        let src_pos = i as f32 / ratio;
        let lo = src_pos.floor() as usize;
        let hi = (lo + 1).min(signal.len() - 1);
        let t = src_pos - lo as f32;
        out.push(signal[lo] * (1.0 - t) + signal[hi] * t);
    }
    out
}

/// RMS of a frame.
fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() { return 0.0; }
    let sum: f32 = frame.iter().map(|&s| s * s).sum();
    (sum / frame.len() as f32).sqrt()
}

/// Zero-crossing rate of a frame: fraction of consecutive samples
/// that have opposite signs. Voiced speech sits in a moderate range;
/// hum and quiet noise have very low ZCR, sibilants very high.
fn zero_crossing_rate(frame: &[f32]) -> f32 {
    if frame.len() < 2 { return 0.0; }
    let mut crossings = 0;
    for i in 1..frame.len() {
        if (frame[i] >= 0.0) != (frame[i - 1] >= 0.0) { crossings += 1; }
    }
    crossings as f32 / (frame.len() - 1) as f32
}

/// Per-frame VAD decision. `noise_rms` is a rolling estimate of the
/// quiet background; voiced is anything substantially above that
/// AND with a plausible ZCR.
pub fn frame_is_voiced(frame: &[f32], noise_rms: f32) -> bool {
    let frame_rms = rms(frame);
    let zcr = zero_crossing_rate(frame);
    frame_rms > noise_rms.max(1e-4) * VAD_RMS_MULT
        && (VAD_ZCR_MIN..=VAD_ZCR_MAX).contains(&zcr)
}

/// Compute a VAD mask over the whole signal at the analysis hop.
/// Returns a Vec<bool> with one entry per hop frame.
///
/// The noise floor is estimated as the 10th-percentile RMS across
/// the entire recording. This is more robust than a first-second
/// median, which can fail when:
///   - the recording starts inside a sentence
///   - background noise is intermittent rather than constant
///   - the user trimmed silence off the head before importing
///
/// The 10th percentile assumes at least ~10% of the recording is
/// quiet, which holds for almost any podcast (room tone between
/// utterances, breaths between sentences, intro/outro silence).
pub fn voice_activity_mask(signal: &[f32]) -> Vec<bool> {
    if signal.len() < FRAME_SAMPLES { return Vec::new(); }
    let n_frames = (signal.len() - FRAME_SAMPLES) / HOP_SAMPLES + 1;
    // Compute all-frame RMS.
    let all_rms: Vec<f32> = (0..n_frames).map(|f| {
        let start = f * HOP_SAMPLES;
        rms(&signal[start..start + FRAME_SAMPLES])
    }).collect();
    // 10th percentile = noise floor.
    let mut sorted = all_rms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let noise = sorted.get(sorted.len() / 10).copied().unwrap_or(1e-4);
    // Frame-by-frame decisions.
    (0..n_frames).map(|f| {
        let start = f * HOP_SAMPLES;
        frame_is_voiced(&signal[start..start + FRAME_SAMPLES], noise)
    }).collect()
}

/// One detected speech segment.
#[derive(Debug, Clone, PartialEq)]
pub struct DiarSegment {
    /// Start time in seconds (of the resampled 16 kHz audio).
    pub start_s: f32,
    /// End time in seconds (exclusive).
    pub end_s: f32,
    /// Cluster id (0-indexed). Same id = same speaker across the
    /// whole recording, per the clustering's belief.
    pub speaker_id: u32,
}

/// Group a VAD mask into continuous voiced segments. Voiced runs
/// shorter than `min_ms` are dropped (likely noise); silent gaps
/// shorter than `gap_ms` are bridged (within-segment pauses).
pub fn vad_to_segments(
    mask: &[bool], min_ms: u32, gap_ms: u32,
) -> Vec<(f32, f32)> {
    let frame_ms = (HOP_SAMPLES as f32 / TARGET_SR as f32) * 1000.0;
    let min_frames = ((min_ms as f32) / frame_ms) as usize;
    let gap_frames = ((gap_ms as f32) / frame_ms) as usize;
    let mut out = Vec::new();
    let mut in_voiced = false;
    let mut seg_start = 0usize;
    let mut silence_run = 0usize;
    for (i, &v) in mask.iter().enumerate() {
        if v {
            if !in_voiced { seg_start = i; in_voiced = true; }
            silence_run = 0;
        } else if in_voiced {
            silence_run += 1;
            if silence_run > gap_frames {
                let seg_end = i - silence_run;
                if seg_end >= seg_start + min_frames {
                    out.push((
                        seg_start as f32 * HOP_SAMPLES as f32 / TARGET_SR as f32,
                        seg_end as f32 * HOP_SAMPLES as f32 / TARGET_SR as f32,
                    ));
                }
                in_voiced = false;
                silence_run = 0;
            }
        }
    }
    if in_voiced && mask.len() >= seg_start + min_frames {
        out.push((
            seg_start as f32 * HOP_SAMPLES as f32 / TARGET_SR as f32,
            mask.len() as f32 * HOP_SAMPLES as f32 / TARGET_SR as f32,
        ));
    }
    out
}

/// Pool MFCC frames across a segment into one embedding vector.
/// Mean over the segment's frames; we drop the first coefficient
/// (already excluded in looper::compute_mfcc).
pub fn segment_embedding(signal: &[f32], start_s: f32, end_s: f32) -> [f32; MFCC_DIM] {
    let start_sample = (start_s * TARGET_SR as f32) as usize;
    let end_sample = (end_s * TARGET_SR as f32) as usize;
    let mut sum = [0.0_f32; MFCC_DIM];
    let mut n_frames = 0;
    let mut start = start_sample;
    while start + FRAME_SAMPLES <= end_sample && start + FRAME_SAMPLES <= signal.len() {
        let mf = compute_mfcc(&signal[start..start + FRAME_SAMPLES], TARGET_SR);
        for (acc, m) in sum.iter_mut().zip(mf.iter()) { *acc += *m; }
        n_frames += 1;
        start += HOP_SAMPLES;
    }
    if n_frames > 0 {
        for v in sum.iter_mut() { *v /= n_frames as f32; }
    }
    sum
}

/// Cosine distance between two embedding vectors. 0 = identical
/// direction, 2 = opposite direction, 1 = orthogonal.
pub fn cosine_distance(a: &[f32; MFCC_DIM], b: &[f32; MFCC_DIM]) -> f32 {
    let mut dot = 0.0_f32;
    let mut na = 0.0_f32;
    let mut nb = 0.0_f32;
    for i in 0..MFCC_DIM {
        dot += a[i] * b[i];
        na  += a[i] * a[i];
        nb  += b[i] * b[i];
    }
    let denom = (na * nb).sqrt() + 1e-9;
    1.0 - dot / denom
}

/// Online k-means clustering. Each new embedding either joins the
/// nearest existing cluster (if cosine distance < threshold) or
/// starts a new one (up to MAX_SPEAKERS).
///
/// Joining updates the cluster centroid with a running mean (weighted
/// by sample count). This lets the centroid drift slightly to absorb
/// within-speaker variability without losing identity.
#[derive(Debug, Default)]
pub struct OnlineClusters {
    centroids: Vec<[f32; MFCC_DIM]>,
    counts: Vec<usize>,
}

impl OnlineClusters {
    pub fn new() -> Self { Self::default() }

    /// Add an embedding and return its assigned cluster id.
    pub fn assign(&mut self, emb: &[f32; MFCC_DIM]) -> u32 {
        if self.centroids.is_empty() {
            self.centroids.push(*emb);
            self.counts.push(1);
            return 0;
        }
        // Find nearest existing cluster.
        let (best_idx, best_dist) = self.centroids.iter().enumerate()
            .map(|(i, c)| (i, cosine_distance(emb, c)))
            .fold((0_usize, f32::INFINITY), |(bi, bd), (i, d)| {
                if d < bd { (i, d) } else { (bi, bd) }
            });
        if best_dist < CLUSTER_JOIN_THRESHOLD || self.centroids.len() >= MAX_SPEAKERS {
            // Join nearest: running-mean update.
            let n = self.counts[best_idx] as f32;
            for i in 0..MFCC_DIM {
                self.centroids[best_idx][i] =
                    (self.centroids[best_idx][i] * n + emb[i]) / (n + 1.0);
            }
            self.counts[best_idx] += 1;
            best_idx as u32
        } else {
            // New cluster.
            self.centroids.push(*emb);
            self.counts.push(1);
            (self.centroids.len() - 1) as u32
        }
    }

    pub fn cluster_count(&self) -> usize { self.centroids.len() }
    pub fn count_of(&self, id: u32) -> Option<usize> {
        self.counts.get(id as usize).copied()
    }
}

/// Full diarization. Pass mono PCM at any sample rate; returns a
/// list of (start, end, speaker_id) segments. Internally resamples
/// to 16 kHz, computes VAD, segments, embeds, clusters.
///
/// Note: returned timestamps are relative to the RESAMPLED audio.
/// For source-rate audio at src_sr, the timestamps are the same in
/// wall-clock seconds (resampling preserves duration).
pub fn diarize(signal: &[f32], src_sr: u32) -> Vec<DiarSegment> {
    let rs = resample_linear(signal, src_sr, TARGET_SR);
    let mask = voice_activity_mask(&rs);
    let raw_segs = vad_to_segments(&mask, MIN_SEGMENT_MS, SILENCE_GAP_MS);
    let mut clusters = OnlineClusters::new();
    let mut out = Vec::with_capacity(raw_segs.len());
    for (s, e) in raw_segs {
        let emb = segment_embedding(&rs, s, e);
        let id = clusters.assign(&emb);
        out.push(DiarSegment { start_s: s, end_s: e, speaker_id: id });
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    /// Synth: pure tone at given freq, given duration, given amplitude.
    /// Used to build test signals with known properties.
    fn tone(freq: f32, duration_s: f32, amp: f32, sr: u32) -> Vec<f32> {
        let n = (duration_s * sr as f32) as usize;
        (0..n).map(|i| {
            (2.0 * PI * freq * i as f32 / sr as f32).sin() * amp
        }).collect()
    }

    fn silence(duration_s: f32, sr: u32) -> Vec<f32> {
        vec![0.0_f32; (duration_s * sr as f32) as usize]
    }

    #[test]
    fn resample_linear_same_rate_is_identity() {
        let sig = vec![0.1, 0.2, 0.3, 0.4, 0.5];
        let out = resample_linear(&sig, 16000, 16000);
        assert_eq!(out, sig);
    }

    #[test]
    fn resample_linear_doubles_length_at_2x_rate() {
        let sig = vec![0.0_f32; 100];
        let out = resample_linear(&sig, 8000, 16000);
        assert_eq!(out.len(), 200);
    }

    #[test]
    fn resample_linear_halves_length_at_half_rate() {
        let sig = vec![0.0_f32; 100];
        let out = resample_linear(&sig, 16000, 8000);
        assert_eq!(out.len(), 50);
    }

    #[test]
    fn resample_linear_empty_input_returns_empty() {
        let out = resample_linear(&[], 44100, 16000);
        assert!(out.is_empty());
    }

    #[test]
    fn rms_zero_for_silence() {
        assert_eq!(rms(&[0.0; 100]), 0.0);
    }

    #[test]
    fn rms_correct_for_constant() {
        assert!((rms(&[0.5; 100]) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn zcr_zero_for_constant() {
        assert_eq!(zero_crossing_rate(&[0.5; 100]), 0.0);
    }

    #[test]
    fn zcr_high_for_alternating() {
        // [+, -, +, -, ...]: every consecutive pair crosses.
        let sig: Vec<f32> = (0..100).map(|i| if i % 2 == 0 { 1.0 } else { -1.0 }).collect();
        let zcr = zero_crossing_rate(&sig);
        assert!(zcr > 0.95, "ZCR for fully alternating should be ~1.0, got {}", zcr);
    }

    #[test]
    fn vad_silence_is_not_voiced() {
        let frame = vec![0.0_f32; FRAME_SAMPLES];
        assert!(!frame_is_voiced(&frame, 0.001));
    }

    #[test]
    fn vad_loud_tone_is_voiced() {
        // 200 Hz tone — well within the voiced ZCR range.
        let frame: Vec<f32> = (0..FRAME_SAMPLES).map(|i| {
            (2.0 * PI * 200.0 * i as f32 / TARGET_SR as f32).sin() * 0.5
        }).collect();
        assert!(frame_is_voiced(&frame, 0.001));
    }

    #[test]
    fn vad_pure_dc_is_not_voiced() {
        // DC offset → ZCR = 0 → rejected even if loud.
        let frame = vec![0.5_f32; FRAME_SAMPLES];
        assert!(!frame_is_voiced(&frame, 0.001));
    }

    #[test]
    fn vad_mask_detects_speech_after_silence() {
        // 0.5 s of silence then 1 s of 200 Hz "speech."
        let mut sig = silence(0.5, TARGET_SR);
        sig.extend(tone(200.0, 1.0, 0.4, TARGET_SR));
        let mask = voice_activity_mask(&sig);
        // Mid of the silence — should be unvoiced.
        let mid_silence = mask.len() / 5;   // ~early
        assert!(!mask[mid_silence],
                "frame {} in early silence should be unvoiced", mid_silence);
        // Mid of the tone — should be voiced.
        let mid_tone = mask.len() * 3 / 4;
        assert!(mask[mid_tone],
                "frame {} during tone should be voiced", mid_tone);
    }

    #[test]
    fn vad_to_segments_groups_continuous_voiced() {
        // 1000 frames: voiced [200..500), silent [500..900), voiced [900..1000)
        let mut mask = vec![false; 1000];
        for i in 200..500 { mask[i] = true; }
        for i in 900..1000 { mask[i] = true; }
        let segs = vad_to_segments(&mask, 100, 100);
        // Should find two segments.
        assert!(segs.len() >= 1, "got {} segments", segs.len());
        // First segment starts near 200 frames * 10ms = 2.0s
        assert!((segs[0].0 - 2.0).abs() < 0.1,
                "first segment start = {}", segs[0].0);
    }

    #[test]
    fn vad_to_segments_bridges_short_gaps() {
        // Voiced [200..400), silent [400..420) (gap of 200 ms), voiced [420..600)
        // — with gap_ms = 300 ms, gap should be bridged → one segment.
        let mut mask = vec![false; 700];
        for i in 200..400 { mask[i] = true; }
        for i in 420..600 { mask[i] = true; }
        let segs = vad_to_segments(&mask, 100, 300);
        assert_eq!(segs.len(), 1,
            "200ms gap should be bridged at gap_ms=300, got {} segs", segs.len());
    }

    #[test]
    fn vad_to_segments_drops_short_segments() {
        // 50ms voiced (too short to keep).
        let mut mask = vec![false; 200];
        for i in 50..55 { mask[i] = true; }
        let segs = vad_to_segments(&mask, 300, 300);
        assert!(segs.is_empty(), "50ms segment should be dropped, got {:?}", segs);
    }

    #[test]
    fn cosine_distance_self_is_zero() {
        let v = [1.0_f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0, 9.0, 10.0, 11.0, 12.0, 13.0];
        assert!(cosine_distance(&v, &v) < 1e-5);
    }

    #[test]
    fn cosine_distance_opposite_is_two() {
        let a = [1.0_f32; MFCC_DIM];
        let b = [-1.0_f32; MFCC_DIM];
        let d = cosine_distance(&a, &b);
        assert!((d - 2.0).abs() < 1e-5, "opposite vectors → 2.0, got {}", d);
    }

    #[test]
    fn online_clusters_first_assignment_is_zero() {
        let mut c = OnlineClusters::new();
        let e = [1.0_f32; MFCC_DIM];
        assert_eq!(c.assign(&e), 0);
        assert_eq!(c.cluster_count(), 1);
    }

    #[test]
    fn online_clusters_identical_inputs_same_cluster() {
        let mut c = OnlineClusters::new();
        let e = [1.0_f32; MFCC_DIM];
        assert_eq!(c.assign(&e), 0);
        assert_eq!(c.assign(&e), 0);
        assert_eq!(c.assign(&e), 0);
        assert_eq!(c.cluster_count(), 1);
        assert_eq!(c.count_of(0), Some(3));
    }

    #[test]
    fn online_clusters_orthogonal_inputs_get_separate_clusters() {
        let mut c = OnlineClusters::new();
        let mut a = [0.0_f32; MFCC_DIM]; a[0] = 1.0;
        let mut b = [0.0_f32; MFCC_DIM]; b[6] = 1.0;
        assert_eq!(c.assign(&a), 0);
        // Orthogonal → cosine distance = 1.0 ≥ threshold (0.4) → new cluster.
        assert_eq!(c.assign(&b), 1);
        assert_eq!(c.cluster_count(), 2);
    }

    #[test]
    fn online_clusters_caps_at_max_speakers() {
        let mut c = OnlineClusters::new();
        // Feed many orthogonal-ish vectors; cluster count should cap.
        for i in 0..MAX_SPEAKERS * 3 {
            let mut e = [0.0_f32; MFCC_DIM];
            // Use a position that varies enough to be far from prior centroids.
            e[i % MFCC_DIM] = 1.0;
            // Adding a small "stamp" to keep them unique.
            e[(i + 1) % MFCC_DIM] = (i as f32) * 0.01;
            c.assign(&e);
        }
        assert!(c.cluster_count() <= MAX_SPEAKERS,
                "cluster count {} exceeded MAX_SPEAKERS {}",
                c.cluster_count(), MAX_SPEAKERS);
    }

    #[test]
    fn diarize_silence_produces_no_segments() {
        let sig = silence(3.0, TARGET_SR);
        let segs = diarize(&sig, TARGET_SR);
        assert!(segs.is_empty(), "silence should produce no segments");
    }

    #[test]
    fn diarize_two_distinct_tones_finds_two_clusters() {
        // Build a signal alternating between a low tone (Speaker A)
        // and a high tone (Speaker B), each 1 s with 0.5 s silence
        // between. MFCCs of 150 Hz vs 800 Hz should be distinguishable.
        let mut sig = Vec::new();
        for _ in 0..2 {
            sig.extend(tone(150.0, 1.0, 0.4, TARGET_SR));
            sig.extend(silence(0.5, TARGET_SR));
            sig.extend(tone(800.0, 1.0, 0.4, TARGET_SR));
            sig.extend(silence(0.5, TARGET_SR));
        }
        let segs = diarize(&sig, TARGET_SR);
        assert!(segs.len() >= 2, "expected ≥2 segments, got {}", segs.len());
        // At least two distinct speaker_ids should appear.
        let mut ids: Vec<u32> = segs.iter().map(|s| s.speaker_id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert!(ids.len() >= 2,
                "expected ≥2 distinct speakers, got {} (ids = {:?})",
                ids.len(), ids);
    }

    #[test]
    fn diarize_segments_are_time_ordered() {
        let mut sig = Vec::new();
        sig.extend(tone(200.0, 0.5, 0.4, TARGET_SR));
        sig.extend(silence(0.5, TARGET_SR));
        sig.extend(tone(400.0, 0.5, 0.4, TARGET_SR));
        let segs = diarize(&sig, TARGET_SR);
        for w in segs.windows(2) {
            assert!(w[0].end_s <= w[1].start_s,
                "segments out of order: {:?} → {:?}", w[0], w[1]);
        }
    }

    #[test]
    fn diarize_handles_arbitrary_sample_rate() {
        // 22050 Hz input should be resampled to 16k internally without
        // crashing or producing nonsensical timestamps.
        let sig = tone(300.0, 1.0, 0.4, 22050);
        let segs = diarize(&sig, 22050);
        // A 1-second tone should yield one segment near 1 s long.
        if let Some(s) = segs.first() {
            assert!(s.end_s - s.start_s > 0.5,
                "expected segment ≥0.5s, got {:.2}s", s.end_s - s.start_s);
            assert!(s.end_s - s.start_s < 1.2,
                "expected segment ≤1.2s, got {:.2}s", s.end_s - s.start_s);
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
    fn fuzz_diarize_handles_arbitrary_random_audio() {
        // Random noise input across many seeds and lengths. Diarize
        // must NOT panic; segments must be time-ordered; speaker
        // ids must be in range [0, MAX_SPEAKERS).
        for seed in 0..20_u64 {
            let mut rng = prng(0xD1A1_F00D ^ seed);
            let secs = 1.0 + ((seed % 5) as f32);
            let n = (secs * TARGET_SR as f32) as usize;
            let sig: Vec<f32> = (0..n).map(|_| rng() * 0.3).collect();
            let segs = diarize(&sig, TARGET_SR);
            // Time-order.
            for w in segs.windows(2) {
                assert!(w[0].end_s <= w[1].start_s + 1e-3,
                    "seed {}: segments not time-ordered", seed);
            }
            // Speaker ids within bounds.
            for s in &segs {
                assert!((s.speaker_id as usize) < MAX_SPEAKERS,
                    "seed {}: speaker_id {} >= MAX_SPEAKERS",
                    seed, s.speaker_id);
                assert!(s.end_s > s.start_s,
                    "seed {}: segment with non-positive duration", seed);
            }
        }
    }

    #[test]
    fn fuzz_resample_never_drops_or_explodes_length() {
        // For any src/dst rate pair, output length should be
        // approximately len * (dst/src). No NaN/Inf in output.
        for seed in 0..50_u64 {
            let mut rng = prng(0x1234_ABCD ^ seed);
            let src_sr: u32 = [8000, 16000, 22050, 44100, 48000]
                [(seed % 5) as usize];
            let dst_sr: u32 = [8000, 16000, 22050, 44100, 48000]
                [((seed >> 3) % 5) as usize];
            let len = 100 + ((seed % 1000) as usize);
            let sig: Vec<f32> = (0..len).map(|_| rng()).collect();
            let out = resample_linear(&sig, src_sr, dst_sr);
            let expected = (len as f32 * dst_sr as f32 / src_sr as f32) as usize;
            // Allow ±2 sample slop from integer rounding.
            assert!((out.len() as i64 - expected as i64).abs() <= 2,
                "seed {}: {}→{} len={} → {} (expected ~{})",
                seed, src_sr, dst_sr, len, out.len(), expected);
            for &v in &out {
                assert!(v.is_finite(),
                    "seed {}: non-finite output {} at {}→{}",
                    seed, v, src_sr, dst_sr);
            }
        }
    }

    #[test]
    fn fuzz_online_clusters_never_exceeds_max_speakers() {
        // Feed 1000 random embeddings; cluster count must cap.
        for seed in 0..10_u64 {
            let mut rng = prng(0xC1F5_7E80 ^ seed);
            let mut clusters = OnlineClusters::new();
            for _ in 0..1000 {
                let mut emb = [0.0_f32; MFCC_DIM];
                for v in emb.iter_mut() { *v = rng(); }
                clusters.assign(&emb);
            }
            assert!(clusters.cluster_count() <= MAX_SPEAKERS,
                "seed {}: cluster count {} > MAX_SPEAKERS {}",
                seed, clusters.cluster_count(), MAX_SPEAKERS);
        }
    }

    #[test]
    fn fuzz_vad_silence_alone_produces_no_voiced_frames() {
        // Pure silence at any length → mask must be all-false.
        for len_s in [0.5, 1.0, 2.0, 5.0] {
            let sig = silence(len_s, TARGET_SR);
            let mask = voice_activity_mask(&sig);
            let voiced = mask.iter().filter(|&&v| v).count();
            assert_eq!(voiced, 0,
                "silence ({}s) produced {} voiced frames", len_s, voiced);
        }
    }
}
