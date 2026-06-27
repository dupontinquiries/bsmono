// src/channel_routing.rs
#![allow(dead_code)]
//
// Channel routing: maps separated stems to named virtual channels,
// and virtual channels to playback device URLs. Sits between
// stream_separator (which produces stems) and bo-mux (which serves
// channels over HTTP and the network mesh handles per-device sync).
//
// The user's typical config: 3-4 devices in different rooms, each
// playing one stem (drums in one room, vocals in another, etc.).
// But we also support more elaborate mixes — e.g. "front" = voice +
// 0.6 × drums, sent to the device with the soundbar; "ambient" =
// piano + synth, sent to the bookshelf pair.
//
// Why a separate module: separation produces stems with fixed names
// (drums/voice/piano/synth); routing decides what to DO with them
// per-device. Same separation, different routing per setup → a
// pure config layer makes the latter swappable without re-running
// the former.
//
// Routing JSON shape (what we persist via the KV store):
//   {
//     "channels": [
//       { "name": "front",   "stems": [["voice",1.0],["drums",0.6]] },
//       { "name": "ambient", "stems": [["piano",1.0],["synth",1.0]] }
//     ],
//     "devices": {
//       "http://192.168.2.12": ["front"],
//       "http://192.168.2.14": ["ambient"]
//     }
//   }

use std::collections::HashMap;
use crate::stream_separator::{Stem, StemBlock};

/// One named virtual channel: a weighted mix of one or more stems.
/// `name` is the channel identifier exposed over the bo-mux HTTP
/// surface as `/ch/<name>`. `stems` lists the stems contributing
/// and their gains; gains are in linear amplitude (1.0 = unity,
/// 0.0 = silent, no upper bound but ≥ 4.0 is impractical).
#[derive(Debug, Clone, PartialEq)]
pub struct VirtualChannel {
    pub name: String,
    pub stems: Vec<(Stem, f32)>,
}

impl VirtualChannel {
    /// Create a channel containing exactly one stem at unity gain.
    /// Convenience for the simple "device N plays stem X" case.
    pub fn solo(name: impl Into<String>, stem: Stem) -> Self {
        Self { name: name.into(), stems: vec![(stem, 1.0)] }
    }

    /// Mix this channel from a block of separated stems.
    /// Returns a Vec the same length as one stem block. Per sample:
    ///   out[i] = Σ stem[i] × gain
    /// No clipping or limiting — caller can apply if needed.
    pub fn mix(&self, stems: &StemBlock) -> Vec<f32> {
        let n = stems.len();
        let mut out = vec![0.0_f32; n];
        for (stem, gain) in &self.stems {
            // Defensive: skip non-finite gain rather than poisoning
            // the mix. Use stable gain values for normal operation.
            let g = if gain.is_finite() { *gain } else { 0.0 };
            if g == 0.0 { continue; }
            let src = match stem {
                Stem::Drums => &stems.drums,
                Stem::Voice => &stems.voice,
                Stem::Piano => &stems.piano,
                Stem::Synth => &stems.synth,
            };
            // If a stem block is shorter than expected (warmup edge
            // case), mix only the overlap region.
            let m = src.len().min(n);
            for i in 0..m { out[i] += src[i] * g; }
        }
        out
    }
}

/// Equal-power pan law. Maps a pan position in [-1, +1] (left→right,
/// 0 = center) to (left_gain, right_gain) such that left²+right² == 1
/// at every position — constant perceived loudness across the arc, the
/// standard choice for placing a source in the stereo field. At center
/// both gains are √½ ≈ 0.707 (the −3 dB pan-center convention), at hard
/// left (−1) it's (1, 0), at hard right (+1) it's (0, 1). NaN/out-of-
/// range pans are clamped so a bad value can't blow up the field.
pub fn equal_power_pan(pan: f32) -> (f32, f32) {
    let p = if pan.is_finite() { pan.clamp(-1.0, 1.0) } else { 0.0 };
    // Map [-1,+1] → angle [0, π/2]. 0 → π/4 (center).
    let theta = (p + 1.0) * 0.5 * std::f32::consts::FRAC_PI_2;
    (theta.cos(), theta.sin())
}

/// One stem placed in the stereo field: which stem, its gain, and its
/// pan position (-1 left … 0 center … +1 right).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StemPlacement {
    pub stem: Stem,
    pub gain: f32,
    pub pan:  f32,
}

/// A stereo virtual channel — the spatializer for separated stems. Where
/// `VirtualChannel` sums stems into a MONO channel (gain only), this
/// places each stem in the stereo field with an equal-power pan, so a
/// four-stem separation can be spread out: e.g. drums center, lead vocal
/// center, piano panned left, synth panned right. Output is interleaved
/// stereo (L, R pairs).
///
/// `width` post-scales the resulting Side (difference) component using
/// the same M/S trick as the realtime mixer's spatializer, so the whole
/// placed field can be widened or narrowed as a unit on top of the
/// per-stem pans. width 1.0 = pans as-authored; 0 = mono; up to 2.0.
#[derive(Debug, Clone, PartialEq)]
pub struct StereoStemChannel {
    pub name: String,
    pub placements: Vec<StemPlacement>,
    pub width: f32,
}

impl StereoStemChannel {
    /// A channel with the four stems spread across a natural default
    /// stage: rhythm section anchored center, harmonic instruments out
    /// to the sides. A sensible starting point the user can then tweak.
    pub fn default_stage(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            placements: vec![
                StemPlacement { stem: Stem::Drums, gain: 1.0, pan:  0.0 },
                StemPlacement { stem: Stem::Voice, gain: 1.0, pan:  0.0 },
                StemPlacement { stem: Stem::Piano, gain: 1.0, pan: -0.6 },
                StemPlacement { stem: Stem::Synth, gain: 1.0, pan:  0.6 },
            ],
            width: 1.0,
        }
    }

    /// Mix + spatialize into interleaved stereo. Returns a Vec of
    /// (left, right) pairs, one per input sample. Pure + allocation-per-
    /// call (mirrors VirtualChannel::mix; a smoothed variant can follow
    /// the ChannelGainSmoother pattern if click-free pan moves are
    /// needed). No clipping — the caller limits if required.
    pub fn mix(&self, stems: &StemBlock) -> Vec<(f32, f32)> {
        let n = stems.len();
        let mut out = vec![(0.0_f32, 0.0_f32); n];
        for p in &self.placements {
            let g = if p.gain.is_finite() { p.gain } else { 0.0 };
            if g == 0.0 { continue; }
            let (lg, rg) = equal_power_pan(p.pan);
            let src = match p.stem {
                Stem::Drums => &stems.drums,
                Stem::Voice => &stems.voice,
                Stem::Piano => &stems.piano,
                Stem::Synth => &stems.synth,
            };
            let m = src.len().min(n);
            for i in 0..m {
                let s = src[i] * g;
                out[i].0 += s * lg;
                out[i].1 += s * rg;
            }
        }
        // Apply the channel-wide width via M/S, matching the realtime
        // mixer's spatializer semantics.
        let w = if self.width.is_finite() { self.width.clamp(0.0, 2.0) } else { 1.0 };
        if (w - 1.0).abs() > f32::EPSILON {
            for frame in out.iter_mut() {
                let mid  = (frame.0 + frame.1) * 0.5;
                let side = (frame.0 - frame.1) * 0.5 * w;
                frame.0 = mid + side;
                frame.1 = mid - side;
            }
        }
        out
    }
}

