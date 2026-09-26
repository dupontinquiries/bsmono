//! flp_read — FL Studio's `.flp`, read natively: no Python, runs on a phone.
//!
//! Produces the SAME inventory shape `tools/flp_dump.py` writes (so
//! `flp_import` takes either), plus what PyFLP gets wrong: the MIXER ROUTING.
//! Event ids and struct layouts are PyFLP's reverse-engineering (credited);
//! the grouping below is a small state machine over the flat event stream.
//!
//! ## The format
//!
//! `FLhd` (u32 6; u16 format, u16 channels, u16 PPQ), then `FLdt` (u32 size)
//! and events: a one-byte id, and a payload sized by the id's range --
//! 0..=63 one byte, 64..=127 two, 128..=191 four, 192..=255 a 7-bit varint
//! length then that many bytes (text is UTF-16LE in any FL this century).
//!
//! ## Grouping
//!
//! `ChannelID.New` (64) opens a channel, `PatternID.New` (65) a pattern,
//! `ArrangementID.New` (99) an arrangement whose `TrackID.Data` (238) events
//! follow its playlist (233). The first `InsertID.Flags` (236) starts the
//! mixer: from there plugin events (201 internal name, 203 name) belong to a
//! SLOT, closed by `SlotID.Index` (98), and `InsertID.Output` (147) closes an
//! INSERT. Inserts are numbered as FL numbers them: 0 is the master.
//!
//! ## Routing (where PyFLP is wrong)
//!
//! `InsertID.Routing` (235) is one flag per insert: flag k set = this insert
//! sends to insert k. The LEVEL of that send is in `MixerID.Params` (225), as
//! param id 64 + k. PyFLP pairs the flags with EVERY param id >= 64 -- fader
//! (192) and pan (193) included -- and so reports the insert's own fader as
//! its routes (seen on Blue Moon: routes == volume on every insert).

use serde_json::{json, Map, Value};

#[derive(Debug)]
pub struct FlpError(pub String);

struct Reader<'a> { b: &'a [u8], i: usize }

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8], FlpError> {
        if self.i + n > self.b.len() { return Err(FlpError(format!("truncated at byte {}", self.i))); }
        let s = &self.b[self.i..self.i + n];
        self.i += n;
        Ok(s)
    }
    fn varint(&mut self) -> Result<usize, FlpError> {
        let (mut v, mut shift) = (0usize, 0);
        loop {
            let b = self.take(1)?[0];
            v |= ((b & 0x7F) as usize) << shift;
            if b & 0x80 == 0 { return Ok(v); }
            shift += 7;
            if shift > 28 { return Err(FlpError("varint too long".into())); }
        }
    }
}

/// One event: its id, and either a number or bytes.
#[derive(Debug, Clone, PartialEq)]
pub enum Ev { Num(u8, u32), Data(u8, Vec<u8>) }

/// The header's PPQ and every event, in order.
pub fn events(b: &[u8]) -> Result<(u16, Vec<Ev>), FlpError> {
    let mut r = Reader { b, i: 0 };
    if r.take(4)? != b"FLhd" { return Err(FlpError("not an FL Studio project (no FLhd)".into())); }
    let hsize = u32::from_le_bytes(r.take(4)?.try_into().unwrap()) as usize;
    let h = r.take(hsize)?;
    if h.len() < 6 { return Err(FlpError("short header".into())); }
    let ppq = u16::from_le_bytes([h[4], h[5]]);
    if r.take(4)? != b"FLdt" { return Err(FlpError("no FLdt chunk".into())); }
    let size = u32::from_le_bytes(r.take(4)?.try_into().unwrap()) as usize;
    let end = (r.i + size).min(b.len());
    let mut out = Vec::new();
    while r.i < end {
        let id = r.take(1)?[0];
        out.push(match id {
            0..=63 => Ev::Num(id, r.take(1)?[0] as u32),
            64..=127 => { let x = r.take(2)?; Ev::Num(id, u16::from_le_bytes([x[0], x[1]]) as u32) }
            128..=191 => { let x = r.take(4)?; Ev::Num(id, u32::from_le_bytes([x[0], x[1], x[2], x[3]])) }
            _ => { let n = r.varint()?; Ev::Data(id, r.take(n)?.to_vec()) }
        });
    }
    Ok((ppq, out))
}

