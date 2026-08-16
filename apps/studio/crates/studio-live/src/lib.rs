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
use std::collections::BTreeMap;
use serde::{Deserialize, Serialize};

/// Musical time. Tempo lives here rather than on the timeline because a project
/// can hold several tempos and the timeline is in absolute ticks regardless.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Tempo {
    pub bpm: f32,
    /// Steps per beat — 4 gives the classic 16-steps-per-bar rack.
    pub steps_per_beat: u32,
    /// 0..=1. Swing delays every SECOND step (the off-16ths) by up to half a
    /// step — 0 is straight, ~0.15 is a typical trap hat feel, 1 pushes the
    /// off-step all the way to the following 32nd-triplet-ish extreme. It is
    /// a render-time placement offset, not a change to the grid: [`Tempo::
    /// step_at`] and the grid playhead stay on the straight grid, which is
    /// how every hardware sequencer draws it — the LEDs march evenly while
    /// the audio swings.
    pub swing: f32,
}

impl Default for Tempo {
    fn default() -> Self { Tempo { bpm: 120.0, steps_per_beat: 4, swing: 0.0 } }
}

impl Tempo {
    pub fn step_len(&self) -> Ticks {
        Ticks::from_seconds(60.0 / self.bpm as f64 / self.steps_per_beat as f64)
    }
    /// How many samples late a hit on `step` starts, from swing. Even steps
    /// are never moved — swing that shifted the downbeats would just be a
    /// slower tempo wearing a costume.
    pub fn swing_delay_samples(&self, step: u32, rate: u32) -> usize {
        if step % 2 == 0 || self.swing <= 0.0 { return 0; }
        let step_samples = self.step_len().to_samples(rate).max(1) as f32;
        (self.swing.clamp(0.0, 1.0) * step_samples * 0.5) as usize
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SynthKind {
    /// A plain sine — always available, so a rack is playable with no plugins.
    Sine { hz: f32 },
    /// A one-shot sample from a project source (the classic drum rack channel).
    Sampler { source: studio_core::SourceId },
    /// `apps/beatspace`: a GAN walking a latent space. `z` is the position; a
    /// pattern step can move it, so latent drift becomes automation instead of
    /// a live-only gesture. Rendered by the host when beatspace is present.
    Beatspace { z: [f32; 3], crossfade: f32 },
    /// The 808: a sine core with optional pitch glide and saturation drive,
    /// built in (unlike Beatspace, it needs no external model). The trap rack
    /// item this crate exists to support.
    Sub {
        /// Semitone offset from [`SUB_REFERENCE_HZ`] (55 Hz, A1 — a common
        /// 808 fundamental).
        note: i8,
        /// `Some(prev)` glides from `prev`'s pitch to `note`'s over
        /// `glide_ms`, instead of starting at `note` directly. Carrying the
        /// source pitch explicitly here — rather than the synth remembering
        /// "the last note played" as hidden mutable state — is the same
        /// choice `studio-core::Op` makes everywhere else: an absolute value
        /// the caller supplies, not a delta inferred from history, so
        /// `render` stays a pure function of its inputs and a step-mod layer
        /// built later can decide per-hit whether to slide without this
        /// synth needing to track anything across calls.
        glide_from: Option<i8>,
        glide_ms: f32,
        /// 0..=1, fed into a `studio_fx::saturation::Saturator` (`Profile::Tube`
        /// — asymmetric, strong 2nd harmonic, the classic 808 warmth). 0
        /// skips the saturator entirely.
        drive: f32,
        /// Amplitude decay time: roughly the time to fall to -60dB.
        decay_ms: f32,
    },
    /// A synthesised drum voice — kick, snare, clap, hats — built in for the
    /// same reason `Sub` is: a trap rack has to be playable with zero sample
    /// packs loaded, or the first thing a new project asks for is an asset
    /// hunt. `Sampler` remains the path for anyone who brings their own kit.
    Drum {
        voice: DrumVoice,
        /// 0..=1, voice-specific colour: the kick's start pitch and click,
        /// the snare's and hats' brightness, the clap's tail darkness. One
        /// knob per channel rather than a synth page of them — the rack is a
        /// performance surface, and detailed sound design belongs to a
        /// dedicated synth kind later, not to more fields here.
        tone: f32,
        /// Amplitude decay to -60dB, same convention as [`SynthKind::Sub`].
        /// This is also the only difference between a closed and an open
        /// hat besides their default — there is no choke group yet (a closed
        /// hat does not yet cut a ringing open one), which is named here
        /// rather than pretended: choke is a cross-channel rule and belongs
        /// in [`render_pattern`], not inside a voice.
        decay_ms: f32,
        /// Semitone offset applied to the voice's internal frequencies, so a
        /// [`StepMod::tune`] means the same thing here it does on `Sub`.
        tune: i8,
    },
}

/// Which drum a [`SynthKind::Drum`] channel is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DrumVoice {
    /// Sine with a fast exponential pitch drop and a click transient.
    Kick,
    /// A short tonal body plus high-passed noise.
    Snare,
    /// One noise stream through the classic multi-burst envelope.
    Clap,
    /// A metallic square-bank plus noise, high-passed hard, short decay.
    HatClosed,
    /// The same metal, allowed to ring.
    HatOpen,
}

/// Reference fundamental for [`SynthKind::Sub`]'s semitone offsets: 55 Hz,
/// standard A1 tuning and a common 808 fundamental in trap production.
pub const SUB_REFERENCE_HZ: f32 = 55.0;

/// Semitone offset to Hz, relative to [`SUB_REFERENCE_HZ`].
pub fn sub_freq(note: i8) -> f32 {
    SUB_REFERENCE_HZ * 2f32.powf(note as f32 / 12.0)
}

/// One rack channel.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pattern {
    pub name: String,
    pub steps: u32,
    /// One mask per channel; bit `i` set means "fire on step i".
    pub rows: Vec<u64>,
    /// Sparse per-(channel, step) modifiers. A step with no entry behaves
    /// exactly like a plain hit — velocity 1, no ratchet, no retune, no
    /// slide, fires every time — which is the property that keeps a
    /// straightforward beat (most steps use none of this) cheap to store and
    /// cheap to send, the same reasoning that made `rows` a bitmask.
    pub mods: BTreeMap<(usize, u32), StepMod>,
}

/// Per-step trap-rack modifiers, layered on top of the plain on/off bitmask.
///
/// A default `StepMod` is defined to be indistinguishable from no entry at
/// all — see [`Pattern::set_mod`], which relies on that to keep storage
/// sparse automatically rather than accumulating a `StepMod::default()` for
/// every step someone has ever touched.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct StepMod {
    /// 0..=1, scales the channel's own gain for this hit only.
    pub vel: f32,
    /// 1..=4 retriggers inside the step, evenly subdivided — a hat roll.
    /// Values outside the range are clamped when rendered, not refused; a
    /// pattern is a value, and clamping keeps it always renderable rather
    /// than making a bounce fail over one bad byte.
    pub ratchet: u8,
    /// Semitone offset from the channel's own note/pitch. Meaningful for
    /// [`SynthKind::Sub`] (added to `note`) and [`SynthKind::Sine`] (scales
    /// `hz`); a no-op for `Sampler` and `Beatspace`, which have no pitch
    /// parameter to offset.
    pub tune: i8,
    /// Glide from whatever note this channel last played, instead of
    /// starting at this hit's own pitch directly. If this is the channel's
    /// first hit in the render (nothing to glide from), `slide` is silently
    /// a no-op rather than an error — a pattern's first note can't legally
    /// mean anything else.
    pub slide: bool,
    /// 0..=1. Below 1, the hit fires probabilistically rather than always —
    /// see [`render_pattern`]'s docs on how this stays deterministic despite
    /// being random-looking.
    pub prob: f32,
    /// `Some((n, k))` fires only on passes where `pass % k == n` — "every
    /// other bar," "the 3rd of 4." `None` (the common case) fires on every
    /// pass, subject to `prob`.
    pub every: Option<(u8, u8)>,
}

impl Default for StepMod {
    fn default() -> StepMod {
        StepMod { vel: 1.0, ratchet: 1, tune: 0, slide: false, prob: 1.0, every: None }
    }
}

