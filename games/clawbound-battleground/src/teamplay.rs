//! teamplay — what the bot team is doing, as opposed to what each bot is doing.
//!
//! `ai::think` decides Push / Fight / Retreat per god, on that god's own lane,
//! from what that god can see. Five bots doing that competently still lose to
//! five humans, because nothing makes them arrive anywhere together: three
//! push three lanes while the enemy takes a titan with four.
//!
//! This is the missing layer — one `Call` for the team, which individual
//! brains defer to. It decides nothing about movement or targeting; those stay
//! in `ai`. It decides WHERE THE TEAM IS GOING and, just as importantly, when
//! it is not going anywhere.
//!
//! ## A call needs a quorum, and that is the whole feature
//!
//! Calling a titan with two gods is how a team loses a game, not how it wins
//! one. Every objective here carries the number of bodies it needs, and a call
//! that cannot be met is not made — the team keeps farming, which is the
//! correct answer and the one an eager objective-seeker gets wrong.
//!
//! ## Self-preservation outranks the call
//!
//! A bot at 15% health walking into a titan pit because the team called it is
//! worse than a bot that ignores calls. [`Call::binds`] refuses to bind a god
//! who should be retreating, so `Retreat` stays a local decision and the call
//! layer cannot override it. The team loses one body from the push; that is
//! cheaper than feeding one.
//!
//! ## Hysteresis, or the team dithers
//!
//! Re-deciding every tick makes five bots oscillate between two objectives and
//! arrive at neither. A call holds for [`CALL_MIN_HOLD`] unless it becomes
//! impossible, which is the same reason a human shotcaller does not change
//! their mind every two seconds.

use crate::consts::Lane;

/// What the team is trying to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Objective {
    /// Farm your own lane. The default, and not a failure to decide — most of
    /// a match is this, and a team that always has somewhere to be is a team
    /// that is always out of position.
    Farm,
    /// Group and push one lane's towers.
    Push(Lane),
    /// Take the jungle boss.
    Boss,
    /// Commit to the enemy titan. The game-ending objective and the one most
    /// worth refusing.
    Titan,
    /// Fall back and defend a lane the enemy is pushing.
    Defend(Lane),
}

impl Objective {
    /// Bodies this needs to be worth calling.
    ///
    /// Not a preference — a threshold. These are the numbers that decide
    /// whether a call is a play or a throw.
    pub fn quorum(self) -> usize {
        match self {
            // One god farms a lane. That is what a lane is.
            Objective::Farm => 1,
            // Two can take a tower: one to tank it, one to hit it.
            Objective::Push(_) => 2,
            // The boss hits hard enough that three is the floor.
            Objective::Boss => 3,
            // Four, and this is the number that matters most. A titan with
            // three is a lost teamfight next to a structure that shoots.
            Objective::Titan => 4,
            // Defending is the exception: one body that arrives beats three
            // that are still walking, because the tower fights with you.
            Objective::Defend(_) => 1,
        }
    }

    /// Does this objective need everyone in one place?
    ///
    /// Farming is the only one that does not, which is why it is the default
    /// rather than a fallback — it is the objective that does not cost the map.
    pub fn is_grouped(self) -> bool {
        !matches!(self, Objective::Farm)
    }
}

/// Seconds a call holds before it may be reconsidered.
///
/// Long enough to cross the map. Shorter than this and the team re-decides
/// mid-walk and arrives nowhere, which is worse than a wrong call followed
/// through.
pub const CALL_MIN_HOLD: f32 = 6.0;

/// The team's current call.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Call {
    pub objective: Objective,
    /// When it was made, in game seconds.
    pub made_t: f32,
    /// How many gods were available when it was made.
    pub committed: usize,
}

impl Default for Call {
    fn default() -> Self {
        Call { objective: Objective::Farm, made_t: 0.0, committed: 0 }
    }
}

