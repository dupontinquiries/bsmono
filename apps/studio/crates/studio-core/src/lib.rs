//! studio-core — the editing model behind Studio.
//!
//! ## Why this exists, and why it is not a transliteration
//!
//! kdenlive and Audacity solve overlapping problems with very different
//! machinery. kdenlive's timeline (`src/timeline2/model/*`) is a Qt model tree
//! over MLT producers: clips reference bin clips by id and carry an in-point,
//! a length, an effect stack, and group membership. Audacity's editor is a
//! `WaveTrack` of `Sequence` blocks — audio held in chunks so an edit rewrites
//! a few blocks instead of a whole file. Both then bolt undo on top as command
//! objects, and neither can be edited by two people at once.
//!
//! Studio keeps the good parts of both and makes one structural change: **every
//! edit is a value, not a method call**. An [`Op`] is a small serializable
//! record that can be applied, inverted, logged, and sent over a wire. From
//! that single decision you get, for free:
//!
//! * **Undo** — the inverse of the op you just applied (no command classes).
//! * **Collaboration** — an op is exactly what you send to a peer.
//! * **Automation/scripting** — a macro is a list of ops.
//! * **Deterministic tests** — a project is a fold of ops over an empty doc.
//!
//! The rules that make concurrent editing converge are baked into the data
//! model rather than bolted on:
//!
//! * Clips are addressed by **stable ids**, never by index, so an insert or a
//!   delete elsewhere cannot make a remote op point at the wrong clip (the
//!   classic reason index-based edit logs need operational transforms).
//! * Positions and lengths are **absolute values on a timebase**, not deltas,
//!   so two peers moving the same clip converge on one of the two intended
//!   positions rather than summing into a place neither asked for.
//! * Track membership is a field on the clip, so moving between tracks is the
//!   same op as moving in time.
//!
//! Audio and video share this model: a video clip and an audio clip differ in
//! which [`TrackKind`] they live on and which effects apply, not in structure.
//! Audacity-style block storage lives behind [`SourceId`] — the timeline never
//! owns samples, only references into a source.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

mod curves;
pub mod journal;
mod grouping;
pub use grouping::GroupTangle;
pub use curves::{Binding, Curve, Gate, Opening, Response, Sweep};
mod batch;
mod room;
pub use room::{Overlap, LANE_SUFFIX};
mod takes;

/// A point or duration on the project timebase, in ticks.
///
/// One tick is 1/[`TICKS_PER_SECOND`] of a second, chosen so that common video
/// frame rates AND common audio sample rates land on whole numbers: 44100,
/// 48000, 24, 25, 30, 50, 60, and 30000/1001 (NTSC) all divide it evenly enough
/// to avoid the drift that bites editors which store seconds as f64. Integer
/// ticks also make edits exactly reversible, which floating seconds are not.
pub const TICKS_PER_SECOND: i64 = 705_600_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, Default)]
pub struct Ticks(pub i64);

impl Ticks {
    pub const ZERO: Ticks = Ticks(0);
    pub fn from_seconds(s: f64) -> Ticks { Ticks((s * TICKS_PER_SECOND as f64).round() as i64) }
    pub fn from_samples(n: i64, rate: u32) -> Ticks { Ticks(n * (TICKS_PER_SECOND / rate as i64)) }
    pub fn from_frames(n: i64, fps_num: u32, fps_den: u32) -> Ticks {
        Ticks(n * TICKS_PER_SECOND * fps_den as i64 / fps_num as i64)
    }
    pub fn to_seconds(self) -> f64 { self.0 as f64 / TICKS_PER_SECOND as f64 }
    pub fn to_samples(self, rate: u32) -> i64 { self.0 / (TICKS_PER_SECOND / rate as i64) }
    pub fn saturating_add(self, o: Ticks) -> Ticks { Ticks(self.0.saturating_add(o.0)) }
    pub fn saturating_sub(self, o: Ticks) -> Ticks { Ticks(self.0.saturating_sub(o.0)) }
}

macro_rules! id_type {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        pub struct $name(pub u64);
    };
}
id_type!(ClipId, "A clip's stable identity. Never reused, never an index — this is what lets a remote op survive concurrent edits elsewhere on the timeline.");
id_type!(TrackId, "A track's stable identity.");
id_type!(SourceId, "A media source (Audacity's sequence/blocks, kdenlive's bin clip). The timeline references samples; it never owns them.");
id_type!(BusId, "A bus's stable identity. Busses form a routing graph (parent hierarchy plus sends), not a flat list, which is why they get ids rather than being addressed by position.");
id_type!(GroupId, "A composition's stable identity: a named set of clips the timeline folds to one block — kdenlive's pre-batched group, After Effects' precomp, as the smallest model that supports the gesture. Membership is a PROJECT fact (a collaborator sees your composition); folded open or closed is view state in the device's workspace sidecar, the solo rule.");
id_type!(CurveId, "A reusable automation curve's identity — see `curves`. A curve is named and addressed on its own, not through the target it drives, because the same curve may drive twelve clips.");
id_type!(BindingId, "One binding of a curve to a target through a gate — see `curves::Binding`.");

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TrackKind { Video, Audio }

/// One clip on one track. The video/audio distinction is the track it sits on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub source: SourceId,
    pub track: TrackId,
    /// Where the clip starts on the timeline.
    pub pos: Ticks,
    /// Offset into the source where playback begins (kdenlive's in-point).
    pub src_in: Ticks,
    /// How much of the source is used.
    pub len: Ticks,
    /// Linear gain, 1.0 = unity. Applies to audio tracks and to a video clip's
    /// audio stream alike.
    pub gain: f32,
    /// Effects in order, each with opaque named parameters. Deliberately not an
    /// enum: the host registers effect kinds, so a project made with a plugin
    /// you lack still round-trips instead of failing to parse.
    pub effects: Vec<Effect>,
}

impl Clip {
    pub fn end(&self) -> Ticks { self.pos.saturating_add(self.len) }
    /// Does this clip overlap `other` in time? (Same-track overlap is what the
    /// timeline forbids; clips on different tracks overlap freely.)
    pub fn overlaps(&self, other: &Clip) -> bool {
        self.track == other.track && self.pos < other.end() && other.pos < self.end()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Effect {
    pub kind: String,
    pub params: BTreeMap<String, f32>,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub kind: TrackKind,
    pub name: String,
    pub muted: bool,
    pub locked: bool,
    /// The track's fader, applied to every clip it carries (multiplied with
    /// each clip's own gain) — the mixer's per-track level. `serde(default)`
    /// to UNITY, deliberately not to `f32::default()`: a pre-fader project
    /// loading silent would be the worst possible reading of "missing
    /// field".
    #[serde(default = "unity_gain")]
    pub gain: f32,
    /// Which bus this track's signal feeds. `None` means straight to master —
    /// the implicit root every bus graph has whether or not any `Bus` value
    /// exists for it, so a project with no busses at all still mixes exactly
    /// as it always did.
    pub route: Option<BusId>,
}

/// A send: a parallel, weighted copy of a bus's signal into another bus.
/// Distinct from `parent` (the bus's single primary output) — a bus has at
/// most one parent but any number of sends, which is what lets a vocal bus
/// feed both master (via parent) and a shared reverb bus (via a send) at
/// once.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Send {
    pub to: BusId,
    /// Linear gain on the sent copy, 1.0 = unity.
    pub level: f32,
    /// Tapped before this bus's own fader/chain gain is applied, vs. after.
    pub pre_fader: bool,
}

/// A mixer bus: something tracks or other busses route into, with its own
/// gain and effect chain, that in turn routes into a parent (or master, if
/// `parent` is `None`). The vocals/drums/808/melody separation the whole
/// point of a trap mixer is just four `Bus` values with distinct `parent`s
/// (or none) — nothing more structural than that.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bus {
    pub id: BusId,
    pub name: String,
    /// This bus's primary output. `None` means master.
    pub parent: Option<BusId>,
    /// Linear gain, 1.0 = unity.
    pub gain: f32,
    pub chain: Vec<Effect>,
    pub sends: Vec<Send>,
    /// A muted bus (and everything summed into it) is absent from both its
    /// sends and its parent — mute is an absolute override, checked before
    /// solo, which keeps the two rules independent rather than needing a
    /// combined truth table.
    pub muted: bool,
    /// A SILENT bus sums, processes and fades exactly as usual, and then
    /// feeds nothing: not its parent, not its sends. It is a measurement
    /// tap — a "dummy channel" in the FL Studio sense. Tracks and busses
    /// feed it through ordinary sends (the splitter: a send is already a
    /// parallel copy), it renders like any other bus, and
    /// `studio_audio::bus_signal` reads its output to drive something —
    /// a meter, a sidechain, or the multicam switcher's camera choice.
    ///
    /// Distinct from `muted`, and deliberately not the same flag: a muted
    /// bus is DROPPED before its chain runs, so it costs nothing and has no
    /// output to read. A silent bus is fully rendered and then not mixed.
    #[serde(default)]
    pub silent: bool,
}

/// The project: tracks, clips, the sources they reference, and the bus graph
/// they route into.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub tracks: Vec<Track>,
    /// Clips keyed by id. A map rather than per-track vectors so an op never has
    /// to search, and so ids stay valid no matter what happens to ordering.
    pub clips: BTreeMap<ClipId, Clip>,
    pub sources: BTreeMap<SourceId, Source>,
    /// Busses keyed by id. Empty is a valid, common project: every track
    /// routes to the implicit master and mixes exactly as before busses
    /// existed at all.
    pub busses: BTreeMap<BusId, Bus>,
    pub name: String,
    /// Automation lanes: points per target, each lane sorted by time. A
    /// lane RIDES its fader (the render multiplies the two), it does not
    /// replace it — the fader stays a trim you can grab without fighting
    /// the curve, and a project with no lanes renders bit-identically to
    /// one from before lanes existed. Sparse: an emptied lane drops its
    /// entry, and a target that was removed may leave a dangling lane —
    /// harmless (the render skips unknown targets) and cheaper than making
    /// `RemoveTrack` responsible for a second cleanup undo can't restore.
    /// `serde(default)` so any pre-automation serialization still loads.
    #[serde(default)]
    pub automation: BTreeMap<AutoTarget, Vec<(Ticks, f32)>>,
    /// Compositions (see [`GroupId`]). A clip may sit in several groups; a
    /// removed clip may dangle in one — the automation rule again:
    /// harmless, filtered at draw time, cheaper than a second cleanup
    /// undo could not restore.
    #[serde(default)]
    pub groups: BTreeMap<GroupId, Group>,
    /// Archive-mode provenance per source: the exact hash it was last
    /// verified against on disk, and — when the original was replaced by a
    /// lower-quality stand-in and deleted — how to get it back. Sparse and
    /// optional on purpose: a source with no entry here is just an
    /// ordinary source, no different from before this existed.
    /// `serde(default)` for the usual reason (a project saved before this
    /// existed still loads).
    #[serde(default)]
    pub provenance: BTreeMap<SourceId, Provenance>,
    /// Per-source perceptual fingerprints for fuzzy content matching
    /// (transcoded/re-encoded duplicate detection) -- sparse and
    /// independent of `provenance` on purpose: a project may want fuzzy
    /// matching without archive tracking, or vice versa.
    #[serde(default)]
    pub fingerprints: BTreeMap<SourceId, FingerprintData>,
    /// Reusable automation curves, addressed by id — see the `curves`
    /// module. A curve with no binding is inert; a curve is never removed
    /// while a binding names it. `serde(default)` for the usual reason.
    /// Per-clip fades, in ticks from the clip's own start and end.
    ///
    /// A SPARSE MAP, not a field on `Clip`, for two reasons. There are 221
    /// `Clip` literals in this tree and most clips have no fade, so the common
    /// project stores nothing — the same trade `Rack::swing_overrides` makes.
    /// And widening `Clip` would have meant a mechanical edit across all 221,
    /// which is exactly the rewrite that corrupted studio-app earlier.
    ///
    /// On the PROJECT rather than the engine, which is the actual fix here.
    /// `studio_audio::Fades` and the render's `fade.gain_at` already existed,
    /// and `studio_engine::Device::set_fades` could hold them — with no
    /// callers anywhere. So the machinery worked and a fade could not be
    /// created, saved, undone, or seen by a collaborator. A fade is a fact
    /// about the edit, not about this device's playback.
    #[serde(default)]
    pub fades: BTreeMap<ClipId, (Ticks, Ticks)>,
    /// Each bus's pan, -1 (left) ..= 1 (right); absent = centre. A BALANCE,
    /// as an FL mixer insert's pan is on a stereo signal: right turns the left
    /// side down, never the right up, so centre changes nothing. Kept beside
    /// `Bus` (like `fades` beside clips) so a pan is its own op and every
    /// place that builds a `Bus` is untouched. Studio had no pan at all in its
    /// timeline mix before c20.
    #[serde(default)]
    pub bus_pans: BTreeMap<BusId, f32>,
    #[serde(default)]
    pub curves: BTreeMap<CurveId, Curve>,
    /// Which curves drive which targets through which gates. Multiplied
    /// into the target's lane at render time; a binding whose gate clip is
    /// gone dangles harmlessly, the lanes' and groups' own rule.
    #[serde(default)]
    pub bindings: BTreeMap<BindingId, Binding>,
    /// The song's key, when the writer has decided one. `None` means "no
    /// opinion" and every note is in key — a project must never be told
    /// its notes are wrong because nobody set a field.
    #[serde(default)]
    pub key: Option<Key>,
    /// Named points on the timeline — the one feature both the FL Studio
    /// and kdenlive communities ask for in the same words ("markers",
    /// "jump to", "delete all"). Sorted by position on read, so the
    /// keyboard's next/previous is just a scan.
    #[serde(default)]
    pub markers: Vec<Marker>,
}

/// A key: a root pitch class (0 = C) and a scale.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Key {
    pub root: u8,
    pub scale: Scale,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Scale { Major, Minor, Dorian, Mixolydian, Chromatic }

impl Scale {
    /// Semitone offsets from the root.
    pub fn degrees(self) -> &'static [u8] {
        match self {
            Scale::Major => &[0, 2, 4, 5, 7, 9, 11],
            Scale::Minor => &[0, 2, 3, 5, 7, 8, 10],
            Scale::Dorian => &[0, 2, 3, 5, 7, 9, 10],
            Scale::Mixolydian => &[0, 2, 4, 5, 7, 9, 10],
            Scale::Chromatic => &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Scale::Major => "major", Scale::Minor => "minor",
            Scale::Dorian => "dorian", Scale::Mixolydian => "mixolydian",
            Scale::Chromatic => "chromatic",
        }
    }
}

/// The twelve names, sharps — the spelling a piano roll uses.
pub const PITCH_NAMES: [&str; 12] =
    ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// A roll key's MIDI number. Key 0 is A1 (55 Hz, MIDI 33) — the rack's
/// own `sub_freq` reference, so a label and a pitch cannot disagree.
pub const KEY_ZERO_MIDI: i32 = 33;

pub fn key_midi(key: i8) -> i32 { KEY_ZERO_MIDI + key as i32 }

/// The label a piano roll shows: `A1` for key 0, `C#4`, `A5`.
pub fn note_label(key: i8) -> String {
    let m = key_midi(key);
    let pc = m.rem_euclid(12) as usize;
    let octave = m.div_euclid(12) - 1;
    format!("{}{}", PITCH_NAMES[pc], octave)
}

impl Key {
    /// Is this roll key in the scale? A `None` key (no opinion) is handled
    /// by the caller — everything is in key when nothing was chosen.
    pub fn contains(&self, key: i8) -> bool {
        let pc = (key_midi(key) - self.root as i32).rem_euclid(12) as u8;
        self.scale.degrees().contains(&pc)
    }
    pub fn name(&self) -> String {
        format!("{} {}", PITCH_NAMES[(self.root % 12) as usize], self.scale.name())
    }
    /// The roll key `steps` SCALE degrees away from `key` — up for
    /// positive, down for negative. A third is 2 degrees, a fifth 4, an
    /// octave the scale's length. A key that is not in the scale steps
    /// from the nearest scale note below it, so an out-of-key note still
    /// harmonizes to something in the key rather than nothing.
    pub fn scale_step(&self, key: i8, steps: i32) -> i8 {
        let degrees = self.scale.degrees();
        let n = degrees.len() as i32;
        let pc = (key_midi(key) - self.root as i32).rem_euclid(12) as u8;
        // Degree index of the note, or of the nearest scale note below.
        let idx = degrees.iter().rposition(|&d| d <= pc).unwrap_or(0) as i32;
        let base = key as i32 - (pc as i32 - degrees[idx as usize] as i32);
        let target = idx + steps;
        let octaves = target.div_euclid(n);
        let deg = degrees[target.rem_euclid(n) as usize] as i32;
        (base + octaves * 12 + (deg - degrees[idx as usize] as i32)).clamp(i8::MIN as i32, i8::MAX as i32) as i8
    }
}

/// A named point on the timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Marker {
    pub at: Ticks,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Group {
    pub name: String,
    pub clips: Vec<ClipId>,
    /// This song's own key, independent of `Project.key` — an album
    /// where each song is in a different key. `None` means "no opinion,"
    /// same meaning as the project-wide key's `None`: every note is in
    /// key when nobody set one, never "this song's notes are wrong."
    /// `#[serde(default)]` so a project saved before this field existed
    /// still loads, with every group reading as "no opinion" rather than
    /// failing to deserialize.
    #[serde(default)]
    pub key: Option<Key>,
    /// The span of its own contents this comp SHOWS, absolute ticks.
    ///
    /// `None` — the default, and what every existing project loads as —
    /// means "all of it", and the comp's extent stays its members' min..max.
    ///
    /// This exists so splitting a folded comp is a TRIM OF THE
    /// REPRESENTATION rather than an edit of the material. Without a window
    /// there was nowhere to record "this comp shows the first half", so the
    /// only way to halve a comp was to cut the clips inside it — which is the
    /// bug: content was spliced out of the comp, and a comp is a view OF the
    /// content, not a copy of it.
    ///
    /// Members are never touched by a window. Two comps can show
    /// non-overlapping halves of the SAME clips, and deleting one leaves the
    /// material whole.
    ///
    /// ## What a window does NOT do: change what you hear
    ///
    /// I twice wrote that "playback must honour the window or a trimmed comp
    /// still sounds whole". That was wrong, and worth correcting here rather
    /// than leaving as a phantom to-do.
    ///
    /// A group gates nothing audible. It carries a name, a membership, a
    /// key, a tempo map and an active TAKE — playback reads clips off tracks
    /// and never consults a group at all. So splitting a comp changes what a
    /// gesture grabs and what the fold draws, and changes nothing about the
    /// mix. Which is correct and is the same rule clips already follow:
    /// splitting a clip does not change what you hear either.
    ///
    /// Removing half a comp is the case where a user might expect audio to
    /// change, and it does not: `RemoveGroup` removes the VIEW. Deleting the
    /// material is a separate gesture on the clips, which is the right split
    /// of responsibilities — a fold is not a container, and unfolding must
    /// never be able to lose a take.
    #[serde(default)]
    pub window: Option<(Ticks, Ticks)>,
    /// This song's own tempo map, independent of every other song's.
    /// `None` means "no opinion" — the timeline draws this song's beat
    /// grid at a DAW-default 120 BPM rather than refusing to draw one,
    /// same "no opinion is not an error" shape as `key`. `#[serde(default)]`
    /// for the same reason: a project saved before this field existed
    /// still loads.
    #[serde(default)]
    pub tempo: Option<TempoMap>,
    /// Which member is the "active take", when this group is a
    /// take-bucket rather than a plain composition. `None` = not a
    /// bucket, or no choice made -- an ordinary group, exactly as before
    /// this field existed. `#[serde(default)]` for the usual reason.
    #[serde(default)]
    pub active: Option<ClipId>,
}

/// Archive-mode provenance for one source: what it was last verified as on
/// disk, and — for a proxy standing in for deleted original media — how to
/// get the real thing back.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Provenance {
    /// Whole-file SHA-256 of THIS source's media, as of `verified_at`.
    pub content_hash: [u8; 32],
    pub file_size: u64,
    /// Where this exact content was found, last time it was verified —
    /// may differ from the source's OWN registered path if a relocation
    /// search found it moved. `None` if it has never been found again
    /// since being recorded (deleted, or not yet searched for).
    pub last_verified_path: Option<String>,
    /// Freeform timestamp (ISO 8601, or whatever the caller uses) of the
    /// last verification — deliberately not a typed date: this crate has
    /// no reason to parse it, only to show it.
    pub verified_at: Option<String>,
    /// Set when this source is a lower-quality stand-in for original
    /// media that no longer exists at any known location.
    #[serde(default)]
    pub proxy_of: Option<ProxyInfo>,
}

