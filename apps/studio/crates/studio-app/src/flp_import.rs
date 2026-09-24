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
//! * **Mixer**: each FL insert a channel uses becomes a bus with its name and
//!   fader; its effect slots map to Studio effects where there is one, at
//!   Studio's DEFAULT settings (PyFLP does not decode plugin parameters), and
//!   the rest are named, not faked. Routing between inserts is not imported
//!   (PyFLP's `routes` repeat the insert's own fader: not trustworthy).
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
            keys.entry(r).or_default().push(i(&n["key"]).unwrap_or(60));
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
        let plugin = s(&c["plugin_name"]).or(s(&c["plugin"])).unwrap_or("").to_string();
        let kind = if kind_s == "Sampler" || plugin == "Fruity slicer" {
            match file_of(c).and_then(|f| source_for(&f, &mut ops, &mut missing)) {
                Some((id, _)) => {
                    if plugin == "Fruity slicer" {
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
        rack.channels.push(Channel { name: ch_name(c), kind, gain: (vol / 10000.0).clamp(0.0, 1.28) as f32,
                                     pan: 0.0, muted: false, choke_group: None });
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
                let key = i(&n["key"]).unwrap_or(60);
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
    let mut bus_for = |insert: i64, ops: &mut Vec<Op>| -> Option<BusId> {
        if insert <= 0 { return None; } // 0 is FL's master: route to Studio's master
        let m = mixer.get(&insert)?;
        let n = busses.len() as u64 + 1;
        Some(*busses.entry(insert).or_insert_with(|| {
            let id = BusId(n);
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
            ops.push(Op::AddBus { bus: Bus { id, name, parent: None, gain, chain, sends: Vec::new(), muted: false,
                                             silent: false } });
            id
        }))
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
    if automation_clips > 0 { report.push(format!("{automation_clips} automation clip{} not imported (PyFLP does not give their points)", if automation_clips == 1 { "" } else { "s" })); }
    if !approx.is_empty() { report.push(format!("approximate effects: {}", approx.join("; "))); }
    report.push("every mapped effect is at Studio's DEFAULT settings -- the plugin parameters are not decoded".into());
    if !unmapped.is_empty() {
        report.push(format!("effects with no Studio counterpart, left out: {}",
            unmapped.iter().map(|(k, n)| if *n > 1 { format!("{k} x{n}") } else { k.clone() }).collect::<Vec<_>>().join(", ")));
    }
    if let Some(master) = mixer.get(&-1) {
        let slots: Vec<&str> = master["slots"].as_array().into_iter().flatten().filter_map(|x| s(&x["name"]).or(s(&x["plugin"]))).collect();
        if !slots.is_empty() { report.push(format!("master chain not imported: {}", slots.join(" -> "))); }
    }
    report.push("routing between inserts not imported: every bus goes to master".into());
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
                    {"rack": 1, "pos": 0, "len": 96, "key": 67, "vel": 64},
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
                {"iid": -1, "slots": [{"plugin": "Fruity Limiter"}, {"plugin": "Fruity Wrapper", "name": "Ozone 8"}]},
                {"iid": 1, "name": "Drums", "volume": 12800, "slots": [{"plugin": "Fruity Parametric EQ 2"}, {"plugin": "Soundgoodizer"}]},
                {"iid": 2, "name": "Vox bus", "volume": 6400, "slots": [{"plugin": "Pitcher"}, {"plugin": "Fruity Wrapper", "name": "Neutron 3"}, {"plugin": "Fruity Chorus"}]}
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
