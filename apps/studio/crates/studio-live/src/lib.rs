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
        self.swing_delay_samples_at(step, rate, self.swing)
    }

    /// The same, at an explicit swing amount.
    ///
    /// Split out so a channel can swing differently from the kit — see
    /// [`Rack::channel_swing`]. Even steps are still never moved, whoever is
    /// asking: that rule is about the downbeat, not about who owns the knob.
    pub fn swing_delay_samples_at(&self, step: u32, rate: u32, swing: f32) -> usize {
        if step % 2 == 0 || swing <= 0.0 { return 0; }
        let step_samples = self.step_len().to_samples(rate).max(1) as f32;
        (swing.clamp(0.0, 1.0) * step_samples * 0.5) as usize
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
    /// A compact POLY synth: everything generated, nothing sampled -- a patch
    /// is ~60 bytes where a sample-library synth is gigabytes. Built for the
    /// imports first (a plugin synth's part becomes notes + a patch instead of
    /// a sine), and for a phone, where 50 GB of library is not an option.
    /// `hz` is the pitch at the pattern's origin, retuned per note like `Sine`.
    Poly { hz: f32, patch: PolyPatch },
    /// A plain sine — always available, so a rack is playable with no plugins.
    Sine { hz: f32 },
    /// A one-shot sample from a project source (the classic drum rack channel).
    Sampler { source: studio_core::SourceId },
    /// A SoundFont instrument: `bank:preset` out of an `.sf2` file.
    ///
    /// Racked like any other channel, so a pattern step plays a note on
    /// it exactly as it plays one on `Sub` — which is the point. An sf2
    /// is a multi-sampled instrument, and a pattern builder that can
    /// only reach one-shots and synthesised voices cannot write a piano
    /// part.
    ///
    /// DESCRIBED, not linked, the same way `Beatspace` is: the file path
    /// and the program number are project data, and rendering needs an
    /// sf2 engine this crate does not depend on. A host that has one
    /// registers a `Synth` and gets audio; a host without one gets
    /// silence and a project that still opens, edits and saves.
    ///
    /// That matters more here than for Beatspace. A soundfont lives on
    /// the operator's disk and travels badly — the project must survive
    /// opening on a machine where the file is missing, and it does,
    /// because nothing in the pattern depends on the sf2 being loadable.
    Sf2 {
        /// Where the file was when it was racked. Absolute: a soundfont
        /// is usually outside the project and a relative path would
        /// break the moment the project moved.
        path: std::path::PathBuf,
        /// General MIDI bank. 0 for melodic, 128 for percussion — the
        /// convention every sf2 follows.
        bank: u16,
        /// Program (patch) within the bank, 0..=127.
        preset: u8,
        /// Semitone offset from the pattern's note, like `Sub::note`.
        /// Kept HERE rather than baked into the pattern so the same
        /// pattern can drive two channels an octave apart.
        transpose: i8,
    },
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
    /// Channels sharing a group id choke each other: a new hit in the group
    /// cuts (with a ~2ms fade) whatever the group was still ringing — the
    /// 808's closed-hat-silences-open-hat rule, which is most of why a real
    /// hat line breathes. `None` (the default, and what any pre-choke rack
    /// deserializes to) never chokes or is choked. Hits that land on the
    /// same sample LAYER rather than choke — simultaneity is a chord, not a
    /// cut, and which one "wins" would otherwise be channel-order trivia.
    #[serde(default)]
    pub choke_group: Option<u8>,
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
    /// The piano roll: held notes per channel, sorted by (start, key).
    /// Sparse for the same reason `mods` is — most channels are drums and
    /// never grow one. `serde(default)` so a rack saved before the roll
    /// existed still loads, with the empty roll it effectively had.
    #[serde(default)]
    pub rolls: BTreeMap<usize, Vec<Note>>,
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
    /// [`SynthKind::Sub`] (added to `note`), [`SynthKind::Sine`] (scales
    /// `hz`) and [`SynthKind::Drum`]; for `Sampler` — which has no pitch
    /// parameter of its own — [`render_pattern`] repitches the rendered
    /// voice instead, so a chopped one-shot plays melodies. Only
    /// `Beatspace` remains a no-op: a latent position has no defined
    /// semitone.
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

/// One held note in a pattern's piano roll.
///
/// The step grid answers "which steps fire"; it cannot say "hold this for a
/// bar", and its per-step retune is one pitch per (channel, step) — no
/// chords. `Note` is the other half: a pitch held for a length, polyphonic
/// because a channel's roll is simply a list. A roll note renders through
/// the same placement pass as a step hit, so choke, swing, the bounce and
/// the collab rack sync all treat it as what it is — more pattern.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Note {
    /// Semitone offset, the same convention as [`SynthKind::Sub`]'s `note`
    /// and [`StepMod::tune`]: 0 is the channel's own pitch.
    pub key: i8,
    /// Step index the note starts on. A note starting at or past
    /// [`Pattern::steps`] is hidden, not deleted — the same rule the rows'
    /// bitmask follows, so shorten-then-lengthen round-trips.
    pub start: u32,
    /// Length in steps, >= 1. The note's END is its gate: the voice is cut
    /// there with a short fade, and a voice whose own envelope dies earlier
    /// (a short-decay 808 under a long note) simply ends earlier — the note
    /// bounds the ring, it does not stretch it.
    pub len: u32,
    /// 0..=1, scales the channel's gain for this note, like [`StepMod::vel`].
    pub vel: f32,
    /// Glide from the previous roll note on this channel (by start order)
    /// instead of starting at `key` directly — the 808 slide as a note
    /// property, which is where FL puts it. A channel's first roll note
    /// ignores it, same as [`StepMod::slide`].
    pub slide: bool,
}

/// The order an arpeggio runs its chord in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arp { Up, Down, UpDown }

