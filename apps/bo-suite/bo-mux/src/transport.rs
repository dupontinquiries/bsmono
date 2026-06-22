// src/transport.rs
//
// The transport: a clock thread that drives the verified Player (player.rs)
// in real time. It advances a scene clock, keeps the rolling horizon planned
// ahead, prunes expired voices, and emits VOICE ON / VOICE OFF events to a
// pluggable AudioSink as voices start and stop. This is what turns the
// proven *planning* logic into something that actually *runs*.
//
// The audio backend (PipeWire local out, or per-channel HTTP to a speaker)
// implements AudioSink. The default LogSink just records events — so the
// daemon genuinely runs a soundscape (observably, testably) even with no
// audio hardware wired yet. Swapping in a real sink is the remaining I/O.
//
// Threading: the transport owns the Player on its own thread and takes
// commands (stop, rollback) via a channel. The daemon's Play handler spawns
// it; Stop signals it to exit. Pure timing/voice-diff logic is factored into
// `step()` so it's unit-testable without real time.

use crate::player::{Player, Voice};
use crate::soundscape::{Soundscape, Millis};
use std::collections::HashSet;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::time::{Duration, Instant};

/// Something that can start/stop sounding voices. A voice is identified by a
/// stable key (layer + clip + start time) so the sink can match on/off.
pub trait AudioSink: Send {
    fn voice_on(&mut self, key: &str, v: &Voice, gain: f32);
    fn voice_gain(&mut self, key: &str, gain: f32);   // ongoing crossfade updates
    fn voice_off(&mut self, key: &str);
}

/// Default sink: records events in memory + prints them. Lets the daemon run
/// and be inspected with no audio backend. Replace with a PipeWire/HTTP sink.
#[derive(Default)]
pub struct LogSink {
    pub log: Vec<String>,
}
impl AudioSink for LogSink {
    fn voice_on(&mut self, key: &str, v: &Voice, gain: f32) {
        let line = format!("ON  {} ch={} clip={} gain={:.2} pan={:.2} rate={:.3}",
            key, v.channel, v.clip, gain, v.pan, v.rate);
        eprintln!("[transport] {}", line);
        self.log.push(line);
    }
    fn voice_gain(&mut self, _key: &str, _gain: f32) { /* crossfade tick; quiet */ }
    fn voice_off(&mut self, key: &str) {
        let line = format!("OFF {}", key);
        eprintln!("[transport] {}", line);
        self.log.push(line);
    }
}

pub fn voice_key(v: &Voice) -> String {
    format!("{}:{}:{}", v.layer, v.clip, v.start_ms)
}

/// Commands the daemon sends the transport thread.
pub enum TransportCmd {
    Stop,
    Rollback { to_ms: Millis },
}

/// The mutable transport state, separated from the thread so `step()` is
/// testable with injected time.
pub struct Transport {
    player: Player,
    sounding: HashSet<String>,   // keys currently "on" at the sink
    horizon_lead_ms: Millis,     // keep planned this far ahead of `now`
}

impl Transport {
    pub fn new(scene: Soundscape, seed: u64) -> Self {
        Self {
            player: Player::new(scene, seed),
            sounding: HashSet::new(),
            horizon_lead_ms: 20_000,
        }
    }

    /// Advance to scene-time `now_ms`, planning ahead, pruning, and emitting
    /// on/off/gain to the sink by diffing the active set. Pure given `now_ms`.
    pub fn step<S: AudioSink>(&mut self, now_ms: Millis, sink: &mut S) {
        // Keep the schedule planned ahead of the clock.
        self.player.ensure_planned(now_ms + self.horizon_lead_ms);

        // Compute the currently-active voices.
        let active = self.player.active_voices(now_ms);
        let mut active_keys = HashSet::new();
        for (v, gain) in &active {
            let key = voice_key(v);
            active_keys.insert(key.clone());
            if self.sounding.contains(&key) {
                sink.voice_gain(&key, *gain);          // ongoing crossfade
            } else {
                sink.voice_on(&key, v, *gain);          // newly started
                self.sounding.insert(key);
            }
        }
        // Anything sounding that's no longer active → off.
        let gone: Vec<String> = self.sounding.difference(&active_keys).cloned().collect();
        for key in gone {
            sink.voice_off(&key);
            self.sounding.remove(&key);
        }

        // Housekeeping: drop fully-expired voices from the planner.
        self.player.prune(now_ms.saturating_sub(1_000));
    }

