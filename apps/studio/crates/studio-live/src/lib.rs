//! studio-live — patterns, synths, and recording.
//!
//! ## Why a pattern rack next to a timeline
//!
//! Audacity's model is a tape recorder: tracks of recorded audio you cut. FL
//! Studio's is a **step rack**: channels of instruments triggered by patterns,
//! and patterns arranged on a playlist. Neither is wrong, and an editor that
//! only does one of them is annoying in a predictable way — you end up bouncing
//! a loop to audio just to place it, or drawing a hundred identical clips
//! because there is no pattern to repeat.
//!
//! Studio keeps the timeline as the arrangement and adds the rack as a
//! *generator*: a [`Pattern`] placed on the timeline is a clip whose source is
//! synthesised rather than decoded. That way one model serves both — the
//! collaboration, undo and mixdown machinery already built does not care where
//! samples come from.
//!
//! ## Synths, and beatspace as one
//!
//! [`Synth`] is deliberately tiny: fill a buffer for a note. `apps/beatspace`
//! is a mel-spectrogram GAN that walks a latent space and crossfades generated
//! beats; its player already exposes `next_sample()`, which is exactly this
//! interface. So beatspace becomes a synth *kind* whose "note" is a position in
//! latent space — [`SynthKind::Beatspace`] carries `z`, and a pattern step can
//! move it, which makes latent drift a sequenced, automatable parameter rather
//! than something you only do by dragging a thumb across a pad.
//!
//! The heavy model is NOT linked here. [`SynthKind`] is a description; a host
//! that has beatspace loaded registers a renderer for it, and one that does not
//! still opens, edits and plays the rest of the project. That is the same
//! bargain the effect list makes in `studio-core`, for the same reason: a
//! project must never fail to open because a plugin is missing.

pub mod transport;

use studio_audio::SourceAudio;
use studio_core::Ticks;

/// Musical time. Tempo lives here rather than on the timeline because a project
/// can hold several tempos and the timeline is in absolute ticks regardless.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tempo {
    pub bpm: f32,
    /// Steps per beat — 4 gives the classic 16-steps-per-bar rack.
    pub steps_per_beat: u32,
}

impl Default for Tempo {
    fn default() -> Self { Tempo { bpm: 120.0, steps_per_beat: 4 } }
}

impl Tempo {
    pub fn step_len(&self) -> Ticks {
        Ticks::from_seconds(60.0 / self.bpm as f64 / self.steps_per_beat as f64)
    }
    pub fn bar_len(&self, beats_per_bar: u32) -> Ticks {
        Ticks(self.step_len().0 * self.steps_per_beat as i64 * beats_per_bar as i64)
    }
    /// Which step contains `t`, counting from the pattern's start.
    pub fn step_at(&self, t: Ticks) -> i64 {
        let s = self.step_len().0.max(1);
        t.0.div_euclid(s)
    }
}

/// What a channel is playing.
#[derive(Debug, Clone, PartialEq)]
pub enum SynthKind {
    /// A plain sine — always available, so a rack is playable with no plugins.
    Sine { hz: f32 },
    /// A one-shot sample from a project source (the classic drum rack channel).
    Sampler { source: studio_core::SourceId },
    /// `apps/beatspace`: a GAN walking a latent space. `z` is the position; a
    /// pattern step can move it, so latent drift becomes automation instead of
    /// a live-only gesture. Rendered by the host when beatspace is present.
    Beatspace { z: [f32; 3], crossfade: f32 },
}

/// One rack channel.
#[derive(Debug, Clone, PartialEq)]
pub struct Channel {
    pub name: String,
    pub kind: SynthKind,
    pub gain: f32,
    pub pan: f32,
    pub muted: bool,
}

