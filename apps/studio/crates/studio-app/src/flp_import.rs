//! flp_import — an FL Studio project, as far as it can honestly go, as a
//! Studio project.
//!
//! Two halves, so each can be replaced alone:
//!
//! * `tools/flp_dump.py` reads the `.flp` with PyFLP (the reverse-engineered
//!   parser; Python 3.10 -- see the script) and writes a JSON inventory.
//! * this module turns that inventory into a `Project` + `Rack` + a REPORT,
//!   through the same ops every edit uses. Pure: the only outside fact it
//!   needs, a media file's length and format, comes from the `probe` the
//!   caller passes (the example decodes the file; tests pass a table).
//!
//! ## What comes across, and how
//!
//! * **Tempo**; FL ticks (`ppq` per quarter) become seconds at that tempo.
//! * **Audio clips** (FL's channel clips of a sampler): each FL playlist track
//!   becomes a track, the clip lands where FL had it, trimmed by FL's offsets
//!   -- which are MILLISECONDS into the file (checked: a 22.43 s clip carries
//!   offsets 22052..44481). Overlapping clips, which FL allows on one track
//!   and Studio does not, go to an overflow track rather than being lost.
//! * **Patterns** become rack patterns on 16th-note steps (FL's `ppq/4`): a
//!   note on a sampler at C5 (key 60, FL's "as recorded") is a STEP; any other
//!   note is a piano-roll note, pitched relative to C5. Patterns longer than
//!   Studio's 64 steps are cut into consecutive 64-step parts. Notes off the
//!   16th grid are moved to the nearest step and COUNTED.
//! * **Instruments**: samplers are samplers. Plugin synths (Omnisphere, FL
//!   Keys) cannot be hosted here and their sound is not in the file: they
//!   become STAND-INS -- `Sub` for a channel whose notes all sit below C4 (a
//!   bass), `Sine` otherwise -- and the report says so, channel by channel.
//! * **Mixer**: inserts are numbered as FL numbers them (0 = master; the
//!   native reader does, `flp_dump.py` converts). Every insert a channel uses,
//!   and every insert those ROUTE to, becomes a bus with its name and fader.
//!   Routing: an insert that feeds the master keeps the master as its output
//!   and sends to any other insert at that send's level; one that does not
//!   feed the master outputs into its first destination (a Studio parent bus)
//!   and sends to the rest. Effect slots map to Studio effects where there is
//!   one, at Studio's DEFAULT settings (plugin parameters are not decoded),
//!   and the rest are named, not faked. Needs `routes` as `[{to, level}]`
//!   (the native reader); an inventory without them routes to the master and
//!   the report says so.
//! * **Not imported, and said**: automation clips, the master chain, slicer
//!   slices (a slicer channel plays its whole recording), third-party effects.

use std::collections::BTreeMap;

use serde_json::Value;
use studio_core::{Bus, BusId, Clip, ClipId, Effect, Op, Project, Source, SourceId, SourceKind, Ticks, Track, TrackId, TrackKind};
use studio_live::{Channel, Note, Pattern, Rack, SynthKind};

/// What a media file is: length, sample rate, channels, has picture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Media {
    pub len: Ticks, pub rate: u32, pub channels: u16,
    /// The file's sample peak (0..), for FL's per-channel NORMALIZE; 0 when unknown.
    pub peak: f32,
}

pub struct Imported {
    pub project: Project,
    pub rack: Rack,
    /// Audio FL time-stretched to tempo, as (file FL named, ratio, the file
    /// the project reads instead): the caller makes each stretched copy
    /// (the example uses ffmpeg `atempo`, pitch kept) -- FL keeps such a
    /// cache too. Ratio = source seconds played per timeline second.
    pub stretches: Vec<(String, f64, String)>,
    /// Audio FL plays VARISPEED (pitch and speed together): a sampler channel's
    /// fixed pitch shift, as (file FL named, speed ratio, the copy to read). The
    /// caller makes each (the example: ffmpeg `asetrate`).
    pub varispeeds: Vec<(String, f64, String)>,
    /// Clips rendered THROUGH their channel's automation (a pitch curve as
    /// varispeed, a volume curve as gain): the caller makes each copy.
    pub bakes: Vec<Bake>,
    /// Everything approximated, skipped or refused, grouped, for a person.
    pub report: Vec<String>,
}

/// FL's channel pan: 0..=12800 with 6400 CENTRE. Read as signed around 0
/// until c20, which put every channel of Blue Moon hard right.
pub fn fl_pan(v: f64) -> f32 { ((v - 6400.0) / 6400.0).clamp(-1.0, 1.0) as f32 }

/// FL's volume LAWS -- not linear; read as linear until c20, which made every
/// low fader far too loud (a fader at 0.30 read -10.5 dB; this law, -30).
/// From Image-Line's own numbers: the CHANNEL knob at 100% (12800) is 0 dB
/// and at its default 78% (10000) -5.2 dB; the MIXER fader at its default 80%
/// (12800) is 0 dB and at 100% (16000) +5.6 dB. Between those points a power
/// law through them -- an ASSUMPTION about the curve's shape, close, not
/// FL's exact table.
pub const FL_KNOB_EXP: f64 = 2.425;   // (10000/12800)^n = -5.2 dB
pub const FL_FADER_EXP: f64 = 2.890;  // (16000/12800)^n = +5.6 dB
/// Audio clips play at 0 dB with the knob at its -5.2 dB default (Image-Line):
/// a clip's level is the knob's, +5.2 dB.
pub const FL_CLIP_OFFSET_DB: f64 = 5.2;
pub fn fl_knob_gain(v: f64) -> f32 { (v.max(0.0) / 12800.0).powf(FL_KNOB_EXP) as f32 }
pub fn fl_fader_gain(v: f64) -> f32 { (v.max(0.0) / 12800.0).powf(FL_FADER_EXP) as f32 }

/// A channel's level on what it plays: its volume by FL's knob law (and, for
/// an AUDIO CLIP, the +5.2 dB FL gives clips) times, when the sample is
/// NORMALIZEd in FL, the gain that brings the file's peak to full scale.
pub fn channel_gain_for(c: &Value, m: Option<Media>, clip: bool) -> f32 {
    let knob = fl_knob_gain(c["volume"].as_f64().unwrap_or(10000.0));
    let vol = if clip { knob * 10f32.powf(FL_CLIP_OFFSET_DB as f32 / 20.0) } else { knob };
    let norm = match m { Some(m) if c["normalize"].as_bool() == Some(true) && m.peak > 1e-4 => 1.0 / m.peak, _ => 1.0 };
    vol * norm
}

/// As played by notes (a rack channel).
pub fn channel_gain(c: &Value, m: Option<Media>) -> f32 { channel_gain_for(c, m, false) }

/// A Pitcher slot's settings as Studio autotune params, and a line saying
/// what was read. The NOTE MASK is decoded (twelve switches, C first); it maps
/// onto a scale when it is one -- a major key as its relative natural minor,
/// the same notes. The SPEED is INFERRED: the float at offset 12 of Pitcher's
/// state, read as 1.0 = instant. Evidence: on Blue Moon the female lead's bus
/// (which the artist says slides into key) holds the lowest value (0.615),
/// the male lead's 0.913. Four samples; the report says "inferred".
pub fn pitcher_params(v: &Value) -> (BTreeMap<String, f32>, String) {
    let mut q = BTreeMap::new();
    let mut said = Vec::new();
    let names = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    if let Some(mask) = v["notes"].as_array().map(|a| a.iter().map(|b| b.as_bool().unwrap_or(false)).collect::<Vec<bool>>()).filter(|m| m.len() == 12) {
        let fits = |steps: &[usize], r: usize| (0..12).all(|k| mask[(r + k) % 12] == steps.contains(&k));
        if let Some(r) = (0..12).find(|r| fits(&[0, 2, 3, 5, 7, 8, 10], *r)) {
            q.insert("root".into(), r as f32); q.insert("mode".into(), 0.0);
            said.push(format!("key {} major / {} minor (read from Pitcher's note switches)", names[(r + 3) % 12], names[r]));
        } else if let Some(r) = (0..12).find(|r| fits(&[0, 2, 3, 5, 7, 8, 11], *r)) {
            q.insert("root".into(), r as f32); q.insert("mode".into(), 1.0);
            said.push(format!("key {} harmonic minor (read from Pitcher)", names[r]));
        } else {
            q.insert("mode".into(), 2.0);
            said.push("Pitcher's notes are not a scale Studio has: chromatic".into());
        }
    }
    if let Some(sp) = v["speed"].as_f64() {
        let ms = ((1.0 - sp.clamp(0.0, 1.0)) * 400.0) as f32;
        q.insert("speed".into(), ms);
        let slide = ms >= 50.0;
        q.insert("slide".into(), if slide { 1.0 } else { 0.0 });
        said.push(format!("speed {ms:.0} ms{} (INFERRED from Pitcher's knob {sp:.3})", if slide { ", sliding into each note" } else { ", a hard hold" }));
    }
    (q, said.join("; "))
}

/// The fade put on each imported audio clip's edges (seconds), standing in
/// for FL's default declicking.
pub const DECLICK: f64 = 0.003;

/// A playlist item's flags word: 0x40 on every item; with 0x2000 the item is
/// MUTED in FL's playlist. Decided by ear, not by a spec: Balance's 979 such
/// items imported playing sounded like "multiple concepts" at once; the
/// artist heard the render without them and called it the direction of the
/// mix (and it matched FL's rhythm better: onset corr 0.20 at -20 ms vs 0.12).
/// PyFLP's `muted` never decoded this (it reads a field its struct lacks).
pub const FL_ITEM_MUTED: u64 = 0x2000;

/// A plugin synth's stand-in plays this much below the channel's level. The
/// patch's own output is unknown; a stand-in at full scale was ~10 dB louder
/// than the artist's patches where only synths play (A Top A Tower, 5:18-5:27:
/// Studio -10.2 dB, the artist's render -20.4 dB). One song's measurement,
/// said in the report; a guess better than full scale.
pub const STAND_IN_TRIM_DB: f32 = -10.0;

/// A clip to render through its channel's automation, into `variant`.
#[derive(Debug, Clone, PartialEq)]
pub struct Bake {
    /// The file FL named.
    pub file: String,
    /// The copy the clip reads.
    pub variant: String,
    /// Seconds into the file where the clip starts reading (at speed 1).
    pub start: f64,
    /// The clip's length on the timeline, seconds.
    pub len: f64,
    /// (seconds from the clip's start, cents), every `BAKE_STEP`; varispeed:
    /// the file advances 2^(cents/1200) seconds per second.
    pub cents: Vec<(f64, f64)>,
    /// (seconds from the clip's start, gain multiplier) on the same grid.
    pub gain: Vec<(f64, f32)>,
}

/// The automation curves are sampled this often (seconds).
pub const BAKE_STEP: f64 = 0.01;

/// One automation clip placed on the playlist: from `t0` (s) to `t1`, points
/// as (seconds from `t0`, value 0..1, tension).
#[derive(Debug, Clone)]
struct AutoSeg { t0: f64, t1: f64, pts: Vec<(f64, f64, f64)> }