/// What's known about the original media a proxy replaced.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProxyInfo {
    /// The original's own content hash, if it was ever computed before
    /// the original was lost — lets a re-acquired copy be VERIFIED as the
    /// same footage, not just assumed to be.
    pub original_hash: Option<[u8; 32]>,
    /// How to get the original back: a camera/card name, a URL, a shoot
    /// date, a person to ask — whatever the operator recorded. Freeform on
    /// purpose; this crate has no way to act on it, only to show it.
    pub reacquire_hint: Option<String>,
    /// Why this proxy exists, in the operator's own words ("original
    /// deleted after import to free the card", "downloaded compressed
    /// copy, original never had"). Distinct from `reacquire_hint`: this is
    /// the "why", that is the "how".
    pub note: Option<String>,
}

/// A source's perceptual temporal fingerprint, stored as pure data --
/// computed by `studio_video::fingerprint` (which needs `ffmpeg`), kept
/// here so this crate can hold and serialize the RESULT without depending
/// on anything that shells out. One dHash per sampled position, evenly
/// spaced by `interval`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FingerprintData {
    pub hashes: Vec<u64>,
    pub interval: Ticks,
}

/// An effect parameter's key, fixed-width so an [`AutoTarget`] stays
/// `Copy` and an op stays fixed-shape on the wire: up to 16 ASCII bytes,
/// NUL-padded. Every key `studio-fx`'s schema declares fits (the longest,
/// `preserve_pitch`, is 14); one that does not is refused at construction
/// rather than truncated into a different key.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ParamKey(pub [u8; 16]);

impl ParamKey {
    pub fn new(key: &str) -> Option<ParamKey> {
        let b = key.as_bytes();
        if b.is_empty() || b.len() > 16 || !b.iter().all(|c| c.is_ascii() && *c != 0) { return None; }
        let mut k = [0u8; 16];
        k[..b.len()].copy_from_slice(b);
        Some(ParamKey(k))
    }
    pub fn as_str(&self) -> &str {
        let n = self.0.iter().position(|&c| c == 0).unwrap_or(16);
        std::str::from_utf8(&self.0[..n]).unwrap_or("")
    }
}

impl std::fmt::Debug for ParamKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { write!(f, "ParamKey({:?})", self.as_str()) }
}
impl std::fmt::Display for ParamKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(self.as_str()) }
}

/// What an automation lane drives. Track and bus gain first — the volume
/// ride is the automation everyone reaches for — and now an effect
/// parameter on a bus's or a clip's chain, addressed by the effect's
/// index in that chain and the parameter's key.
///
/// A parameter lane follows the fader rule: the value the effect runs
/// with is `stored × lane × Π(bindings)`, so 1.0 is transparent and a
/// curve is a ride on top of whatever the knob says — a filter sweep is
/// a cutoff at 1 kHz and a curve from 0.1 to 8. The render applies it at
/// control rate (a block of frames, not each sample) through
/// `studio_fx::Instance::set_param`, which changes only what an effect
/// can change without losing its state; a key it cannot move live is
/// ignored, and the schema says which those are.
///
/// The index, not an effect id: effects have none — a chain is a Vec and
/// `SetBusChain` replaces it whole — so a lane on `effect: 1` follows
/// whatever sits second. Reordering the chain moves the ride, the same
/// way deleting a clip leaves a binding dangling and harmless.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum AutoTarget {
    TrackGain(TrackId),
    BusGain(BusId),
    BusParam { bus: BusId, effect: u8, key: ParamKey },
    ClipParam { clip: ClipId, effect: u8, key: ParamKey },
}

/// A lane's value at `at`: linear between points, held flat before the
/// first and after the last (the ends are what the automation last said,
/// not a cliff back to unity), and 1.0 — a transparent ride — for a lane
/// with no points at all.
/// [`eval_lane`] for a caller sweeping time FORWARD, which every renderer
/// is: `cursor` remembers the segment last used, so a lane of ten thousand
/// points costs one comparison per sample instead of a binary search.
/// Falls back to the search whenever `at` moves backwards (a loop wrap, a
/// seek), so it is exactly `eval_lane` for any input — pinned by a test
/// that sweeps both and compares every sample.
pub fn eval_lane_cursor(points: &[(Ticks, f32)], at: Ticks, cursor: &mut usize) -> f32 {
    if points.is_empty() { return 1.0; }
    if *cursor >= points.len() || (*cursor > 0 && points[*cursor - 1].0 > at) {
        *cursor = points.partition_point(|(t, _)| *t <= at);
    }
    while *cursor < points.len() && points[*cursor].0 <= at { *cursor += 1; }
    let i = *cursor;
    if i == 0 { return points[0].1.max(0.0); }
    if i == points.len() { return points[i - 1].1.max(0.0); }
    let (t0, v0) = points[i - 1];
    let (t1, v1) = points[i];
    let span = (t1.0 - t0.0).max(1) as f64;
    let frac = ((at.0 - t0.0) as f64 / span) as f32;
    (v0 + (v1 - v0) * frac).max(0.0)
}

pub fn eval_lane(points: &[(Ticks, f32)], at: Ticks) -> f32 {
    if points.is_empty() { return 1.0; }
    let i = points.partition_point(|(t, _)| *t <= at);
    if i == 0 { return points[0].1.max(0.0); }
    if i == points.len() { return points[i - 1].1.max(0.0); }
    let (t0, v0) = points[i - 1];
    let (t1, v1) = points[i];
    let span = (t1.0 - t0.0).max(1) as f64;
    let frac = ((at.0 - t0.0) as f64 / span) as f32;
    (v0 + (v1 - v0) * frac).max(0.0)
}

/// A tempo map: piecewise-constant BPM over the timeline, per song.
///
/// The failure this exists to avoid is named directly in the request that
/// prompted it: stretching every clip to fit one fixed workspace tempo,
/// which is what makes tempo changes hard to align in the tools that try
/// it. This does the opposite — clips keep their own native timing in
/// TICKS (already time, not beats, per [`Ticks`]'s own definition), and a
/// tempo map is a read-only LENS a beat grid or a quantizer looks through
/// to ask "which beat is at this tick" or "which tick is beat N". Nothing
/// about a clip's audio or its tick position changes when the tempo map
/// changes underneath it — exactly the "detent markers with variable
/// offset" framing: the detents move, the clips don't.
///
/// A segment's BPM holds from its own tick UP TO the next segment's tick
/// (or forever, for the last one) — the same "held flat, changes at a
/// point" shape [`eval_lane`] uses for automation, deliberately, since a
/// tempo change is a step (bar 17 becomes 140 BPM), not a ramp, in every
/// piece of music notation and every DAW this was checked against.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TempoMap {
    /// Sorted ascending by `.0`. Never empty after construction — see
    /// [`TempoMap::new`] — so every lookup has at least one segment to
    /// read, the same reason `Project` never needs an `Option` around it
    /// at the call site even where the SONG's map itself is optional.
    points: Vec<(Ticks, f32)>,
}

/// Below this, a beat stops meaning anything a grid can draw at a legible
/// spacing (at 1 BPM, one bar is a minute wide) and a segment length of
/// zero divides by it. Not a realistic tempo either way — the floor exists
/// to keep the map's own arithmetic finite, not to bless slow music.
const MIN_BPM: f32 = 1.0;

impl TempoMap {
    /// A constant-tempo map — what a song with "just set the tempo" wants,
    /// and the only way to build one with no risk of an out-of-order or
    /// non-positive point sneaking in through construction.
    pub fn constant(bpm: f32) -> TempoMap {
        TempoMap { points: vec![(Ticks::ZERO, bpm.max(MIN_BPM))] }
    }

    /// Build from explicit (tick, bpm) points, in any order, with any
    /// BPM. Sorted and floored on the way in — every OTHER method on this
    /// type gets to assume sorted-ascending-and-positive rather than
    /// re-checking it, the same trade `Marker` storage already makes by
    /// sorting once on write. Always has at least one point: an empty
    /// input becomes a single implicit 120 BPM point at `Ticks::ZERO`,
    /// the genre-agnostic DAW default, so "no opinion" still answers every
    /// query rather than needing its own `Option` branch at every call
    /// site.
    pub fn new(mut points: Vec<(Ticks, f32)>) -> TempoMap {
        for p in &mut points { p.1 = p.1.max(MIN_BPM); }
        points.sort_by_key(|(t, _)| *t);
        points.dedup_by_key(|(t, _)| *t);
        if points.is_empty() {
            points.push((Ticks::ZERO, 120.0));
        }
        TempoMap { points }
    }

    pub fn points(&self) -> &[(Ticks, f32)] { &self.points }

    /// The tempo in effect at `at` — the segment it falls in, held flat
    /// before the first point (there is no "before the first," since
    /// construction guarantees one exists, but a tick before the first
    /// point's own tick still reads as that point's tempo, same as
    /// `eval_lane`'s pre-first-point rule).
    pub fn bpm_at(&self, at: Ticks) -> f32 {
        let i = self.points.partition_point(|(t, _)| *t <= at);
        self.points[i.saturating_sub(1).min(self.points.len() - 1)].1
    }

    /// Ticks per beat at a given tempo — the one conversion factor
    /// everything else here is built from.
    fn ticks_per_beat(bpm: f32) -> f64 {
        TICKS_PER_SECOND as f64 * 60.0 / bpm as f64
    }

    /// How many beats have elapsed by `at`, counting from beat 0 at
    /// `Ticks::ZERO` — fractional, so a tick that lands mid-beat gives a
    /// mid-beat answer rather than truncating. Walks every segment before
    /// `at` and sums its beat length, which is the direct, unoptimized
    /// version; a cursor variant (mirroring `eval_lane_cursor`) is the
    /// obvious next step if a beat-grid renderer ever calls this once per
    /// visible beat per frame, but nothing does yet.
    pub fn tick_to_beat(&self, at: Ticks) -> f64 {
        let mut beats = 0.0f64;
        for i in 0..self.points.len() {
            let (seg_start, bpm) = self.points[i];
            let seg_end = self.points.get(i + 1).map(|(t, _)| *t).unwrap_or(Ticks(i64::MAX));
            if at <= seg_start { break; }
            let span_end = at.min(seg_end);
            let span_ticks = (span_end.0 - seg_start.0).max(0) as f64;
            beats += span_ticks / Self::ticks_per_beat(bpm);
            if at <= seg_end { break; }
        }
        beats
    }

    /// The inverse of [`Self::tick_to_beat`]: which tick is beat number
    /// `beat` (fractional beats allowed — "beat 4.5" is the off-beat).
    /// Walks the same segments accumulating beats until `beat` falls
    /// inside one, then converts the REMAINDER within that segment back
    /// to ticks at that segment's own tempo — never assumes one global
    /// ticks-per-beat, which is the whole reason this is a map and not a
    /// single number.
    pub fn beat_to_tick(&self, beat: f64) -> Ticks {
        if beat <= 0.0 { return self.points[0].0; }
        let mut acc_beats = 0.0f64;
        for i in 0..self.points.len() {
            let (seg_start, bpm) = self.points[i];
            let seg_end_tick = self.points.get(i + 1).map(|(t, _)| *t);
            let seg_beats = match seg_end_tick {
                Some(end) => (end.0 - seg_start.0).max(0) as f64 / Self::ticks_per_beat(bpm),
                None => f64::INFINITY, // the last segment never ends
            };
            if beat <= acc_beats + seg_beats || seg_end_tick.is_none() {
                let remainder_beats = beat - acc_beats;
                let remainder_ticks = remainder_beats * Self::ticks_per_beat(bpm);
                return Ticks(seg_start.0 + remainder_ticks.round() as i64);
            }
            acc_beats += seg_beats;
        }
        unreachable!("the last segment has no end, so the loop always returns inside it")
    }
}

/// What a [`Source`] actually is.
///
/// Before this, every source was a file on disk, so a drum pattern had to be
/// baked to audio once (`studio_live::render_pattern`) and imported like any
/// other sample. That is why "Trap A" appears on the timeline as `demo.wav`:
/// by the time the timeline saw it, the pattern was gone and only the render
/// remained. The bake also froze it — editing the pattern did nothing to
/// clips already placed, because they referenced the rendered file rather
/// than the pattern.
///
/// Internally tagged and defaulting to [`SourceKind::File`], so a project
/// written before this field existed still loads: absent `kind` means file,
/// which is what every old source was.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceKind {
    /// Audio or video decoded from [`Source::path`].
    #[default]
    File,
    /// A generator: the project's pattern at `pattern` renders on demand.
    ///
    /// `name` is denormalised from the pattern deliberately. A clip must be
    /// able to label itself without reaching into `project.patterns` — the
    /// timeline draws clips for tracks whose pattern may have been removed,
    /// and a clip that renders as a blank rectangle because its generator is
    /// gone is worse than one that still says "Trap A" and reports itself
    /// dangling.
    Pattern { pattern: usize, name: String },
}

