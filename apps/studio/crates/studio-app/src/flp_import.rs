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
pub struct Media { pub len: Ticks, pub rate: u32, pub channels: u16 }

pub struct Imported {
    pub project: Project,
    pub rack: Rack,
    /// Everything approximated, skipped or refused, grouped, for a person.
    pub report: Vec<String>,
}

/// Steps per pattern at most (the rack rows are a u64 bitmask).
const MAX_STEPS: u32 = 64;

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
    let mut source_for = |file: &str, ops: &mut Vec<Op>, missing: &mut Vec<String>| -> Option<(SourceId, Media)> {
        let m = probe(file);
        let Some(m) = m else { if !missing.contains(&file.to_string()) { missing.push(file.to_string()); } return None };
        let id = *sources.entry(file.to_string()).or_insert_with(|| {
            let id = SourceId(next_source);
            next_source += 1;
            ops.push(Op::AddSource { source: Source { id, path: file.to_string(), len: m.len, sample_rate: m.rate,
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
        let kind = if kind_s == "Sampler" || internal == "Fruity slicer" {
            match file_of(c).and_then(|f| source_for(&f, &mut ops, &mut missing)) {
                Some((id, _)) => {
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
        let vol = f(&c["volume"]).unwrap_or(10000.0);
        rack_ix.insert(*fl, rack.channels.len());
        let pan = (f(&c["pan"]).unwrap_or(0.0) / 6400.0).clamp(-1.0, 1.0) as f32;
        rack.channels.push(Channel { name: ch_name(c), kind, gain: (vol / 10000.0).clamp(0.0, 1.28) as f32,
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
    // Create the bus for `insert` -- its destinations FIRST, so a parent
    // always exists when its child is added. FL routing has no cycles; a
    // depth guard makes a corrupt file an error message, not a stack overflow.
    fn make(insert: i64, depth: usize, mixer: &BTreeMap<i64, &Value>, busses: &mut BTreeMap<i64, BusId>, ops: &mut Vec<Op>,
            unmapped: &mut BTreeMap<String, usize>, approx: &mut Vec<String>, sends_made: &mut usize, unroutable: &mut usize) -> Option<BusId> {
        if insert <= 0 || depth > 32 { return None; } // 0 is FL's master: Studio's master
        if let Some(b) = busses.get(&insert) { return Some(*b); }
        let m = *mixer.get(&insert)?;
        let routes: Vec<(i64, f32)> = m["routes"].as_array().into_iter().flatten()
            .filter_map(|r| Some((i(&r["to"])?, (f(&r["level"]).unwrap_or(12800.0) / 12800.0) as f32))).collect();
        let to_master = routes.is_empty() || routes.iter().any(|(t, _)| *t == 0);
        let mut dests: Vec<(BusId, f32)> = Vec::new();
        for (t, lvl) in routes.iter().filter(|(t, _)| *t > 0) {
            match make(*t, depth + 1, mixer, busses, ops, unmapped, approx, sends_made, unroutable) {
                Some(b) => dests.push((b, *lvl)),
                None => *unroutable += 1,
            }
        }
        let (parent, sends): (Option<BusId>, Vec<(BusId, f32)>) = if to_master || dests.is_empty() { (None, dests) }
            else { (Some(dests[0].0), dests[1..].to_vec()) };
        *sends_made += sends.len();
        let id = BusId(busses.len() as u64 + 1);
        busses.insert(insert, id);
        let mut chain = Vec::new();
        for slot in m["slots"].as_array().into_iter().flatten() {
            let plug = s(&slot["plugin"]).unwrap_or("");
            let label = if plug == "Fruity Wrapper" { s(&slot["name"]).unwrap_or("a plugin") } else { plug };
            match effect_for(plug) {
                Some((kind, how)) => {
                    if how != "exact kind" && !approx.iter().any(|a| a.starts_with(plug)) { approx.push(format!("{plug}: {how}")); }
                    chain.push(Effect { kind: kind.into(), params: BTreeMap::new(), enabled: slot["enabled"].as_bool().unwrap_or(true) });
                }
                None => *unmapped.entry(label.to_string()).or_default() += 1,
            }
        }
        let name = s(&m["name"]).map(str::to_string).unwrap_or_else(|| format!("Insert {insert}"));
        let gain = (f(&m["volume"]).unwrap_or(12800.0) / 12800.0) as f32;
        ops.push(Op::AddBus { bus: Bus { id, name, parent, gain, chain,
            sends: sends.into_iter().map(|(to, level)| studio_core::Send { to, level, pre_fader: false }).collect(),
            muted: false, silent: false } });
        Some(id)
    }
    let mut bus_for = |insert: i64, ops: &mut Vec<Op>| -> Option<BusId> {
        make(insert, 0, &mixer, &mut busses, ops, &mut unmapped, &mut approx, &mut sends_made, &mut unroutable)
    };

    // ── the playlist ────────────────────────────────────────────────────
    let arr = inv["arrangements"].as_array().and_then(|a| a.first()).ok_or("no arrangement")?;
    let mut next_track = 1u64;
    let mut next_clip = 1u64;
    let mut pattern_sources: BTreeMap<usize, SourceId> = BTreeMap::new();
    let (mut automation_clips, mut placed, mut overflowed) = (0usize, 0usize, 0usize);
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
            let mut clips: Vec<(SourceId, Ticks, Ticks, Ticks)> = Vec::new(); // (source, at, src_in, len)
            match s(&it["kind"]) {
                Some("ChannelPLItem") => {
                    let Some(c) = i(&it["channel"]).and_then(|c| chans.get(&c)) else { continue };
                    if s(&c["kind"]) == Some("Automation") { automation_clips += 1; continue; }
                    let Some(file) = file_of(c) else { continue };
                    let Some((id, m)) = source_for(&file, &mut ops, &mut missing) else { continue };
                    let off = it["offsets"].as_array().and_then(|o| o.first()).and_then(|v| v.as_f64()).unwrap_or(-1.0);
                    let src_in = if off > 0.0 { Ticks::from_seconds(off / 1000.0) } else { Ticks::ZERO };
                    let len = len_t.min(m.len.saturating_sub(src_in));
                    if len > Ticks::ZERO { clips.push((id, pos, src_in, len)); }
                    first_insert = first_insert.or(i(&c["insert"]));
                    label.get_or_insert_with(|| ch_name(c));
                }
                Some("PatternPLItem") => {
                    let Some(pid) = i(&it["pattern"]) else { continue };
                    for (rp, from) in parts.get(&pid).into_iter().flatten() {
                        let part_at = at((*from as f64 * step_ticks) as i64);
                        if part_at >= len_t { continue; }
                        let part_len = at((rack.patterns[*rp].steps as f64 * step_ticks) as i64).min(len_t.saturating_sub(part_at));
                        let id = *pattern_sources.entry(*rp).or_insert_with(|| {
                            let id = SourceId(10_000 + *rp as u64);
                            ops.push(Op::AddSource { source: Source { id, path: String::new(), len: Ticks::ZERO, sample_rate: 48_000,
                                channels: 2, has_video: false, kind: SourceKind::Pattern { pattern: *rp, name: rack.patterns[*rp].name.clone() } } });
                            id
                        });
                        clips.push((id, pos.saturating_add(part_at), Ticks::ZERO, part_len));
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
            for (src, pos, src_in, len) in clips {
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
                                                    gain: 1.0, effects: vec![] } });
                next_clip += 1;
                placed += 1;
            }
        }
        if let Some(ins) = first_insert {
            if let Some(bus) = bus_for(ins, &mut ops) {
                for (tid, _) in &lanes { ops.push(Op::SetRoute { track: *tid, bus: Some(bus) }); }
            }
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
    if automation_clips > 0 { report.push(format!("{automation_clips} automation clip{} not imported (their points are not read yet)", if automation_clips == 1 { "" } else { "s" })); }
    if !approx.is_empty() { report.push(format!("approximate effects: {}", approx.join("; "))); }
    report.push("every mapped effect is at Studio's DEFAULT settings -- the plugin parameters are not decoded".into());
    if !unmapped.is_empty() {
        report.push(format!("effects with no Studio counterpart, left out: {}",
            unmapped.iter().map(|(k, n)| if *n > 1 { format!("{k} x{n}") } else { k.clone() }).collect::<Vec<_>>().join(", ")));
    }
    if let Some(master) = mixer.get(&0) {
        let slots: Vec<&str> = master["slots"].as_array().into_iter().flatten().filter_map(|x| s(&x["name"]).or(s(&x["plugin"]))).collect();
        if !slots.is_empty() { report.push(format!("master chain not imported: {}", slots.join(" -> "))); }
    }
    if has_routes {
        let parented = p.busses.values().filter(|b| b.parent.is_some()).count();
        report.push(format!("routing: {parented} bus{} output into another bus, {sends_made} send{} at FL's send levels; the rest feed the master{}",
            if parented == 1 { "" } else { "ses" }, if sends_made == 1 { "" } else { "s" },
            if unroutable > 0 { format!(" ({unroutable} route{} to an insert not in the file, left out)", if unroutable == 1 { "" } else { "s" }) } else { String::new() }));
    } else {
        report.push("no routing in this inventory (PyFLP's is not trustworthy -- use the native reader): every bus feeds the master".into());
    }
    let panned = mixer.values().filter(|m| f(&m["pan"]).is_some_and(|p| p != 0.0) && busses.contains_key(&i(&m["iid"]).unwrap_or(-1))).count();
    if panned > 0 { report.push(format!("{panned} insert pan{} not imported (Studio busses have no pan)", if panned == 1 { "" } else { "s" })); }
    report.push("faders are FL's value / 12800 as linear gain: FL's fader law is not linear, so levels are close, not exact".into());
    if !missing.is_empty() { report.push(format!("media not found beside the project: {}", missing.join(", "))); }
    Ok(Imported { project: p, rack, report })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn probe(file: &str) -> Option<Media> {
        match file {
            "kick.wav" => Some(Media { len: Ticks::from_seconds(0.4), rate: 44_100, channels: 1 }),
            "vox.wav" => Some(Media { len: Ticks::from_seconds(30.0), rate: 48_000, channels: 2 }),
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
                {"iid": 3, "kind": "Sampler", "name": "Vox", "sample_file": "vox.wav", "insert": 2},
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
                {"iid": 3, "items": [{"kind": "PatternPLItem", "pattern": 2, "pos": 0, "len": 1728}]}
            ]}],
            "mixer": [
                {"iid": 0, "slots": [{"plugin": "Fruity Limiter"}, {"plugin": "Fruity Wrapper", "name": "Ozone 8"}]},
                {"iid": 1, "name": "Drums", "volume": 12800, "routes": [{"to": 0, "level": 12800}, {"to": 3, "level": 6400}],
                 "slots": [{"plugin": "Fruity Parametric EQ 2"}, {"plugin": "Soundgoodizer"}]},
                {"iid": 2, "name": "Vox bus", "volume": 6400, "pan": -800, "routes": [{"to": 4, "level": 12800}, {"to": 3, "level": 3200}],
                 "slots": [{"plugin": "Pitcher"}, {"plugin": "Fruity Wrapper", "name": "Neutron 3"}, {"plugin": "Fruity Chorus"}]},
                {"iid": 3, "name": "Verb send", "routes": [{"to": 0, "level": 12800}], "slots": [{"plugin": "Fruity Reeverb 2"}]},
                {"iid": 4, "name": "Vocal group", "routes": [{"to": 0, "level": 12800}]}
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
        assert_eq!(drums.parent, None, "feeds the master");
        assert_eq!(drums.sends.len(), 1);
        assert_eq!((drums.sends[0].to, drums.sends[0].level), (verb.id, 0.5), "and sends to the reverb at half");
        assert_eq!(vox.parent, Some(group.id), "no master route: its first destination is its output");
        assert_eq!((vox.sends[0].to, vox.sends[0].level), (verb.id, 0.25));
        assert_eq!(group.parent, None);
        assert_eq!(verb.chain[0].kind, "reverb", "a bus nothing but routing reaches is still made");
        let r = im.report.join("\n");
        assert!(r.contains("routing: 1 bus output into another bus, 2 sends"), "{r}");
        assert!(r.contains("1 insert pan not imported"), "{r}");
    }

    #[test]
    fn inserts_become_busses_with_mapped_effects_and_the_rest_is_named() {
        let im = import(&inventory(), "demo", &probe).unwrap();
        let p = &im.project;
        let vox = p.busses.values().find(|b| b.name == "Vox bus").expect("a bus per used insert");
        assert!((vox.gain - 0.5).abs() < 1e-6);
        assert_eq!(vox.chain.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(), vec!["autotune"]);
        let drums = p.busses.values().find(|b| b.name == "Drums").unwrap();
        assert_eq!(drums.chain.iter().map(|e| e.kind.as_str()).collect::<Vec<_>>(), vec!["eq", "multiband"]);
        let vt = p.tracks.iter().find(|t| t.name == "Vocals").unwrap();
        assert_eq!(vt.route, Some(vox.id));
        let r = im.report.join("\n");
        assert!(r.contains("Neutron 3") && r.contains("Fruity Chorus"), "{r}");
        assert!(r.contains("Soundgoodizer: approximate"), "{r}");
        assert!(r.contains("master chain not imported: Fruity Limiter -> Ozone 8"), "{r}");
        assert!(!r.contains("refused by the model"), "{r}");
    }
}