/// What the coordinator needs to know. Deliberately a plain struct of facts
/// rather than a borrow of `Game`: this layer is decided by counting, and
/// keeping it countable is what makes it testable without a match running.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TeamView {
    /// Gods alive and healthy enough to commit.
    pub available: usize,
    /// Gods on the team at all, alive or not.
    pub roster: usize,
    /// Enemy gods currently visible and alive.
    pub enemies_up: usize,
    /// Is the jungle boss alive and takeable?
    pub boss_up: bool,
    /// Enemy titan's health fraction, 0..1. `None` if it is not yet exposed —
    /// a titan behind standing towers is not an objective.
    pub titan_exposed: Option<f32>,
    /// A lane the enemy is pushing hard enough to threaten a tower.
    pub lane_under_threat: Option<Lane>,
    /// The lane with the FEWEST enemy structures still standing -- the one closest to
    /// opening the titan, and so the natural place to push. (The doc said "most" while
    /// nothing filled the struct; `ai::team_view` fills it with the fewest.)
    pub weakest_lane: Option<Lane>,
}

impl Call {
    /// Decide what the team should do now, holding the current call unless it
    /// has expired or become impossible.
    ///
    /// Returns the call to use. Same-objective returns keep the original
    /// `made_t`, so holding is holding rather than a fresh call each tick.
    pub fn decide(self, v: &TeamView, now_t: f32) -> Call {
        let held = now_t - self.made_t;
        let still_possible = self.objective.quorum() <= v.available
            && match self.objective {
                Objective::Boss => v.boss_up,
                Objective::Titan => v.titan_exposed.is_some(),
                _ => true,
            };
        // DEFENDING PRE-EMPTS A HELD CALL. Everything else waits its turn; a
        // tower falling while the team walks to the boss is how a won fight
        // becomes a lost game.
        if let Some(l) = v.lane_under_threat {
            let d = Objective::Defend(l);
            if self.objective != d {
                return Call { objective: d, made_t: now_t, committed: v.available };
            }
        }
        // FARM IS NOT A COMMITMENT, so it never holds. Holding exists to stop
        // the team abandoning a walk halfway; farming is what the team does
        // when it has nowhere to be, and there is nothing to abandon.
        //
        // Without this, `Call::default()` -- Farm at made_t 0.0 -- counted as
        // a call being held, so at game start the team was pinned to Farm for
        // the first six seconds and ignored a titan it could have taken. The
        // default state was masquerading as a decision.
        let committed_to_something = self.objective.is_grouped();
        if committed_to_something && held < CALL_MIN_HOLD && still_possible {
            return self;
        }

        let next = Self::best(v);
        if next == self.objective && still_possible {
            // Same answer: keep the original timestamp so a long-held call is
            // visibly long-held rather than perpetually new.
            self
        } else {
            Call { objective: next, made_t: now_t, committed: v.available }
        }
    }

    /// The best objective the team can actually field, ignoring hysteresis.
    ///
    /// Ordered by what wins games, then filtered by quorum — so the team
    /// reaches for the titan when it can and farms when it cannot, with
    /// nothing in between that it cannot staff.
    pub fn best(v: &TeamView) -> Objective {
        if let Some(l) = v.lane_under_threat {
            return Objective::Defend(l);
        }
        // A titan is worth taking when it is exposed AND the enemy cannot
        // contest: either they are dead, or we outnumber them enough that the
        // fight is ours. Numbers, not bravery.
        if let Some(hp) = v.titan_exposed {
            let uncontested = v.enemies_up + 1 < v.available;
            if Objective::Titan.quorum() <= v.available && (uncontested || hp < 0.35) {
                return Objective::Titan;
            }
        }
        if v.boss_up && Objective::Boss.quorum() <= v.available && v.enemies_up < v.available {
            return Objective::Boss;
        }
        if let Some(l) = v.weakest_lane {
            if Objective::Push(l).quorum() <= v.available {
                return Objective::Push(l);
            }
        }
        Objective::Farm
    }