impl Pattern {
    pub fn new(name: &str, steps: u32, channels: usize) -> Pattern {
        Pattern { name: name.into(), steps: steps.min(64), rows: vec![0; channels], mods: BTreeMap::new() }
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
    /// The modifiers at (ch, step), or the all-defaults `StepMod` if none
    /// were ever set — indistinguishable from "a plain hit" by design.
    pub fn mod_at(&self, ch: usize, step: u32) -> StepMod {
        self.mods.get(&(ch, step)).copied().unwrap_or_default()
    }
    /// Setting a step back to `StepMod::default()` removes its sparse entry
    /// rather than storing an all-defaults value — this is what keeps
    /// `mods` proportional to the steps someone actually customised, not to
    /// every step anyone has ever touched a knob on.
    pub fn set_mod(&mut self, ch: usize, step: u32, m: StepMod) {
        if m == StepMod::default() {
            self.mods.remove(&(ch, step));
        } else {
            self.mods.insert((ch, step), m);
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
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Rack {
    pub channels: Vec<Channel>,
    pub patterns: Vec<Pattern>,
    pub tempo_bpm: f32,
    /// 0..=1, see [`Tempo::swing`]. `serde(default)` so a rack saved before
    /// swing existed still loads — straight, which is what it was.
    #[serde(default)]
    pub swing: f32,
}

impl Rack {
    /// The [`Tempo`] this rack renders at. One source of truth: the rack is
    /// the serialized value, `Tempo` is its render-time view, and deriving
    /// the second from the first here is what keeps a BPM knob in a UI from
    /// quietly disagreeing with what a bounce used.
    pub fn tempo(&self) -> Tempo {
        Tempo { bpm: self.tempo_bpm.max(1.0), steps_per_beat: 4, swing: self.swing }
    }

    /// Add a channel, keeping every pattern's row count in step. The
    /// invariant (one row per channel) lives HERE because breaking it fails
    /// silently everywhere else: `Pattern::set` on a missing row is a no-op
    /// by design, so a channel added without its rows would be a grid you
    /// can paint on that never plays — the silently-dropped-edit failure,
    /// wearing a synth.
    pub fn add_channel(&mut self, channel: Channel) -> usize {
        self.channels.push(channel);
        for p in &mut self.patterns {
            while p.rows.len() < self.channels.len() {
                p.rows.push(0);
            }
        }
        self.channels.len() - 1
    }

    /// A playable trap kit with a one-bar starter groove — the rack analogue
    /// of `Studio::demo()`'s timeline, for the same stated reason: an empty
    /// grid gives a new user nothing to press, and is indistinguishable from
    /// a broken one. 140 BPM, light swing, an 808 that slides.
    pub fn trap_kit() -> Rack {
        let ch = |name: &str, kind: SynthKind, gain: f32| Channel {
            name: name.into(), kind, gain, pan: 0.0, muted: false,
        };
        let channels = vec![
            ch("808",   SynthKind::Sub { note: 0, glide_from: None, glide_ms: 70.0,
                                          drive: 0.35, decay_ms: 420.0 }, 0.9),
            ch("Kick",  SynthKind::Drum { voice: DrumVoice::Kick, tone: 0.45,
                                          decay_ms: 160.0, tune: 0 }, 0.95),
            ch("Snare", SynthKind::Drum { voice: DrumVoice::Snare, tone: 0.55,
                                          decay_ms: 190.0, tune: 0 }, 0.8),
            ch("Clap",  SynthKind::Drum { voice: DrumVoice::Clap, tone: 0.5,
                                          decay_ms: 220.0, tune: 0 }, 0.7),
            ch("Hat",   SynthKind::Drum { voice: DrumVoice::HatClosed, tone: 0.6,
                                          decay_ms: 55.0, tune: 0 }, 0.55),
            ch("OHat",  SynthKind::Drum { voice: DrumVoice::HatOpen, tone: 0.6,
                                          decay_ms: 320.0, tune: 0 }, 0.5),
        ];
        let mut p = Pattern::new("Trap A", 16, channels.len());
        // 808 shadows the kick and slides down through the bar.
        for s in [0, 6, 10] { p.set(0, s, true); }
        p.set_mod(0, 6,  StepMod { tune: -2, ..StepMod::default() });
        p.set_mod(0, 10, StepMod { tune: -5, slide: true, ..StepMod::default() });
        for s in [0, 6, 10] { p.set(1, s, true); }
        for s in [4, 12] { p.set(2, s, true); }
        p.set(3, 12, true);
        for s in 0..16 { p.set(4, s, true); }
        // Hat accents on the beats, a ghost between, and rolls into beat 3
        // and the next bar — the part of a trap groove the mods exist for.
        for s in [1u32, 3, 5, 9, 11, 13] {
            p.set_mod(4, s, StepMod { vel: 0.65, ..StepMod::default() });
        }
        p.set_mod(4, 7,  StepMod { vel: 0.8, ratchet: 2, ..StepMod::default() });
        p.set_mod(4, 15, StepMod { vel: 0.85, ratchet: 3, ..StepMod::default() });
        p.set(5, 14, true);
        p.set_mod(5, 14, StepMod { vel: 0.6, ..StepMod::default() });
        Rack { channels, patterns: vec![p], tempo_bpm: 140.0, swing: 0.14 }
    }
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
                    if src.sample_rate == rate {
                        for (i, s) in out.iter_mut().enumerate() {
                            *s = if i < src.frames() { src.mono(i) } else { 0.0 };
                        }
                    } else {
                        // Linear resample at render time, so a 44.1k drum
                        // library plays at pitch on a 48k render (and vice
                        // versa) instead of shifting ~9% sharp — the classic
                        // "why is my sample chipmunked" bug. Linear is
                        // audibly fine for one-shots; a windowed-sinc pass
                        // is a quality upgrade for later, not a correctness
                        // requirement for now.
                        let step = src.sample_rate as f64 / rate as f64;
                        let frames = src.frames();
                        for (i, s) in out.iter_mut().enumerate() {
                            let pos = i as f64 * step;
                            let i0 = pos as usize;
                            if i0 + 1 >= frames { *s = 0.0; continue; }
                            let frac = (pos - i0 as f64) as f32;
                            *s = src.mono(i0) * (1.0 - frac) + src.mono(i0 + 1) * frac;
                        }
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
            SynthKind::Sub { note, glide_from, glide_ms, drive, decay_ms } => {
                let target_hz = sub_freq(*note);
                let start_hz = glide_from.map(sub_freq).unwrap_or(target_hz);
                let glide_samples = ((*glide_ms / 1000.0) * rate as f32).round().max(0.0) as usize;
                let glide_samples = glide_samples.min(out.len());
                // Decay time constant so amplitude reaches -60dB (1/1000) by
                // decay_ms: exp(-decay_ms/1000/tau) = 1/1000 → tau =
                // (decay_ms/1000) / ln(1000).
                let tau = ((*decay_ms).max(1.0) / 1000.0) / 6.907_755;
                // Phase is integrated every sample and NEVER reset, even
                // while the frequency is sweeping — that is what makes the
                // glide phase-continuous by construction rather than by
                // careful tuning: a discontinuity only happens if something
                // jumps the phase, and nothing here does.
                let mut phase = 0.0f64;
                for (i, s) in out.iter_mut().enumerate() {
                    let hz = if glide_samples == 0 || i >= glide_samples {
                        target_hz
                    } else {
                        // Exponential (log-frequency) interpolation — a
                        // musical glide sweeps octaves at a constant rate,
                        // not Hz at a constant rate.
                        let t = i as f32 / glide_samples as f32;
                        start_hz * (target_hz / start_hz).powf(t)
                    };
                    phase += std::f64::consts::TAU * hz as f64 / rate as f64;
                    let t_s = i as f32 / rate as f32;
                    let env = (-(t_s as f64 / tau.max(1e-6) as f64)).exp() as f32;
                    *s = phase.sin() as f32 * env;
                }
                if *drive > 0.0 {
                    let mut sat = studio_fx::saturation::Saturator::new(studio_fx::saturation::Profile::Tube);
                    sat.drive = drive.clamp(0.0, 1.0);
                    sat.process_block(out);
                }
            }
            SynthKind::Drum { voice, tone, decay_ms, tune } => {
                render_drum(*voice, tone.clamp(0.0, 1.0), decay_ms.max(1.0), *tune, rate, out);
            }
        }
    }
}

// ── drum synthesis ──────────────────────────────────────────────────────────
//
// Everything below is deterministic: the "noise" is core_kit's SplitMix64
// from a seed fixed per voice, so the same hit renders the same samples every
// time — the property render_pattern's own docs promise ("the same pass of
// the same pattern always renders identically"), extended to the voices
// themselves. A system RNG here would make every bounce a different file.

/// -60dB decay time constant, same convention as `SynthKind::Sub`.
fn tau_60(decay_ms: f32) -> f32 { (decay_ms.max(1.0) / 1000.0) / 6.907_755 }

/// One-pole high-pass. Small enough to live here; `studio_fx`'s EQ is a
/// block-based effect with its own state and is the wrong shape for a voice
/// that filters while it synthesises.
struct OnePoleHp { a: f32, y: f32, x1: f32 }
impl OnePoleHp {
    fn new(fc: f32, rate: u32) -> OnePoleHp {
        let rc = 1.0 / (std::f32::consts::TAU * fc.max(1.0));
        let dt = 1.0 / rate as f32;
        OnePoleHp { a: rc / (rc + dt), y: 0.0, x1: 0.0 }
    }
    fn step(&mut self, x: f32) -> f32 {
        self.y = self.a * (self.y + x - self.x1);
        self.x1 = x;
        self.y
    }
}

/// One-pole low-pass, the same size and for the same reason.
struct OnePoleLp { a: f32, y: f32 }
impl OnePoleLp {
    fn new(fc: f32, rate: u32) -> OnePoleLp {
        let rc = 1.0 / (std::f32::consts::TAU * fc.max(1.0));
        let dt = 1.0 / rate as f32;
        OnePoleLp { a: dt / (rc + dt), y: 0.0 }
    }
    fn step(&mut self, x: f32) -> f32 {
        self.y += self.a * (x - self.y);
        self.y
    }
}

fn render_drum(voice: DrumVoice, tone: f32, decay_ms: f32, tune: i8, rate: u32, out: &mut [f32]) {
    let pitch = 2f32.powf(tune as f32 / 12.0);
    let dt = 1.0 / rate as f32;
    let tau = tau_60(decay_ms);
    let mut rng = core_kit::rng::SplitMix64::new(0x_D2_B0 + voice as u64);
    let mut noise = move || (rng.next_f64() as f32) * 2.0 - 1.0;

    match voice {
        DrumVoice::Kick => {
            // Pitch falls exponentially from a tone-set start to the ~50Hz
            // fundamental; phase is integrated and never reset, the same
            // phase-continuity argument Sub's glide makes.
            let f_end = 48.0 * pitch;
            let f_start = (140.0 + tone * 220.0) * pitch;
            let pitch_tau = 0.020;
            let click = 0.15 + tone * 0.35;
            let mut hp = OnePoleHp::new(1800.0, rate);
            let mut phase = 0.0f64;
            for (i, s) in out.iter_mut().enumerate() {
                let t = i as f32 * dt;
                let hz = f_end + (f_start - f_end) * (-t / pitch_tau).exp();
                phase += std::f64::consts::TAU * hz as f64 * dt as f64;
                let body = phase.sin() as f32 * (-t / tau).exp();
                // The click is the first two milliseconds of high-passed
                // noise — the part of a kick that survives a phone speaker.
                let c = if t < 0.002 { hp.step(noise()) * click * (1.0 - t / 0.002) } else { 0.0 };
                *s = body + c;
            }
        }
        DrumVoice::Snare => {
            let body_hz = 190.0 * pitch;
            let body_tau = tau_60((decay_ms * 0.4).min(90.0));
            let mut hp = OnePoleHp::new(1200.0 + tone * 4000.0, rate);
            let mut phase = 0.0f64;
            for (i, s) in out.iter_mut().enumerate() {
                let t = i as f32 * dt;
                phase += std::f64::consts::TAU * body_hz as f64 * dt as f64;
                let body = phase.sin() as f32 * (-t / body_tau).exp() * (0.9 - tone * 0.4);
                let snap = hp.step(noise()) * (-t / tau).exp();
                *s = body * 0.6 + snap * 0.85;
            }
        }
        DrumVoice::Clap => {
            // One noise stream, retriggered envelope: three short bursts
            // ~11ms apart, then the real hit rings for decay_ms — the 909
            // recipe, which reads as hands rather than as a snare.
            let bursts = [0.0f32, 0.011, 0.022];
            let burst_tau = tau_60(9.0);
            let main_at = 0.033;
            let mut hp = OnePoleHp::new(900.0, rate);
            let mut lp = OnePoleLp::new(5200.0 - tone * 2600.0, rate);
            for (i, s) in out.iter_mut().enumerate() {
                let t = i as f32 * dt;
                let mut env = 0.0f32;
                for b in bursts {
                    if t >= b { env = env.max((-(t - b) / burst_tau).exp() * 0.8); }
                }
                if t >= main_at { env = env.max((-(t - main_at) / tau).exp()); }
                *s = lp.step(hp.step(noise())) * env * 1.4;
            }
        }
        DrumVoice::HatClosed | DrumVoice::HatOpen => {
            // Six detuned squares (the 808's cymbal bank ratios, near enough)
            // high-passed twice, plus a little noise so the top end shimmers.
            let ratios = [2.0f32, 3.0, 4.16, 5.43, 6.79, 8.21];
            let base = 100.0 * pitch * (0.9 + tone * 0.4);
            let mut phases = [0.0f64; 6];
            let fc = 6500.0 + tone * 2500.0;
            let mut hp1 = OnePoleHp::new(fc, rate);
            let mut hp2 = OnePoleHp::new(fc, rate);
            for (i, s) in out.iter_mut().enumerate() {
                let t = i as f32 * dt;
                let mut metal = 0.0f32;
                for (p, r) in phases.iter_mut().zip(ratios) {
                    *p += (base * r) as f64 * dt as f64;
                    metal += if p.fract() < 0.5 { 1.0 } else { -1.0 };
                }
                let x = metal / 6.0 * 0.8 + noise() * 0.3;
                *s = hp2.step(hp1.step(x)) * (-t / tau).exp();
            }
        }
    }
}

/// Turn a channel's base [`SynthKind`] plus a step's [`StepMod`] into the
/// kind actually rendered for this hit. `prev_note` is the note this channel
/// last played in this render (tracked by [`render_pattern`]'s own loop, not
/// by the synth) — see `SynthKind::Sub::glide_from`'s docs on why glide
/// state lives in the value passed to `render`, not in hidden state.
fn apply_step_mod(kind: &SynthKind, m: &StepMod, prev_note: Option<i8>) -> SynthKind {
    match kind {
        SynthKind::Sub { note, glide_ms, drive, decay_ms, .. } => SynthKind::Sub {
            note: note.saturating_add(m.tune),
            glide_from: if m.slide { prev_note } else { None },
            glide_ms: *glide_ms,
            drive: *drive,
            decay_ms: *decay_ms,
        },
        SynthKind::Sine { hz } => SynthKind::Sine { hz: hz * 2f32.powf(m.tune as f32 / 12.0) },
        SynthKind::Drum { voice, tone, decay_ms, tune } => SynthKind::Drum {
            voice: *voice, tone: *tone, decay_ms: *decay_ms,
            tune: tune.saturating_add(m.tune),
        },
        // Sampler and Beatspace have no pitch parameter `tune` or `slide`
        // could act on — left untouched rather than pretending to apply
        // something that has no defined meaning for them.
        other => other.clone(),
    }
}

/// The kind for a ratchet subdivision after the first: same pitch, no
/// inherited glide. Sliding into your own retrigger has no obvious meaning,
/// so it's deliberately not attempted — only the first subdivision of a
/// ratchet carries whatever glide the step asked for.
fn strip_glide(kind: &SynthKind) -> SynthKind {
    match kind {
        SynthKind::Sub { note, glide_ms, drive, decay_ms, .. } => {
            SynthKind::Sub { note: *note, glide_from: None, glide_ms: *glide_ms, drive: *drive, decay_ms: *decay_ms }
        }
        other => other.clone(),
    }
}

/// Render a pattern to mono samples — the bounce that turns a rack into a clip.
///
/// `pass` is which loop iteration this bounce represents (0 for a one-shot
/// bounce), and is the only thing that makes [`StepMod::prob`] and
/// [`StepMod::every`] meaningful: `render_pattern` is otherwise a pure
/// function of (pattern, rack, tempo, rate), same as `studio_audio::mixdown`,
/// and probability is made deterministic rather than actually random by
/// seeding a PRNG from `(pass, channel, step)` — the same pass of the same
/// pattern always renders identically, which is what keeps a render (and a
/// test of one) reproducible. This is the same trade `unendlich`'s worldgen
/// makes (`core_kit::rng::SplitMix64`, deterministic by seed rather than a
/// system RNG), reused here rather than re-derived.
pub fn render_pattern(
    pattern: &Pattern,
    rack: &Rack,
    synth: &mut dyn Synth,
    tempo: &Tempo,
    rate: u32,
    pass: u32,
) -> Vec<f32> {
    let step = tempo.step_len().to_samples(rate).max(1) as usize;
    let total = step * pattern.steps as usize;
    let mut out = vec![0.0f32; total];
    // A hit may ring past its step — up to four steps — so hits overlap the way
    // they do on a real rack instead of being truncated at the grid.
    let voice_len = (step * 4).min(total.max(step));
    let mut scratch = vec![0.0f32; voice_len];
    // The note each channel most recently played, in step order — local to
    // this render, not the synth's business. See `apply_step_mod`'s docs.
    let mut last_note: BTreeMap<usize, i8> = BTreeMap::new();

    for (ch_i, step_i) in pattern.hits() {
        let Some(ch) = rack.channels.get(ch_i) else { continue };
        if ch.muted { continue; }
        let m = pattern.mod_at(ch_i, step_i);

        if let Some((n, k)) = m.every {
            if k > 0 && pass % k as u32 != n as u32 {
                continue;
            }
        }
        if m.prob < 1.0 {
            let seed = ((pass as u64) << 32) ^ ((ch_i as u64) << 16) ^ (step_i as u64);
            let mut rng = core_kit::rng::SplitMix64::new(seed);
            if (rng.next_f64() as f32) >= m.prob {
                continue; // this trial did not fire
            }
        }

        let kind = apply_step_mod(&ch.kind, &m, last_note.get(&ch_i).copied());
        if let SynthKind::Sub { note, .. } = &kind {
            last_note.insert(ch_i, *note);
        }

        let ratchet = m.ratchet.clamp(1, 4) as usize;
        let sub_step = (step / ratchet).max(1);
        // Swing moves the hit, not the grid: the delay is applied here, at
        // placement, so step_at / the playhead keep reporting the straight
        // column the hit belongs to. Ratchet subdivisions ride along with
        // their (possibly swung) parent step rather than being re-swung
        // individually — a roll is one gesture, not four independent notes.
        let swing = tempo.swing_delay_samples(step_i, rate);
        for r in 0..ratchet {
            scratch.fill(0.0);
            let this_kind = if r == 0 { kind.clone() } else { strip_glide(&kind) };
            synth.render(&this_kind, rate, &mut scratch);
            let at = step_i as usize * step + swing + r * sub_step;
            for (i, v) in scratch.iter().enumerate() {
                if at + i >= out.len() { break; }
                out[at + i] += v * ch.gain * m.vel;
            }
        }
    }
    out
}

/// The ops that land a pattern's render onto the timeline as a clip —
/// "bounce the beat" made real. Deliberately the same shape as
/// [`Recorder::commit_ops`]: a pattern render and a vocal take are both
/// "audio that needs to become a clip," and sharing the shape is what keeps
/// a bounce going through the identical undo/collab path a punch-in take
/// does, rather than needing its own special case in the sync layer.
///
/// The actual samples (`render_pattern`'s own return value, analogous to
/// [`Recorder::to_audio`]) are the caller's responsibility to store keyed by
/// `source` — this function only returns the two ops that describe where
/// they live and how long they are, exactly like `commit_ops` does for a
/// take.
pub fn bounce_ops(
    pattern: &Pattern,
    tempo: &Tempo,
    rate: u32,
    source: studio_core::SourceId,
    clip: studio_core::ClipId,
    track: studio_core::TrackId,
    at: Ticks,
    path: &str,
) -> Vec<studio_core::Op> {
    let step = tempo.step_len().to_samples(rate).max(1) as usize;
    let total_frames = step * pattern.steps as usize;
    let len = Ticks::from_samples(total_frames as i64, rate);
    vec![
        studio_core::Op::AddSource {
            source: studio_core::Source {
                id: source, path: path.into(), len,
                sample_rate: rate, channels: 1, has_video: false,
            },
        },
        studio_core::Op::AddClip {
            clip: studio_core::Clip {
                id: clip, source, track,
                pos: at, src_in: Ticks::ZERO, len,
                gain: 1.0, effects: Vec::new(),
            },
        },
    ]
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
            tempo_bpm: 120.0, swing: 0.0,
        }
    }

    #[test]
    fn steps_land_on_the_beat_grid() {
        let t = Tempo { bpm: 120.0, steps_per_beat: 4, swing: 0.0 };
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
        let t = Tempo { bpm: 120.0, steps_per_beat: 4, swing: 0.0 };
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        let out = render_pattern(&p, &r, &mut synth, &t, 48_000, 0);
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
        let out = render_pattern(&p, &r, &mut synth, &t, 48_000, 0);
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
        let out = render_pattern(&p, &r, &mut synth, &Tempo::default(), 48_000, 0);
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

    #[test]
    fn a_committed_take_undoes_as_one_action_via_apply_group() {
        // commit_ops predates apply_group in this codebase's own history —
        // this checks the two actually compose correctly, not just that
        // each works in isolation. A real "record a verse" flow (see
        // studio-copilot's set_up_a_verse) ends with exactly this: apply
        // the take's ops as one group, so one undo removes both the source
        // and the clip together, matching what the user perceived doing
        // (one take, not two edits).
        let mut project = studio_core::Project { name: "t".into(), ..Default::default() };
        project.apply(&Op::AddTrack { track: studio_core::Track {
            id: TrackId(2), kind: studio_core::TrackKind::Audio, name: "Vox".into(),
            muted: false, locked: false, route: None,
        }, at: 0 }).unwrap();
        let before = project.clone();

        let mut rec = Recorder::new(48_000, 2, Ticks::from_seconds(8.0));
        rec.arm(true);
        rec.feed(&[0.25; 96_000]);
        let ops = rec.commit_ops(SourceId(7), ClipId(70), TrackId(2), "takes/take-1.wav");

        let mut history = studio_core::History::default();
        history.apply_group(&mut project, &ops).expect("a fresh take's ops should apply cleanly as a group");
        assert_eq!(project.sources.len(), 1);
        assert_eq!(project.clips.len(), 1);

        let n = history.undo_group(&mut project);
        assert_eq!(n, 2, "the source and the clip together — one action, two ops, undone as one");
        assert_eq!(project, before, "undo_group should restore the project to exactly its pre-take state");
    }

    fn sub_hz_of(samples: &[f32], rate: u32) -> f32 {
        // Zero-crossing period estimate over the given window — good enough
        // for a synthesized sine with no other partials in it.
        let mut crossings = Vec::new();
        for i in 1..samples.len() {
            if samples[i - 1] <= 0.0 && samples[i] > 0.0 {
                crossings.push(i);
            }
        }
        if crossings.len() < 2 {
            return 0.0;
        }
        let periods: Vec<f32> = crossings.windows(2).map(|w| (w[1] - w[0]) as f32).collect();
        let mean_period = periods.iter().sum::<f32>() / periods.len() as f32;
        rate as f32 / mean_period
    }

    /// Recompute the exact expected waveform independently of the
    /// implementation's control flow, from the same documented formula
    /// (phase integrated every sample, exponential/log-frequency
    /// interpolation during the glide, exponential amplitude decay). This is
    /// a stronger check than a statistical pitch estimate: it pins the
    /// literal sample values, not just "roughly the right frequency."
    fn expected_sub_samples(rate: u32, target_hz: f32, start_hz: f32, glide_samples: usize, tau: f32, n: usize) -> Vec<f32> {
        let mut phase = 0.0f64;
        let mut out = vec![0.0f32; n];
        for (i, s) in out.iter_mut().enumerate() {
            let hz = if glide_samples == 0 || i >= glide_samples {
                target_hz
            } else {
                let t = i as f32 / glide_samples as f32;
                start_hz * (target_hz / start_hz).powf(t)
            };
            phase += std::f64::consts::TAU * hz as f64 / rate as f64;
            let t_s = i as f32 / rate as f32;
            let env = (-(t_s as f64 / tau.max(1e-6) as f64)).exp() as f32;
            *s = phase.sin() as f32 * env;
        }
        out
    }

    fn sub_tau(decay_ms: f32) -> f32 {
        (decay_ms.max(1.0) / 1000.0) / 6.907_755
    }

    #[test]
    fn sub_freq_matches_standard_octave_steps_from_a1() {
        assert!((sub_freq(0) - SUB_REFERENCE_HZ).abs() < 1e-3, "note 0 is the reference itself");
        assert!((sub_freq(12) - 110.0).abs() < 0.1, "an octave up from A1 is A2, 110Hz");
        assert!((sub_freq(-12) - 27.5).abs() < 0.1, "an octave down is A0, 27.5Hz");
    }

    #[test]
    fn a_sub_hit_with_no_glide_holds_its_target_pitch_throughout() {
        let rate = 48_000;
        let kind = SynthKind::Sub { note: 12, glide_from: None, glide_ms: 0.0, drive: 0.0, decay_ms: 2000.0 };
        let mut out = vec![0.0f32; 4800]; // 100ms
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        synth.render(&kind, rate, &mut out);
        let hz = sub_hz_of(&out[..2400], rate); // first 50ms, well within a slow decay
        assert!((hz - sub_freq(12)).abs() < 1.0, "no glide requested: pitch should be A2 (110Hz) throughout, measured {hz}");
    }

    #[test]
    fn glide_matches_the_documented_closed_form_exactly() {
        let rate = 48_000;
        let target_hz = sub_freq(12);
        let start_hz = sub_freq(0);
        let glide_ms = 50.0;
        let decay_ms = 2000.0;
        let glide_samples = ((glide_ms / 1000.0) * rate as f32).round() as usize;
        let n = 9600;
        let kind = SynthKind::Sub { note: 12, glide_from: Some(0), glide_ms, drive: 0.0, decay_ms };
        let mut out = vec![0.0f32; n];
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        synth.render(&kind, rate, &mut out);

        let expected = expected_sub_samples(rate, target_hz, start_hz, glide_samples, sub_tau(decay_ms), n);
        for i in 0..n {
            assert!((out[i] - expected[i]).abs() < 1e-4,
                "sample {i}: got {} expected {} — the implementation drifted from its documented formula", out[i], expected[i]);
        }
    }

    #[test]
    fn glide_produces_no_phase_discontinuity() {
        // A model-independent check, unlike the closed-form test above: a
        // phase RESET (the classic glide-click bug) would blow past the
        // derivative bound of the fastest pure tone involved, regardless of
        // what formula produced it. This is what would catch a regression
        // that broke phase continuity while still matching some formula.
        let rate = 48_000;
        let kind = SynthKind::Sub { note: 12, glide_from: Some(0), glide_ms: 50.0, drive: 0.0, decay_ms: 2000.0 };
        let mut out = vec![0.0f32; 9600];
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        synth.render(&kind, rate, &mut out);

        let max_hz = sub_freq(12).max(SUB_REFERENCE_HZ);
        let bound = 2.0 * std::f32::consts::PI * max_hz / rate as f32 * 1.5; // 1.5x slack for the envelope's own slope
        for w in out.windows(2) {
            let d = (w[1] - w[0]).abs();
            assert!(d <= bound + 0.02, "sample-to-sample jump {d} exceeds the continuity bound {bound} — looks like a phase reset");
        }
    }

    #[test]
    fn a_sub_hit_with_glide_from_the_same_note_is_flat_not_degenerate() {
        // glide_from == note (ratio 1.0 in the exponential interpolation) is
        // a real input a caller could send (e.g. a step-mod layer that
        // always fills glide_from and only sometimes actually changes pitch)
        // and must not divide-by-zero or NaN out.
        let rate = 48_000;
        let kind = SynthKind::Sub { note: 3, glide_from: Some(3), glide_ms: 30.0, drive: 0.0, decay_ms: 500.0 };
        let mut out = vec![0.0f32; 2400];
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        synth.render(&kind, rate, &mut out);
        assert!(out.iter().all(|s| s.is_finite()), "must not NaN when start and target pitch are identical");

        let no_glide = SynthKind::Sub { note: 3, glide_from: None, glide_ms: 0.0, drive: 0.0, decay_ms: 500.0 };
        let mut reference = vec![0.0f32; 2400];
        synth.render(&no_glide, rate, &mut reference);
        for i in 0..out.len() {
            assert!((out[i] - reference[i]).abs() < 1e-4,
                "glide_from == note should render identically to no glide at all (sample {i})");
        }
    }

    #[test]
    fn the_amplitude_envelope_decays_toward_silence_by_decay_ms() {
        let rate = 48_000;
        let decay_ms = 100.0;
        let kind = SynthKind::Sub { note: 0, glide_from: None, glide_ms: 0.0, drive: 0.0, decay_ms };
        let mut out = vec![0.0f32; 24_000]; // 500ms, well past the decay
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        synth.render(&kind, rate, &mut out);

        let peak = |s: &[f32]| s.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        let early = peak(&out[0..480]); // first 10ms
        let at_decay = peak(&out[4700..4800]); // right around t=decay_ms (4800 samples)
        assert!(at_decay < early * 0.05, "amplitude at decay_ms ({at_decay}) should have fallen to roughly -60dB of the onset ({early})");
    }

    #[test]
    fn drive_audibly_changes_the_output_and_never_produces_nan() {
        let rate = 48_000;
        let clean = SynthKind::Sub { note: 0, glide_from: None, glide_ms: 0.0, drive: 0.0, decay_ms: 1000.0 };
        let driven = SynthKind::Sub { note: 0, glide_from: None, glide_ms: 0.0, drive: 0.9, decay_ms: 1000.0 };
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };

        let mut a = vec![0.0f32; 4800];
        let mut b = vec![0.0f32; 4800];
        synth.render(&clean, rate, &mut a);
        synth.render(&driven, rate, &mut b);

        assert!(b.iter().all(|s| s.is_finite()), "a driven 808 must never produce NaN/Inf");
        let differs = a.iter().zip(b.iter()).any(|(x, y)| (x - y).abs() > 1e-4);
        assert!(differs, "drive > 0 should audibly change the waveform vs drive = 0");

        // The saturator's own DC blocker should keep a driven signal centred
        // near zero rather than drifting — a light integration check, since
        // saturation.rs already tests the blocker itself exhaustively.
        let mean: f32 = b[2400..].iter().sum::<f32>() / (b.len() - 2400) as f32;
        assert!(mean.abs() < 0.05, "driven output has drifted off zero: mean {mean}");
    }

    #[test]
    fn render_pattern_plays_a_sub_channel_end_to_end() {
        let mut rack = rack();
        rack.channels.push(Channel {
            name: "808".into(),
            kind: SynthKind::Sub { note: -5, glide_from: None, glide_ms: 0.0, drive: 0.3, decay_ms: 300.0 },
            gain: 1.0, pan: 0.0, muted: false,
        });
        let mut pattern = Pattern::new("beat", 16, rack.channels.len());
        pattern.toggle(3, 0); // the new 808 channel, on step 0
        let tempo = Tempo { bpm: 140.0, steps_per_beat: 4, swing: 0.0 };
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        let out = render_pattern(&pattern, &rack, &mut synth, &tempo, 48_000, 0);
        assert!(out.iter().any(|s| *s != 0.0), "a pattern with a Sub hit must produce audible output");
    }

    #[test]
    fn bounce_ops_declares_a_source_length_matching_render_patterns_actual_output() {
        let rack = rack();
        let mut pattern = Pattern::new("beat", 16, rack.channels.len());
        pattern.toggle(0, 0);
        pattern.toggle(1, 4);
        let tempo = Tempo { bpm: 128.0, steps_per_beat: 4, swing: 0.0 };
        let rate = 48_000;
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        let rendered = render_pattern(&pattern, &rack, &mut synth, &tempo, rate, 0);

        let ops = bounce_ops(&pattern, &tempo, rate, SourceId(9), ClipId(9), TrackId(1), Ticks::ZERO, "bounce.wav");
        let Op::AddSource { source } = &ops[0] else { panic!("expected AddSource first") };
        assert_eq!(source.len.to_samples(rate) as usize, rendered.len(),
            "the declared source length must match render_pattern's actual sample count exactly, or a later mixdown could read out of bounds or truncate");
        assert_eq!(source.channels, 1, "render_pattern is mono");
    }

    #[test]
    fn bounce_ops_apply_cleanly_and_land_a_playable_clip() {
        let rack = rack();
        let mut pattern = Pattern::new("beat", 16, rack.channels.len());
        pattern.toggle(0, 0);
        let tempo = Tempo::default();

        let mut project = studio_core::Project { name: "t".into(), ..Default::default() };
        project.apply(&Op::AddTrack { track: studio_core::Track {
            id: TrackId(1), kind: studio_core::TrackKind::Audio, name: "Drums".into(),
            muted: false, locked: false, route: None,
        }, at: 0 }).unwrap();

        let ops = bounce_ops(&pattern, &tempo, 48_000, SourceId(1), ClipId(1), TrackId(1), Ticks::from_seconds(2.0), "bounce.wav");
        for op in ops {
            project.apply(&op).expect("bounce ops should apply cleanly onto a project with the target track already present");
        }
        let clips = project.track_clips(TrackId(1));
        assert_eq!(clips.len(), 1);
        assert_eq!(clips[0].pos, Ticks::from_seconds(2.0), "the clip should land at the requested position, not always at the timeline start");
    }

    #[test]
    fn a_bounced_pattern_is_audible_through_a_real_mixdown() {
        // The full loop: render the pattern's actual samples, bounce its ops
        // onto a project, register the rendered audio under the same
        // SourceId the ops reference, and mix down — proving the bounce
        // path produces something that is not just structurally valid but
        // genuinely plays.
        let rack = rack();
        let mut pattern = Pattern::new("beat", 16, rack.channels.len());
        pattern.toggle(0, 0);
        pattern.toggle(0, 8);
        let tempo = Tempo { bpm: 128.0, steps_per_beat: 4, swing: 0.0 };
        let rate = 48_000;
        let sources_for_render = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources_for_render };
        let rendered = render_pattern(&pattern, &rack, &mut synth, &tempo, rate, 0);
        assert!(rendered.iter().any(|s| *s != 0.0), "sanity: the pattern itself should be audible before bouncing it");

        let mut project = studio_core::Project { name: "t".into(), ..Default::default() };
        project.apply(&Op::AddTrack { track: studio_core::Track {
            id: TrackId(1), kind: studio_core::TrackKind::Audio, name: "Drums".into(),
            muted: false, locked: false, route: None,
        }, at: 0 }).unwrap();
        for op in bounce_ops(&pattern, &tempo, rate, SourceId(1), ClipId(1), TrackId(1), Ticks::ZERO, "bounce.wav") {
            project.apply(&op).unwrap();
        }

        let mut audio_map = std::collections::BTreeMap::new();
        audio_map.insert(SourceId(1), SourceAudio { sample_rate: rate, channels: vec![rendered.clone()] });

        let clip_len = Ticks::from_samples(rendered.len() as i64, rate);
        let mix = studio_audio::mixdown(&project, &audio_map, &std::collections::BTreeMap::new(),
            Ticks::ZERO, clip_len, rate, &[], &[]);
        assert!(mix.iter().any(|s| *s != 0.0), "a bounced pattern, mixed down through the normal timeline path, must be audible");
    }