fn text(d: &[u8]) -> String {
    let u: Vec<u16> = d.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&u).trim_end_matches('\0').to_string()
}
fn u32at(d: &[u8], o: usize) -> Option<u32> { d.get(o..o + 4).map(|x| u32::from_le_bytes(x.try_into().unwrap())) }
fn i32at(d: &[u8], o: usize) -> Option<i32> { u32at(d, o).map(|v| v as i32) }
fn u16at(d: &[u8], o: usize) -> Option<u16> { d.get(o..o + 2).map(|x| u16::from_le_bytes([x[0], x[1]])) }
fn f32at(d: &[u8], o: usize) -> Option<f32> { u32at(d, o).map(f32::from_bits) }

/// The plugin's name from an install path inside a wrapper blob: the file
/// stem of the first ".vst3" / ".vst" / ".dll" / ".component" path found.
pub fn vst_name_from_path(d: &[u8]) -> Option<String> {
    let text: String = d.iter().map(|b| if b.is_ascii_graphic() || *b == b' ' { *b as char } else { '\n' }).collect();
    for run in text.split('\n') {
        for ext in [".vst3", ".vst", ".dll", ".component"] {
            if let Some(i) = run.find(ext) {
                let path = &run[..i];
                let stem = path.rsplit(['/', '\\']).next().unwrap_or(path).trim();
                if !stem.is_empty() { return Some(stem.to_string()); }
            }
        }
    }
    None
}