/// Stateful per-channel gain smoother. Avoids audible clicks when
/// the user changes a stem gain mid-playback.
///
/// Pattern borrowed from Surge's `LipolPs` (linear-interpolated
/// parameter smoother): hold the gain the last block actually
/// applied (`current_gain`), each call ramp from that toward the
/// target gain over `RAMP_SAMPLES` samples. After the ramp
/// completes the rest of the block uses the target verbatim. The
/// caller treats this as a drop-in replacement for `VirtualChannel
/// ::mix` once they've allocated a smoother for the channel.
///
/// Why it matters: stems summed at static gain produce a
/// continuous waveform; if the gain jumps mid-sample the next
/// sample is discontinuous and produces an audible pop, scaled
/// by the gain delta. A 256-sample ramp at 44.1 kHz is ~5.8 ms,
/// imperceptible to the listener as a slope but enough to elide
/// the click.
#[derive(Debug, Clone, Default)]
pub struct ChannelGainSmoother {
    /// Per-stem current gain, indexed by stem enum's discriminant
    /// (0=Drums, 1=Voice, 2=Piano, 3=Synth). One smoother covers
    /// one channel; the caller allocates one per channel.
    current: [f32; 4],
    /// Whether we've ever applied gain (first call has nothing to
    /// ramp from; we initialize current = target to avoid an
    /// unnecessary fade-in from 0 on the first block).
    primed: bool,
}

/// Number of samples over which a gain change ramps from old to
/// new value. ~5.8 ms at 44.1 kHz; ~5.3 ms at 48 kHz. Long enough
/// to elide clicks, short enough to be imperceptible as a slope.
pub const RAMP_SAMPLES: usize = 256;

impl ChannelGainSmoother {
    pub fn new() -> Self { Self::default() }

    /// Mix the channel with click-free gain transitions. The first
    /// call initializes the smoother's state to the channel's
    /// current targets (no fade-in from zero). Subsequent calls
    /// ramp from the previously-applied gains to the new targets
    /// over the first `RAMP_SAMPLES` samples of each block.
    ///
    /// Equivalent to `VirtualChannel::mix(stems)` when gains
    /// haven't changed since the last call.
    pub fn mix(&mut self, ch: &VirtualChannel, stems: &StemBlock) -> Vec<f32> {
        let n = stems.len();
        let mut out = vec![0.0_f32; n];

        // Collect per-stem target gains from the channel definition.
        // Channels may list the same stem twice (unusual but legal);
        // we accumulate the targets so the smoother sees the
        // effective gain rather than only the last-listed entry.
        let mut targets = [0.0_f32; 4];
        let mut touched = [false; 4];
        for (stem, gain) in &ch.stems {
            let g = if gain.is_finite() { *gain } else { 0.0 };
            let idx = stem_index(*stem);
            targets[idx] += g;
            touched[idx] = true;
        }
        // For stems the channel doesn't include, the effective
        // target is zero so the smoother fades them out cleanly
        // when the user removes a stem.
        for (i, t) in touched.iter().enumerate() {
            if !*t { targets[i] = 0.0; }
        }

        // First call: initialize current = target so we don't fade
        // in from silence. Subsequent calls keep the previous block's
        // ending gain as the starting point.
        if !self.primed {
            self.current = targets;
            self.primed = true;
        }

        for stem in Stem::all() {
            let idx = stem_index(stem);
            let cur = self.current[idx];
            let tgt = targets[idx];
            // Per-stem source slice.
            let src = match stem {
                Stem::Drums => &stems.drums,
                Stem::Voice => &stems.voice,
                Stem::Piano => &stems.piano,
                Stem::Synth => &stems.synth,
            };
            let m = src.len().min(n);

            if (cur - tgt).abs() < 1e-9 {
                // No change → straight multiply, no per-sample math.
                if tgt != 0.0 {
                    for i in 0..m { out[i] += src[i] * tgt; }
                }
            } else {
                // Linear ramp over the first RAMP_SAMPLES samples
                // (capped at block length). After the ramp completes
                // the rest of the block applies the target gain
                // directly.
                let ramp_len = RAMP_SAMPLES.min(m);
                if ramp_len > 0 {
                    let step = (tgt - cur) / ramp_len as f32;
                    let mut g = cur;
                    for i in 0..ramp_len {
                        g += step;
                        out[i] += src[i] * g;
                    }
                }
                if m > ramp_len && tgt != 0.0 {
                    for i in ramp_len..m { out[i] += src[i] * tgt; }
                }
            }
            // The block ended at the target gain (assuming the
            // ramp fit within the block); next call starts there.
            self.current[idx] = tgt;
        }
        out
    }

    /// Reset the smoother's state so the next mix() initializes
    /// fresh against the channel's targets. Use when seeking,
    /// changing tracks, or otherwise wanting to discard the
    /// previous block's accumulated gain.
    pub fn reset(&mut self) {
        self.current = [0.0; 4];
        self.primed = false;
    }
}

#[inline]
fn stem_index(stem: Stem) -> usize {
    match stem {
        Stem::Drums => 0,
        Stem::Voice => 1,
        Stem::Piano => 2,
        Stem::Synth => 3,
    }
}