    #[test]
    fn a_default_step_mod_is_indistinguishable_from_no_entry_and_stays_sparse() {
        let mut p = Pattern::new("beat", 16, 2);
        assert_eq!(p.mod_at(0, 3), StepMod::default(), "an untouched step reads as all-defaults");
        p.set_mod(0, 3, StepMod { vel: 0.5, ..StepMod::default() });
        assert_eq!(p.mods.len(), 1);
        p.set_mod(0, 3, StepMod::default());
        assert!(p.mods.is_empty(), "setting a step back to all-defaults must remove its sparse entry");
    }

    #[test]
    fn apply_step_mod_offsets_a_subs_note_and_leaves_others_untouched() {
        let sub = SynthKind::Sub { note: 3, glide_from: None, glide_ms: 40.0, drive: 0.2, decay_ms: 300.0 };
        let m = StepMod { tune: 5, ..StepMod::default() };
        match apply_step_mod(&sub, &m, None) {
            SynthKind::Sub { note, glide_from, glide_ms, drive, decay_ms } => {
                assert_eq!(note, 8, "3 + 5 semitones");
                assert_eq!(glide_from, None, "slide was false: no glide requested");
                assert_eq!(glide_ms, 40.0);
                assert_eq!(drive, 0.2);
                assert_eq!(decay_ms, 300.0);
            }
            other => panic!("unexpected kind: {other:?}"),
        }

        // Sampler has no pitch parameter tune could act on — untouched.
        let sampler = SynthKind::Sampler { source: SourceId(1) };
        assert_eq!(apply_step_mod(&sampler, &m, None), sampler);
    }

