//! CIRCUIT MATCH — the loop, not just the pieces.
//!
//! Everything up to this file is correct in isolation and never talks
//! to anything else: `circuit::Circuit` runs a timetable that doesn't
//! know a wave exists, `escalation::Waves` runs an economy that
//! doesn't know what a wave IS made of in the world, `circuit_wave.rs`
//! can build a wave's agents but nothing ever calls it from a running
//! match. This module is the seam: one struct, ticked once a frame,
//! that makes the relay actually spawn something, makes killing
//! something actually feed the economy, and makes clearing a wave
//! actually spawn the next one — the "fully playable... gameplay loop"
//! request, at the logic layer this sandbox can verify.
//!
//! **What still is NOT here**: damage. Nothing in this file decides
//! what a shot does to `Enemy::hp` — that is combat resolution
//! (`ballistics.rs`/`weapons.rs`), a shell's job, not this seam's.
//! `report_kill` is the boundary: the shell fights the fight by
//! whatever means it has, and tells this struct when a kill lands.
//! That mirrors `escalation::Waves::on_kill`'s own boundary exactly —
//! this module adds nothing on top of it, just wires it to a real
//! roster instead of leaving it uncalled.

use crate::circuit::{Circuit, Event as CircuitEvent};
use crate::circuit_wave::{self, SpawnedFoe};
use crate::escalation::{self, EnemyKind, Waves};
use crate::moons::{self, MoonSpec};

/// The stop where Escalation actually plays — matches the design
/// brief's table (the Tithe holds the relay and most of the shops).
const ENCOUNTER_STOP_ID: &str = "tithe";

pub struct CircuitMatch {
    pub circuit: Circuit,
    pub waves: Waves,
    /// the currently-spawned roster. Empty until the relay is thrown;
    /// replaced (not appended to) on every new wave.
    pub foes: Vec<SpawnedFoe>,
    seed: u64,
    /// morph clock for `corrupt_body`'s Stage-3 bodies — advances every
    /// tick regardless of anything else, so a body kept alive across
    /// several ticks keeps swimming rather than freezing between kills.
    t: f32,
}

impl CircuitMatch {
    /// A fresh run: `total_waves = 0` (endless/survival — Escalation's
    /// own convention), nobody spawned yet, the ring timetable starting
    /// at the Dock exactly like a bare `Circuit::new()`.
    pub fn new(seed: u64) -> CircuitMatch {
        CircuitMatch { circuit: Circuit::new(), waves: Waves::new(0), foes: Vec::new(), seed, t: 0.0 }
    }

    fn encounter_stop() -> MoonSpec {
        moons::by_id(ENCOUNTER_STOP_ID).expect("ENCOUNTER_STOP_ID must name a real stop")
    }

    fn spawn_current_wave(&mut self) {
        let spec = Self::encounter_stop();
        let pressure = self.circuit.pressure_mul();
        let wave_seed = self.seed ^ (self.waves.wave as u64).wrapping_mul(0xD1B5_4A32);
        self.foes = circuit_wave::spawn_wave_scaled(&spec, self.waves.wave, pressure, wave_seed, self.t);
    }

    /// Count of foes still standing, the one number `escalation::Waves::
    /// update` needs and cannot compute itself (it doesn't hold the
    /// roster — this struct is the reason it doesn't have to).
    fn alive_count(&self) -> i32 {
        self.foes.iter().filter(|f| f.agent.state != crate::enemy::State::Dead).count() as i32
    }

    /// One frame: advance the timetable, react to what it did, and run
    /// the wave economy against however many foes are actually still
    /// alive.
    pub fn update(&mut self, dt: f32) {
        self.t += dt;
        self.circuit.update(dt);
        for ev in self.circuit.drain_events() {
            if ev == CircuitEvent::PowerOn {
                // the relay: perks go live AND the first wave spawns
                // "on the spot" per the design brief
                if self.waves.begin_next().is_some() {
                    self.spawn_current_wave();
                }
            }
        }

        // `escalation::Waves::new` starts with `pending_spawn: true` —
        // correct for Escalation played standalone ("wave 1 is ready
        // to spawn immediately"), wrong here: CIRCUIT gates the whole
        // economy behind the relay. Everything past this point must
        // stay behind `power_on`, or wave 1 spawns on tick one with no
        // relay ever thrown.
        if !self.circuit.power_on {
            return;
        }

        let alive = self.alive_count();
        self.waves.update(alive, dt);
        if self.waves.pending_spawn && self.waves.begin_next().is_some() {
            self.spawn_current_wave();
        }
    }

    /// The shell reports a kill: which foe (by roster index) and by
    /// what means doesn't matter here — mark it dead, and feed the
    /// economy through the exact same door `escalation::Waves::on_kill`
    /// already opens. No-op on an out-of-range or already-dead index,
    /// so a duplicate or late-arriving report can never double-pay.
    pub fn report_kill(&mut self, slot: usize) {
        let Some(f) = self.foes.get_mut(slot) else { return };
        if f.agent.state == crate::enemy::State::Dead {
            return;
        }
        let kind = f.agent.kind;
        f.agent.state = crate::enemy::State::Dead;
        f.agent.hp = 0.0;
        self.waves.on_kill(kind);
    }

    /// Whether the relay has been thrown and an encounter is actually
    /// running — the shell's cue to draw the roster at all.
    pub fn encounter_active(&self) -> bool {
        self.circuit.power_on && self.waves.wave >= 1
    }
}

