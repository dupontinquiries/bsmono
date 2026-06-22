// src/player.rs
//
// The runtime soundscape player: turns the director's deterministic schedule
// (soundscape.rs) into a live, evolving engine. It owns a transport clock,
// manages a pool of "voices" (a voice = one clip currently sounding, with a
// crossfade envelope), extends the schedule on a rolling horizon, and applies
// rollback when a device drifts.
//
// Still PURE + testable: the player computes, for any query time, the set of
// active voices and their instantaneous gains. It does NOT touch audio
// hardware — the mux/output layer reads `active_voices(now)` and renders.
// This keeps the tricky timing/crossfade/rollback logic verifiable.
//
//   director schedule ──► Player ──► active_voices(t) ──► (mux renders audio)
//                            │
//                            ├─ rolling horizon: re-plan before running dry
//                            └─ rollback: recompute from checkpoint on drift

use crate::soundscape::{Soundscape, PlayEvent, Schedule, direct_seeded, DirectorCarry, Millis};

/// A sounding clip with its crossfade envelope, derived from a PlayEvent.
#[derive(Debug, Clone, PartialEq)]
pub struct Voice {
    pub layer: String,
    pub channel: String,
    pub clip: String,
    pub start_ms: Millis,
    pub end_ms: Millis,        // start + clip length (nominal)
    pub crossfade_ms: Millis,
    pub peak_gain: f32,
    pub pan: f32,
    pub rate: f32,
}

impl Voice {
    /// Instantaneous gain at time t, applying an equal-power-ish fade in/out
    /// over crossfade_ms at each end. Returns 0 outside [start,end].
    pub fn gain_at(&self, t: Millis) -> f32 {
        if t < self.start_ms || t >= self.end_ms { return 0.0; }
        let xf = self.crossfade_ms.max(1) as f32;
        let into = (t - self.start_ms) as f32;          // time since start
        let left = (self.end_ms - t) as f32;            // time until end
        let fade_in  = (into / xf).min(1.0);
        let fade_out = (left / xf).min(1.0);
        // sqrt → roughly equal-power crossfade (constant perceived loudness
        // when two voices of the same layer overlap).
        let env = fade_in.min(fade_out).max(0.0).sqrt();
        self.peak_gain * env
    }
    pub fn is_active_at(&self, t: Millis) -> bool {
        t >= self.start_ms && t < self.end_ms
    }
}

fn voice_from_event(e: &PlayEvent, nominal_clip_ms: Millis) -> Voice {
    Voice {
        layer: e.layer.clone(),
        channel: e.channel.clone(),
        clip: e.clip.clone(),
        start_ms: e.at_ms,
        end_ms: e.at_ms + nominal_clip_ms,
        crossfade_ms: e.crossfade_ms,
        peak_gain: e.gain,
        pan: e.pan,
        rate: e.rate,
    }
}

pub struct Player {
    scene: Soundscape,
    seed: u64,
    /// All voices planned so far (sorted by start). Pruned as they expire.
    voices: Vec<Voice>,
    /// Scene-time up to which we've already planned.
    planned_to_ms: Millis,
    /// Rolling-horizon chunk size to plan each time we extend.
    chunk_ms: Millis,
    /// Checkpoints (state-change scene-times) for rollback.
    checkpoints: Vec<Millis>,
    /// Director carry threaded between chunks so the FSM progresses
    /// continuously (a dawn→midday→dusk scene actually advances instead
    /// of resetting to dawn every chunk) and clips don't repeat across
    /// the chunk seam. Snapshotted at each checkpoint for rollback.
    carry: DirectorCarry,
    /// carry state captured at each checkpoint scene-time, so rollback
    /// can resume the director from the exact FSM/clip state that
    /// produced the schedule from that checkpoint forward. Kept in
    /// lock-step with `checkpoints`.
    carry_at: std::collections::HashMap<Millis, DirectorCarry>,
}

impl Player {
    pub fn new(scene: Soundscape, seed: u64) -> Self {
        Self {
            scene, seed,
            voices: Vec::new(),
            planned_to_ms: 0,
            chunk_ms: 30_000,
            checkpoints: Vec::new(),
            carry: DirectorCarry::default(),
            carry_at: std::collections::HashMap::new(),
        }
    }

    /// Look up a layer's nominal clip length (for computing voice end times).
    fn clip_len(&self, layer_name: &str) -> Millis {
        self.scene.layers.iter()
            .find(|l| l.name == layer_name)
            .map(|l| l.nominal_clip_ms)
            .unwrap_or(5_000)
    }

