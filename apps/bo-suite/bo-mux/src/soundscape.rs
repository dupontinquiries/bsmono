// src/soundscape.rs
//
// The soundscape engine + FSM director. A soundscape is a living scene
// assembled from independent LAYERS (water, birds, wind, a pad...), each
// drawing from a POOL of clips and evolving under RULES (loop+crossfade,
// random gain/pan/pitch drift, probabilistic triggering). An FSM DIRECTOR
// steers the whole scene through STATES over time (dawn → midday → dusk),
// re-weighting layers as it goes, with weighted-random transitions adding
// variation.
//
// Design intent (from the user's brief):
//   • Slot in files, build looping soundscapes with crossfades.
//   • Random adjustment / patterning for variety, driven by a small FSM
//     feeding a shared scheduler — LOW latency, because layers are
//     independent and don't need tight cross-device alignment.
//   • Optional synchronous mode for challenging pieces (handled by the
//     scheduler + sync.rs, not here).
//   • Combine tracks (e.g. vocals on one speaker, instrumental on another)
//     → that's just two layers pinned to different channels.
//
// This module is PURE: it computes *what should happen when* (a schedule of
// timed playback events) from the scene definition + a seeded RNG. No audio
// I/O, no threads — so it's fully testable. The scheduler/mux turns these
// events into actual cross-device playback.

use serde::{Deserialize, Serialize};

pub type Millis = u64;

// ─── Deterministic RNG (so schedules are reproducible + testable) ───────────
// Small xorshift; we don't need crypto quality, we need determinism so the
// same seed yields the same scene evolution (important for the rollback sync
// system: every device can recompute the identical schedule from the seed).
#[derive(Debug, Clone)]
pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Self { Self(seed.max(1)) }
    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13; x ^= x >> 7; x ^= x << 17;
        self.0 = x; x
    }
    /// f32 in [0,1)
    pub fn unit(&mut self) -> f32 {
        (self.next_u64() >> 11) as f32 / (1u64 << 53) as f32
    }
    /// f32 in [lo,hi)
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 { lo + self.unit() * (hi - lo) }
    /// Uniform integer in [0, n). Uses the HIGH bits of the xorshift state
    /// via a widening multiply (Lemire's method, bias-free for our ranges),
    /// NOT `next_u64() % n`. xorshift64's LOW bits have short periods, so a
    /// raw `% n` (n small, as for clip pools and decorrelation offsets)
    /// inherits that structure and biases the choice. The widening multiply
    /// reads the top bits, which are well-mixed. n==0 returns 0.
    pub fn bounded(&mut self, n: usize) -> usize {
        if n == 0 { return 0; }
        let x = self.next_u64() as u128;
        ((x * (n as u128)) >> 64) as usize
    }
    /// pick a weighted index; weights need not sum to 1.
    ///
    /// Defended against:
    ///   - Empty weights: returns 0 (caller is expected to index-guard,
    ///     but returning a sentinel rather than panicking matches the
    ///     "no fires this period" semantic.)
    ///   - NaN/Inf weights: non-finite weights are treated as 0. If
    ///     every weight is non-finite, returns 0. This matches the
    ///     "broken config produces silence, not crash" expectation.
    ///   - All-zero weights: returns 0 (no useful pick).
    pub fn weighted(&mut self, weights: &[f32]) -> usize {
        if weights.is_empty() { return 0; }
        let total: f32 = weights.iter()
            .map(|w| if w.is_finite() && *w > 0.0 { *w } else { 0.0 })
            .sum();
        if !total.is_finite() || total <= 0.0 { return 0; }
        let mut r = self.unit() * total;
        if !r.is_finite() { return 0; }
        for (i, w) in weights.iter().enumerate() {
            let w = if w.is_finite() && *w > 0.0 { *w } else { 0.0 };
            if r < w { return i; }
            r -= w;
        }
        weights.len() - 1
    }
}