    #[test]
    fn apply_step_mod_scales_a_sines_hz_by_the_tune_offset() {
        let sine = SynthKind::Sine { hz: 440.0 };
        let up_an_octave = StepMod { tune: 12, ..StepMod::default() };
        match apply_step_mod(&sine, &up_an_octave, None) {
            SynthKind::Sine { hz } => assert!((hz - 880.0).abs() < 0.01, "up 12 semitones doubles the frequency, got {hz}"),
            other => panic!("unexpected kind: {other:?}"),
        }
    }

    #[test]
    fn slide_glides_from_the_previous_hit_but_only_when_requested() {
        let sub = SynthKind::Sub { note: 7, glide_from: None, glide_ms: 40.0, drive: 0.0, decay_ms: 300.0 };
        let no_slide = StepMod::default();
        let with_slide = StepMod { slide: true, ..StepMod::default() };

        match apply_step_mod(&sub, &no_slide, Some(2)) {
            SynthKind::Sub { glide_from, .. } => assert_eq!(glide_from, None, "slide=false must ignore a previous note even if one is known"),
            _ => unreachable!(),
        }
        match apply_step_mod(&sub, &with_slide, Some(2)) {
            SynthKind::Sub { glide_from, .. } => assert_eq!(glide_from, Some(2), "slide=true should carry the previous note forward"),
            _ => unreachable!(),
        }
        match apply_step_mod(&sub, &with_slide, None) {
            SynthKind::Sub { glide_from, .. } => assert_eq!(glide_from, None, "nothing to slide from on a channel's first hit — silently a no-op, not an error"),
            _ => unreachable!(),
        }
    }

