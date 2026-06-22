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
    fn channel_urls_format() {
        let base = channel_base_url("192.168.2.30", &MuxConfig::default());
        assert_eq!(base, "http://192.168.2.30:47720");
        assert_eq!(channel_url(&base, "FL"), "http://192.168.2.30:47720/ch/FL");
    }
}