    /// Should this god follow the call?
    ///
    /// `hp_frac` and `retreating` are the god's own business and they WIN. A
    /// bot walking into a pit at 15% because the team called it is worse than
    /// a bot that ignores calls entirely.
    pub fn binds(&self, hp_frac: f32, retreating: bool) -> bool {
        if retreating || hp_frac < 0.3 {
            return false;
        }
        self.objective.is_grouped()
    }

    /// Seconds this call has been held.
    pub fn age(&self, now_t: f32) -> f32 {
        (now_t - self.made_t).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view(available: usize, enemies_up: usize) -> TeamView {
        TeamView {
            available,
            roster: 5,
            enemies_up,
            boss_up: false,
            titan_exposed: None,
            lane_under_threat: None,
            weakest_lane: Some(Lane::Mid),
        }
    }

    /// THE QUORUM IS THE FEATURE. Calling a titan with two gods loses games.
    #[test]
    fn a_titan_is_not_called_without_the_bodies_for_it() {
        let mut v = view(2, 0);
        v.titan_exposed = Some(0.9);
        assert_ne!(Call::best(&v), Objective::Titan, "two gods is a throw");
        v.available = 4;
        assert_eq!(Call::best(&v), Objective::Titan);
    }

    /// An exposed titan the enemy CAN contest is not free, so numbers decide
    /// rather than eagerness.
    #[test]
    fn a_contested_titan_is_refused_unless_it_is_nearly_dead() {
        let mut v = view(4, 4);
        v.titan_exposed = Some(0.9);
        assert_ne!(Call::best(&v), Objective::Titan, "4v4 at full titan is a coinflip");
        // Nearly dead changes the maths: the race is worth it.
        v.titan_exposed = Some(0.2);
        assert_eq!(Call::best(&v), Objective::Titan);
    }

    /// FARM IS A REAL ANSWER. A team that always has somewhere to be is always
    /// out of position, so an unstaffable map means farm rather than the next
    /// thing down the list.
    #[test]
    fn a_team_with_nobody_available_farms() {
        let v = view(1, 5);
        assert_eq!(Call::best(&v), Objective::Farm);
        assert!(!Objective::Farm.is_grouped(), "farming does not cost the map");
    }

    /// DEFENCE PRE-EMPTS, because a tower falling while the team walks to the
    /// boss turns a won fight into a lost game.
    #[test]
    fn a_threatened_lane_interrupts_a_held_call() {
        let mut v = view(4, 0);
        v.boss_up = true;
        let call = Call::default().decide(&v, 10.0);
        assert_eq!(call.objective, Objective::Boss);
        v.lane_under_threat = Some(Lane::Top);
        // Only one second later -- well inside the hold.
        let next = call.decide(&v, 11.0);
        assert_eq!(next.objective, Objective::Defend(Lane::Top),
                   "defence does not wait for the hold to expire");
    }

    /// Defending needs ONE body, not three: one that arrives beats three still
    /// walking, because the tower fights alongside you.
    #[test]
    fn defending_needs_only_the_body_that_arrives() {
        assert_eq!(Objective::Defend(Lane::Mid).quorum(), 1);
        let mut v = view(1, 3);
        v.lane_under_threat = Some(Lane::Bot);
        assert_eq!(Call::best(&v), Objective::Defend(Lane::Bot));
    }

    /// HYSTERESIS. Re-deciding every tick makes five bots oscillate and arrive
    /// nowhere; a wrong call followed through beats a right one abandoned.
    #[test]
    fn a_call_holds_even_when_a_better_one_appears() {
        let mut v = view(4, 0);
        let pushing = Call::default().decide(&v, 0.0);
        assert_eq!(pushing.objective, Objective::Push(Lane::Mid));
        // The boss comes up two seconds later: better, but not yet.
        v.boss_up = true;
        let held = pushing.decide(&v, 2.0);
        assert_eq!(held.objective, Objective::Push(Lane::Mid), "inside the hold");
        // Past the hold it switches.
        let switched = pushing.decide(&v, CALL_MIN_HOLD + 0.1);
        assert_eq!(switched.objective, Objective::Boss);
    }

    /// An IMPOSSIBLE call is dropped immediately — holding is for better
    /// options, not for objectives that have stopped existing.
    #[test]
    fn a_call_that_became_impossible_is_dropped_inside_the_hold() {
        let mut v = view(4, 0);
        v.boss_up = true;
        let call = Call::default().decide(&v, 0.0);
        assert_eq!(call.objective, Objective::Boss);
        // Someone else took the boss one second in.
        v.boss_up = false;
        let next = call.decide(&v, 1.0);
        assert_ne!(next.objective, Objective::Boss, "a dead boss is not an objective");
    }

    /// Losing the bodies mid-call also makes it impossible.
    #[test]
    fn a_call_is_dropped_when_the_team_dies() {
        let mut v = view(4, 0);
        v.titan_exposed = Some(0.1);
        let call = Call::default().decide(&v, 0.0);
        assert_eq!(call.objective, Objective::Titan);
        v.available = 1;
        assert_ne!(call.decide(&v, 1.0).objective, Objective::Titan);
    }

    /// SELF-PRESERVATION OUTRANKS THE CALL. A bot at 15% walking into a pit
    /// because the team called it is worse than one that ignores calls.
    #[test]
    fn a_call_does_not_bind_a_god_who_should_be_retreating() {
        let v = {
            let mut v = view(4, 0);
            v.titan_exposed = Some(0.1);
            v
        };
        let call = Call::default().decide(&v, 0.0);
        assert!(call.binds(0.9, false), "a healthy god commits");
        assert!(!call.binds(0.15, false), "a nearly-dead one does not");
        assert!(!call.binds(0.9, true), "and Retreat stays local");
    }

    /// Farming binds nobody anywhere, which is what makes it the default
    /// rather than an instruction.
    #[test]
    fn farming_never_binds() {
        let call = Call { objective: Objective::Farm, made_t: 0.0, committed: 5 };
        assert!(!call.binds(1.0, false));
    }

    /// THE DEFAULT STATE IS NOT A DECISION. `Call::default()` is Farm at
    /// made_t 0.0, and treating that as a held call pinned the team to farming
    /// for the first six seconds of the match -- ignoring a titan it could
    /// have taken, at exactly the moment a fresh team is grouped.
    ///
    /// Five tests failed on this and all five were this one rule. Written
    /// before the module ever ran, which is how it shipped.
    #[test]
    fn farming_never_holds_so_the_opening_is_not_wasted() {
        let mut v = view(4, 0);
        v.titan_exposed = Some(0.1);
        // t = 0, straight from default: inside any hold window.
        let call = Call::default().decide(&v, 0.0);
        assert_eq!(call.objective, Objective::Titan,
                   "a fresh team must be able to act immediately");
    }

    /// ...but a real commitment still holds, or the fix would have removed
    /// hysteresis altogether.
    #[test]
    fn a_grouped_call_still_holds_after_the_fix() {
        let mut v = view(4, 0);
        let pushing = Call::default().decide(&v, 0.0);
        assert!(pushing.objective.is_grouped());
        v.boss_up = true;
        assert_eq!(pushing.decide(&v, 1.0).objective, pushing.objective,
                   "a commitment is not abandoned one second in");
    }

    /// A held call keeps its timestamp, so "held for 9 seconds" is readable
    /// rather than perpetually zero.
    #[test]
    fn holding_preserves_the_original_time() {
        let v = view(4, 0);
        let first = Call::default().decide(&v, 5.0);
        let again = first.decide(&v, 9.0);
        assert_eq!(again.made_t, first.made_t);
        assert!((again.age(9.0) - 4.0).abs() < 1e-6);
    }
}