// ─── Layer definition ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layer {
    pub name: String,
    /// Clip file paths/uris to draw from. One picked per trigger (random).
    pub pool: Vec<String>,
    /// Channel/speaker role this layer prefers (e.g. "SL", "water", "vocals").
    pub channel: String,
    /// Crossfade duration between successive clips.
    pub crossfade_ms: Millis,
    /// Gain randomization range (multiplier), e.g. (0.8, 1.0).
    pub gain_range: (f32, f32),
    /// Pan randomization range (-1 left .. +1 right), e.g. (-0.3, 0.3).
    pub pan_range: (f32, f32),
    /// Pitch/rate randomization (1.0 = none), e.g. (0.98, 1.02).
    pub rate_range: (f32, f32),
    /// Probability [0,1] that a given trigger actually fires (sparsity).
    pub trigger_prob: f32,
    /// Approx clip length used for scheduling the next trigger when unknown.
    pub nominal_clip_ms: Millis,
    /// Spatial spread: the resolved (channel, gain) targets this layer
    /// should sound from. When this has 2+ entries, the scheduler emits
    /// a DECORRELATED copy of each trigger to every channel — slightly
    /// offset in time and independently clip-picked — so the layer forms
    /// an enveloping field across multiple speakers instead of a single
    /// point source (which combs/phases when the identical signal hits
    /// several speakers). Empty or single-entry → single-channel behavior
    /// (uses `channel`). Populated by soundscape_config::compile from the
    /// layer's RouteStrategy; hand-built scenes can leave it empty.
    #[serde(default)]
    pub spread: Vec<(String, f32)>,
    /// Swell: a slow gain LFO that makes the layer "breathe" — its
    /// loudness drifts up and down over tens of seconds, the way wind
    /// rises and falls or a brook's intensity wanders. Without it every
    /// trigger sits in the same gain band and the bed sounds mechanically
    /// flat. `swell_period_ms` is the full cycle length; `swell_depth` in
    /// [0,1] is the peak fractional gain deviation (0 = no swell, the
    /// default, so existing scenes are unchanged). Each layer gets a
    /// seed-derived phase so layers don't all swell in lock-step.
    #[serde(default)]
    pub swell_period_ms: Millis,
    #[serde(default)]
    pub swell_depth: f32,
}

impl Layer {
    pub fn simple(name: &str, channel: &str, clips: &[&str], clip_ms: Millis) -> Self {
        Self {
            name: name.into(), channel: channel.into(),
            pool: clips.iter().map(|s| s.to_string()).collect(),
            crossfade_ms: 2000,
            gain_range: (0.85, 1.0),
            pan_range: (-0.2, 0.2),
            rate_range: (1.0, 1.0),
            trigger_prob: 1.0,
            nominal_clip_ms: clip_ms,
            spread: Vec::new(),
            swell_period_ms: 0,
            swell_depth: 0.0,
        }
    }
}

// ─── FSM director ────────────────────────────────────────────────────────────

/// A named scene state with per-layer weighting. weight 0 = layer silent in
/// this state; higher = more present (scales trigger probability + density).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SceneState {
    pub name: String,
    /// layer_name → presence weight in this state.
    pub layer_weights: std::collections::HashMap<String, f32>,
    /// How long (range) to dwell here before transitioning.
    pub dwell_ms: (Millis, Millis),
    /// Weighted transitions to other state indices: (target_index, weight).
    pub transitions: Vec<(usize, f32)>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Soundscape {
    pub name: String,
    pub layers: Vec<Layer>,
    pub states: Vec<SceneState>,
    pub start_state: usize,
    /// Optional preferred output sink (pactl sink name, e.g.
    /// "bluez_sink.AC_8E_BD..." or "alsa_output.usb-..."). When
    /// present, the daemon routes this scene's clips to this sink
    /// via paplay's `--device=`. None means "use the system default
    /// sink at spawn time."
    ///
    /// Also used by activity_watch: if this sink disappears from
    /// `pactl list sinks` (e.g. Bluetooth turned off), bo-mux
    /// enters passive mode instead of spawning child processes
    /// against a phantom target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_sink: Option<String>,
}

// ─── Scheduled events (the director's output) ───────────────────────────────

/// One concrete playback event the scheduler will realize on a device.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayEvent {
    pub at_ms: Millis,        // scene-relative time to begin
    pub layer: String,
    pub channel: String,
    pub clip: String,
    pub gain: f32,
    pub pan: f32,
    pub rate: f32,
    pub crossfade_ms: Millis,
}