/// A pattern: for each channel, which steps fire.
///
/// Stored as a bitmask per channel rather than a Vec<bool> because a rack is
/// edited by toggling single steps thousands of times a session, and because a
/// pattern that fits in a word is trivially cheap to send to a collaborator.
#[derive(Debug, Clone, PartialEq)]
pub struct Pattern {
    pub name: String,
    pub steps: u32,
    /// One mask per channel; bit `i` set means "fire on step i".
    pub rows: Vec<u64>,
}

impl Pattern {
    pub fn new(name: &str, steps: u32, channels: usize) -> Pattern {
        Pattern { name: name.into(), steps: steps.min(64), rows: vec![0; channels] }
    }
    pub fn is_set(&self, ch: usize, step: u32) -> bool {
        step < self.steps && self.rows.get(ch).map(|r| r >> step & 1 == 1).unwrap_or(false)
    }
    pub fn toggle(&mut self, ch: usize, step: u32) {
        if step >= self.steps { return; }
        if let Some(r) = self.rows.get_mut(ch) { *r ^= 1 << step; }
    }
    pub fn set(&mut self, ch: usize, step: u32, on: bool) {
        if step >= self.steps { return; }
        if let Some(r) = self.rows.get_mut(ch) {
            if on { *r |= 1 << step } else { *r &= !(1 << step) }
        }
    }
    /// Every (channel, step) that fires, in step order — what a renderer walks.
    pub fn hits(&self) -> Vec<(usize, u32)> {
        let mut v = Vec::new();
        for step in 0..self.steps {
            for (ch, _) in self.rows.iter().enumerate() {
                if self.is_set(ch, step) { v.push((ch, step)); }
            }
        }
        v
    }
    pub fn len(&self, tempo: &Tempo) -> Ticks {
        Ticks(tempo.step_len().0 * self.steps as i64)
    }
}

/// The rack: channels plus the patterns over them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Rack {
    pub channels: Vec<Channel>,
    pub patterns: Vec<Pattern>,
    pub tempo_bpm: f32,
}

/// Anything that can turn a channel hit into samples.
///
/// One method, because everything else (envelopes, filters, the model itself)
/// belongs to the implementation. The host registers these; the model does not
/// link them.
pub trait Synth {
    /// Render one hit into `out` (mono), starting at index 0. `out.len()` is
    /// how much room the caller has, not how long the sound is — a synth that
    /// wants to ring longer is cut off, which is the caller's decision.
    fn render(&mut self, kind: &SynthKind, rate: u32, out: &mut [f32]);
}

/// The always-available synth: sine tones and sample playback. A rack made only
/// of these needs no plugins at all, which is what keeps a project openable.
pub struct BuiltinSynth<'a> {
    pub sources: &'a std::collections::BTreeMap<studio_core::SourceId, SourceAudio>,
}

impl Synth for BuiltinSynth<'_> {
    fn render(&mut self, kind: &SynthKind, rate: u32, out: &mut [f32]) {
        match kind {
            SynthKind::Sine { hz } => {
                // a short percussive envelope, so a step reads as a hit rather
                // than a drone that never stops
                let n = out.len() as f32;
                for (i, s) in out.iter_mut().enumerate() {
                    let t = i as f32 / rate as f32;
                    let env = (1.0 - i as f32 / n).powf(3.0);
                    *s = (t * hz * std::f32::consts::TAU).sin() * env;
                }
            }
            SynthKind::Sampler { source } => {
                if let Some(src) = self.sources.get(source) {
                    for (i, s) in out.iter_mut().enumerate() {
                        *s = if i < src.frames() { src.mono(i) } else { 0.0 };
                    }
                }
            }
            SynthKind::Beatspace { .. } => {
                // Silence, deliberately and audibly harmless: this build has no
                // beatspace model. A host that loads one registers its own Synth
                // and gets real audio; everything else about the project still
                // works, which is the point of describing rather than linking.
                out.fill(0.0);
            }
        }
    }
}

