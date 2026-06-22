// src/delivery.rs
//
// Two adaptive decisions, both pure/testable:
//
//   1. DELIVERY MODE — per channel/device, pick how the audio gets there:
//        WholeFile     : push the entire file, device caches + plays locally
//                        (best when the device can store it & we want zero
//                        streaming jitter — ideal for looping soundscape clips)
//        OpusSubstream : re-encode just the needed channel(s) as Opus and
//                        stream (best when bandwidth-constrained or many
//                        channels share one coordinator uplink)
//        HttpsStream    : live PCM/WAV stream per device (simplest; what dumb
//                        B&O endpoints can pull; higher bandwidth)
//      The director chooses adaptively from file size, channel count, link
//      headroom, and whether the device can cache.
//
//   2. ENDPOINT OUTPUT — on a device running the app, choose the physical
//      output by priority: line-out → Bluetooth → wireless LAN, with
//      fallback if the preferred one is absent.
//
// Plus the ROLLBACK checkpoint math: since the soundscape director is
// deterministic from (seed, scene), a device that drifts or rejoins doesn't
// need state shipped to it — it recomputes from the last checkpoint. This
// module computes which checkpoint to roll back to.

use serde::{Deserialize, Serialize};

// ─── 1. Delivery mode ────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum DeliveryMode {
    WholeFile,
    OpusSubstream,
    HttpsStream,
}

#[derive(Debug, Clone, Copy)]
pub struct LinkConditions {
    /// Estimated usable bandwidth to this device, kbps.
    pub bandwidth_kbps: u32,
    /// Can this device cache a whole file locally? (app-running devices yes;
    /// dumb B&O speakers no — they only pull a stream.)
    pub can_cache: bool,
    /// How many channels this one coordinator is serving concurrently.
    pub concurrent_channels: u32,
}

#[derive(Debug, Clone, Copy)]
pub struct ClipProfile {
    pub file_kbytes: u32,
    /// True for short looping soundscape clips (favor whole-file caching).
    pub is_loop_clip: bool,
}

/// Choose the delivery mode. Reasoning:
///   - A cacheable device + a loop clip → WholeFile (zero stream jitter, the
///     clip loops locally with perfect timing; ideal for soundscapes).
///   - Can't cache (dumb speaker) → must stream. If the uplink is tight
///     (many channels or low bandwidth) → OpusSubstream to fit; else the
///     simpler HttpsStream (WAV/L16) the speaker can pull directly.
pub fn choose_delivery(link: &LinkConditions, clip: &ClipProfile) -> DeliveryMode {
    if link.can_cache && clip.is_loop_clip {
        return DeliveryMode::WholeFile;
    }
    // Streaming. Estimate per-channel PCM demand ~ 1411 kbps (CD stereo) or
    // ~705 kbps mono. With several channels on one uplink, prefer Opus.
    let pcm_per_channel = 705u32;
    let projected = pcm_per_channel * link.concurrent_channels.max(1);
    if projected > link.bandwidth_kbps || link.bandwidth_kbps < 1500 {
        DeliveryMode::OpusSubstream
    } else {
        DeliveryMode::HttpsStream
    }
}

// ─── 2. Endpoint output selection ────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OutputSink {
    LineOut,
    Bluetooth(String),   // device name
    WirelessLan(String), // speaker id / url
    None,
}

#[derive(Debug, Clone, Default)]
pub struct AvailableOutputs {
    pub line_out: bool,
    pub bluetooth: Vec<String>,
    pub wireless: Vec<String>,
}

/// Priority: line-out → Bluetooth → wireless LAN. Returns the first present.
pub fn choose_output(av: &AvailableOutputs) -> OutputSink {
    if av.line_out { return OutputSink::LineOut; }
    if let Some(bt) = av.bluetooth.first() { return OutputSink::Bluetooth(bt.clone()); }
    if let Some(w) = av.wireless.first() { return OutputSink::WirelessLan(w.clone()); }
    OutputSink::None
}