/// The inventory, in `flp_dump.py`'s shape (mixer inserts in FL numbering,
/// with real `routes`: `[{to, level}]`).
pub fn inventory(b: &[u8]) -> Result<Value, FlpError> {
    let (ppq, evs) = events(b)?;
    let mut proj = Map::new();
    proj.insert("ppq".into(), json!(ppq));
    let mut channels: Vec<Map<String, Value>> = Vec::new();
    let mut ch_type: Vec<u32> = Vec::new();
    let mut patterns: std::collections::BTreeMap<u32, Map<String, Value>> = Default::default();
    let mut cur_pat: Option<u32> = None;
    let mut arrangements: Vec<Map<String, Value>> = Vec::new();
    let mut playlists: Vec<Vec<u8>> = Vec::new();
    let mut tracks: Vec<Vec<Map<String, Value>>> = Vec::new();
    let mut mixer_on = false;
    let mut inserts: Vec<Map<String, Value>> = vec![Map::new()];
    let mut slots: Vec<Map<String, Value>> = Vec::new();
    let mut slot = Map::new();
    let mut routing: Vec<Vec<bool>> = vec![Vec::new()];
    let mut params: Vec<u8> = Vec::new();
    for e in &evs {
        match e {
            Ev::Num(156, v) => { proj.insert("tempo".into(), json!(*v as f64 / 1000.0)); }
            Ev::Data(194, d) => { proj.insert("title".into(), json!(text(d))); }
            Ev::Data(207, d) => { proj.insert("artists".into(), json!(text(d))); }
            Ev::Data(199, d) => { proj.insert("version".into(), json!(String::from_utf8_lossy(d).trim_end_matches('\0').to_string())); }
            Ev::Data(225, d) => params = d.clone(),
            // ── patterns ──
            Ev::Num(65, v) => { cur_pat = Some(*v); patterns.entry(*v).or_insert_with(|| { let mut m = Map::new(); m.insert("iid".into(), json!(v)); m }); }
            Ev::Data(193, d) if cur_pat.is_some() && !mixer_on => { patterns.get_mut(&cur_pat.unwrap()).unwrap().insert("name".into(), json!(text(d))); }
            Ev::Data(224, d) if cur_pat.is_some() => {
                let notes: Vec<Value> = d.chunks_exact(24).map(|n| json!({
                    "pos": u32at(n, 0), "slide": u16at(n, 4).map(|f| f & (1 << 3) != 0), "rack": u16at(n, 6),
                    "len": u32at(n, 8), "key": u16at(n, 12), "fine": n[16], "pan": n[20], "vel": n[21] })).collect();
                patterns.get_mut(&cur_pat.unwrap()).unwrap().insert("notes".into(), Value::Array(notes));
            }
            // ── channels ──
            Ev::Num(64, v) if !mixer_on => {
                let mut m = Map::new();
                m.insert("iid".into(), json!(v));
                channels.push(m);
                ch_type.push(0);
                cur_pat = None;
            }
            Ev::Num(21, v) if !mixer_on && !channels.is_empty() => { *ch_type.last_mut().unwrap() = *v; }
            Ev::Num(22, v) if !mixer_on && !channels.is_empty() => { channels.last_mut().unwrap().insert("insert".into(), json!(*v as u8 as i8)); }
            Ev::Num(0, v) if !mixer_on && !channels.is_empty() => { channels.last_mut().unwrap().insert("enabled".into(), json!(*v != 0)); }
            Ev::Data(192, d) | Ev::Data(203, d) if !mixer_on && !channels.is_empty() => {
                channels.last_mut().unwrap().insert("name".into(), json!(text(d)));
            }
            Ev::Data(201, d) if !mixer_on && !channels.is_empty() => { channels.last_mut().unwrap().insert("plugin".into(), json!(text(d))); }
            Ev::Data(196, d) if !mixer_on && !channels.is_empty() => {
                let p = text(d);
                let file = p.rsplit(['\\', '/']).next().unwrap_or(&p).to_string();
                let c = channels.last_mut().unwrap();
                c.insert("sample_path".into(), json!(p));
                if !file.is_empty() { c.insert("sample_file".into(), json!(file)); }
            }
            // ChannelID.Parameters: byte 80 is the sample's NORMALIZE switch --
            // on for Blue Moon's quiet female lead, which FL brings up ~19 dB
            // with it; reading it is the difference between her there or not.
            Ev::Data(215, d) if !mixer_on && !channels.is_empty() => {
                if let Some(n) = d.get(80) { channels.last_mut().unwrap().insert("normalize".into(), json!(*n != 0)); }
                // time-stretch mode (PyFLP: -1 stretch, 0 RESAMPLE -- the one
                // that shifts pitch -- 1.. the elastique modes; FL 20.7 also
                // writes 12, which PyFLP's list does not name)
                if let Some(m) = i32at(d, 112) { channels.last_mut().unwrap().insert("stretch_mode".into(), json!(m)); }
            }
            Ev::Data(219, d) if !mixer_on && !channels.is_empty() => {
                let c = channels.last_mut().unwrap();
                if let Some(p) = i32at(d, 0) { c.insert("pan".into(), json!(p)); }
                if let Some(v) = u32at(d, 4) { c.insert("volume".into(), json!(v)); }
            }
            // ── arrangements ──
            Ev::Num(99, v) => {
                let mut m = Map::new(); m.insert("iid".into(), json!(v));
                arrangements.push(m); playlists.push(Vec::new()); tracks.push(Vec::new());
            }
            Ev::Data(241, d) if !arrangements.is_empty() => { arrangements.last_mut().unwrap().insert("name".into(), json!(text(d))); }
            Ev::Data(233, d) if !playlists.is_empty() => *playlists.last_mut().unwrap() = d.clone(),
            Ev::Data(238, d) if !tracks.is_empty() => {
                let mut t = Map::new();
                t.insert("iid".into(), json!(u32at(d, 0)));
                t.insert("enabled".into(), json!(d.get(12).map(|b| *b != 0)));
                tracks.last_mut().unwrap().push(t);
            }
            Ev::Data(239, d) if !tracks.is_empty() && !mixer_on => {
                if let Some(t) = tracks.last_mut().unwrap().last_mut() { t.insert("name".into(), json!(text(d))); }
            }
            // ── mixer ──
            Ev::Data(236, _) => mixer_on = true,
            Ev::Data(201, d) if mixer_on => { slot.insert("plugin".into(), json!(text(d))); }
            Ev::Data(203, d) if mixer_on => { slot.insert("name".into(), json!(text(d))); }
            // Pitcher's state: an "SSLF" block; f32 at 12 is the knob read as
            // SPEED (inferred -- see flp_import::pitcher_params), and bytes
            // 144..156 are the twelve note switches, C first. Decoded from
            // Blue Moon's four Pitchers (all D major there).
            Ev::Data(213, d) if mixer_on && slot.get("plugin").and_then(Value::as_str) == Some("Pitcher") && d.len() >= 156 && &d[..4] == b"SSLF" => {
                slot.insert("pitcher".into(), json!({
                    "speed": f32at(d, 12).map(|x| x as f64),
                    "notes": d[144..156].iter().map(|b| *b != 0).collect::<Vec<bool>>(),
                }));
            }
            // A VST's wrapper state (212) or plugin data (213 -- where Vocal Doubler's
            // path turned out to be) carries its install path; when FL stored
            // no display name, the path's file stem names it ("…/Vocal Doubler.vst3"
            // was "a plugin" on all three of A Top A Tower's vocal chains).
            Ev::Data(212, d) | Ev::Data(213, d) if mixer_on && !slot.contains_key("name") && slot.get("plugin").and_then(Value::as_str) == Some("Fruity Wrapper") => {
                if let Some(n) = vst_name_from_path(d) { slot.insert("name".into(), json!(n)); }
            }
            Ev::Num(98, v) if mixer_on => {
                let mut s = std::mem::take(&mut slot);
                s.insert("index".into(), json!(v));
                if s.contains_key("plugin") || s.contains_key("name") { slots.push(s); }
            }
            Ev::Data(204, d) if mixer_on => { inserts.last_mut().unwrap().insert("name".into(), json!(text(d))); }
            Ev::Data(235, d) if mixer_on => { *routing.last_mut().unwrap() = d.iter().map(|b| *b != 0).collect(); }
            Ev::Num(147, _) if mixer_on => {
                inserts.last_mut().unwrap().insert("slots".into(), Value::Array(std::mem::take(&mut slots).into_iter().map(Value::Object).collect()));
                inserts.push(Map::new());
                routing.push(Vec::new());
            }
            _ => {}
        }
    }
    inserts.pop(); routing.pop(); // the empty one opened after the last Output
    // ── mixer params: fader, pan, send levels, slot enable ──
    let mut own: std::collections::BTreeMap<(usize, u8), i32> = Default::default();
    let mut slot_on: std::collections::BTreeMap<(usize, usize), bool> = Default::default();
    for p in params.chunks_exact(12) {
        let (id, cd, msg) = (p[4], u16::from_le_bytes([p[6], p[7]]) as usize, i32::from_le_bytes([p[8], p[9], p[10], p[11]]));
        let (ins, sl) = ((cd >> 6) & 0x7F, cd & 0x3F);
        if id == 0 { slot_on.insert((ins, sl), msg != 0); } else if id != 1 { own.insert((ins, id), msg); }
    }
    let mixer: Vec<Value> = inserts.into_iter().enumerate().map(|(k, mut m)| {
        m.insert("iid".into(), json!(k));
        m.insert("volume".into(), json!(own.get(&(k, 192)).copied().unwrap_or(12800)));
        m.insert("pan".into(), json!(own.get(&(k, 193)).copied().unwrap_or(0)));
        let routes: Vec<Value> = routing.get(k).into_iter().flatten().enumerate().filter(|(_, on)| **on)
            .map(|(to, _)| json!({"to": to, "level": own.get(&(k, 64 + to as u8)).copied().unwrap_or(12800)})).collect();
        m.insert("routes".into(), Value::Array(routes));
        // Whether the file said anything about this insert's routing: an
        // insert WITH routing data and no route out is silent in FL (a meter,
        // a sidechain tap); one without data defaults to the master.
        m.insert("routing_known".into(), json!(routing.get(k).is_some_and(|r| !r.is_empty())));
        if let Some(Value::Array(ss)) = m.get_mut("slots") {
            for s in ss.iter_mut() {
                let idx = s["index"].as_u64().unwrap_or(0) as usize;
                s.as_object_mut().unwrap().insert("enabled".into(), json!(slot_on.get(&(k, idx)).copied().unwrap_or(true)));
            }
        }
        Value::Object(m)
    }).collect();
    // ── channels: kind ──
    for (c, t) in channels.iter_mut().zip(&ch_type) {
        let plugin = c.get("plugin").and_then(Value::as_str).unwrap_or("").to_string();
        let kind = if *t == 5 { "Automation" } else if !plugin.is_empty() && plugin != "Fruity Wrapper" && c.contains_key("sample_file") && plugin != "Fruity slicer" { "Sampler" }
                   else if !plugin.is_empty() { "Instrument" } else if *t == 3 { "Layer" } else { "Sampler" };
        c.insert("kind".into(), json!(kind));
        if kind == "Instrument" {
            // a wrapped VST names itself in the channel's plugin-name event
            c.insert("plugin_name".into(), c.get("name").cloned().unwrap_or(Value::Null));
        }
    }
    // ── playlist items onto their tracks ──
    let arrs: Vec<Value> = arrangements.into_iter().zip(playlists).zip(tracks).map(|((mut a, pl), ts)| {
        let size = if !pl.is_empty() && pl.len() % 60 == 0 && pl.len() % 32 != 0 { 60 } else { 32 };
        let mut by_track: std::collections::BTreeMap<u32, Vec<Value>> = Default::default();
        for it in pl.chunks_exact(size) {
            let (base, idx) = (u16at(it, 4).unwrap_or(20480) as u32, u16at(it, 6).unwrap_or(0) as u32);
            let track = 500 - u16at(it, 12).unwrap_or(0) as u32; // stored reversed from 499; iids start at 1
            let mut v = json!({"pos": u32at(it, 0), "len": u32at(it, 8), "flags": u16at(it, 18),
                               "offsets": [f32at(it, 24).map(|x| x as f64).filter(|x| x.is_finite()), f32at(it, 28).map(|x| x as f64).filter(|x| x.is_finite())]});
            let o = v.as_object_mut().unwrap();
            if idx <= base { o.insert("kind".into(), json!("ChannelPLItem")); o.insert("channel".into(), json!(idx)); }
            else {
                o.insert("kind".into(), json!("PatternPLItem"));
                o.insert("pattern".into(), json!(idx - base));
                // A PATTERN clip's start/end are INTEGER TICKS in the same slots an
                // audio clip keeps float milliseconds (read as floats they are
                // denormals: 1536 ticks = 2.15e-42); -1 = none. A clip cut to start
                // mid-pattern -- the artist's chopped drum reuse in Balance, 275
                // such clips -- plays from there.
                let (s, e) = (i32at(it, 24).unwrap_or(-1), i32at(it, 28).unwrap_or(-1));
                if s >= 0 { o.insert("start_ticks".into(), json!(s)); }
                if e >= 0 { o.insert("end_ticks".into(), json!(e)); }
            }
            by_track.entry(track).or_default().push(v);
        }
        let tracks: Vec<Value> = ts.into_iter().filter_map(|mut t| {
            let iid = t.get("iid").and_then(Value::as_u64).unwrap_or(0) as u32;
            let items = by_track.remove(&iid).unwrap_or_default();
            if items.is_empty() && !t.contains_key("name") { return None; }
            t.insert("items".into(), Value::Array(items));
            Some(Value::Object(t))
        }).collect();
        a.insert("tracks".into(), Value::Array(tracks));
        Value::Object(a)
    }).collect();
    Ok(json!({
        "source": "flp_read (native)",
        "project": Value::Object(proj),
        "channels": channels.into_iter().map(Value::Object).collect::<Vec<_>>(),
        "patterns": patterns.into_values().filter(|p| p.contains_key("notes")).map(Value::Object).collect::<Vec<_>>(),
        "arrangements": arrs,
        "mixer": mixer,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev_num(id: u8, v: u32) -> Vec<u8> {
        let mut b = vec![id];
        match id { 0..=63 => b.push(v as u8), 64..=127 => b.extend((v as u16).to_le_bytes()), _ => b.extend(v.to_le_bytes()) }
        b
    }
    fn ev_data(id: u8, d: &[u8]) -> Vec<u8> {
        let mut b = vec![id];
        let mut n = d.len();
        loop { let mut x = (n & 0x7F) as u8; n >>= 7; if n > 0 { x |= 0x80; } b.push(x); if n == 0 { break; } }
        b.extend_from_slice(d);
        b
    }
    fn utf16(s: &str) -> Vec<u8> { s.encode_utf16().chain([0]).flat_map(|c| c.to_le_bytes()).collect() }
    fn file(evs: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = evs.concat();
        let mut f = b"FLhd".to_vec();
        f.extend(6u32.to_le_bytes()); f.extend(0u16.to_le_bytes()); f.extend(3u16.to_le_bytes()); f.extend(96u16.to_le_bytes());
        f.extend(b"FLdt"); f.extend((body.len() as u32).to_le_bytes()); f.extend(body);
        f
    }
    fn param(id: u8, ins: u16, slot: u16, msg: i32) -> Vec<u8> {
        let mut p = vec![0u8; 4]; p.push(id); p.push(0); p.extend(((ins << 6) | slot).to_le_bytes()); p.extend(msg.to_le_bytes()); p
    }
    fn note(pos: u32, rack: u16, len: u32, key: u16, vel: u8) -> Vec<u8> {
        let mut n = Vec::new();
        n.extend(pos.to_le_bytes()); n.extend(0u16.to_le_bytes()); n.extend(rack.to_le_bytes()); n.extend(len.to_le_bytes());
        n.extend(key.to_le_bytes()); n.extend(0u16.to_le_bytes()); n.extend([120, 0, 64, 0, 64, vel, 128, 128]);
        n
    }
    fn item(pos: u32, idx: u16, len: u32, track: u32, a: f32, b: f32) -> Vec<u8> {
        let mut i = Vec::new();
        i.extend(pos.to_le_bytes()); i.extend(20480u16.to_le_bytes()); i.extend(idx.to_le_bytes()); i.extend(len.to_le_bytes());
        i.extend(((500 - track) as u16).to_le_bytes()); i.extend(0u16.to_le_bytes()); i.extend([120, 0]); i.extend(64u16.to_le_bytes());
        i.extend([64, 100, 128, 128]); i.extend(a.to_le_bytes()); i.extend(b.to_le_bytes());
        i
    }

    /// A tiny project, event by event: tempo, a pattern, a sampler routed to
    /// insert 1, a playlist, and a mixer where insert 1 sends to insert 2 at
    /// half level and insert 2 to the master.
    fn tiny() -> Vec<u8> {
        let mut track = vec![0u8; 13]; track[0] = 1; track[12] = 1;
        let mut track2 = vec![0u8; 13]; track2[0] = 2; track2[12] = 0;
        let params = [param(192, 1, 0, 9000), param(193, 1, 0, -200), param(64 + 2, 1, 0, 6400), param(192, 2, 0, 12800),
                      param(0, 2, 0, 0)].concat();
        file(&[
            ev_num(156, 140_000), ev_data(194, &utf16("Tiny")),
            ev_num(65, 1), ev_data(193, &utf16("Beat")), ev_data(224, &[note(0, 0, 24, 60, 100), note(48, 0, 24, 62, 80)].concat()),
            ev_num(64, 0), ev_num(21, 0), ev_data(203, &utf16("Kick")), ev_data(196, &utf16("C:\\x\\Kick.wav")), ev_num(22, 1),
            ev_data(219, &[(-100i32).to_le_bytes(), 8000u32.to_le_bytes()].concat()),
            ev_num(99, 0), ev_data(233, &[item(0, 20481, 384, 1, f32::NAN, f32::NAN), item(96, 0, 96, 2, 500.0, 1500.0)].concat()),
            ev_data(238, &track), ev_data(239, &utf16("Drums")), ev_data(238, &track2),
            ev_data(225, &params),
            // master
            ev_data(236, &[0; 12]), ev_data(235, &[0, 0, 0]), ev_num(147, 0),
            // insert 1: an EQ in slot 0, routed to insert 2
            ev_data(236, &[0; 12]), ev_data(201, &utf16("Fruity Parametric EQ 2")), ev_num(98, 0), ev_data(204, &utf16("Kick bus")),
            ev_data(235, &[0, 0, 1]), ev_num(147, 0),
            // insert 2: a limiter, disabled, routed to master
            ev_data(236, &[0; 12]), ev_data(201, &utf16("Fruity Limiter")), ev_num(98, 0), ev_data(204, &utf16("Group")),
            ev_data(235, &[1, 0, 0]), ev_num(147, 0),
        ])
    }

    #[test]
    fn a_small_project_reads_into_the_dumps_shape() {
        let inv = inventory(&tiny()).unwrap();
        assert_eq!(inv["project"]["tempo"], json!(140.0));
        assert_eq!(inv["project"]["ppq"], json!(96));
        assert_eq!(inv["project"]["title"], json!("Tiny"));
        let c = &inv["channels"][0];
        assert_eq!((c["name"].as_str(), c["kind"].as_str(), c["sample_file"].as_str(), c["insert"].as_i64()),
                   (Some("Kick"), Some("Sampler"), Some("Kick.wav"), Some(1)));
        assert_eq!(c["volume"], json!(8000));
        let p = &inv["patterns"][0];
        assert_eq!(p["name"], json!("Beat"));
        assert_eq!(p["notes"][1], json!({"pos": 48, "slide": false, "rack": 0, "len": 24, "key": 62, "fine": 120, "pan": 64, "vel": 80}));
        let tr = &inv["arrangements"][0]["tracks"];
        assert_eq!(tr[0]["name"], json!("Drums"));
        assert_eq!(tr[0]["items"][0]["kind"], json!("PatternPLItem"));
        assert_eq!(tr[0]["items"][0]["pattern"], json!(1));
        assert_eq!(tr[0]["items"][0]["offsets"], json!([null, null]), "NaN is no offset: null");
        assert_eq!(tr[1]["enabled"], json!(false));
        assert_eq!(tr[1]["items"][0]["channel"], json!(0));
        assert_eq!(tr[1]["items"][0]["offsets"], json!([500.0, 1500.0]));
    }

    #[test]
    fn routing_is_the_flags_with_their_own_send_levels_not_the_fader() {
        let inv = inventory(&tiny()).unwrap();
        let m = inv["mixer"].as_array().unwrap();
        assert_eq!(m.len(), 3, "master + 2");
        assert_eq!(m[1]["name"], json!("Kick bus"));
        assert_eq!(m[1]["volume"], json!(9000));
        assert_eq!(m[1]["pan"], json!(-200));
        assert_eq!(m[1]["routes"], json!([{"to": 2, "level": 6400}]), "the send, at its level -- not the 9000 fader");
        assert_eq!(m[2]["routes"], json!([{"to": 0, "level": 12800}]));
        assert_eq!(m[1]["slots"][0]["plugin"], json!("Fruity Parametric EQ 2"));
        assert_eq!(m[2]["slots"][0]["enabled"], json!(false), "slot 0 of insert 2 switched off in the params");
    }

    #[test]
    fn a_vst_with_no_display_name_is_named_by_its_install_path() {
        let mut blob = vec![1u8, 0, 0, 0, 57, 0];
        blob.extend(b"/Library/Audio/Plug-Ins/VST3/Vocal Doubler.vst3");
        blob.extend([0, 8, 0, 0]);
        assert_eq!(vst_name_from_path(&blob).as_deref(), Some("Vocal Doubler"));
        assert_eq!(vst_name_from_path(b"C:\\VST\\Serum_x64.dll\0").as_deref(), Some("Serum_x64"));
        assert_eq!(vst_name_from_path(b"no path here"), None);
    }

    #[test]
    fn a_file_that_is_not_an_flp_says_so() {
        assert!(inventory(b"RIFF....WAVE").unwrap_err().0.contains("FLhd"));
        let mut t = tiny(); t.truncate(t.len() - 3);
        assert!(inventory(&t).is_err(), "a truncated event is an error, not a silent stop");
    }
}