impl SourceKind {
    pub fn is_generator(&self) -> bool {
        matches!(self, SourceKind::Pattern { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Source {
    pub id: SourceId,

    /// Where the media lives, for [`SourceKind::File`]. Empty for a
    /// generator, which has nothing on disk until it is exported.
    pub path: String,
    pub len: Ticks,
    pub sample_rate: u32,
    pub channels: u16,
    /// True when the source carries picture as well as sound.
    pub has_video: bool,
    /// File or generator. Defaults to `File` so pre-existing projects load
    /// unchanged.
    #[serde(default)]
    pub kind: SourceKind,
}

impl Source {
    /// A file-backed source — what every source was before generators.
    pub fn file(id: SourceId, path: impl Into<String>, len: Ticks, sample_rate: u32, channels: u16, has_video: bool) -> Source {
        Source { id, path: path.into(), len, sample_rate, channels, has_video, kind: SourceKind::File }
    }

    /// A pattern-backed generator source. No path: nothing is on disk.
    pub fn pattern(id: SourceId, pattern: usize, name: impl Into<String>, len: Ticks, sample_rate: u32, channels: u16) -> Source {
        let name = name.into();
        Source {
            id,
            path: String::new(),
            len,
            sample_rate,
            channels,
            has_video: false,
            kind: SourceKind::Pattern { pattern, name },
        }
    }

    pub fn is_generator(&self) -> bool {
        self.kind.is_generator()
    }

    /// What the timeline should write on a clip of this source.
    ///
    /// The whole point of the type: a pattern says its own name, a file says
    /// its stem. Callers must not derive this from `path` themselves — that
    /// is the bug that put `demo.wav` on a drum pattern.
    pub fn display_name(&self) -> &str {
        match &self.kind {
            SourceKind::Pattern { name, .. } => name.as_str(),
            SourceKind::File => {
                // Stem without allocating: find the last separator and drop a
                // trailing extension.
                let file = self.path.rsplit(['/', '\\']).next().unwrap_or(&self.path);
                match file.rfind('.') {
                    // A dotfile ("..wav" or ".gitignore") has no stem before
                    // the dot, so keep the whole name rather than returning
                    // an empty label.
                    Some(0) | None => file,
                    Some(i) => &file[..i],
                }
            }
        }
    }

    /// Is this source's generator still present?
    ///
    /// Takes a COUNT rather than the patterns themselves: `Pattern` lives in
    /// `studio-live`, which depends on this crate, and reaching the other way
    /// would invert that. The caller has the project.
    pub fn generator_is_live(&self, pattern_count: usize) -> bool {
        match &self.kind {
            SourceKind::File => true,
            SourceKind::Pattern { pattern, .. } => *pattern < pattern_count,
        }
    }
}

fn unity_gain() -> f32 { 1.0 }

/// Every edit, as a value.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Op {
    AddTrack { track: Track, at: usize },
    RemoveTrack { id: TrackId },
    SetTrackFlags { id: TrackId, muted: bool, locked: bool },
    /// The track fader. Refused on a locked track — the fader is a mix
    /// edit, not the lock toggle; `SetTrackFlags` stays the only edit a
    /// locked track accepts, because something has to be able to unlock it.
    SetTrackGain { id: TrackId, gain: f32 },
    /// Place (or move, or restate) one automation point: absolute (time,
    /// value), so two peers editing the same lane converge per-point the
    /// way clips converge per-field — a point at the same tick is the same
    /// point, and the last writer's value wins on every replica. Refused on
    /// a locked track for `SetTrackGain`'s reason: a ride is a mix edit.
    SetAutoPoint { target: AutoTarget, at: Ticks, value: f32 },
    /// Remove the point at exactly (target, at). Refused when there is no
    /// such point — the inverse an undo needs (`SetAutoPoint` of the old
    /// value) would otherwise have nothing to restore.
    RemoveAutoPoint { target: AutoTarget, at: Ticks },
    AddClip { clip: Clip },
    RemoveClip { id: ClipId },
    /// Absolute placement, not a delta — see the module docs on convergence.
    MoveClip { id: ClipId, pos: Ticks, track: TrackId },
    /// Absolute in-point and length, so two peers trimming opposite edges of
    /// the same clip do not fight over a shared "delta" they each computed
    /// against a different starting state.
    TrimClip { id: ClipId, src_in: Ticks, len: Ticks },
    SetGain { id: ClipId, gain: f32 },
    SetEffects { id: ClipId, effects: Vec<Effect> },
    AddSource { source: Source },
    /// Refused while any clip still references this source (see
    /// `Op::RemoveTrack`/`Op::RemoveBus` for the same "in use" refusal
    /// pattern) — before this variant existed, `Op::AddSource`'s own
    /// "inverse" was itself (a workaround for idempotent duplicate catch-up
    /// delivery, not a real undo), which meant recording a source and then
    /// undoing left the source permanently in the project. This is the fix.
    RemoveSource { id: SourceId },
    /// Point a source at a different file: the same media somewhere else, or
    /// the camera original in place of the proxy that was cut with.
    ///
    /// Only the path moves. Length, rate, channels and `has_video` stay as
    /// they are, because every clip's trim was validated against them — a
    /// relink that silently changed the length would move cuts. A file that
    /// really is a different length is an import, not a relink.
    SetSourcePath { id: SourceId, path: String },
    /// Refused if `bus.parent` doesn't exist, or is the bus's own id (a
    /// direct self-loop) — see `Project::apply`'s cycle check for why a
    /// longer cycle can never arise from `AddBus` alone.
    AddBus { bus: Bus },
    /// Refused while anything still points at this bus — a track routed to
    /// it, another bus's `parent`, or another bus's `sends` — so deletion
    /// never silently reroutes audio; the caller re-routes first, same
    /// principle as a locked track refusing edits.
    RemoveBus { id: BusId },
    SetBusGain { id: BusId, gain: f32 },
    /// Bus-level mute, persisted like `SetTrackFlags` — unlike solo, which
    /// (as for tracks) is a mixdown-time parameter, not project state: it's
    /// a listening choice for right now, not an edit worth undoing or
    /// shipping to a collaborator.
    SetBusFlags { id: BusId, muted: bool },
    /// See [`Bus::silent`]. A separate op rather than another field on
    /// `SetBusFlags` so journals and macros written before silent busses
    /// existed still deserialize.
    SetBusSilent { id: BusId, silent: bool },
    SetBusChain { id: BusId, chain: Vec<Effect> },    /// Replaces the whole sends list, like `SetEffects` does for a clip's
    /// effects — the inverse is then just "the old vector," no per-send
    /// add/remove bookkeeping needed.
    SetBusSends { id: BusId, sends: Vec<Send> },
    /// `bus: None` routes the track straight to master.
    SetRoute { track: TrackId, bus: Option<BusId> },
    /// Re-parent a bus — the INPUT side of the graph, so a mixer strip can
    /// say "this feeds me" instead of routing only ever being edited from
    /// the source end and only at creation. `None` is master. Refused when
    /// the new parent chain reaches back to `id`: a bus cannot feed itself.
    SetBusParent { id: BusId, parent: Option<BusId> },
    /// Create or restate a composition, absolutely — name and full
    /// membership every time, the LWW-friendly shape. Refused empty: a
    /// group of nothing folds to nothing.
    SetGroup { id: GroupId, name: String, clips: Vec<ClipId> },
    /// Dissolve a composition. Its clips are untouched — ungrouping is an
    /// organizational act, never an edit to the music.
    RemoveGroup { id: GroupId },
    /// Set (or clear) a source's provenance record, absolutely -- same
    /// LWW-friendly shape as `SetGroup`. A source with no record is
    /// ordinary; this is additive metadata, never required for a project
    /// to be valid.
    SetProvenance { source: SourceId, provenance: Provenance },
    RemoveProvenance { source: SourceId },
    SetFingerprint { source: SourceId, fingerprint: FingerprintData },
    RemoveFingerprint { source: SourceId },
    /// Set (or clear) the song's key. Absolute, like every other setting
    /// here, so two writers converge on the last word rather than on a
    /// sum of transpositions.
    SetKey { key: Option<Key> },
    /// Same, scoped to one song within a multi-song project — an album
    /// where each track is in a different key. Deliberately its own op
    /// rather than a field folded into `SetGroup`: `SetGroup` restates
    /// name+membership absolutely on every rename, and a bare key field
    /// there would be silently overwritten (or need every existing
    /// `SetGroup` caller to start threading a key through a rename that
    /// has nothing to do with key). Refused on a group that does not
    /// exist — unlike the project-wide key, which always has a project
    /// to belong to, a group-scoped key needs a group.
    SetGroupKey { id: GroupId, key: Option<Key> },
    /// Set (or clear) the span of its contents a comp shows.
    ///
    /// Its own op rather than a field on `SetGroup`, for the reason given
    /// there: `SetGroup` restates name and membership, so folding the window
    /// into it would make every trim also rewrite the membership, and a
    /// concurrent rename would silently revert the trim.
    SetGroupWindow { id: GroupId, window: Option<(Ticks, Ticks)> },
    /// Same shape as `SetGroupKey`, for a song's tempo map — its own op
    /// for the identical reason: `SetGroup` restates name+membership on
    /// every rename and must not be the thing a tempo map rides along
    /// with, or a rename would need to start threading tempo through too.
    /// Refused on a group that does not exist, same as `SetGroupKey`.
    SetGroupTempo { id: GroupId, tempo: Option<TempoMap> },
    /// Same shape again, for which member is the "active take" -- the one
    /// that actually renders/exports when the group is a take-bucket
    /// rather than a plain composition. Pure data: this field carries no
    /// opinion about how a UI shows it (badge, thumbnail, data view,
    /// nothing at all) -- every view mode reads the SAME `active` value,
    /// there is exactly one of it, never a per-view copy. `None` means
    /// "not a take-bucket" / "no choice made yet", not an error -- a group
    /// with no active member is an ordinary composition, unchanged from
    /// before this field existed. Refused on a group that does not exist,
    /// same as `SetGroupKey`/`SetGroupTempo`. Does NOT validate that
    /// `active` is actually a member of `clips` -- a take temporarily
    /// removed from the bucket (or a bucket about to gain a new take)
    /// should not have to clear this first; a UI reads a dangling active
    /// pointer the same way it already reads a dangling group member.
    SetGroupActive { id: GroupId, active: Option<ClipId> },
    /// Add or move a named marker. Absolute by NAME: setting a marker
    /// that already exists moves it, which is what "set the chorus here"
    /// means when you meant it the second time.
    /// Set a clip's fade in/out, in ticks from its own start and end.
    ///
    /// Both in one op because they are one gesture in every editor that has
    /// them — you drag a clip's corner — and two ops would let `u` leave half
    /// a crossfade. `(0, 0)` removes the entry rather than storing a no-op
    /// fade, which keeps the map sparse and makes "has a fade" a real question.
    SetClipFade { id: ClipId, fade_in: Ticks, fade_out: Ticks },
    /// A bus's pan (see `Project::bus_pans`); 0 = centre.
    SetBusPan { id: BusId, pan: f32 },
    SetMarker { at: Ticks, name: String },
    RemoveMarker { name: String },
    /// Both communities ask for this by name ("Delete All Timeline
    /// Markers"). One op, one undo — a hundred RemoveMarkers would be a
    /// hundred presses of `u`.
    ClearMarkers,
    /// Create or rename a reusable automation curve, absolutely (the
    /// `SetGroup` shape: the last name written wins on every replica).
    SetCurve { id: CurveId, name: String },
    /// Remove a curve. Refused while any binding names it — the
    /// `RemoveSource` rule, so deleting a curve can never silently
    /// un-automate twelve clips. The inverse restores name AND points as
    /// one `Group`, the `ClearMarkers` precedent.
    RemoveCurve { id: CurveId },
    /// Place, move or restate one point on a curve, in CURVE-LOCAL time —
    /// exactly `SetAutoPoint`'s semantics one level of indirection over.
    SetCurvePoint { curve: CurveId, at: Ticks, value: f32 },
    RemoveCurvePoint { curve: CurveId, at: Ticks },
    /// Aim a curve at a target through a gate, or restate the aim. Absolute
    /// (whole binding every time). Refused for a missing curve, a missing
    /// target, a locked track's gain (a ride is a mix edit), a gate on a
    /// missing clip, or an empty window.
    SetBinding { id: BindingId, binding: Binding },
    RemoveBinding { id: BindingId },
    /// Several ops as ONE step: applied in order, inverted as a whole (the
    /// inverse is the members' inverses in reverse). A partial application
    /// rolls back what landed — half an edit is worse than none. This is
    /// what lets a multi-move mastering pass be a single press of `u`.
    Group { ops: Vec<Op> },
}

#[derive(Debug, Clone, PartialEq)]
pub enum EditError {
    NoSuchClip(ClipId),
    /// A `RemoveAutoPoint` aimed at a point that is not there.
    NoSuchAutoPoint,
    /// A `SetGroup` with no members.
    EmptyGroup,
    /// A comp split at a tick outside the comp's own span. Named like
    /// `OutOfSource`, which refuses the same mistake on a clip: a cut at or
    /// past an edge is a non-event, not an empty half.
    OutOfComp { id: GroupId },
    /// A marker must have a name — an unnamed one cannot be jumped to.
    /// A fade cannot be negative.
    NegativeFade { id: ClipId },
    /// `fade_in + fade_out` exceeds the clip. Refused rather than clamped:
    /// clamping silently changes what was asked for, and `gain_at` would cope
    /// either way, which is what makes the silence dangerous.
    FadeLongerThanClip { id: ClipId, len: Ticks },
    EmptyMarker,
    NoSuchMarker(String),
    NoSuchGroup(GroupId),
    NoSuchTrack(TrackId),
    TrackLocked(TrackId),
    /// Refused because clips are still on this track — see `Op::RemoveTrack`.
    /// Removing them (or moving them off first) is the caller's job; a
    /// track's own removal must never be the thing that silently deletes
    /// clips undo can't bring back.
    TrackInUse(TrackId),
    /// Two clips would occupy the same span of the same track.
    Overlap { id: ClipId, with: ClipId },
    /// A trim that would read past the end of the source, or a negative length.
    OutOfSource { id: ClipId },
    NoSuchBus(BusId),
    /// Something still routes into this bus — see `Op::RemoveBus`.
    BusInUse(BusId),
    /// Adding this edge (a `parent` or a `send`) would close a loop back to
    /// `id`, which would make mixdown never terminate.
    BusCycle { id: BusId },
    NoSuchSource(SourceId),
    /// A clip still references this source — see `Op::RemoveSource`.
    SourceInUse(SourceId),
    NoSuchCurve(CurveId),
    /// A binding still names this curve — see `Op::RemoveCurve`.
    CurveInUse(CurveId),
    NoSuchBinding(BindingId),
    /// A `Gate::Window` whose end is not after its start can never open.
    EmptyWindow,
    /// A comp span that no single take on `lane` covers whole — see
    /// `Project::comp_ops`.
    NoTakeOver { lane: TrackId, from: Ticks, to: Ticks },
}

/// The (left, right) gains of a balance at `pan` (-1..=1): the far side is
/// turned down linearly, the near side left alone.
pub fn balance_gains(pan: f32) -> (f32, f32) {
    let p = pan.clamp(-1.0, 1.0);
    ((1.0 - p).min(1.0), (1.0 + p).min(1.0))
}

impl Project {
    /// Apply an op, returning the op that undoes it.
    ///
    /// The inverse is computed from the state BEFORE the change, which is what
    /// makes undo exact rather than approximate — and it is also why undo and
    /// collaboration share one mechanism: both are just ops.
    /// An automation target must exist to be ridden, and a locked track
    /// refuses its lane the way it refuses its fader.
    fn check_auto_target(&self, target: AutoTarget) -> Result<(), EditError> {
        match target {
            AutoTarget::TrackGain(id) => self.check_track(id),
            AutoTarget::BusGain(id) | AutoTarget::BusParam { bus: id, .. } => {
                // The bus must exist; the effect index need not, yet — a
                // lane may be drawn before the chain is built, the same
                // way a binding may outlive its clip.
                if self.busses.contains_key(&id) { Ok(()) } else { Err(EditError::NoSuchBus(id)) }
            }
            AutoTarget::ClipParam { clip, .. } => {
                let c = self.clips.get(&clip).ok_or(EditError::NoSuchClip(clip))?;
                self.check_track(c.track)
            }
        }
    }

    pub fn apply(&mut self, op: &Op) -> Result<Op, EditError> {
        match op {
            Op::AddTrack { track, at } => {
                let at = (*at).min(self.tracks.len());
                self.tracks.insert(at, track.clone());
                Ok(Op::RemoveTrack { id: track.id })
            }
            Op::RemoveTrack { id } => {
                let idx = self.tracks.iter().position(|t| t.id == *id)
                    .ok_or(EditError::NoSuchTrack(*id))?;
                // Removing a track that still has clips on it would need to
                // return more than one inverse op (the track AND every clip)
                // to be undoable — and `apply` only ever returns one. Rather
                // than silently cascading and losing those clips on undo
                // (which is what this arm used to do — a real bug, caught
                // by noticing the comment describing "the clips' own
                // inverse ops" promised something the return type can't
                // deliver), this refuses outright, the same "in use" refusal
                // `Op::RemoveBus` already applies for exactly this reason.
                // The caller removes the clips first — each of THOSE has its
                // own exact single-op inverse, so nothing is lost either way.
                if self.clips.values().any(|c| c.track == *id) {
                    return Err(EditError::TrackInUse(*id));
                }
                let track = self.tracks.remove(idx);
                Ok(Op::AddTrack { track, at: idx })
            }
            Op::SetTrackFlags { id, muted, locked } => {
                let t = self.tracks.iter_mut().find(|t| t.id == *id)
                    .ok_or(EditError::NoSuchTrack(*id))?;
                let inv = Op::SetTrackFlags { id: *id, muted: t.muted, locked: t.locked };
                t.muted = *muted;
                t.locked = *locked;
                Ok(inv)
            }
            Op::SetTrackGain { id, gain } => {
                self.check_track(*id)?; // exists, and not locked
                let t = self.tracks.iter_mut().find(|t| t.id == *id)
                    .ok_or(EditError::NoSuchTrack(*id))?;
                let inv = Op::SetTrackGain { id: *id, gain: t.gain };
                t.gain = *gain;
                Ok(inv)
            }
            Op::SetAutoPoint { target, at, value } => {
                self.check_auto_target(*target)?;
                let lane = self.automation.entry(*target).or_default();
                match lane.iter_mut().find(|(t, _)| t == at) {
                    Some(p) => {
                        // Restating a point at its own tick is a value
                        // change; the inverse restores the old value.
                        let inv = Op::SetAutoPoint { target: *target, at: *at, value: p.1 };
                        p.1 = *value;
                        Ok(inv)
                    }
                    None => {
                        lane.push((*at, *value));
                        lane.sort_by_key(|(t, _)| *t);
                        Ok(Op::RemoveAutoPoint { target: *target, at: *at })
                    }
                }
            }
            Op::RemoveAutoPoint { target, at } => {
                self.check_auto_target(*target)?;
                let lane = self.automation.get_mut(target)
                    .ok_or(EditError::NoSuchAutoPoint)?;
                let i = lane.iter().position(|(t, _)| t == at)
                    .ok_or(EditError::NoSuchAutoPoint)?;
                let (_, old) = lane.remove(i);
                // An emptied lane drops its entry — sparse, the same rule
                // `Pattern::set_mod` follows one crate over.
                if lane.is_empty() { self.automation.remove(target); }
                Ok(Op::SetAutoPoint { target: *target, at: *at, value: old })
            }
            Op::SetCurve { id, name } => {
                match self.curves.get_mut(id) {
                    Some(c) => {
                        let inv = Op::SetCurve { id: *id, name: c.name.clone() };
                        c.name = name.clone();
                        Ok(inv)
                    }
                    None => {
                        self.curves.insert(*id, Curve { name: name.clone(), points: Vec::new() });
                        Ok(Op::RemoveCurve { id: *id })
                    }
                }
            }
            Op::RemoveCurve { id } => {
                if !self.curves.contains_key(id) { return Err(EditError::NoSuchCurve(*id)); }
                if self.bindings.values().any(|b| b.curve == *id) {
                    return Err(EditError::CurveInUse(*id));
                }
                let old = self.curves.remove(id).ok_or(EditError::NoSuchCurve(*id))?;
                let mut restore = vec![Op::SetCurve { id: *id, name: old.name }];
                restore.extend(old.points.into_iter()
                    .map(|(at, value)| Op::SetCurvePoint { curve: *id, at, value }));
                Ok(Op::Group { ops: restore })
            }
            Op::SetCurvePoint { curve, at, value } => {
                let c = self.curves.get_mut(curve).ok_or(EditError::NoSuchCurve(*curve))?;
                match c.points.iter_mut().find(|(t, _)| t == at) {
                    Some(p) => {
                        let inv = Op::SetCurvePoint { curve: *curve, at: *at, value: p.1 };
                        p.1 = *value;
                        Ok(inv)
                    }
                    None => {
                        c.points.push((*at, *value));
                        c.points.sort_by_key(|(t, _)| *t);
                        Ok(Op::RemoveCurvePoint { curve: *curve, at: *at })
                    }
                }
            }
            Op::RemoveCurvePoint { curve, at } => {
                let c = self.curves.get_mut(curve).ok_or(EditError::NoSuchCurve(*curve))?;
                let i = c.points.iter().position(|(t, _)| t == at).ok_or(EditError::NoSuchAutoPoint)?;
                let (_, old) = c.points.remove(i);
                Ok(Op::SetCurvePoint { curve: *curve, at: *at, value: old })
            }
            Op::SetBinding { id, binding } => {
                if !self.curves.contains_key(&binding.curve) {
                    return Err(EditError::NoSuchCurve(binding.curve));
                }
                self.check_auto_target(binding.target)?;
                match binding.gate {
                    Gate::Always => {}
                    Gate::Clip(c) => { if !self.clips.contains_key(&c) { return Err(EditError::NoSuchClip(c)); } }
                    Gate::Window { start, end } => { if end <= start { return Err(EditError::EmptyWindow); } }
                    Gate::Group(g) => { if !self.groups.contains_key(&g) { return Err(EditError::NoSuchGroup(g)); } }
                }
                let inv = match self.bindings.insert(*id, *binding) {
                    Some(old) => Op::SetBinding { id: *id, binding: old },
                    None => Op::RemoveBinding { id: *id },
                };
                Ok(inv)
            }
            Op::RemoveBinding { id } => {
                let old = self.bindings.remove(id).ok_or(EditError::NoSuchBinding(*id))?;
                Ok(Op::SetBinding { id: *id, binding: old })
            }
            Op::AddClip { clip } => {
                self.check_track(clip.track)?;
                self.check_room(clip)?;
                self.clips.insert(clip.id, clip.clone());
                Ok(Op::RemoveClip { id: clip.id })
            }
            Op::RemoveClip { id } => {
                let clip = self.clips.remove(id).ok_or(EditError::NoSuchClip(*id))?;
                Ok(Op::AddClip { clip })
            }
            Op::MoveClip { id, pos, track } => {
                let old = self.clips.get(id).cloned().ok_or(EditError::NoSuchClip(*id))?;
                self.check_track(*track)?;
                let mut moved = old.clone();
                moved.pos = *pos;
                moved.track = *track;
                self.check_room(&moved)?;
                self.clips.insert(*id, moved);
                Ok(Op::MoveClip { id: *id, pos: old.pos, track: old.track })
            }
            Op::TrimClip { id, src_in, len } => {
                let old = self.clips.get(id).cloned().ok_or(EditError::NoSuchClip(*id))?;
                if len.0 <= 0 || src_in.0 < 0 {
                    return Err(EditError::OutOfSource { id: *id });
                }
                if let Some(src) = self.sources.get(&old.source) {
                    if src_in.saturating_add(*len).0 > src.len.0 {
                        return Err(EditError::OutOfSource { id: *id });
                    }
                }
                let mut t = old.clone();
                t.src_in = *src_in;
                t.len = *len;
                self.check_room(&t)?;
                self.clips.insert(*id, t);
                Ok(Op::TrimClip { id: *id, src_in: old.src_in, len: old.len })
            }
            Op::SetGain { id, gain } => {
                let c = self.clips.get_mut(id).ok_or(EditError::NoSuchClip(*id))?;
                let inv = Op::SetGain { id: *id, gain: c.gain };
                c.gain = *gain;
                Ok(inv)
            }
            Op::SetEffects { id, effects } => {
                let c = self.clips.get_mut(id).ok_or(EditError::NoSuchClip(*id))?;
                let inv = Op::SetEffects { id: *id, effects: c.effects.clone() };
                c.effects = effects.clone();
                Ok(inv)
            }
            Op::AddSource { source } => {
                if let Some(old) = self.sources.get(&source.id) {
                    // idempotent by id — a duplicate catch-up delivery
                    // re-affirms the same source, not a fresh insert, so
                    // its inverse is "put back whatever was already there,"
                    // not the new value (that was the bug: the old code
                    // returned `source.clone()` here, which is only a
                    // correct inverse if the two happen to be identical).
                    let old = old.clone();
                    self.sources.insert(source.id, source.clone());
                    return Ok(Op::AddSource { source: old });
                }
                self.sources.insert(source.id, source.clone());
                Ok(Op::RemoveSource { id: source.id })
            }
            Op::SetSourcePath { id, path } => {
                let s = self.sources.get_mut(id).ok_or(EditError::NoSuchSource(*id))?;
                let inv = Op::SetSourcePath { id: *id, path: s.path.clone() };
                s.path = path.clone();
                Ok(inv)
            }
            Op::RemoveSource { id } => {
                if self.clips.values().any(|c| c.source == *id) {
                    return Err(EditError::SourceInUse(*id));
                }
                let source = self.sources.remove(id).ok_or(EditError::NoSuchSource(*id))?;
                Ok(Op::AddSource { source })
            }
            Op::AddBus { bus } => {
                if self.busses.contains_key(&bus.id) {
                    // idempotent by id, same as AddSource — a duplicate
                    // catch-up delivery must not error the joiner.
                    let old = self.busses.insert(bus.id, bus.clone());
                    return Ok(Op::AddBus { bus: old.unwrap() });
                }
                if let Some(p) = bus.parent {
                    if p == bus.id {
                        return Err(EditError::BusCycle { id: bus.id });
                    }
                    self.check_bus(p)?;
                }
                // A brand-new id cannot already be reachable from anywhere
                // else in the graph, so a direct self-loop (just handled
                // above) is the only cycle `AddBus` can introduce on its
                // own; the general check below is a backstop, not load-
                // bearing, but kept for symmetry with the other bus ops.
                if let Some(p) = bus.parent {
                    if self.would_create_cycle(bus.id, p) {
                        return Err(EditError::BusCycle { id: bus.id });
                    }
                }
                self.busses.insert(bus.id, bus.clone());
                Ok(Op::RemoveBus { id: bus.id })
            }
            Op::RemoveBus { id } => {
                let bus = self.busses.get(id).cloned().ok_or(EditError::NoSuchBus(*id))?;
                let in_use = self.tracks.iter().any(|t| t.route == Some(*id))
                    || self.busses.values().any(|b| b.id != *id && (b.parent == Some(*id) || b.sends.iter().any(|s| s.to == *id)));
                if in_use {
                    return Err(EditError::BusInUse(*id));
                }
                self.busses.remove(id);
                self.bus_pans.remove(id);
                Ok(Op::AddBus { bus })
            }
            Op::SetBusGain { id, gain } => {
                let b = self.busses.get_mut(id).ok_or(EditError::NoSuchBus(*id))?;
                let inv = Op::SetBusGain { id: *id, gain: b.gain };
                b.gain = *gain;
                Ok(inv)
            }
            Op::SetBusFlags { id, muted } => {
                let b = self.busses.get_mut(id).ok_or(EditError::NoSuchBus(*id))?;
                let inv = Op::SetBusFlags { id: *id, muted: b.muted };
                b.muted = *muted;
                Ok(inv)
            }
            Op::SetBusSilent { id, silent } => {
                let b = self.busses.get_mut(id).ok_or(EditError::NoSuchBus(*id))?;
                let inv = Op::SetBusSilent { id: *id, silent: b.silent };
                b.silent = *silent;
                Ok(inv)
            }
            Op::SetBusChain { id, chain } => {
                let b = self.busses.get_mut(id).ok_or(EditError::NoSuchBus(*id))?;
                let inv = Op::SetBusChain { id: *id, chain: b.chain.clone() };
                b.chain = chain.clone();
                Ok(inv)
            }
            Op::SetBusSends { id, sends } => {
                self.check_bus(*id)?;
                for s in sends {
                    self.check_bus(s.to)?;
                    // Checked against this bus's OWN sends excluded (they're
                    // what's being replaced) so re-setting an unchanged sends
                    // list is never spuriously refused for "depending on
                    // itself".
                    if self.would_create_cycle_excluding(*id, s.to, *id) {
                        return Err(EditError::BusCycle { id: *id });
                    }
                }
                let b = self.busses.get_mut(id).unwrap();
                let inv = Op::SetBusSends { id: *id, sends: b.sends.clone() };
                b.sends = sends.clone();
                Ok(inv)
            }
            Op::SetRoute { track, bus } => {
                self.check_track(*track)?;
                if let Some(b) = bus {
                    self.check_bus(*b)?;
                }
                let t = self.tracks.iter_mut().find(|t| t.id == *track).unwrap();
                let inv = Op::SetRoute { track: *track, bus: t.route };
                t.route = *bus;
                Ok(inv)
            }
            Op::SetBusParent { id, parent } => {
                let Some(b) = self.busses.get(id) else {
                    return Err(EditError::NoSuchBus(*id));
                };
                if let Some(p) = parent {
                    if !self.busses.contains_key(p) {
                        return Err(EditError::NoSuchBus(*p));
                    }
                    // Walk the proposed parent's chain; reaching `id` means
                    // the graph would feed itself. Capped at the bus count
                    // so even a corrupt project terminates.
                    let mut at = *p;
                    for _ in 0..=self.busses.len() {
                        if at == *id { return Err(EditError::BusCycle { id: *id }); }
                        match self.busses.get(&at).and_then(|b| b.parent) {
                            Some(next) => at = next,
                            None => break,
                        }
                    }
                }
                let inv = Op::SetBusParent { id: *id, parent: b.parent };
                self.busses.get_mut(id).unwrap().parent = *parent;
                Ok(inv)
            }
            Op::SetGroup { id, name, clips } => {
                if clips.is_empty() {
                    return Err(EditError::EmptyGroup);
                }
                // The window joins these for the reason below: a rename must
                // not silently erase a trim.
                let (old_key, old_tempo, old_active, old_window) = self.groups.get(id)
                    .map(|g| (g.key, g.tempo.clone(), g.active, g.window))
                    .unwrap_or((None, None, None, None));
                let inv = match self.groups.get(id) {
                    Some(old) => Op::SetGroup { id: *id, name: old.name.clone(),
                                                clips: old.clips.clone() },
                    None => Op::RemoveGroup { id: *id },
                };
                // Neither key, tempo, nor active travels through `SetGroup`
                // — each has its own op and its own last-write-wins slot,
                // same reasoning as why renaming a track doesn't reset its
                // fader. A rename must not silently erase any of them.
                self.groups.insert(*id, Group { name: name.clone(), clips: clips.clone(),
                                                 key: old_key, tempo: old_tempo, active: old_active,
                                                 window: old_window });
                Ok(inv)
            }
            Op::SetKey { key } => {
                let inv = Op::SetKey { key: self.key };
                self.key = *key;
                Ok(inv)
            }
            Op::SetGroupKey { id, key } => {
                let g = self.groups.get_mut(id).ok_or(EditError::NoSuchGroup(*id))?;
                let inv = Op::SetGroupKey { id: *id, key: g.key };
                g.key = *key;
                Ok(inv)
            }
            Op::SetGroupWindow { id, window } => {
                let g = self.groups.get_mut(id).ok_or(EditError::NoSuchGroup(*id))?;
                let inv = Op::SetGroupWindow { id: *id, window: g.window };
                g.window = *window;
                Ok(inv)
            }
            Op::SetGroupTempo { id, tempo } => {
                let g = self.groups.get_mut(id).ok_or(EditError::NoSuchGroup(*id))?;
                let inv = Op::SetGroupTempo { id: *id, tempo: g.tempo.clone() };
                g.tempo = tempo.clone();
                Ok(inv)
            }
            Op::SetGroupActive { id, active } => {
                let g = self.groups.get_mut(id).ok_or(EditError::NoSuchGroup(*id))?;
                let inv = Op::SetGroupActive { id: *id, active: g.active };
                g.active = *active;
                Ok(inv)
            }
            Op::SetBusPan { id, pan } => {
                if !self.busses.contains_key(id) { return Err(EditError::NoSuchBus(*id)); }
                let prev = self.bus_pans.get(id).copied().unwrap_or(0.0);
                let pan = if pan.is_finite() { pan.clamp(-1.0, 1.0) } else { 0.0 };
                if pan == 0.0 { self.bus_pans.remove(id); } else { self.bus_pans.insert(*id, pan); }
                Ok(Op::SetBusPan { id: *id, pan: prev })
            }
            Op::SetClipFade { id, fade_in, fade_out } => {
                if !self.clips.contains_key(id) {
                    return Err(EditError::NoSuchClip(*id));
                }
                // A fade longer than the clip is refused rather than clamped.
                // Clamping looks helpful and silently changes what the person
                // asked for; refusing lets the caller decide whether it meant
                // "the whole clip" or made a mistake. `gain_at` would cope
                // either way, which is what makes the silence dangerous.
                let len = self.clips[id].len;
                if fade_in.0 < 0 || fade_out.0 < 0 {
                    return Err(EditError::NegativeFade { id: *id });
                }
                if fade_in.0 + fade_out.0 > len.0 {
                    return Err(EditError::FadeLongerThanClip { id: *id, len });
                }
                let prev = self.fades.get(id).copied().unwrap_or((Ticks(0), Ticks(0)));
                let inv = Op::SetClipFade { id: *id, fade_in: prev.0, fade_out: prev.1 };
                if fade_in.0 == 0 && fade_out.0 == 0 {
                    self.fades.remove(id);
                } else {
                    self.fades.insert(*id, (*fade_in, *fade_out));
                }
                Ok(inv)
            }
            Op::SetMarker { at, name } => {
                if name.trim().is_empty() { return Err(EditError::EmptyMarker); }
                let inv = match self.markers.iter().find(|m| m.name == *name) {
                    Some(old) => Op::SetMarker { at: old.at, name: old.name.clone() },
                    None => Op::RemoveMarker { name: name.clone() },
                };
                self.markers.retain(|m| m.name != *name);
                self.markers.push(Marker { at: *at, name: name.clone() });
                self.markers.sort_by_key(|m| m.at);
                Ok(inv)
            }
            Op::RemoveMarker { name } => {
                let Some(i) = self.markers.iter().position(|m| m.name == *name) else {
                    return Err(EditError::NoSuchMarker(name.clone()));
                };
                let old = self.markers.remove(i);
                Ok(Op::SetMarker { at: old.at, name: old.name })
            }
            Op::ClearMarkers => {
                if self.markers.is_empty() { return Err(EditError::NoSuchMarker("any".into())); }
                let all = std::mem::take(&mut self.markers);
                // The inverse of clearing many is a GROUP that puts each
                // one back — one undo, as promised.
                Ok(Op::Group { ops: all.into_iter()
                    .map(|m| Op::SetMarker { at: m.at, name: m.name }).collect() })
            }
            Op::Group { ops } => {
                let mut inverses = Vec::with_capacity(ops.len());
                for op in ops {
                    match self.apply(op) {
                        Ok(inv) => inverses.push(inv),
                        Err(e) => {
                            for inv in inverses.into_iter().rev() {
                                let _ = self.apply(&inv);
                            }
                            return Err(e);
                        }
                    }
                }
                inverses.reverse();
                Ok(Op::Group { ops: inverses })
            }
            Op::RemoveGroup { id } => {
                let old = self.groups.remove(id).ok_or(EditError::NoSuchGroup(*id))?;
                let restore = Op::SetGroup { id: *id, name: old.name, clips: old.clips };
                // Same reasoning as the key: key, tempo, and active each
                // have their own op, and by the time this inverse runs the
                // group is already gone, so `SetGroup`'s own preservation
                // logic has nothing to read from. Bundle whichever
                // actually had a value — a plain group's undo stays the
                // single op it always was.
                let mut restores = vec![restore];
                if let Some(key) = old.key {
                    restores.push(Op::SetGroupKey { id: *id, key: Some(key) });
                }
                if let Some(tempo) = old.tempo {
                    restores.push(Op::SetGroupTempo { id: *id, tempo: Some(tempo) });
                }
                if let Some(active) = old.active {
                    restores.push(Op::SetGroupActive { id: *id, active: Some(active) });
                }
                // The window has the same property the comment above names:
                // `SetGroup`'s preservation logic reads the EXISTING group,
                // and by the time this inverse runs there is none. Omitted,
                // undoing the deletion of a trimmed comp restored it
                // untrimmed — the material would reappear correct and the view
                // would silently widen.
                if let Some(window) = old.window {
                    restores.push(Op::SetGroupWindow { id: *id, window: Some(window) });
                }
                if restores.len() == 1 {
                    Ok(restores.pop().unwrap())
                } else {
                    Ok(Op::Group { ops: restores })
                }
            }
            Op::SetProvenance { source, provenance } => {
                let inv = match self.provenance.get(source) {
                    Some(old) => Op::SetProvenance { source: *source, provenance: old.clone() },
                    None => Op::RemoveProvenance { source: *source },
                };
                self.provenance.insert(*source, provenance.clone());
                Ok(inv)
            }
            Op::RemoveProvenance { source } => {
                let old = self.provenance.remove(source).ok_or(EditError::NoSuchSource(*source))?;
                Ok(Op::SetProvenance { source: *source, provenance: old })
            }
            Op::SetFingerprint { source, fingerprint } => {
                let inv = match self.fingerprints.get(source) {
                    Some(old) => Op::SetFingerprint { source: *source, fingerprint: old.clone() },
                    None => Op::RemoveFingerprint { source: *source },
                };
                self.fingerprints.insert(*source, fingerprint.clone());
                Ok(inv)
            }
            Op::RemoveFingerprint { source } => {
                let old = self.fingerprints.remove(source).ok_or(EditError::NoSuchSource(*source))?;
                Ok(Op::SetFingerprint { source: *source, fingerprint: old })
            }
        }
    }

    fn check_bus(&self, id: BusId) -> Result<(), EditError> {
        if self.busses.contains_key(&id) { Ok(()) } else { Err(EditError::NoSuchBus(id)) }
    }

    /// Would adding an edge `from -> to` (a `parent` pointer or a `send`)
    /// close a loop back to `from`? True exactly when `to` can already reach
    /// `from` by following existing `parent`/`send` edges — including the
    /// trivial case `from == to`, a direct self-loop.
    fn would_create_cycle(&self, from: BusId, to: BusId) -> bool {
        self.reachable(to, from, None)
    }

    /// Same check, but pretend `exclude`'s own `sends` don't exist while
    /// searching — used by `SetBusSends`, which is replacing that bus's
    /// sends and must not be blocked by the very edges it's about to
    /// overwrite. Its `parent` edge (a different field, unaffected by this
    /// op) still counts.
    fn would_create_cycle_excluding(&self, from: BusId, to: BusId, exclude: BusId) -> bool {
        self.reachable(to, from, Some(exclude))
    }

    fn reachable(&self, from: BusId, to: BusId, exclude_sends_of: Option<BusId>) -> bool {
        let mut stack = vec![from];
        let mut seen = std::collections::BTreeSet::new();
        while let Some(cur) = stack.pop() {
            if cur == to {
                return true;
            }
            if !seen.insert(cur) {
                continue;
            }
            if let Some(b) = self.busses.get(&cur) {
                if let Some(p) = b.parent {
                    stack.push(p);
                }
                if Some(cur) != exclude_sends_of {
                    for s in &b.sends {
                        stack.push(s.to);
                    }
                }
            }
        }
        false
    }

    /// A processing order for the bus graph: every bus appears after
    /// everything it depends on (its parent, its sends) has already been
    /// resolved — children before parents, so a mixdown fold can sum a bus
    /// only once every input that feeds it is already computed.
    ///
    /// `apply` refuses cycles as they're introduced, so this should never
    /// see one from a project built through `Op`s — but a loaded or foreign
    /// file did not necessarily come from that path, and a cycle here would
    /// otherwise mean mixdown never terminates. So this is checked
    /// independently rather than trusted, matching the "a damaged file is
    /// refused outright" rule the rest of Studio's persistence follows.
    pub fn bus_order(&self) -> Result<Vec<BusId>, EditError> {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark { Temp, Perm }
        fn visit(
            id: BusId,
            busses: &BTreeMap<BusId, Bus>,
            marks: &mut BTreeMap<BusId, Mark>,
            out: &mut Vec<BusId>,
        ) -> Result<(), EditError> {
            match marks.get(&id) {
                Some(Mark::Perm) => return Ok(()),
                Some(Mark::Temp) => return Err(EditError::BusCycle { id }),
                None => {}
            }
            marks.insert(id, Mark::Temp);
            if let Some(b) = busses.get(&id) {
                let mut targets: Vec<BusId> = Vec::new();
                if let Some(p) = b.parent {
                    targets.push(p);
                }
                for s in &b.sends {
                    targets.push(s.to);
                }
                for t in targets {
                    visit(t, busses, marks, out)?;
                }
            }
            marks.insert(id, Mark::Perm);
            out.push(id);
            Ok(())
        }
        let mut marks = BTreeMap::new();
        let mut out = Vec::new();
        for id in self.busses.keys().copied() {
            visit(id, &self.busses, &mut marks, &mut out)?;
        }
        // `visit` finishes a node only after everything it points to is
        // fully resolved, so the raw push order has parents before the
        // children that feed them — the reverse of what a mixdown fold
        // wants (children summed first, then rolled into the parent).
        out.reverse();
        Ok(out)
    }

    fn check_track(&self, id: TrackId) -> Result<(), EditError> {
        let t = self.tracks.iter().find(|t| t.id == id).ok_or(EditError::NoSuchTrack(id))?;
        if t.locked { return Err(EditError::TrackLocked(id)); }
        Ok(())
    }

    fn check_room(&self, clip: &Clip) -> Result<(), EditError> {
        for other in self.clips.values() {
            if other.id != clip.id && clip.overlaps(other) {
                return Err(EditError::Overlap { id: clip.id, with: other.id });
            }
        }
        Ok(())
    }

    /// Clips on a track, in timeline order. Computed rather than stored, so
    /// there is no ordering invariant for a concurrent edit to violate.
    pub fn track_clips(&self, track: TrackId) -> Vec<&Clip> {
        let mut v: Vec<&Clip> = self.clips.values().filter(|c| c.track == track).collect();
        v.sort_by_key(|c| c.pos);
        v
    }

    /// The project's extent: the end of its last clip.
    pub fn duration(&self) -> Ticks {
        self.clips.values().map(|c| c.end()).max().unwrap_or(Ticks::ZERO)
    }

    /// Split a clip at an absolute timeline position, as both editors' `S` key
    /// does. Returns the ops (not the mutation) so the caller can route them
    /// through undo and collaboration like any other edit.
    /// The span a comp occupies on the timeline: its members' extent, clipped
    /// to its window if it has one.
    ///
    /// `None` when the group is unknown or empty.
    pub fn group_span(&self, id: GroupId) -> Option<(Ticks, Ticks)> {
        let g = self.groups.get(&id)?;
        let mut lo: Option<Ticks> = None;
        let mut hi: Option<Ticks> = None;
        for c in g.clips.iter().filter_map(|c| self.clips.get(c)) {
            lo = Some(lo.map_or(c.pos, |x: Ticks| if c.pos < x { c.pos } else { x }));
            let e = c.end();
            hi = Some(hi.map_or(e, |x: Ticks| if e > x { e } else { x }));
        }
        let (mut a, mut b) = (lo?, hi?);
        if let Some((wa, wb)) = g.window {
            if wa > a { a = wa; }
            if wb < b { b = wb; }
        }
        if b <= a { None } else { Some((a, b)) }
    }

    /// The members of a comp that its window actually shows.
    ///
    /// Without a window this is every member, which is what it always was.
    /// With one it is the members that INTERSECT the window — and that
    /// distinction is load-bearing once a comp has been split, because the two
    /// halves share one member list. A gesture that selected every member of
    /// the left half would be holding the right half's clips too, and nudging
    /// it would drag material that is not on screen.
    ///
    /// Intersection, not containment: a clip straddling the cut is shown by
    /// BOTH halves, and each should be able to act on it. Excluding it would
    /// make a straddling clip unreachable from either side, which is worse
    /// than it being reachable from both.
    pub fn group_members_in_window(&self, id: GroupId) -> Vec<ClipId> {
        let Some(g) = self.groups.get(&id) else { return Vec::new() };
        let Some((a, b)) = g.window else { return g.clips.clone() };
        g.clips.iter().copied().filter(|c| {
            self.clips.get(c).map_or(false, |c| c.pos < b && c.end() > a)
        }).collect()
    }

    /// Split a COMP at `at` — a trim of the representation, not a cut of the
    /// material.
    ///
    /// This is what was wrong. Splitting a folded comp went through
    /// `split_ops` on whichever member clip happened to be selected or under
    /// the playhead, so the cut reached inside and spliced the content out of
    /// the comp. A comp is a VIEW of its clips; halving the view must not
    /// touch them.
    ///
    /// So: two groups over the SAME member list, with complementary windows.
    /// Not one clip op between them. Undo is two `SetGroupWindow`s and a
    /// `RemoveGroup`, and the audio is bit-identical throughout because
    /// nothing about the audio changed.
    ///
    /// The new group takes the RIGHT half, matching `split_ops`, where the new
    /// clip is the tail — so the id the caller just minted means the same
    /// thing in both operations.
    pub fn split_group_ops(&self, id: GroupId, at: Ticks, new_id: GroupId)
        -> Result<Vec<Op>, EditError>
    {
        let g = self.groups.get(&id).ok_or(EditError::NoSuchGroup(id))?;
        let (a, b) = self.group_span(id).ok_or(EditError::EmptyGroup)?;
        // A cut outside the comp is refused rather than producing an empty
        // half. Splitting at the very edge is the same non-event as splitting
        // a clip at its own start.
        if at <= a || at >= b {
            return Err(EditError::OutOfComp { id });
        }
        Ok(vec![
            Op::SetGroupWindow { id, window: Some((a, at)) },
            Op::SetGroup { id: new_id, name: format!("{} (2)", g.name), clips: g.clips.clone() },
            Op::SetGroupWindow { id: new_id, window: Some((at, b)) },
        ])
    }

    pub fn split_ops(&self, id: ClipId, at: Ticks, new_id: ClipId) -> Result<Vec<Op>, EditError> {
        let c = self.clips.get(&id).ok_or(EditError::NoSuchClip(id))?;
        if at <= c.pos || at >= c.end() {
            return Err(EditError::OutOfSource { id });
        }
        let left_len = at.saturating_sub(c.pos);
        let right = Clip {
            id: new_id,
            source: c.source,
            track: c.track,
            pos: at,
            src_in: c.src_in.saturating_add(left_len),
            len: c.len.saturating_sub(left_len),
            gain: c.gain,
            effects: c.effects.clone(),
        };
        Ok(vec![
            Op::TrimClip { id, src_in: c.src_in, len: left_len },
            Op::AddClip { clip: right },
        ])
    }
}

/// Undo/redo over the op log. Both stacks hold ops, so a session's history is
/// serializable and can be replayed or shipped — which the command-object undo
/// in both source editors cannot do.
#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Op>,
    redo: Vec<Op>,
    /// Positions in `undo` where an `apply_group` call began — the marker
    /// `undo_group` unwinds back to. A plain `apply` never pushes one of
    /// these, so single-op undo/redo behave exactly as before; this is
    /// purely additive.
    undo_group_starts: Vec<usize>,
    /// The `redo`-side mirror, recorded by `undo_group` so a full group can
    /// be redone as a unit too.
    redo_group_starts: Vec<usize>,
}

impl History {
    pub fn apply(&mut self, project: &mut Project, op: &Op) -> Result<(), EditError> {
        let inverse = project.apply(op)?;
        self.undo.push(inverse);
        self.redo.clear();
        Ok(())
    }
    pub fn undo(&mut self, project: &mut Project) -> Option<Op> {
        let op = self.undo.pop()?;
        let redo = project.apply(&op).ok()?;
        self.redo.push(redo);
        Some(op)
    }
    pub fn redo(&mut self, project: &mut Project) -> Option<Op> {
        let op = self.redo.pop()?;
        let undo = project.apply(&op).ok()?;
        self.undo.push(undo);
        Some(op)
    }
    pub fn can_undo(&self) -> bool { !self.undo.is_empty() }
    pub fn can_redo(&self) -> bool { !self.redo.is_empty() }

    /// Apply every op in `ops` as one undo group: a single later
    /// `undo_group` call reverts all of them together. This is what makes
    /// `studio-copilot`'s `Proposal.ops` — several ops from one findings-
    /// backed suggestion — undo as the one action the user perceived
    /// applying, rather than needing N separate presses of undo to get back
    /// to where they started.
    ///
    /// If any op fails partway through, everything already applied in this
    /// call is rolled back (via the ordinary single-op `undo` machinery) and
    /// the project is left exactly as it was before the call — a group
    /// either fully lands or leaves no trace, never half of one.
    pub fn apply_group(&mut self, project: &mut Project, ops: &[Op]) -> Result<(), EditError> {
        let start = self.undo.len();
        for op in ops {
            if let Err(e) = self.apply(project, op) {
                while self.undo.len() > start {
                    self.undo(project);
                }
                // A failed group never really succeeded, so there is
                // nothing meaningful left to redo — clear the trail the
                // rollback's own `undo()` calls just built.
                self.redo.clear();
                return Err(e);
            }
        }
        self.undo_group_starts.push(start);
        Ok(())
    }

    pub fn can_undo_group(&self) -> bool { !self.undo_group_starts.is_empty() }
    pub fn can_redo_group(&self) -> bool { !self.redo_group_starts.is_empty() }

    /// Undo back to the start of the most recent group boundary, however
    /// many individual `undo` steps that takes from wherever the stack
    /// currently is. Returns how many ops it actually undid.
    ///
    /// This is deliberately robust to interleaving with plain `undo`/`redo`
    /// rather than assuming a fixed count: if some of the group's entries
    /// were already undone one at a time before this is called, it simply
    /// finishes what's left — the `while` condition naturally stops at the
    /// group's start position regardless of how the stack got there. See
    /// this module's tests for exactly that scenario pinned as a real case,
    /// not a hypothetical.
    pub fn undo_group(&mut self, project: &mut Project) -> usize {
        let Some(start) = self.undo_group_starts.pop() else { return 0 };
        let mut n = 0;
        while self.undo.len() > start {
            if self.undo(project).is_none() { break; }
            n += 1;
        }
        if n > 0 {
            self.redo_group_starts.push(self.redo.len() - n);
        }
        n
    }

    /// The `redo` counterpart: re-apply everything a preceding `undo_group`
    /// call undid, as one action. Returns how many ops it redid.
    pub fn redo_group(&mut self, project: &mut Project) -> usize {
        let Some(start) = self.redo_group_starts.pop() else { return 0 };
        let mut n = 0;
        while self.redo.len() > start {
            if self.redo(project).is_none() { break; }
            n += 1;
        }
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proj() -> Project {
        let mut p = Project { name: "t".into(), ..Default::default() };
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video,
            name: "V1".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "a.wav".into(),
            len: Ticks::from_seconds(30.0), sample_rate: 48_000, channels: 2, has_video: false, kind: SourceKind::File } }).unwrap();
        p
    }
    fn clip(id: u64, pos: f64, len: f64) -> Clip {
        Clip { id: ClipId(id), source: SourceId(1), track: TrackId(1), pos: Ticks::from_seconds(pos),
               src_in: Ticks::ZERO, len: Ticks::from_seconds(len), gain: 1.0, effects: Vec::new() }
    }