/// Full routing config: every defined channel + an assignment of
/// channels to device URLs. A device may play multiple channels;
/// a channel may go to multiple devices (e.g. stereo pair).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelRouting {
    pub channels: Vec<VirtualChannel>,
    /// device_url → list of channel names this device should play.
    /// The bo-mux deploy logic walks this map to point each device
    /// at its `/ch/<name>` endpoint URLs.
    pub device_assignments: HashMap<String, Vec<String>>,
}

impl ChannelRouting {
    pub fn new() -> Self { Self::default() }

    /// Find a channel by name. None when missing — names are not
    /// case-folded, exact match only.
    pub fn channel(&self, name: &str) -> Option<&VirtualChannel> {
        self.channels.iter().find(|c| c.name == name)
    }

    /// Validate the config. Returns Ok if every device_assignment
    /// names channels that exist, channel names are unique, no
    /// channel has zero stems, and gains are finite. Returns the
    /// first error found. The first-error-and-stop policy is
    /// deliberate: most config errors are simple typos and the
    /// caller wants a clear single message.
    pub fn validate(&self) -> Result<(), String> {
        // 1. Channel names are unique.
        for (i, c) in self.channels.iter().enumerate() {
            if c.name.is_empty() {
                return Err(format!("channel at index {} has empty name", i));
            }
            // Channel name must be URL-safe — it shows up in
            // /ch/<name>. Reject anything that needs encoding
            // rather than silently mojibake'ing the URL.
            for ch in c.name.chars() {
                if !(ch.is_ascii_alphanumeric() || ch == '_' || ch == '-') {
                    return Err(format!(
                        "channel name '{}' contains unsafe char '{}'; \
                         use ASCII alphanumeric, '_', or '-'",
                        c.name, ch));
                }
            }
            for c2 in self.channels.iter().skip(i + 1) {
                if c.name == c2.name {
                    return Err(format!("duplicate channel name '{}'", c.name));
                }
            }
            if c.stems.is_empty() {
                return Err(format!(
                    "channel '{}' has no stems — every channel must \
                     mix at least one", c.name));
            }
            for (_, gain) in &c.stems {
                if !gain.is_finite() {
                    return Err(format!(
                        "channel '{}' has non-finite gain", c.name));
                }
                if *gain < 0.0 {
                    return Err(format!(
                        "channel '{}' has negative gain {} — use \
                         polarity-inverted stems via a separate \
                         channel if you want subtraction",
                        c.name, gain));
                }
            }
        }
        // 2. Every device assignment references a known channel.
        for (url, channels) in &self.device_assignments {
            if url.is_empty() {
                return Err("device assignment with empty URL".into());
            }
            if channels.is_empty() {
                return Err(format!(
                    "device '{}' has no channel assignment", url));
            }
            for cname in channels {
                if self.channel(cname).is_none() {
                    return Err(format!(
                        "device '{}' references unknown channel '{}'",
                        url, cname));
                }
            }
        }
        Ok(())
    }

    /// Default 4-stem-to-4-device routing: one stem per channel,
    /// one channel per device. The user supplies device URLs in
    /// drums/voice/piano/synth order. Channels are named
    /// after their stem.
    pub fn one_stem_per_device(device_urls: [&str; 4]) -> Self {
        let mut r = Self::new();
        for stem in Stem::all() {
            r.channels.push(VirtualChannel::solo(stem.as_str(), stem));
        }
        for (i, url) in device_urls.iter().enumerate() {
            r.device_assignments.insert(
                url.to_string(),
                vec![Stem::all()[i].as_str().to_string()],
            );
        }
        r
    }

    /// Music-focused preset for 3 devices: a "front" channel with
    /// voice + drums (the rhythmic backbone), "harmonic" with piano,
    /// "ambient" with synth. Suits "soundbar in front, bookshelves
    /// on the sides" setups.
    pub fn three_zone_music(front: &str, harmonic: &str, ambient: &str) -> Self {
        let mut r = Self::new();
        r.channels.push(VirtualChannel {
            name: "front".into(),
            stems: vec![(Stem::Voice, 1.0), (Stem::Drums, 0.85)],
        });
        r.channels.push(VirtualChannel::solo("harmonic", Stem::Piano));
        r.channels.push(VirtualChannel::solo("ambient",  Stem::Synth));
        r.device_assignments.insert(front.into(),    vec!["front".into()]);
        r.device_assignments.insert(harmonic.into(), vec!["harmonic".into()]);
        r.device_assignments.insert(ambient.into(),  vec!["ambient".into()]);
        r
    }

    /// Serialize to a compact JSON form for storage in the app KV.
    /// Manual ser to avoid pulling serde derive into this module —
    /// the shape is small and stable.
    pub fn to_json(&self) -> String {
        let mut s = String::from("{\"channels\":[");
        for (i, c) in self.channels.iter().enumerate() {
            if i > 0 { s.push(','); }
            s.push_str(&format!("{{\"name\":\"{}\",\"stems\":[",
                                escape_json(&c.name)));
            for (j, (stem, gain)) in c.stems.iter().enumerate() {
                if j > 0 { s.push(','); }
                s.push_str(&format!("[\"{}\",{}]", stem.as_str(), gain));
            }
            s.push_str("]}");
        }
        s.push_str("],\"devices\":{");
        let mut keys: Vec<&String> = self.device_assignments.keys().collect();
        keys.sort();  // determinism for tests + diffability
        for (i, k) in keys.iter().enumerate() {
            if i > 0 { s.push(','); }
            let v = &self.device_assignments[*k];
            s.push_str(&format!("\"{}\":[", escape_json(k)));
            for (j, cname) in v.iter().enumerate() {
                if j > 0 { s.push(','); }
                s.push_str(&format!("\"{}\"", escape_json(cname)));
            }
            s.push(']');
        }
        s.push_str("}}");
        s
    }