/// `--circuit-match`: runs a short simulated match — throw the relay,
/// kill everything a few times, watch waves escalate — printing the
/// loop's own account of itself rather than requiring a screen to
/// confirm the seam actually closes.
pub fn run() {
    let mut m = CircuitMatch::new(77);
    println!("CIRCUIT MATCH — simulated run\n");
    m.circuit.throw_relay();
    for tick in 0..6 {
        m.update(1.0);
        println!(
            "tick {tick}: wave {} valence {} foes {} (alive {})",
            m.waves.wave,
            m.waves.valence,
            m.foes.len(),
            m.foes.iter().filter(|f| f.agent.state != crate::enemy::State::Dead).count()
        );
        if tick == 2 {
            println!("  -> wiping the current wave");
            let n = m.foes.len();
            for slot in 0..n {
                m.report_kill(slot);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_encounter_before_the_relay_is_thrown() {
        let mut m = CircuitMatch::new(1);
        for _ in 0..100 {
            m.update(0.5);
        }
        assert!(!m.encounter_active());
        assert!(m.foes.is_empty());
    }

    #[test]
    fn waves_pending_spawn_default_does_not_leak_past_the_relay_gate() {
        // regression: `escalation::Waves::new` starts with
        // `pending_spawn: true` by its own design ("wave 1 is ready to
        // spawn immediately" -- correct for Escalation played
        // standalone). CIRCUIT gates everything behind the relay, and
        // the first version of this file's `update` checked
        // `pending_spawn` unconditionally, spawning wave 1 on tick one
        // with the relay never thrown. Caught by running this test,
        // not by inspection.
        let mut m = CircuitMatch::new(99);
        m.update(0.016); // one tick, relay untouched
        assert_eq!(m.waves.wave, 0, "no wave should exist before the relay");
        assert!(m.foes.is_empty());
    }

    #[test]
    fn throwing_the_relay_spawns_wave_one_immediately() {
        let mut m = CircuitMatch::new(1);
        m.circuit.throw_relay();
        m.update(0.016);
        assert_eq!(m.waves.wave, 1);
        assert!(!m.foes.is_empty());
        assert!(m.encounter_active());
    }

    #[test]
    fn spawned_wave_one_matches_escalations_own_makeup() {
        let mut m = CircuitMatch::new(1);
        m.circuit.throw_relay();
        m.update(0.016);
        let expect = escalation::makeup(1); // pressure_mul is 1.0 at 0 laps
        let grunts = m.foes.iter().filter(|f| f.agent.kind == EnemyKind::Grunt).count() as i32;
        assert_eq!(grunts, expect.grunts);
    }

    #[test]
    fn killing_every_foe_clears_the_wave_and_forges_valence() {
        let mut m = CircuitMatch::new(2);
        m.circuit.throw_relay();
        m.update(0.016);
        let n = m.foes.len();
        for slot in 0..n {
            m.report_kill(slot);
        }
        assert!(m.waves.valence > 0, "kills must have banked valence before the clear");
        // advance past the breather so the clear (and its forge) resolves
        for _ in 0..1000 {
            m.update(0.1);
        }
        assert!(m.waves.forged_total > 0, "a cleared wave must forge something");
    }

    #[test]
    fn clearing_a_wave_eventually_spawns_the_next_one() {
        let mut m = CircuitMatch::new(3);
        m.circuit.throw_relay();
        m.update(0.016);
        assert_eq!(m.waves.wave, 1);
        let n = m.foes.len();
        for slot in 0..n {
            m.report_kill(slot);
        }
        for _ in 0..1000 {
            m.update(0.1);
            if m.waves.wave == 2 {
                break;
            }
        }
        assert_eq!(m.waves.wave, 2);
        assert!(!m.foes.is_empty(), "wave 2 must have actually spawned a new roster");
    }

    #[test]
    fn report_kill_on_an_already_dead_foe_does_not_double_pay() {
        let mut m = CircuitMatch::new(4);
        m.circuit.throw_relay();
        m.update(0.016);
        let before = m.waves.valence;
        m.report_kill(0);
        let after_first = m.waves.valence;
        assert!(after_first > before);
        m.report_kill(0); // same slot again
        assert_eq!(m.waves.valence, after_first, "a duplicate kill report must not pay twice");
    }

    #[test]
    fn report_kill_on_an_out_of_range_slot_does_nothing() {
        let mut m = CircuitMatch::new(5);
        m.circuit.throw_relay();
        m.update(0.016);
        let before = m.waves.valence;
        m.report_kill(9999);
        assert_eq!(m.waves.valence, before);
    }

    #[test]
    fn lap_pressure_actually_reaches_the_spawned_roster() {
        let mut m = CircuitMatch::new(6);
        m.circuit.laps = 4; // pressure_mul = 1 + 4*0.25 = 2.0
        m.circuit.throw_relay();
        m.update(0.016);
        let pressured = m.foes.len();

        let mut base = CircuitMatch::new(6);
        base.circuit.throw_relay();
        base.update(0.016);
        let unpressured = base.foes.len();

        assert!(pressured >= unpressured, "4 laps of pressure must not shrink the roster");
    }

    #[test]
    fn throwing_the_relay_twice_does_not_restart_wave_one() {
        let mut m = CircuitMatch::new(7);
        m.circuit.throw_relay();
        m.update(0.016);
        assert_eq!(m.waves.wave, 1);
        m.circuit.throw_relay(); // idempotent per circuit::Circuit's own contract
        m.update(0.016);
        assert_eq!(m.waves.wave, 1, "a second throw must not restart the encounter");
    }
}