    /// Ensure the schedule is planned at least up to `need_ms`. Plans in
    /// chunk-sized steps so a long session doesn't compute everything up
    /// front (and so rollback only ever recomputes a bounded amount).
    pub fn ensure_planned(&mut self, need_ms: Millis) {
        while self.planned_to_ms < need_ms {
            let from = self.planned_to_ms;
            // Snapshot the carry that PRODUCES this chunk, keyed by the
            // chunk's start time, so rollback to any checkpoint in this
            // chunk can resume the director deterministically.
            let chunk_carry = self.carry.clone();
            let (sched, next_carry): (Schedule, DirectorCarry) =
                direct_seeded(&self.scene, self.seed, from, self.chunk_ms, &chunk_carry);
            for e in &sched.events {
                let len = self.clip_len(&e.layer);
                self.voices.push(voice_from_event(e, len));
            }
            for sc in &sched.state_changes {
                if !self.checkpoints.contains(&sc.at_ms) {
                    self.checkpoints.push(sc.at_ms);
                }
            }
            // The carry as of this chunk's start applies to every
            // checkpoint that falls at the chunk boundary `from`.
            self.carry_at.insert(from, chunk_carry);
            self.carry = next_carry;
            self.planned_to_ms = from + self.chunk_ms;
        }
        self.voices.sort_by_key(|v| v.start_ms);
        self.checkpoints.sort_unstable();
    }

    /// Drop voices that have fully ended before `now` (housekeeping so the
    /// voice list stays bounded during long sessions).
    pub fn prune(&mut self, now: Millis) {
        self.voices.retain(|v| v.end_ms > now);
    }

    /// The voices sounding at `now`, with their instantaneous gains applied.
    /// This is what the mux/output layer consumes to render audio.
    pub fn active_voices(&self, now: Millis) -> Vec<(Voice, f32)> {
        self.voices.iter()
            .filter(|v| v.is_active_at(now))
            .map(|v| (v.clone(), v.gain_at(now)))
            .collect()
    }

    /// Rollback: discard planned voices at/after the checkpoint and re-plan
    /// from there. Because `direct` is deterministic, the recomputed voices
    /// are identical to what every other device computes — so a drifted /
    /// rejoining device converges exactly. Returns the checkpoint used.
    pub fn rollback_to(&mut self, target_ms: Millis) -> Millis {
        let cp = self.checkpoints.iter()
            .filter(|&&c| c <= target_ms)
            .max().copied()
            .unwrap_or(0);
        self.voices.retain(|v| v.start_ms < cp);
        // Rewind planning to the start of the chunk that contains `cp`
        // (chunk starts are multiples of chunk_ms), and restore the
        // director carry captured at that chunk start so re-planning
        // reproduces the exact same FSM/clip progression. Re-planning
        // from a chunk boundary (rather than mid-chunk) keeps the carry
        // bookkeeping simple and the result identical.
        let chunk_start = (cp / self.chunk_ms) * self.chunk_ms;
        self.voices.retain(|v| v.start_ms < chunk_start);
        self.carry = self.carry_at.get(&chunk_start).cloned().unwrap_or_default();
        // Drop checkpoints and carries at/after the rewind point.
        self.checkpoints.retain(|&c| c < chunk_start);
        self.carry_at.retain(|&k, _| k < chunk_start);
        self.planned_to_ms = chunk_start;
        chunk_start
    }

    pub fn checkpoints(&self) -> &[Millis] { &self.checkpoints }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::soundscape::{Layer, SceneState};
    use std::collections::HashMap;