/// A state transition marker (for UI + the rollback system's checkpoints).
#[derive(Debug, Clone, PartialEq)]
pub struct StateChange { pub at_ms: Millis, pub state: String }

#[derive(Debug, Clone, PartialEq)]
pub struct Schedule {
    pub events: Vec<PlayEvent>,
    pub state_changes: Vec<StateChange>,
    pub horizon_ms: Millis,
}

/// Carry state threaded between consecutive `direct_seeded` chunks so a
/// rolling-horizon session evolves continuously instead of resetting at
/// every chunk boundary. Without this, a multi-chunk session would:
///   * snap the FSM back to `start_state` each chunk (a dawn→midday→dusk
///     scene would jump back to dawn every chunk — the scene never
///     progressed through its cycle), and
///   * forget the last clip per layer, allowing a back-to-back repeat
///     across the seam.
/// It's plain data (Clone + Default), so callers that don't need
/// continuity (one-shot planning, tests) just ignore it.
#[derive(Debug, Clone, Default)]
pub struct DirectorCarry {
    /// FSM state index to resume from. `None` = start at scene.start_state.
    pub state_idx: Option<usize>,
    /// Per-layer last-chosen clip index, to avoid back-to-back repeats.
    pub last_clip_idx: std::collections::HashMap<String, usize>,
    /// Scene-time remaining in the current state's dwell at the chunk
    /// boundary, so a state that straddles a boundary isn't cut short or
    /// restarted. 0 = pick a fresh dwell on entry.
    pub dwell_remaining_ms: Millis,
}

/// The director: deterministically computes a schedule for the next
/// `horizon_ms` from a soundscape + seed + start offset. Because it's
/// deterministic, EVERY device can compute the identical schedule from the
/// same (seed, scene) — which is what makes the rollback sync system work:
/// a device that falls behind just recomputes from the last checkpoint.
///
/// This is the stateless entry point (FSM starts at `start_state`). For a
/// continuous rolling-horizon session use `direct_seeded` and thread the
/// returned `DirectorCarry` between chunks.
pub fn direct(scene: &Soundscape, seed: u64, start_ms: Millis, horizon_ms: Millis) -> Schedule {
    direct_seeded(scene, seed, start_ms, horizon_ms, &DirectorCarry::default()).0
}