impl Pattern {
    pub fn new(name: &str, steps: u32, channels: usize) -> Pattern {
        Pattern { name: name.into(), steps: steps.min(64), rows: vec![0; channels],
                  mods: BTreeMap::new(), rolls: BTreeMap::new() }
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
    /// Bjorklund's algorithm: `pulses` hits spread as evenly as `steps`
    /// allows, rotated by `rotate`. E(8,3) is the tresillo, E(16,5) the
    /// bossa bell, E(12,7) the West African bell — the rhythms a drummer
    /// reaches for first, one integer each. Returned as a row mask so it
    /// slots straight into `rows`.
    pub fn euclid(steps: u32, pulses: u32, rotate: u32) -> u64 {
        let steps = steps.clamp(1, 64);
        let pulses = pulses.min(steps);
        if pulses == 0 { return 0; }
        // Bjorklund's recursion proper (the SNS timing-system algorithm
        // Toussaint's paper is about): distribute the remainders level by
        // level. Bresenham gives the same necklace but a different first
        // hit, and E(8,3) is written x..x..x., not x..x.x.., so the spelling
        // matters.
        let mut counts: Vec<u32> = Vec::new();
        let mut remainders: Vec<u32> = vec![pulses];
        let mut divisor = steps - pulses;
        let mut level = 0usize;
        loop {
            counts.push(divisor / remainders[level]);
            remainders.push(divisor % remainders[level]);
            divisor = remainders[level];
            level += 1;
            if remainders[level] <= 1 { break; }
        }
        counts.push(divisor);
        fn build(level: i64, counts: &[u32], remainders: &[u32], out: &mut Vec<bool>) {
            if level == -1 { out.push(false); }
            else if level == -2 { out.push(true); }
            else {
                for _ in 0..counts[level as usize] { build(level - 1, counts, remainders, out); }
                if remainders[level as usize] != 0 { build(level - 2, counts, remainders, out); }
            }
        }
        let mut bits: Vec<bool> = Vec::with_capacity(steps as usize);
        build(level as i64, &counts, &remainders, &mut bits);
        bits.truncate(steps as usize);
        let mut mask = 0u64;
        for (i, b) in bits.iter().enumerate() { if *b { mask |= 1 << i; } }
        // Start on a hit, as every notation does.
        let first = mask.trailing_zeros();
        let width = steps;
        let rot = |m: u64, by: u32| -> u64 {
            let by = by % width;
            if by == 0 { return m; }
            let full = if width == 64 { u64::MAX } else { (1u64 << width) - 1 };
            ((m >> by) | (m << (width - by))) & full
        };
        let normalised = rot(mask, first);
        // then apply the caller's rotation (to the right: later steps)
        let by = rotate % width;
        if by == 0 { normalised } else { rot(normalised, width - by) }
    }

    /// Fill a channel's row with a euclidean rhythm, replacing what was there.
    pub fn set_euclid(&mut self, ch: usize, pulses: u32, rotate: u32) {
        if let Some(row) = self.rows.get_mut(ch) {
            *row = Self::euclid(self.steps, pulses, rotate);
        }
    }

    /// Humanise a channel: every hit gets a velocity within `amount` of 1
    /// (0.15 = the ±15% a hand gives), drawn from a hash of (seed, channel,
    /// step) so the same seed always plays the same take — a groove you can
    /// keep or re-roll, never a different one on every playback. Off-beat
    /// steps lean softer than on-beat ones, which is what a hand does.
    pub fn humanize(&mut self, ch: usize, amount: f32, seed: u32) {
        let amount = amount.clamp(0.0, 0.5);
        let Some(&row) = self.rows.get(ch) else { return };
        for step in 0..self.steps {
            if row & (1 << step) == 0 { continue; }
            let h = Self::hash3(seed, ch as u32, step);
            let r = (h % 10_000) as f32 / 10_000.0 * 2.0 - 1.0; // −1..1
            let lean = if step % 4 == 0 { 0.0 } else { -0.3 };
            let vel = (1.0 + amount * (r * 0.7 + lean)).clamp(0.2, 1.0);
            let mut m = self.mod_at(ch, step);
            m.vel = vel;
            self.mods.insert((ch, step), m);
        }
    }

    fn hash3(a: u32, b: u32, c: u32) -> u32 {
        let mut h = a.wrapping_mul(0x9E37_79B9) ^ b.wrapping_mul(0x85EB_CA6B) ^ c.wrapping_mul(0xC2B2_AE35);
        h ^= h >> 15; h = h.wrapping_mul(0x2C1B_3C6D); h ^= h >> 12; h
    }

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
    /// This channel's roll notes, sorted by (start, key). Empty for the
    /// common (drum) case.
    pub fn roll(&self, ch: usize) -> &[Note] {
        self.rolls.get(&ch).map(Vec::as_slice).unwrap_or(&[])
    }
    /// The roll note at (ch, key) covering `step`, if any — "is there ink
    /// under this cell", which is what a click on the roll needs to know.
    pub fn note_at(&self, ch: usize, key: i8, step: u32) -> Option<Note> {
        self.roll(ch).iter().copied()
            .find(|n| n.key == key && n.start <= step && step < n.start + n.len.max(1))
    }
    /// Add a note. A note with the same (start, key) is REPLACED, not
    /// duplicated — two identical placements are one intention, and a
    /// second add is how a velocity or length gets re-stated. `len` is
    /// clamped to >= 1 and the list stays sorted, which keeps the codec's
    /// output canonical: the same roll always encodes to the same bytes.
    pub fn add_note(&mut self, ch: usize, mut n: Note) {
        n.len = n.len.max(1);
        let notes = self.rolls.entry(ch).or_default();
        notes.retain(|m| !(m.start == n.start && m.key == n.key));
        notes.push(n);
        notes.sort_by_key(|m| (m.start, m.key));
    }
    /// Remove the note at (start, key). Returns whether one was there. An
    /// emptied channel drops its map entry, keeping `rolls` sparse the way
    /// `set_mod` keeps `mods` sparse.
    pub fn remove_note(&mut self, ch: usize, start: u32, key: i8) -> bool {
        let Some(notes) = self.rolls.get_mut(&ch) else { return false };
        let before = notes.len();
        notes.retain(|m| !(m.start == start && m.key == key));
        let removed = notes.len() != before;
        if notes.is_empty() { self.rolls.remove(&ch); }
        removed
    }
    /// Set the length of the note at (start, key), clamped to >= 1.
    pub fn resize_note(&mut self, ch: usize, start: u32, key: i8, len: u32) {
        if let Some(notes) = self.rolls.get_mut(&ch) {
            if let Some(n) = notes.iter_mut().find(|m| m.start == start && m.key == key) {
                n.len = len.max(1);
            }
        }
    }
    /// Set the velocity of EVERY note starting at `start` on this channel.
    /// A chord is one gesture — its notes struck together carry one
    /// dynamic, which is also the only unambiguous meaning a single lane
    /// bar over a stacked column can have.
    pub fn set_start_vel(&mut self, ch: usize, start: u32, vel: f32) {
        if let Some(notes) = self.rolls.get_mut(&ch) {
            for n in notes.iter_mut().filter(|n| n.start == start) {
                n.vel = vel.clamp(0.0, 1.0);
            }
        }
    }
    /// The loudest velocity among notes starting at `start`, if any — what
    /// a lane bar over that column draws.
    pub fn start_vel(&self, ch: usize, start: u32) -> Option<f32> {
        self.roll(ch).iter().filter(|n| n.start == start)
            .map(|n| n.vel).fold(None, |a, v| Some(a.map_or(v, |m: f32| m.max(v))))
    }
    /// Move the note at (start, key) to (to_start, to_key), keeping its
    /// length, velocity and slide. A note already at the destination is
    /// REPLACED (`add_note`'s rule: one placement, one note). Returns whether
    /// there was a note to move. One edit for a drag-and-drop, not a remove
    /// and an add a collaborator could see half of.
    pub fn move_note(&mut self, ch: usize, start: u32, key: i8, to_start: u32, to_key: i8) -> bool {
        let Some(n) = self.roll(ch).iter().copied().find(|m| m.start == start && m.key == key) else { return false };
        if (to_start, to_key) == (start, key) { return true; }
        self.remove_note(ch, start, key);
        self.add_note(ch, Note { start: to_start, key: to_key, ..n });
        true
    }
    /// Flip the slide flag of the note at (start, key).
    pub fn toggle_note_slide(&mut self, ch: usize, start: u32, key: i8) {
        if let Some(notes) = self.rolls.get_mut(&ch) {
            if let Some(n) = notes.iter_mut().find(|m| m.start == start && m.key == key) {
                n.slide = !n.slide;
            }
        }
    }
    /// The channel's chords: every start with two or more notes, with the
    /// notes at that start sorted low to high.
    pub fn chords(&self, ch: usize) -> Vec<(u32, Vec<Note>)> {
        let mut out: Vec<(u32, Vec<Note>)> = Vec::new();
        for n in self.roll(ch) {
            match out.last_mut() {
                Some((start, notes)) if *start == n.start => notes.push(*n),
                _ => out.push((n.start, vec![*n])),
            }
        }
        out.retain(|(_, notes)| notes.len() >= 2);
        for (_, notes) in out.iter_mut() { notes.sort_by_key(|n| n.key); }
        out
    }

    /// Harmonize: a copy of every note in the channel `degrees` scale
    /// degrees away (2 = a third, 4 = a fifth, 7 = an octave in a
    /// seven-note scale; negative for below), in `key` when the project
    /// has one, else in semitones (3rd = 4, 5th = 7, octave = 12 — the
    /// major intervals). Same start, length, velocity and slide; a note
    /// that would collide with an existing (start, key) is not added, so
    /// harmonizing twice does not stack. Returns how many were added.
    pub fn harmonize(&mut self, ch: usize, degrees: i32, key: Option<studio_core::Key>) -> usize {
        let existing: Vec<Note> = self.roll(ch).to_vec();
        let taken: std::collections::BTreeSet<(u32, i8)> = existing.iter().map(|n| (n.start, n.key)).collect();
        let mut added = 0;
        for n in existing {
            let k = match key {
                Some(k) => k.scale_step(n.key, degrees),
                None => {
                    // No key: the major-scale interval in semitones.
                    const MAJOR: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
                    let oct = degrees.div_euclid(7);
                    let deg = MAJOR[degrees.rem_euclid(7) as usize];
                    (n.key as i32 + oct * 12 + deg).clamp(i8::MIN as i32, i8::MAX as i32) as i8
                }
            };
            if taken.contains(&(n.start, k)) { continue; }
            self.add_note(ch, Note { key: k, ..n });
            added += 1;
        }
        added
    }

    /// Arpeggiate: every chord in the channel becomes a run of its notes,
    /// `note_len` steps each, cycling in `order` until the chord's span
    /// (its longest note) is filled. A single note is left alone.
    /// Returns how many chords were broken up.
    pub fn arpeggiate(&mut self, ch: usize, order: Arp, note_len: u32) -> usize {
        let note_len = note_len.max(1);
        let chords = self.chords(ch);
        for (start, notes) in &chords {
            let span = notes.iter().map(|n| n.len.max(1)).max().unwrap_or(1);
            for n in notes { self.remove_note(ch, *start, n.key); }
            let mut keys: Vec<i8> = notes.iter().map(|n| n.key).collect();
            match order {
                Arp::Up => {}
                Arp::Down => keys.reverse(),
                Arp::UpDown => {
                    // Up then back down without repeating the ends: C E G E.
                    let mut down: Vec<i8> = keys.iter().rev().skip(1).copied().collect();
                    down.pop();
                    keys.extend(down);
                }
            }
            let vel = notes.iter().map(|n| n.vel).fold(0.0f32, f32::max);
            let mut t = 0;
            let mut i = 0;
            while t < span {
                let len = note_len.min(span - t);
                self.add_note(ch, Note { key: keys[i % keys.len()], start: start + t, len, vel, slide: false });
                t += note_len;
                i += 1;
            }
        }
        chords.len()
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
    /// Per-channel swing overrides, by channel index. Absent = follow `swing`.
    ///
    /// ## Why a trap kit needs this
    ///
    /// The rack's swing moves every odd step on every channel, and for the
    /// genre this app is mostly used for that is wrong. You swing the HATS and
    /// keep the 808 dead straight: the hats' shuffle is what makes the beat
    /// breathe, and the same shuffle on the 808 makes the low end sound drunk,
    /// because a bassline that arrives late reads as a mistake rather than as
    /// feel. Producers do this by hand on hardware and every modern sequencer
    /// exposes it per track.
    ///
    /// `Some(0.0)` is meaningful and NOT the same as absent: it pins a channel
    /// straight while the kit swings, which is exactly the 808's setting.
    ///
    /// Sparse, and a map rather than a field on `Channel`, for the reason
    /// `Pattern.mods` gives: most channels want the kit's swing, so the common
    /// rack stores nothing. It also means the 39 `Channel` literals in this
    /// tree did not all have to be edited to add a feature most of them do not
    /// use.
    #[serde(default)]
    pub swing_overrides: BTreeMap<usize, f32>,
}

impl Rack {
    /// The swing a channel actually plays with.
    ///
    /// Its override if it has one, otherwise the kit's. This is the one place
    /// that resolves it, so a caller cannot read `rack.swing` directly and
    /// silently ignore the override — which is how a per-track feature ends up
    /// working in the sequencer and not in the bounce.
    pub fn channel_swing(&self, channel: usize) -> f32 {
        self.swing_overrides.get(&channel).copied().unwrap_or(self.swing)
    }

    /// Pin a channel straight while the kit swings — the 808's setting.
    pub fn set_channel_straight(&mut self, channel: usize) {
        self.swing_overrides.insert(channel, 0.0);
    }

    /// Give a channel back to the kit's swing.
    pub fn clear_channel_swing(&mut self, channel: usize) {
        self.swing_overrides.remove(&channel);
    }

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
            name: name.into(), kind, gain, pan: 0.0, muted: false, choke_group: None,
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
        let mut channels = channels;
        // The hats choke each other — the open hat breathes until the next
        // closed hat shuts it, which is most of what makes a hat line a line.
        channels[4].choke_group = Some(1);
        channels[5].choke_group = Some(1);
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
        Rack { channels, patterns: vec![p], tempo_bpm: 140.0, swing: 0.14, ..Default::default() }
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

    /// Render with a GATE, for a piano-roll note: the tonal kinds — `Sub`
    /// and `Sine` — sustain while `hold` samples last and release into the
    /// rest of `out`; everything else delegates to [`Synth::render`],
    /// because the sampler's window already IS its gate and a drum's decay
    /// is its identity. Default-implemented so a host `Synth` (a beatspace
    /// build, say) inherits held notes without writing envelope code.
    fn render_gated(&mut self, kind: &SynthKind, rate: u32, out: &mut [f32], hold: usize) {
        match kind {
            SynthKind::Sub { note, glide_from, glide_ms, drive, decay_ms } => {
                sub_voice(*note, *glide_from, *glide_ms, *drive, *decay_ms,
                          rate, out, Some(hold.min(out.len())));
            }
            SynthKind::Poly { hz, patch } => poly_voice(*hz, patch, rate, out, Some(hold.min(out.len()))),
            SynthKind::Sine { hz } => {
                let hold = hold.min(out.len()).max(1);
                let attack = (rate as usize / 500).max(1).min(hold);
                let release = (out.len() - hold).max(1);
                for (i, s) in out.iter_mut().enumerate() {
                    let t = i as f32 / rate as f32;
                    let env = if i < attack {
                        i as f32 / attack as f32
                    } else if i < hold {
                        1.0
                    } else {
                        // The hit path's cubic fade, applied to the release
                        // span: same shape a Sine hit already has, starting
                        // where the gate ends.
                        (1.0 - (i - hold) as f32 / release as f32).powf(3.0)
                    };
                    *s = (t * hz * std::f32::consts::TAU).sin() * env;
                }
            }
            _ => self.render(kind, rate, out),
        }
    }
}

/// A Poly patch. Every field a number: small on the wire, diffable, and
/// editable as knobs.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PolyPatch {
    /// 0 sine, 1 triangle, 2 saw, 3 square (saw and square band-limited).
    pub wave: u8,
    /// Unison voices, 1..=7, spread over `detune` cents.
    pub voices: u8,
    pub detune: f32,
    /// A sine an octave down, 0..=1.
    pub sub: f32,
    pub attack_ms: f32,
    pub decay_ms: f32,
    /// 0..=1
    pub sustain: f32,
    pub release_ms: f32,
    /// Low-pass cutoff (Hz) and resonance (0..=1).
    pub cutoff: f32,
    pub resonance: f32,
    /// How far the amp envelope opens the filter, in octaves.
    pub env_amount: f32,
    /// Vibrato depth, cents (5 Hz).
    pub vibrato: f32,
    pub gain: f32,
    /// TEXTURE (c20): a HARMONIC PROFILE -- the level of harmonics 1..=16,
    /// 0..=255. All zero: `wave` is the oscillator. Otherwise each note builds
    /// one band-limited cycle from these and plays it: the timbre, in 16 bytes,
    /// and FITTABLE from a render (studio-app's synth_fit).
    #[serde(default)]
    pub partials: [u8; 16],
    /// A noise layer (air, breath, grit), 0..=1, through the same filter.
    #[serde(default)]
    pub noise: f32,
    /// 2-operator FM: the modulator at `fm_ratio` x the note, depth `fm_index`
    /// (radians) -- metallic and bell edges. 0 = off.
    #[serde(default)]
    pub fm_ratio: f32,
    #[serde(default)]
    pub fm_index: f32,
    /// Saturation before the filter, 0..=1 -- thickness.
    #[serde(default)]
    pub drive: f32,
}

impl Default for PolyPatch {
    fn default() -> PolyPatch {
        PolyPatch { wave: 2, voices: 2, detune: 8.0, sub: 0.0, attack_ms: 5.0, decay_ms: 300.0, sustain: 0.7, release_ms: 250.0,
                    cutoff: 3000.0, resonance: 0.2, env_amount: 1.0, vibrato: 0.0, gain: 0.5,
                    partials: [0; 16], noise: 0.0, fm_ratio: 0.0, fm_index: 0.0, drive: 0.0 }
    }
}

/// PolyBLEP: the correction that band-limits a naive saw/square edge.
fn poly_blep(t: f32, dt: f32) -> f32 {
    if t < dt { let x = t / dt; x + x - x * x - 1.0 }
    else if t > 1.0 - dt { let x = (t - 1.0) / dt; x * x + x + x + 1.0 }
    else { 0.0 }
}

/// Render one Poly note into `out`: held for `hold` samples (the gate), then
/// released; `None` = a hit (the gate closes after attack + decay).
pub fn poly_voice(hz: f32, p: &PolyPatch, rate: u32, out: &mut [f32], hold: Option<usize>) {
    let r = rate as f32;
    let ms = |x: f32| ((x.max(0.0) * 0.001 * r) as usize).max(1);
    let (att, dec, rel) = (ms(p.attack_ms), ms(p.decay_ms), ms(p.release_ms));
    let gate = hold.unwrap_or(att + dec).min(out.len());
    let voices = p.voices.clamp(1, 7) as usize;
    let mut phase: Vec<f32> = (0..voices).map(|k| (k as f32 * 0.618_034).fract()).collect();
    let spread = |k: usize| if voices == 1 { 0.0 } else { (k as f32 / (voices - 1) as f32 - 0.5) * p.detune };
    let mut sub_phase = 0.0f32;
    // TPT state-variable low-pass
    let (mut ic1, mut ic2) = (0.0f32, 0.0f32);
    let k_res = 2.0 - 1.9 * p.resonance.clamp(0.0, 1.0);
    let norm = 1.0 / (voices as f32).sqrt();
    let mut level_at_gate = 0.0f32;
    // the additive cycle, if the patch has a harmonic profile: harmonics above
    // ~0.45 x the rate are left out (band-limited for THIS note), peak -> 1
    const TABLE: usize = 2048;
    let table: Option<Vec<f32>> = if p.partials.iter().any(|a| *a > 0) {
        let mut t = vec![0.0f32; TABLE];
        for (k, a) in p.partials.iter().enumerate() {
            let h = (k + 1) as f32;
            if *a == 0 || h * hz > 0.45 * r { continue; }
            let amp = *a as f32 / 255.0;
            for (i, v) in t.iter_mut().enumerate() { *v += amp * (std::f32::consts::TAU * h * i as f32 / TABLE as f32).sin(); }
        }
        let peak = t.iter().fold(0.0f32, |m, v| m.max(v.abs())).max(1e-6);
        for v in t.iter_mut() { *v /= peak; }
        Some(t)
    } else { None };
    let read = |t: &[f32], ph: f32| { let x = ph.rem_euclid(1.0) * TABLE as f32; let i = x as usize % TABLE; let f = x - x.floor();
                                     t[i] * (1.0 - f) + t[(i + 1) % TABLE] * f };
    let mut fm_phase = 0.0f32;
    let mut noise_state = 0x2545_f491_u32;
    let drive_k = 1.0 + p.drive.clamp(0.0, 1.0) * 4.0;
    for (i, s) in out.iter_mut().enumerate() {
        // envelope
        let env = if i < gate {
            let e = if i < att { i as f32 / att as f32 }
                    else { let d = (i - att) as f32 / dec as f32; p.sustain + (1.0 - p.sustain) * (1.0 - d.min(1.0)).powi(2) };
            level_at_gate = e;
            e
        } else {
            let d = (i - gate) as f32 / rel as f32;
            if d >= 1.0 { 0.0 } else { level_at_gate * (1.0 - d).powi(2) }
        };
        let vib = if p.vibrato > 0.0 { 2f32.powf(p.vibrato / 1200.0 * (i as f32 / r * 5.0 * std::f32::consts::TAU).sin()) } else { 1.0 };
        let mut x = 0.0f32;
        let fm = if p.fm_index > 0.0 && p.fm_ratio > 0.0 {
            let m = p.fm_index * (fm_phase * std::f32::consts::TAU).sin();
            fm_phase = (fm_phase + hz * p.fm_ratio / r).fract();
            m / std::f32::consts::TAU
        } else { 0.0 };
        for (k, ph) in phase.iter_mut().enumerate() {
            let f = hz * vib * 2f32.powf(spread(k) / 1200.0);
            let dt = (f / r).min(0.49);
            let v = if let Some(t) = &table { read(t, *ph + fm) } else if fm != 0.0 { (((*ph + fm) * std::f32::consts::TAU).sin()) } else { match p.wave {
                0 => (*ph * std::f32::consts::TAU).sin(),
                1 => 1.0 - 4.0 * (*ph - 0.5).abs(),
                3 => { let sq = if *ph < 0.5 { 1.0 } else { -1.0 }; sq + poly_blep(*ph, dt) - poly_blep((*ph + 0.5).fract(), dt) }
                _ => 2.0 * *ph - 1.0 - poly_blep(*ph, dt),
            } };
            x += v;
            *ph = (*ph + dt).fract();
        }
        x *= norm;
        if p.sub > 0.0 {
            x += p.sub * (sub_phase * std::f32::consts::TAU).sin();
            sub_phase = (sub_phase + hz * 0.5 / r).fract();
        }
        if p.noise > 0.0 {
            noise_state ^= noise_state << 13; noise_state ^= noise_state >> 17; noise_state ^= noise_state << 5;
            x += p.noise * (noise_state as f32 / u32::MAX as f32 * 2.0 - 1.0);
        }
        if p.drive > 0.0 { x = (x * drive_k).tanh() / drive_k.tanh(); }
        // filter, opened by the envelope
        let fc = (p.cutoff * 2f32.powf(p.env_amount * env)).clamp(20.0, 0.45 * r);
        let g = (std::f32::consts::PI * fc / r).tan();
        let a1 = 1.0 / (1.0 + g * (g + k_res));
        let v1 = a1 * (ic1 + g * (x - ic2));
        let v2 = ic2 + g * v1;
        ic1 = 2.0 * v1 - ic1;
        ic2 = 2.0 * v2 - ic2;
        *s = v2 * env * p.gain;
    }
}

/// The always-available synth: sine tones and sample playback. A rack made only
/// of these needs no plugins at all, which is what keeps a project openable.
pub struct BuiltinSynth<'a> {
    pub sources: &'a std::collections::BTreeMap<studio_core::SourceId, SourceAudio>,
}