    #[test]
    fn render_pattern_carries_slide_across_consecutive_hits_on_the_same_channel() {
        // Two hits on one Sub channel: step 0 at note 0, step 4 slides to
        // note 12. The second hit's rendered pitch must glide FROM the first
        // hit's ACTUAL note (0), not from the channel's base SynthKind note
        // — proving render_pattern tracks last_note per channel correctly.
        let rack = Rack {
            channels: vec![Channel {
                name: "808".into(),
                kind: SynthKind::Sub { note: 0, glide_from: None, glide_ms: 30.0, drive: 0.0, decay_ms: 2000.0 },
                gain: 1.0, pan: 0.0, muted: false,
            }],
            patterns: vec![], tempo_bpm: 120.0, swing: 0.0,
        };
        let mut pattern = Pattern::new("beat", 16, 1);
        pattern.toggle(0, 0);
        pattern.toggle(0, 4);
        pattern.set_mod(0, 4, StepMod { tune: 12, slide: true, ..StepMod::default() });
        let tempo = Tempo { bpm: 120.0, steps_per_beat: 4, swing: 0.0 };
        let rate = 48_000;
        let step_samples = tempo.step_len().to_samples(rate) as usize;

        // Render just the second hit's kind by re-deriving it exactly the
        // way render_pattern does, and compare against a synthesized full
        // pattern render at the sample level for the slid hit's onset.
        let expected_kind = SynthKind::Sub { note: 12, glide_from: Some(0), glide_ms: 30.0, drive: 0.0, decay_ms: 2000.0 };
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        let mut expected_scratch = vec![0.0f32; step_samples * 4];
        synth.render(&expected_kind, rate, &mut expected_scratch);

        let out = render_pattern(&pattern, &rack, &mut synth, &tempo, rate, 0);
        let onset = 4 * step_samples;
        for i in 0..2000.min(expected_scratch.len()).min(out.len() - onset) {
            assert!((out[onset + i] - expected_scratch[i]).abs() < 1e-4,
                "sample {i} after the slid hit's onset: render_pattern's slide did not glide from the first hit's actual note");
        }
    }