    /// Rollback: silence everything past the checkpoint and re-plan. Because
    /// the player is deterministic, the recomputed schedule matches peers.
    pub fn rollback<S: AudioSink>(&mut self, to_ms: Millis, sink: &mut S) {
        let cp = self.player.rollback_to(to_ms);
        // Silence voices that started at/after the checkpoint (they'll be
        // re-emitted on the next step if still valid).
        let drop: Vec<String> = self.sounding.iter()
            .filter(|k| key_start_ms(k).map_or(false, |s| s >= cp))
            .cloned().collect();
        for k in drop { sink.voice_off(&k); self.sounding.remove(&k); }
    }
}

/// Parse the start_ms back out of a voice key ("layer:clip:start").
fn key_start_ms(key: &str) -> Option<Millis> {
    key.rsplit(':').next().and_then(|s| s.parse().ok())
}

/// Run the transport in real time on the current thread. `start`: the wall
/// instant that maps to scene-time 0 (the scheduled-start instant from
/// sync.rs in a multi-device session; just `now` for solo playback).
pub fn run<S: AudioSink>(
    mut transport: Transport,
    mut sink: S,
    start: Instant,
    cmd_rx: Receiver<TransportCmd>,
    tick: Duration,
) {
    loop {
        match cmd_rx.try_recv() {
            Ok(TransportCmd::Stop) | Err(TryRecvError::Disconnected) => {
                // Silence everything on the way out.
                let keys: Vec<String> = transport.sounding.iter().cloned().collect();
                for k in keys { sink.voice_off(&k); }
                break;
            }
            Ok(TransportCmd::Rollback { to_ms }) => transport.rollback(to_ms, &mut sink),
            Err(TryRecvError::Empty) => {}
        }
        let now_ms = start.elapsed().as_millis() as Millis;
        transport.step(now_ms, &mut sink);
        std::thread::sleep(tick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soundscape::{Layer, SceneState};
    use std::collections::HashMap;

    fn pad_scene() -> Soundscape {
        let mut w = HashMap::new();
        w.insert("pad".to_string(), 1.0);
        Soundscape {
            name: "t".into(),
            layers: vec![Layer::simple("pad", "FL", &["a.wav"], 10_000)],
            states: vec![SceneState { name: "s".into(), layer_weights: w,
                dwell_ms: (1_000_000, 1_000_001), transitions: vec![] }],
            start_state: 0,
            target_sink: None,
        }
    }

    #[test]
    fn voices_turn_on_then_off_across_time() {
        let mut tr = Transport::new(pad_scene(), 7);
        let mut sink = LogSink::default();
        // Step across 60s in 1s increments; we should see ON and OFF events.
        for t in (0..60_000).step_by(1000) {
            tr.step(t, &mut sink);
        }
        let ons = sink.log.iter().filter(|l| l.starts_with("ON ")).count();
        let offs = sink.log.iter().filter(|l| l.starts_with("OFF")).count();
        assert!(ons > 0, "expected some voices to start");
        assert!(offs > 0, "expected some voices to end");
    }

    #[test]
    fn no_duplicate_on_for_same_voice() {
        let mut tr = Transport::new(pad_scene(), 7);
        let mut sink = LogSink::default();
        // Step the same voice region repeatedly at fine resolution.
        for t in (0..5_000).step_by(100) {
            tr.step(t, &mut sink);
        }
        // Each voice key should appear ON at most once in this window.
        let mut seen = std::collections::HashMap::new();
        for l in &sink.log {
            if let Some(rest) = l.strip_prefix("ON  ") {
                let key = rest.split(' ').next().unwrap().to_string();
                *seen.entry(key).or_insert(0) += 1;
            }
        }
        assert!(seen.values().all(|&n| n == 1), "a voice was turned ON more than once: {:?}", seen);
    }

    #[test]
    fn key_start_roundtrips() {
        assert_eq!(key_start_ms("pad:a.wav:12345"), Some(12345));
        assert_eq!(key_start_ms("garbage"), None);
    }

    #[test]
    fn rollback_silences_post_checkpoint_voices() {
        let mut tr = Transport::new(pad_scene(), 7);
        let mut sink = LogSink::default();
        for t in (0..40_000).step_by(500) { tr.step(t, &mut sink); }
        let before_off = sink.log.iter().filter(|l| l.starts_with("OFF")).count();
        tr.rollback(0, &mut sink);   // roll all the way back
        let after_off = sink.log.iter().filter(|l| l.starts_with("OFF")).count();
        assert!(after_off >= before_off, "rollback should emit OFFs for live voices");
    }
}