// ─── 3. Rollback checkpoints ─────────────────────────────────────────────────
//
// The director emits StateChanges at known scene-times; these are natural
// CHECKPOINTS. Because the schedule is deterministic from (seed, scene), a
// device only needs to know "you are at scene-time T under seed S" to
// reconstruct everything. If a device drifts beyond tolerance, we roll it
// back to the most recent checkpoint at or before the agreed scene-time and
// let it recompute forward — no buffered state to ship.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Checkpoint { pub scene_ms: u64 }

/// Given checkpoint scene-times (sorted) and the agreed current scene-time,
/// return the checkpoint to roll back to (most recent at or before now).
pub fn rollback_target(checkpoints: &[u64], agreed_now_ms: u64) -> Option<Checkpoint> {
    checkpoints.iter()
        .filter(|&&c| c <= agreed_now_ms)
        .max()
        .map(|&c| Checkpoint { scene_ms: c })
}

/// Decide whether a device needs a rollback: if its measured scene position
/// differs from the agreed position by more than `tolerance_ms`.
pub fn needs_rollback(device_scene_ms: u64, agreed_scene_ms: u64, tolerance_ms: u64) -> bool {
    device_scene_ms.abs_diff(agreed_scene_ms) > tolerance_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loop_clip_on_cacheable_device_uses_whole_file() {
        let link = LinkConditions { bandwidth_kbps: 50_000, can_cache: true, concurrent_channels: 2 };
        let clip = ClipProfile { file_kbytes: 800, is_loop_clip: true };
        assert_eq!(choose_delivery(&link, &clip), DeliveryMode::WholeFile);
    }

    #[test]
    fn dumb_speaker_tight_uplink_uses_opus() {
        let link = LinkConditions { bandwidth_kbps: 1000, can_cache: false, concurrent_channels: 4 };
        let clip = ClipProfile { file_kbytes: 5000, is_loop_clip: false };
        assert_eq!(choose_delivery(&link, &clip), DeliveryMode::OpusSubstream);
    }

    #[test]
    fn dumb_speaker_fat_uplink_uses_plain_stream() {
        let link = LinkConditions { bandwidth_kbps: 50_000, can_cache: false, concurrent_channels: 1 };
        let clip = ClipProfile { file_kbytes: 5000, is_loop_clip: false };
        assert_eq!(choose_delivery(&link, &clip), DeliveryMode::HttpsStream);
    }

    #[test]
    fn output_priority_line_out_first() {
        let av = AvailableOutputs { line_out: true,
            bluetooth: vec!["Buds".into()], wireless: vec!["Stage".into()] };
        assert_eq!(choose_output(&av), OutputSink::LineOut);
    }

    #[test]
    fn output_priority_bluetooth_when_no_line_out() {
        let av = AvailableOutputs { line_out: false,
            bluetooth: vec!["Beo Grace".into()], wireless: vec!["Stage".into()] };
        assert_eq!(choose_output(&av), OutputSink::Bluetooth("Beo Grace".into()));
    }

    #[test]
    fn output_priority_wireless_last() {
        let av = AvailableOutputs { line_out: false, bluetooth: vec![], wireless: vec!["Stage".into()] };
        assert_eq!(choose_output(&av), OutputSink::WirelessLan("Stage".into()));
    }

    #[test]
    fn output_none_when_nothing() {
        assert_eq!(choose_output(&AvailableOutputs::default()), OutputSink::None);
    }

    #[test]
    fn rollback_picks_most_recent_prior_checkpoint() {
        let cps = [0u64, 12_000, 28_000, 41_000];
        assert_eq!(rollback_target(&cps, 30_000), Some(Checkpoint { scene_ms: 28_000 }));
        assert_eq!(rollback_target(&cps, 5_000),  Some(Checkpoint { scene_ms: 0 }));
        assert_eq!(rollback_target(&[], 5_000), None);
    }

    #[test]
    fn needs_rollback_respects_tolerance() {
        assert!(!needs_rollback(10_030, 10_000, 50));  // within 50ms → fine
        assert!(needs_rollback(10_200, 10_000, 50));   // 200ms off → roll back
    }
}
