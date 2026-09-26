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
    /// Everything approximated, skipped or refused, grouped, for a person.
    pub report: Vec<String>,
}

/// FL's channel pan: 0..=12800 with 6400 CENTRE. Read as signed around 0
/// until c20, which put every channel of Blue Moon hard right.
pub fn fl_pan(v: f64) -> f32 { ((v - 6400.0) / 6400.0).clamp(-1.0, 1.0) as f32 }

/// A channel's level on what it plays: its volume (10000 = unity, FL's
/// default) times, when the sample is NORMALIZEd in FL, the gain that brings
/// the file's peak to full scale -- FL applies that before the channel.
pub fn channel_gain(c: &Value, m: Option<Media>) -> f32 {
    let vol = (c["volume"].as_f64().unwrap_or(10000.0) / 10000.0) as f32;
    let norm = match m { Some(m) if c["normalize"].as_bool() == Some(true) && m.peak > 1e-4 => 1.0 / m.peak, _ => 1.0 };
    vol * norm
}

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
            stand_ins.push(format!("{} ({}) -> {}", ch_name(c), if plugin.is_empty() { "plugin" } else { &plugin },
                                   if low { "Sub" } else { "Sine" }));
            if low { SynthKind::Sub { note: 0, glide_from: None, glide_ms: 0.0, drive: 0.0, decay_ms: 400.0 } }
            else { SynthKind::Sine { hz: 261.63 } }
        };
        rack_ix.insert(*fl, rack.channels.len());
        let pan = fl_pan(f(&c["pan"]).unwrap_or(6400.0));
        rack.channels.push(Channel { name: ch_name(c), kind, gain: channel_gain(c, media).clamp(0.0, 16.0),
                                     pan, muted: false, choke_group: None });
    }
    if !stand_ins.is_empty() {
        report.push(format!("{} plugin synth{} cannot be hosted and their sound is not in the file -- STAND-INS, notes kept: {}",
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
            gain: (f(&m["volume"]).unwrap_or(12800.0) / 12800.0) as f32, chain, sends: Vec::new(), muted: false, silent: false } });
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
            .filter_map(|r| Some((i(&r["to"])?, (f(&r["level"]).unwrap_or(12800.0) / 12800.0) as f32))).collect();
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
        let gain = (f(&m["volume"]).unwrap_or(12800.0) / 12800.0) as f32;
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
    let mut stretch_modes: BTreeMap<String, i64> = BTreeMap::new();
    let mut track_at = 0usize;
    for t in arr["tracks"].as_array().into_iter().flatten() {
        let items: Vec<&Value> = t["items"].as_array().into_iter().flatten().collect();
        // (studio track, occupied spans) -- a new overflow track when a clip would overlap
        let mut lanes: Vec<(TrackId, Vec<(Ticks, Ticks)>)> = Vec::new();
        let fl_iid = i(&t["iid"]).unwrap_or(0);
        // A track switched off in FL's playlist is how an instrumental is
        // made from the full song: muted here, never dropped.
        let off = t["enabled"].as_bool() == Some(false);
        let mut first_insert: Option<i64> = None;
        let mut label: Option<String> = s(&t["name"]).map(str::to_string);
        for it in items {
            let pos = at(i(&it["pos"]).unwrap_or(0));
            let len_t = at(i(&it["len"]).unwrap_or(0));
            let mut clips: Vec<(SourceId, Ticks, Ticks, Ticks, f32)> = Vec::new(); // (source, at, src_in, len, gain)
            match s(&it["kind"]) {
                Some("ChannelPLItem") => {
                    let Some(c) = i(&it["channel"]).and_then(|c| chans.get(&c)) else { continue };
                    if s(&c["kind"]) == Some("Automation") { automation_clips += 1; continue; }
                    let Some(file) = file_of(c) else { continue };
                    let Some((id, m)) = source_for(&file, &mut ops, &mut missing, None) else { continue };
                    let offs: Vec<f64> = it["offsets"].as_array().into_iter().flatten().map(|v| v.as_f64().unwrap_or(-1.0)).collect();
                    let off = offs.first().copied().unwrap_or(-1.0);
                    // FL TIME-STRETCH: the file span a clip plays (its offsets, ms)
                    // against its length on the timeline. Balance's "WHAM 100bpm"
                    // plays 19.2 s of file in 12 s -- 100 -> 160 BPM.
                    let ratio = match offs.as_slice() {
                        [a, b] if *a >= 0.0 && *b > *a && len_t > Ticks::from_seconds(0.05) => (b - a) / 1000.0 / len_t.to_seconds(),
                        _ => 1.0,
                    };
                    if (ratio - 1.0).abs() > 0.02 && (0.25..=4.0).contains(&ratio) {
                        let r = (ratio * 1000.0).round() / 1000.0;
                        let stem = std::path::Path::new(&file).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| file.clone());
                        let variant = format!("{stem}.stretch{r:.3}.opus");
                        let Some((sid, _)) = source_for(&file, &mut ops, &mut missing, Some((&variant, r))) else { continue };
                        if !stretches.iter().any(|(_, _, v)| *v == variant) { stretches.push((file.clone(), r, variant.clone())); }
                        if let Some(mode) = i(&c["stretch_mode"]) { stretch_modes.insert(ch_name(c), mode); }
                        let src_in = Ticks::from_seconds(off.max(0.0) / 1000.0 / r);
                        let len = len_t.min(Ticks::from_seconds(m.len.to_seconds() / r).saturating_sub(src_in));
                        if len > Ticks::ZERO { clips.push((sid, pos, src_in, len, channel_gain(c, Some(m)))); }
                    } else {
                    let src_in = if off > 0.0 { Ticks::from_seconds(off / 1000.0) } else { Ticks::ZERO };
                    let len = len_t.min(m.len.saturating_sub(src_in));
                    if len > Ticks::ZERO { clips.push((id, pos, src_in, len, channel_gain(c, Some(m)))); }
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
                    for (rp, from) in seq.iter() {
                        let part_at = at((*from as f64 * step_ticks) as i64);
                        if part_at >= len_t { continue; }
                        let part_len = at((rack.patterns[*rp].steps as f64 * step_ticks) as i64).min(len_t.saturating_sub(part_at));
                        let id = *pattern_sources.entry(*rp).or_insert_with(|| {
                            let id = SourceId(10_000 + *rp as u64);
                            ops.push(Op::AddSource { source: Source { id, path: String::new(), len: Ticks::ZERO, sample_rate: 48_000,
                                channels: 2, has_video: false, kind: SourceKind::Pattern { pattern: *rp, name: rack.patterns[*rp].name.clone() } } });
                            id
                        });
                        clips.push((id, pos.saturating_add(part_at), Ticks::ZERO, part_len, 1.0));
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
                let lane = match lanes.iter().position(|(_, spans)| spans.iter().all(|(a, b)| pos >= *b || pos.saturating_add(len) <= *a)) {
                    Some(l) => l,
                    None => {
                        if !lanes.is_empty() { overflowed += 1; }
                        let id = TrackId(next_track);
                        next_track += 1;
                        let nm = format!("{}{}", label.clone().unwrap_or_else(|| format!("FL track {fl_iid}")),
                                         if lanes.is_empty() { String::new() } else { format!(" ({})", lanes.len() + 1) });
                        ops.push(Op::AddTrack { track: Track { id, kind: TrackKind::Audio, name: nm, muted: off, locked: false,
                                                               gain: 1.0, route: None }, at: track_at });
                        track_at += 1;
                        lanes.push((id, Vec::new()));
                        lanes.len() - 1
                    }
                };
                lanes[lane].1.push((pos, pos.saturating_add(len)));
                ops.push(Op::AddClip { clip: Clip { id: ClipId(next_clip), source: src, track: lanes[lane].0, pos, src_in, len,
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
            for (tid, _) in &lanes { ops.push(Op::SetRoute { track: *tid, bus: Some(bus) }); }
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
    if declicked > 0 { report.push(format!("{declicked} audio clips declicked: {:.0} ms fades at each edge, as FL does by default", DECLICK * 1000.0)); }
    if automation_clips > 0 { report.push(format!("{automation_clips} automation clip{} not imported (their points are not read yet)", if automation_clips == 1 { "" } else { "s" })); }
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
    report.push("faders are FL's value / 12800 as linear gain: FL's fader law is not linear, so levels are close, not exact".into());
    if !missing.is_empty() { report.push(format!("media not found beside the project: {}", missing.join(", "))); }
    Ok(Imported { project: p, rack, report, stretches })
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
                {"iid": 2, "kind": "Instrument", "name": "Bass", "plugin": "Fruity Wrapper", "plugin_name": "Omnisphere", "insert": 0, "volume": 10000},
                {"iid": 3, "kind": "Sampler", "name": "Vox", "sample_file": "vox.wav", "insert": 2, "volume": 5000, "pan": 6400, "normalize": true},
                {"iid": 4, "kind": "Automation", "name": "Vox - Volume"}
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
                {"iid": 4, "name": "Loops", "items": [{"kind": "ChannelPLItem", "channel": 3, "pos": 0, "len": 1152, "offsets": [1000.0, 13000.0]}]},
                {"iid": 5, "name": "Beat x3", "items": [{"kind": "PatternPLItem", "pattern": 1, "pos": 0, "len": 1152}]}
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
        assert!(im.report.iter().any(|r| r.contains("1 automation clip not imported")), "{:?}", im.report);
        assert!(im.report.iter().any(|r| r.contains("overflow track")));
        assert!(vocals.iter().all(|t| t.muted), "a playlist track switched off in FL is muted, overflow lane too");
        assert!(p.tracks.iter().filter(|t| !t.name.starts_with("Vocals")).all(|t| !t.muted));
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
        assert!(matches!(im.rack.channels[omni].kind, SynthKind::Sine { .. }));
        let bass = im.rack.channels.iter().position(|c| c.name == "Bass").unwrap();
        assert!(matches!(im.rack.channels[bass].kind, SynthKind::Sub { .. }), "all notes below C4: a Sub stand-in");
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
    /// at half volume, NORMALIZE on. The clip carries 0.5 x 1/0.125 = 4.
    #[test]
    fn a_normalized_quiet_take_is_brought_up_as_fl_does_and_centre_pan_is_centre() {
        let im = import(&inventory(), "demo", &probe).unwrap();
        let vt = im.project.tracks.iter().find(|t| t.name == "Vocals").unwrap();
        let clip = &im.project.track_clips(vt.id)[0];
        assert!((clip.gain - 4.0).abs() < 1e-5, "volume 0.5 x normalize 8 = 4, got {}", clip.gain);
        assert_eq!(fl_pan(6400.0), 0.0);
        assert_eq!(fl_pan(0.0), -1.0);
        assert_eq!(fl_pan(12800.0), 1.0);
        let kick = im.rack.channels.iter().find(|c| c.name == "Kick").unwrap();
        assert!((kick.gain - 1.0).abs() < 1e-6, "not normalized: its volume alone");
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
        let b = im.project.tracks.iter().find(|t| t.name == "Beat x3").unwrap();
        let clips = im.project.track_clips(b.id);
        assert_eq!(clips.len(), 1, "one pass, not three: {clips:?}");
        assert!((clips[0].len.to_seconds() - 2.0).abs() < 1e-6, "the pattern's own length");
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
        assert_eq!((drums.sends[0].to, drums.sends[0].level), (verb.id, 0.5), "and sends to the reverb at half");
        assert_eq!(vox.parent, Some(group.id), "no master route: its first destination is its output");
        assert_eq!((vox.sends[0].to, vox.sends[0].level), (verb.id, 0.25));
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
        assert!((vox.gain - 0.5).abs() < 1e-6);
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