/// Like `direct`, but resumes from `carry` (FSM state + no-repeat memory +
/// in-flight dwell) and returns the carry to feed the next chunk. Still
/// fully deterministic in (scene, seed, start_ms, carry).
pub fn direct_seeded(
    scene: &Soundscape,
    seed: u64,
    start_ms: Millis,
    horizon_ms: Millis,
    carry: &DirectorCarry,
) -> (Schedule, DirectorCarry) {
    let mut rng = Rng::new(seed ^ start_ms.wrapping_mul(0x9E3779B97F4A7C15));
    let mut events = Vec::new();
    let mut state_changes = Vec::new();

    // No-repeat clip memory, seeded from the carry so the seam between
    // chunks doesn't allow a back-to-back repeat.
    let mut last_clip_idx = carry.last_clip_idx.clone();

    let mut t = start_ms;
    let end = start_ms + horizon_ms;
    // Resume the FSM where the previous chunk left off.
    let mut state_idx = carry.state_idx
        .unwrap_or(scene.start_state)
        .min(scene.states.len().saturating_sub(1));
    // If a dwell was in flight across the boundary, honor its remainder
    // before transitioning; otherwise pick a fresh dwell on entry.
    let mut carry_dwell = carry.dwell_remaining_ms;

    while t < end && !scene.states.is_empty() {
        let state = &scene.states[state_idx];
        state_changes.push(StateChange { at_ms: t, state: state.name.clone() });

        let dwell = if carry_dwell > 0 {
            let d = carry_dwell;
            carry_dwell = 0;
            d
        } else {
            rng.range(state.dwell_ms.0 as f32,
                      state.dwell_ms.1.max(state.dwell_ms.0 + 1) as f32) as Millis
        };
        let state_end_unclamped = t + dwell;
        let state_end = state_end_unclamped.min(end);

        // For each layer, lay down triggers across this state's dwell window.
        for layer in &scene.layers {
            let weight = *state.layer_weights.get(&layer.name).unwrap_or(&0.0);
            if weight <= 0.0 { continue; }

            let mut lt = t;
            while lt < state_end {
                // Density scales with weight: heavier presence → shorter gaps.
                let gap = (layer.nominal_clip_ms as f32 / weight.max(0.1)).max(50.0) as Millis;
                let step = (layer.nominal_clip_ms.saturating_sub(layer.crossfade_ms)).max(gap / 2);

                let fires = rng.unit() < (layer.trigger_prob * weight).min(1.0);
                if fires && !layer.pool.is_empty() {
                    // Determine the channel set this trigger sounds from.
                    // With a populated `spread` (2+ resolved channels) we
                    // emit a DECORRELATED copy per channel so the layer
                    // envelops the room rather than collapsing to one
                    // speaker. With 0/1 spread entries it's a single event
                    // on the layer's nominal channel (unchanged behavior).
                    let targets: Vec<(String, f32)> = if layer.spread.len() >= 2 {
                        layer.spread.clone()
                    } else {
                        vec![(layer.channel.clone(), 1.0)]
                    };
                    for (ci, (chan, chan_gain)) in targets.iter().enumerate() {
                        // Independent clip pick per channel so copies aren't
                        // identical (identical signal on multiple speakers
                        // comb-filters). Track per (layer, channel) so each
                        // speaker still avoids its own back-to-back repeats.
                        let key = if targets.len() >= 2 {
                            format!("{}\u{1}{}", layer.name, chan)
                        } else {
                            layer.name.clone()
                        };
                        let idx = pick_clip_idx(&mut rng, layer.pool.len(),
                                                last_clip_idx.get(&key).copied());
                        last_clip_idx.insert(key, idx);
                        let clip = layer.pool[idx].clone();
                        // Per-channel decorrelation delay: 0 for the first
                        // channel, then up to ~40ms staggered. Stays under
                        // the Haas fusion threshold (~50ms) so it widens the
                        // image without being heard as a discrete echo.
                        let decorr_ms = if targets.len() >= 2 {
                            (ci as Millis) * (12 + rng.bounded(18) as Millis)
                        } else { 0 };
                        events.push(PlayEvent {
                            at_ms: lt + decorr_ms,
                            layer: layer.name.clone(),
                            channel: chan.clone(),
                            clip,
                            gain: rng.range(layer.gain_range.0, layer.gain_range.1)
                                  * chan_gain
                                  * swell_gain(layer, lt + decorr_ms),
                            pan: rng.range(layer.pan_range.0, layer.pan_range.1),
                            rate: rng.range(layer.rate_range.0, layer.rate_range.1),
                            crossfade_ms: layer.crossfade_ms,
                        });
                    }
                }
                lt += step.max(50);
            }
        }

        // If the dwell ran past the horizon, the state straddles the
        // chunk boundary: stop here and carry the remainder so the next
        // chunk continues the SAME state rather than transitioning early.
        if state_end_unclamped > end {
            let remaining = state_end_unclamped - end;
            events.sort_by_key(|e| e.at_ms);
            return (
                Schedule { events, state_changes, horizon_ms },
                DirectorCarry {
                    state_idx: Some(state_idx),
                    last_clip_idx,
                    dwell_remaining_ms: remaining,
                },
            );
        }

        // Transition to next state (weighted random).
        if state.transitions.is_empty() { t = state_end; continue; }
        let weights: Vec<f32> = state.transitions.iter().map(|(_, w)| *w).collect();
        let pick = rng.weighted(&weights);
        state_idx = state.transitions[pick].0.min(scene.states.len() - 1);
        t = state_end;
    }

    events.sort_by_key(|e| e.at_ms);
    (
        Schedule { events, state_changes, horizon_ms },
        DirectorCarry {
            state_idx: Some(state_idx),
            last_clip_idx,
            dwell_remaining_ms: 0,
        },
    )
}