/// Render a pattern to mono samples — the bounce that turns a rack into a clip.
pub fn render_pattern(
    pattern: &Pattern,
    rack: &Rack,
    synth: &mut dyn Synth,
    tempo: &Tempo,
    rate: u32,
) -> Vec<f32> {
    let step = tempo.step_len().to_samples(rate).max(1) as usize;
    let total = step * pattern.steps as usize;
    let mut out = vec![0.0f32; total];
    // A hit may ring past its step — up to four steps — so hits overlap the way
    // they do on a real rack instead of being truncated at the grid.
    let voice_len = (step * 4).min(total.max(step));
    let mut scratch = vec![0.0f32; voice_len];
    for (ch_i, step_i) in pattern.hits() {
        let Some(ch) = rack.channels.get(ch_i) else { continue };
        if ch.muted { continue; }
        scratch.fill(0.0);
        synth.render(&ch.kind, rate, &mut scratch);
        let at = step_i as usize * step;
        for (i, v) in scratch.iter().enumerate() {
            if at + i >= out.len() { break; }
            out[at + i] += v * ch.gain;
        }
    }
    out
}

// ── recording ───────────────────────────────────────────────────────────────

/// Capture state for punch-in recording.
///
/// The device callback pushes into [`Recorder::feed`] from the audio thread;
/// everything else happens on the UI thread. `feed` is the only method that
/// must be cheap, so it does nothing but append — no allocation in the steady
/// state, no locking decisions, no resampling.
#[derive(Debug, Clone)]
pub struct Recorder {
    pub rate: u32,
    pub channels: u16,
    /// Where on the timeline this take starts.
    pub punch_in: Ticks,
    buf: Vec<f32>,
    armed: bool,
    /// Highest absolute sample seen, for the input meter — computed while the
    /// samples are already in cache rather than by scanning the buffer later.
    peak: f32,
}

impl Recorder {
    pub fn new(rate: u32, channels: u16, punch_in: Ticks) -> Recorder {
        Recorder { rate, channels, punch_in, buf: Vec::with_capacity(rate as usize * 2), armed: false, peak: 0.0 }
    }
    pub fn arm(&mut self, on: bool) { self.armed = on; }
    pub fn is_armed(&self) -> bool { self.armed }
    /// Append interleaved input. Ignored unless armed, so a stray callback
    /// before the user hits record cannot smuggle audio into the take.
    pub fn feed(&mut self, interleaved: &[f32]) {
        if !self.armed { return; }
        for s in interleaved {
            self.peak = self.peak.max(s.abs());
        }
        self.buf.extend_from_slice(interleaved);
    }
    pub fn frames(&self) -> usize { self.buf.len() / self.channels.max(1) as usize }
    pub fn duration(&self) -> Ticks { Ticks::from_samples(self.frames() as i64, self.rate) }
    /// Peak since the last call, then reset — meter semantics, so a transient
    /// is never missed between UI frames.
    pub fn take_peak(&mut self) -> f32 { std::mem::replace(&mut self.peak, 0.0) }

    /// De-interleave the take into planar audio for a source.
    pub fn to_audio(&self) -> SourceAudio {
        let ch = self.channels.max(1) as usize;
        let mut planes = vec![Vec::with_capacity(self.frames()); ch];
        for (i, s) in self.buf.iter().enumerate() {
            planes[i % ch].push(*s);
        }
        SourceAudio { sample_rate: self.rate, channels: planes }
    }

    /// The ops that land this take on the timeline.
    ///
    /// Returning OPS rather than mutating is what makes a recording shareable:
    /// the same two records go into undo and out to every collaborator, so a
    /// take someone else just played appears on your timeline by exactly the
    /// mechanism a drag does. Nothing about recording is special-cased in the
    /// sync layer, which is why it works at all.
    pub fn commit_ops(
        &self,
        source: studio_core::SourceId,
        clip: studio_core::ClipId,
        track: studio_core::TrackId,
        path: &str,
    ) -> Vec<studio_core::Op> {
        let len = self.duration();
        vec![
            studio_core::Op::AddSource {
                source: studio_core::Source {
                    id: source, path: path.into(), len,
                    sample_rate: self.rate, channels: self.channels, has_video: false,
                },
            },
            studio_core::Op::AddClip {
                clip: studio_core::Clip {
                    id: clip, source, track,
                    pos: self.punch_in, src_in: Ticks::ZERO, len,
                    gain: 1.0, effects: Vec::new(),
                },
            },
        ]
    }