    /// Deserialize. Returns Err on malformed JSON or invalid
    /// content (delegates content-checks to validate() after parse).
    pub fn from_json(text: &str) -> Result<Self, String> {
        let v: serde_json::Value = serde_json::from_str(text)
            .map_err(|e| format!("channel_routing: parse: {}", e))?;
        let mut r = Self::new();
        if let Some(arr) = v.get("channels").and_then(|x| x.as_array()) {
            for entry in arr {
                let name = entry.get("name").and_then(|x| x.as_str())
                    .ok_or("channel missing name".to_string())?.to_string();
                let stems_arr = entry.get("stems").and_then(|x| x.as_array())
                    .ok_or(format!("channel '{}' missing stems", name))?;
                let mut stems: Vec<(Stem, f32)> = Vec::new();
                for s in stems_arr {
                    let pair = s.as_array()
                        .ok_or(format!("channel '{}' stem entry not a pair",
                                       name))?;
                    if pair.len() != 2 {
                        return Err(format!(
                            "channel '{}' stem entry wrong length", name));
                    }
                    let stem_name = pair[0].as_str()
                        .ok_or(format!("channel '{}' stem name not string",
                                       name))?;
                    let stem = stem_from_str(stem_name)
                        .ok_or(format!("channel '{}' references unknown \
                                        stem '{}'", name, stem_name))?;
                    let gain = pair[1].as_f64()
                        .ok_or(format!("channel '{}' stem '{}' gain \
                                        not a number", name, stem_name))?
                        as f32;
                    stems.push((stem, gain));
                }
                r.channels.push(VirtualChannel { name, stems });
            }
        }
        if let Some(obj) = v.get("devices").and_then(|x| x.as_object()) {
            for (url, cv) in obj {
                let arr = cv.as_array()
                    .ok_or(format!("device '{}' channels not array", url))?;
                let names: Vec<String> = arr.iter()
                    .filter_map(|x| x.as_str().map(|s| s.to_string()))
                    .collect();
                r.device_assignments.insert(url.clone(), names);
            }
        }
        r.validate()?;
        Ok(r)
    }

    /// Convenience: produce the list of (device_url, channel_url)
    /// pairs that the bo-mux deploy logic should push to each
    /// device. base_url is the coordinator's bo-mux HTTP base
    /// (e.g. "http://192.168.2.30:47720"); each channel becomes
    /// "<base>/ch/<channel_name>". Devices that play multiple
    /// channels appear once per channel.
    pub fn device_channel_urls(&self, base_url: &str)
        -> Vec<(String, String)>
    {
        let mut out = Vec::new();
        let base = base_url.trim_end_matches('/');
        // Deterministic order: sort by device, then by channel name.
        let mut keys: Vec<&String> = self.device_assignments.keys().collect();
        keys.sort();
        for k in keys {
            let mut channels = self.device_assignments[k].clone();
            channels.sort();
            for cname in channels {
                out.push((k.clone(), format!("{}/ch/{}", base, cname)));
            }
        }
        out
    }
}

// ── Helpers ───────────────────────────────────────────────────────────

fn stem_from_str(s: &str) -> Option<Stem> {
    match s {
        "drums" => Some(Stem::Drums),
        "voice" => Some(Stem::Voice),
        "piano" => Some(Stem::Piano),
        "synth" => Some(Stem::Synth),
        _ => None,
    }
}