    #[test]
    fn the_timebase_is_exact_for_video_and_audio_rates() {
        // The reason for integer ticks: an editor that stores seconds as f64
        // accumulates drift until a cut lands half a sample early. Every common
        // rate must round-trip exactly.
        for rate in [44_100u32, 48_000, 96_000, 8_000] {
            let t = Ticks::from_samples(12_345, rate);
            assert_eq!(t.to_samples(rate), 12_345, "sample round-trip at {rate}Hz");
        }
        for (num, den) in [(24u32, 1u32), (25, 1), (30, 1), (50, 1), (60, 1), (30_000, 1001)] {
            let a = Ticks::from_frames(1000, num, den);
            let b = Ticks::from_frames(1001, num, den);
            assert!(b.0 > a.0, "frames advance at {num}/{den}");
            assert_eq!((b.0 - a.0) * num as i64 / den as i64, TICKS_PER_SECOND,
                       "one frame at {num}/{den} is exact");
        }
    }

    #[test]
    fn every_op_has_an_exact_inverse() {
        // Undo is op inversion, so this IS the undo test — and it is the same
        // mechanism collaboration uses.
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        h.apply(&mut p, &Op::AddClip { clip: clip(1, 0.0, 5.0) }).unwrap();
        h.apply(&mut p, &Op::MoveClip { id: ClipId(1), pos: Ticks::from_seconds(9.0), track: TrackId(1) }).unwrap();
        h.apply(&mut p, &Op::TrimClip { id: ClipId(1), src_in: Ticks::from_seconds(1.0), len: Ticks::from_seconds(2.0) }).unwrap();
        h.apply(&mut p, &Op::SetGain { id: ClipId(1), gain: 0.25 }).unwrap();
        assert!(h.can_undo());
        while h.can_undo() { h.undo(&mut p); }
        assert_eq!(p, start, "undoing everything must restore the opening state exactly");
        while h.can_redo() { h.redo(&mut p); }
        assert_eq!(p.clips[&ClipId(1)].gain, 0.25, "and redo puts it all back");
    }