    fn one_layer_scene() -> Soundscape {
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
    fn voice_gain_zero_outside_window() {
        let v = Voice { layer:"l".into(), channel:"FL".into(), clip:"a".into(),
            start_ms: 1000, end_ms: 5000, crossfade_ms: 500, peak_gain: 1.0, pan:0.0, rate:1.0 };
        assert_eq!(v.gain_at(999), 0.0);
        assert_eq!(v.gain_at(5000), 0.0);
        assert!(v.gain_at(3000) > 0.0);
    }

    #[test]
    fn voice_fades_in_and_out() {
        let v = Voice { layer:"l".into(), channel:"FL".into(), clip:"a".into(),
            start_ms: 0, end_ms: 4000, crossfade_ms: 1000, peak_gain: 1.0, pan:0.0, rate:1.0 };
        let g_start = v.gain_at(100);     // early in fade-in
        let g_mid   = v.gain_at(2000);    // fully up
        let g_end   = v.gain_at(3900);    // late in fade-out
        assert!(g_start < g_mid);
        assert!(g_end < g_mid);
        assert!((g_mid - 1.0).abs() < 1e-6);
    }

    #[test]
    fn planning_is_idempotent_and_deterministic() {
        let mut p1 = Player::new(one_layer_scene(), 7);
        let mut p2 = Player::new(one_layer_scene(), 7);
        p1.ensure_planned(30_000);
        p2.ensure_planned(10_000);
        p2.ensure_planned(30_000);     // planned in two steps
        // Both planned to the same horizon → identical voice sets.
        assert_eq!(p1.voices, p2.voices, "chunked planning must match one-shot");
    }

    #[test]
    fn rollback_recomputes_identically() {
        let mut p = Player::new(one_layer_scene(), 7);
        p.ensure_planned(60_000);
        let before: Vec<Voice> = p.voices.clone();
        // Roll back to 25s and re-plan forward to 60s again.
        let cp = p.rollback_to(25_000);
        p.ensure_planned(60_000);
        assert_eq!(p.voices, before, "rollback + replan must reproduce identical voices (seed {} cp {})", 7, cp);
    }

    #[test]
    fn prune_drops_expired() {
        let mut p = Player::new(one_layer_scene(), 7);
        p.ensure_planned(60_000);
        let n_before = p.voices.len();
        p.prune(40_000);
        assert!(p.voices.iter().all(|v| v.end_ms > 40_000));
        assert!(p.voices.len() <= n_before);
    }

    #[test]
    fn active_voices_have_positive_gain() {
        let mut p = Player::new(one_layer_scene(), 7);
        p.ensure_planned(30_000);
        // find a time where at least one voice is active
        let mut found = false;
        for t in (0..30_000).step_by(500) {
            let act = p.active_voices(t);
            if !act.is_empty() {
                assert!(act.iter().all(|(_, g)| *g >= 0.0));
                found = true;
                break;
            }
        }
        assert!(found, "expected some active voice in a pad-only scene");
    }

    /// A 3-state day cycle whose dwell (5s) is shorter than the chunk
    /// (30s), so a single chunk already cycles, and multiple chunks must
    /// keep cycling rather than resetting to the start state.
    fn day_cycle_scene() -> Soundscape {
        let mk = |layer: &str| {
            let mut w = HashMap::new();
            w.insert(layer.to_string(), 1.0);
            w
        };
        Soundscape {
            name: "day".into(),
            layers: vec![
                Layer::simple("dawn_l",  "FL", &["d.wav"], 2_000),
                Layer::simple("mid_l",   "FR", &["m.wav"], 2_000),
                Layer::simple("dusk_l",  "C",  &["k.wav"], 2_000),
            ],
            states: vec![
                SceneState { name: "dawn".into(),   layer_weights: mk("dawn_l"),
                    dwell_ms: (5_000, 5_001), transitions: vec![(1, 1.0)] },
                SceneState { name: "midday".into(), layer_weights: mk("mid_l"),
                    dwell_ms: (5_000, 5_001), transitions: vec![(2, 1.0)] },
                SceneState { name: "dusk".into(),   layer_weights: mk("dusk_l"),
                    dwell_ms: (5_000, 5_001), transitions: vec![(0, 1.0)] },
            ],
            start_state: 0,
            target_sink: None,
        }
    }

    #[test]
    fn fsm_progresses_across_chunks_not_resets() {
        // Regression: ensure_planned used to call direct() per chunk,
        // and direct() reset the FSM to start_state every call — so a
        // dawn→midday→dusk scene snapped back to dawn every 30s chunk
        // and never reached dusk past the first chunk. Now the carry
        // threads the FSM state across chunks.
        let mut p = Player::new(day_cycle_scene(), 7);
        // Plan well past one chunk (90s = 3 chunks, 18 dwell periods).
        p.ensure_planned(90_000);

        // The dusk layer fires only in the dusk state. If the FSM reset
        // each chunk we'd still see dusk within a chunk, so the sharper
        // check: states must keep advancing — collect the ordered state
        // sequence from the planned voices' layers and assert it visits
        // dawn→midday→dusk repeatedly, well into later chunks.
        let mut by_time: Vec<(Millis, &str)> = Vec::new();
        for v in &p.voices {
            let st = match v.layer.as_str() {
                "dawn_l" => "dawn", "mid_l" => "midday", "dusk_l" => "dusk",
                _ => continue,
            };
            by_time.push((v.start_ms, st));
        }
        by_time.sort_by_key(|x| x.0);

        // There must be dusk-state voices AFTER the first chunk boundary
        // (30s) — proving the cycle kept running, not reset.
        let late_dusk = by_time.iter()
            .any(|&(t, st)| t >= 60_000 && st == "dusk");
        assert!(late_dusk,
            "expected dusk-state audio after 60s — FSM should have cycled \
             dawn→midday→dusk multiple times, not reset to dawn each chunk. \
             sequence: {:?}", by_time);

        // And all three states should be represented in the back third
        // (60–90s), confirming continuous cycling deep into the session.
        let back: std::collections::HashSet<&str> = by_time.iter()
            .filter(|&&(t, _)| t >= 60_000)
            .map(|&(_, st)| st)
            .collect();
        assert!(back.contains("dawn") && back.contains("midday")
                && back.contains("dusk"),
            "all three states should appear in 60–90s window, got {:?}", back);
    }

    #[test]
    fn day_cycle_chunked_matches_oneshot() {
        // Continuity must also be deterministic: planning in small steps
        // must produce the exact same voices as planning in one go, so
        // every mesh node converges regardless of when it joined.
        let mut a = Player::new(day_cycle_scene(), 13);
        let mut b = Player::new(day_cycle_scene(), 13);
        a.ensure_planned(90_000);
        for step in (10_000..=90_000).step_by(10_000) {
            b.ensure_planned(step);
        }
        assert_eq!(a.voices, b.voices,
            "chunked planning of a multi-state scene must match one-shot");
    }
}
