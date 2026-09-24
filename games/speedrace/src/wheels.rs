//! wheels — the training wheels, and the reason they are not free.
//!
//! A practice tool that hands out hints for nothing trains you to ask for
//! hints. The fix is not to make help stingy; it is to make it **cost the one
//! thing the game is about**. Every rung on this ladder adds a time penalty
//! to your finish. Taking a hint can still be correct — a 15s penalty beats
//! being stuck for 90s — but it is now a decision with a number attached,
//! which is exactly the judgement a real deadline asks for.
//!
//! The ladder is ordered by how much it tells you, and priced accordingly.
//! You cannot skip rungs: to see the algorithm you must first have spent the
//! category. That ordering is the pedagogy — it forces you to try the cheap
//! nudge before the expensive answer.

use serde::{Deserialize, Serialize};

use crate::opponent::Ms;

/// One rung of assistance.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Wheel {
    /// "This is a sliding-window problem." Names the shape, nothing else.
    Category,
    /// The complexity you should be aiming for. Kills brute force early.
    TargetComplexity,
    /// A failing case you have not thought of. The most useful rung.
    EdgeCase,
    /// Function signature + the types. Removes the boilerplate fumble.
    Signature,
    /// Prose description of the algorithm. No code.
    Algorithm,
    /// The first few lines, typed in for you.
    Skeleton,
    /// The reference solution. Race over; you are reading now, not racing.
    Solution,
}

impl Wheel {
    pub const LADDER: [Wheel; 7] = [
        Wheel::Category,
        Wheel::TargetComplexity,
        Wheel::EdgeCase,
        Wheel::Signature,
        Wheel::Algorithm,
        Wheel::Skeleton,
        Wheel::Solution,
    ];

    /// Seconds added to your finish time. Roughly: what this rung saves a
    /// stuck person, so taking it is close to neutral and the choice is real.
    pub fn penalty_ms(self) -> Ms {
        match self {
            Wheel::Category => 10_000,
            Wheel::TargetComplexity => 15_000,
            Wheel::EdgeCase => 20_000,
            Wheel::Signature => 25_000,
            Wheel::Algorithm => 45_000,
            Wheel::Skeleton => 60_000,
            Wheel::Solution => 120_000,
        }
    }

    pub fn rung(self) -> usize {
        Self::LADDER.iter().position(|w| *w == self).unwrap()
    }

    pub fn label(self) -> &'static str {
        match self {
            Wheel::Category => "what kind of problem",
            Wheel::TargetComplexity => "target complexity",
            Wheel::EdgeCase => "a case you missed",
            Wheel::Signature => "signature + types",
            Wheel::Algorithm => "the algorithm, in prose",
            Wheel::Skeleton => "first lines, typed in",
            Wheel::Solution => "the whole solution",
        }
    }
}

/// Wheels taken this race, and what they cost.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Wheels {
    taken: Vec<Wheel>,
}

/// Why a rung was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    AlreadyTaken,
    /// You must spend the cheaper rungs first.
    SkipsRung(Wheel),
}

impl Wheels {
    pub fn new() -> Self {
        Self::default()
    }

    /// The next rung you are allowed to take.
    pub fn next_available(&self) -> Option<Wheel> {
        Wheel::LADDER.iter().copied().find(|w| !self.taken.contains(w))
    }

    pub fn take(&mut self, w: Wheel) -> Result<Ms, Refused> {
        if self.taken.contains(&w) {
            return Err(Refused::AlreadyTaken);
        }
        // every cheaper rung must already be spent
        for lower in Wheel::LADDER.iter().take(w.rung()) {
            if !self.taken.contains(lower) {
                return Err(Refused::SkipsRung(*lower));
            }
        }
        self.taken.push(w);
        Ok(w.penalty_ms())
    }

    pub fn penalty_ms(&self) -> Ms {
        self.taken.iter().map(|w| w.penalty_ms()).sum()
    }

    pub fn taken(&self) -> &[Wheel] {
        &self.taken
    }

    pub fn has(&self, w: Wheel) -> bool {
        self.taken.contains(&w)
    }

    /// A run that reached [`Wheel::Solution`] is study, not a race. Scores
    /// from it should never enter [`crate::opponent::Calibration::history`],
    /// or the pace car learns from a time you did not earn.
    pub fn is_scoreable(&self) -> bool {
        !self.has(Wheel::Solution)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_must_be_climbed_in_order() {
        let mut w = Wheels::new();
        assert_eq!(w.take(Wheel::Algorithm), Err(Refused::SkipsRung(Wheel::Category)));
        assert_eq!(w.take(Wheel::Category), Ok(10_000));
        assert_eq!(w.take(Wheel::Algorithm), Err(Refused::SkipsRung(Wheel::TargetComplexity)));
    }

    #[test]
    fn full_climb_accumulates_every_penalty() {
        let mut w = Wheels::new();
        for rung in Wheel::LADDER {
            assert!(w.take(rung).is_ok(), "{rung:?}");
        }
        let expect: Ms = Wheel::LADDER.iter().map(|r| r.penalty_ms()).sum();
        assert_eq!(w.penalty_ms(), expect);
        assert_eq!(w.penalty_ms(), 295_000);
    }

    #[test]
    fn no_double_charging() {
        let mut w = Wheels::new();
        w.take(Wheel::Category).unwrap();
        assert_eq!(w.take(Wheel::Category), Err(Refused::AlreadyTaken));
        assert_eq!(w.penalty_ms(), 10_000);
    }

    #[test]
    fn penalties_increase_up_the_ladder() {
        // If a more revealing rung were ever cheaper, the ladder would invert
        // and the cheapest path would be to ask for the answer.
        for pair in Wheel::LADDER.windows(2) {
            assert!(pair[1].penalty_ms() > pair[0].penalty_ms(),
                    "{:?} not dearer than {:?}", pair[1], pair[0]);
        }
    }

    #[test]
    fn next_available_walks_the_ladder() {
        let mut w = Wheels::new();
        assert_eq!(w.next_available(), Some(Wheel::Category));
        w.take(Wheel::Category).unwrap();
        assert_eq!(w.next_available(), Some(Wheel::TargetComplexity));
    }

    #[test]
    fn reading_the_solution_voids_the_score() {
        let mut w = Wheels::new();
        for rung in Wheel::LADDER.iter().take(6) {
            w.take(*rung).unwrap();
        }
        assert!(w.is_scoreable(), "six rungs is still a race");
        w.take(Wheel::Solution).unwrap();
        assert!(!w.is_scoreable(), "solution read must not feed calibration");
    }
}