    #[test]
    fn a_track_cannot_hold_two_clips_in_the_same_place() {
        let mut p = proj();
        p.apply(&Op::AddClip { clip: clip(1, 0.0, 5.0) }).unwrap();
        let err = p.apply(&Op::AddClip { clip: clip(2, 2.0, 5.0) });
        assert!(matches!(err, Err(EditError::Overlap { .. })), "overlap must be refused: {err:?}");
        // butt-jointed clips are fine — that is an edit, not a collision
        p.apply(&Op::AddClip { clip: clip(3, 5.0, 5.0) }).unwrap();
        assert_eq!(p.track_clips(TrackId(1)).len(), 2);
    }

    #[test]
    fn splitting_preserves_the_material_either_side_of_the_cut() {
        let mut p = proj();
        p.apply(&Op::AddClip { clip: clip(1, 0.0, 10.0) }).unwrap();
        let before = p.duration();
        for op in p.split_ops(ClipId(1), Ticks::from_seconds(4.0), ClipId(2)).unwrap() {
            p.apply(&op).unwrap();
        }
        let l = &p.clips[&ClipId(1)];
        let r = &p.clips[&ClipId(2)];
        assert_eq!(l.len, Ticks::from_seconds(4.0));
        assert_eq!(r.pos, Ticks::from_seconds(4.0));
        assert_eq!(r.src_in, Ticks::from_seconds(4.0), "the right half reads on from the cut");
        assert_eq!(l.len.saturating_add(r.len), Ticks::from_seconds(10.0), "no material lost");
        assert_eq!(p.duration(), before, "and the timeline is the same length");
        // a cut outside the clip is refused rather than silently clamped
        assert!(p.split_ops(ClipId(1), Ticks::from_seconds(99.0), ClipId(3)).is_err());
    }

    #[test]
    fn a_trim_cannot_read_past_the_end_of_its_source() {
        let mut p = proj();
        p.apply(&Op::AddClip { clip: clip(1, 0.0, 5.0) }).unwrap();
        let err = p.apply(&Op::TrimClip { id: ClipId(1),
            src_in: Ticks::from_seconds(29.0), len: Ticks::from_seconds(5.0) });
        assert!(matches!(err, Err(EditError::OutOfSource { .. })), "past the source end: {err:?}");
        let err = p.apply(&Op::TrimClip { id: ClipId(1), src_in: Ticks::ZERO, len: Ticks(0) });
        assert!(matches!(err, Err(EditError::OutOfSource { .. })), "zero length: {err:?}");
    }

    #[test]
    fn a_locked_track_refuses_edits() {
        let mut p = proj();
        p.apply(&Op::SetTrackFlags { id: TrackId(1), muted: false, locked: true }).unwrap();
        assert!(matches!(p.apply(&Op::AddClip { clip: clip(1, 0.0, 5.0) }),
                         Err(EditError::TrackLocked(_))));
    }

    #[test]
    fn removing_a_track_with_clips_on_it_is_refused_not_silently_cascaded() {
        // Regression test for a real bug: RemoveTrack used to silently
        // delete every clip on the track as a side effect, but `apply`
        // returns exactly one inverse Op — so undo only brought the TRACK
        // back, not the clips. The fix is the same "in use" refusal
        // Op::RemoveBus already uses: the caller removes the clips first.
        let mut p = proj();
        p.apply(&Op::AddClip { clip: clip(1, 0.0, 5.0) }).unwrap();
        let err = p.apply(&Op::RemoveTrack { id: TrackId(1) });
        assert!(matches!(err, Err(EditError::TrackInUse(TrackId(1)))), "{err:?}");
        assert_eq!(p.tracks.len(), 1, "the refused removal must leave the track in place");
        assert!(p.clips.contains_key(&ClipId(1)), "and the clip must still be there too");
    }

    #[test]
    fn removing_an_empty_track_still_works_and_undoes_cleanly() {
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        h.apply(&mut p, &Op::RemoveTrack { id: TrackId(1) }).unwrap();
        assert!(p.tracks.is_empty());
        h.undo(&mut p);
        assert_eq!(p, start, "removing and undoing an empty track must round-trip exactly, same as before this fix");
    }

    #[test]
    fn removing_a_track_with_clips_via_a_group_first_removes_clips_then_the_track_and_undoes_as_one() {
        // This is the actual right way to do "delete this track and
        // everything on it" now: bundle RemoveClip for every clip plus the
        // final RemoveTrack into one apply_group call, so the whole
        // compound action undoes together as the single thing the user
        // perceived doing — the payoff of the undo-group work pairing with
        // this fix, not two unrelated changes.
        let mut p = proj();
        p.apply(&Op::AddClip { clip: clip(1, 0.0, 5.0) }).unwrap();
        p.apply(&Op::AddClip { clip: clip(2, 6.0, 2.0) }).unwrap();
        let before_group = p.clone();

        let mut h = History::default();
        // proj()'s own AddTrack/AddClip calls above weren't through `h`, so
        // start a fresh group covering just the removal itself.
        h.apply_group(&mut p, &[
            Op::RemoveClip { id: ClipId(1) },
            Op::RemoveClip { id: ClipId(2) },
            Op::RemoveTrack { id: TrackId(1) },
        ]).unwrap();
        assert!(p.tracks.is_empty());
        assert!(p.clips.is_empty());

        let n = h.undo_group(&mut p);
        assert_eq!(n, 3);
        assert_eq!(p, before_group, "one undo_group call should restore both clips and the track together");
    }

    #[test]
    fn removing_a_source_still_referenced_by_a_clip_is_refused() {
        let mut p = proj();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "a.wav".into(),
            len: Ticks::from_seconds(10.0), sample_rate: 48_000, channels: 2, has_video: false, kind: SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1),
            pos: Ticks::ZERO, src_in: Ticks::ZERO, len: Ticks::from_seconds(2.0), gain: 1.0, effects: Vec::new() } }).unwrap();
        let err = p.apply(&Op::RemoveSource { id: SourceId(1) });
        assert!(matches!(err, Err(EditError::SourceInUse(SourceId(1)))), "{err:?}");
        assert!(p.sources.contains_key(&SourceId(1)), "the refused removal must leave the source in place");
    }

    #[test]
    fn removing_an_unreferenced_source_works_and_undoes_cleanly() {
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        h.apply(&mut p, &Op::AddSource { source: Source { id: SourceId(1), path: "a.wav".into(),
            len: Ticks::from_seconds(10.0), sample_rate: 48_000, channels: 2, has_video: false, kind: SourceKind::File } }).unwrap();
        assert_eq!(p.sources.len(), 1);
        h.undo(&mut p);
        assert_eq!(p, start, "adding then undoing a source must fully round-trip — the bug this fixes is that it previously didn't");
    }

    #[test]
    fn recording_a_take_now_fully_undoes_via_apply_group_source_included() {
        // The exact regression this fix targets, expressed at the Op level
        // (studio-live's own test covers the same thing through
        // Recorder::commit_ops directly): AddSource followed by AddClip,
        // applied as one group, must undo BOTH — not leave an orphaned
        // source behind the way the old "AddSource's inverse is itself"
        // workaround did.
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        h.apply_group(&mut p, &[
            Op::AddSource { source: Source { id: SourceId(1), path: "take.wav".into(),
                len: Ticks::from_seconds(4.0), sample_rate: 48_000, channels: 2, has_video: false, kind: SourceKind::File } },
            Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1), track: TrackId(1),
                pos: Ticks::ZERO, src_in: Ticks::ZERO, len: Ticks::from_seconds(4.0), gain: 1.0, effects: Vec::new() } },
        ]).unwrap();
        assert_eq!(p.sources.len(), 1);
        assert_eq!(p.clips.len(), 1);

        let n = h.undo_group(&mut p);
        assert_eq!(n, 2, "both the source and the clip should undo together");
        assert_eq!(p, start, "no orphaned source should remain — this is the bug, made concrete");
    }

    #[test]
    fn a_duplicate_add_source_delivery_restores_the_original_on_undo_not_the_duplicate() {
        // A subtler bug the same fix incidentally corrects: the OLD
        // "idempotent" branch returned the NEW value as its own inverse,
        // which is only a correct undo if the two values happen to be
        // byte-identical. Here they deliberately aren't (different path),
        // so undo must restore the ORIGINAL, not the duplicate.
        let mut p = proj();
        let mut h = History::default();
        h.apply(&mut p, &Op::AddSource { source: Source { id: SourceId(1), path: "original.wav".into(),
            len: Ticks::from_seconds(5.0), sample_rate: 48_000, channels: 2, has_video: false, kind: SourceKind::File } }).unwrap();
        h.apply(&mut p, &Op::AddSource { source: Source { id: SourceId(1), path: "duplicate-delivery.wav".into(),
            len: Ticks::from_seconds(5.0), sample_rate: 48_000, channels: 2, has_video: false, kind: SourceKind::File } }).unwrap();
        assert_eq!(p.sources[&SourceId(1)].path, "duplicate-delivery.wav");

        h.undo(&mut p);
        assert_eq!(p.sources[&SourceId(1)].path, "original.wav", "undo of the duplicate delivery must restore the ORIGINAL value, not just re-apply the duplicate");
    }

    fn bus(id: u64, name: &str, parent: Option<u64>) -> Bus {
        Bus { id: BusId(id), name: name.into(), parent: parent.map(BusId), gain: 1.0, chain: Vec::new(), sends: Vec::new(), muted: false, silent: false }
    }

    #[test]
    fn every_bus_op_has_an_exact_inverse() {
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        h.apply(&mut p, &Op::AddBus { bus: bus(1, "Vocals", None) }).unwrap();
        h.apply(&mut p, &Op::AddBus { bus: bus(2, "Verb", None) }).unwrap();
        h.apply(&mut p, &Op::SetBusGain { id: BusId(1), gain: 0.5 }).unwrap();
        h.apply(&mut p, &Op::SetBusSends { id: BusId(1), sends: vec![Send { to: BusId(2), level: 0.3, pre_fader: false }] }).unwrap();
        h.apply(&mut p, &Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        h.apply(&mut p, &Op::SetBusFlags { id: BusId(1), muted: true }).unwrap();
        assert!(h.can_undo());
        while h.can_undo() { h.undo(&mut p); }
        assert_eq!(p, start, "undoing every bus op must restore the opening project exactly");
    }

    #[test]
    fn a_duplicate_add_bus_delivery_restores_the_original_on_undo_not_the_duplicate() {
        // The same check just applied to AddSource's now-fixed bug, run
        // against AddBus's own idempotent-duplicate branch — which reasoning
        // says should already be correct (BTreeMap::insert returns the
        // PREVIOUS value it displaced, which is what gets used as the
        // inverse here, unlike AddSource's old code which independently
        // re-derived the wrong value). Reasoning that a similar-looking
        // branch is fine is exactly the kind of assumption that let the
        // AddSource bug sit for this long — so this checks it directly
        // rather than trusting the re-derivation on its own.
        let mut p = proj();
        let mut h = History::default();
        h.apply(&mut p, &Op::AddBus { bus: bus(1, "Vocals", None) }).unwrap();
        let mut duplicate = bus(1, "Vocals", None);
        duplicate.gain = 0.42; // deliberately different, so a wrong inverse is detectable
        h.apply(&mut p, &Op::AddBus { bus: duplicate }).unwrap();
        assert_eq!(p.busses[&BusId(1)].gain, 0.42);

        h.undo(&mut p);
        assert_eq!(p.busses[&BusId(1)].gain, 1.0, "undo of the duplicate delivery must restore the ORIGINAL gain, not just re-apply the duplicate");
    }

    #[test]
    fn undo_group_reverts_every_op_in_the_group_together() {
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        h.apply_group(&mut p, &[
            Op::AddBus { bus: bus(1, "Vocals", None) },
            Op::SetBusGain { id: BusId(1), gain: 0.5 },
            Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) },
        ]).unwrap();
        assert_ne!(p, start, "the group should have visibly changed the project");
        assert!(h.can_undo_group());
        let n = h.undo_group(&mut p);
        assert_eq!(n, 3, "should have undone exactly the three ops in the group");
        assert_eq!(p, start, "one undo_group call should fully revert a three-op group");
    }

    #[test]
    fn a_failing_op_partway_through_a_group_rolls_back_atomically() {
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        let err = h.apply_group(&mut p, &[
            Op::AddBus { bus: bus(1, "Vocals", None) },
            Op::SetBusGain { id: BusId(1), gain: 0.5 },
            Op::SetBusGain { id: BusId(99), gain: 0.5 }, // no such bus — must fail
        ]);
        assert!(err.is_err());
        assert_eq!(p, start, "a group that fails partway through must leave the project completely unchanged");
        assert!(!h.can_undo_group(), "a failed group must not register as an undoable group");
        assert!(!h.can_undo(), "a failed group must not leave any of its partial ops on the plain undo stack either");
    }

    #[test]
    fn undo_group_is_lifo_across_multiple_groups() {
        let mut p = proj();
        let mut h = History::default();
        h.apply_group(&mut p, &[Op::AddBus { bus: bus(1, "Vocals", None) }]).unwrap();
        let after_group1 = p.clone();
        h.apply_group(&mut p, &[Op::AddBus { bus: bus(2, "Drums", None) }]).unwrap();
        assert_eq!(p.busses.len(), 2);

        h.undo_group(&mut p);
        assert_eq!(p, after_group1, "the SECOND group should undo first (LIFO), leaving the first group's bus intact");
        assert_eq!(p.busses.len(), 1);

        h.undo_group(&mut p);
        assert_eq!(p.busses.len(), 0, "the first group should now undo too");
    }

    #[test]
    fn undo_group_composes_correctly_with_a_partial_plain_undo_inside_it() {
        // A real scenario, not a hypothetical: something calls plain
        // single-op undo() a couple of times (e.g. the user pressing Ctrl-Z
        // manually) partway through what was originally applied as a group.
        // undo_group must then just finish whatever is left of that group,
        // not double-undo or panic.
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        h.apply_group(&mut p, &[
            Op::AddBus { bus: bus(1, "Vocals", None) },
            Op::AddBus { bus: bus(2, "Drums", None) },
            Op::AddBus { bus: bus(3, "808", None) },
        ]).unwrap();

        h.undo(&mut p); // manually undo just the last op (removes bus 3)
        assert_eq!(p.busses.len(), 2);

        let n = h.undo_group(&mut p); // should finish the remaining two
        assert_eq!(n, 2, "undo_group should report only what IT undid, not the group's original size");
        assert_eq!(p, start, "the group should still end up fully reverted in total");
    }

    #[test]
    fn redo_group_reapplies_a_whole_undone_group() {
        let mut p = proj();
        let mut h = History::default();
        h.apply_group(&mut p, &[
            Op::AddBus { bus: bus(1, "Vocals", None) },
            Op::AddBus { bus: bus(2, "Drums", None) },
        ]).unwrap();
        let after_apply = p.clone();
        h.undo_group(&mut p);
        assert_eq!(p.busses.len(), 0);
        assert!(h.can_redo_group());

        let n = h.redo_group(&mut p);
        assert_eq!(n, 2);
        assert_eq!(p, after_apply, "redo_group should restore the project exactly as the group left it");
    }

    #[test]
    fn plain_undo_still_works_one_step_at_a_time_regardless_of_grouping() {
        // Confirms apply_group is purely additive: ordinary single-op
        // undo/redo behave exactly as before, unaware of and unaffected by
        // group boundaries — a caller that never uses groups sees no
        // change at all.
        let mut p = proj();
        let start = p.clone();
        let mut h = History::default();
        h.apply(&mut p, &Op::AddBus { bus: bus(1, "Vocals", None) }).unwrap();
        h.apply_group(&mut p, &[
            Op::AddBus { bus: bus(2, "Drums", None) },
            Op::AddBus { bus: bus(3, "808", None) },
        ]).unwrap();
        assert_eq!(p.busses.len(), 3);

        h.undo(&mut p);
        assert_eq!(p.busses.len(), 2, "one plain undo should remove exactly one bus, group or not");
        h.undo(&mut p);
        assert_eq!(p.busses.len(), 1);
        h.undo(&mut p);
        assert_eq!(p, start, "three plain undos should fully revert, same total effect as grouping would have");
    }

    #[test]
    fn relinking_a_source_moves_the_path_and_nothing_else() {
        let mut p = Project::default();
        p.apply(&Op::AddSource { source: Source::file(SourceId(1), "/proxies/C0057.mkv",
            Ticks::from_seconds(54.0), 48_000, 2, true) }).unwrap();
        let before = p.sources[&SourceId(1)].clone();
        let inv = p.apply(&Op::SetSourcePath { id: SourceId(1),
            path: "/card/A001/C0057.MXF".into() }).expect("relinks");
        let now = &p.sources[&SourceId(1)];
        assert_eq!(now.path, "/card/A001/C0057.MXF");
        assert_eq!((now.len, now.sample_rate, now.channels, now.has_video),
                   (before.len, before.sample_rate, before.channels, before.has_video),
                   "a relink must not move a single cut");
        p.apply(&inv).unwrap();
        assert_eq!(p.sources[&SourceId(1)], before, "undo puts the proxy back");
        assert!(p.apply(&Op::SetSourcePath { id: SourceId(9), path: "/x".into() }).is_err());
    }

    #[test]
    fn silent_is_its_own_flag_undoes_and_loads_from_a_project_that_predates_it() {
        let mut p = Project::default();
        p.apply(&Op::AddBus { bus: bus(1, "Control", None) }).unwrap();
        let inv = p.apply(&Op::SetBusSilent { id: BusId(1), silent: true }).unwrap();
        assert!(p.busses[&BusId(1)].silent && !p.busses[&BusId(1)].muted, "silent is not mute");
        p.apply(&inv).unwrap();
        assert!(!p.busses[&BusId(1)].silent, "undo restores it");
        assert!(p.apply(&Op::SetBusSilent { id: BusId(9), silent: true }).is_err());
        // a bus serialized before silent existed still loads
        let old = r#"{"id":4,"name":"Drums","parent":null,"gain":1.0,"chain":[],"sends":[],"muted":false}"#;
        let b: Bus = serde_json::from_str(old).expect("loads");
        assert!(!b.silent);
    }

    #[test]
    fn default_route_is_master_and_costs_nothing() {
        // A project with zero busses must still be exactly as valid as
        // before busses existed — the whole point of `route: None` meaning
        // "master" rather than requiring an explicit master Bus value.
        let p = proj();
        assert!(p.busses.is_empty());
        assert_eq!(p.tracks[0].route, None);
    }

    #[test]
    fn a_bus_parent_must_exist() {
        let mut p = proj();
        let err = p.apply(&Op::AddBus { bus: bus(1, "Vocals", Some(99)) });
        assert!(matches!(err, Err(EditError::NoSuchBus(BusId(99)))), "{err:?}");
    }

    #[test]
    fn a_bus_cannot_be_its_own_parent() {
        let mut p = proj();
        let err = p.apply(&Op::AddBus { bus: bus(1, "Loopy", Some(1)) });
        assert!(matches!(err, Err(EditError::BusCycle { id: BusId(1) })), "{err:?}");
    }

    #[test]
    fn a_send_that_would_close_a_loop_is_refused() {
        let mut p = proj();
        p.apply(&Op::AddBus { bus: bus(1, "Vocals", None) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(2, "Verb", Some(1)) }).unwrap(); // Verb -> Vocals (parent)
        // Vocals sending into Verb would close Vocals -> Verb -> Vocals.
        let err = p.apply(&Op::SetBusSends { id: BusId(1), sends: vec![Send { to: BusId(2), level: 0.2, pre_fader: false }] });
        assert!(matches!(err, Err(EditError::BusCycle { id: BusId(1) })), "{err:?}");
    }

    #[test]
    fn resetting_a_buss_own_unchanged_sends_is_never_spuriously_refused() {
        let mut p = proj();
        p.apply(&Op::AddBus { bus: bus(1, "Vocals", None) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(2, "Verb", None) }).unwrap();
        let sends = vec![Send { to: BusId(2), level: 0.4, pre_fader: false }];
        p.apply(&Op::SetBusSends { id: BusId(1), sends: sends.clone() }).unwrap();
        // Re-applying the identical sends list must not trip the cycle
        // check against itself.
        p.apply(&Op::SetBusSends { id: BusId(1), sends }).unwrap();
    }

    #[test]
    fn removing_a_bus_something_depends_on_is_refused() {
        let mut p = proj();
        p.apply(&Op::AddBus { bus: bus(1, "Drums", None) }).unwrap();
        p.apply(&Op::SetRoute { track: TrackId(1), bus: Some(BusId(1)) }).unwrap();
        let err = p.apply(&Op::RemoveBus { id: BusId(1) });
        assert!(matches!(err, Err(EditError::BusInUse(BusId(1)))), "{err:?}");

        // Reroute away, then removal succeeds.
        p.apply(&Op::SetRoute { track: TrackId(1), bus: None }).unwrap();
        p.apply(&Op::RemoveBus { id: BusId(1) }).unwrap();
        assert!(p.busses.is_empty());
    }

    #[test]
    fn bus_order_processes_children_before_the_parents_they_feed() {
        let mut p = proj();
        p.apply(&Op::AddBus { bus: bus(1, "Master-ish", None) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(2, "Drums", Some(1)) }).unwrap();
        p.apply(&Op::AddBus { bus: bus(3, "808", Some(1)) }).unwrap();
        let order = p.bus_order().unwrap();
        let pos = |id: u64| order.iter().position(|b| *b == BusId(id)).unwrap();
        assert!(pos(2) < pos(1), "Drums must be processed before the bus it feeds");
        assert!(pos(3) < pos(1), "808 must be processed before the bus it feeds");
    }

    #[test]
    fn bus_order_detects_a_cycle_even_in_data_that_bypassed_apply() {
        // A cycle can only reach live state via a corrupt/foreign file, since
        // `apply` refuses to create one — simulate that by building the
        // graph directly rather than through ops.
        let mut p = proj();
        p.busses.insert(BusId(1), Bus { id: BusId(1), name: "A".into(), parent: Some(BusId(2)), gain: 1.0, chain: vec![], sends: vec![], muted: false, silent: false });
        p.busses.insert(BusId(2), Bus { id: BusId(2), name: "B".into(), parent: Some(BusId(1)), gain: 1.0, chain: vec![], sends: vec![], muted: false, silent: false });
        assert!(matches!(p.bus_order(), Err(EditError::BusCycle { .. })));
    }
}