/// Slow per-layer gain LFO ("breathing"). Returns a multiplier around
/// 1.0: `1 + depth * sin(2π·t/period + phase)`. The phase is derived
/// deterministically from the layer name so different layers swell out
/// of sync (a forest where the wind and the brook don't pulse together
/// sounds alive; one where they do sounds like a tremolo effect). Uses
/// absolute scene time so the swell is continuous across schedule chunks
/// and identical on every mesh node. Returns 1.0 when swell is disabled
/// (depth 0 or period 0), so non-swelling layers are bit-for-bit
/// unchanged.
fn swell_gain(layer: &Layer, t_ms: Millis) -> f32 {
    if layer.swell_depth <= 0.0 || layer.swell_period_ms == 0 {
        return 1.0;
    }
    // Stable phase in [0, 2π) from a hash of the layer name.
    let mut h: u64 = 1469598103934665603; // FNV offset basis
    for b in layer.name.as_bytes() {
        h ^= *b as u64;
        h = h.wrapping_mul(1099511628211);
    }
    let phase = (h % 6283) as f32 / 1000.0; // ~[0, 2π)
    let frac = (t_ms % layer.swell_period_ms) as f32 / layer.swell_period_ms as f32;
    let lfo = (2.0 * std::f32::consts::PI * frac + phase).sin();
    let depth = layer.swell_depth.clamp(0.0, 1.0);
    (1.0 + depth * lfo).max(0.0)
}


