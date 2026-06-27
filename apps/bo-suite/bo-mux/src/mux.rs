// src/mux.rs
//
// The channel mux: takes a multichannel source (or named ambient stems) and
// exposes one HTTP endpoint per channel so each claimed speaker/device can
// pull just its channel. This is the "coordinator-holds-and-serves" model
// from DESIGN.md.
//
// This is a sketch — the HTTP serving + decode pipeline needs real audio
// crates (symphonia for decode, a tiny HTTP server, optionally an encoder)
// that aren't wired in the build env. The structure and the data flow are
// real; the gaps are marked TODO with the specific crate that fills them.
//
//                 ┌─────────────────────────────────────────┐
//   source file → │ decode → [optional upmix] → deinterleave │ → per-channel
//   or stems      │          (upmix.rs)          into N rings │   ring buffers
//                 └─────────────────────────────────────────┘
//                                                   │
//             GET /ch/<name>  ──────────────────────┘  (PCM/encoded stream)
//
// Endpoints:
//   GET /ch/FL   → mono stream of the front-left channel
//   GET /ch/water → mono stream of the "water" stem
//   ...
// B&O speakers are pointed at these via the app's existing PlayQueue/setData
// stream calls; local devices pull them into PipeWire.

use crate::upmix::Upmixer;
use std::collections::HashMap;

/// Source of channels: either a single multichannel file, or a set of named
/// stems (one file per stem — your "water + birds + pad" ambient case).
pub enum Source {
    /// One file with N interleaved channels (wav/flac/etc).
    Multichannel { path: String, channels: Vec<String> },
    /// Named stems, each its own file. Key = channel name.
    Stems(HashMap<String, String>),
    /// A stereo file to be expanded to surround via upmix.rs.
    StereoUpmixed { path: String, target: Vec<String> },
}

impl Source {
    /// The channel names this source will serve.
    pub fn channel_names(&self) -> Vec<String> {
        match self {
            Source::Multichannel { channels, .. } => channels.clone(),
            Source::Stems(map) => map.keys().cloned().collect(),
            Source::StereoUpmixed { target, .. } => target.clone(),
        }
    }
}

/// Configuration for a running mux server.
pub struct MuxConfig {
    pub bind_port: u16,
    pub sample_rate: u32,
    /// Encode channels as lossless PCM (simplest, ~big) or a codec (smaller).
    pub encode_pcm: bool,
}

impl Default for MuxConfig {
    fn default() -> Self { Self { bind_port: 47720, sample_rate: 48000, encode_pcm: true } }
}

/// The base URL other devices use to fetch channels, e.g.
/// "http://192.168.2.30:47720" → they GET "/ch/FL" etc.
pub fn channel_base_url(my_ip: &str, cfg: &MuxConfig) -> String {
    format!("http://{}:{}", my_ip, cfg.bind_port)
}

pub fn channel_url(base: &str, channel: &str) -> String {
    format!("{}/ch/{}", base, channel)
}

/// Run the mux server. BLOCKS — call on its own thread.
///
/// Pipeline (TODO markers show the crates that complete each stage):
///   1. Open + decode the source:
///        TODO: symphonia (already a transitive dep via bliss-audio when the
///        `analysis` feature is on) to decode wav/flac/mp3 → f32 frames.
///   2. If StereoUpmixed, run each frame through upmix.rs → 5.1, then map
///      the requested target channels out of the Surround51 frame.
///   3. Deinterleave into one ring buffer per channel.
///   4. Serve GET /ch/<name>: stream that channel's ring as either raw PCM
///      (WAV header + samples) or an encoded stream.
///        TODO: tiny_http or hyper for the HTTP side. WAV streaming is the
///        most compatible with B&O's DLNA pull (audio/wav or audio/L16).
///
/// Sync hook: the coordinator does NOT start filling/serving until the
/// scheduled Start instant logic (sync.rs) says to, and stamps each stream
/// so late-joiners can be positioned. See sync.rs::choose_start_instant.
// NOTE: a working per-channel HTTP server now lives in `chan_server.rs` —
// it serves each channel of a WAV source as mono WAV (verified end-to-end
// with a live socket round-trip), and `fetch_channel_to_temp` is the client
// side. This `run_mux` remains the sketch for the *streaming/decode* variant
// (symphonia decode + live re-encode), which is the next step beyond the
// file-serving path that chan_server already implements.
pub fn run_mux(_source: Source, _cfg: MuxConfig) {
    // Sketch only — see TODOs above. The shape:
    //
    // let decoded = decode_source(&source, cfg.sample_rate);     // (1)
    // let channels = match source {
    //     Source::StereoUpmixed { target, .. } => {
    //         let mut up = Upmixer::new(cfg.sample_rate as f32);
    //         upmix_to_channels(decoded, &mut up, &target)        // (2)
    //     }
    //     _ => deinterleave(decoded, &source.channel_names()),    // (3)
    // };
    // serve_http(channels, cfg);                                  // (4)
    let _ = Upmixer::new(48000.0); // keep the dependency wired
    unimplemented!("mux serving is a sketch; see TODOs and DESIGN.md");
}