impl Synth for BuiltinSynth<'_> {
    fn render(&mut self, kind: &SynthKind, rate: u32, out: &mut [f32]) {
        match kind {
            SynthKind::Poly { hz, patch } => poly_voice(*hz, patch, rate, out, None),
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
            SynthKind::Sf2 { .. } => {
                // Silence, for the same reason as Beatspace below: no sf2
                // engine is linked here. The distinction that matters is
                // that this silence is a MISSING RENDERER, not a missing
                // instrument — the channel, its transpose and every step
                // written against it are intact, so a project opened on
                // a machine without the soundfont still edits and saves
                // without losing the part.
                out.fill(0.0);
            }
            SynthKind::Beatspace { .. } => {
                // Silence, deliberately and audibly harmless: this build has no
                // beatspace model. A host that loads one registers its own Synth
                // and gets real audio; everything else about the project still
                // works, which is the point of describing rather than linking.
                out.fill(0.0);
            }
            SynthKind::Sub { note, glide_from, glide_ms, drive, decay_ms } => {
                sub_voice(*note, *glide_from, *glide_ms, *drive, *decay_ms,
                          rate, out, None);
            }
            SynthKind::Drum { voice, tone, decay_ms, tune } => {
                render_drum(*voice, tone.clamp(0.0, 1.0), decay_ms.max(1.0), *tune, rate, out);
            }
        }
    }
}

/// Note-off on a held Sub is a CUT with a musical fade, not a second
/// decay: 30ms to -60dB, short enough that "the note stops at its end"
/// stays true at any sane tempo, long enough never to click. The first
/// draft reused `decay_ms` as the release — one knob, both modes — and
/// the roll's own oldest pin refused it: a 2-step note on a 2-second 808
/// must stop at 2 steps, and the long ring-out is the HIT path's sound.
const SUB_RELEASE_MS: f32 = 30.0;