    pub fn clear(&mut self) { self.buf.clear(); self.peak = 0.0; }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::{ClipId, Op, SourceId, TrackId};

    fn rack() -> Rack {
        Rack {
            channels: vec![
                Channel { name: "kick".into(), kind: SynthKind::Sine { hz: 60.0 }, gain: 1.0, pan: 0.0, muted: false },
                Channel { name: "hat".into(), kind: SynthKind::Sine { hz: 8000.0 }, gain: 0.6, pan: 0.0, muted: false },
                Channel { name: "beatspace".into(),
                          kind: SynthKind::Beatspace { z: [0.1, -0.4, 0.0], crossfade: 0.5 },
                          gain: 1.0, pan: 0.0, muted: false },
            ],
            patterns: vec![],
            tempo_bpm: 120.0,
        }
    }

    #[test]
    fn steps_land_on_the_beat_grid() {
        let t = Tempo { bpm: 120.0, steps_per_beat: 4 };
        // 120bpm = 0.5s a beat, so a 16th is 0.125s
        assert!((t.step_len().to_seconds() - 0.125).abs() < 1e-9);
        // a 4/4 bar is 2 seconds
        assert!((t.bar_len(4).to_seconds() - 2.0).abs() < 1e-9);
        assert_eq!(t.step_at(Ticks::from_seconds(0.0)), 0);
        assert_eq!(t.step_at(Ticks::from_seconds(0.3)), 2);
        assert_eq!(t.step_at(Ticks::from_seconds(2.0)), 16, "the next bar starts at step 16");
    }

    #[test]
    fn toggling_steps_is_exactly_reversible() {
        let mut p = Pattern::new("A", 16, 3);
        assert!(!p.is_set(0, 4));
        p.toggle(0, 4);
        assert!(p.is_set(0, 4));
        p.toggle(0, 4);
        assert!(!p.is_set(0, 4), "a rack that cannot un-toggle is unusable");
        // out-of-range steps are ignored rather than panicking or wrapping into
        // a neighbouring channel's bits
        p.toggle(0, 99);
        p.set(9, 1, true);
        assert_eq!(p.rows[0], 0);
    }

    #[test]
    fn hits_come_out_in_step_order() {
        let mut p = Pattern::new("A", 8, 2);
        p.set(0, 0, true); p.set(0, 4, true);
        p.set(1, 2, true); p.set(1, 4, true);
        let hits = p.hits();
        assert_eq!(hits, vec![(0, 0), (1, 2), (0, 4), (1, 4)]);
        let steps: Vec<u32> = hits.iter().map(|(_, s)| *s).collect();
        assert!(steps.windows(2).all(|w| w[0] <= w[1]), "a renderer walks these in time order");
    }

    #[test]
    fn a_rendered_pattern_has_audio_where_the_steps_are_and_silence_where_they_are_not() {
        let r = rack();
        let mut p = Pattern::new("A", 8, 3);
        p.set(0, 0, true);
        p.set(0, 4, true);
        let t = Tempo { bpm: 120.0, steps_per_beat: 4 };
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        let out = render_pattern(&p, &r, &mut synth, &t, 48_000);
        let step = t.step_len().to_samples(48_000) as usize;
        assert_eq!(out.len(), step * 8);
        let energy = |from: usize, to: usize| out[from..to.min(out.len())]
            .iter().map(|s| s.abs()).sum::<f32>();
        assert!(energy(0, step) > 1.0, "step 0 fires");
        assert!(energy(step * 4, step * 5) > 1.0, "step 4 fires");
        // step 6 has no hit and is far enough past step 4 for the tail to have decayed
        assert!(energy(step * 6, step * 7) < energy(step * 4, step * 5) * 0.2,
                "an empty step is quiet");
    }

