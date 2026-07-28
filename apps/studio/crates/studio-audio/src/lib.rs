//! studio-audio — waveforms and mixdown.
//!
//! ## Peaks: the idea worth taking from Audacity
//!
//! Audacity does not read a file to draw it. Each `Sequence` carries summary
//! blocks — min/max/RMS at 256 samples per point and again at 65,536 — so
//! drawing a ten-hour recording zoomed out reads a few kilobytes instead of
//! gigabytes, and the zoom level picks which summary to use. That structure is
//! the reason it stays responsive on files that make other editors crawl, and
//! it is worth far more than any of its UI code.
//!
//! [`Peaks`] is that idea in a few dozen lines: a pyramid of min/max pairs,
//! each level a fixed factor coarser than the one below, with [`Peaks::for_px`]
//! choosing the level whose buckets are closest to one screen pixel. Min AND
//! max are kept (rather than an average or an absolute value) because a
//! waveform drawn from averages loses exactly the transients an editor is
//! looking for — a click, a clipped peak, the attack of a drum.
//!
//! ## Mixdown
//!
//! [`mixdown`] turns a stretch of timeline into interleaved samples: it walks
//! the clips that intersect the window, reads their source, applies clip gain
//! and the fades, and sums. It is deliberately a pure function of
//! (project, sources, window) so a render is reproducible and testable — no
//! device, no callback, no clock.

use std::collections::BTreeMap;
use studio_core::{Project, SourceId, Ticks, TrackKind};

/// Audio for one source, as mono-per-channel planes.
///
/// Real playback will stream these from disk; the model does not care, which is
/// why decode lives outside this crate. Tests build them by hand.
#[derive(Debug, Clone, Default)]
pub struct SourceAudio {
    pub sample_rate: u32,
    /// One vector per channel, all the same length.
    pub channels: Vec<Vec<f32>>,
}

impl SourceAudio {
    pub fn frames(&self) -> usize { self.channels.first().map(|c| c.len()).unwrap_or(0) }

    /// Mono sum, for peak computation and for mixing a multi-channel source
    /// into a mono bus. Averaged, not summed, so a stereo file does not come
    /// out 6 dB hotter than a mono one.
    pub fn mono(&self, frame: usize) -> f32 {
        if self.channels.is_empty() { return 0.0; }
        let n = self.channels.len() as f32;
        self.channels.iter().map(|c| c.get(frame).copied().unwrap_or(0.0)).sum::<f32>() / n
    }
}

/// One bucket of a waveform: the extremes of the samples it covers.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MinMax {
    pub min: f32,
    pub max: f32,
}

impl MinMax {
    pub fn merge(self, o: MinMax) -> MinMax {
        MinMax { min: self.min.min(o.min), max: self.max.max(o.max) }
    }
    /// Peak-to-peak height — what actually gets drawn.
    pub fn span(&self) -> f32 { self.max - self.min }
}

/// A multi-resolution peak pyramid. Level 0 is the finest.
#[derive(Debug, Clone)]
pub struct Peaks {
    /// Samples covered by one bucket at level 0.
    pub base: usize,
    /// How much coarser each level is than the one below.
    pub factor: usize,
    pub levels: Vec<Vec<MinMax>>,
    pub frames: usize,
}

impl Peaks {
    /// Build the pyramid. `base` is Audacity's 256; `factor` of 256 gives its
    /// second summary at 65,536 in one more level.
    pub fn build(audio: &SourceAudio, base: usize, factor: usize) -> Peaks {
        let base = base.max(1);
        let factor = factor.max(2);
        let frames = audio.frames();
        let mut level0 = Vec::with_capacity(frames.div_ceil(base));
        let mut i = 0;
        while i < frames {
            let end = (i + base).min(frames);
            let mut mm = MinMax { min: f32::INFINITY, max: f32::NEG_INFINITY };
            for f in i..end {
                let v = audio.mono(f);
                mm.min = mm.min.min(v);
                mm.max = mm.max.max(v);
            }
            level0.push(mm);
            i = end;
        }
        let mut levels = vec![level0];
        // Each level is built from the one below, not from the samples again —
        // an O(n) pyramid rather than O(n log n), and the reason building peaks
        // for a long file is cheap.
        while levels.last().map(|l| l.len()).unwrap_or(0) > 1 {
            let prev = levels.last().unwrap();
            let mut next = Vec::with_capacity(prev.len().div_ceil(factor));
            let mut j = 0;
            while j < prev.len() {
                let end = (j + factor).min(prev.len());
                let mut mm = prev[j];
                for b in &prev[j + 1..end] { mm = mm.merge(*b); }
                next.push(mm);
                j = end;
            }
            levels.push(next);
        }
        Peaks { base, factor, levels, frames }
    }

