//! `sim::phrase_learning` — how parrots and Lauschers learn, relay, and slowly
//! garble a spoken phrase.
//!
//! From `03_Voice_and_Audio/animal_voice_lauscher.docx`. Player speech is a physical
//! acoustic event; acoustic fauna respond to it. Two poles: the **parrot** is
//! faithful and transparent — it repeats what it heard, and one generation of parrot
//! processing introduces only a *small* degradation. The **Lauscher** is the echo
//! hunter — it stores what it heard and replays a *more* garbled copy. Left to run,
//! a phrase passes parrot → Lauscher → parrot in a **degradation loop**: each relay
//! drops the fidelity a little, so the phrase mutates and eventually stops being
//! recognisable as the original.

/// Which animal relayed a phrase (sets how much fidelity it costs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Learner {
    /// Faithful mimic — small per-generation degradation.
    Parrot,
    /// Echo hunter — replays a more garbled copy.
    Lauscher,
}

impl Learner {
    /// Fidelity lost when this learner relays a phrase once.
    pub const fn degradation(self) -> f32 {
        match self {
            Learner::Parrot => 0.05,
            Learner::Lauscher => 0.12,
        }
    }
}

/// A phrase in flight through the acoustic fauna, with its remaining fidelity.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Phrase {
    /// Identity of the *original* utterance (a hash of the spoken content).
    pub origin: u64,
    /// Remaining fidelity to the original, `1.0` = perfect … `0.0` = noise.
    pub fidelity: f32,
    /// How many relays it has been through.
    pub generation: u32,
}

impl Phrase {
    /// A phrase as originally spoken by the player — perfect fidelity, generation 0.
    pub fn spoken(origin: u64) -> Phrase {
        Phrase { origin, fidelity: 1.0, generation: 0 }
    }

    /// The phrase after `learner` hears and relays it once: fidelity drops by the
    /// learner's degradation and the generation counter advances. Fidelity floors
    /// at 0 (it can't go negative — beyond that it's just noise).
    pub fn relayed_by(&self, learner: Learner) -> Phrase {
        Phrase {
            origin: self.origin,
            fidelity: (self.fidelity - learner.degradation()).max(0.0),
            generation: self.generation.saturating_add(1),
        }
    }

    /// Is the phrase still recognisable as the original (fidelity at or above the
    /// listener's recognition threshold)?
    pub fn is_recognizable(&self, threshold: f32) -> bool {
        self.fidelity >= threshold
    }
}

/// Run the parrot ↔ Lauscher degradation loop for `hops` relays, starting from a
/// freshly-spoken phrase and alternating parrot, Lauscher, parrot, … Returns the
/// final phrase. Demonstrates the doc's loop: fidelity decays monotonically, and
/// the Lauscher hops cost more than the parrot hops.
pub fn degradation_loop(origin: u64, hops: u32) -> Phrase {
    let mut p = Phrase::spoken(origin);
    for i in 0..hops {
        let learner = if i % 2 == 0 { Learner::Parrot } else { Learner::Lauscher };
        p = p.relayed_by(learner);
    }
    p
}

/// How many relays until the phrase drops below `threshold` recognisability under
/// the alternating loop — the "half-life" of a phrase in the acoustic ecosystem.
pub fn generations_until_lost(threshold: f32, cap: u32) -> u32 {
    let mut p = Phrase::spoken(0);
    let mut i = 0;
    while i < cap {
        let learner = if i % 2 == 0 { Learner::Parrot } else { Learner::Lauscher };
        p = p.relayed_by(learner);
        i += 1;
        if !p.is_recognizable(threshold) {
            break;
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spoken_phrase_is_perfect() {
        let p = Phrase::spoken(0xABCD);
        assert_eq!(p.fidelity, 1.0);
        assert_eq!(p.generation, 0);
        assert!(p.is_recognizable(0.5));
    }

    #[test]
    fn lauscher_garbles_more_than_parrot() {
        let base = Phrase::spoken(1);
        let via_parrot = base.relayed_by(Learner::Parrot);
        let via_lauscher = base.relayed_by(Learner::Lauscher);
        assert!(via_lauscher.fidelity < via_parrot.fidelity);
        assert_eq!(via_parrot.generation, 1);
    }

    #[test]
    fn loop_decays_fidelity_monotonically() {
        let mut last = 1.0;
        for hops in 0..8 {
            let p = degradation_loop(7, hops);
            assert!(p.fidelity <= last + 1e-6, "fidelity should not rise");
            last = p.fidelity;
            assert_eq!(p.origin, 7); // identity is preserved even as it garbles
        }
    }

    #[test]
    fn fidelity_floors_at_zero() {
        let p = degradation_loop(1, 100);
        assert!(p.fidelity >= 0.0);
        assert!(!p.is_recognizable(0.1));
    }

    #[test]
    fn phrase_has_a_finite_half_life() {
        // With a 0.5 threshold and alternating parrot(0.05)/Lauscher(0.12), the
        // phrase falls below recognisability in a handful of hops.
        let n = generations_until_lost(0.5, 100);
        assert!(n > 0 && n < 100, "should be lost in bounded hops, got {n}");
    }
}