/// chosen on the previous trigger for this layer) when possible. With a
/// single-clip pool there's no choice. With ≥2 clips we draw uniformly
/// from the pool MINUS the last index, so the same clip never plays
/// twice in a row. Deterministic given the rng (schedule reproducibility
/// for mesh convergence is preserved — every node runs the same draw).
fn pick_clip_idx(rng: &mut Rng, len: usize, last: Option<usize>) -> usize {
    if len <= 1 { return 0; }
    match last {
        Some(prev) if prev < len => {
            // Draw from [0, len-1) then skip over `prev` so the
            // distribution over the remaining len-1 clips is uniform.
            let r = rng.bounded(len - 1);
            if r >= prev { r + 1 } else { r }
        }
        _ => rng.bounded(len),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn forest() -> Soundscape {
        let mut dawn = HashMap::new();
        dawn.insert("birds".to_string(), 1.0);
        dawn.insert("water".to_string(), 0.6);
        let mut night = HashMap::new();
        night.insert("water".to_string(), 1.0);
        night.insert("birds".to_string(), 0.0);

        Soundscape {
            name: "forest".into(),
            layers: vec![
                Layer::simple("water", "SL", &["water1.wav", "water2.wav"], 8000),
                Layer::simple("birds", "SR", &["bird1.wav", "bird2.wav", "bird3.wav"], 4000),
            ],
            states: vec![
                SceneState { name: "dawn".into(), layer_weights: dawn,
                    dwell_ms: (10_000, 20_000), transitions: vec![(1, 1.0)] },
                SceneState { name: "night".into(), layer_weights: night,
                    dwell_ms: (10_000, 20_000), transitions: vec![(0, 1.0)] },
            ],
            start_state: 0,
            target_sink: None,
        }
    }

    #[test]
    fn deterministic_same_seed_same_schedule() {
        let s = forest();
        let a = direct(&s, 42, 0, 60_000);
        let b = direct(&s, 42, 0, 60_000);
        assert_eq!(a, b, "same seed must yield identical schedule (rollback relies on this)");
    }

    #[test]
    fn different_seed_diverges() {
        let s = forest();
        let a = direct(&s, 1, 0, 60_000);
        let b = direct(&s, 2, 0, 60_000);
        assert_ne!(a.events, b.events);
    }

    #[test]
    fn silent_layers_produce_no_events() {
        let s = forest();
        // In the "night" state birds weight = 0; ensure no bird events fire
        // while exclusively in night. Start at night, tiny horizon within dwell.
        let mut s2 = s.clone();
        s2.start_state = 1;
        // self-loop night so we stay there
        s2.states[1].transitions = vec![(1, 1.0)];
        let sched = direct(&s2, 7, 0, 8_000);
        assert!(sched.events.iter().all(|e| e.layer != "birds"),
                "birds are weighted 0 in night and must not play");
    }

    #[test]
    fn events_are_time_sorted() {
        let s = forest();
        let sched = direct(&s, 99, 0, 120_000);
        for w in sched.events.windows(2) {
            assert!(w[0].at_ms <= w[1].at_ms);
        }
    }

    #[test]
    fn randomization_within_bounds() {
        let s = forest();
        let sched = direct(&s, 5, 0, 60_000);
        for e in &sched.events {
            assert!(e.gain >= 0.85 && e.gain <= 1.0);
            assert!(e.pan >= -0.2 && e.pan <= 0.2);
        }
    }

    #[test]
    fn weighted_rng_respects_weights() {
        let mut rng = Rng::new(123);
        let mut counts = [0u32; 3];
        for _ in 0..10_000 { counts[rng.weighted(&[1.0, 0.0, 3.0])] += 1; }
        assert_eq!(counts[1], 0, "zero-weight option never picked");
        assert!(counts[2] > counts[0], "weight 3 picked more than weight 1");
    }

    #[test]
    fn bounded_is_uniform_and_in_range() {
        // bounded() must stay within [0,n) and be roughly uniform — it
        // reads the well-mixed HIGH bits, unlike `next_u64() % n` which
        // inherits xorshift64's weak low-bit structure.
        let mut rng = Rng::new(0xDEADBEEF);
        let n = 7usize;
        let mut counts = vec![0u32; n];
        let trials = 70_000;
        for _ in 0..trials {
            let v = rng.bounded(n);
            assert!(v < n, "bounded({}) returned {}", n, v);
            counts[v] += 1;
        }
        let expect = trials / n;
        for (i, &c) in counts.iter().enumerate() {
            let dev = (c as i32 - expect as i32).abs();
            assert!(dev < (expect as i32) / 8,
                "bucket {} count {} deviates too far from {}", i, c, expect);
        }
        // Edge cases: n==0 and n==1 are well-defined.
        assert_eq!(rng.bounded(0), 0);
        assert_eq!(rng.bounded(1), 0);
    }

    #[test]
    fn combine_tracks_via_channels() {
        // vocals on one channel, instrumental on another — just two layers
        // pinned to different channels, both always-on.
        let mut both = HashMap::new();
        both.insert("vocals".to_string(), 1.0);
        both.insert("instr".to_string(), 1.0);
        let scene = Soundscape {
            name: "song".into(),
            layers: vec![
                Layer::simple("vocals", "FL", &["vox.wav"], 200_000),
                Layer::simple("instr", "FR", &["band.wav"], 200_000),
            ],
            states: vec![SceneState { name: "play".into(), layer_weights: both,
                dwell_ms: (200_000, 200_001), transitions: vec![] }],
            start_state: 0,
            target_sink: None,
        };
        let sched = direct(&scene, 1, 0, 10_000);
        assert!(sched.events.iter().any(|e| e.channel == "FL" && e.layer == "vocals"));
        assert!(sched.events.iter().any(|e| e.channel == "FR" && e.layer == "instr"));
    }

    #[test]
    fn no_immediate_clip_repeat_within_layer() {
        // A layer with several clips should never schedule the same clip
        // twice in a row — that's the audible-repetition artifact the
        // no-repeat picker prevents. We use a high trigger probability
        // and a long horizon so there are many consecutive events.
        let mut w = HashMap::new();
        w.insert("bed".to_string(), 1.0);
        let scene = Soundscape {
            name: "rep".into(),
            layers: vec![Layer {
                name: "bed".into(),
                pool: vec!["a.opus".into(), "b.opus".into(),
                           "c.opus".into(), "d.opus".into()],
                channel: "FL".into(),
                crossfade_ms: 500,
                gain_range: (1.0, 1.0),
                pan_range: (0.0, 0.0),
                rate_range: (1.0, 1.0),
                trigger_prob: 1.0,
                nominal_clip_ms: 2000,
                spread: Vec::new(),
                swell_period_ms: 0,
                swell_depth: 0.0,
            }],
            states: vec![SceneState { name: "s".into(), layer_weights: w,
                dwell_ms: (120_000, 120_001), transitions: vec![] }],
            start_state: 0,
            target_sink: None,
        };
        // Try several seeds — the property must hold for all of them.
        for seed in 0..16u64 {
            let sched = direct(&scene, seed, 0, 120_000);
            let clips: Vec<&str> = sched.events.iter()
                .filter(|e| e.layer == "bed")
                .map(|e| e.clip.as_str())
                .collect();
            assert!(clips.len() > 10, "expected many events, got {}", clips.len());
            for win in clips.windows(2) {
                assert_ne!(win[0], win[1],
                    "clip repeated back-to-back (seed {}): {:?}", seed, win);
            }
        }
    }

    #[test]
    fn single_clip_pool_still_works() {
        // A one-clip layer can't avoid repeats — it must still schedule
        // that clip rather than panicking or stalling.
        let mut w = HashMap::new();
        w.insert("drone".to_string(), 1.0);
        let scene = Soundscape {
            name: "one".into(),
            layers: vec![Layer::simple("drone", "C", &["only.opus"], 3000)],
            states: vec![SceneState { name: "s".into(), layer_weights: w,
                dwell_ms: (30_000, 30_001), transitions: vec![] }],
            start_state: 0,
            target_sink: None,
        };
        let sched = direct(&scene, 7, 0, 30_000);
        assert!(sched.events.iter().all(|e| e.clip == "only.opus"));
        assert!(!sched.events.is_empty());
    }

    #[test]
    fn pick_clip_idx_is_uniform_over_remaining() {
        // With last=Some(prev), the picker must return every other index
        // with roughly equal frequency and never `prev`.
        let mut rng = Rng::new(42);
        let len = 5usize;
        let prev = 2usize;
        let mut counts = vec![0u32; len];
        for _ in 0..50_000 {
            let i = pick_clip_idx(&mut rng, len, Some(prev));
            assert_ne!(i, prev, "picker returned the excluded index");
            counts[i] += 1;
        }
        // Each of the 4 remaining indices should get ~25% (12500).
        for (i, &c) in counts.iter().enumerate() {
            if i == prev { assert_eq!(c, 0); continue; }
            assert!((11_000..14_000).contains(&c),
                "index {} got {} hits, expected ~12500", i, c);
        }
    }

    #[test]
    fn spread_layer_emits_on_all_channels() {
        // A layer with a 3-channel spread should produce events on every
        // one of those channels (the enveloping field), not collapse to
        // one speaker.
        let mut w = HashMap::new();
        w.insert("amb".to_string(), 1.0);
        let mut layer = Layer::simple("amb", "FL",
            &["a.opus", "b.opus", "c.opus"], 4000);
        layer.spread = vec![
            ("FL".into(), 0.7),
            ("FR".into(), 0.7),
            ("C".into(),  0.5),
        ];
        let scene = Soundscape {
            name: "spread".into(),
            layers: vec![layer],
            states: vec![SceneState { name: "s".into(), layer_weights: w,
                dwell_ms: (60_000, 60_001), transitions: vec![] }],
            start_state: 0,
            target_sink: None,
        };
        let sched = direct(&scene, 3, 0, 60_000);
        use std::collections::HashSet;
        let chans: HashSet<&str> = sched.events.iter()
            .map(|e| e.channel.as_str()).collect();
        assert!(chans.contains("FL") && chans.contains("FR")
                && chans.contains("C"),
            "spread layer should sound on all 3 channels, got {:?}", chans);
    }

    #[test]
    fn spread_copies_are_decorrelated() {
        // Within one trigger, the per-channel copies must NOT be byte-for
        // byte identical: they should differ in clip choice and/or have a
        // small inter-channel time offset (Haas-range), which is what
        // turns a point source into an enveloping field.
        let mut w = HashMap::new();
        w.insert("amb".to_string(), 1.0);
        let mut layer = Layer::simple("amb", "FL",
            &["a.opus", "b.opus", "c.opus", "d.opus"], 4000);
        layer.spread = vec![("FL".into(), 1.0), ("FR".into(), 1.0)];
        layer.trigger_prob = 1.0;
        let scene = Soundscape {
            name: "decorr".into(),
            layers: vec![layer],
            states: vec![SceneState { name: "s".into(), layer_weights: w,
                dwell_ms: (60_000, 60_001), transitions: vec![] }],
            start_state: 0,
            target_sink: None,
        };
        let sched = direct(&scene, 9, 0, 60_000);
        // Group events into near-simultaneous trigger clusters and check
        // each cluster's FL vs FR copy differs in clip or time.
        let mut fl: Vec<&PlayEvent> = sched.events.iter()
            .filter(|e| e.channel == "FL").collect();
        let mut fr: Vec<&PlayEvent> = sched.events.iter()
            .filter(|e| e.channel == "FR").collect();
        fl.sort_by_key(|e| e.at_ms);
        fr.sort_by_key(|e| e.at_ms);
        assert!(!fl.is_empty() && fl.len() == fr.len(),
            "expected paired FL/FR copies, got {} / {}", fl.len(), fr.len());
        let mut decorrelated = 0;
        for (a, b) in fl.iter().zip(fr.iter()) {
            if a.clip != b.clip || a.at_ms != b.at_ms { decorrelated += 1; }
        }
        // The vast majority of pairs should be decorrelated (a few may
        // coincidentally match clip + have 0 offset on the first channel).
        assert!(decorrelated as f32 / fl.len() as f32 > 0.5,
            "expected most spread copies decorrelated, only {}/{} were",
            decorrelated, fl.len());
    }

    #[test]
    fn swell_disabled_is_a_no_op() {
        // depth 0 (or period 0) must leave gain exactly untouched, so
        // non-swelling scenes are bit-for-bit identical to before.
        let mut l = Layer::simple("x", "FL", &["a.opus"], 2000);
        assert_eq!(swell_gain(&l, 0), 1.0);
        assert_eq!(swell_gain(&l, 12_345), 1.0);
        l.swell_depth = 0.3; // period still 0
        assert_eq!(swell_gain(&l, 12_345), 1.0);
        l.swell_period_ms = 40_000; l.swell_depth = 0.0; // depth 0
        assert_eq!(swell_gain(&l, 12_345), 1.0);
    }

    #[test]
    fn swell_modulates_within_depth_band() {
        // With depth d the multiplier must stay within [1-d, 1+d] and
        // actually move (not constant) across a cycle.
        let mut l = Layer::simple("wind", "FL", &["a.opus"], 2000);
        l.swell_period_ms = 40_000;
        l.swell_depth = 0.25;
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for t in (0..40_000).step_by(500) {
            let g = swell_gain(&l, t);
            assert!(g >= 1.0 - 0.25 - 1e-6 && g <= 1.0 + 0.25 + 1e-6,
                "swell {} out of band at t={}", g, t);
            lo = lo.min(g); hi = hi.max(g);
        }
        assert!(hi - lo > 0.3, "swell should span most of its band, got {}..{}", lo, hi);
    }

    #[test]
    fn swell_is_continuous_across_chunk_boundary() {
        // The swell uses absolute scene time, so the value just before
        // and just after a chunk boundary must be nearly equal (no jump).
        let mut l = Layer::simple("wind", "FL", &["a.opus"], 2000);
        l.swell_period_ms = 30_000;
        l.swell_depth = 0.2;
        let before = swell_gain(&l, 29_999);
        let after = swell_gain(&l, 30_001);
        assert!((before - after).abs() < 0.01,
            "swell jumped across boundary: {} -> {}", before, after);
    }

    #[test]
    fn different_layers_swell_out_of_phase() {
        // Two layers with the same period but different names should have
        // different phases, so they don't pulse in lock-step (which would
        // sound like one tremolo instead of a living environment).
        let mut a = Layer::simple("wind", "FL", &["x.opus"], 2000);
        let mut b = Layer::simple("brook", "FR", &["y.opus"], 2000);
        for l in [&mut a, &mut b] { l.swell_period_ms = 40_000; l.swell_depth = 0.3; }
        // At t=0 their swell values should differ (different phase).
        let diff = (swell_gain(&a, 0) - swell_gain(&b, 0)).abs();
        assert!(diff > 0.01, "layers swell in lock-step (phase not decorrelated)");
    }
}