    /// Samples covered by one bucket at `level`.
    pub fn samples_per_bucket(&self, level: usize) -> usize {
        self.base * self.factor.pow(level as u32)
    }

    /// The level whose buckets are closest to (but not finer than) one pixel
    /// covering `samples_per_px`. Picking a level that is too fine wastes work;
    /// too coarse and the waveform visibly steps.
    pub fn for_px(&self, samples_per_px: f64) -> usize {
        let mut best = 0;
        for l in 0..self.levels.len() {
            if (self.samples_per_bucket(l) as f64) <= samples_per_px.max(1.0) {
                best = l;
            } else {
                break;
            }
        }
        best
    }

    /// Peaks for a span of samples, reduced to `columns` buckets — exactly what
    /// a waveform widget needs for a rectangle `columns` pixels wide.
    pub fn window(&self, from: usize, to: usize, columns: usize) -> Vec<MinMax> {
        let (from, to) = (from.min(self.frames), to.min(self.frames));
        if columns == 0 || to <= from { return Vec::new(); }
        let spp = (to - from) as f64 / columns as f64;
        let level = self.for_px(spp);
        let buckets = &self.levels[level];
        let per = self.samples_per_bucket(level);
        (0..columns)
            .map(|c| {
                let s0 = from + (c as f64 * spp) as usize;
                let s1 = from + (((c + 1) as f64 * spp) as usize).max(1);
                let b0 = s0 / per;
                let b1 = ((s1 + per - 1) / per).max(b0 + 1).min(buckets.len());
                buckets[b0.min(buckets.len().saturating_sub(1))..b1]
                    .iter()
                    .copied()
                    .fold(MinMax { min: 0.0, max: 0.0 }, MinMax::merge)
            })
            .collect()
    }
}

/// A fade on a clip, in ticks from its own start/end.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Fades {
    pub fade_in: Ticks,
    pub fade_out: Ticks,
}

impl Fades {
    /// Gain multiplier at `offset` into a clip of length `len`. Linear, which is
    /// what both source editors default to; the curve is where an equal-power
    /// option would go.
    pub fn gain_at(&self, offset: Ticks, len: Ticks) -> f32 {
        let mut g = 1.0f32;
        if self.fade_in.0 > 0 && offset.0 < self.fade_in.0 {
            g *= offset.0 as f32 / self.fade_in.0 as f32;
        }
        let from_end = len.0 - offset.0;
        if self.fade_out.0 > 0 && from_end < self.fade_out.0 {
            g *= (from_end.max(0) as f32) / self.fade_out.0 as f32;
        }
        g.clamp(0.0, 1.0)
    }
}

/// Render `[from, to)` of the timeline to interleaved stereo at `rate`.
///
/// Pure: same inputs, same samples, every time. Muted tracks contribute
/// nothing; when any track is soloed, only soloed tracks are heard — the rule
/// every DAW shares and every reimplementation gets backwards at least once.
pub fn mixdown(
    project: &Project,
    audio: &BTreeMap<SourceId, SourceAudio>,
    fades: &BTreeMap<studio_core::ClipId, Fades>,
    from: Ticks,
    to: Ticks,
    rate: u32,
    solo: &[studio_core::TrackId],
) -> Vec<f32> {
    let frames = (to.to_samples(rate) - from.to_samples(rate)).max(0) as usize;
    let mut out = vec![0.0f32; frames * 2];
    for track in &project.tracks {
        if track.kind != TrackKind::Audio && !project.clips.values().any(|c| c.track == track.id) {
            continue;
        }
        if track.muted { continue; }
        if !solo.is_empty() && !solo.contains(&track.id) { continue; }

        for clip in project.track_clips(track.id) {
            // skip clips entirely outside the window
            if clip.end() <= from || clip.pos >= to { continue; }
            let Some(src) = audio.get(&clip.source) else { continue };
            if src.channels.is_empty() { continue; }
            let fade = fades.get(&clip.id).copied().unwrap_or_default();

            let start = clip.pos.max(from);
            let stop = clip.end().min(to);
            let out_off = (start.to_samples(rate) - from.to_samples(rate)).max(0) as usize;
            let n = (stop.to_samples(rate) - start.to_samples(rate)).max(0) as usize;
            for i in 0..n {
                let at = Ticks::from_samples(start.to_samples(rate) + i as i64, rate);
                let into_clip = at.saturating_sub(clip.pos);
                let src_frame = (clip.src_in.saturating_add(into_clip)).to_samples(rate) as usize;
                if src_frame >= src.frames() { break; }
                let g = clip.gain * fade.gain_at(into_clip, clip.len);
                let oi = (out_off + i) * 2;
                if oi + 1 >= out.len() { break; }
                // a mono source feeds both sides; a stereo one keeps its image
                let (l, r) = if src.channels.len() >= 2 {
                    (src.channels[0][src_frame], src.channels[1][src_frame])
                } else {
                    let v = src.channels[0][src_frame];
                    (v, v)
                };
                out[oi] += l * g;
                out[oi + 1] += r * g;
            }
        }
    }
    out
}