#[cfg(test)]
mod track_gain_tests {
    use super::*;

    fn track(id: u64) -> Track {
        Track { id: TrackId(id), kind: TrackKind::Audio, name: "A".into(),
                muted: false, locked: false, gain: 1.0, route: None }
    }

    #[test]
    fn the_fader_applies_and_its_inverse_restores() {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: track(1), at: 0 }).unwrap();
        let inv = p.apply(&Op::SetTrackGain { id: TrackId(1), gain: 0.5 }).unwrap();
        assert_eq!(p.tracks[0].gain, 0.5);
        p.apply(&inv).unwrap();
        assert_eq!(p.tracks[0].gain, 1.0, "the inverse op is a real undo");
    }

    #[test]
    fn a_locked_track_keeps_its_fader() {
        // SetTrackFlags stays the only edit a locked track accepts; the
        // fader is a mix edit like any clip move.
        let mut p = Project::default();
        let mut t = track(1);
        t.locked = true;
        p.apply(&Op::AddTrack { track: t, at: 0 }).unwrap();
        assert!(matches!(p.apply(&Op::SetTrackGain { id: TrackId(1), gain: 0.2 }),
                         Err(EditError::TrackLocked(_))));
        assert_eq!(p.tracks[0].gain, 1.0);
    }

    #[test]
    fn a_pre_fader_track_deserializes_at_unity_not_silence() {
        // serde(default) is a compatibility promise: a project saved before
        // faders existed must load sounding exactly as it did, and the
        // zero-default a bare f32 would give is the one wrong answer.
        let old = r#"{"id":3,"kind":"Audio","name":"A1","muted":false,"locked":false,"route":null}"#;
        let t: Track = serde_json::from_str(old).expect("pre-fader track loads");
        assert_eq!(t.gain, 1.0);
    }
}

#[cfg(test)]
mod automation_tests {
    use super::*;

    fn project() -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio,
            name: "A1".into(), muted: false, locked: false, gain: 1.0, route: None }, at: 0 }).unwrap();
        p
    }
    fn tg() -> AutoTarget { AutoTarget::TrackGain(TrackId(1)) }

    #[test]
    fn a_point_lands_sorted_and_its_inverse_removes_it() {
        let mut p = project();
        p.apply(&Op::SetAutoPoint { target: tg(), at: Ticks(200), value: 0.5 }).unwrap();
        let inv = p.apply(&Op::SetAutoPoint { target: tg(), at: Ticks(100), value: 0.8 }).unwrap();
        assert_eq!(p.automation[&tg()], vec![(Ticks(100), 0.8), (Ticks(200), 0.5)],
                   "sorted by time regardless of edit order");
        p.apply(&inv).unwrap();
        assert_eq!(p.automation[&tg()], vec![(Ticks(200), 0.5)], "the inverse is a removal");
    }

    #[test]
    fn restating_a_points_tick_changes_its_value_and_undoes_to_the_old_one() {
        let mut p = project();
        p.apply(&Op::SetAutoPoint { target: tg(), at: Ticks(100), value: 0.8 }).unwrap();
        let inv = p.apply(&Op::SetAutoPoint { target: tg(), at: Ticks(100), value: 0.2 }).unwrap();
        assert_eq!(p.automation[&tg()], vec![(Ticks(100), 0.2)], "same tick is the same point");
        p.apply(&inv).unwrap();
        assert_eq!(p.automation[&tg()], vec![(Ticks(100), 0.8)]);
    }

    #[test]
    fn removing_the_last_point_drops_the_lane_and_undo_restores_it() {
        let mut p = project();
        p.apply(&Op::SetAutoPoint { target: tg(), at: Ticks(100), value: 0.8 }).unwrap();
        let inv = p.apply(&Op::RemoveAutoPoint { target: tg(), at: Ticks(100) }).unwrap();
        assert!(p.automation.is_empty(), "an emptied lane keeps the map sparse");
        p.apply(&inv).unwrap();
        assert_eq!(p.automation[&tg()], vec![(Ticks(100), 0.8)]);
    }

    #[test]
    fn removing_a_point_that_is_not_there_is_refused_out_loud() {
        let mut p = project();
        assert!(matches!(p.apply(&Op::RemoveAutoPoint { target: tg(), at: Ticks(9) }),
                         Err(EditError::NoSuchAutoPoint)),
                "an inverse with nothing to restore must never be minted");
    }

    #[test]
    fn a_locked_track_refuses_its_lane_like_it_refuses_its_fader() {
        let mut p = project();
        p.apply(&Op::SetTrackFlags { id: TrackId(1), muted: false, locked: true }).unwrap();
        assert!(matches!(p.apply(&Op::SetAutoPoint { target: tg(), at: Ticks(0), value: 0.5 }),
                         Err(EditError::TrackLocked(_))));
        assert!(matches!(p.apply(&Op::SetAutoPoint {
            target: AutoTarget::BusGain(BusId(7)), at: Ticks(0), value: 0.5 }),
                         Err(EditError::NoSuchBus(_))),
                "and a lane cannot be aimed at a bus that does not exist");
    }

    #[test]
    fn concurrent_lane_edits_converge_in_either_order() {
        // Two peers, two different points on one lane: absolute (time,
        // value) means both survive and both replicas agree — the same
        // property clips get from absolute positions.
        let a = Op::SetAutoPoint { target: tg(), at: Ticks(100), value: 0.2 };
        let b = Op::SetAutoPoint { target: tg(), at: Ticks(300), value: 0.9 };
        let mut ab = project();
        ab.apply(&a).unwrap(); ab.apply(&b).unwrap();
        let mut ba = project();
        ba.apply(&b).unwrap(); ba.apply(&a).unwrap();
        assert_eq!(ab, ba);
    }

    #[test]
    fn eval_is_linear_inside_held_at_the_ends_and_unity_when_empty() {
        let lane = vec![(Ticks(100), 0.0), (Ticks(300), 1.0)];
        assert_eq!(eval_lane(&lane, Ticks(0)), 0.0, "held flat before the first point");
        assert_eq!(eval_lane(&lane, Ticks(100)), 0.0);
        assert!((eval_lane(&lane, Ticks(200)) - 0.5).abs() < 1e-6, "linear between");
        assert_eq!(eval_lane(&lane, Ticks(300)), 1.0);
        assert_eq!(eval_lane(&lane, Ticks(9999)), 1.0, "held flat after the last");
        assert_eq!(eval_lane(&[], Ticks(0)), 1.0, "no lane is a transparent ride");
        // A negative value never reaches the mix as a phase flip.
        assert_eq!(eval_lane(&[(Ticks(0), -2.0)], Ticks(5)), 0.0);
    }
}

#[cfg(test)]
mod group_and_bus_input_tests {
    use super::*;

    fn project() -> Project {
        let mut p = Project::default();
        for id in [1u64, 2, 3] {
            p.apply(&Op::AddBus { bus: Bus { id: BusId(id), name: format!("B{id}"), silent: false,
                parent: None, gain: 1.0, chain: vec![], sends: vec![], muted: false } }).unwrap();
        }
        p
    }

    #[test]
    fn reparenting_a_bus_is_undoable_and_a_cycle_is_refused_by_name() {
        let mut p = project();
        // Chain: 3 → 2 → 1 → master, built from the INPUT side.
        p.apply(&Op::SetBusParent { id: BusId(2), parent: Some(BusId(1)) }).unwrap();
        let inv = p.apply(&Op::SetBusParent { id: BusId(3), parent: Some(BusId(2)) }).unwrap();
        assert_eq!(p.busses[&BusId(3)].parent, Some(BusId(2)));
        // 1 ← 3 would make the chain feed itself: refused, said which bus.
        assert!(matches!(p.apply(&Op::SetBusParent { id: BusId(1), parent: Some(BusId(3)) }),
                         Err(EditError::BusCycle { id: BusId(1) })));
        assert!(matches!(p.apply(&Op::SetBusParent { id: BusId(1), parent: Some(BusId(1)) }),
                         Err(EditError::BusCycle { .. })), "self-feeding is the shortest cycle");
        p.apply(&inv).unwrap();
        assert_eq!(p.busses[&BusId(3)].parent, None, "undo puts the input back");
    }

    #[test]
    fn a_group_restates_absolutely_and_dissolving_touches_no_clip() {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio,
            name: "A".into(), muted: false, locked: false, gain: 1.0, route: None },
            at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "x".into(),
            len: Ticks::from_seconds(10.0), sample_rate: 48_000, channels: 1,
            has_video: false, kind: SourceKind::File } }).unwrap();
        for id in [1u64, 2] {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(1),
                track: TrackId(1), pos: Ticks::from_seconds(id as f64 * 2.0),
                src_in: Ticks::ZERO, len: Ticks::from_seconds(1.0),
                gain: 1.0, effects: vec![] } }).unwrap();
        }
        assert!(matches!(p.apply(&Op::SetGroup { id: GroupId(1), name: "hook".into(),
                                                  clips: vec![] }),
                         Err(EditError::EmptyGroup)));
        let inv = p.apply(&Op::SetGroup { id: GroupId(1), name: "hook".into(),
                                          clips: vec![ClipId(1), ClipId(2)] }).unwrap();
        assert!(matches!(inv, Op::RemoveGroup { .. }), "undo of a new group dissolves it");
        let clips_before = p.clips.clone();
        let inv2 = p.apply(&Op::RemoveGroup { id: GroupId(1) }).unwrap();
        assert_eq!(p.clips, clips_before, "ungrouping is organizational, never musical");
        p.apply(&inv2).unwrap();
        assert_eq!(p.groups[&GroupId(1)].name, "hook");
    }

    #[test]
    fn a_groups_key_has_its_own_op_and_survives_rename_and_remove_undo() {
        let mut p = project();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio,
            name: "A".into(), muted: false, locked: false, gain: 1.0, route: None },
            at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "x".into(),
            len: Ticks::from_seconds(10.0), sample_rate: 48_000, channels: 1,
            has_video: false, kind: SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1),
            track: TrackId(1), pos: Ticks::ZERO, src_in: Ticks::ZERO,
            len: Ticks::from_seconds(1.0), gain: 1.0, effects: vec![] } }).unwrap();
        p.apply(&Op::SetGroup { id: GroupId(1), name: "Track 1".into(),
                                clips: vec![ClipId(1)] }).unwrap();

        // SetGroupKey refuses a group that does not exist — a group-scoped
        // key needs a group, unlike the project-wide key.
        assert!(matches!(p.apply(&Op::SetGroupKey { id: GroupId(99), key: None }),
                         Err(EditError::NoSuchGroup(GroupId(99)))));

        let d_dorian = Key { root: 2, scale: Scale::Dorian };
        let inv = p.apply(&Op::SetGroupKey { id: GroupId(1), key: Some(d_dorian) }).unwrap();
        assert_eq!(p.groups[&GroupId(1)].key, Some(d_dorian));
        assert_eq!(inv, Op::SetGroupKey { id: GroupId(1), key: None }, "invert = the old value");
        p.apply(&inv).unwrap();
        assert_eq!(p.groups[&GroupId(1)].key, None, "inverse round-trips cleanly");
        p.apply(&Op::SetGroupKey { id: GroupId(1), key: Some(d_dorian) }).unwrap();

        // Renaming (SetGroup) must NOT reset the key — a rename has
        // nothing to do with key, and SetGroup restates name+membership
        // absolutely on every call.
        p.apply(&Op::SetGroup { id: GroupId(1), name: "Verse".into(),
                                clips: vec![ClipId(1)] }).unwrap();
        assert_eq!(p.groups[&GroupId(1)].key, Some(d_dorian),
                   "a rename silently erasing the key would be exactly the bug this guards");

        // RemoveGroup's undo must restore the key too, not just name+membership —
        // by the time the inverse runs the group is already gone, so `SetGroup`
        // alone (which reads the CURRENT entry to preserve a key) has nothing to
        // read from.
        let inv_remove = p.apply(&Op::RemoveGroup { id: GroupId(1) }).unwrap();
        assert!(p.groups.get(&GroupId(1)).is_none());
        p.apply(&inv_remove).unwrap();
        assert_eq!(p.groups[&GroupId(1)].name, "Verse");
        assert_eq!(p.groups[&GroupId(1)].key, Some(d_dorian),
                   "undo-of-remove must not silently drop the song's key");
    }

    #[test]
    fn a_groups_active_take_has_its_own_op_and_survives_rename_and_remove_undo() {
        let mut p = project();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Video,
            name: "A".into(), muted: false, locked: false, gain: 1.0, route: None },
            at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "x.mov".into(),
            len: Ticks::from_seconds(10.0), sample_rate: 48_000, channels: 2,
            has_video: true, kind: SourceKind::File } }).unwrap();
        for id in [1u64, 2] {
            p.apply(&Op::AddClip { clip: Clip { id: ClipId(id), source: SourceId(1),
                track: TrackId(1), pos: Ticks::from_seconds(id as f64 * 2.0),
                src_in: Ticks::ZERO, len: Ticks::from_seconds(1.0),
                gain: 1.0, effects: vec![] } }).unwrap();
        }
        p.apply(&Op::SetGroup { id: GroupId(1), name: "Take Bucket".into(),
                                clips: vec![ClipId(1), ClipId(2)] }).unwrap();

        // Same refusal shape as SetGroupKey/SetGroupTempo -- a take-bucket
        // pointer needs a group to belong to.
        assert!(matches!(p.apply(&Op::SetGroupActive { id: GroupId(99), active: None }),
                         Err(EditError::NoSuchGroup(GroupId(99)))));

        let inv = p.apply(&Op::SetGroupActive { id: GroupId(1), active: Some(ClipId(2)) }).unwrap();
        assert_eq!(p.groups[&GroupId(1)].active, Some(ClipId(2)));
        assert_eq!(inv, Op::SetGroupActive { id: GroupId(1), active: None }, "invert = the old value");
        p.apply(&inv).unwrap();
        assert_eq!(p.groups[&GroupId(1)].active, None, "inverse round-trips cleanly");
        p.apply(&Op::SetGroupActive { id: GroupId(1), active: Some(ClipId(2)) }).unwrap();

        // The whole reason this is its own op, not a `SetGroup` field: a
        // rename/restate must not silently flip which take is chosen.
        p.apply(&Op::SetGroup { id: GroupId(1), name: "Take Bucket (renamed)".into(),
                                clips: vec![ClipId(1), ClipId(2)] }).unwrap();
        assert_eq!(p.groups[&GroupId(1)].active, Some(ClipId(2)),
                   "a rename silently resetting which take is active would be exactly the bug this guards");

        // Same "the group is already gone by the time this inverse runs"
        // reasoning as the key/tempo test.
        let inv_remove = p.apply(&Op::RemoveGroup { id: GroupId(1) }).unwrap();
        assert!(p.groups.get(&GroupId(1)).is_none());
        p.apply(&inv_remove).unwrap();
        assert_eq!(p.groups[&GroupId(1)].active, Some(ClipId(2)),
                   "undo-of-remove must not silently drop which take was chosen");
    }

    /// Data, not semantics: `active` doesn't have to point at a CURRENT
    /// member. A take temporarily pulled from the bucket (or one about to
    /// be added) shouldn't force clearing the pointer first -- same
    /// "dangling is a consumer's problem, not this layer's" contract
    /// `groups` itself already documents for membership.
    #[test]
    fn active_may_point_at_a_clip_no_longer_in_the_group_without_erroring() {
        let mut p = project();
        p.apply(&Op::SetGroup { id: GroupId(1), name: "hook".into(), clips: vec![ClipId(1)] }).unwrap();
        p.apply(&Op::SetGroupActive { id: GroupId(1), active: Some(ClipId(99)) }).unwrap();
        assert_eq!(p.groups[&GroupId(1)].active, Some(ClipId(99)), "no membership validation -- that's a UI/consumer concern, not this layer's");
    }
}