/// The Sub oscillator, shared by the hit path and the gated (piano-roll)
/// path. `gate: None` is the HIT envelope — exponential decay from t=0,
/// reaching -60dB by `decay_ms`, the 808 one-shot. `gate: Some(hold)` is
/// the HELD-NOTE envelope: ~2ms attack, sustain at unity while the gate
/// lasts, then the fixed [`SUB_RELEASE_MS`] release. Phase is integrated
/// and never reset, so the glide stays phase-continuous by construction
/// in both paths.
#[allow(clippy::too_many_arguments)]
fn sub_voice(note: i8, glide_from: Option<i8>, glide_ms: f32, drive: f32,
             decay_ms: f32, rate: u32, out: &mut [f32], gate: Option<usize>) {
    let target_hz = sub_freq(note);
    let start_hz = glide_from.map(sub_freq).unwrap_or(target_hz);
    let glide_samples = ((glide_ms / 1000.0) * rate as f32).round().max(0.0) as usize;
    let glide_samples = glide_samples.min(out.len());
    // -60dB by decay_ms: exp(-decay_ms/1000/tau) = 1/1000 →
    // tau = (decay_ms/1000) / ln(1000).
    let tau = (decay_ms.max(1.0) / 1000.0) / 6.907_755;
    let attack = gate.map(|h| ((rate as usize / 500).max(1)).min(h.max(1)));
    let mut phase = 0.0f64;
    for (i, s) in out.iter_mut().enumerate() {
        let hz = if glide_samples == 0 || i >= glide_samples {
            target_hz
        } else {
            // Log-frequency interpolation — a musical glide sweeps octaves
            // at a constant rate, not Hz at a constant rate.
            let t = i as f32 / glide_samples as f32;
            start_hz * (target_hz / start_hz).powf(t)
        };
        phase += std::f64::consts::TAU * hz as f64 / rate as f64;
        let env = match gate {
            None => {
                let t_s = i as f32 / rate as f32;
                (-(t_s as f64 / tau.max(1e-6) as f64)).exp() as f32
            }
            Some(hold) => {
                let a = attack.unwrap_or(1);
                if i < a {
                    i as f32 / a as f32
                } else if i < hold {
                    1.0
                } else {
                    let t_s = (i - hold) as f32 / rate as f32;
                    let rel_tau = (SUB_RELEASE_MS / 1000.0) / 6.907_755;
                    (-(t_s as f64 / rel_tau as f64)).exp() as f32
                }
            }
        };
        *s = phase.sin() as f32 * env;
    }
    if drive > 0.0 {
        let mut sat = studio_fx::saturation::Saturator::new(studio_fx::saturation::Profile::Tube);
        sat.drive = drive.clamp(0.0, 1.0);
        sat.process_block(out);
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
/// The step's modifiers folded into a voice. Public because a PLAYED note
/// is the same operation a sequenced one is — tune and velocity applied to
/// the channel's synth — and the shell must not grow a second, drifting copy
/// of that rule for its keyboard.
pub fn apply_step_mod(kind: &SynthKind, m: &StepMod, prev_note: Option<i8>) -> SynthKind {
    match kind {
        SynthKind::Sub { note, glide_ms, drive, decay_ms, .. } => SynthKind::Sub {
            note: note.saturating_add(m.tune),
            glide_from: if m.slide { prev_note } else { None },
            glide_ms: *glide_ms,
            drive: *drive,
            decay_ms: *decay_ms,
        },
        SynthKind::Sine { hz } => SynthKind::Sine { hz: hz * 2f32.powf(m.tune as f32 / 12.0) },
        SynthKind::Poly { hz, patch } => SynthKind::Poly { hz: hz * 2f32.powf(m.tune as f32 / 12.0), patch: *patch },
        // The roll's key and the step's tune fold into the transpose, so
        // a host's sf2 engine reads ONE number: the semitones above the
        // roll's origin (`studio_core::KEY_ZERO_MIDI`) to play. Before
        // this fold every note on an sf2 channel would have played the
        // same pitch.
        SynthKind::Sf2 { path, bank, preset, transpose } => SynthKind::Sf2 {
            path: path.clone(), bank: *bank, preset: *preset,
            transpose: transpose.saturating_add(m.tune),
        },
        SynthKind::Drum { voice, tone, decay_ms, tune } => SynthKind::Drum {
            voice: *voice, tone: *tone, decay_ms: *decay_ms,
            tune: tune.saturating_add(m.tune),
        },
        // Sampler and Beatspace have no pitch parameter `tune` or `slide`
        // could act on — left untouched here. The sampler is not a no-op
        // overall, though: [`render_pattern`] repitches its RENDERED voice
        // by the step's tune (see `Placed::pitch`), which is where a pitch
        // has to live for a synth whose kind carries none.
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
    // The note each channel most recently played, in step order — local to
    // this render, not the synth's business. See `apply_step_mod`'s docs.
    let mut last_note: BTreeMap<usize, i8> = BTreeMap::new();

    // ── pass 1: place ───────────────────────────────────────────────────
    // Decide WHERE every voice starts before rendering ANY of them, because
    // choke needs to know the group's next hit to know where this one ends —
    // a single interleaved pass would be adding tails it hasn't yet learned
    // are cut.
    struct Placed {
        at: usize,
        kind: SynthKind,
        amp: f32,
        group: Option<u8>,
        /// The voice's natural render window: `voice_len` for a step hit,
        /// the note's own length for a roll note. Choke may still shorten it.
        window: usize,
        /// A gated voice (a roll note) always fades over its window's last
        /// ~2ms — its end is a musical decision, and a decision must not
        /// click. A hit only fades when choke actually cut it.
        gate: bool,
        /// `Some(samples)`: a roll note on a TONAL kind — sustain this long,
        /// then release into the rest of the window (which placement sized
        /// to fit the tail). `None` everywhere else.
        hold: Option<usize>,
        /// Semitone repitch applied to the RENDERED voice — nonzero only for
        /// [`SynthKind::Sampler`], which has no pitch parameter of its own
        /// for `apply_step_mod` to act on. The kinds that understand `tune`
        /// pitch themselves; the sampler's output is resampled here instead,
        /// with the same linear quality stance the rate-mismatch path takes.
        pitch: i8,
    }
    let mut placed: Vec<Placed> = Vec::new();
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
        // A sampler has no pitch field for `apply_step_mod` to offset, so the
        // Tune lane's value rides to the render as a repitch of the voice's
        // output — the grid's pitch lane stops being a documented no-op for
        // sample channels.
        let pitch = if matches!(ch.kind, SynthKind::Sampler { .. }) { m.tune } else { 0 };
        for r in 0..ratchet {
            let this_kind = if r == 0 { kind.clone() } else { strip_glide(&kind) };
            placed.push(Placed {
                at: step_i as usize * step + swing + r * sub_step,
                kind: this_kind,
                amp: ch.gain * m.vel,
                group: ch.choke_group,
                window: voice_len,
                gate: false,
                hold: None,
                pitch,
            });
        }
    }

    // ── pass 1b: place the roll ─────────────────────────────────────────
    // Held notes join the same placement list as hits: swing moves them by
    // the same off-step rule, choke groups see them, and the bounce renders
    // them — a roll note is more pattern, not a second engine.
    for (&ch_i, notes) in &pattern.rolls {
        let Some(ch) = rack.channels.get(ch_i) else { continue };
        if ch.muted { continue; }
        // Glide chains through the channel's roll in start order (the list's
        // own order). Within a chord — several notes sharing a start — the
        // chain passes through its members in key order; sliding INTO a
        // chord is well-defined, sliding within one is not attempted.
        let mut prev: Option<i8> = None;
        for n in notes {
            if n.start >= pattern.steps { continue; } // hidden by a shortened pattern
            let m = StepMod { vel: n.vel, tune: n.key, slide: n.slide, ..StepMod::default() };
            let kind = apply_step_mod(&ch.kind, &m, prev);
            if let SynthKind::Sub { note, .. } = &kind { prev = Some(*note); }
            let swing = tempo.swing_delay_samples(n.start, rate);
            let pitch = if matches!(ch.kind, SynthKind::Sampler { .. }) { n.key } else { 0 };
            // The note's end is its gate; the tonal synths then RELEASE
            // past it (their tail sized from their own decay), the way a
            // hit already rings past its step — so the window is gate plus
            // tail for them and exactly the gate for everything else,
            // whose length semantics live elsewhere (the sampler's window
            // IS its gate).
            let gate_len = (n.len.max(1).min(pattern.steps.max(1)) as usize) * step;
            let (window, hold) = match &kind {
                SynthKind::Sub { .. } => {
                    // Room for the fixed release to reach silence: twice
                    // SUB_RELEASE_MS covers -120dB.
                    let tail = ((SUB_RELEASE_MS * 2.0 / 1000.0) * rate as f32) as usize;
                    (gate_len + tail, Some(gate_len))
                }
                SynthKind::Sine { .. } => (gate_len + rate as usize / 8, Some(gate_len)),
                _ => (gate_len, None),
            };
            placed.push(Placed {
                at: n.start as usize * step + swing,
                kind,
                amp: ch.gain * n.vel.clamp(0.0, 1.0),
                group: ch.choke_group,
                window,
                gate: true,
                hold,
                pitch,
            });
        }
    }

    // Scratch sized to the longest placed voice — a held note can want more
    // than a hit's four steps. Allocated after placement because only
    // placement knows the answer.
    let scratch_len = placed.iter().map(|p| p.window).max().unwrap_or(voice_len).max(1);
    let mut scratch = vec![0.0f32; scratch_len];
    let mut pitched = vec![0.0f32; scratch_len];

    // Every group's hit-start list, sorted, so "when does the group fire
    // next" is a partition-point lookup per voice.
    let mut group_starts: BTreeMap<u8, Vec<usize>> = BTreeMap::new();
    for p in &placed {
        if let Some(g) = p.group {
            group_starts.entry(g).or_default().push(p.at);
        }
    }
    for starts in group_starts.values_mut() {
        starts.sort_unstable();
    }

    // ── pass 2: render ──────────────────────────────────────────────────
    let fade = (rate as usize / 500).max(1); // ~2ms: cut, not click
    for p in &placed {
        if p.at >= out.len() { continue; }
        // What the voice would naturally occupy (its ring, clipped by the
        // buffer end) vs what choke allows. The synth ALWAYS renders its full
        // window: some voices (Sine's percussive envelope) shape themselves
        // to the buffer they're given, so "render into fewer samples" is a
        // different, faster sound than "render fully, cut at add time" — and
        // add-time cutting is what the pre-choke renderer did.
        let natural = p.window.min(out.len() - p.at);
        let mut allowed = natural;
        if let Some(starts) = p.group.and_then(|g| group_starts.get(&g)) {
            // First group start STRICTLY after this one — simultaneous group
            // hits layer (see `choke_group`'s docs), and a voice must not
            // choke itself at its own start index.
            let next = starts.partition_point(|&s| s <= p.at);
            if let Some(&cut_at) = starts.get(next) {
                allowed = allowed.min(cut_at - p.at);
            }
        }
        if allowed == 0 { continue; }
        // Render into the voice's OWN window, not the shared scratch's full
        // length: Sine shapes its envelope to the buffer it is given, so a
        // held sine note decays across the note rather than across whatever
        // the longest voice in the pattern happened to be.
        let sw = p.window.min(scratch.len());
        scratch[..sw].fill(0.0);
        match p.hold {
            Some(h) => synth.render_gated(&p.kind, rate, &mut scratch[..sw], h.min(sw)),
            None => synth.render(&p.kind, rate, &mut scratch[..sw]),
        }
        let voice: &[f32] = if p.pitch != 0 {
            // Repitch by reading the rendered voice at a fractional rate —
            // up consumes the material sooner (silence after), down means
            // the gate cuts what no longer fits. Linear interpolation, the
            // same stance the sampler's rate-mismatch path already takes.
            let ratio = 2f64.powf(p.pitch as f64 / 12.0);
            for (i, dst) in pitched[..sw].iter_mut().enumerate() {
                let pos = i as f64 * ratio;
                let i0 = pos as usize;
                *dst = if i0 + 1 >= sw { 0.0 } else {
                    let frac = (pos - i0 as f64) as f32;
                    scratch[i0] * (1.0 - frac) + scratch[i0 + 1] * frac
                };
            }
            &pitched[..sw]
        } else {
            &scratch[..sw]
        };
        let choked = allowed < natural;
        for (i, v) in voice[..allowed.min(sw)].iter().enumerate() {
            let mut v = *v;
            if (choked || p.gate) && allowed >= fade && i >= allowed - fade {
                // Linear ramp over the last ~2ms before the cut: choke's cut
                // and a note's gate must not click.
                v *= (allowed - i) as f32 / fade as f32;
            }
            out[p.at + i] += v * p.amp;
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
                sample_rate: rate, channels: 1, has_video: false, kind: studio_core::SourceKind::File,
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
                    sample_rate: self.rate, channels: self.channels, has_video: false, kind: studio_core::SourceKind::File,
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
    #[test]
    fn a_moved_note_keeps_its_length_velocity_and_slide_and_replaces_what_it_lands_on() {
        let mut p = Pattern::new("m", 16, 1);
        p.add_note(0, Note { key: 3, start: 2, len: 4, vel: 0.4, slide: true });
        p.add_note(0, Note { key: 5, start: 8, len: 1, vel: 1.0, slide: false });
        assert!(p.move_note(0, 2, 3, 8, 5));
        assert_eq!(p.roll(0), &[Note { key: 5, start: 8, len: 4, vel: 0.4, slide: true }]);
        assert!(!p.move_note(0, 2, 3, 0, 0), "nothing left at the old place");
        assert!(p.move_note(0, 8, 5, 8, 5), "a move to where it is, is a no-op that found the note");
        assert_eq!(p.roll(0).len(), 1);
    }

    use super::*;
    use studio_core::{ClipId, Op, SourceId, TrackId};

    fn rack() -> Rack {
        Rack {
            channels: vec![
                Channel { name: "kick".into(), kind: SynthKind::Sine { hz: 60.0 }, gain: 1.0, pan: 0.0, muted: false, choke_group: None },
                Channel { name: "hat".into(), kind: SynthKind::Sine { hz: 8000.0 }, gain: 0.6, pan: 0.0, muted: false, choke_group: None },
                Channel { name: "beatspace".into(),
                          kind: SynthKind::Beatspace { z: [0.1, -0.4, 0.0], crossfade: 0.5 },
                          gain: 1.0, pan: 0.0, muted: false, choke_group: None },
            ],
            patterns: vec![],
            tempo_bpm: 120.0, swing: 0.0, ..Default::default() }
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
            muted: false, locked: false, gain: 1.0, route: None,
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
            gain: 1.0, pan: 0.0, muted: false, choke_group: None,
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
    fn an_sf2_channel_hands_its_engine_the_roll_key_folded_into_the_transpose() {
        // No engine here; a spy synth records the kinds it is asked to
        // render. Before the fold, every note on an sf2 channel arrived as
        // the channel's own transpose — one pitch for the whole part.
        struct Spy(Vec<SynthKind>);
        impl Synth for Spy {
            fn render(&mut self, kind: &SynthKind, _: u32, out: &mut [f32]) { self.0.push(kind.clone()); out.fill(0.0); }
        }
        let mut rack = rack();
        rack.channels.push(Channel {
            name: "piano".into(),
            kind: SynthKind::Sf2 { path: "/nowhere.sf2".into(), bank: 0, preset: 0, transpose: 12 },
            gain: 1.0, pan: 0.0, muted: false, choke_group: None,
        });
        let ch = rack.channels.len() - 1;
        let mut pattern = Pattern::new("keys", 16, rack.channels.len());
        pattern.add_note(ch, Note { key: 3, start: 0, len: 4, vel: 0.9, slide: false });  // C2 on the roll
        pattern.add_note(ch, Note { key: 7, start: 4, len: 4, vel: 0.9, slide: false });  // E2
        pattern.toggle(ch, 8);
        pattern.set_mod(ch, 8, StepMod { tune: -5, ..StepMod::default() });                  // a step hit, tuned
        let tempo = Tempo { bpm: 120.0, steps_per_beat: 4, swing: 0.0 };
        let mut spy = Spy(Vec::new());
        render_pattern(&pattern, &rack, &mut spy, &tempo, 48_000, 0);
        let mut transposes: Vec<i8> = spy.0.iter().filter_map(|k| match k {
            SynthKind::Sf2 { transpose, .. } => Some(*transpose), _ => None }).collect();
        transposes.sort(); // step hits render before roll notes; the order is not the claim
        assert_eq!(transposes, vec![7, 15, 19], "the tuned step 12 - 5, then 12 + 3 and 12 + 7");
        assert!(spy.0.iter().all(|k| matches!(k, SynthKind::Sf2 { preset: 0, bank: 0, .. })), "bank and preset ride along");
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
            muted: false, locked: false, gain: 1.0, route: None,
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
            muted: false, locked: false, gain: 1.0, route: None,
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
                gain: 1.0, pan: 0.0, muted: false, choke_group: None,
            }],
            patterns: vec![], tempo_bpm: 120.0, swing: 0.0, ..Default::default() };
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
                gain: 1.0, pan: 0.0, muted: false, choke_group: None }],
            patterns: vec![], tempo_bpm: 120.0, swing: 0.0, ..Default::default() };
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
            gain: 0.9, pan: 0.0, muted: false, choke_group: None,
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

#[cfg(test)]
mod choke_tests {
    use super::*;

    fn hat_rack() -> Rack {
        let ch = |name: &str, voice: DrumVoice, decay: f32, grp: Option<u8>| Channel {
            name: name.into(),
            kind: SynthKind::Drum { voice, tone: 0.5, decay_ms: decay, tune: 0 },
            gain: 1.0, pan: 0.0, muted: false, choke_group: grp,
        };
        Rack {
            channels: vec![
                ch("ohat", DrumVoice::HatOpen, 600.0, Some(1)),
                ch("hat",  DrumVoice::HatClosed, 40.0, Some(1)),
            ],
            patterns: vec![], tempo_bpm: 120.0, swing: 0.0, ..Default::default() }
    }

    fn render(rack: &Rack, p: &Pattern) -> (Vec<f32>, usize) {
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        let t = rack.tempo();
        let out = render_pattern(p, rack, &mut synth, &t, 48_000, 0);
        (out, t.step_len().to_samples(48_000) as usize)
    }

    #[test]
    fn a_closed_hat_shuts_the_open_hat_it_shares_a_group_with() {
        let rack = hat_rack();
        let mut p = Pattern::new("A", 8, 2);
        p.set(0, 0, true); // open hat rings 600ms...
        p.set(1, 2, true); // ...until the closed hat two steps later
        let (choked, step) = render(&rack, &p);

        let mut open_only = Pattern::new("B", 8, 2);
        open_only.set(0, 0, true);
        let (free, _) = render(&rack, &open_only);

        let e = |b: &[f32], from: usize, to: usize| b[from..to].iter().map(|s| s.abs()).sum::<f32>();
        // Before the cut the two renders are the same sound...
        assert!((e(&choked, 0, step) - e(&free, 0, step)).abs() < e(&free, 0, step) * 0.01,
                "the open hat is untouched before the choke");
        // ...after it, the free render still rings and the choked one holds
        // only the closed hat's own short burst, which dies within a step.
        let ring_free = e(&free, step * 3, step * 4);
        let ring_choked = e(&choked, step * 3, step * 4);
        assert!(ring_free > 1.0, "unchoked open hat is still ringing ({ring_free})");
        assert!(ring_choked < ring_free * 0.15,
                "choke cut the tail: {ring_choked} vs free {ring_free}");
    }

    #[test]
    fn the_cut_is_a_fade_not_a_cliff() {
        // A hard truncation of a ringing waveform is a click. Compare the
        // choked render against an unchoked one: inside the ~2ms fade window
        // the choked signal must only ever be the free signal scaled DOWN,
        // and the last pre-cut sample must carry almost none of it.
        let rack = hat_rack();
        let mut p = Pattern::new("A", 8, 2);
        p.set(0, 0, true);
        // Cut at step 2, well INSIDE the open hat's natural 4-step window —
        // a cut at the window's own edge would make choke a no-op and this
        // test vacuous (which is exactly how its first draft shipped).
        p.set(1, 2, true);
        let (out, step) = render(&rack, &p);
        let mut solo_open = Pattern::new("B", 8, 2);
        solo_open.set(0, 0, true);
        let (free, _) = render(&rack, &solo_open);

        let cut = step * 2;
        let fade = 48_000 / 500;
        assert!(free[cut - fade..cut].iter().any(|s| s.abs() > 1e-3),
                "test premise: the open hat is actually ringing at the cut");
        for i in (cut - fade)..cut {
            assert!(out[i].abs() <= free[i].abs() + 1e-4,
                    "sample {i}: faded exceeds unfaded — that is not a fade");
        }
        assert!(out[cut - 1].abs() <= free[cut - 1].abs() / fade as f32 + 1e-4,
                "the ramp's last sample is ~zero, not a cliff edge");
    }

    #[test]
    fn simultaneous_group_hits_layer_instead_of_fighting() {
        let rack = hat_rack();
        let mut both = Pattern::new("A", 4, 2);
        both.set(0, 0, true);
        both.set(1, 0, true);
        let (layered, step) = render(&rack, &both);
        let mut just_open = Pattern::new("B", 4, 2);
        just_open.set(0, 0, true);
        let (open, _) = render(&rack, &just_open);
        // The open hat's late tail survives a same-instant closed hat.
        let e = |b: &[f32]| b[step * 2..step * 3].iter().map(|s| s.abs()).sum::<f32>();
        assert!(e(&layered) > e(&open) * 0.9,
                "same-sample hits are a chord, not a cut: {} vs {}", e(&layered), e(&open));
    }

    #[test]
    fn ungrouped_channels_never_choke_anything() {
        let mut rack = hat_rack();
        rack.channels[1].choke_group = None;
        let mut p = Pattern::new("A", 8, 2);
        p.set(0, 0, true);
        p.set(1, 2, true);
        let (out, step) = render(&rack, &p);
        let mut solo = Pattern::new("B", 8, 2);
        solo.set(0, 0, true);
        let (free, _) = render(&rack, &solo);
        let e = |b: &[f32]| b[step * 3..step * 4].iter().map(|s| s.abs()).sum::<f32>();
        assert!(e(&out) > e(&free) * 0.9, "no group, no cut");
    }
}

#[cfg(test)]
mod roll_tests {
    use super::*;

    fn note(key: i8, start: u32, len: u32) -> Note { Note { key, start, len, vel: 0.8, slide: false } }

    #[test]
    fn harmonize_adds_the_interval_in_key_and_never_stacks() {
        let mut p = Pattern::new("p", 16, 1);
        let c = 3i8; // C2 on the roll (key 0 is A1)
        p.add_note(0, note(c, 0, 4));
        p.add_note(0, note(c + 7, 4, 4)); // G
        let c_major = studio_core::Key { root: 0, scale: studio_core::Scale::Major };
        assert_eq!(p.harmonize(0, 2, Some(c_major)), 2, "a third above each");
        let keys: Vec<(u32, i8)> = p.roll(0).iter().map(|n| (n.start, n.key)).collect();
        assert_eq!(keys, vec![(0, c), (0, c + 4), (4, c + 7), (4, c + 11)], "C+E, G+B — the diatonic thirds");
        assert_eq!(p.harmonize(0, 2, Some(c_major)), 2, "thirds of the thirds: G and D");
        assert_eq!(p.harmonize(0, 2, Some(c_major)), 2, "and again: B and F");
        assert_eq!(p.roll(0).len(), 8);
        // Fifths: G and B exist already at each start (from the thirds);
        // only D and F over the top two are new — the collision rule at
        // work — and a second pass of the same interval adds nothing.
        assert_eq!(p.harmonize(0, 4, Some(c_major)), 4, "two new per start, the rest already there");
        let mut seen = std::collections::BTreeSet::new();
        assert!(p.roll(0).iter().all(|n| seen.insert((n.start, n.key))), "no (start, key) twice — nothing stacks");
        assert_eq!(p.harmonize(0, 0, Some(c_major)), 0, "a unison is every note's own place: nothing added");
        assert!(p.roll(0).iter().all(|n| n.len == 4 && (n.vel - 0.8).abs() < 1e-6), "copies keep length and velocity");
        // No key: major intervals in semitones.
        let mut q = Pattern::new("q", 16, 1);
        q.add_note(0, note(10, 0, 2));
        assert_eq!(q.harmonize(0, 4, None), 1);
        assert_eq!(q.roll(0)[1].key, 17, "a fifth is seven semitones");
        assert_eq!(q.harmonize(0, -7, None), 2, "an octave below each of the two");
        assert_eq!(q.roll(0).iter().map(|n| n.key).min(), Some(-2));
    }

    #[test]
    fn arpeggiate_breaks_each_chord_into_a_run_that_fills_its_span() {
        let mut p = Pattern::new("p", 16, 1);
        for k in [3, 7, 10] { p.add_note(0, note(k, 0, 8)); }   // C E G, 8 steps
        for k in [5, 8] { p.add_note(0, note(k, 8, 4)); }       // D F, 4 steps
        p.add_note(0, note(0, 12, 2));                           // a lone A: untouched
        assert_eq!(p.chords(0).len(), 2);
        assert_eq!(p.arpeggiate(0, Arp::Up, 2), 2);
        let run: Vec<(u32, i8, u32)> = p.roll(0).iter().map(|n| (n.start, n.key, n.len)).collect();
        assert_eq!(run, vec![(0, 3, 2), (2, 7, 2), (4, 10, 2), (6, 3, 2),   // C E G C
                             (8, 5, 2), (10, 8, 2),                          // D F
                             (12, 0, 2)],                                    // the lone A
                   "{run:?}");
        assert!(p.chords(0).is_empty(), "no chords left");
        // Down, and up-down, on a fresh triad; an odd span ends with a short note.
        let mut d = Pattern::new("d", 16, 1);
        for k in [3, 7, 10] { d.add_note(0, note(k, 0, 5)); }
        d.arpeggiate(0, Arp::Down, 2);
        assert_eq!(d.roll(0).iter().map(|n| (n.key, n.len)).collect::<Vec<_>>(), vec![(10, 2), (7, 2), (3, 1)]);
        let mut u = Pattern::new("u", 16, 1);
        for k in [3, 7, 10] { u.add_note(0, note(k, 0, 8)); }
        u.arpeggiate(0, Arp::UpDown, 2);
        assert_eq!(u.roll(0).iter().map(|n| n.key).collect::<Vec<_>>(), vec![3, 7, 10, 7], "C E G E: no repeated ends");
        // A pattern with no chords: nothing happens, and it says so.
        assert_eq!(u.arpeggiate(0, Arp::Up, 1), 0);
        assert_eq!(u.roll(0).len(), 4);
    }

    fn sub_rack(decay_ms: f32, glide_ms: f32) -> Rack {
        Rack {
            channels: vec![Channel {
                name: "808".into(),
                kind: SynthKind::Sub { note: 12, glide_from: None, glide_ms, drive: 0.0, decay_ms },
                gain: 1.0, pan: 0.0, muted: false, choke_group: None,
            }],
            patterns: Vec::new(), tempo_bpm: 120.0, swing: 0.0, ..Default::default() }
    }

    fn render(rack: &Rack, p: &Pattern) -> (Vec<f32>, usize) {
        let t = rack.tempo();
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        let out = render_pattern(p, rack, &mut synth, &t, 48_000, 0);
        (out, t.step_len().to_samples(48_000) as usize)
    }

    fn energy(b: &[f32], from: usize, to: usize) -> f32 {
        b[from.min(b.len())..to.min(b.len())].iter().map(|s| s.abs()).sum()
    }

    fn crossings(b: &[f32]) -> usize {
        b.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count()
    }

    #[test]
    fn a_note_sounds_for_its_length_and_gates_at_its_end() {
        // The note's end is a musical decision: a 2-step note on a 2-second
        // 808 must stop at 2 steps, faded, not ring out its whole decay.
        let rack = sub_rack(2000.0, 0.0);
        let mut p = Pattern::new("A", 6, 1);
        p.add_note(0, Note { key: 0, start: 0, len: 2, vel: 1.0, slide: false });
        let (out, step) = render(&rack, &p);
        let sounding = energy(&out, 0, step);
        let after = energy(&out, step * 3, step * 4);
        assert!(sounding > 10.0, "the note is audible in its window ({sounding})");
        assert!(after < sounding * 0.01, "and silent past its gate: {after} vs {sounding}");
    }

    #[test]
    fn a_held_note_outlasts_a_step_hits_four_step_window() {
        // The whole reason the roll exists beside the grid: a step hit's ring
        // is capped at four steps, a note holds for as long as it says. This
        // also pins the scratch buffer actually growing past `voice_len`.
        let rack = sub_rack(2000.0, 0.0);
        let mut held = Pattern::new("held", 8, 1);
        held.add_note(0, Note { key: 0, start: 0, len: 8, vel: 1.0, slide: false });
        let mut hit = Pattern::new("hit", 8, 1);
        hit.set(0, 0, true);
        let (held_out, step) = render(&rack, &held);
        let (hit_out, _) = render(&rack, &hit);
        let held_late = energy(&held_out, step * 5, step * 6);
        let hit_late = energy(&hit_out, step * 5, step * 6);
        assert!(held_late > 1.0, "the held note still rings at step 5 ({held_late})");
        assert!(hit_late < held_late * 0.01,
                "the step hit died at its window: {hit_late} vs held {held_late}");
    }

    #[test]
    fn a_chord_is_exactly_the_sum_of_its_notes() {
        // Placements are additive and independent; two notes sharing a start
        // must layer, not interact. Compared sample-for-sample, not by energy.
        let rack = sub_rack(400.0, 0.0);
        let mut chord = Pattern::new("chord", 4, 1);
        chord.add_note(0, Note { key: 0, start: 0, len: 2, vel: 1.0, slide: false });
        chord.add_note(0, Note { key: 7, start: 0, len: 2, vel: 1.0, slide: false });
        let mut root = Pattern::new("root", 4, 1);
        root.add_note(0, Note { key: 0, start: 0, len: 2, vel: 1.0, slide: false });
        let mut fifth = Pattern::new("fifth", 4, 1);
        fifth.add_note(0, Note { key: 7, start: 0, len: 2, vel: 1.0, slide: false });
        let (c, _) = render(&rack, &chord);
        let (r, _) = render(&rack, &root);
        let (f, _) = render(&rack, &fifth);
        for i in 0..c.len() {
            assert!((c[i] - (r[i] + f[i])).abs() < 1e-5,
                    "sample {i}: chord {} vs sum {}", c[i], r[i] + f[i]);
        }
    }

    #[test]
    fn slide_glides_from_the_previous_roll_note() {
        // An octave jump with slide spends its glide sweeping up from the
        // previous note's pitch, so its early window oscillates SLOWER than
        // the same note without slide. Measured by zero crossings — the same
        // instrument-blind yardstick the realtune tests use.
        let rack = sub_rack(2000.0, 200.0);
        let mut slid = Pattern::new("slide", 4, 1);
        slid.add_note(0, Note { key: 0, start: 0, len: 2, vel: 1.0, slide: false });
        slid.add_note(0, Note { key: 12, start: 2, len: 2, vel: 1.0, slide: true });
        let mut plain = slid.clone();
        plain.toggle_note_slide(0, 2, 12);
        let (s, step) = render(&rack, &slid);
        let (p, _) = render(&rack, &plain);
        let early = |b: &[f32]| crossings(&b[step * 2..step * 2 + 4800]); // first 100ms of note 2
        let (cs, cp) = (early(&s), early(&p));
        assert!(cs < cp * 8 / 10,
                "the slide's early window is still climbing: {cs} crossings vs {cp} straight");
    }

    #[test]
    fn a_roll_note_is_cut_by_its_choke_group() {
        // A held note is a citizen of the same rack rules as a hit: a group
        // hit landing mid-note cuts it, fade and all.
        let mut rack = sub_rack(2000.0, 0.0);
        rack.channels[0].choke_group = Some(1);
        rack.channels.push(Channel {
            name: "hat".into(),
            kind: SynthKind::Drum { voice: DrumVoice::HatClosed, tone: 0.5, decay_ms: 80.0, tune: 0 },
            gain: 1.0, pan: 0.0, muted: false, choke_group: Some(1),
        });
        let mut cut = Pattern::new("cut", 8, 2);
        cut.add_note(0, Note { key: 0, start: 0, len: 8, vel: 1.0, slide: false });
        cut.set(1, 2, true);
        let mut free = cut.clone();
        free.set(1, 2, false);
        let (c, step) = render(&rack, &cut);
        let (f, _) = render(&rack, &free);
        let late_cut = energy(&c, step * 5, step * 6);
        let late_free = energy(&f, step * 5, step * 6);
        assert!(late_free > 1.0, "unchoked, the note still rings ({late_free})");
        assert!(late_cut < late_free * 0.05,
                "the group hit cut the note: {late_cut} vs free {late_free}");
    }

    #[test]
    fn shortening_the_pattern_hides_notes_without_eating_them() {
        // The rows' rule, extended: a note past the new length falls silent
        // but survives, so shorten-then-lengthen round-trips.
        let rack = sub_rack(400.0, 0.0);
        let mut p = Pattern::new("A", 16, 1);
        p.add_note(0, Note { key: 0, start: 12, len: 2, vel: 1.0, slide: false });
        p.steps = 8;
        let (short, _) = render(&rack, &p);
        assert!(short.iter().all(|s| s.abs() < 1e-6), "a hidden note renders nothing");
        p.steps = 16;
        let (long, step) = render(&rack, &p);
        assert!(energy(&long, step * 12, step * 13) > 10.0, "and it is back at full length");
        assert!(p.note_at(0, 0, 12).is_some(), "the note itself never left");
    }

    #[test]
    fn adding_at_the_same_start_and_key_replaces_rather_than_stacks() {
        let mut p = Pattern::new("A", 16, 1);
        p.add_note(0, Note { key: 0, start: 0, len: 1, vel: 1.0, slide: false });
        p.add_note(0, Note { key: 0, start: 0, len: 4, vel: 0.5, slide: false });
        assert_eq!(p.roll(0).len(), 1, "one intention, one note");
        assert_eq!(p.roll(0)[0].len, 4);
        assert!((p.roll(0)[0].vel - 0.5).abs() < 1e-6);
        // A different key at the same start is a chord, not a replacement.
        p.add_note(0, Note { key: 7, start: 0, len: 1, vel: 1.0, slide: false });
        assert_eq!(p.roll(0).len(), 2);
        // And removing the last note drops the channel's entry entirely,
        // keeping `rolls` sparse the way `set_mod` keeps `mods` sparse.
        assert!(p.remove_note(0, 0, 0));
        assert!(p.remove_note(0, 0, 7));
        assert!(p.rolls.is_empty());
    }

    #[test]
    fn a_pattern_saved_before_the_roll_existed_still_loads() {
        // serde(default): a pre-roll pattern deserializes with the empty
        // roll it effectively had, rather than refusing the whole rack.
        let p = Pattern::new("old", 16, 2);
        let mut v = serde_json::to_value(&p).expect("serializes");
        v.as_object_mut().expect("a map").remove("rolls").expect("the field exists to strip");
        let back: Pattern = serde_json::from_value(v).expect("a roll-less pattern still loads");
        assert!(back.rolls.is_empty());
        assert_eq!(back, p);
    }

    #[test]
    fn a_sampler_roll_note_plays_the_chop_at_pitch() {
        // +12 semitones reads the rendered voice twice as fast — roughly
        // double the zero crossings in the note window. This is what makes
        // a one-shot chop a melodic instrument, FL's oldest trick.
        let sine: Vec<f32> = (0..24_000)
            .map(|i| (i as f32 / 48_000.0 * 100.0 * std::f32::consts::TAU).sin())
            .collect();
        let mut sources = std::collections::BTreeMap::new();
        sources.insert(studio_core::SourceId(1),
                       SourceAudio { sample_rate: 48_000, channels: vec![sine] });
        let rack = Rack {
            channels: vec![Channel {
                name: "chop".into(),
                kind: SynthKind::Sampler { source: studio_core::SourceId(1) },
                gain: 1.0, pan: 0.0, muted: false, choke_group: None,
            }],
            patterns: Vec::new(), tempo_bpm: 120.0, swing: 0.0, ..Default::default() };
        let t = rack.tempo();
        let mut up = Pattern::new("up", 4, 1);
        up.add_note(0, Note { key: 12, start: 0, len: 2, vel: 1.0, slide: false });
        let mut flat = Pattern::new("flat", 4, 1);
        flat.add_note(0, Note { key: 0, start: 0, len: 2, vel: 1.0, slide: false });
        let mut synth = BuiltinSynth { sources: &sources };
        let u = render_pattern(&up, &rack, &mut synth, &t, 48_000, 0);
        let f = render_pattern(&flat, &rack, &mut synth, &t, 48_000, 0);
        let step = t.step_len().to_samples(48_000) as usize;
        let (cu, cf) = (crossings(&u[..step]), crossings(&f[..step]));
        assert!(cu > cf * 18 / 10 && cu < cf * 22 / 10,
                "+12 doubles the rate: {cu} crossings vs {cf}");
    }

    #[test]
    fn the_tune_lane_pitches_a_sampler_step_hit_too() {
        // The same repitch serves the grid: StepMod::tune on a sample channel
        // was a documented no-op, and now it means what it says everywhere.
        let sine: Vec<f32> = (0..24_000)
            .map(|i| (i as f32 / 48_000.0 * 100.0 * std::f32::consts::TAU).sin())
            .collect();
        let mut sources = std::collections::BTreeMap::new();
        sources.insert(studio_core::SourceId(1),
                       SourceAudio { sample_rate: 48_000, channels: vec![sine] });
        let rack = Rack {
            channels: vec![Channel {
                name: "chop".into(),
                kind: SynthKind::Sampler { source: studio_core::SourceId(1) },
                gain: 1.0, pan: 0.0, muted: false, choke_group: None,
            }],
            patterns: Vec::new(), tempo_bpm: 120.0, swing: 0.0, ..Default::default() };
        let t = rack.tempo();
        let mut up = Pattern::new("up", 4, 1);
        up.set(0, 0, true);
        up.set_mod(0, 0, StepMod { tune: 12, ..StepMod::default() });
        let mut flat = Pattern::new("flat", 4, 1);
        flat.set(0, 0, true);
        let mut synth = BuiltinSynth { sources: &sources };
        let u = render_pattern(&up, &rack, &mut synth, &t, 48_000, 0);
        let f = render_pattern(&flat, &rack, &mut synth, &t, 48_000, 0);
        let step = t.step_len().to_samples(48_000) as usize;
        let (cu, cf) = (crossings(&u[..step]), crossings(&f[..step]));
        assert!(cu > cf * 18 / 10 && cu < cf * 22 / 10,
                "+12 doubles the rate on a hit as well: {cu} vs {cf}");
    }
}

#[cfg(test)]
mod pattern_length_tests {
    use super::*;

    #[test]
    fn shortening_a_pattern_hides_steps_without_eating_them() {
        // The rack panel edits `steps` directly; the bits past the new end
        // must survive so shorten-then-lengthen round-trips — losing the
        // back half of a 32-step groove to a moment of 16-step curiosity
        // would be a silent edit nobody made.
        let mut p = Pattern::new("A", 32, 1);
        p.set(0, 30, true);
        p.set(0, 5, true);
        p.steps = 16;
        assert_eq!(p.hits(), vec![(0, 5)], "hidden steps do not render");
        p.steps = 32;
        assert_eq!(p.hits(), vec![(0, 5), (0, 30)], "and come back intact");
    }

    #[test]
    fn a_long_pattern_renders_its_whole_length() {
        let rack = Rack::trap_kit();
        let mut p = Pattern::new("long", 64, rack.channels.len());
        p.set(1, 0, true);
        p.set(1, 63, true);
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        let t = rack.tempo();
        let out = render_pattern(&p, &rack, &mut synth, &t, 48_000, 0);
        let step = t.step_len().to_samples(48_000) as usize;
        assert_eq!(out.len(), step * 64, "four bars of buffer");
        let tail: f32 = out[step * 63..].iter().map(|v| v.abs()).sum();
        assert!(tail > 1.0, "the step-63 kick is in the last step's audio");
    }
}

#[cfg(test)]
mod gated_synth_tests {
    use super::*;

    fn sub_rack_decay(decay_ms: f32) -> Rack {
        let mut r = Rack::trap_kit();
        r.channels = vec![Channel {
            name: "808".into(),
            kind: SynthKind::Sub { note: 0, glide_from: None, glide_ms: 0.0,
                                   drive: 0.0, decay_ms },
            gain: 1.0, pan: 0.0, muted: false, choke_group: None,
        }];
        for p in &mut r.patterns { p.rows = vec![0]; p.mods.clear(); p.rolls.clear(); }
        r
    }
    fn rms(b: &[f32], from: usize, to: usize) -> f32 {
        let s = &b[from.min(b.len())..to.min(b.len())];
        if s.is_empty() { return 0.0; }
        (s.iter().map(|v| v * v).sum::<f32>() / s.len() as f32).sqrt()
    }

    #[test]
    fn a_held_note_sustains_where_a_short_decay_used_to_die() {
        // THE wiring this exists for: before render_gated, a held roll note
        // on a 120ms-decay Sub was silent by its second step — the synth
        // was reachable from the roll but could not be HELD. Now the note
        // sustains at full level to its gate, then cuts with the fixed
        // release.
        let rack = sub_rack_decay(120.0);
        let mut p = Pattern::new("hold", 8, 1);
        p.add_note(0, Note { key: 0, start: 0, len: 6, vel: 1.0, slide: false });
        let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
        let tempo = rack.tempo();
        let out = render_pattern(&p, &rack, &mut synth, &tempo, 48_000, 0);
        let step = tempo.step_len().to_samples(48_000).max(1) as usize;
        let early = rms(&out, step / 2, step);
        let late = rms(&out, step * 5, step * 11 / 2); // deep in the hold
        assert!(late > early * 0.7,
                "held means HELD: step-5 rms {late} vs step-0 {early}");
        // And the gate still gates: half a step past note-off, silence —
        // the roll's oldest pin, restated against the sustaining envelope.
        assert!(rms(&out, step * 13 / 2, step * 7) < early * 0.01,
                "note-off is a cut with a fade, not a second decay");
    }

    #[test]
    fn the_rolls_key_is_the_synths_pitch() {
        // key +12 through the tune mod must land exactly an octave up —
        // pinned by zero-crossing count over the sustained region, where
        // the tone is steady by construction.
        let rack = sub_rack_decay(200.0);
        let zc_of = |key: i8| {
            let mut p = Pattern::new("k", 8, 1);
            p.add_note(0, Note { key, start: 0, len: 8, vel: 1.0, slide: false });
            let sources = std::collections::BTreeMap::new();
        let mut synth = BuiltinSynth { sources: &sources };
            let tempo = rack.tempo();
            let out = render_pattern(&p, &rack, &mut synth, &tempo, 48_000, 0);
            let step = tempo.step_len().to_samples(48_000).max(1) as usize;
            let s = &out[step..step * 6];
            s.windows(2).filter(|w| (w[0] >= 0.0) != (w[1] >= 0.0)).count() as f64
        };
        let (base, octave) = (zc_of(0), zc_of(12));
        let ratio = octave / base.max(1.0);
        assert!((ratio - 2.0).abs() < 0.06,
                "an octave in the roll is an octave out of the synth: {ratio}");
    }
}

#[cfg(test)]
mod rhythm_tests {
    use super::*;

    fn show(mask: u64, steps: u32) -> String {
        (0..steps).map(|i| if mask & (1 << i) != 0 { 'x' } else { '.' }).collect()
    }

    #[test]
    fn the_named_euclidean_rhythms_come_out_as_written() {
        assert_eq!(show(Pattern::euclid(8, 3, 0), 8), "x..x..x.", "tresillo");
        assert_eq!(show(Pattern::euclid(8, 5, 0), 8), "x.xx.xx.", "cinquillo");
        assert_eq!(show(Pattern::euclid(16, 5, 0), 16), "x..x..x..x..x...", "bossa bell");
        // E(12,7): Toussaint writes x.xx.x.xx.x.; Bjorklund's recursion lands one
        // rotation over. Same necklace — assert that, since the spelling differs by
        // implementation and the rhythm does not.
        let bell = Pattern::euclid(12, 7, 0);
        assert!((0..12).any(|r| show(Pattern::euclid(12, 7, r), 12) == "x.xx.x.xx.x."), "{}", show(bell, 12));
        assert_eq!(show(Pattern::euclid(4, 4, 0), 4), "xxxx");
        assert_eq!(show(Pattern::euclid(8, 0, 0), 8), "........");
        assert_eq!(show(Pattern::euclid(8, 9, 0), 8), "xxxxxxxx", "pulses cap at steps");
    }

    #[test]
    fn rotation_moves_the_hits_later_and_wraps() {
        assert_eq!(show(Pattern::euclid(8, 3, 1), 8), ".x..x..x");
        assert_eq!(show(Pattern::euclid(8, 3, 8), 8), "x..x..x.", "a full turn is identity");
        assert_eq!(Pattern::euclid(8, 3, 3).count_ones(), 3);
    }

    #[test]
    fn humanise_is_bounded_deterministic_and_softer_off_the_beat() {
        let mut p = Pattern::new("h", 16, 2);
        p.set_euclid(0, 16, 0);
        p.humanize(0, 0.15, 7);
        let a: Vec<f32> = (0..16).map(|s| p.mod_at(0, s).vel).collect();
        assert!(a.iter().all(|v| (0.55..=1.0).contains(v)), "{a:?}");
        let on: f32 = [0, 4, 8, 12].iter().map(|&s| a[s]).sum::<f32>() / 4.0;
        let off: f32 = (0..16).filter(|s| s % 4 != 0).map(|s| a[s]).sum::<f32>() / 12.0;
        assert!(on > off, "on-beat {on} should lean louder than off-beat {off}");
        let mut q = Pattern::new("h", 16, 2);
        q.set_euclid(0, 16, 0);
        q.humanize(0, 0.15, 7);
        assert_eq!((0..16).map(|s| q.mod_at(0, s).vel).collect::<Vec<_>>(), a, "same seed, same take");
        q.humanize(0, 0.15, 8);
        assert_ne!((0..16).map(|s| q.mod_at(0, s).vel).collect::<Vec<_>>(), a, "a new seed is a new take");
        assert!(p.mod_at(1, 0).vel == 1.0, "the other channel is untouched");
    }

    // ── per-channel swing ────────────────────────────────────────────────

    /// A rack with no overrides behaves exactly as it always did.
    #[test]
    fn channels_follow_the_kit_by_default() {
        let mut r = Rack::default();
        r.swing = 0.6;
        assert!(r.swing_overrides.is_empty());
        for ch in 0..4 {
            assert_eq!(r.channel_swing(ch), 0.6);
        }
    }

    /// THE TRAP SETTING. Swing the hats, keep the 808 straight -- the same
    /// shuffle on the low end reads as a mistake rather than as feel.
    #[test]
    fn an_808_can_be_pinned_straight_while_the_hats_swing() {
        let mut r = Rack::default();
        r.swing = 0.62;
        let (eight_o_eight, hat) = (0usize, 1usize);
        r.set_channel_straight(eight_o_eight);
        assert_eq!(r.channel_swing(eight_o_eight), 0.0);
        assert_eq!(r.channel_swing(hat), 0.62, "the hats keep the kit's feel");
    }

    /// `Some(0.0)` is not the same as absent, which is the distinction the
    /// whole override rests on.
    #[test]
    fn a_zero_override_is_not_the_same_as_no_override() {
        let mut r = Rack::default();
        r.swing = 0.5;
        r.set_channel_straight(2);
        assert_eq!(r.channel_swing(2), 0.0);
        r.clear_channel_swing(2);
        assert_eq!(r.channel_swing(2), 0.5, "back to the kit");
    }

    /// A channel can also swing HARDER than the kit -- hats often do.
    #[test]
    fn a_channel_can_exceed_the_kits_swing() {
        let mut r = Rack::default();
        r.swing = 0.2;
        r.swing_overrides.insert(1, 0.75);
        assert!(r.channel_swing(1) > r.channel_swing(0));
    }

    /// The delay must follow the resolved swing, not the rack's -- otherwise
    /// the override works in the UI and not in the audio.
    #[test]
    fn the_rendered_delay_follows_the_channels_own_swing() {
        let t = Tempo { bpm: 120.0, steps_per_beat: 4, swing: 0.8 };
        let straight = t.swing_delay_samples_at(1, 48_000, 0.0);
        let swung = t.swing_delay_samples_at(1, 48_000, 0.8);
        assert_eq!(straight, 0, "a straight channel is never delayed");
        assert!(swung > 0);
        // And the downbeat rule holds regardless of who is asking.
        assert_eq!(t.swing_delay_samples_at(0, 48_000, 1.0), 0,
            "even steps are never moved, whatever the channel wants");
    }
}

#[cfg(test)]
mod poly_tests {
    use super::*;

    fn zero_crossings(v: &[f32]) -> usize { v.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count() }

    #[test]
    fn a_poly_sine_plays_at_pitch_and_its_envelope_holds_then_releases() {
        let p = PolyPatch { wave: 0, voices: 1, detune: 0.0, sub: 0.0, attack_ms: 1.0, decay_ms: 10.0, sustain: 1.0, release_ms: 100.0,
                            cutoff: 20000.0, resonance: 0.0, env_amount: 0.0, vibrato: 0.0, gain: 1.0, ..Default::default() };
        let mut out = vec![0.0f32; 48_000];
        poly_voice(220.0, &p, 48_000, &mut out, Some(24_000));
        let held = &out[1000..23_000];
        let hz = zero_crossings(held) as f32 / (held.len() as f32 / 48_000.0);
        assert!((hz - 220.0).abs() < 2.0, "pitch {hz}");
        let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
        assert!(rms(&out[20_000..23_000]) > 0.6, "sustained while held");
        assert!(rms(&out[30_000..]).abs() < 1e-6, "silent after the 100 ms release");
        assert!(out.iter().all(|x| x.is_finite()));
    }

    #[test]
    fn the_filter_darkens_a_saw_and_unison_widens_nothing_it_should_not() {
        let bright = PolyPatch { cutoff: 18_000.0, env_amount: 0.0, voices: 1, ..Default::default() };
        let dark = PolyPatch { cutoff: 300.0, ..bright };
        let (mut a, mut b) = (vec![0.0f32; 24_000], vec![0.0f32; 24_000]);
        poly_voice(220.0, &bright, 48_000, &mut a, Some(20_000));
        poly_voice(220.0, &dark, 48_000, &mut b, Some(20_000));
        // high-frequency energy: the first difference
        let hf = |v: &[f32]| v.windows(2).skip(2000).take(15_000).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>();
        assert!(hf(&b) < 0.1 * hf(&a), "a 300 Hz cutoff removes most of the saw's highs: {} vs {}", hf(&b), hf(&a));
    }

    /// Goertzel magnitude of `f` in `v` (for the texture tests).
    fn mag(v: &[f32], f: f32, rate: f32) -> f32 {
        let w = std::f32::consts::TAU * f / rate;
        let (c, mut s1, mut s2) = (2.0 * w.cos(), 0.0f32, 0.0f32);
        for x in v { let s0 = x + c * s1 - s2; s2 = s1; s1 = s0; }
        (s1 * s1 + s2 * s2 - c * s1 * s2).sqrt() / v.len() as f32
    }

    /// A harmonic profile IS the timbre: 1 : 0.5 at harmonics 1 and 3 comes out
    /// with the 3rd at half the 1st, the 2nd absent -- and an all-zero profile
    /// leaves the old oscillator alone.
    #[test]
    fn a_harmonic_profile_sets_the_timbre() {
        let mut partials = [0u8; 16];
        partials[0] = 255; partials[2] = 128;
        let p = PolyPatch { voices: 1, detune: 0.0, attack_ms: 1.0, sustain: 1.0, cutoff: 20_000.0, env_amount: 0.0, gain: 1.0, partials, ..Default::default() };
        let mut out = vec![0.0f32; 48_000];
        poly_voice(200.0, &p, 48_000, &mut out, Some(40_000));
        let body = &out[4_800..38_400];
        let (h1, h2, h3) = (mag(body, 200.0, 48_000.0), mag(body, 400.0, 48_000.0), mag(body, 600.0, 48_000.0));
        assert!((h3 / h1 - 0.5).abs() < 0.08, "3rd at half the 1st: {}", h3 / h1);
        assert!(h2 / h1 < 0.05, "no 2nd: {}", h2 / h1);
    }

    #[test]
    fn noise_fm_and_drive_add_what_they_say() {
        let base = PolyPatch { wave: 0, voices: 1, detune: 0.0, attack_ms: 1.0, sustain: 1.0, cutoff: 20_000.0, env_amount: 0.0, gain: 1.0, ..Default::default() };
        let render = |p: &PolyPatch| { let mut o = vec![0.0f32; 24_000]; poly_voice(220.0, p, 48_000, &mut o, Some(20_000)); o };
        let hf = |v: &[f32]| v.windows(2).skip(2000).take(15_000).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>();
        let (plain, noisy) = (render(&base), render(&PolyPatch { noise: 0.3, ..base }));
        assert!(hf(&noisy) > 5.0 * hf(&plain), "noise adds highs");
        let fm = render(&PolyPatch { fm_ratio: 2.0, fm_index: 2.0, ..base });
        let body = &fm[2_000..18_000];
        assert!(mag(body, 220.0 * 3.0, 48_000.0) > 5.0 * mag(&plain[2_000..18_000], 660.0, 48_000.0), "FM makes a sideband at 220 + 2x220");
        let driven = render(&PolyPatch { drive: 1.0, ..base });
        assert!(mag(&driven[2_000..18_000], 660.0, 48_000.0) > 5.0 * mag(&plain[2_000..18_000], 660.0, 48_000.0), "drive makes a 3rd harmonic");
    }

    #[test]
    fn a_note_retunes_a_poly_like_a_sine() {
        let k = SynthKind::Poly { hz: 261.63, patch: PolyPatch::default() };
        let m = StepMod { tune: 12, ..Default::default() };
        match apply_step_mod(&k, &m, None) { SynthKind::Poly { hz, .. } => assert!((hz - 523.26).abs() < 0.01), other => panic!("{other:?}") }
    }
}