/// Peak level of a rendered buffer, in dBFS. `-inf` for silence — reported as
/// f32::NEG_INFINITY rather than a magic -100.0, so a caller cannot mistake
/// "very quiet" for "nothing".
pub fn peak_dbfs(samples: &[f32]) -> f32 {
    let peak = samples.iter().fold(0.0f32, |a, s| a.max(s.abs()));
    if peak <= 0.0 { f32::NEG_INFINITY } else { 20.0 * peak.log10() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{Clip, ClipId, Op, Source, Track, TrackId};

    fn tone(frames: usize, rate: u32, hz: f32, amp: f32) -> SourceAudio {
        let ch: Vec<f32> = (0..frames)
            .map(|i| (i as f32 / rate as f32 * hz * std::f32::consts::TAU).sin() * amp)
            .collect();
        SourceAudio { sample_rate: rate, channels: vec![ch] }
    }

    fn project_with(len_s: f64, gain: f32) -> (Project, BTreeMap<SourceId, SourceAudio>) {
        let rate = 48_000;
        let mut p = Project { name: "mix".into(), ..Default::default() };
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio,
            name: "A1".into(), muted: false, locked: false }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "t.wav".into(),
            len: Ticks::from_seconds(len_s), sample_rate: rate, channels: 1, has_video: false } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1),
            pos: Ticks::ZERO, src_in: Ticks::ZERO, len: Ticks::from_seconds(len_s),
            gain, effects: Vec::new() } }).unwrap();
        let mut audio = BTreeMap::new();
        audio.insert(SourceId(1), tone((len_s * rate as f64) as usize, rate, 440.0, 0.5));
        (p, audio)
    }

    #[test]
    fn peaks_match_a_brute_force_scan() {
        // The pyramid is only useful if it tells the truth: every level must
        // agree with reading the samples directly.
        let a = tone(50_000, 48_000, 300.0, 0.8);
        let p = Peaks::build(&a, 256, 4);
        assert!(p.levels.len() > 3, "a pyramid, not one flat level");
        for level in 0..p.levels.len() {
            let per = p.samples_per_bucket(level);
            for (bi, bucket) in p.levels[level].iter().enumerate() {
                let s0 = bi * per;
                let s1 = ((bi + 1) * per).min(a.frames());
                if s0 >= s1 { continue; }
                let (mut mn, mut mx) = (f32::INFINITY, f32::NEG_INFINITY);
                for f in s0..s1 { let v = a.mono(f); mn = mn.min(v); mx = mx.max(v); }
                assert!((bucket.min - mn).abs() < 1e-6, "level {level} bucket {bi} min");
                assert!((bucket.max - mx).abs() < 1e-6, "level {level} bucket {bi} max");
            }
        }
    }

    #[test]
    fn min_and_max_are_kept_so_transients_survive_zooming_out() {
        // A waveform drawn from averages loses the click you were looking for.
        // One spike in a quiet file must still be visible at the coarsest level.
        let mut a = tone(100_000, 48_000, 200.0, 0.02);
        a.channels[0][61_234] = 0.97;
        let p = Peaks::build(&a, 256, 8);
        let top = p.levels.last().unwrap();
        assert!(top.iter().any(|b| b.max > 0.9), "the transient survives to the top of the pyramid");
        // and it is visible in a 40-column render of the whole file
        let cols = p.window(0, a.frames(), 40);
        assert_eq!(cols.len(), 40);
        assert!(cols.iter().any(|c| c.max > 0.9), "and in a zoomed-out draw");
    }

    #[test]
    fn the_level_chosen_tracks_the_zoom() {
        let a = tone(500_000, 48_000, 100.0, 0.5);
        let p = Peaks::build(&a, 256, 4);
        // zoomed right in (a handful of samples per pixel) → the finest level
        assert_eq!(p.for_px(4.0), 0);
        // one bucket per pixel → level 0 exactly
        assert_eq!(p.for_px(256.0), 0);
        // four buckets per pixel → one level up
        assert_eq!(p.for_px(1024.0), 1);
        // absurdly far out → coarsest available, not a panic
        let far = p.for_px(1e12);
        assert_eq!(far, p.levels.len() - 1);
    }

    #[test]
    fn silence_in_gives_silence_out() {
        let (p, mut audio) = project_with(1.0, 1.0);
        audio.insert(SourceId(1), SourceAudio { sample_rate: 48_000, channels: vec![vec![0.0; 48_000]] });
        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(1.0), 48_000, &[]);
        assert_eq!(out.len(), 48_000 * 2, "stereo interleaved, one second");
        assert!(out.iter().all(|s| *s == 0.0));
        assert_eq!(peak_dbfs(&out), f32::NEG_INFINITY, "silence is -inf, not a magic number");
    }

    #[test]
    fn clip_gain_scales_the_render() {
        let (full, audio) = project_with(0.5, 1.0);
        let (half, _) = project_with(0.5, 0.5);
        let w = (Ticks::ZERO, Ticks::from_seconds(0.5));
        let a = mixdown(&full, &audio, &BTreeMap::new(), w.0, w.1, 48_000, &[]);
        let b = mixdown(&half, &audio, &BTreeMap::new(), w.0, w.1, 48_000, &[]);
        let (pa, pb) = (peak_dbfs(&a), peak_dbfs(&b));
        assert!((pa - pb - 6.02).abs() < 0.2, "half gain is ~6 dB down: {pa} vs {pb}");
    }

    #[test]
    fn a_muted_track_is_silent_and_solo_wins_over_everything_else() {
        let (mut p, audio) = project_with(0.5, 1.0);
        // mute → silence
        p.apply(&Op::SetTrackFlags { id: TrackId(1), muted: true, locked: false }).unwrap();
        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5), 48_000, &[]);
        assert!(out.iter().all(|s| *s == 0.0), "a muted track contributes nothing");
        p.apply(&Op::SetTrackFlags { id: TrackId(1), muted: false, locked: false }).unwrap();
        // solo on ANOTHER track silences this one — the rule reimplementations invert
        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5),
                          48_000, &[TrackId(99)]);
        assert!(out.iter().all(|s| *s == 0.0), "un-soloed tracks are silent while anything is soloed");
        let out = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(0.5),
                          48_000, &[TrackId(1)]);
        assert!(out.iter().any(|s| *s != 0.0), "and the soloed track is heard");
    }

    #[test]
    fn fades_reach_silence_at_the_edges_and_unity_in_the_middle() {
        let len = Ticks::from_seconds(4.0);
        let f = Fades { fade_in: Ticks::from_seconds(1.0), fade_out: Ticks::from_seconds(1.0) };
        assert_eq!(f.gain_at(Ticks::ZERO, len), 0.0, "silent at the very start");
        assert!((f.gain_at(Ticks::from_seconds(0.5), len) - 0.5).abs() < 1e-4, "halfway up");
        assert_eq!(f.gain_at(Ticks::from_seconds(2.0), len), 1.0, "unity in the middle");
        assert!((f.gain_at(Ticks::from_seconds(3.5), len) - 0.5).abs() < 1e-4, "halfway down");
        assert!(f.gain_at(len, len) <= 1e-6, "silent at the very end");
        // a clip with no fades is unity throughout
        let none = Fades::default();
        assert_eq!(none.gain_at(Ticks::ZERO, len), 1.0);
    }

    #[test]
    fn only_the_requested_window_is_rendered() {
        // Rendering a middle second must not shift the material: the samples in
        // the window have to be the same ones a full render would put there.
        let (p, audio) = project_with(3.0, 1.0);
        let full = mixdown(&p, &audio, &BTreeMap::new(), Ticks::ZERO, Ticks::from_seconds(3.0), 48_000, &[]);
        let mid = mixdown(&p, &audio, &BTreeMap::new(),
                          Ticks::from_seconds(1.0), Ticks::from_seconds(2.0), 48_000, &[]);
        assert_eq!(mid.len(), 48_000 * 2);
        let off = 48_000 * 2;
        for i in 0..mid.len() {
            assert!((mid[i] - full[off + i]).abs() < 1e-6, "window sample {i} must match the full render");
        }
    }
}