#[cfg(test)]
mod provenance_tests {
    use super::*;

    fn project_with_source() -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "clip.mov".into(),
            len: Ticks(100_000), sample_rate: 48_000, channels: 2, has_video: true, kind: SourceKind::File } }).unwrap();
        p
    }

    fn sample_provenance(byte: u8) -> Provenance {
        Provenance {
            content_hash: [byte; 32],
            file_size: 42,
            last_verified_path: Some("clip.mov".into()),
            verified_at: Some("2026-09-14T00:00:00Z".into()),
            proxy_of: None,
        }
    }

    #[test]
    fn set_then_undo_restores_no_record_at_all() {
        let mut p = project_with_source();
        assert!(!p.provenance.contains_key(&SourceId(1)), "no record before anything is set");
        let inv = p.apply(&Op::SetProvenance { source: SourceId(1), provenance: sample_provenance(7) }).unwrap();
        assert_eq!(p.provenance[&SourceId(1)].content_hash, [7u8; 32]);
        p.apply(&inv).unwrap();
        assert!(!p.provenance.contains_key(&SourceId(1)), "undo of the FIRST set must remove the record entirely, not restore an empty one");
    }

    #[test]
    fn restating_provenance_is_absolute_and_undoes_to_the_previous_value() {
        let mut p = project_with_source();
        p.apply(&Op::SetProvenance { source: SourceId(1), provenance: sample_provenance(1) }).unwrap();
        let inv = p.apply(&Op::SetProvenance { source: SourceId(1), provenance: sample_provenance(2) }).unwrap();
        assert_eq!(p.provenance[&SourceId(1)].content_hash, [2u8; 32]);
        p.apply(&inv).unwrap();
        assert_eq!(p.provenance[&SourceId(1)].content_hash, [1u8; 32], "undo restores the PREVIOUS record, not nothing");
    }

    #[test]
    fn remove_provenance_undoes_by_restoring_the_exact_record() {
        let mut p = project_with_source();
        let mut prov = sample_provenance(3);
        prov.proxy_of = Some(ProxyInfo {
            original_hash: Some([9u8; 32]),
            reacquire_hint: Some("card slot 2".into()),
            note: Some("deleted after import".into()),
        });
        p.apply(&Op::SetProvenance { source: SourceId(1), provenance: prov.clone() }).unwrap();
        let inv = p.apply(&Op::RemoveProvenance { source: SourceId(1) }).unwrap();
        assert!(!p.provenance.contains_key(&SourceId(1)));
        p.apply(&inv).unwrap();
        assert_eq!(p.provenance[&SourceId(1)], prov, "undo of remove must restore the record byte for byte, including the proxy info");
    }

    #[test]
    fn removing_provenance_that_does_not_exist_is_refused_by_name() {
        let mut p = project_with_source();
        assert!(matches!(p.apply(&Op::RemoveProvenance { source: SourceId(1) }), Err(EditError::NoSuchSource(SourceId(1)))));
    }

    /// Provenance is deliberately independent of the source it describes:
    /// removing the SOURCE does not clean up its provenance record (same
    /// "sparse, may dangle, harmless" contract `groups` already uses for
    /// clip membership) -- pinned here so that stays a decision, not an
    /// accident.
    #[test]
    fn removing_the_source_leaves_its_provenance_record_dangling_not_cleaned_up() {
        let mut p = project_with_source();
        p.apply(&Op::SetProvenance { source: SourceId(1), provenance: sample_provenance(4) }).unwrap();
        p.apply(&Op::RemoveSource { id: SourceId(1) }).unwrap();
        assert!(p.provenance.contains_key(&SourceId(1)), "provenance is sparse metadata, not cleaned up by RemoveSource -- this pins that as deliberate");
    }
}

#[cfg(test)]
mod fingerprint_op_tests {
    use super::*;

    fn project_with_source() -> Project {
        let mut p = Project::default();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "clip.mov".into(),
            len: Ticks(100_000), sample_rate: 48_000, channels: 2, has_video: true, kind: SourceKind::File } }).unwrap();
        p
    }

    fn sample_fingerprint(seed: u64) -> FingerprintData {
        FingerprintData { hashes: vec![seed, seed + 1, seed + 2], interval: Ticks::from_seconds(1.0) }
    }

    #[test]
    fn set_then_undo_restores_no_record_at_all() {
        let mut p = project_with_source();
        let inv = p.apply(&Op::SetFingerprint { source: SourceId(1), fingerprint: sample_fingerprint(10) }).unwrap();
        assert_eq!(p.fingerprints[&SourceId(1)].hashes, vec![10, 11, 12]);
        p.apply(&inv).unwrap();
        assert!(!p.fingerprints.contains_key(&SourceId(1)));
    }

    #[test]
    fn restating_is_absolute_and_undoes_to_the_previous_value() {
        let mut p = project_with_source();
        p.apply(&Op::SetFingerprint { source: SourceId(1), fingerprint: sample_fingerprint(1) }).unwrap();
        let inv = p.apply(&Op::SetFingerprint { source: SourceId(1), fingerprint: sample_fingerprint(100) }).unwrap();
        assert_eq!(p.fingerprints[&SourceId(1)].hashes[0], 100);
        p.apply(&inv).unwrap();
        assert_eq!(p.fingerprints[&SourceId(1)].hashes[0], 1);
    }

    #[test]
    fn remove_undoes_by_restoring_the_exact_record() {
        let mut p = project_with_source();
        let fp = sample_fingerprint(5);
        p.apply(&Op::SetFingerprint { source: SourceId(1), fingerprint: fp.clone() }).unwrap();
        let inv = p.apply(&Op::RemoveFingerprint { source: SourceId(1) }).unwrap();
        assert!(!p.fingerprints.contains_key(&SourceId(1)));
        p.apply(&inv).unwrap();
        assert_eq!(p.fingerprints[&SourceId(1)], fp);
    }

    #[test]
    fn removing_a_fingerprint_that_does_not_exist_is_refused_by_name() {
        let mut p = project_with_source();
        assert!(matches!(p.apply(&Op::RemoveFingerprint { source: SourceId(1) }), Err(EditError::NoSuchSource(SourceId(1)))));
    }
}


#[cfg(test)]
mod tempo_map_tests {
    use super::*;

    #[test]
    fn constant_tempo_round_trips_beat_and_tick_exactly() {
        let m = TempoMap::constant(120.0);
        // At 120 BPM, one beat = half a second.
        assert_eq!(m.tick_to_beat(Ticks::ZERO), 0.0);
        assert!((m.tick_to_beat(Ticks::from_seconds(2.0)) - 4.0).abs() < 1e-6);
        assert_eq!(m.beat_to_tick(0.0), Ticks::ZERO);
        assert_eq!(m.beat_to_tick(4.0), Ticks::from_seconds(2.0));
        // The two functions must be exact inverses at a sweep of points,
        // not just at the ones the other assertions happen to check.
        for beat in [0.0, 0.5, 1.0, 3.25, 100.0] {
            let tick = m.beat_to_tick(beat);
            assert!((m.tick_to_beat(tick) - beat).abs() < 1e-6, "beat {beat}");
        }
    }

    #[test]
    fn a_tempo_change_bends_the_grid_without_moving_earlier_beats() {
        // 120 BPM for the first 4 beats (2 seconds), then 90 BPM.
        let m = TempoMap::new(vec![(Ticks::ZERO, 120.0), (Ticks::from_seconds(2.0), 90.0)]);
        assert_eq!(m.bpm_at(Ticks::ZERO), 120.0);
        assert_eq!(m.bpm_at(Ticks::from_seconds(1.0)), 120.0);
        assert_eq!(m.bpm_at(Ticks::from_seconds(2.0)), 90.0, "the tempo changes AT the point");
        assert_eq!(m.bpm_at(Ticks::from_seconds(5.0)), 90.0, "holds after the last point");
        // Beat 4 sits exactly at the tempo change (4 beats at 120bpm = 2s).
        assert_eq!(m.beat_to_tick(4.0), Ticks::from_seconds(2.0));
        // Beat 5 is one MORE beat, but at 90bpm now (2s + 60/90s).
        let beat5 = m.beat_to_tick(5.0);
        assert!((beat5.to_seconds() - (2.0 + 60.0 / 90.0)).abs() < 1e-6);
        // The round trip still holds across the boundary.
        for beat in [0.0, 3.9, 4.0, 4.1, 10.0] {
            let tick = m.beat_to_tick(beat);
            assert!((m.tick_to_beat(tick) - beat).abs() < 1e-6, "beat {beat}");
        }
    }

    #[test]
    fn empty_and_non_positive_input_never_produces_a_broken_map() {
        // No points at all -> a sensible default, not a panic or an
        // unusable map with nothing to look up against.
        let empty = TempoMap::new(vec![]);
        assert_eq!(empty.points().len(), 1);
        assert!(empty.bpm_at(Ticks::ZERO) > 0.0);
        // Zero or negative BPM would divide by zero or run time backwards —
        // floored rather than trusted.
        let m = TempoMap::new(vec![(Ticks::ZERO, 0.0), (Ticks::from_seconds(1.0), -5.0)]);
        assert!(m.points().iter().all(|(_, bpm)| *bpm >= MIN_BPM));
        // Unsorted input is sorted on the way in.
        let m2 = TempoMap::new(vec![(Ticks::from_seconds(2.0), 90.0), (Ticks::ZERO, 120.0)]);
        assert_eq!(m2.points()[0].0, Ticks::ZERO);
    }

    #[test]
    fn set_group_tempo_has_its_own_op_and_survives_rename_and_remove_undo() {
        let mut p = Project::default();
        p.apply(&Op::AddTrack { track: Track { id: TrackId(1), kind: TrackKind::Audio,
            name: "A".into(), muted: false, locked: false, gain: 1.0, route: None },
            at: 0 }).unwrap();
        p.apply(&Op::AddSource { source: Source { id: SourceId(1), path: "x".into(),
            len: Ticks::from_seconds(10.0), sample_rate: 48_000, channels: 1,
            has_video: false, kind: SourceKind::File } }).unwrap();
        p.apply(&Op::AddClip { clip: Clip { id: ClipId(1), source: SourceId(1),
            track: TrackId(1), pos: Ticks::ZERO, src_in: Ticks::ZERO,
            len: Ticks::from_seconds(1.0), gain: 1.0, effects: vec![] } }).unwrap();
        p.apply(&Op::SetGroup { id: GroupId(1), name: "Track 1".into(),
                                clips: vec![ClipId(1)] }).unwrap();

        assert!(matches!(p.apply(&Op::SetGroupTempo { id: GroupId(99), tempo: None }),
                         Err(EditError::NoSuchGroup(GroupId(99)))));

        let fast = TempoMap::constant(174.0);
        let inv = p.apply(&Op::SetGroupTempo { id: GroupId(1), tempo: Some(fast.clone()) })
            .unwrap();
        assert_eq!(p.groups[&GroupId(1)].tempo, Some(fast.clone()));
        assert_eq!(inv, Op::SetGroupTempo { id: GroupId(1), tempo: None });

        // Rename must not reset it.
        p.apply(&Op::SetGroup { id: GroupId(1), name: "Drop".into(),
                                clips: vec![ClipId(1)] }).unwrap();
        assert_eq!(p.groups[&GroupId(1)].tempo, Some(fast.clone()));

        // A key AND a tempo both set, then removed: undo must restore both,
        // bundled as one Group op — the generalised form of the same bug
        // the key-only case already guards.
        let dorian = Key { root: 2, scale: Scale::Dorian };
        p.apply(&Op::SetGroupKey { id: GroupId(1), key: Some(dorian) }).unwrap();
        let inv_remove = p.apply(&Op::RemoveGroup { id: GroupId(1) }).unwrap();
        p.apply(&inv_remove).unwrap();
        assert_eq!(p.groups[&GroupId(1)].key, Some(dorian));
        assert_eq!(p.groups[&GroupId(1)].tempo, Some(fast),
                   "undo-of-remove must not silently drop the song's tempo either");
    }
}

#[cfg(test)]
mod lane_cursor_tests {
    use super::*;

    #[test]
    fn the_cursor_sweep_equals_the_search_at_every_sample_including_backwards() {
        // The optimization's only job is to be identical. A dense lane, a
        // forward sweep at sample resolution, then a jump BACKWARDS (a
        // loop wrap or a seek — the case a naive cursor gets wrong).
        let points: Vec<(Ticks, f32)> = (0..200)
            .map(|i| (Ticks::from_seconds(i as f64 * 0.05),
                      ((i as f32 * 0.37).sin() * 0.5 + 0.5)))
            .collect();
        let mut cursor = 0usize;
        for s in 0..4_000 {
            let at = Ticks::from_samples(s * 12, 48_000);
            let a = eval_lane(&points, at);
            let b = eval_lane_cursor(&points, at, &mut cursor);
            assert!((a - b).abs() < 1e-6, "sample {s}: {a} vs {b}");
        }
        // Wrap to the start: the cursor must reset rather than hold the
        // late segment.
        for s in 0..500 {
            let at = Ticks::from_samples(s * 12, 48_000);
            let a = eval_lane(&points, at);
            let b = eval_lane_cursor(&points, at, &mut cursor);
            assert!((a - b).abs() < 1e-6, "after wrap, sample {s}: {a} vs {b}");
        }
        // Edges and the empty lane behave as the search does.
        let mut c2 = 0;
        assert_eq!(eval_lane_cursor(&[], Ticks::from_seconds(1.0), &mut c2), 1.0);
        let far = Ticks::from_seconds(1000.0);
        assert_eq!(eval_lane_cursor(&points, far, &mut c2), eval_lane(&points, far));
    }
}

#[cfg(test)]
mod key_and_marker_tests {
    use super::*;

    #[test]
    fn note_labels_agree_with_the_synths_own_pitch_reference() {
        // Key 0 is A1 because sub_freq's reference is 55 Hz — a label that
        // disagreed with the sound would be worse than no label.
        assert_eq!(note_label(0), "A1");
        assert_eq!(note_label(3), "C2");
        assert_eq!(note_label(12), "A2");
        assert_eq!(note_label(-1), "G#1");
        assert_eq!(note_label(-9), "C1");
        assert_eq!(note_label(4), "C#2");
        assert_eq!(note_label(51), "C6", "midi 84 — the octave boundary, not a sharp");
        assert_eq!(note_label(52), "C#6");
        assert_eq!(note_label(48), "A5");
    }

    #[test]
    fn a_scale_step_walks_the_degrees_and_an_out_of_key_note_snaps_below() {
        // Roll key 0 is A1; in C major, C is key 3 (A, A#, B, C).
        let c_major = Key { root: 0, scale: Scale::Major };
        let c = 3i8;
        assert_eq!(c_major.scale_step(c, 2), c + 4, "a third above C is E");
        assert_eq!(c_major.scale_step(c, 4), c + 7, "a fifth above C is G");
        assert_eq!(c_major.scale_step(c, 7), c + 12, "seven degrees is the octave");
        assert_eq!(c_major.scale_step(c + 11, 2), c + 14, "a third above B wraps: D");
        assert_eq!(c_major.scale_step(c + 4, -2), c, "a third below E is C");
        assert_eq!(c_major.scale_step(c, -1), c - 1, "a step below C is B");
        assert_eq!(c_major.scale_step(c + 1, 2), c + 4, "C# is not in C major: steps from C, to E");
        let a_minor = Key { root: 9, scale: Scale::Minor };
        assert_eq!(a_minor.scale_step(0, 2), 3, "a third above A in A minor is C");
        assert_eq!(a_minor.scale_step(0, 4), 7, "a fifth above A is E");
        let chromatic = Key { root: 0, scale: Scale::Chromatic };
        assert_eq!(chromatic.scale_step(5, 4), 9, "chromatic: a degree is a semitone");
    }

    #[test]
    fn a_key_knows_its_own_notes_and_no_key_judges_nothing() {
        let c_major = Key { root: 0, scale: Scale::Major };
        // C2 is key 3; D2 is 5; C#2 (key 4) is not in C major.
        assert!(c_major.contains(3));
        assert!(c_major.contains(5));
        assert!(!c_major.contains(4));
        assert_eq!(c_major.name(), "C major");
        // A minor shares C major's notes — the classic check.
        let a_minor = Key { root: 9, scale: Scale::Minor };
        for k in 0..24 { assert_eq!(a_minor.contains(k), c_major.contains(k), "key {k}"); }
        // Chromatic accepts everything, which is how "no opinion" is said
        // when someone HAS chosen it deliberately.
        let chrom = Key { root: 7, scale: Scale::Chromatic };
        assert!((0..12).all(|k| chrom.contains(k)));
    }

    #[test]
    fn markers_move_by_name_and_clearing_them_all_is_one_undo() {
        let mut p = Project::default();
        p.apply(&Op::SetMarker { at: Ticks::from_seconds(4.0), name: "chorus".into() }).unwrap();
        p.apply(&Op::SetMarker { at: Ticks::from_seconds(1.0), name: "intro".into() }).unwrap();
        assert_eq!(p.markers.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(),
                   vec!["intro", "chorus"], "kept in time order for next/previous");
        // Setting an existing name MOVES it rather than duplicating: "no,
        // the chorus is here".
        let inv = p.apply(&Op::SetMarker { at: Ticks::from_seconds(8.0),
                                           name: "chorus".into() }).unwrap();
        assert_eq!(p.markers.len(), 2);
        assert_eq!(p.markers[1].at, Ticks::from_seconds(8.0));
        p.apply(&inv).unwrap();
        assert_eq!(p.markers[1].at, Ticks::from_seconds(4.0), "undo puts it back");
        assert!(matches!(p.apply(&Op::SetMarker { at: Ticks::ZERO, name: "  ".into() }),
                         Err(EditError::EmptyMarker)));
        // Clear all, then ONE undo restores every one.
        let inv = p.apply(&Op::ClearMarkers).unwrap();
        assert!(p.markers.is_empty());
        p.apply(&inv).unwrap();
        assert_eq!(p.markers.len(), 2, "one undo, every marker back");
        assert!(matches!(p.apply(&Op::RemoveMarker { name: "nope".into() }),
                         Err(EditError::NoSuchMarker(_))));
    }

    #[test]
    fn the_key_is_absolute_and_undoable() {
        let mut p = Project::default();
        let inv = p.apply(&Op::SetKey { key: Some(Key { root: 2, scale: Scale::Dorian }) }).unwrap();
        assert_eq!(p.key.unwrap().name(), "D dorian");
        p.apply(&inv).unwrap();
        assert_eq!(p.key, None, "back to having no opinion");
    }
}