/// Minimal JSON string escape. Channel and URL inputs are validated
/// for unsafe chars in validate(), but we still escape \ and " to
/// keep the JSON parser-safe.
fn escape_json(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"'  => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_stems(n: usize) -> StemBlock {
        StemBlock {
            drums: vec![0.5_f32; n],
            voice: vec![0.3_f32; n],
            piano: vec![0.2_f32; n],
            synth: vec![0.1_f32; n],
        }
    }

    #[test]
    fn routing__solo_passes_one_stem_through() {
        let ch = VirtualChannel::solo("d", Stem::Drums);
        let stems = mk_stems(8);
        let out = ch.mix(&stems);
        assert_eq!(out, vec![0.5_f32; 8]);
    }

    #[test]
    fn pan__equal_power_law_is_constant_loudness() {
        // left² + right² == 1 at every pan position.
        for p in [-1.0_f32, -0.5, 0.0, 0.3, 1.0] {
            let (l, r) = equal_power_pan(p);
            assert!(((l * l + r * r) - 1.0).abs() < 1e-5,
                "pan {} not constant-power: l={} r={}", p, l, r);
        }
        // Center is -3 dB equal split.
        let (l, r) = equal_power_pan(0.0);
        assert!((l - r).abs() < 1e-6 && (l - 0.707_106_77).abs() < 1e-4);
        // Hard left / right.
        let (ll, lr) = equal_power_pan(-1.0);
        assert!(ll > 0.999 && lr < 1e-4);
        let (rl, rr) = equal_power_pan(1.0);
        assert!(rr > 0.999 && rl < 1e-4);
        // NaN clamps to center, not a blown-up value.
        let (nl, nr) = equal_power_pan(f32::NAN);
        assert!((nl - nr).abs() < 1e-6);
    }

    #[test]
    fn stereo__panned_stem_lands_on_correct_side() {
        // Piano hard-left, synth hard-right; verify each appears only on
        // its side. drums/voice silent here so we isolate the pans.
        let ch = StereoStemChannel {
            name: "s".into(),
            placements: vec![
                StemPlacement { stem: Stem::Piano, gain: 1.0, pan: -1.0 },
                StemPlacement { stem: Stem::Synth, gain: 1.0, pan:  1.0 },
            ],
            width: 1.0,
        };
        let stems = mk_stems(4);   // piano=0.2, synth=0.1
        let out = ch.mix(&stems);
        // Left should carry the piano (0.2), right the synth (0.1).
        assert!((out[0].0 - 0.2).abs() < 1e-4, "left={}", out[0].0);
        assert!((out[0].1 - 0.1).abs() < 1e-4, "right={}", out[0].1);
    }

    #[test]
    fn stereo__centered_stems_are_equal_l_r() {
        // A centered stem appears equally in both channels at -3 dB.
        let ch = StereoStemChannel {
            name: "c".into(),
            placements: vec![
                StemPlacement { stem: Stem::Voice, gain: 1.0, pan: 0.0 },
            ],
            width: 1.0,
        };
        let out = ch.mix(&mk_stems(4));   // voice = 0.3
        assert!((out[0].0 - out[0].1).abs() < 1e-6, "center should be equal L/R");
        assert!((out[0].0 - 0.3 * 0.707_106_77).abs() < 1e-4);
    }

    #[test]
    fn stereo__width_zero_collapses_pans_to_mono() {
        let ch = StereoStemChannel {
            name: "w".into(),
            placements: vec![
                StemPlacement { stem: Stem::Piano, gain: 1.0, pan: -1.0 },
                StemPlacement { stem: Stem::Synth, gain: 1.0, pan:  1.0 },
            ],
            width: 0.0,
        };
        let out = ch.mix(&mk_stems(4));
        // width 0 → L == R (mono) even though stems were hard-panned.
        assert!((out[0].0 - out[0].1).abs() < 1e-6,
            "width 0 should collapse to mono: {} vs {}", out[0].0, out[0].1);
    }

    #[test]
    fn stereo__default_stage_spreads_four_stems() {
        let ch = StereoStemChannel::default_stage("stage");
        let out = ch.mix(&mk_stems(4));
        // Piano panned left, synth right → left and right differ.
        assert!(out[0].0.is_finite() && out[0].1.is_finite());
        assert!((out[0].0 - out[0].1).abs() > 1e-4,
            "default stage should produce a non-mono field");
    }

    #[test]
    fn routing__multi_stem_mix_adds_with_gain() {
        let ch = VirtualChannel {
            name: "mix".into(),
            stems: vec![(Stem::Drums, 1.0), (Stem::Voice, 0.5)],
        };
        let stems = mk_stems(4);
        let out = ch.mix(&stems);
        // 0.5 + 0.3*0.5 = 0.65
        for v in out { assert!((v - 0.65).abs() < 1e-6); }
    }

    #[test]
    fn routing__zero_gain_skipped() {
        let ch = VirtualChannel {
            name: "z".into(),
            stems: vec![(Stem::Drums, 0.0), (Stem::Voice, 1.0)],
        };
        let stems = mk_stems(4);
        let out = ch.mix(&stems);
        assert_eq!(out, vec![0.3_f32; 4]);
    }

    #[test]
    fn routing__non_finite_gain_ignored_not_propagated() {
        let ch = VirtualChannel {
            name: "n".into(),
            stems: vec![(Stem::Drums, f32::NAN), (Stem::Voice, 1.0)],
        };
        let stems = mk_stems(4);
        let out = ch.mix(&stems);
        // NaN drum gain skipped → only voice contributes.
        for v in out { assert!(v.is_finite() && (v - 0.3).abs() < 1e-6); }
    }

    #[test]
    fn routing__empty_stem_block_yields_empty_output() {
        let ch = VirtualChannel::solo("d", Stem::Drums);
        let stems = StemBlock::default();
        let out = ch.mix(&stems);
        assert!(out.is_empty());
    }

    #[test]
    fn routing__one_stem_per_device_preset_validates() {
        let r = ChannelRouting::one_stem_per_device(
            ["http://10.0.0.1", "http://10.0.0.2",
             "http://10.0.0.3", "http://10.0.0.4"]);
        r.validate().expect("preset should validate");
        assert_eq!(r.channels.len(), 4);
        assert_eq!(r.device_assignments.len(), 4);
        // Each device gets exactly one channel.
        for v in r.device_assignments.values() {
            assert_eq!(v.len(), 1);
        }
    }

    #[test]
    fn routing__three_zone_music_preset_validates() {
        let r = ChannelRouting::three_zone_music(
            "http://10.0.0.1", "http://10.0.0.2", "http://10.0.0.3");
        r.validate().expect("preset should validate");
        assert_eq!(r.channels.len(), 3);
    }

    #[test]
    fn routing__validate_rejects_duplicate_channel_names() {
        let mut r = ChannelRouting::new();
        r.channels.push(VirtualChannel::solo("x", Stem::Drums));
        r.channels.push(VirtualChannel::solo("x", Stem::Voice));
        assert!(r.validate().is_err());
    }

    #[test]
    fn routing__validate_rejects_empty_channel_name() {
        let mut r = ChannelRouting::new();
        r.channels.push(VirtualChannel::solo("", Stem::Drums));
        assert!(r.validate().is_err());
    }

    #[test]
    fn routing__validate_rejects_url_unsafe_channel_name() {
        let mut r = ChannelRouting::new();
        r.channels.push(VirtualChannel::solo("front room", Stem::Drums));
        assert!(r.validate().is_err(),
                "space should be rejected");
        let mut r = ChannelRouting::new();
        r.channels.push(VirtualChannel::solo("ch/sub", Stem::Drums));
        assert!(r.validate().is_err(),
                "slash should be rejected");
    }

    #[test]
    fn routing__validate_rejects_unknown_channel_in_assignment() {
        let mut r = ChannelRouting::new();
        r.channels.push(VirtualChannel::solo("a", Stem::Drums));
        r.device_assignments.insert(
            "http://x".into(), vec!["nonexistent".into()]);
        assert!(r.validate().is_err());
    }

    #[test]
    fn routing__validate_rejects_negative_gain() {
        let mut r = ChannelRouting::new();
        r.channels.push(VirtualChannel {
            name: "x".into(),
            stems: vec![(Stem::Drums, -0.5)],
        });
        let err = r.validate().unwrap_err();
        assert!(err.contains("negative gain"), "got: {}", err);
    }

    #[test]
    fn routing__validate_rejects_empty_stems_list() {
        let mut r = ChannelRouting::new();
        r.channels.push(VirtualChannel { name: "x".into(), stems: vec![] });
        assert!(r.validate().is_err());
    }

    #[test]
    fn routing__json_round_trip() {
        let r = ChannelRouting::three_zone_music(
            "http://10.0.0.1", "http://10.0.0.2", "http://10.0.0.3");
        let json = r.to_json();
        let back = ChannelRouting::from_json(&json)
            .expect("round trip should succeed");
        assert_eq!(back.channels, r.channels);
        assert_eq!(back.device_assignments, r.device_assignments);
    }

    #[test]
    fn routing__json_from_malformed_returns_err() {
        assert!(ChannelRouting::from_json("not json at all").is_err());
        assert!(ChannelRouting::from_json("{}").is_ok());  // empty is valid
        assert!(ChannelRouting::from_json(r#"{"channels":[{"name":"x"}]}"#).is_err(),
                "missing stems should fail");
    }

    #[test]
    fn routing__json_with_unknown_stem_rejected() {
        let bad = r#"{"channels":[{"name":"x","stems":[["unknownstem",1.0]]}]}"#;
        assert!(ChannelRouting::from_json(bad).is_err());
    }

    #[test]
    fn routing__device_channel_urls_lists_all_pairs() {
        let r = ChannelRouting::one_stem_per_device(
            ["http://a", "http://b", "http://c", "http://d"]);
        let pairs = r.device_channel_urls("http://coord:47720");
        assert_eq!(pairs.len(), 4);
        // Every device appears once with its assigned channel URL.
        for (dev, url) in &pairs {
            assert!(url.starts_with("http://coord:47720/ch/"));
            assert!(["http://a","http://b","http://c","http://d"]
                    .contains(&dev.as_str()));
        }
    }

    #[test]
    fn routing__device_channel_urls_handles_multi_channel_device() {
        let mut r = ChannelRouting::new();
        r.channels.push(VirtualChannel::solo("a", Stem::Drums));
        r.channels.push(VirtualChannel::solo("b", Stem::Voice));
        r.device_assignments.insert(
            "http://multi".into(), vec!["a".into(), "b".into()]);
        let pairs = r.device_channel_urls("http://coord");
        assert_eq!(pairs.len(), 2);
    }

    #[test]
    fn routing__device_channel_urls_deterministic_order() {
        let r = ChannelRouting::one_stem_per_device(
            ["http://z", "http://y", "http://x", "http://w"]);
        let p1 = r.device_channel_urls("http://c");
        let p2 = r.device_channel_urls("http://c");
        assert_eq!(p1, p2, "order must be deterministic across calls");
        // Devices should be alphabetic.
        let devs: Vec<&str> = p1.iter().map(|(d, _)| d.as_str()).collect();
        let mut sorted = devs.clone();
        sorted.sort();
        assert_eq!(devs, sorted);
    }

    #[test]
    fn routing__short_stem_block_doesnt_panic() {
        // If one stem is shorter (warmup edge case), mix only the
        // overlap region — must not panic.
        let ch = VirtualChannel {
            name: "x".into(),
            stems: vec![(Stem::Drums, 1.0), (Stem::Voice, 1.0)],
        };
        let stems = StemBlock {
            drums: vec![0.5, 0.5, 0.5],
            voice: vec![0.3, 0.3],  // shorter!
            piano: vec![],
            synth: vec![],
        };
        let out = ch.mix(&stems);
        // Output length follows StemBlock::len() (drums length).
        assert_eq!(out.len(), 3);
        // First two samples include both; third has only drums.
        assert!((out[0] - 0.8).abs() < 1e-6);
        assert!((out[1] - 0.8).abs() < 1e-6);
        assert!((out[2] - 0.5).abs() < 1e-6);
    }

    // ── Randomized fuzz tests for the multi-output engine ────────
    //
    // The ChannelRouting layer is the OTHER half of the multi-output
    // management engine: stream_separator produces stems, then
    // ChannelRouting collapses them into N named virtual channels
    // each going to one or more devices. A bug in this layer turns
    // into "user added 4 devices but only 3 got sound" or "channel
    // had silent samples mid-stream".
    //
    // Deterministic xorshift seeds so failures reproduce.

    fn xorshift_next(state: &mut u64) -> u64 {
        let mut x = *state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        *state = x;
        x
    }
    fn rand_f32(state: &mut u64, lo: f32, hi: f32) -> f32 {
        let bits = xorshift_next(state);
        let n = (bits as f64 / u64::MAX as f64) as f32;
        lo + (hi - lo) * n
    }
    fn rand_stem(state: &mut u64) -> Stem {
        let stems = Stem::all();
        let idx = (xorshift_next(state) as usize) % stems.len();
        stems[idx]
    }
    fn rand_stem_block(state: &mut u64, n: usize) -> StemBlock {
        let mut mk = |s: &mut u64| (0..n).map(|_| rand_f32(s, -1.0, 1.0)).collect();
        StemBlock {
            drums: mk(state),
            voice: mk(state),
            piano: mk(state),
            synth: mk(state),
        }
    }

    #[test]
    fn routing_fuzz__random_channels_finite_input_finite_output() {
        // For any random channel built from 1-4 stems with finite
        // gains, mixing finite stems must yield finite output.
        let mut seed = 0xc8a4_dada_8877_4242_u64;
        for _trial in 0..400 {
            let n_stems = ((xorshift_next(&mut seed) % 4) + 1) as usize;
            let stems: Vec<(Stem, f32)> = (0..n_stems)
                .map(|_| (rand_stem(&mut seed), rand_f32(&mut seed, 0.0, 4.0)))
                .collect();
            let ch = VirtualChannel { name: "x".into(), stems };
            let block = rand_stem_block(&mut seed, 256);
            let out = ch.mix(&block);
            for (i, v) in out.iter().enumerate() {
                assert!(v.is_finite(),
                        "non-finite mix at sample {} (channel stems={:?})",
                        i, ch.stems);
            }
        }
    }

    #[test]
    fn routing_fuzz__non_finite_gain_never_contaminates_output() {
        // The non-finite-gain defense in `mix` must survive every
        // combination: NaN gain on any subset of stems, with finite
        // gain on others. The finite-gain contribution should still
        // make it through; the NaN-gain stems get skipped silently.
        let mut seed = 0xfa11_b0_b_4242_4242_u64;
        for _trial in 0..200 {
            let bad_pos = (xorshift_next(&mut seed) % 4) as usize;
            let bad_kind = match xorshift_next(&mut seed) % 3 {
                0 => f32::NAN,
                1 => f32::INFINITY,
                _ => f32::NEG_INFINITY,
            };
            let stems = Stem::all();
            let mix_stems: Vec<(Stem, f32)> = stems.iter().enumerate()
                .map(|(i, &s)| (s, if i == bad_pos { bad_kind } else {
                    rand_f32(&mut seed, 0.1, 1.0)
                }))
                .collect();
            let ch = VirtualChannel { name: "x".into(), stems: mix_stems };
            let block = rand_stem_block(&mut seed, 128);
            let out = ch.mix(&block);
            for v in out.iter() {
                assert!(v.is_finite(),
                        "non-finite leaked into output (bad pos {}, kind {})",
                        bad_pos, bad_kind);
            }
        }
    }

    #[test]
    fn routing_fuzz__solo_passthrough_is_lossless() {
        // Solo-stem channel at unity gain must reproduce the stem
        // sample-for-sample.
        let mut seed = 0x5010_1555_5555_5555_u64;
        for &stem in &Stem::all() {
            let ch = VirtualChannel::solo("p", stem);
            let block = rand_stem_block(&mut seed, 512);
            let expected = match stem {
                Stem::Drums => &block.drums,
                Stem::Voice => &block.voice,
                Stem::Piano => &block.piano,
                Stem::Synth => &block.synth,
            };
            let out = ch.mix(&block);
            assert_eq!(out.len(), expected.len());
            for (a, b) in out.iter().zip(expected.iter()) {
                assert!((a - b).abs() < 1e-7,
                        "solo {:?} passthrough lost data: {} vs {}",
                        stem, a, b);
            }
        }
    }

    #[test]
    fn routing_fuzz__mix_is_linear_in_block_length() {
        // Mixing a block of length N takes O(N × stems) time;
        // verify output length equals input stem length regardless
        // of which stem index serves as the "block length"
        // reference. StemBlock::len() returns drums.len() by
        // convention so we drive the drums slot explicitly.
        let mut seed = 0xeeeebea7_eeeebea7_u64;
        for n in [0, 1, 7, 64, 255, 1024] {
            let block = rand_stem_block(&mut seed, n);
            let ch = VirtualChannel {
                name: "any".into(),
                stems: vec![
                    (Stem::Drums, 1.0),
                    (Stem::Voice, 0.5),
                    (Stem::Piano, 0.25),
                    (Stem::Synth, 0.1),
                ],
            };
            let out = ch.mix(&block);
            assert_eq!(out.len(), n,
                "expected len {}, got {} for block of {} samples",
                n, out.len(), n);
        }
    }

    #[test]
    fn routing_fuzz__many_devices_per_channel_no_extra_pairs() {
        // The bo-mux deployer iterates `device_channel_urls(base)`
        // to push each device its assigned URL. With N devices on
        // one channel, we expect exactly N pairs back — not N²,
        // not N + 1.
        let mut seed = 0xdeadbeef_deadbe55_u64;
        for n in [1, 2, 4, 8, 16] {
            let mut r = ChannelRouting::new();
            r.channels.push(VirtualChannel::solo("shared", Stem::Voice));
            for i in 0..n {
                r.device_assignments.insert(
                    format!("http://10.0.0.{}", i + 1),
                    vec!["shared".into()],
                );
            }
            r.validate().expect("should validate");
            let pairs = r.device_channel_urls("http://coord");
            assert_eq!(pairs.len(), n,
                "{} devices on one channel produced {} pairs",
                n, pairs.len());
            // Every pair points to the same channel URL.
            for (_, url) in &pairs {
                assert!(url.ends_with("/ch/shared"));
            }
            // Each device appears exactly once.
            let mut devs: Vec<&str> = pairs.iter()
                .map(|(d, _)| d.as_str()).collect();
            devs.sort();
            devs.dedup();
            assert_eq!(devs.len(), n);
            let _ = &mut seed;  // keep generator alive
        }
    }

    #[test]
    fn routing_fuzz__json_round_trip_preserves_random_configs() {
        // For any random ChannelRouting that validates, to_json →
        // from_json must round-trip losslessly. Catches encoding/
        // decoding asymmetries in the manual JSON path.
        let mut seed = 0x724f_554e_445f_5550_u64;
        for _trial in 0..50 {
            let n_channels = 1 + (xorshift_next(&mut seed) % 4) as usize;
            let mut r = ChannelRouting::new();
            let mut channel_names: Vec<String> = Vec::new();
            for i in 0..n_channels {
                // URL-safe name (alphanumeric + _ + -)
                let name = format!("ch_{}", i);
                channel_names.push(name.clone());
                let n_stems = 1 + (xorshift_next(&mut seed) % 4) as usize;
                let stems: Vec<(Stem, f32)> = (0..n_stems)
                    .map(|_| (rand_stem(&mut seed), rand_f32(&mut seed, 0.1, 2.0)))
                    .collect();
                r.channels.push(VirtualChannel { name, stems });
            }
            // Assign 1-3 random channels to each of 1-4 devices.
            let n_devices = 1 + (xorshift_next(&mut seed) % 4) as usize;
            for d in 0..n_devices {
                let url = format!("http://10.0.0.{}", d + 1);
                let pick_count = 1 + (xorshift_next(&mut seed) as usize % channel_names.len());
                let mut assigned: Vec<String> = Vec::new();
                for _ in 0..pick_count {
                    let idx = (xorshift_next(&mut seed) as usize) % channel_names.len();
                    let name = channel_names[idx].clone();
                    if !assigned.contains(&name) { assigned.push(name); }
                }
                r.device_assignments.insert(url, assigned);
            }
            // De-dup channel names since random generation could
            // collide. validate() rejects dupes which would mask
            // the round-trip property we actually want to test.
            r.channels.sort_by(|a, b| a.name.cmp(&b.name));
            r.channels.dedup_by(|a, b| a.name == b.name);
            if r.validate().is_err() { continue; }

            let json = r.to_json();
            let back = ChannelRouting::from_json(&json)
                .expect("round trip should succeed");
            assert_eq!(back.channels.len(), r.channels.len());
            for orig in &r.channels {
                let found = back.channel(&orig.name)
                    .expect(&format!("channel {} missing in round-trip", orig.name));
                assert_eq!(found.stems.len(), orig.stems.len());
                for ((s1, g1), (s2, g2)) in
                    orig.stems.iter().zip(found.stems.iter())
                {
                    assert_eq!(s1, s2);
                    // Float comparison: round-trip through JSON
                    // text → f64 → f32 has bounded error.
                    assert!((g1 - g2).abs() < 1e-5);
                }
            }
            assert_eq!(back.device_assignments, r.device_assignments);
        }
    }

    // ── Gain smoother tests ───────────────────────────────────────

    #[test]
    fn smoother__first_call_initializes_without_fade_in() {
        // The first mix() with a fresh smoother should produce
        // straight target-gain output — no fade-in from zero. The
        // user expects the audio to start at full level when they
        // first hit play.
        let ch = VirtualChannel::solo("a", Stem::Drums);
        let stems = StemBlock {
            drums: vec![0.5_f32; 512],
            voice: vec![],
            piano: vec![],
            synth: vec![],
        };
        let mut sm = ChannelGainSmoother::new();
        let out = sm.mix(&ch, &stems);
        // Every sample should be 0.5 × 1.0 = 0.5 (no ramp).
        for v in out {
            assert!((v - 0.5).abs() < 1e-6,
                "first call should not fade in from zero, got {}", v);
        }
    }

    #[test]
    fn smoother__gain_change_ramps_smoothly() {
        // Mix once at gain 1.0, then change to gain 0.0 and mix
        // again. The transition should ramp from 1.0 → 0.0 over
        // RAMP_SAMPLES samples — no discontinuity at the block
        // boundary, no sample more than `step` smaller than the
        // previous one.
        let mut ch = VirtualChannel::solo("a", Stem::Drums);
        let stems = StemBlock {
            drums: vec![1.0_f32; 1024],
            voice: vec![],
            piano: vec![],
            synth: vec![],
        };
        let mut sm = ChannelGainSmoother::new();
        let _block_1 = sm.mix(&ch, &stems);

        // Now change the gain to 0.0.
        ch.stems[0].1 = 0.0;
        let block_2 = sm.mix(&ch, &stems);

        // Ramp from 1.0 → 0.0 over RAMP_SAMPLES, then 0.0 for the
        // rest of the block. Sample 0 should be roughly 1.0 - step.
        let step = 1.0 / RAMP_SAMPLES as f32;
        // First sample within ramp:
        assert!((block_2[0] - (1.0 - step)).abs() < 1e-5,
            "first sample of ramp should be near {} ; got {}",
            1.0 - step, block_2[0]);
        // Halfway through the ramp:
        let mid = RAMP_SAMPLES / 2;
        assert!((block_2[mid] - 0.5).abs() < 0.01,
            "mid-ramp sample should be near 0.5; got {}", block_2[mid]);
        // After the ramp completes:
        assert!(block_2[RAMP_SAMPLES].abs() < 1e-5,
            "post-ramp sample should be 0; got {}", block_2[RAMP_SAMPLES]);
        assert!(block_2[1000].abs() < 1e-5);

        // Monotonicity inside the ramp: each sample is no more
        // than `step + epsilon` below the previous one. Catches
        // any backward jump in the ramp.
        for i in 1..RAMP_SAMPLES {
            let delta = block_2[i] - block_2[i - 1];
            assert!(delta <= 1e-6,
                "ramp went backward at sample {}: prev={}, cur={}",
                i, block_2[i - 1], block_2[i]);
            assert!(delta > -step - 1e-6,
                "ramp jumped more than step={} at sample {}: delta={}",
                step, i, delta);
        }
    }

    #[test]
    fn smoother__unchanged_gain_uses_direct_path_bit_equal() {
        // When gain hasn't changed across blocks, the smoother
        // should produce bit-identical output to VirtualChannel::mix.
        // This is the common case (user not touching the slider)
        // and must not introduce any drift.
        let ch = VirtualChannel {
            name: "any".into(),
            stems: vec![
                (Stem::Drums, 0.7),
                (Stem::Voice, 0.3),
            ],
        };
        let stems = StemBlock {
            drums: vec![0.5_f32; 512],
            voice: vec![0.2_f32; 512],
            piano: vec![],
            synth: vec![],
        };
        let mut sm = ChannelGainSmoother::new();
        let _ = sm.mix(&ch, &stems);  // prime the smoother
        let smoothed = sm.mix(&ch, &stems);  // second call — gain unchanged
        let direct = ch.mix(&stems);
        assert_eq!(smoothed.len(), direct.len());
        for (s, d) in smoothed.iter().zip(direct.iter()) {
            assert!((s - d).abs() < 1e-7,
                "unchanged gain diverged from direct mix: {} vs {}",
                s, d);
        }
    }

    #[test]
    fn smoother__nan_gain_does_not_corrupt_state() {
        // NaN target → smoother treats it as 0, fades cleanly to
        // silence rather than poisoning current_gain. Subsequent
        // valid gain changes should still ramp normally.
        let stems = StemBlock {
            drums: vec![0.5_f32; 512],
            voice: vec![], piano: vec![], synth: vec![],
        };
        let mut ch = VirtualChannel::solo("a", Stem::Drums);
        let mut sm = ChannelGainSmoother::new();
        // Prime at gain 1.0.
        let _ = sm.mix(&ch, &stems);
        // Set NaN gain.
        ch.stems[0].1 = f32::NAN;
        let out = sm.mix(&ch, &stems);
        for v in &out {
            assert!(v.is_finite(),
                "NaN gain leaked into output sample {}", v);
        }
        // Set gain back to 0.5 — should ramp from 0.0 (where NaN
        // collapsed to) to 0.5.
        ch.stems[0].1 = 0.5;
        let out2 = sm.mix(&ch, &stems);
        for v in &out2 {
            assert!(v.is_finite());
            assert!(*v <= 0.5 + 1e-5);
        }
    }

    #[test]
    fn smoother__reset_clears_primed_flag() {
        let ch = VirtualChannel::solo("a", Stem::Drums);
        let stems = StemBlock {
            drums: vec![0.5_f32; 512],
            voice: vec![], piano: vec![], synth: vec![],
        };
        let mut sm = ChannelGainSmoother::new();
        let _ = sm.mix(&ch, &stems);
        sm.reset();
        // After reset, the next call should re-initialize without
        // fade-in (same as the very first call).
        let out = sm.mix(&ch, &stems);
        for v in out {
            assert!((v - 0.5).abs() < 1e-6);
        }
    }

    #[test]
    fn smoother__removing_a_stem_fades_it_out() {
        // If the user removes a stem from a channel, the smoother
        // should fade that stem's contribution out rather than
        // dropping it instantly. Test: channel A has Drums + Voice,
        // remove Voice, verify ramp.
        let mut ch = VirtualChannel {
            name: "a".into(),
            stems: vec![(Stem::Drums, 1.0), (Stem::Voice, 1.0)],
        };
        let stems = StemBlock {
            drums: vec![0.3_f32; 1024],
            voice: vec![0.4_f32; 1024],
            piano: vec![],
            synth: vec![],
        };
        let mut sm = ChannelGainSmoother::new();
        // Prime: drums=1.0, voice=1.0 → out=0.7 per sample.
        let primed = sm.mix(&ch, &stems);
        assert!((primed[0] - 0.7).abs() < 1e-5);

        // Remove Voice.
        ch.stems.retain(|(s, _)| !matches!(s, Stem::Voice));
        let out = sm.mix(&ch, &stems);
        // Sample 0: voice contribution ramping from 1.0 toward 0.0
        // (one step), drums still 1.0. Roughly:
        //   drums × 0.3 + voice × 0.4 × (1.0 - step) ≈ 0.7 - step×0.4
        let step = 1.0 / RAMP_SAMPLES as f32;
        let expected_0 = 0.3 + 0.4 * (1.0 - step);
        assert!((out[0] - expected_0).abs() < 0.001,
            "first sample after voice removal: expected ~{}, got {}",
            expected_0, out[0]);
        // After the ramp, only drums remains.
        assert!((out[RAMP_SAMPLES + 1] - 0.3).abs() < 1e-5);
    }
}