    #[test]
    fn ratchet_subdivides_a_step_into_evenly_spaced_sample_accurate_retriggers() {
        let mut rack = rack();
        rack.channels[0].kind = SynthKind::Sine { hz: 4000.0 };
        let mut pattern = Pattern::new("beat", 4, rack.channels.len());
        pattern.toggle(0, 0);
        pattern.set_mod(0, 0, StepMod { ratchet: 4, ..StepMod::default() });
        let tempo = Tempo { bpm: 120.0, steps_per_beat: 4, swing: 0.0 };
        let rate = 48_000;
        let step_samples = tempo.step_len().to_samples(rate) as usize;
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        let out = render_pattern(&pattern, &rack, &mut synth, &tempo, rate, 0);

        let sub_step = step_samples / 4;
        let mut reference = vec![0.0f32; step_samples * 4]; // must match render_pattern's own voice_len exactly
        synth.render(&SynthKind::Sine { hz: 4000.0 }, rate, &mut reference);

        // First onset: no overlap from any earlier retrigger yet, so this
        // segment must match a one-off render exactly — the strongest,
        // least-derived check available.
        for i in 0..sub_step {
            assert!((out[i] - reference[i]).abs() < 1e-4,
                "ratchet 0, offset {i}: expected a fresh onset matching a one-off render, got {} vs {}", out[i], reference[i]);
        }

        // Later onsets legitimately overlap with the ringing tail of earlier
        // retriggers (a voice may ring past its own step, by design), so
        // reconstruct the additive sum the same way render_pattern does and
        // compare the WHOLE buffer, rather than assuming any later segment
        // is isolated.
        let mut expected = vec![0.0f32; out.len()];
        for r in 0..4 {
            let onset = r * sub_step;
            for (i, v) in reference.iter().enumerate() {
                if onset + i >= expected.len() { break; }
                expected[onset + i] += v;
            }
        }
        for i in 0..out.len() {
            assert!((out[i] - expected[i]).abs() < 1e-4,
                "sample {i}: ratchet output does not match 4 evenly-spaced (every {sub_step} samples), fully additive retriggers");
        }
    }