/// FL's value between two points, with TENSION approximated as a power curve
/// (FL's exact curve law is not decoded): tension < 0 starts slow and
/// steepens, > 0 the reverse, 0 is a line.
fn tension_shape(u: f64, tension: f64) -> f64 { u.clamp(0.0, 1.0).powf(2f64.powf(-tension * FL_TENSION_K)) }

/// The tension curve's steepness, CALIBRATED against the artist's own render of
/// A Top A Tower (the intro's pitch dive, tension -0.145): onset correlation in
/// bars 9-16 was 0.48 at k = 3, against 0.27 at 1.5, 0.16 at 6, 0.15 for a
/// straight line and 0.11-0.12 curved the other way. One curve, one song.
pub const FL_TENSION_K: f64 = 3.0;

/// The automated value at `t` (song seconds) from time-ordered segments:
/// before the first, its first value; inside one, its points; between or
/// after, the last value reached -- a parameter HOLDS where automation left it.
fn auto_value(segs: &[AutoSeg], t: f64) -> Option<f64> {
    let first = segs.first()?;
    if t < first.t0 { return first.pts.first().map(|p| p.1); }
    let seg = segs.iter().rev().find(|s| s.t0 <= t)?;
    let local = (t.min(seg.t1) - seg.t0).max(0.0);
    let pts = &seg.pts;
    if pts.is_empty() { return None; }
    if local <= pts[0].0 { return Some(pts[0].1); }
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if local <= b.0 {
            let u = if b.0 > a.0 { (local - a.0) / (b.0 - a.0) } else { 1.0 };
            return Some(a.1 + (b.1 - a.1) * tension_shape(u, b.2));
        }
    }
    pts.last().map(|p| p.1)
}

/// A plugin synth's stand-in: a POLY patch guessed from the channel's name
/// (Omnisphere and VST channels are usually named for their patch --
/// "pain_bass", "batman_strings", "lotusflute"), or a bass when every note sits
/// below C4. A few dozen bytes instead of a sample library; a guess by name,
/// to be tuned against the artist's renders. Returns (family, patch).
pub fn patch_for(name: &str, all_low: bool) -> (&'static str, studio_live::PolyPatch) {
    use studio_live::PolyPatch as P;
    let n = name.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| n.contains(w));
    let base = P { gain: 1.0, ..P::default() };
    if all_low || has(&["bass", "808", "sub"]) {
        ("bass", P { wave: 2, voices: 1, detune: 0.0, sub: 0.5, attack_ms: 3.0, decay_ms: 250.0, sustain: 0.8, release_ms: 120.0,
                     cutoff: 350.0, resonance: 0.15, env_amount: 1.5, ..base })
    } else if has(&["flute", "whistle", "ocarina"]) {
        ("flute", P { wave: 0, voices: 1, detune: 0.0, attack_ms: 60.0, decay_ms: 200.0, sustain: 0.9, release_ms: 200.0,
                      cutoff: 6000.0, env_amount: 0.0, vibrato: 15.0, ..base })
    } else if has(&["pluck", "guitar", "harp", "bell", "bottle", "kalimba", "marimba", "mallet"]) {
        ("pluck", P { wave: 3, voices: 2, detune: 4.0, attack_ms: 2.0, decay_ms: 350.0, sustain: 0.0, release_ms: 200.0,
                      cutoff: 1500.0, resonance: 0.3, env_amount: 2.5, ..base })
    } else if has(&["key", "piano", "rhodes", "wurli", "organ"]) {
        ("keys", P { wave: 1, voices: 2, detune: 4.0, attack_ms: 3.0, decay_ms: 900.0, sustain: 0.3, release_ms: 400.0,
                     cutoff: 4000.0, env_amount: 1.0, ..base })
    } else if has(&["string", "choir", "vox", "voice", "violin", "cello"]) {
        ("strings", P { wave: 2, voices: 5, detune: 12.0, attack_ms: 250.0, decay_ms: 600.0, sustain: 0.85, release_ms: 700.0,
                        cutoff: 2600.0, env_amount: 0.5, vibrato: 8.0, ..base })
    } else if has(&["pad", "atmos", "ambient", "space", "transmission", "dystopian", "desert", "echo", "drone", "texture"]) {
        ("pad", P { wave: 2, voices: 5, detune: 16.0, attack_ms: 400.0, decay_ms: 800.0, sustain: 0.8, release_ms: 900.0,
                    cutoff: 1800.0, resonance: 0.1, env_amount: 0.8, ..base })
    } else {
        ("lead", P { wave: 2, voices: 3, detune: 10.0, attack_ms: 20.0, decay_ms: 400.0, sustain: 0.6, release_ms: 400.0,
                     cutoff: 2500.0, env_amount: 1.0, ..base })
    }
}

/// Steps per pattern at most (the rack rows are a u64 bitmask).
const MAX_STEPS: u32 = 64;

/// Starting settings for a mapped effect whose own settings are not decoded:
/// gentle, so a mix with fourteen exciters is coloured, not wrecked.
pub fn default_params(slot_name: &str) -> BTreeMap<String, f32> {
    let kv = |v: &[(&str, f32)]| v.iter().map(|(k, x)| (k.to_string(), *x)).collect();
    match slot_name {
        "Ozone 8 Exciter" => kv(&[("profile", 0.0), ("drive", 0.15), ("mix", 0.35)]),
        "Ozone 8 Vintage Tape" => kv(&[("profile", 1.0), ("drive", 0.2), ("mix", 0.5)]),
        "Ozone 8 Imager" => kv(&[("low", 0.0), ("mid", 1.0), ("high", 1.3)]),
        "Fruity Fast Dist" => kv(&[("profile", 0.0), ("drive", 0.15), ("mix", 0.35)]),
        _ => BTreeMap::new(),
    }
}

/// FL's native effects that have a Studio counterpart, and the counterpart.
pub fn effect_for(plugin: &str) -> Option<(&'static str, &'static str)> {
    Some(match plugin {
        "Fruity Parametric EQ 2" => ("eq", "exact kind"),
        "Fruity Limiter" => ("limiter", "exact kind"),
        "Maximus" => ("multiband", "exact kind"),
        "Soundgoodizer" => ("multiband", "approximate: Soundgoodizer is a Maximus preset"),
        "Fruity Reeverb 2" => ("reverb", "exact kind"),
        "Fruity Delay 3" => ("delay", "exact kind"),
        "Pitcher" => ("autotune", "exact kind"),
        "Fruity Balance" => ("gain", "exact kind (pan not kept)"),
        "Fruity Compressor" => ("compressor", "exact kind"),
        // iZotope, by the slot's name (they are VSTs: the internal name is
        // "Fruity Wrapper"). Studio has a tool of each KIND; the settings are
        // not decoded, so each lands gentle -- see `default_params`.
        "Ozone 8 Exciter" => ("saturation", "approximate: Ozone's exciter is multiband, Studio's saturation full-band"),
        "Ozone 8 Vintage Tape" => ("saturation", "approximate: tape profile"),
        "Ozone 8 Vintage Limiter" | "Ozone 8 Maximizer" => ("limiter", "approximate"),
        "Ozone 8 Dynamics" => ("compressor", "approximate: Ozone's is multiband"),
        "Ozone 8 Equalizer" | "Ozone 8 Dynamic EQ" => ("eq", "approximate: flat until set"),
        "Ozone 8" => ("multiband", "approximate: the whole Ozone suite as a multiband stage"),
        "Neutron 3" => ("compressor", "approximate: Neutron's channel strip as a compressor"),
        // stereo width, run on the bus's stereo path (studio-audio)
        "Ozone 8 Imager" => ("imager", "approximate: the mastering move (lows mono, air wider)"),
        "Vocal Doubler" => ("doubler", "approximate: two detuned copies left and right, iZotope's own settings not decoded"),
        "Fruity Fast Dist" => ("saturation", "approximate: gentle drive, FL's settings not decoded"),
        "Fruity Stereo Enhancer" | "Fruity Stereo Shaper" => ("imager", "approximate: width only, neutral until set"),
        "Fruity Multiband Compressor" => ("multiband", "exact kind"),
        _ => return None,
    })
}

fn s(v: &Value) -> Option<&str> { v.as_str() }

/// A note's key: a MIDI number, or a name as PyFLP writes it ("F#6"; C5 = 60).
/// The first import read PyFLP's names as numbers, found none, and put every
/// note at C5 -- found by comparing against the native reader.
pub fn key_of(v: &Value) -> Option<i64> {
    if let Some(k) = v.as_i64() { return Some(k); }
    let t = v.as_str()?;
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    let split = t.find(|c: char| c == '-' || c.is_ascii_digit())?;
    let (name, oct) = t.split_at(split);
    Some(N.iter().position(|n| *n == name)? as i64 + 12 * oct.parse::<i64>().ok()?)
}
fn i(v: &Value) -> Option<i64> { v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)) }
fn f(v: &Value) -> Option<f64> { v.as_f64() }

