//! THE CORRUPTED — Jhiaxus' experiment, mechanized.
//!
//! Source: `TF_JHIAXUS_AND_THE_CORRUPTED.md`, the screenplay's Film 2
//! horror design document. A Decepticon exposed to Unicron's neural
//! remnant does not die; its transformation protocol is hijacked and
//! reissued forever, with no defined endpoint, so the body attempts
//! transformation perpetually. This module is the treatment's "Powers
//! & Capabilities" section turned into rules a fight can run on:
//!
//! * **Spark resonance** — the doc's hunting mechanism: "they do not
//!   have conventional sensory systems... they hunt by detecting
//!   active sparks... the only things they do not register: humans
//!   and dead machinery." `detects` is that sentence, verbatim as
//!   logic — and it is why the watch-bearer (`chi.rs`, `clones.rs`,
//!   `watch.rs`) is not a liability against them, structurally,
//!   regardless of how the fight is going for everyone else.
//! * **Valence cascade transmission** — "any Transformer struck by it
//!   risks beginning Stage 1 corruption." A human is never at risk;
//!   the doc's transmission is a Transformer-to-Transformer problem,
//!   same as detection.
//! * **Continuous transformation** — "weapons that target a specific
//!   component miss because that component has shifted." Modelled as
//!   a flat evasion the stage grants against any TARGETED (non-area)
//!   hit — not against everything, because the doc is explicit that
//!   Stage 1 "is still coherent — still speaks — still fights"; the
//!   wrongness is aim, not durability.
//! * **Environmental absorption** — Stage 3 only, and the doc's own
//!   gate: "a Corrupted loose in a shipyard... is significantly more
//!   dangerous than one in open terrain." `absorb` refuses outside
//!   Stage 3 rather than silently doing nothing, so a caller cannot
//!   mistake a no-op for a success.
//! * **Harmonic disruption** — close-range proximity cost to nearby
//!   Transformers, explicitly "not corruption — not yet — but a
//!   warning."
//!
//! Three stages, hours/days/weeks after exposure in the doc; here,
//! `Stage::escalate` reads generations instead, since the game has no
//! wall-clock between fights and the doc's own apex-generation logic
//! ("each generation slightly more autonomous... rudimentary
//! strategy") is generation-keyed already.

use crate::weapons::Element;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Stage {
    /// hours after exposure: coherent, still speaks, still fights —
    /// "the most horrifying stage because they know something is wrong"
    One,
    /// days after: multiple alt-mode components emerging in wrong
    /// places, speech degrading — "they are still in there"
    Two,
    /// weeks after: no fixed silhouette, no longer speaks, absorbs mass
    Three,
}

impl Stage {
    /// Chance a TARGETED hit simply misses — the shifted-component
    /// problem. Stage 1 is nearly a normal fight; Stage 3 is fighting
    /// something that is not in the place you aimed.
    pub fn evasion(self) -> f32 {
        match self {
            Stage::One => 0.05,
            Stage::Two => 0.22,
            Stage::Three => 0.45,
        }
    }
    /// "Still coherent — still speaks" (Stage 1/2) vs. "it no longer
    /// speaks. it resonates." (Stage 3) — a caller-facing fact, not
    /// flavor: dialogue systems and barks can gate on this.
    pub fn coherent(self) -> bool {
        self != Stage::Three
    }
    pub fn absorbs(self) -> bool {
        self == Stage::Three
    }
    /// How far the harmonic reaches before a nearby Transformer risks
    /// disruption. Grows with the body: "personal, close, wrong scale"
    /// at Stage 1, a felt-in-the-teeth resonance by Stage 3.
    pub fn harmonic_radius(self) -> f32 {
        match self {
            Stage::One => 4.0,
            Stage::Two => 9.0,
            Stage::Three => 20.0,
        }
    }
}

/// One Corrupted body: what it has become and what it has eaten.
#[derive(Clone, Copy, Debug)]
pub struct Corrupted {
    pub stage: Stage,
    /// generations of pattern learning past the first — the doc's
    /// "rudimentary strategy," Stage 3 apex only in practice since
    /// nothing gets old enough to generation-up before then
    pub generation: u32,
    /// mass absorbed from the environment, Stage 3 only
    pub absorbed: f32,
}

impl Corrupted {
    pub fn new(stage: Stage) -> Corrupted {
        Corrupted { stage, generation: 0, absorbed: 0.0 }
    }