/// Map a Surround51 frame to an arbitrary requested channel set, so the
/// upmix output can satisfy whatever layout was claimed. Pure + testable.
pub fn pick_channels_51(
    f: &crate::upmix::Surround51,
    wanted: &[String],
) -> Vec<(String, f32)> {
    wanted.iter().map(|name| {
        let v = match name.as_str() {
            "FL" | "L"  => f.fl,
            "FR" | "R"  => f.fr,
            "C"         => f.c,
            "LFE"       => f.lfe,
            "SL" | "RL" => f.sl,
            "SR" | "RR" => f.sr,
            _ => 0.0,
        };
        (name.clone(), v)
    }).collect()
}

/// Synthesize one upmixed channel from a 16-bit stereo WAV, returning it
/// as a mono WAV. This is the bridge between the pure `Upmixer` DSP and
/// the per-channel file-serving path (chan_server): a four-speaker mesh
/// asks for "/ch/RL" of a stereo source, and this derives the rear-left
/// signal on the fly. `quad` selects synthesis mode:
///   * true  → process_quad41 (FL FR RL RR [LFE]) — quadraphonic + sub
///   * false → process (full 5.1: adds C, SL/SR)
/// A channel the chosen frame doesn't carry resolves to silence (a clean
/// mono WAV of zeros), never an error, so an over-asking mesh degrades
/// gracefully.
pub fn upmix_stereo_wav_channel(
    stereo_wav: &[u8],
    channel: &str,
    quad: bool,
    lfe_enabled: bool,
) -> Result<Vec<u8>, String> {
    use crate::chan_server::{parse_wav, build_mono_wav};
    let (fmt, data_start, data_end) = parse_wav(stereo_wav)?;
    if fmt.channels != 2 {
        return Err(format!("upmix expects stereo, got {} ch", fmt.channels));
    }
    if fmt.bits != 16 {
        return Err(format!("upmix expects 16-bit PCM, got {} bits", fmt.bits));
    }
    let data = &stereo_wav[data_start..data_end];
    let mut up = crate::upmix::Upmixer::new(fmt.sample_rate as f32);
    up.lfe_enabled = lfe_enabled;

    let frame_bytes = 4; // 2 ch * 2 bytes
    let n = data.len() / frame_bytes;
    let mut out = Vec::with_capacity(n * 2);
    let want = [channel.to_string()];
    for i in 0..n {
        let base = i * frame_bytes;
        let l = i16::from_le_bytes([data[base], data[base + 1]]) as f32 / 32768.0;
        let r = i16::from_le_bytes([data[base + 2], data[base + 3]]) as f32 / 32768.0;
        let v = if quad {
            let f = up.process_quad41(l, r);
            pick_channels_quad(&f, &want)[0].1
        } else {
            let f = up.process(l, r);
            pick_channels_51(&f, &want)[0].1
        };
        let s = (v.clamp(-1.0, 1.0) * 32767.0) as i16;
        out.extend_from_slice(&s.to_le_bytes());
    }
    Ok(build_mono_wav(&out, fmt.sample_rate, 16))
}