    #[test]
    fn ratchet_is_clamped_into_range_rather_than_refused() {
        let mut rack = rack();
        rack.channels[0].kind = SynthKind::Sine { hz: 2000.0 };
        let mut pattern = Pattern::new("beat", 4, rack.channels.len());
        pattern.toggle(0, 0);
        pattern.set_mod(0, 0, StepMod { ratchet: 200, ..StepMod::default() }); // absurd, must clamp not panic
        let tempo = Tempo::default();
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        let out = render_pattern(&pattern, &rack, &mut synth, &tempo, 48_000, 0);
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn velocity_scales_the_hit_relative_to_the_channels_own_gain() {
        let mut rack = rack();
        rack.channels[0].kind = SynthKind::Sine { hz: 1000.0 };
        rack.channels[0].gain = 1.0;
        let mut full = Pattern::new("beat", 4, rack.channels.len());
        full.toggle(0, 0);
        let mut half = full.clone();
        half.set_mod(0, 0, StepMod { vel: 0.5, ..StepMod::default() });

        let tempo = Tempo::default();
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        let a = render_pattern(&full, &rack, &mut synth, &tempo, 48_000, 0);
        let b = render_pattern(&half, &rack, &mut synth, &tempo, 48_000, 0);
        let peak = |s: &[f32]| s.iter().fold(0.0f32, |acc, v| acc.max(v.abs()));
        assert!((peak(&b) - peak(&a) * 0.5).abs() < 1e-4, "vel=0.5 should exactly halve the hit's amplitude");
    }

    #[test]
    fn probability_zero_never_fires_and_one_always_does() {
        let mut rack = rack();
        rack.channels[0].kind = SynthKind::Sine { hz: 1000.0 };
        let mut never = Pattern::new("beat", 4, rack.channels.len());
        never.toggle(0, 0);
        never.set_mod(0, 0, StepMod { prob: 0.0, ..StepMod::default() });
        let mut always = never.clone();
        always.set_mod(0, 0, StepMod { prob: 1.0, ..StepMod::default() });

        let tempo = Tempo::default();
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        for pass in 0..10 {
            let out = render_pattern(&never, &rack, &mut synth, &tempo, 48_000, pass);
            assert!(out.iter().all(|s| *s == 0.0), "prob=0.0 must never fire (pass {pass})");
        }
        let out = render_pattern(&always, &rack, &mut synth, &tempo, 48_000, 0);
        assert!(out.iter().any(|s| *s != 0.0), "prob=1.0 must always fire");
    }

    #[test]
    fn probability_is_deterministic_per_pass_and_roughly_matches_its_rate_across_many() {
        let mut rack = rack();
        rack.channels[0].kind = SynthKind::Sine { hz: 1000.0 };
        let mut pattern = Pattern::new("beat", 4, rack.channels.len());
        pattern.toggle(0, 0);
        pattern.set_mod(0, 0, StepMod { prob: 0.5, ..StepMod::default() });
        let tempo = Tempo::default();
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };

        // Same pass rendered twice must agree exactly — determinism, not
        // "randomness" in the usual sense.
        let a = render_pattern(&pattern, &rack, &mut synth, &tempo, 48_000, 7);
        let b = render_pattern(&pattern, &rack, &mut synth, &tempo, 48_000, 7);
        assert_eq!(a, b, "the same (pattern, pass) must render identically every time");

        let fired = (0..200)
            .filter(|&pass| render_pattern(&pattern, &rack, &mut synth, &tempo, 48_000, pass).iter().any(|s| *s != 0.0))
            .count();
        assert!((60..140).contains(&fired), "prob=0.5 across 200 passes should land near 100 fires, got {fired}");
    }

    #[test]
    fn every_gates_a_hit_to_specific_passes() {
        let mut rack = rack();
        rack.channels[0].kind = SynthKind::Sine { hz: 1000.0 };
        let mut even_only = Pattern::new("beat", 4, rack.channels.len());
        even_only.toggle(0, 0);
        even_only.set_mod(0, 0, StepMod { every: Some((0, 2)), ..StepMod::default() });
        let tempo = Tempo::default();
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };

        for pass in 0..6u32 {
            let out = render_pattern(&even_only, &rack, &mut synth, &tempo, 48_000, pass);
            let fired = out.iter().any(|s| *s != 0.0);
            assert_eq!(fired, pass % 2 == 0, "pass {pass}: every=(0,2) should fire only on even passes");
        }
    }
}

#[cfg(test)]
mod trap_tests {
    use super::*;

    fn render_voice(voice: DrumVoice, decay_ms: f32, len: usize) -> Vec<f32> {
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        let mut out = vec![0.0f32; len];
        synth.render(&SynthKind::Drum { voice, tone: 0.5, decay_ms, tune: 0 }, 48_000, &mut out);
        out
    }

    fn energy(buf: &[f32], from: usize, to: usize) -> f32 {
        buf[from.min(buf.len())..to.min(buf.len())].iter().map(|s| s.abs()).sum()
    }