    /// Absorb metal from the environment. Refuses outside Stage 3
    /// rather than quietly doing nothing — Stage 1/2 bodies are not
    /// "absorbing a little"; the doc gives this power to Stage 3 alone.
    pub fn absorb(&mut self, mass: f32) -> Result<(), &'static str> {
        if !self.stage.absorbs() {
            return Err("this body has not reached the absorbing stage");
        }
        self.absorbed += mass.max(0.0);
        Ok(())
    }

    /// Scale grows with what it has eaten — bigger fights in shipyards
    /// than in open terrain, mechanically.
    pub fn threat_mult(&self) -> f32 {
        1.0 + (self.absorbed / 400.0).min(3.0)
    }

    /// The apex mechanic: "learn which approaches work... loosely
    /// coordinate... a shared harmonic." A generation step is earned by
    /// surviving an engagement, not by a timer — the doc's own claim
    /// that it comes from experience, not age.
    pub fn learn(&mut self) {
        self.generation += 1;
    }
    /// Rudimentary strategy activates a few generations in — early
    /// Corrupted "do not have" it; the apex "developed something ...
    /// disturbingly like" it.
    pub fn has_strategy(&self) -> bool {
        self.generation >= 3
    }

    /// "The only things they do not register: humans and dead
    /// machinery." A Transformer with an inactive/dead spark (a wreck,
    /// a powered-down hulk) is exactly as invisible as a human — the
    /// doc's own equivalence, not an invented exception.
    pub fn detects(&self, target_is_human: bool, target_spark_active: bool) -> bool {
        !target_is_human && target_spark_active
    }

    /// Melee contact risk: "any Transformer struck by it risks
    /// beginning Stage 1 corruption." Humans are never at risk — the
    /// doc's transmission is spark-to-spark. Deterministic given a
    /// seed the caller controls, so a fight replays identically.
    pub fn cascade_risk(&self, target_is_human: bool, seed: u64) -> bool {
        if target_is_human {
            return false;
        }
        // higher stages bleed more valence, more often
        let chance = match self.stage {
            Stage::One => 0.08,
            Stage::Two => 0.16,
            Stage::Three => 0.28,
        };
        let mut r = crate::raidgen::Rng::new(seed);
        r.f(0.0, 1.0) < chance
    }

    /// Proximity cost to a Transformer standing within the harmonic
    /// radius: "involuntary partial transformations... not corruption
    /// — not yet — but a warning." Returns a 0..1 disruption strength,
    /// strongest at the body and fading to nothing at the radius edge.
    /// Humans do not feel this the same way sparks do — the caller
    /// applying it to the player's own gear/mods is the seam, not this
    /// module's job.
    pub fn harmonic_disruption(&self, dist: f32) -> f32 {
        let r = self.stage.harmonic_radius();
        if dist >= r {
            0.0
        } else {
            (1.0 - dist / r).clamp(0.0, 1.0)
        }
    }
}

/// The Corrupted fight everything as no element in particular — the
/// doc never gives them one, and forcing a ring position onto "a mass
/// of shifting metal" would invent an advantage/weakness pair the
/// treatment does not support.
pub const ELEMENT: Element = Element::None;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_humans_and_dead_machinery_go_undetected() {
        let c = Corrupted::new(Stage::One);
        assert!(!c.detects(true, true), "a human with an active spark was hunted");
        assert!(!c.detects(false, false), "a dead machine was hunted");
        assert!(c.detects(false, true), "a live Transformer went unnoticed");
    }

    #[test]
    fn cascade_transmission_never_touches_a_human_and_scales_with_stage() {
        let mut hits = [0u32; 3];
        for seed in 0..2000u64 {
            for (i, stage) in [Stage::One, Stage::Two, Stage::Three].into_iter().enumerate() {
                let c = Corrupted::new(stage);
                assert!(!c.cascade_risk(true, seed), "a human caught the cascade");
                if c.cascade_risk(false, seed) {
                    hits[i] += 1;
                }
            }
        }
        assert!(hits[0] < hits[1] && hits[1] < hits[2], "{hits:?}: higher stages did not bleed more");
    }

    #[test]
    fn evasion_climbs_with_stage_but_stage_one_is_still_mostly_a_normal_fight() {
        assert!(Stage::One.evasion() < Stage::Two.evasion());
        assert!(Stage::Two.evasion() < Stage::Three.evasion());
        assert!(Stage::One.evasion() < 0.10, "stage one dodged like a boss already");
        assert!(Stage::One.coherent() && Stage::Two.coherent());
        assert!(!Stage::Three.coherent(), "stage three still spoke");
    }

    #[test]
    fn only_stage_three_absorbs_and_it_actually_scales_the_threat() {
        let mut early = Corrupted::new(Stage::Two);
        assert!(early.absorb(500.0).is_err(), "a stage two body ate a shipyard");
        assert_eq!(early.threat_mult(), 1.0);

        let mut apex = Corrupted::new(Stage::Three);
        let base = apex.threat_mult();
        apex.absorb(800.0).unwrap();
        assert!(apex.threat_mult() > base, "eating a shipyard changed nothing");
    }

    #[test]
    fn strategy_is_earned_by_generations_not_granted_at_birth() {
        let mut c = Corrupted::new(Stage::Three);
        assert!(!c.has_strategy(), "a first-generation body already had a strategy");
        for _ in 0..3 {
            c.learn();
        }
        assert!(c.has_strategy(), "three survived engagements taught it nothing");
    }

    #[test]
    fn the_harmonic_fades_with_distance_and_stops_at_the_radius() {
        let c = Corrupted::new(Stage::Two);
        let close = c.harmonic_disruption(0.5);
        let mid = c.harmonic_disruption(c.stage.harmonic_radius() * 0.5);
        let far = c.harmonic_disruption(c.stage.harmonic_radius() + 1.0);
        assert!(close > mid && mid > far, "{close} {mid} {far}");
        assert_eq!(far, 0.0);
        // a bigger body reaches further, same as the doc's escalation
        assert!(Stage::Three.harmonic_radius() > Stage::One.harmonic_radius());
    }
}