/// Map a quad (4.1) frame to a requested channel set. A four-speaker mesh
/// asks for FL/FR/RL/RR (and optionally LFE on a sub-capable node); any
/// other requested channel resolves to silence rather than guessing.
pub fn pick_channels_quad(
    f: &crate::upmix::Quad41,
    wanted: &[String],
) -> Vec<(String, f32)> {
    wanted.iter().map(|name| {
        let v = match name.as_str() {
            "FL" | "L"          => f.fl,
            "FR" | "R"          => f.fr,
            "RL" | "SL" | "BL"  => f.rl,
            "RR" | "SR" | "BR"  => f.rr,
            "LFE" | "SUB"       => f.lfe,
            _ => 0.0,
        };
        (name.clone(), v)
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::upmix::Surround51;

    #[test]
    fn pick_maps_aliases() {
        let f = Surround51 { fl: 1.0, fr: 2.0, c: 3.0, lfe: 4.0, sl: 5.0, sr: 6.0 };
        let got = pick_channels_51(&f, &["RL".into(), "RR".into(), "C".into()]);
        assert_eq!(got, vec![("RL".into(), 5.0), ("RR".into(), 6.0), ("C".into(), 3.0)]);
    }

    #[test]
    fn pick_quad_maps_four_channels_and_sub() {
        use crate::upmix::Quad41;
        let f = Quad41 { fl: 1.0, fr: 2.0, rl: 3.0, rr: 4.0, lfe: 5.0 };
        let got = pick_channels_quad(&f,
            &["FL".into(), "FR".into(), "RL".into(), "RR".into(), "LFE".into()]);
        assert_eq!(got, vec![
            ("FL".into(), 1.0), ("FR".into(), 2.0),
            ("RL".into(), 3.0), ("RR".into(), 4.0), ("LFE".into(), 5.0)]);
        // Unknown channel → silence, not a guess.
        let none = pick_channels_quad(&f, &["C".into()]);
        assert_eq!(none, vec![("C".into(), 0.0)]);
        // Side aliases fold to the rears.
        let alias = pick_channels_quad(&f, &["SL".into(), "SR".into()]);
        assert_eq!(alias, vec![("SL".into(), 3.0), ("SR".into(), 4.0)]);
    }

    #[test]
    fn channel_urls_format() {
        let base = channel_base_url("192.168.2.30", &MuxConfig::default());
        assert_eq!(base, "http://192.168.2.30:47720");
        assert_eq!(channel_url(&base, "FL"), "http://192.168.2.30:47720/ch/FL");
    }

    #[test]
    fn upmix_stereo_wav_channel_round_trips() {
        use crate::chan_server::{build_mono_wav, parse_wav};
        // Tiny antiphase stereo WAV: 4 frames, real side content for rears.
        let frames: [(i16, i16); 4] =
            [(10000, -10000), (12000, -12000), (8000, -8000), (5000, -5000)];
        let mut pcm = Vec::new();
        for (l, r) in frames {
            pcm.extend_from_slice(&l.to_le_bytes());
            pcm.extend_from_slice(&r.to_le_bytes());
        }
        let mut wav = Vec::new();
        let data_len = pcm.len() as u32;
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVE");
        wav.extend_from_slice(b"fmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes());     // PCM
        wav.extend_from_slice(&2u16.to_le_bytes());     // stereo
        wav.extend_from_slice(&48000u32.to_le_bytes());
        wav.extend_from_slice(&(48000u32 * 4).to_le_bytes());
        wav.extend_from_slice(&4u16.to_le_bytes());     // block align
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        wav.extend_from_slice(&pcm);

        // FL passes through: derived FL ≈ source L.
        let fl = upmix_stereo_wav_channel(&wav, "FL", true, true).unwrap();
        let (fmt, ds, de) = parse_wav(&fl).unwrap();
        assert_eq!(fmt.channels, 1, "output is mono");
        assert_eq!(fmt.sample_rate, 48000);
        let body = &fl[ds..de];
        let first = i16::from_le_bytes([body[0], body[1]]);
        assert!((first - 10000).abs() < 2, "FL should pass through L");

        // A valid quad channel produces a same-length mono WAV.
        let rl = upmix_stereo_wav_channel(&wav, "RL", true, true).unwrap();
        let (_, rds, rde) = parse_wav(&rl).unwrap();
        assert_eq!(rde - rds, body.len(), "RL same frame count as FL");

        // Non-stereo input is rejected.
        let mono = build_mono_wav(&pcm, 48000, 16);
        assert!(upmix_stereo_wav_channel(&mono, "FL", true, true).is_err());

        // A channel the quad frame doesn't carry (C) → silent, not error.
        let c = upmix_stereo_wav_channel(&wav, "C", true, true).unwrap();
        let (_, cds, cde) = parse_wav(&c).unwrap();
        assert!(c[cds..cde].iter().all(|&b| b == 0), "absent channel silent");
    }

}