    #[test]
    fn a_missing_beatspace_model_is_silent_rather_than_fatal() {
        // The bargain the whole plugin design rests on: a project made with
        // beatspace still opens, edits and plays without it.
        let r = rack();
        let mut p = Pattern::new("A", 4, 3);
        p.set(2, 0, true); // the beatspace channel
        let t = Tempo::default();
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        let out = render_pattern(&p, &r, &mut synth, &t, 48_000);
        assert!(!out.is_empty(), "the pattern still renders");
        assert!(out.iter().all(|s| *s == 0.0), "and is silent, not a crash or a click");
        // the channel description survives so a host WITH beatspace can play it
        assert!(matches!(r.channels[2].kind, SynthKind::Beatspace { .. }));
    }

    #[test]
    fn a_muted_channel_contributes_nothing() {
        let mut r = rack();
        r.channels[0].muted = true;
        let mut p = Pattern::new("A", 4, 3);
        p.set(0, 0, true);
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        let out = render_pattern(&p, &r, &mut synth, &Tempo::default(), 48_000);
        assert!(out.iter().all(|s| *s == 0.0));
    }

    #[test]
    fn recording_only_captures_while_armed() {
        let mut rec = Recorder::new(48_000, 2, Ticks::from_seconds(4.0));
        rec.feed(&[0.5; 480]);
        assert_eq!(rec.frames(), 0, "audio before arming must never enter the take");
        rec.arm(true);
        rec.feed(&[0.5; 480]);
        assert_eq!(rec.frames(), 240, "interleaved stereo: 480 samples is 240 frames");
        assert!((rec.duration().to_seconds() - 240.0 / 48_000.0).abs() < 1e-9);
    }

    #[test]
    fn the_input_meter_reports_a_peak_once_and_then_resets() {
        let mut rec = Recorder::new(48_000, 1, Ticks::ZERO);
        rec.arm(true);
        rec.feed(&[0.1, -0.8, 0.2]);
        assert!((rec.take_peak() - 0.8).abs() < 1e-6, "the transient is caught");
        assert_eq!(rec.take_peak(), 0.0, "and not reported twice");
    }

    #[test]
    fn a_take_commits_as_ops_so_collaborators_get_it_by_the_normal_path() {
        // The point: recording is not special-cased in the sync layer. A take
        // arrives on a peer's timeline by exactly the mechanism a drag does.
        let mut rec = Recorder::new(48_000, 2, Ticks::from_seconds(8.0));
        rec.arm(true);
        rec.feed(&[0.25; 96_000]); // one second of stereo
        let ops = rec.commit_ops(SourceId(7), ClipId(70), TrackId(2), "takes/take-1.wav");
        assert_eq!(ops.len(), 2, "a source and the clip that references it");
        match (&ops[0], &ops[1]) {
            (Op::AddSource { source }, Op::AddClip { clip }) => {
                assert_eq!(source.sample_rate, 48_000);
                assert_eq!(source.channels, 2);
                assert!((source.len.to_seconds() - 1.0).abs() < 1e-6);
                assert_eq!(clip.pos, Ticks::from_seconds(8.0), "the take lands at the punch-in");
                assert_eq!(clip.len, source.len, "and is as long as what was played");
                assert_eq!(clip.source, source.id);
            }
            other => panic!("unexpected ops: {other:?}"),
        }
        // and the planar form is what the mixer wants
        let audio = rec.to_audio();
        assert_eq!(audio.channels.len(), 2);
        assert_eq!(audio.frames(), 48_000);
    }
}