#[cfg(test)]
mod source_kind_tests {
    use super::*;

    /// The bug this type exists for: a drum pattern showed on the timeline as
    /// `demo.wav`, because the only thing a source carried was a path to the
    /// file it had been baked into.
    #[test]
    fn a_pattern_source_labels_itself_by_name_not_by_a_rendered_file() {
        let s = Source::pattern(SourceId(1), 0, "Trap A", Ticks(10_000), 48_000, 2);
        assert_eq!(s.display_name(), "Trap A");
        assert!(s.is_generator());
        assert!(s.path.is_empty(), "a generator has nothing on disk");
    }

    #[test]
    fn a_file_source_labels_itself_by_stem() {
        let f = |p: &str| Source::file(SourceId(1), p, Ticks(1), 48_000, 1, false).display_name().to_string();
        assert_eq!(f("kick.wav"), "kick");
        assert_eq!(f("/takes/day2/vox_04.flac"), "vox_04");
        assert_eq!(f(r"C:\media\b-roll.mov"), "b-roll");
        assert_eq!(f("no_extension"), "no_extension");
        // Several dots: only the last is the extension.
        assert_eq!(f("stem.take.3.wav"), "stem.take.3");
    }

    /// A dotfile has no stem before the dot. Returning "" would label the
    /// clip with nothing at all, which is worse than an odd name.
    #[test]
    fn a_dotfile_keeps_its_whole_name() {
        let f = |p: &str| Source::file(SourceId(1), p, Ticks(1), 48_000, 1, false).display_name().to_string();
        assert_eq!(f(".gitignore"), ".gitignore");
        assert_eq!(f("/a/b/.hidden"), ".hidden");
        assert_eq!(f(""), "");
    }

    /// THE COMPATIBILITY TEST. Every project file written before `kind`
    /// existed omits it, and must still load as a file source. If this
    /// breaks, every saved project in the wild fails to open.
    #[test]
    fn a_source_saved_before_kind_existed_still_loads_as_a_file() {
        let old = r#"{
            "id": 7,
            "path": "vox.wav",
            "len": 480000,
            "sample_rate": 48000,
            "channels": 1,
            "has_video": false
        }"#;
        let s: Source = serde_json::from_str(old).expect("pre-kind project must still parse");
        assert_eq!(s.kind, SourceKind::File);
        assert!(!s.is_generator());
        assert_eq!(s.display_name(), "vox");
    }

    #[test]
    fn source_kind_round_trips_through_json() {
        for s in [
            Source::file(SourceId(1), "a/b.wav", Ticks(5), 48_000, 2, false),
            Source::pattern(SourceId(2), 3, "Trap A", Ticks(5), 48_000, 2),
        ] {
            let j = serde_json::to_string(&s).unwrap();
            assert_eq!(serde_json::from_str::<Source>(&j).unwrap(), s, "round trip failed for {j}");
        }
    }

    /// A pattern can be deleted while clips referencing it remain. The clip
    /// must still be able to name itself and to report that it is dangling —
    /// rendering a blank rectangle and leaving the user guessing is the
    /// failure this guards.
    #[test]
    fn a_generator_whose_pattern_was_deleted_still_names_itself_but_reports_dangling() {
        let s = Source::pattern(SourceId(1), 4, "Trap A", Ticks(10), 48_000, 2);
        assert!(s.generator_is_live(5), "pattern 4 of 5 exists");
        assert!(!s.generator_is_live(4), "pattern 4 is gone when only 0..3 remain");
        assert!(!s.generator_is_live(0));
        assert_eq!(s.display_name(), "Trap A", "still labels itself with the pattern gone");
    }

    /// A file source has no generator to dangle.
    #[test]
    fn a_file_source_is_always_live() {
        let s = Source::file(SourceId(1), "x.wav", Ticks(1), 48_000, 1, false);
        assert!(s.generator_is_live(0));
    }

    // ── comp split: a trim of the view, not a cut of the material ──────────

    fn comp_project() -> (Project, GroupId) {
        // Three clips at 0..100, 100..200, 200..300 on one track, grouped.
        let mut p = Project::default();
        let t = TrackId(1);
        p.apply(&Op::AddTrack {
            track: Track { id: t, kind: TrackKind::Audio, name: "t".into(),
                           muted: false, locked: false, gain: 1.0, route: None },
            at: 0,
        }).unwrap();
        let src = SourceId(1);
        p.apply(&Op::AddSource { source: Source::file(src, "/a.wav", Ticks(1000), 48000, 2, false) }).unwrap();
        let ids: Vec<ClipId> = (0..3).map(|i| ClipId(i + 1)).collect();
        for (i, id) in ids.iter().enumerate() {
            // Clip has no Default -- every field is a real decision, so a
            // default clip would be one that could not exist.
            p.apply(&Op::AddClip { clip: Clip {
                id: *id, source: src, track: t,
                pos: Ticks(i as i64 * 100), src_in: Ticks(0), len: Ticks(100),
                gain: 1.0, effects: Vec::new() } }).unwrap();
        }
        let g = GroupId(1);
        p.apply(&Op::SetGroup { id: g, name: "comp 1".into(), clips: ids }).unwrap();
        (p, g)
    }

    /// THE BUG. Splitting a comp used to reach inside and cut a member, so the
    /// content was spliced out of the comp. It must not emit a single clip op.
    #[test]
    fn splitting_a_comp_does_not_touch_the_clips_inside_it() {
        let (mut p, g) = comp_project();
        let before = p.clips.clone();
        let ops = p.split_group_ops(g, Ticks(150), GroupId(2)).unwrap();
        assert!(ops.iter().all(|o| matches!(o,
            Op::SetGroupWindow { .. } | Op::SetGroup { .. })),
            "a comp split is groups only: {ops:?}");
        for op in &ops { p.apply(op).unwrap(); }
        assert_eq!(p.clips, before, "the material is untouched");
    }

    /// Two comps over the same clips, showing complementary halves.
    #[test]
    fn the_two_halves_cover_the_original_span_and_share_their_members() {
        let (mut p, g) = comp_project();
        let ops = p.split_group_ops(g, Ticks(150), GroupId(2)).unwrap();
        for op in &ops { p.apply(op).unwrap(); }
        assert_eq!(p.group_span(g), Some((Ticks(0), Ticks(150))));
        assert_eq!(p.group_span(GroupId(2)), Some((Ticks(150), Ticks(300))));
        assert_eq!(p.groups[&g].clips, p.groups[&GroupId(2)].clips,
            "both halves are views of the SAME clips");
    }

    /// Deleting one half must leave the material whole -- the property that
    /// makes this a view rather than a copy.
    #[test]
    fn removing_one_half_leaves_every_clip_intact() {
        let (mut p, g) = comp_project();
        let before = p.clips.clone();
        for op in p.split_group_ops(g, Ticks(150), GroupId(2)).unwrap() { p.apply(&op).unwrap(); }
        p.apply(&Op::RemoveGroup { id: GroupId(2) }).unwrap();
        assert_eq!(p.clips, before);
    }

    /// A cut at or outside an edge is a non-event, not an empty half.
    #[test]
    fn a_cut_outside_the_comp_is_refused() {
        let (p, g) = comp_project();
        for at in [Ticks(-10), Ticks(0), Ticks(300), Ticks(999)] {
            assert_eq!(p.split_group_ops(g, at, GroupId(2)),
                       Err(EditError::OutOfComp { id: g }), "at {at:?}");
        }
    }

    /// Splitting twice narrows further rather than re-widening -- the second
    /// cut works on the already-trimmed view.
    #[test]
    fn a_second_split_trims_the_already_trimmed_half() {
        let (mut p, g) = comp_project();
        for op in p.split_group_ops(g, Ticks(200), GroupId(2)).unwrap() { p.apply(&op).unwrap(); }
        for op in p.split_group_ops(g, Ticks(100), GroupId(3)).unwrap() { p.apply(&op).unwrap(); }
        assert_eq!(p.group_span(g), Some((Ticks(0), Ticks(100))));
        assert_eq!(p.group_span(GroupId(3)), Some((Ticks(100), Ticks(200))));
        assert_eq!(p.group_span(GroupId(2)), Some((Ticks(200), Ticks(300))));
    }

    /// A rename must not silently erase a trim, the same rule key/tempo follow.
    #[test]
    fn renaming_a_comp_keeps_its_window() {
        let (mut p, g) = comp_project();
        for op in p.split_group_ops(g, Ticks(150), GroupId(2)).unwrap() { p.apply(&op).unwrap(); }
        let clips = p.groups[&g].clips.clone();
        p.apply(&Op::SetGroup { id: g, name: "renamed".into(), clips }).unwrap();
        assert_eq!(p.group_span(g), Some((Ticks(0), Ticks(150))), "the trim survived the rename");
    }

    /// Undo restores the whole comp, and the window op is its own inverse.
    #[test]
    fn undoing_a_comp_split_restores_the_full_span() {
        let (mut p, g) = comp_project();
        let inv = p.apply(&Op::SetGroupWindow { id: g, window: Some((Ticks(0), Ticks(150)))}).unwrap();
        assert_eq!(p.group_span(g), Some((Ticks(0), Ticks(150))));
        p.apply(&inv).unwrap();
        assert_eq!(p.group_span(g), Some((Ticks(0), Ticks(300))), "back to all of it");
    }

    /// An untrimmed comp behaves exactly as before the window existed.
    #[test]
    fn a_comp_with_no_window_spans_its_members() {
        let (p, g) = comp_project();
        assert!(p.groups[&g].window.is_none(), "None is the default");
        assert_eq!(p.group_span(g), Some((Ticks(0), Ticks(300))));
    }

    /// THE CONSEQUENCE OF SHARING A MEMBER LIST. After a split both halves
    /// list every clip, so a gesture on one half must not reach the other's
    /// material.
    #[test]
    fn each_half_of_a_split_comp_only_shows_its_own_clips() {
        let (mut p, g) = comp_project();
        for op in p.split_group_ops(g, Ticks(150), GroupId(2)).unwrap() { p.apply(&op).unwrap(); }
        // Members are 0..100, 100..200, 200..300; the cut is at 150.
        let left = p.group_members_in_window(g);
        let right = p.group_members_in_window(GroupId(2));
        assert_eq!(left, vec![ClipId(1), ClipId(2)], "left half: {left:?}");
        assert_eq!(right, vec![ClipId(2), ClipId(3)], "right half: {right:?}");
        assert!(p.groups[&g].clips.len() == 3, "though both still LIST all three");
    }

    /// Intersection, not containment: the straddling clip belongs to both
    /// halves. Unreachable from either side would be worse than reachable
    /// from both.
    #[test]
    fn a_clip_straddling_the_cut_is_shown_by_both_halves() {
        let (mut p, g) = comp_project();
        for op in p.split_group_ops(g, Ticks(150), GroupId(2)).unwrap() { p.apply(&op).unwrap(); }
        assert!(p.group_members_in_window(g).contains(&ClipId(2)));
        assert!(p.group_members_in_window(GroupId(2)).contains(&ClipId(2)));
    }

    /// An untrimmed comp shows everything, exactly as before the window
    /// existed.
    #[test]
    fn a_comp_with_no_window_shows_every_member() {
        let (p, g) = comp_project();
        assert_eq!(p.group_members_in_window(g), p.groups[&g].clips);
    }

    /// A window that touches only an edge excludes the clip that merely abuts
    /// it -- a zero-length overlap is not a showing.
    #[test]
    fn a_clip_that_only_abuts_the_window_is_not_shown() {
        let (mut p, g) = comp_project();
        p.apply(&Op::SetGroupWindow { id: g, window: Some((Ticks(100), Ticks(200))) }).unwrap();
        let shown = p.group_members_in_window(g);
        assert_eq!(shown, vec![ClipId(2)], "the neighbours touch but do not overlap: {shown:?}");
    }

    #[test]
    fn an_unknown_group_shows_nothing_rather_than_panicking() {
        let (p, _) = comp_project();
        assert!(p.group_members_in_window(GroupId(99)).is_empty());
    }

    /// Undo of a deleted comp must restore its TRIM too, not just its members.
    /// key/tempo/active are bundled into RemoveGroup's inverse for a stated
    /// reason; the window has the same property and was missed.
    #[test]
    fn undoing_a_deleted_comp_restores_its_window() {
        let (mut p, g) = comp_project();
        for op in p.split_group_ops(g, Ticks(150), GroupId(2)).unwrap() { p.apply(&op).unwrap(); }
        assert_eq!(p.group_span(g), Some((Ticks(0), Ticks(150))));
        let inv = p.apply(&Op::RemoveGroup { id: g }).unwrap();
        assert!(!p.groups.contains_key(&g));
        p.apply(&inv).unwrap();
        assert_eq!(p.group_span(g), Some((Ticks(0), Ticks(150))),
            "the trim came back with it");
    }

    /// An untrimmed comp's undo stays the single op it always was.
    #[test]
    fn undoing_a_plain_comp_is_still_one_op() {
        let (mut p, g) = comp_project();
        let inv = p.apply(&Op::RemoveGroup { id: g }).unwrap();
        assert!(matches!(inv, Op::SetGroup { .. }), "not wrapped in a Group: {inv:?}");
    }

#[cfg(test)]
mod fade_tests {
    use super::*;

    fn one_clip() -> (Project, ClipId) {
        let mut p = Project::default();
        p.apply(&Op::AddTrack {
            track: Track { id: TrackId(1), kind: TrackKind::Audio, name: "A".into(),
                           muted: false, locked: false, gain: 1.0, route: None },
            at: 0,
        }).unwrap();
        p.apply(&Op::AddSource {
            source: Source::file(SourceId(1), "/a.wav", Ticks(1000), 48000, 2, false),
        }).unwrap();
        let id = ClipId(1);
        p.apply(&Op::AddClip { clip: Clip {
            id, source: SourceId(1), track: TrackId(1),
            pos: Ticks(0), src_in: Ticks(0), len: Ticks(100),
            gain: 1.0, effects: Vec::new(),
        }}).unwrap();
        (p, id)
    }

    /// SPARSE: no fade stores nothing, so "has a fade" is a real question and
    /// the common project carries no entries at all.
    #[test]
    fn a_project_starts_with_no_fades_and_zero_removes_one() {
        let (mut p, id) = one_clip();
        assert!(p.fades.is_empty());
        p.apply(&Op::SetClipFade { id, fade_in: Ticks(10), fade_out: Ticks(5) }).unwrap();
        assert_eq!(p.fades.get(&id), Some(&(Ticks(10), Ticks(5))));
        p.apply(&Op::SetClipFade { id, fade_in: Ticks(0), fade_out: Ticks(0) }).unwrap();
        assert!(p.fades.is_empty(), "a zero fade is removed, not stored");
    }

    /// The op returns its own inverse, so a fade is one undo step -- both ends
    /// together, because dragging a clip's corner is one gesture.
    #[test]
    fn undoing_a_fade_restores_the_previous_one() {
        let (mut p, id) = one_clip();
        p.apply(&Op::SetClipFade { id, fade_in: Ticks(10), fade_out: Ticks(10) }).unwrap();
        let inv = p.apply(&Op::SetClipFade { id, fade_in: Ticks(40), fade_out: Ticks(0) }).unwrap();
        p.apply(&inv).unwrap();
        assert_eq!(p.fades.get(&id), Some(&(Ticks(10), Ticks(10))));
    }

    /// REFUSED, NOT CLAMPED. Clamping looks helpful and silently changes what
    /// was asked for; `gain_at` would cope either way, which is exactly what
    /// makes the silence dangerous.
    #[test]
    fn a_fade_longer_than_the_clip_is_refused() {
        let (mut p, id) = one_clip();
        match p.apply(&Op::SetClipFade { id, fade_in: Ticks(60), fade_out: Ticks(60) }) {
            Err(EditError::FadeLongerThanClip { len, .. }) => assert_eq!(len, Ticks(100)),
            other => panic!("must refuse: {other:?}"),
        }
        assert!(p.fades.is_empty(), "and must not have written anything");
        // Exactly the clip's length is fine -- that is a crossfade through the
        // whole clip, which is a real thing to want.
        p.apply(&Op::SetClipFade { id, fade_in: Ticks(50), fade_out: Ticks(50) }).unwrap();
    }

    #[test]
    fn a_negative_fade_is_refused() {
        let (mut p, id) = one_clip();
        assert!(matches!(
            p.apply(&Op::SetClipFade { id, fade_in: Ticks(-1), fade_out: Ticks(0) }),
            Err(EditError::NegativeFade { .. })));
    }

    #[test]
    fn fading_a_clip_that_does_not_exist_is_refused() {
        let (mut p, _) = one_clip();
        assert!(matches!(
            p.apply(&Op::SetClipFade { id: ClipId(99), fade_in: Ticks(1), fade_out: Ticks(0) }),
            Err(EditError::NoSuchClip(_))));
    }
}

#[cfg(test)]
mod abundance_tests {
    //! A big project in the model itself: many sources, tracks, clips, a big
    //! group, many bindings -- every edit undoable, nothing quadratic.
    use super::*;
    use std::time::{Duration, Instant};

    fn timed<T>(what: &str, limit: Duration, f: impl FnOnce() -> T) -> T {
        let t = Instant::now(); let r = f(); let e = t.elapsed();
        eprintln!("abundance: {what}: {e:?}");
        assert!(e < limit, "{what} took {e:?} (limit {limit:?})");
        r
    }

    #[test]
    fn twenty_thousand_clips_build_undo_and_stay_consistent() {
        let mut p = Project::default();
        let mut undo = Vec::new();
        timed("build 500 sources, 200 tracks, 20,000 clips", Duration::from_secs(30), || {
            for s in 0..500u64 {
                undo.push(p.apply(&Op::AddSource { source: Source::file(SourceId(s + 1), format!("C{s}.mkv"), Ticks::from_seconds(600.0), 48_000, 2, true) }).unwrap());
            }
            for t in 0..200u64 {
                undo.push(p.apply(&Op::AddTrack { track: Track { id: TrackId(t + 1), kind: TrackKind::Audio, name: format!("mic {t}"),
                    muted: false, locked: false, gain: 1.0, route: None }, at: t as usize }).unwrap());
            }
            for c in 0..20_000u64 {
                undo.push(p.apply(&Op::AddClip { clip: Clip { id: ClipId(c + 1), source: SourceId(c % 500 + 1), track: TrackId(c % 200 + 1),
                    pos: Ticks::from_seconds((c / 200) as f64 * 3.0), src_in: Ticks::ZERO, len: Ticks::from_seconds(2.5), gain: 1.0, effects: vec![] } }).unwrap());
            }
        });
        assert_eq!(p.clips.len(), 20_000);
        assert_eq!(p.duration(), Ticks::from_seconds(99.0 * 3.0 + 2.5));
        // a group of 5,000 members and its hull
        let members: Vec<ClipId> = (1..=5000).map(ClipId).collect();
        undo.push(p.apply(&Op::SetGroup { id: GroupId(1), name: "scene".into(), clips: members }).unwrap());
        let hull = timed("hull of a 5,000-clip group", Duration::from_secs(2), || p.group_hull(GroupId(1)));
        assert_eq!(hull.map(|h| h.0), Some(Ticks::ZERO));
        // 200 bindings on one target, read 1,000 times
        undo.push(p.apply(&Op::SetCurve { id: CurveId(1), name: "ride".into() }).unwrap());
        undo.push(p.apply(&Op::SetCurvePoint { curve: CurveId(1), at: Ticks::ZERO, value: 0.5 }).unwrap());
        for b in 0..200u64 {
            undo.push(p.apply(&Op::SetBinding { id: BindingId(b + 1), binding: Binding { curve: CurveId(1),
                target: AutoTarget::TrackGain(TrackId(1)), gate: Gate::Always, response: None, fit_span: false } }).unwrap());
        }
        let v = timed("1,000 reads through 200 bindings", Duration::from_secs(5),
            || (0..1000).map(|i| p.auto_value(AutoTarget::TrackGain(TrackId(1)), Ticks::from_seconds(i as f64 * 0.1))).last());
        assert!(v.unwrap() < 1e-30, "200 bindings of 0.5 multiply (0.5^200): {v:?}");
        // and every single edit undoes, in reverse, back to empty
        timed("undo all of it", Duration::from_secs(30), || {
            for inv in undo.into_iter().rev() { p.apply(&inv).expect("each inverse applies"); }
        });
        assert!(p.clips.is_empty() && p.tracks.is_empty() && p.sources.is_empty() && p.bindings.is_empty() && p.groups.is_empty());
    }
}
}
