//! THE LHC SHOWDOWN — a duel run on the paralysis field's own clock.
//!
//! Source: `TF_DOMINION_SCENE_MINDMAP.md`'s Act 3 and
//! `TF_DOMINION_SHIELD_HINTS.md`'s beat breakdown of the same scene:
//! *"LA Hero is running node sabotage between pulse windows. Keelwright
//! is engaging Blackstar directly — timing strikes within the
//! 12-second paralysis intervals."* Two threads, one clock
//! (`pulse::Field`), and — confirmed across both docs — the field
//! only catches Transformers: *"all Autobots in range freeze
//! briefly"* is `TF_DOMINION_SCENE_MINDMAP.md`'s own wording, and LA
//! Hero's whole tactical value in the scene is being *"human
//! scale — too fast for Blackstar to stop."* Nothing here invents a
//! human immunity; it is stated twice, independently, and this module
//! just runs the two threads the doc actually describes:
//!
//! * **Keelwright** can only land a strike during the field's safe
//!   window — a strike attempted while paralyzed does not fail
//!   quietly, it REFUSES, same as every mechanic in this game that
//!   takes `clinch.rs`'s design rule seriously ("random" means "I
//!   could not have seen it coming"). The field's own phase is always
//!   readable (`pulse::Field::until_flip`), so a whiff here is a
//!   player's bad timing, not the game's.
//! * **The saboteur** (LA Hero, or whoever the caller casts in the
//!   role) works on the nodes every tick, paralysis or not — the
//!   doc's entire point about the partnership becoming "genuinely
//!   equal" is that the human half of the fight does not wait for an
//!   opening the Transformer half is denied.
//!
//! This module is a bounded, standalone combat system in the same
//! spirit `clinch.rs` already established for the aerial grapple: real
//! mechanics, fully tested, not yet wired into the live shell — the
//! same honest seam, not a new kind of gap.

use crate::pulse::Field;

/// Canon note, from `TF_ADDITIONAL_SCENE_AIDS.md` section 3's own
/// operational flowchart: Keelwright' half of this scene is described as
/// "buys LA Hero the seconds needed," and Act 4's resolution has
/// Blackstar's network "disrupted — not destroyed" while Blackstar
/// himself retreats rather than falls in single combat. The scene's
/// own resolution condition is `network_stopped`, not `boss_down` —
/// `boss_down` is offered as a real, independently-tested win path
/// because this module is a general-purpose mechanic rather than a
/// hard-coded cutscene, but a caller staging THIS specific scene
/// should treat the sabotage finishing as the actual end, and Keelwright'
/// strikes as pressure bought for it rather than a kill attempt.
pub struct Duel {
    pub field: Field,
    pub boss_hp: f32,
    /// nodes LA Hero still has to reach; counts down to zero
    pub nodes_left: u32,
    /// seconds of sabotage work banked toward the current node
    node_progress: f32,
}

/// How long one node takes to sabotage, at the doc's own "too fast for
/// Blackstar to stop" pace — fast relative to the twelve-second cycle,
/// so the race reads as a race rather than a foregone conclusion
/// either way.
pub const SEC_PER_NODE: f32 = 5.0;

impl Duel {
    pub fn new(boss_hp: f32, nodes: u32) -> Duel {
        Duel { field: Field::new(), boss_hp, nodes_left: nodes, node_progress: 0.0 }
    }

    pub fn tick(&mut self, dt: f32) {
        self.field.tick(dt);
    }

    /// Keelwright's half. Only lands inside the safe window; a strike
    /// attempted mid-paralysis refuses outright and says how long
    /// until the window reopens, so a caller (a controller, an AI, a
    /// test) always has something actionable rather than a dead input.
    pub fn strike(&mut self, dmg: f32) -> Result<f32, String> {
        if self.field.paralyzed() {
            return Err(format!(
                "paralyzed -- the strike does not land; {:.1}s until the window opens",
                self.field.until_flip()
            ));
        }
        let dealt = dmg.min(self.boss_hp.max(0.0));
        self.boss_hp -= dealt;
        Ok(dealt)
    }

    /// LA Hero's half. Runs every tick regardless of the field, per
    /// the doc's own claim that the paralysis never catches her.
    /// Returns true the instant a node completes.
    pub fn sabotage(&mut self, dt: f32) -> bool {
        if self.nodes_left == 0 {
            return false;
        }
        self.node_progress += dt.max(0.0);
        if self.node_progress >= SEC_PER_NODE {
            self.node_progress -= SEC_PER_NODE;
            self.nodes_left -= 1;
            return true;
        }
        false
    }

    pub fn boss_down(&self) -> bool {
        self.boss_hp <= 0.0
    }
    pub fn network_stopped(&self) -> bool {
        self.nodes_left == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pulse::{PERIOD_SEC, SAFE_SEC};

    #[test]
    fn a_strike_refuses_outright_while_paralyzed_and_says_how_long_is_left() {
        let mut d = Duel::new(100.0, 3);
        d.tick(SAFE_SEC + 0.5); // well into the paralyzed stretch
        let err = d.strike(10.0).unwrap_err();
        assert!(err.contains("paralyzed") && err.contains("s until"), "{err}");
        assert_eq!(d.boss_hp, 100.0, "hp moved on a refused strike");
    }

    #[test]
    fn timed_strikes_beat_untimed_ones_over_the_same_duration() {
        // an attacker who only swings inside the safe window lands
        // every hit; one who swings on every tick regardless wastes
        // most of them exactly as the field's own ratio predicts
        let ticks = (60.0 / 0.1) as u32; // one minute, 100ms steps
        let mut timed = Duel::new(1_000_000.0, 0);
        let mut naive = Duel::new(1_000_000.0, 0);
        let (mut timed_dealt, mut naive_dealt) = (0.0, 0.0);
        for _ in 0..ticks {
            timed.tick(0.1);
            naive.tick(0.1);
            if timed.field.safe() {
                timed_dealt += timed.strike(1.0).unwrap();
            }
            if let Ok(d) = naive.strike(1.0) {
                naive_dealt += d;
            }
        }
        assert!((timed_dealt - naive_dealt).abs() < 1e-3, "the two attackers landed a different total");
        // and it matches the field's own stated ratio: a quarter of the cycle
        let expected_hits = (ticks as f32 * (SAFE_SEC / PERIOD_SEC)).round();
        assert!((timed_dealt - expected_hits).abs() <= 2.0, "{timed_dealt} vs expected {expected_hits}");
    }

    #[test]
    fn the_saboteur_never_stops_for_the_paralysis_field() {
        let mut d = Duel::new(1.0, 2);
        // hold the fight in the paralyzed stretch the whole time and
        // confirm the nodes still fall
        let mut t = 0.0;
        while !d.network_stopped() {
            d.tick(0.1);
            d.sabotage(0.1);
            t += 0.1;
            assert!(t < 60.0, "the saboteur stalled waiting on a field she was never caught by");
        }
        assert!(t < SEC_PER_NODE * 2.0 + 1.0, "sabotage ran slower than its own stated pace");
    }

    #[test]
    fn the_two_threads_run_independently_and_either_can_finish_the_scene() {
        let mut d = Duel::new(5.0, 1);
        // Keelwright lands the kill without any sabotage progress at all
        let mut guard = 0;
        while !d.boss_down() {
            d.tick(0.1);
            if d.field.safe() {
                let _ = d.strike(1.0);
            }
            guard += 1;
            assert!(guard < 10_000, "the boss never went down");
        }
        assert!(d.boss_down());
        assert!(!d.network_stopped(), "sabotage finished itself with no ticks spent on it");
    }
}