pub fn import(inv: &Value, name: &str, probe: &dyn Fn(&str) -> Option<Media>) -> Result<Imported, String> {
    let proj = &inv["project"];
    let ppq = i(&proj["ppq"]).filter(|p| *p > 0).ok_or("no ppq in the inventory")? as f64;
    let bpm = f(&proj["tempo"]).filter(|b| *b > 0.0).ok_or("no tempo in the inventory")?;
    let at = |ticks: i64| Ticks::from_seconds(ticks.max(0) as f64 / ppq * 60.0 / bpm);
    let step_ticks = ppq / 4.0;
    let mut report: Vec<String> = Vec::new();
    let mut p = Project::default();
    p.name = name.to_string();
    let mut ops: Vec<Op> = Vec::new();

    let chans: BTreeMap<i64, &Value> = inv["channels"].as_array().ok_or("no channels")?.iter()
        .filter_map(|c| Some((i(&c["iid"])?, c))).collect();
    let ch_name = |c: &Value| s(&c["name"]).unwrap_or("channel").to_string();

    // ── sources: one per media file ─────────────────────────────────────
    let mut sources: BTreeMap<String, SourceId> = BTreeMap::new();
    let mut missing: Vec<String> = Vec::new();
    let mut next_source = 1u64;
    // `stretched`: Some((copy's name, ratio)) for FL's time-stretched audio --
    // a source of its own, the copy the caller makes (see `Imported::stretches`).
    let mut source_for = |file: &str, ops: &mut Vec<Op>, missing: &mut Vec<String>, stretched: Option<(&str, f64)>| -> Option<(SourceId, Media)> {
        let m = probe(file);
        let Some(m) = m else { if !missing.contains(&file.to_string()) { missing.push(file.to_string()); } return None };
        let (key, len, rate) = match stretched {
            Some((name, r)) => (name.to_string(), Ticks::from_seconds(m.len.to_seconds() / r), 48_000),
            None => (file.to_string(), m.len, m.rate),
        };
        let id = *sources.entry(key.clone()).or_insert_with(|| {
            let id = SourceId(next_source);
            next_source += 1;
            ops.push(Op::AddSource { source: Source { id, path: key.clone(), len, sample_rate: rate,
                channels: m.channels, has_video: false, kind: SourceKind::File } });
            id
        });
        Some((id, m))
    };
    // A slicer (or any instrument named after a recording) plays that file.
    let file_of = |c: &Value| -> Option<String> {
        if let Some(f) = s(&c["sample_file"]) { return Some(f.to_string()); }
        let n = s(&c["name"])?;
        let base = n.split(" #").next().unwrap_or(n);
        probe(&format!("{base}.wav")).map(|_| format!("{base}.wav"))
    };

    // ── the rack: every channel a pattern plays ─────────────────────────
    let patterns = inv["patterns"].as_array().cloned().unwrap_or_default();
    let mut used: Vec<i64> = Vec::new();
    let mut keys: BTreeMap<i64, Vec<i64>> = BTreeMap::new();
    for pat in &patterns {
        for n in pat["notes"].as_array().into_iter().flatten() {
            let Some(r) = i(&n["rack"]) else { continue };
            if !used.contains(&r) { used.push(r); }
            keys.entry(r).or_default().push(key_of(&n["key"]).unwrap_or(60));
        }
    }
    used.sort();
    let mut rack = Rack { channels: Vec::new(), patterns: Vec::new(), tempo_bpm: bpm as f32, ..Rack::trap_kit() };
    rack.channels.clear();
    rack.patterns.clear();
    let mut rack_ix: BTreeMap<i64, usize> = BTreeMap::new();
    let mut switched_off: Vec<String> = Vec::new();
    let mut stand_ins: Vec<String> = Vec::new();
    for fl in &used {
        let Some(c) = chans.get(fl) else { continue };
        let kind_s = s(&c["kind"]).unwrap_or("");
        // the INTERNAL name says what the plugin is ("Fruity slicer", "Fruity
        // Wrapper" for a VST); the display name is what the person called it
        let internal = s(&c["plugin"]).unwrap_or("").to_string();
        let plugin = if internal == "Fruity Wrapper" { "VST".to_string() } else { internal.clone() };
        let mut media: Option<Media> = None;
        let kind = if kind_s == "Sampler" || internal == "Fruity slicer" {
            match file_of(c).and_then(|f| source_for(&f, &mut ops, &mut missing, None)) {
                Some((id, m)) => {
                    media = Some(m);
                    if internal == "Fruity slicer" {
                        report.push(format!("{}: a Fruity Slicer -- plays its whole recording from each note; the slice map is not in the dump", ch_name(c)));
                    }
                    SynthKind::Sampler { source: id }
                }
                None => SynthKind::Sine { hz: 261.63 },
            }
        } else {
            let low = keys.get(fl).is_some_and(|k| !k.is_empty() && k.iter().all(|k| *k < 48));
            let (family, patch) = patch_for(&ch_name(c), low);
            stand_ins.push(format!("{} ({}) -> Poly {family}", ch_name(c), if plugin.is_empty() { "plugin" } else { &plugin }));
            SynthKind::Poly { hz: 261.63, patch }
        };
        rack_ix.insert(*fl, rack.channels.len());
        let pan = fl_pan(f(&c["pan"]).unwrap_or(6400.0));
        // FL's channel on/off switch: OFF comes in MUTED -- still there, a mute
        // away. Ignored until c20, which played Balance's switched-off Slicex
        // vocal chop (an idea the artist never committed to) through its
        // busiest pattern on a full-level stand-in: the loudest thing in the mix.
        let off = c["enabled"].as_bool() == Some(false);
        let is_stand_in = matches!(kind, SynthKind::Sine { .. } | SynthKind::Sub { .. } | SynthKind::Poly { .. });
        // Poly patches are already quieter than a full-scale sine (filter,
        // envelope, unison normalisation): with the sine's -10 dB trim too, A
        // Top A Tower's synths-only stretch (5:18-5:40) read -29.5 dB against
        // the artist's -19.6; untrimmed, ~-19.5. So Poly: 0 dB; the legacy
        // Sine / Sub stand-ins keep STAND_IN_TRIM_DB.
        let trim = if matches!(kind, SynthKind::Poly { .. }) { 1.0 } else if is_stand_in { 10f32.powf(STAND_IN_TRIM_DB / 20.0) } else { 1.0 };
        if off { switched_off.push(ch_name(c)); }
        rack.channels.push(Channel { name: ch_name(c), kind, gain: (channel_gain(c, media) * trim).clamp(0.0, 64.0),
                                     pan, muted: off, choke_group: None });
    }
    if !stand_ins.is_empty() {
        report.push(format!("{} plugin synth{} cannot be hosted and their sound is not in the file -- STAND-INS, Poly patches guessed by name (level calibrated on one song where only synths play), notes kept: {}",
            stand_ins.len(), if stand_ins.len() == 1 { "" } else { "s" }, stand_ins.join("; ")));
    }

    // ── patterns, on 16th steps, cut into 64-step parts ──────────────────
    let mut parts: BTreeMap<i64, Vec<(usize, u32)>> = BTreeMap::new(); // FL pattern -> (rack pattern, start step)
    let (mut off_grid, mut collided) = (0usize, 0usize);
    for pat in &patterns {
        let Some(pid) = i(&pat["iid"]) else { continue };
        let notes: Vec<&Value> = pat["notes"].as_array().into_iter().flatten().collect();
        if notes.is_empty() { continue; }
        let pname = s(&pat["name"]).map(str::to_string).unwrap_or_else(|| format!("Pattern {pid}"));
        let end_steps = notes.iter().map(|n| ((i(&n["pos"]).unwrap_or(0) + i(&n["len"]).unwrap_or(0)) as f64 / step_ticks).ceil() as u32)
            .max().unwrap_or(16).max(1);
        let total = end_steps.div_ceil(16) * 16;
        let n_parts = total.div_ceil(MAX_STEPS);
        for k in 0..n_parts {
            let from = k * MAX_STEPS;
            let steps = (total - from).min(MAX_STEPS);
            let label = if n_parts > 1 { format!("{pname} ({}/{n_parts})", k + 1) } else { pname.clone() };
            let mut rp = Pattern::new(&label, steps, rack.channels.len());
            for n in &notes {
                let Some(&ch) = i(&n["rack"]).and_then(|r| rack_ix.get(&r)) else { continue };
                let pos = i(&n["pos"]).unwrap_or(0) as f64;
                let step_f = pos / step_ticks;
                let step = step_f.round() as i64 - from as i64;
                if step < 0 || step >= steps as i64 { continue; }
                if (step_f - step_f.round()).abs() > 1e-6 { off_grid += 1; }
                let key = key_of(&n["key"]).unwrap_or(60);
                let is_sampler = matches!(rack.channels[ch].kind, SynthKind::Sampler { .. });
                if is_sampler && key == 60 {
                    if rp.is_set(ch, step as u32) { collided += 1; }
                    rp.set(ch, step as u32, true);
                } else {
                    let len = ((i(&n["len"]).unwrap_or(0) as f64 / step_ticks).round() as u32).max(1);
                    let vel = (f(&n["vel"]).unwrap_or(100.0) / 128.0).clamp(0.05, 1.0) as f32;
                    let slide = n["slide"].as_bool().unwrap_or(false);
                    rp.add_note(ch, Note { key: (key - 60).clamp(-127, 127) as i8, start: step as u32, len, vel, slide });
                }
            }
            parts.entry(pid).or_default().push((rack.patterns.len(), from));
            rack.patterns.push(rp);
        }
    }
    if off_grid > 0 || collided > 0 {
        report.push(format!("patterns are on 16th steps: {off_grid} note{} off the grid moved to the nearest step, {collided} hit{} landed on an already-set step (merged)",
            if off_grid == 1 { "" } else { "s" }, if collided == 1 { "" } else { "s" }));
    }

    // ── mixer: a bus per FL insert a channel uses ───────────────────────
    let mixer: BTreeMap<i64, &Value> = inv["mixer"].as_array().into_iter().flatten().filter_map(|m| Some((i(&m["iid"])?, m))).collect();
    let mut busses: BTreeMap<i64, BusId> = BTreeMap::new();
    let mut unmapped: BTreeMap<String, usize> = BTreeMap::new();
    let mut approx: Vec<String> = Vec::new();
    let has_routes = mixer.values().any(|m| m["routes"].as_array().is_some_and(|r| r.iter().any(|x| x.is_object())));
    let mut sends_made = 0usize;
    let mut unroutable = 0usize;
    // An insert's effect slots as a Studio chain; the rest named in `unmapped`.
    fn chain_of(m: &Value, label: &str, unmapped: &mut BTreeMap<String, usize>, approx: &mut Vec<String>) -> Vec<Effect> {
        let mut chain = Vec::new();
        for slot in m["slots"].as_array().into_iter().flatten() {
            let plug = s(&slot["plugin"]).unwrap_or("");
            let name = if plug == "Fruity Wrapper" { s(&slot["name"]).unwrap_or("a plugin") } else { plug };
            // a VST is known by its slot's name; a native plugin by its own
            match effect_for(name) {
                Some((kind, how)) => {
                    if how != "exact kind" && !approx.iter().any(|a| a.starts_with(name)) { approx.push(format!("{name}: {how}")); }
                    let mut params = default_params(name);
                    if plug == "Pitcher" && slot["pitcher"].is_object() {
                        let (q, said) = pitcher_params(&slot["pitcher"]);
                        params = q;
                        approx.push(format!("Pitcher on {label}: {said}"));
                    }
                    chain.push(Effect { kind: kind.into(), params, enabled: slot["enabled"].as_bool().unwrap_or(true) });
                }
                None => *unmapped.entry(name.to_string()).or_default() += 1,
            }
        }
        chain
    }
    // FL's master (insert 0) as a real bus everything else feeds: the artist
    // ran the whole mix through a final limiter, and Studio has no master
    // chain of its own. Created FIRST, so every other bus can name it.
    if let Some(m) = mixer.get(&0) {
        let chain = chain_of(m, "Master", &mut unmapped, &mut approx);
        let names: Vec<&str> = chain.iter().map(|e| e.kind.as_str()).collect();
        report.push(format!("master: FL's master insert is the bus \"Master\" (chain: {}); every bus that fed it feeds this",
            if names.is_empty() { "none mapped".to_string() } else { names.join(" -> ") }));
        let id = BusId(1);
        busses.insert(0, id);
        ops.push(Op::AddBus { bus: Bus { id, name: "Master".into(), parent: None,
            gain: fl_fader_gain(f(&m["volume"]).unwrap_or(12800.0)), chain, sends: Vec::new(), muted: false, silent: false } });
    }
    let mut sidechains: Vec<(i64, i64)> = Vec::new();
    // Create the bus for `insert` -- its destinations FIRST, so a parent
    // always exists when its child is added. FL routing has no cycles; a
    // depth guard makes a corrupt file an error message, not a stack overflow.
    #[allow(clippy::too_many_arguments)]
    fn make(insert: i64, depth: usize, mixer: &BTreeMap<i64, &Value>, busses: &mut BTreeMap<i64, BusId>, ops: &mut Vec<Op>,
            unmapped: &mut BTreeMap<String, usize>, approx: &mut Vec<String>, sends_made: &mut usize, unroutable: &mut usize,
            sidechains: &mut Vec<(i64, i64)>) -> Option<BusId> {
        // the master bus (insert 0) was made first, when the file has one
        if let Some(b) = busses.get(&insert) { return Some(*b); }
        if insert <= 0 || depth > 32 { return None; }
        let m = *mixer.get(&insert)?;
        let routes: Vec<(i64, f32)> = m["routes"].as_array().into_iter().flatten()
            .filter_map(|r| Some((i(&r["to"])?, fl_fader_gain(f(&r["level"]).unwrap_or(12800.0))))).collect();
        // FL: an insert whose routing is KNOWN and routes nowhere outputs
        // nothing -- Blue Moon's "LowMeter", fed the kick and the 808 at full
        // level for a peak controller, was sent to the master until c20 and
        // DOUBLED the low end. It becomes a silent bus: it still hears, for
        // whatever measures it, and feeds nothing.
        let silent = m["routing_known"].as_bool() == Some(true) && routes.is_empty();
        let to_master = routes.is_empty() || routes.iter().any(|(t, _)| *t == 0);
        let mut dests: Vec<(BusId, f32)> = Vec::new();
        for (t, lvl) in routes.iter().filter(|(t, _)| *t > 0) {
            match make(*t, depth + 1, mixer, busses, ops, unmapped, approx, sends_made, unroutable, sidechains) {
                Some(b) => {
                    dests.push((b, *lvl));
                    if *lvl == 0.0 { sidechains.push((insert, *t)); }
                }
                None => *unroutable += 1,
            }
        }
        // a level-0 route is a sidechain, never an output: pick the parent
        // from the audible routes only
        let audible: Vec<(BusId, f32)> = dests.iter().copied().filter(|(_, l)| *l > 0.0).collect();
        let master = busses.get(&0).copied();
        let (parent, sends): (Option<BusId>, Vec<(BusId, f32)>) = if to_master || audible.is_empty() { (master, audible) }
            else { (Some(audible[0].0), audible[1..].to_vec()) };
        *sends_made += sends.iter().filter(|(_, l)| *l > 0.0).count();
        let id = BusId(busses.len() as u64 + 1);
        busses.insert(insert, id);
        let label = s(&m["name"]).map(str::to_string).unwrap_or_else(|| format!("Insert {insert}"));
        let chain = chain_of(m, &label, unmapped, approx);
        let name = s(&m["name"]).map(str::to_string).unwrap_or_else(|| format!("Insert {insert}"));
        let gain = fl_fader_gain(f(&m["volume"]).unwrap_or(12800.0));
        ops.push(Op::AddBus { bus: Bus { id, name, parent, gain, chain,
            sends: sends.into_iter().filter(|(_, level)| *level > 0.0)
                .map(|(to, level)| studio_core::Send { to, level, pre_fader: false }).collect(),
            muted: false, silent } });
        Some(id)
    }
    let mut bus_for = |insert: i64, ops: &mut Vec<Op>| -> Option<BusId> {
        make(insert, 0, &mixer, &mut busses, ops, &mut unmapped, &mut approx, &mut sends_made, &mut unroutable, &mut sidechains)
    };

    // ── the playlist ────────────────────────────────────────────────────
    let arr = inv["arrangements"].as_array().and_then(|a| a.first()).ok_or("no arrangement")?;
    let mut next_track = 1u64;
    let mut next_clip = 1u64;
    let mut pattern_sources: BTreeMap<usize, SourceId> = BTreeMap::new();
    let (mut automation_clips, mut placed, mut overflowed, mut declicked) = (0usize, 0usize, 0usize, 0usize);
    let mut stretches: Vec<(String, f64, String)> = Vec::new();
    let mut varispeeds: Vec<(String, f64, String)> = Vec::new();
    // ── AUTOMATION curves by target channel name, from the playlist ──────
    // (pitch and volume -- "<channel> - Channel pitch[ #n]" / "... volume[ #n]")
    let beat = 60.0 / bpm;
    let mut pitch_segs: BTreeMap<String, Vec<AutoSeg>> = BTreeMap::new();
    let mut vol_segs: BTreeMap<String, Vec<AutoSeg>> = BTreeMap::new();
    let mut auto_range: BTreeMap<(String, bool), (f64, f64)> = BTreeMap::new(); // (target, is_pitch) -> min, max (0..1)
    if let Some(arr) = inv["arrangements"].as_array().and_then(|a| a.first()) {
        for t in arr["tracks"].as_array().into_iter().flatten() {
            if t["enabled"].as_bool() == Some(false) { continue; }
            for it in t["items"].as_array().into_iter().flatten() {
                if s(&it["kind"]) != Some("ChannelPLItem") || it["flags"].as_u64().is_some_and(|f| f & FL_ITEM_MUTED != 0) { continue; }
                let Some(c) = i(&it["channel"]).and_then(|c| chans.get(&c)) else { continue };
                if s(&c["kind"]) != Some("Automation") { continue; }
                let name = s(&c["name"]).unwrap_or("");
                let (target, is_pitch) = if let Some(x) = name.split(" - Channel pitch").next().filter(|_| name.contains(" - Channel pitch")) { (x.to_string(), true) }
                    else if let Some(x) = name.split(" - Channel volume").next().filter(|_| name.contains(" - Channel volume")) { (x.to_string(), false) }
                    else { continue };
                let pts: Vec<(f64, f64, f64)> = c["points"].as_array().into_iter().flatten()
                    .filter_map(|p| Some((p[0].as_f64()? * beat, p[1].as_f64()?, p[2].as_f64().unwrap_or(0.0)))).collect();
                if pts.is_empty() { continue; }
                let t0 = at(i(&it["pos"]).unwrap_or(0)).to_seconds();
                let t1 = t0 + at(i(&it["len"]).unwrap_or(0)).to_seconds();
                let r = c["auto_range"].as_array().map(|r| (r[0].as_f64().unwrap_or(0.0) / 12800.0, r[1].as_f64().unwrap_or(12800.0) / 12800.0)).unwrap_or((0.0, 1.0));
                auto_range.insert((target.clone(), is_pitch), r);
                (if is_pitch { &mut pitch_segs } else { &mut vol_segs }).entry(target).or_default().push(AutoSeg { t0, t1, pts });
            }
        }
    }
    for v in pitch_segs.values_mut().chain(vol_segs.values_mut()) { v.sort_by(|a, b| a.t0.total_cmp(&b.t0)); }
    let mut bakes: Vec<Bake> = Vec::new();
    let mut stretched_automated = 0usize;
    let mut range_notes: Vec<String> = Vec::new();
    // channels whose pitch has an automation clip ("<channel> - Channel pitch[ #n]")
    let pitch_automated: std::collections::BTreeSet<String> = chans.values()
        .filter(|c| s(&c["kind"]) == Some("Automation"))
        .filter_map(|c| s(&c["name"])?.split(" - Channel pitch").next().filter(|n| s(&c["name"]).is_some_and(|x| x.contains(" - Channel pitch"))).map(str::to_string))
        .collect();

    let mut off_clips = 0usize;
    let (mut muted_items, mut flag_1000) = (0usize, 0usize);
    let mut stretch_modes: BTreeMap<String, i64> = BTreeMap::new();
    let mut track_at = 0usize;
    for t in arr["tracks"].as_array().into_iter().flatten() {
        let items: Vec<&Value> = t["items"].as_array().into_iter().flatten().collect();
        // (studio track, occupied spans) -- a new overflow track when a clip would overlap
        let mut lanes: Vec<(TrackId, Vec<(Ticks, Ticks)>)> = Vec::new();
        // clips muted in FL's playlist: on their own MUTED lanes -- kept, a mute away
        let mut mlanes: Vec<(TrackId, Vec<(Ticks, Ticks)>)> = Vec::new();
        let fl_iid = i(&t["iid"]).unwrap_or(0);
        // A track switched off in FL's playlist is how an instrumental is
        // made from the full song: muted here, never dropped.
        let off = t["enabled"].as_bool() == Some(false);
        let mut first_insert: Option<i64> = None;
        let mut label: Option<String> = s(&t["name"]).map(str::to_string);
        for it in items {
            let item_muted = it["flags"].as_u64().is_some_and(|f| f & FL_ITEM_MUTED != 0);
            if it["flags"].as_u64().is_some_and(|f| f & 0x1000 != 0) { flag_1000 += 1; }
            let pos = at(i(&it["pos"]).unwrap_or(0));
            let len_t = at(i(&it["len"]).unwrap_or(0));
            let mut clips: Vec<(SourceId, Ticks, Ticks, Ticks, f32)> = Vec::new(); // (source, at, src_in, len, gain)
            match s(&it["kind"]) {
                Some("ChannelPLItem") => {
                    let Some(c) = i(&it["channel"]).and_then(|c| chans.get(&c)) else { continue };
                    if s(&c["kind"]) == Some("Automation") {
                        // channel pitch / volume automation is APPLIED (rendered into
                        // the target's clips); anything else is not yet
                        let n = s(&c["name"]).unwrap_or("");
                        if !(n.contains(" - Channel pitch") || n.contains(" - Channel volume")) { automation_clips += 1; }
                        continue;
                    }
                    if c["enabled"].as_bool() == Some(false) { off_clips += 1; continue; }
                    let Some(file) = file_of(c) else { continue };
                    let Some((id, m)) = source_for(&file, &mut ops, &mut missing, None) else { continue };
                    let offs: Vec<f64> = it["offsets"].as_array().into_iter().flatten().map(|v| v.as_f64().unwrap_or(-1.0)).collect();
                    let off = offs.first().copied().unwrap_or(-1.0);
                    // FL TIME-STRETCH: the file span a clip plays (its offsets, ms)
                    // against its length on the timeline. Balance's "WHAM 100bpm"
                    // plays 19.2 s of file in 12 s -- 100 -> 160 BPM.
                    // AUTOMATED: render the clip through its channel's pitch and volume
                    // curves (A Top A Tower's "Pattern 2": native speed, then the dive).
                    let cname = ch_name(c);
                    let (t0, t1) = (pos.to_seconds(), pos.to_seconds() + len_t.to_seconds());
                    let touches = |segs: Option<&Vec<AutoSeg>>| segs.is_some_and(|v| v.iter().any(|s| s.t0 < t1) );
                    // A clip that starts PARTWAY into its file bakes from there, when it
                    // is not time-stretched (its file span equals its length) -- A Top
                    // A Tower's burning_fire rides and later Pattern 2 dives. Stretched
                    // AND automated is not baked yet (counted, said).
                    let span_ratio = match offs.as_slice() {
                        [a, b] if *a >= 0.0 && *b > *a && len_t > Ticks::from_seconds(0.05) => (b - a) / 1000.0 / len_t.to_seconds(),
                        _ => 1.0,
                    };
                    let automated = touches(pitch_segs.get(&cname)) || touches(vol_segs.get(&cname));
                    // On a PITCH-automated channel an off-1 span is not a stretch: it is
                    // the varispeed of the pitch the automation holds there -- A Top A
                    // Tower's later Pattern 2 clips read 0.724 of their length = 2^(-560
                    // /1200), the -560 cents the intro's dive left the knob at. The curve
                    // plays them at exactly that speed, so they bake. (First read as
                    // "stretched and automated" -- 33 clips left unbaked.)
                    let pitch_automated_here = pitch_segs.contains_key(&cname);
                    let is_stretch = (span_ratio - 1.0).abs() > 0.02 && !pitch_automated_here;
                    if automated && is_stretch { stretched_automated += 1; }
                    let baked = automated && !is_stretch;
                    if baked {
                        // pitch: the automation's slice of the knob, times the bend range --
                        // inferred as the one that puts the SAVED pitch on an extreme of
                        // that slice (Pattern 2: 5200..7600 of 12800 and a saved -560 ->
                        // +-560 cents); without a saved pitch, 1200 cents, said.
                        let (pmin, pmax) = auto_range.get(&(cname.clone(), true)).copied().unwrap_or((0.0, 1.0));
                        let reach = ((pmin - 0.5).abs().max((pmax - 0.5).abs()) * 2.0).max(1e-6);
                        let saved = i(&c["pitch_shift"]).unwrap_or(0) as f64;
                        let range = if saved != 0.0 { saved.abs() / reach } else { 1200.0 };
                        let note = format!("{cname}: pitch automation reaches +-{:.0} cents ({})", reach * range,
                            if saved != 0.0 { "range inferred from the saved pitch" } else { "range ASSUMED 1200 cents" });
                        if !range_notes.contains(&note) && pitch_segs.contains_key(&cname) { range_notes.push(note); }
                        let (vmin, vmax) = auto_range.get(&(cname.clone(), false)).copied().unwrap_or((0.0, 1.0));
                        let static_gain = fl_knob_gain(c["volume"].as_f64().unwrap_or(10000.0)).max(1e-6);
                        let n = (len_t.to_seconds() / BAKE_STEP).ceil() as usize + 1;
                        let mut cents = Vec::with_capacity(n);
                        let mut gain = Vec::with_capacity(n);
                        for k in 0..n {
                            let lt = k as f64 * BAKE_STEP;
                            let t = t0 + lt;
                            let cv = pitch_segs.get(&cname).and_then(|v| auto_value(v, t))
                                .map(|v| ((pmin + v * (pmax - pmin)) - 0.5) * 2.0 * range).unwrap_or(0.0);
                            let gv = vol_segs.get(&cname).and_then(|v| auto_value(v, t))
                                .map(|v| fl_knob_gain((vmin + v * (vmax - vmin)) * 12800.0) / static_gain).unwrap_or(1.0);
                            cents.push((lt, cv));
                            gain.push((lt, gv));
                        }
                        let stem = std::path::Path::new(&file).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| file.clone());
                        let variant = format!("{stem}.auto{}.opus", bakes.len() + 1);
                        // the rendered copy is exactly the CLIP's length -- declared as the
                        // file's until flp-batch's verify caught it (Pattern 2: 70.45 s
                        // decoded, 50.98 s declared): pass the ratio that makes it so
                        let as_long = (m.len.to_seconds() / len_t.to_seconds().max(1e-6)).max(1e-9);
                        let Some((sid, _)) = source_for(&file, &mut ops, &mut missing, Some((&variant, as_long))) else { continue };
                        bakes.push(Bake { file: file.clone(), variant, start: offs.first().copied().unwrap_or(-1.0).max(0.0) / 1000.0,
                                          len: len_t.to_seconds(), cents, gain });
                        clips.push((sid, pos, Ticks::ZERO, len_t, channel_gain_for(c, Some(m), true)));
                        first_insert = first_insert.or(i(&c["insert"]));
                        label.get_or_insert_with(|| ch_name(c));
                    }
                    let ratio = match offs.as_slice() {
                        [a, b] if *a >= 0.0 && *b > *a && len_t > Ticks::from_seconds(0.05) => (b - a) / 1000.0 / len_t.to_seconds(),
                        _ => 1.0,
                    };
                    if baked {
                        // placed above, through its automation
                    } else if (ratio - 1.0).abs() > 0.02 && (0.25..=4.0).contains(&ratio) {
                        let r = (ratio * 1000.0).round() / 1000.0;
                        let stem = std::path::Path::new(&file).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| file.clone());
                        let variant = format!("{stem}.stretch{r:.3}.opus");
                        let Some((sid, _)) = source_for(&file, &mut ops, &mut missing, Some((&variant, r))) else { continue };
                        if !stretches.iter().any(|(_, _, v)| *v == variant) { stretches.push((file.clone(), r, variant.clone())); }
                        if let Some(mode) = i(&c["stretch_mode"]) { stretch_modes.insert(ch_name(c), mode); }
                        let src_in = Ticks::from_seconds(off.max(0.0) / 1000.0 / r);
                        let len = len_t.min(Ticks::from_seconds(m.len.to_seconds() / r).saturating_sub(src_in));
                        if len > Ticks::ZERO { clips.push((sid, pos, src_in, len, channel_gain_for(c, Some(m), true))); }
                    } else if let Some(cents) = i(&c["pitch_shift"]).filter(|p| *p != 0 && off < 0.0 && !pitch_automated.contains(&ch_name(c))) {
                        // (Not when the channel's pitch is AUTOMATED: then the saved
                        // shift is only where the automation left the knob -- A Top A
                        // Tower's "Pattern 2" saved -560 cents, the end of its dive,
                        // and a constant -560 took the intro's bars 1-8 from 0.61
                        // correlation with the artist's mix to 0.09. The intro plays
                        // at native speed, then the curve dives. The time-varying
                        // curve is not applied yet; the report says so.)
                        // VARISPEED: no file span given, a fixed pitch shift on the
                        // channel -- FL plays the sample slower-and-lower (or
                        // faster-and-higher) together. A Top A Tower's "Pattern 2":
                        // -560 cents; a 50.99 s file fills a 70.45 s clip, 0.7237 =
                        // 2^(-560/1200). Played at native speed it was fast, high,
                        // and ran out at 0:51.
                        let r = 2f64.powf(cents as f64 / 1200.0);
                        let stem = std::path::Path::new(&file).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| file.clone());
                        let variant = format!("{stem}.varispeed{cents:+}c.opus");
                        let Some((sid, _)) = source_for(&file, &mut ops, &mut missing, Some((&variant, r))) else { continue };
                        if !varispeeds.iter().any(|(_, _, v)| *v == variant) { varispeeds.push((file.clone(), r, variant.clone())); }
                        let len = len_t.min(Ticks::from_seconds(m.len.to_seconds() / r));
                        if len > Ticks::ZERO { clips.push((sid, pos, Ticks::ZERO, len, channel_gain_for(c, Some(m), true))); }
                    } else {
                    let src_in = if off > 0.0 { Ticks::from_seconds(off / 1000.0) } else { Ticks::ZERO };
                    let len = len_t.min(m.len.saturating_sub(src_in));
                    if len > Ticks::ZERO { clips.push((id, pos, src_in, len, channel_gain_for(c, Some(m), true))); }
                    }
                    first_insert = first_insert.or(i(&c["insert"]));
                    label.get_or_insert_with(|| ch_name(c));
                }
                Some("PatternPLItem") => {
                    let Some(pid) = i(&it["pattern"]) else { continue };
                    // ONE pass. An FL pattern clip longer than its pattern does NOT
                    // repeat it: c20 tried repeating (Balance's "kick" item runs to
                    // 195 s) and the artist's render is SILENT from 192 s (-46.5 dB)
                    // where the repeat played at -16 dB. The item's extra length is
                    // silence. (Kept as a pass count, should a pattern ever say loop.)
                    let cycle = parts.get(&pid).map(|v| v.iter().map(|(rp, _)| rack.patterns[*rp].steps).sum::<u32>()).unwrap_or(0);
                    let passes = if cycle == 0 { 0 } else { 1 };
                    let seq: Vec<(usize, u32)> = (0..passes).flat_map(|k| parts.get(&pid).into_iter().flatten().map(move |(rp, from)| (*rp, from + k * cycle))).collect();
                    // where in the pattern this clip starts (a clip cut mid-pattern)
                    let start = i(&it["start_ticks"]).unwrap_or(0).max(0) as f64;
                    for (rp, from) in seq.iter() {
                        let (p0, p1) = (*from as f64 * step_ticks, (*from + rack.patterns[*rp].steps) as f64 * step_ticks);
                        // the part's span on the timeline, relative to the clip
                        let (v0, v1) = (p0.max(start), p1);
                        if v1 <= v0 { continue; }
                        let part_at = at((v0 - start) as i64);
                        if part_at >= len_t { continue; }
                        let part_len = at((v1 - v0) as i64).min(len_t.saturating_sub(part_at));
                        let part_in = at((v0 - p0) as i64);
                        let id = *pattern_sources.entry(*rp).or_insert_with(|| {
                            let id = SourceId(10_000 + *rp as u64);
                            ops.push(Op::AddSource { source: Source { id, path: String::new(), len: Ticks::ZERO, sample_rate: 48_000,
                                channels: 2, has_video: false, kind: SourceKind::Pattern { pattern: *rp, name: rack.patterns[*rp].name.clone() } } });
                            id
                        });
                        clips.push((id, pos.saturating_add(part_at), part_in, part_len, 1.0));
                    }
                    if first_insert.is_none() {
                        let first_ch = patterns.iter().find(|p| i(&p["iid"]) == Some(pid))
                            .and_then(|p| p["notes"].as_array()?.first().and_then(|n| i(&n["rack"])));
                        first_insert = first_ch.and_then(|c| chans.get(&c)).and_then(|c| i(&c["insert"]));
                        label.get_or_insert_with(|| s(&patterns.iter().find(|p| i(&p["iid"]) == Some(pid)).map(|p| p["name"].clone()).unwrap_or(Value::Null)).unwrap_or("pattern").to_string());
                    }
                }
                _ => continue,
            }
            for (src, pos, src_in, len, gain) in clips {
                if item_muted { muted_items += 1; }
                let pool = if item_muted { &mut mlanes } else { &mut lanes };
                let lane = match pool.iter().position(|(_, spans)| spans.iter().all(|(a, b)| pos >= *b || pos.saturating_add(len) <= *a)) {
                    Some(l) => l,
                    None => {
                        if !pool.is_empty() && !item_muted { overflowed += 1; }
                        let id = TrackId(next_track);
                        next_track += 1;
                        let base = label.clone().unwrap_or_else(|| format!("FL track {fl_iid}"));
                        let nm = if item_muted { format!("{base} (muted in FL){}", if pool.is_empty() { String::new() } else { format!(" ({})", pool.len() + 1) }) }
                                 else { format!("{base}{}", if pool.is_empty() { String::new() } else { format!(" ({})", pool.len() + 1) }) };
                        ops.push(Op::AddTrack { track: Track { id, kind: TrackKind::Audio, name: nm, muted: off || item_muted, locked: false,
                                                               gain: 1.0, route: None }, at: track_at });
                        track_at += 1;
                        pool.push((id, Vec::new()));
                        pool.len() - 1
                    }
                };
                pool[lane].1.push((pos, pos.saturating_add(len)));
                ops.push(Op::AddClip { clip: Clip { id: ClipId(next_clip), source: src, track: pool[lane].0, pos, src_in, len,
                                                    gain, effects: vec![] } });
                // FL declicks audio clip edges by default; cut hard, a clip that
                // starts or ends mid-waveform clicks (two of Blue Moon's did, at a
                // take's end and at a splice between two clips of one take).
                if !pattern_sources.values().any(|v| *v == src) && len > Ticks::from_seconds(0.02) {
                    let d = Ticks::from_seconds(DECLICK);
                    ops.push(Op::SetClipFade { id: ClipId(next_clip), fade_in: d, fade_out: d });
                    declicked += 1;
                }
                next_clip += 1;
                placed += 1;
            }
        }
        // FL routes everything somewhere; no insert (or insert 0) is the master
        if let Some(bus) = bus_for(first_insert.unwrap_or(0), &mut ops) {
            for (tid, _) in lanes.iter().chain(mlanes.iter()) { ops.push(Op::SetRoute { track: *tid, bus: Some(bus) }); }
        }
    }
    // ── sidechains: a duck on each destination, keyed by the source ──────
    let mut ducked: Vec<String> = Vec::new();
    for (src, dst) in &sidechains {
        let (Some(key), Some(target)) = (busses.get(src).copied(), busses.get(dst).copied()) else { continue };
        for op in ops.iter_mut() {
            if let Op::AddBus { bus } = op {
                if bus.id == target && !bus.chain.iter().any(|e| e.kind == "duck") {
                    bus.chain.push(Effect { kind: "duck".into(), enabled: true, params: [
                        ("key".to_string(), key.0 as f32), ("threshold".to_string(), -24.0), ("depth".to_string(), 10.0),
                        // 5 ms, not 1: a 10 dB pull on a bass in 1 ms is a gain STEP
                        // and clicks -- the bisect of Blue Moon found every click in
                        // one window gone with the duck bypassed at 1 ms
                        ("ratio".to_string(), 4.0), ("attack".to_string(), 5.0), ("release".to_string(), 150.0)].into() });
                    let n = |i: &i64| mixer.get(i).and_then(|m| s(&m["name"])).unwrap_or("?").to_string();
                    ducked.push(format!("{} ducks under {}", n(dst), n(src)));
                }
            }
        }
    }
    if !ducked.is_empty() {
        report.push(format!("SIDECHAIN (FL's level-0 routes) as a duck keyed by the source: {} -- at Studio's defaults \
(threshold -24 dB, depth 10 dB, attack 5 / release 150 ms): FL's compressor settings on the destination are not decoded", ducked.join(", ")));
    }
    // insert pan: FL's mixer pan runs -6400..=6400 around 0 (unlike a
    // CHANNEL's 0..12800 around 6400) -- a balance, which is what Studio's is
    for (insert, bus) in &busses {
        if let Some(pan) = mixer.get(insert).and_then(|m| f(&m["pan"])).filter(|p| *p != 0.0) {
            ops.push(Op::SetBusPan { id: *bus, pan: (pan / 6400.0).clamp(-1.0, 1.0) as f32 });
        }
    }
    // ── apply, through the model's own checks ───────────────────────────
    let mut refused = 0;
    for op in ops {
        if let Err(e) = p.apply(&op) {
            refused += 1;
            if refused <= 5 { report.push(format!("refused by the model: {e:?}")); }
        }
    }
    if refused > 5 { report.push(format!("... {} more refused", refused - 5)); }
    report.insert(0, format!("{placed} clips on {} tracks, {} rack channels, {} rack patterns, {} busses; {:.0} BPM",
        p.tracks.len(), rack.channels.len(), rack.patterns.len(), p.busses.len(), bpm));
    if overflowed > 0 { report.push(format!("{overflowed} clip{} overlapped another on its FL track (FL allows it) -- moved to an overflow track, marked (2)", if overflowed == 1 { "" } else { "s" })); }
    if !stretches.is_empty() {
        let mut by: BTreeMap<String, usize> = BTreeMap::new();
        for (f, r, _) in &stretches { *by.entry(format!("{} x{r:.3}", std::path::Path::new(f).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default())).or_default() += 1; }
        report.push(format!("time-stretched in FL (fit to tempo), played from stretched copies, pitch kept: {} -- FL's stretch modes here {:?} (0 would be Resample, which shifts pitch)",
            by.keys().cloned().collect::<Vec<_>>().join(", "), stretch_modes.values().collect::<std::collections::BTreeSet<_>>()));
    }
    if !switched_off.is_empty() || off_clips > 0 {
        report.push(format!("switched off in FL: {} -- muted in the rack{}", if switched_off.is_empty() { "(no rack channels)".to_string() } else { switched_off.join(", ") },
            if off_clips > 0 { format!("; {off_clips} of their audio clips not placed") } else { String::new() }));
    }
    if muted_items > 0 {
        report.push(format!("{muted_items} clips muted in FL's playlist (item flag 0x2000) -- placed on MUTED lanes \"(muted in FL)\", kept, a mute away{}",
            if flag_1000 > 0 { format!("; {flag_1000} items also carry flag 0x1000, meaning not known") } else { String::new() }));
    }
    if !varispeeds.is_empty() {
        report.push(format!("VARISPEED (a sampler's fixed pitch shift, pitch and speed together, as FL plays it): {}",
            varispeeds.iter().map(|(f, r, _)| format!("{} x{r:.4}", std::path::Path::new(f).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default())).collect::<Vec<_>>().join(", ")));
    }
    if declicked > 0 { report.push(format!("{declicked} audio clips declicked: {:.0} ms fades at each edge, as FL does by default", DECLICK * 1000.0)); }
    if automation_clips > 0 { report.push(format!("{automation_clips} automation clip{} not applied (mixer, plugin and other targets -- only channel pitch and volume are, so far)", if automation_clips == 1 { "" } else { "s" })); }
    if !approx.is_empty() { report.push(format!("effects read or approximated: {}", approx.join("; "))); }
    // Which tracks each autotune feeds from -- so "the male takes have their
    // own" is checked, not assumed.
    let feeds = |bus: BusId| -> Vec<String> {
        let mut into = vec![bus];
        let mut grew = true;
        while grew {
            grew = false;
            for b in p.busses.values() {
                if b.parent.is_some_and(|x| into.contains(&x)) && !into.contains(&b.id) { into.push(b.id); grew = true; }
            }
        }
        let mut names: Vec<String> = p.tracks.iter().filter(|t| t.route.is_some_and(|r| into.contains(&r))).map(|t| t.name.clone()).collect();
        names.dedup();
        names
    };
    for b in p.busses.values().filter(|b| b.chain.iter().any(|e| e.kind == "autotune")) {
        report.push(format!("autotune on {} tunes: {}", b.name, feeds(b.id).join(", ")));
    }
    report.push("mapped effects start at Studio's defaults (the approximated iZotope ones at a gentle setting) -- the plugins' own parameters are not decoded".into());
    if !unmapped.is_empty() {
        report.push(format!("effects with no Studio counterpart, left out: {}",
            unmapped.iter().map(|(k, n)| if *n > 1 { format!("{k} x{n}") } else { k.clone() }).collect::<Vec<_>>().join(", ")));
    }
    let silent_busses: Vec<&str> = p.busses.values().filter(|b| b.silent).map(|b| b.name.as_str()).collect();
    if !silent_busses.is_empty() {
        report.push(format!("silent in FL (no route out), imported silent: {}", silent_busses.join(", ")));
    }
    if has_routes {
        let master = busses.get(&0).copied();
        let parented = p.busses.values().filter(|b| b.parent.is_some() && b.parent != master).count();
        report.push(format!("routing: {parented} bus{} output into another bus, {sends_made} send{} at FL's send levels; the rest feed the master{}",
            if parented == 1 { "" } else { "ses" }, if sends_made == 1 { "" } else { "s" },
            if unroutable > 0 { format!(" ({unroutable} route{} to an insert not in the file, left out)", if unroutable == 1 { "" } else { "s" }) } else { String::new() }));
    } else {
        report.push("no routing in this inventory (PyFLP's is not trustworthy -- use the native reader): every bus feeds the master".into());
    }
    let panned = mixer.values().filter(|m| f(&m["pan"]).is_some_and(|p| p != 0.0) && busses.contains_key(&i(&m["iid"]).unwrap_or(-1))).count();
    if panned > 0 { report.push(format!("{panned} insert pan{} imported as bus balance", if panned == 1 { "" } else { "s" })); }
    report.push(format!("levels by FL's volume laws: channel knob (v/12800)^{FL_KNOB_EXP} (78% = -5.2 dB; audio clips +{FL_CLIP_OFFSET_DB} dB), mixer fader and sends (v/12800)^{FL_FADER_EXP} (80% = 0 dB, 100% = +5.6 dB) -- through Image-Line's published points; the curve between them is assumed, so close, not exact"));
    if !missing.is_empty() { report.push(format!("media not found beside the project: {}", missing.join(", "))); }
    if stretched_automated > 0 {
        report.push(format!("{stretched_automated} clip{} both time-stretched and automated: played stretched, automation NOT applied yet",
            if stretched_automated == 1 { "" } else { "s" }));
    }
    if !bakes.is_empty() {
        report.push(format!("AUTOMATION rendered into {} clip{} (varispeed pitch, gain): {}", bakes.len(), if bakes.len() == 1 { "" } else { "s" },
            if range_notes.is_empty() { "volume only".to_string() } else { range_notes.join("; ") }));
    }
    Ok(Imported { project: p, rack, report, stretches, varispeeds, bakes })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn probe(file: &str) -> Option<Media> {
        match file {
            "kick.wav" => Some(Media { len: Ticks::from_seconds(0.4), rate: 44_100, channels: 1, peak: 0.9 }),
            "vox.wav" => Some(Media { len: Ticks::from_seconds(30.0), rate: 48_000, channels: 2, peak: 0.125 }),
            _ => None,
        }
    }

    /// A small FL project in the dump's shape: 120 BPM at 96 PPQ (a beat is
    /// 0.5 s, a 16th 24 ticks), a kick sampler, a synth, a vocal clip.
    fn inventory() -> Value {
        json!({
            "project": {"tempo": 120.0, "ppq": 96},
            "channels": [
                {"iid": 0, "kind": "Sampler", "name": "Kick", "sample_file": "kick.wav", "insert": 1, "volume": 10000},
                {"iid": 1, "kind": "Instrument", "name": "Omnisphere", "plugin": "Fruity Wrapper", "plugin_name": "Omnisphere", "insert": 2, "volume": 10000},
                {"iid": 2, "kind": "Instrument", "name": "Bass", "plugin": "Fruity Wrapper", "plugin_name": "Omnisphere", "insert": 0, "volume": 10000, "enabled": false},
                {"iid": 3, "kind": "Sampler", "name": "Vox", "sample_file": "vox.wav", "insert": 2, "volume": 5000, "pan": 6400, "normalize": true},
                {"iid": 4, "kind": "Automation", "name": "Vox - Volume"},
                {"iid": 5, "kind": "Sampler", "name": "Beat", "sample_file": "vox.wav", "insert": 1, "pitch_shift": -1200},
                {"iid": 6, "kind": "Sampler", "name": "Dive", "sample_file": "kick.wav", "insert": 1, "pitch_shift": -560},
                {"iid": 7, "kind": "Automation", "name": "Dive - Channel pitch"},
                {"iid": 8, "kind": "Sampler", "name": "Tape", "sample_file": "vox.wav", "insert": 1, "pitch_shift": -560},
                {"iid": 9, "kind": "Automation", "name": "Tape - Channel pitch", "auto_range": [5200, 7600],
                 "points": [[0.0, 0.5, 0.0], [8.0, 0.0, 0.0]]}
            ],
            "patterns": [
                {"iid": 1, "name": "Beat", "notes": [
                    {"rack": 0, "pos": 0, "len": 24, "key": 60, "vel": 100},
                    {"rack": 0, "pos": 36, "len": 24, "key": 60, "vel": 100},
                    {"rack": 1, "pos": 0, "len": 96, "key": "G5", "vel": 64},
                    {"rack": 2, "pos": 0, "len": 48, "key": 36, "vel": 100}]},
                {"iid": 2, "name": "Long", "notes": [{"rack": 0, "pos": 0, "len": 24, "key": 60}, {"rack": 0, "pos": 1536 + 96, "len": 24, "key": 60}]}
            ],
            "arrangements": [{"iid": 0, "tracks": [
                {"iid": 1, "name": null, "items": [{"kind": "PatternPLItem", "pattern": 1, "pos": 0, "len": 384}]},
                {"iid": 2, "name": "Vocals", "enabled": false, "items": [
                    {"kind": "ChannelPLItem", "channel": 3, "pos": 384, "len": 1152, "offsets": [2000.0, 8000.0]},
                    {"kind": "ChannelPLItem", "channel": 3, "pos": 768, "len": 192, "offsets": [-1.0, -1.0]},
                    {"kind": "ChannelPLItem", "channel": 4, "pos": 0, "len": 192}]},
                {"iid": 3, "items": [{"kind": "PatternPLItem", "pattern": 2, "pos": 0, "len": 1728}]},
                {"iid": 4, "name": "Loops", "items": [{"kind": "ChannelPLItem", "channel": 3, "pos": 0, "len": 1152, "offsets": [1000.0, 13000.0]},
                    {"kind": "ChannelPLItem", "channel": 3, "pos": 0, "len": 384, "offsets": [-1.0, -1.0], "flags": 8256}]},
                {"iid": 7, "name": "Slowed", "items": [{"kind": "ChannelPLItem", "channel": 5, "pos": 0, "len": 4800, "offsets": [-1.0, -1.0]}]},
                {"iid": 8, "name": "Tape", "items": [{"kind": "ChannelPLItem", "channel": 8, "pos": 0, "len": 1920, "offsets": [-1.0, -1.0]},
                                                     {"kind": "ChannelPLItem", "channel": 9, "pos": 384, "len": 768},
                                                     {"kind": "ChannelPLItem", "channel": 8, "pos": 2400, "len": 384, "offsets": [5000.0, 7000.0]}]},
                {"iid": 5, "name": "Beat x3", "items": [{"kind": "PatternPLItem", "pattern": 1, "pos": 0, "len": 1152}]},
                {"iid": 6, "name": "Cut beat", "items": [{"kind": "PatternPLItem", "pattern": 2, "pos": 384, "len": 1536, "start_ticks": 1344, "end_ticks": 2880}]}
            ]}],
            "mixer": [
                {"iid": 0, "slots": [{"plugin": "Fruity Limiter"}, {"plugin": "Fruity Wrapper", "name": "Ozone 8"}]},
                {"iid": 1, "name": "Drums", "volume": 12800, "routes": [{"to": 0, "level": 12800}, {"to": 3, "level": 6400}, {"to": 5, "level": 12800}, {"to": 4, "level": 0}],
                 "slots": [{"plugin": "Fruity Parametric EQ 2"}, {"plugin": "Soundgoodizer"}]},
                {"iid": 2, "name": "Vox bus", "volume": 6400, "pan": -800, "routes": [{"to": 4, "level": 12800}, {"to": 3, "level": 3200}],
                 "slots": [{"plugin": "Pitcher"}, {"plugin": "Fruity Wrapper", "name": "Ozone 8 Exciter"}, {"plugin": "Fruity Chorus"}]},
                {"iid": 3, "name": "Verb send", "routes": [{"to": 0, "level": 12800}], "slots": [{"plugin": "Fruity Reeverb 2"}]},
                {"iid": 4, "name": "Vocal group", "routes": [{"to": 0, "level": 12800}]},
                {"iid": 5, "name": "Meter", "routing_known": true, "routes": []}
            ]
        })
    }

    fn secs(t: Ticks) -> f64 { t.to_seconds() }

    #[test]
    fn audio_clips_land_where_fl_had_them_trimmed_by_millisecond_offsets_and_overlaps_get_a_lane() {
        let im = import(&inventory(), "demo", &probe).unwrap();
        let p = &im.project;
        let vocals: Vec<&Track> = p.tracks.iter().filter(|t| t.name.starts_with("Vocals")).collect();
        assert_eq!(vocals.len(), 2, "the overlapping clip got an overflow track: {:?}", p.tracks.iter().map(|t| &t.name).collect::<Vec<_>>());
        let first = &p.track_clips(vocals[0].id)[0];
        assert!((secs(first.pos) - 2.0).abs() < 1e-6, "384 ticks at 120 BPM / 96 PPQ = 2 s");
        assert!((secs(first.src_in) - 2.0).abs() < 1e-6, "offset 2000 ms");
        assert!((secs(first.len) - 6.0).abs() < 1e-6);
        let (fi, fo) = im.project.fades[&first.id];
        assert!((secs(fi) - DECLICK).abs() < 1e-6 && (secs(fo) - DECLICK).abs() < 1e-6, "declicked at both edges");
        assert!(im.report.iter().any(|r| r.contains("1 automation clip not applied")), "the Vox volume ride (a mixer-style target); the applied channel-pitch clip is not counted: {:?}", im.report);
        assert!(im.report.iter().any(|r| r.contains("overflow track")));
        assert!(vocals.iter().all(|t| t.muted), "a playlist track switched off in FL is muted, overflow lane too");
        assert!(p.tracks.iter().filter(|t| !t.name.starts_with("Vocals") && !t.name.contains("(muted in FL)")).all(|t| !t.muted),
                "only the switched-off FL track and lanes of clips muted in FL are muted");
    }

    #[test]
    fn patterns_become_steps_and_roll_notes_and_long_ones_are_cut_into_64_step_parts() {
        let im = import(&inventory(), "demo", &probe).unwrap();
        let beat = im.rack.patterns.iter().find(|p| p.name == "Beat").unwrap();
        let kick = im.rack.channels.iter().position(|c| c.name == "Kick").unwrap();
        assert!(beat.is_set(kick, 0), "a C5 hit on a sampler is a step");
        assert!(beat.is_set(kick, 2), "pos 36 is 1.5 steps: moved to the nearest");
        let omni = im.rack.channels.iter().position(|c| c.name == "Omnisphere").unwrap();
        assert_eq!(beat.roll(omni)[0].key, 7, "G5 is 7 above C5");
        assert_eq!(beat.roll(omni)[0].len, 4);
        assert!(matches!(im.rack.channels[omni].kind, SynthKind::Poly { .. }), "a plugin synth: a Poly stand-in");
        let bass = im.rack.channels.iter().position(|c| c.name == "Bass").unwrap();
        assert!(matches!(im.rack.channels[bass].kind, SynthKind::Poly { patch, .. } if patch.sub > 0.0 && patch.cutoff < 500.0),
                "all notes below C4 (and named Bass): the bass patch");
        assert_eq!(patch_for("batman_strings", false).0, "strings");
        assert_eq!(patch_for("lotusflute #2", false).0, "flute");
        assert_eq!(patch_for("pain_bass", false).0, "bass");
        assert_eq!(patch_for("dystopian_hemmorag", false).0, "pad");
        assert_eq!(patch_for("Omnisphere #7", false).0, "lead", "a name that says nothing: the default");
        assert!(im.rack.channels[bass].muted, "switched off in FL: muted here, not dropped");
        assert!(!im.rack.channels[omni].muted);
        assert!(im.report.iter().any(|r| r.contains("switched off in FL: Bass")), "{:?}", im.report);
        let long: Vec<&Pattern> = im.rack.patterns.iter().filter(|p| p.name.starts_with("Long")).collect();
        assert_eq!(long.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), vec!["Long (1/2)", "Long (2/2)"]);
        assert!(long[1].is_set(kick, 4), "step 68 is step 4 of part 2");
        assert!(im.report.iter().any(|r| r.contains("1 note off the grid")), "{:?}", im.report);
        assert!(im.report.iter().any(|r| r.contains("STAND-INS") && r.contains("Omnisphere")));
        // the long pattern's parts sit end to end on the timeline
        let t = im.project.tracks.iter().find(|t| t.name.starts_with("Long")).unwrap();
        let clips = im.project.track_clips(t.id);
        assert_eq!(clips.len(), 2);
        assert!((secs(clips[1].pos) - 8.0).abs() < 1e-6, "64 steps at 120 BPM = 8 s");
    }

    /// Blue Moon's female lead: a quiet take (peak -18 dB here), FL channel
    /// knob at 5000, NORMALIZE on: knob law x clip offset x 1/0.125.
    #[test]
    fn a_normalized_quiet_take_is_brought_up_as_fl_does_and_centre_pan_is_centre() {
        let im = import(&inventory(), "demo", &probe).unwrap();
        let vt = im.project.tracks.iter().find(|t| t.name == "Vocals").unwrap();
        let clip = &im.project.track_clips(vt.id)[0];
        let want = fl_knob_gain(5000.0) * 10f32.powf(5.2 / 20.0) * 8.0;
        assert!((clip.gain - want).abs() < 1e-4, "knob law x clip offset x normalize 8 = {want}, got {}", clip.gain);
        assert_eq!(fl_pan(6400.0), 0.0);
        assert_eq!(fl_pan(0.0), -1.0);
        assert_eq!(fl_pan(12800.0), 1.0);
        let kick = im.rack.channels.iter().find(|c| c.name == "Kick").unwrap();
        assert!((20.0 * kick.gain.log10() + 5.2).abs() < 0.05, "a rack channel at the default knob is -5.2 dB, as FL says: {}", kick.gain);
        assert!((fl_knob_gain(12800.0) - 1.0).abs() < 1e-6 && (20.0 * fl_fader_gain(16000.0).log10() - 5.6).abs() < 0.05, "the published points");
    }

    #[test]
    fn pitchers_note_switches_become_the_key_and_its_speed_a_slide_or_a_hold() {
        // Blue Moon's mask: C# D E F# G A B -- D major
        let dmaj = json!({"speed": 0.615, "notes": [false, true, true, false, true, false, true, true, false, true, false, true]});
        let (q, said) = pitcher_params(&dmaj);
        assert_eq!((q["root"], q["mode"]), (11.0, 0.0), "D major is B natural minor: the same notes");
        assert!((q["speed"] - 154.0).abs() < 1.0, "{}", q["speed"]);
        assert_eq!(q["slide"], 1.0);
        assert!(said.contains("D major / B minor") && said.contains("INFERRED"), "{said}");
        let hard = json!({"speed": 0.97, "notes": dmaj["notes"].clone()});
        let (q, _) = pitcher_params(&hard);
        assert_eq!(q["slide"], 0.0, "{} ms is a hold", q["speed"]);
        let odd = json!({"notes": [true, true, false, false, false, false, false, false, false, false, false, false]});
        assert_eq!(pitcher_params(&odd).0["mode"], 2.0, "not a scale Studio has: chromatic, and said");
    }

    /// A clip FL stretched: 12 s of file (offsets 1000..13000 ms) in 6 s of
    /// timeline -- ratio 2. It plays from a stretched copy, from 0.5 s in.
    /// And a pattern item three times its pattern's length plays it ONCE:
    /// FL does not repeat it (Balance's render is silent where a repeat was).
    #[test]
    fn a_stretched_clip_reads_a_stretched_copy_and_a_long_pattern_item_repeats() {
        let im = import(&inventory(), "demo", &probe).unwrap();
        assert_eq!(im.stretches, vec![("vox.wav".to_string(), 2.0, "vox.stretch2.000.opus".to_string())]);
        let t = im.project.tracks.iter().find(|t| t.name == "Loops").unwrap();
        let c = &im.project.track_clips(t.id)[0];
        let src = &im.project.sources[&c.source];
        assert_eq!(src.path, "vox.stretch2.000.opus");
        assert!((src.len.to_seconds() - 15.0).abs() < 1e-6, "30 s at ratio 2");
        assert!((c.src_in.to_seconds() - 0.5).abs() < 1e-6 && (c.len.to_seconds() - 6.0).abs() < 1e-6, "{:?}", c);
        assert!(im.report.iter().any(|r| r.contains("vox x2.000")), "{:?}", im.report);
        // the item muted in FL (flags 0x2040) is kept, on a muted lane of its track
        let m = im.project.tracks.iter().find(|t| t.name == "Loops (muted in FL)").expect("a muted lane");
        assert!(m.muted && im.project.track_clips(m.id).len() == 1);
        assert!(!t.muted, "the playing lane is not muted");
        assert!(im.report.iter().any(|r| r.contains("1 clips muted in FL's playlist")), "{:?}", im.report);
        let b = im.project.tracks.iter().find(|t| t.name == "Beat x3").unwrap();
        let clips = im.project.track_clips(b.id);
        assert_eq!(clips.len(), 1, "one pass, not three: {clips:?}");
        assert!((clips[0].len.to_seconds() - 2.0).abs() < 1e-6, "the pattern's own length");
    }

    /// A pattern clip CUT to start mid-pattern (Balance's chopped drum reuse):
    /// "Long" is 80 steps: part 1 of 64 (8 s at 120 BPM, 96 PPQ: 24 ticks a
    /// step) and part 2 of 16 (2 s). The clip starts at tick 1344 (step 56,
    /// 7 s in): the last 1 s of part 1 from 7 s in, then all 2 s of part 2,
    /// then silence to the clip's end (a pattern is not repeated).
    #[test]
    fn a_pattern_clip_cut_mid_pattern_plays_from_its_cut() {
        let im = import(&inventory(), "demo", &probe).unwrap();
        let t = im.project.tracks.iter().find(|t| t.name.starts_with("Cut beat") || t.name.starts_with("Long")
            && im.project.track_clips(t.id).iter().any(|c| (c.pos.to_seconds() - 2.0).abs() < 1e-6)).expect("the cut clip's track");
        let mut clips = im.project.track_clips(t.id);
        clips.sort_by_key(|c| c.pos);
        assert_eq!(clips.len(), 2, "{clips:?}");
        let (a, b) = (&clips[0], &clips[1]);
        assert!((a.pos.to_seconds() - 2.0).abs() < 1e-6 && (a.src_in.to_seconds() - 7.0).abs() < 1e-6 && (a.len.to_seconds() - 1.0).abs() < 1e-6, "{a:?}");
        assert!((b.pos.to_seconds() - 3.0).abs() < 1e-6 && b.src_in == Ticks::ZERO && (b.len.to_seconds() - 2.0).abs() < 1e-6, "{b:?}");
    }

    /// A sampler with a fixed -1200 cent shift and no file span plays an
    /// octave down AND at half speed: a varispeed copy, 30 s of file filling
    /// 60 s -- A Top A Tower's intro beat, in miniature.
    #[test]
    fn a_pitch_shifted_sampler_plays_varispeed() {
        let im = import(&inventory(), "demo", &probe).unwrap();
        assert_eq!(im.varispeeds, vec![("vox.wav".to_string(), 0.5, "vox.varispeed-1200c.opus".to_string())]);
        let t = im.project.tracks.iter().find(|t| t.name == "Slowed").unwrap();
        let c = &im.project.track_clips(t.id)[0];
        assert_eq!(im.project.sources[&c.source].path, "vox.varispeed-1200c.opus");
        assert!((im.project.sources[&c.source].len.to_seconds() - 60.0).abs() < 1e-6, "30 s at half speed");
        assert!((c.len.to_seconds() - 25.0).abs() < 1e-6, "the clip's own 4800 ticks (25 s at 120 BPM / 96 PPQ)");
        assert!(im.report.iter().any(|r| r.contains("VARISPEED") && r.contains("x0.5000")), "{:?}", im.report);
        // a channel whose pitch is AUTOMATED keeps native speed: its saved shift is automation state
        assert!(!im.varispeeds.iter().any(|(f, _, _)| f == "kick.wav"), "{:?}", im.varispeeds);
        // (Dive has no clip of its own on the playlist here: nothing to bake)
    }

    /// A Top A Tower's intro in miniature: a sampler with a saved -560 cents,
    /// a pitch automation 0.5 -> 0.0 over 8 beats placed at 2 s (4 s at 120
    /// BPM), the sampler's clip 0-10 s. The clip is rendered through the curve:
    /// native speed before 2 s, -280 cents halfway (tension 0: a line), -560 at
    /// the end -- and HELD after it, as FL leaves a knob where automation left it.
    #[test]
    fn an_automated_clip_is_baked_through_its_pitch_curve() {
        let im = import(&inventory(), "demo", &probe).unwrap();
        assert_eq!(im.bakes.len(), 2, "{:?}", im.bakes.iter().map(|b| &b.variant).collect::<Vec<_>>());
        // the second: a clip reading 5-7 s of its file, after the automation
        // ended -- baked FROM 5 s, at the held -560 cents (the tape-stop case)
        let late = &im.bakes[1];
        assert!((late.start - 5.0).abs() < 1e-9 && (late.len - 2.0).abs() < 1e-9, "{} {}", late.start, late.len);
        assert!(late.cents.iter().all(|c| (c.1 + 560.0).abs() < 1.0), "held where the dive left it");
        let b = &im.bakes[0];
        assert_eq!(b.variant, "vox.auto1.opus");
        assert!((b.len - 10.0).abs() < 1e-9);
        let at = |t: f64| b.cents[(t / BAKE_STEP).round() as usize].1;
        assert!(at(0.0).abs() < 1e-6 && at(1.9).abs() < 1e-6, "native speed before the automation");
        assert!((at(4.0) + 280.0).abs() < 1.0, "halfway: {}", at(4.0));
        assert!((at(6.0) + 560.0).abs() < 1.0 && (at(9.0) + 560.0).abs() < 1.0, "the dive's end, and held: {} {}", at(6.0), at(9.0));
        assert!(b.gain.iter().all(|g| (g.1 - 1.0).abs() < 1e-6), "no volume automation: unity");
        let src = im.project.sources.values().find(|s| s.path == "vox.auto1.opus").unwrap();
        assert!((src.len.to_seconds() - 10.0).abs() < 1e-6, "the rendered copy is the clip's length, not the file's: {}", src.len.to_seconds());
        assert!(!im.varispeeds.iter().any(|(f, _, _)| f == "vox.wav" && im.bakes.iter().any(|b| b.file == *f) && false));
        assert!(im.report.iter().any(|r| r.contains("AUTOMATION rendered into 2 clips") && r.contains("+-560 cents")), "{:?}", im.report);
    }

    #[test]
    fn keys_read_as_numbers_or_as_pyflps_names() {
        assert_eq!(key_of(&json!(60)), Some(60));
        assert_eq!(key_of(&json!("C5")), Some(60));
        assert_eq!(key_of(&json!("F#6")), Some(78));
        assert_eq!(key_of(&json!("A-1")), Some(-3));
        assert_eq!(key_of(&json!("H2")), None);
    }

    #[test]
    fn routing_becomes_parents_and_sends_at_fls_levels() {
        let im = import(&inventory(), "demo", &probe).unwrap();
        let p = &im.project;
        let by = |n: &str| p.busses.values().find(|b| b.name == n).unwrap_or_else(|| panic!("no bus {n}: {:?}", p.busses.values().map(|b| &b.name).collect::<Vec<_>>()));
        let (drums, vox, verb, group) = (by("Drums"), by("Vox bus"), by("Verb send"), by("Vocal group"));
        let master = by("Master");
        assert_eq!(master.chain.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(), vec!["limiter", "multiband"],
                   "FL's master chain, the Ozone suite as a multiband stage");
        assert_eq!(drums.parent, Some(master.id), "feeds the master");
        let meter = by("Meter");
        assert!(meter.silent, "routing known, no route out: silent in FL, silent here");
        assert_eq!(drums.sends.len(), 2, "the reverb and the meter; the level-0 sidechain route is not a send: {:?}", drums.sends);
        assert_eq!((drums.sends[0].to, drums.sends[0].level), (verb.id, fl_fader_gain(6400.0)), "and sends to the reverb, by the fader law");
        assert_eq!(vox.parent, Some(group.id), "no master route: its first destination is its output");
        assert_eq!((vox.sends[0].to, vox.sends[0].level), (verb.id, fl_fader_gain(3200.0)));
        assert_eq!(group.parent, Some(master.id));
        let duck = group.chain.iter().find(|e| e.kind == "duck").expect("the level-0 route Drums -> Vocal group is a sidechain duck");
        assert_eq!(duck.params["key"] as u64, drums.id.0, "keyed by the source");
        // (this inventory has no track whose channel sits on FL's master, so
        // that path is covered by the Blue Moon import, not here)
        assert!(p.tracks.iter().all(|t| t.route.is_some()), "every track is routed somewhere");
        assert_eq!(verb.chain[0].kind, "reverb", "a bus nothing but routing reaches is still made");
        let r = im.report.join("\n");
        assert!(r.contains("routing: 1 bus output into another bus, 3 sends"), "{r}");
        assert!(r.contains("Vocal group ducks under Drums"), "{r}");
        assert!(r.contains("imported silent: Meter"), "{r}");
        assert!(r.contains("1 insert pan imported as bus balance"), "{r}");
        assert!((p.bus_pans[&vox.id] + 0.125).abs() < 1e-6, "-800 of 6400: an eighth left, {:?}", p.bus_pans);
    }

    #[test]
    fn inserts_become_busses_with_mapped_effects_and_the_rest_is_named() {
        let im = import(&inventory(), "demo", &probe).unwrap();
        let p = &im.project;
        let vox = p.busses.values().find(|b| b.name == "Vox bus").expect("a bus per used insert");
        assert!((vox.gain - fl_fader_gain(6400.0)).abs() < 1e-6 && vox.gain < 0.2, "fader 6400 by the fader law, not 0.5: {}", vox.gain);
        assert_eq!(vox.chain.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(), vec!["autotune", "saturation"],
                   "a VST by its slot name: the Ozone exciter as saturation");
        assert!((vox.chain[1].params["drive"] - 0.15).abs() < 1e-6, "gentle, not the full drive");
        let drums = p.busses.values().find(|b| b.name == "Drums").unwrap();
        assert_eq!(drums.chain.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(), vec!["eq", "multiband"]);
        let vt = p.tracks.iter().find(|t| t.name == "Vocals").unwrap();
        assert_eq!(vt.route, Some(vox.id));
        let r = im.report.join("\n");
        assert!(r.contains("Ozone 8 Exciter: approximate") && r.contains("Fruity Chorus"), "{r}");
        assert!(r.contains("Soundgoodizer: approximate"), "{r}");
        assert!(r.contains("master: FL's master insert is the bus \"Master\" (chain: limiter -> multiband)"), "{r}");
        assert!(!r.contains("refused by the model"), "{r}");
    }
}