    #[test]
    fn every_drum_voice_makes_sound_and_the_same_sound_every_time() {
        // The noise is a seeded PRNG, not a system RNG, so a bounce is a
        // reproducible artifact — render_pattern's own promise, extended to
        // the voices. A kit that renders differently per run makes every
        // test of a mix flaky and every re-bounce a different file.
        for v in [DrumVoice::Kick, DrumVoice::Snare, DrumVoice::Clap,
                  DrumVoice::HatClosed, DrumVoice::HatOpen] {
            let a = render_voice(v, 150.0, 4800);
            let b = render_voice(v, 150.0, 4800);
            assert!(energy(&a, 0, 4800) > 0.5, "{v:?} is audible");
            assert_eq!(a, b, "{v:?} renders deterministically");
        }
    }

    #[test]
    fn a_kick_starts_high_and_lands_low() {
        // The pitch drop is the kick. Count zero crossings early vs late:
        // ~200+Hz at the attack must cross more often than the ~50Hz tail.
        let k = render_voice(DrumVoice::Kick, 300.0, 48_000 / 5);
        let crossings = |from: usize, to: usize| k[from..to].windows(2)
            .filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count();
        let early = crossings(0, 1440);          // first 30ms
        let late = crossings(4320, 5760);        // 90..120ms
        assert!(early > late * 2, "early={early} late={late}: the sweep fell");
    }

    #[test]
    fn a_closed_hat_is_short_and_an_open_hat_rings() {
        let closed = render_voice(DrumVoice::HatClosed, 55.0, 48_000 / 2);
        let open = render_voice(DrumVoice::HatOpen, 320.0, 48_000 / 2);
        let after_150ms = 7200;
        let c = energy(&closed, after_150ms, closed.len());
        let o = energy(&open, after_150ms, open.len());
        assert!(o > c * 5.0, "open tail {o} vs closed tail {c}");
    }

    #[test]
    fn a_clap_is_several_hands_not_one() {
        // The 909 recipe: retriggered envelope over one noise stream. After
        // each pre-burst decays the next one must bring the level back UP —
        // a single-burst clap only ever falls.
        let c = render_voice(DrumVoice::Clap, 200.0, 4800);
        let ms = |t: f32| (t / 1000.0 * 48_000.0) as usize;
        let fall_1 = energy(&c, ms(8.0), ms(10.5));
        let rise_2 = energy(&c, ms(11.5), ms(14.0));
        let fall_2 = energy(&c, ms(19.0), ms(21.5));
        let rise_3 = energy(&c, ms(22.5), ms(25.0));
        assert!(rise_2 > fall_1 * 1.3, "second burst retriggers ({rise_2} vs {fall_1})");
        assert!(rise_3 > fall_2 * 1.3, "third burst retriggers ({rise_3} vs {fall_2})");
    }

    #[test]
    fn swing_delays_the_off_steps_and_leaves_the_beats_alone() {
        let rack = Rack {
            channels: vec![Channel { name: "hat".into(),
                kind: SynthKind::Drum { voice: DrumVoice::HatClosed, tone: 0.5,
                                        decay_ms: 40.0, tune: 0 },
                gain: 1.0, pan: 0.0, muted: false }],
            patterns: vec![], tempo_bpm: 120.0, swing: 0.0,
        };
        let mut p = Pattern::new("A", 4, 1);
        p.set(0, 0, true);
        p.set(0, 1, true);
        let sources = std::collections::BTreeMap::new();
        let straight = Tempo { bpm: 120.0, steps_per_beat: 4, swing: 0.0 };
        let swung = Tempo { swing: 0.5, ..straight };
        let mut synth = BuiltinSynth { sources: &sources };
        let a = render_pattern(&p, &rack, &mut synth, &straight, 48_000, 0);
        let mut synth = BuiltinSynth { sources: &sources };
        let b = render_pattern(&p, &rack, &mut synth, &swung, 48_000, 0);

        let step = straight.step_len().to_samples(48_000) as usize;
        let onset = |buf: &[f32], from: usize| buf[from..].iter()
            .position(|s| s.abs() > 1e-4).map(|i| i + from);
        // The downbeat does not move.
        assert_eq!(onset(&a, 0), onset(&b, 0));
        // The off-16th moves late by exactly half of half a step.
        let expected_delay = swung.swing_delay_samples(1, 48_000);
        assert_eq!(expected_delay, step / 4);
        assert_eq!(onset(&b, step), onset(&a, step).map(|i| i + expected_delay));
        // And swing never moves a hit EARLIER — that would be a different
        // (and wrong) feature wearing swing's name.
        assert_eq!(swung.swing_delay_samples(2, 48_000), 0);
    }

    #[test]
    fn the_trap_kit_renders_a_bar_out_of_the_box() {
        // Same argument as Studio::demo(): an empty grid gives a new user
        // nothing to press. The kit must be audible with zero assets loaded.
        let rack = Rack::trap_kit();
        assert!(rack.channels.iter().any(|c| matches!(c.kind, SynthKind::Sub { .. })),
                "a trap kit without an 808 is a drum kit");
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        let t = rack.tempo();
        let out = render_pattern(&rack.patterns[0], &rack, &mut synth, &t, 48_000, 0);
        let step = t.step_len().to_samples(48_000) as usize;
        assert_eq!(out.len(), step * 16);
        // Every quarter of the bar has audio in it — hats run through.
        for q in 0..4 {
            let e: f32 = out[q * step * 4..(q + 1) * step * 4].iter().map(|s| s.abs()).sum();
            assert!(e > 10.0, "quarter {q} is audible");
        }
    }

    #[test]
    fn a_rack_saved_before_swing_existed_still_loads_straight() {
        // serde(default) is a compatibility promise, so it gets a test, not
        // an assumption: the field's absence must mean 0, not a parse error.
        let old = r#"{"channels":[],"patterns":[],"tempo_bpm":140.0}"#;
        let rack: Rack = serde_json::from_str(old).expect("pre-swing rack loads");
        assert_eq!(rack.swing, 0.0);
        assert_eq!(rack.tempo().bpm, 140.0);
    }

    #[test]
    fn step_tune_reaches_a_drum_voice() {
        let base = SynthKind::Drum { voice: DrumVoice::Kick, tone: 0.5,
                                     decay_ms: 150.0, tune: 0 };
        let m = StepMod { tune: 3, ..StepMod::default() };
        match apply_step_mod(&base, &m, None) {
            SynthKind::Drum { tune, .. } => assert_eq!(tune, 3),
            other => panic!("stayed a drum, got {other:?}"),
        }
    }
}

#[cfg(test)]
mod import_tests {
    use super::*;

    #[test]
    fn adding_a_channel_grows_every_pattern_so_its_steps_actually_land() {
        let mut rack = Rack::trap_kit();
        rack.patterns.push(Pattern::new("B", 16, rack.channels.len()));
        let ch = rack.add_channel(Channel {
            name: "snap".into(),
            kind: SynthKind::Sampler { source: studio_core::SourceId(7) },
            gain: 0.9, pan: 0.0, muted: false,
        });
        for p in &rack.patterns {
            assert_eq!(p.rows.len(), rack.channels.len(), "'{}' kept up", p.name);
        }
        // The whole point: a toggle on the new channel is not a silent no-op.
        rack.patterns[0].toggle(ch, 5);
        assert!(rack.patterns[0].is_set(ch, 5));
    }

    #[test]
    fn a_sample_at_the_wrong_rate_plays_at_pitch_not_chipmunked() {
        // A 24k source rendered at 48k must stretch to twice the frames with
        // the same content, not play double-speed. A ramp makes it checkable
        // sample-by-sample: out[i] should sit at src[i/2].
        let ramp: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        let mut sources = std::collections::BTreeMap::new();
        sources.insert(studio_core::SourceId(1),
                       SourceAudio { sample_rate: 24_000, channels: vec![ramp] });
        let mut synth = BuiltinSynth { sources: &sources };
        let mut out = vec![0.0f32; 190];
        synth.render(&SynthKind::Sampler { source: studio_core::SourceId(1) }, 48_000, &mut out);
        assert!((out[100] - 0.50).abs() < 0.02, "halfway out is halfway up the ramp, got {}", out[100]);
        assert!((out[180] - 0.90).abs() < 0.02, "and it lasts twice the source frames");
        // And at a matching rate the fast path is untouched, bit for bit.
        let mut same = vec![0.0f32; 50];
        synth.render(&SynthKind::Sampler { source: studio_core::SourceId(1) }, 24_000, &mut same);
        assert_eq!(same[10], 10.0 / 100.0);
    }
}
