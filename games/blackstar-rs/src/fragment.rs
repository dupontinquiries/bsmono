//! FRAGMENTS OF THE LATTICE — what you take off the Anvil, and what it buys.
//!
//! `raidcon`'s twentieth boss is the Lattice, and `docs/films/CANON_DECISIONS.md`
//! §4 settles what a fragment is: unbounded local voxel authority, which is
//! `compact.rs`'s `Style::Grow` curve held at peak instead of relaxing to zero.
//! `superform.rs` derives the bodies. Nothing connected the three.
//!
//! ## Not a `loot::Mod`, and that is the point
//!
//! `loot.rs` pays out survival mods: each one buys exactly one `Effect` on the
//! envelope, and the whole design rests on slots making a loadout a choice. A
//! fragment is not an effect on a body — it is a temporary suspension of the
//! rules the body is built under. Putting it in the mod table would either
//! give it an `Effect` it does not have, or add a variant that means "not
//! really an effect", and the second is how a clean table stops being one.
//!
//! ## Dispossession, carried through
//!
//! The Anvil's shard line is *"you do not damage it. you take cells it no
//! longer governs."* So fragments are not a drop roll — they are a COUNT OF
//! WHAT YOU PRISED OUT, and [`taken`] is that arithmetic. A raid that went
//! badly yields fewer because less was removed, not because a die came up
//! short. Nothing else in the game drops this way, which is correct: nothing
//! else is fought by subtraction.
//!
//! ## The state is separate, deliberately
//!
//! Canon is explicit that super mode wants to be a separate state rather than
//! a parameter on the fold, because `compact.rs` is only safe to layer onto
//! the shipped morph while its curve returns to zero at both ends and a body
//! at rest stays bit-identical to what it was. [`Charge`] is that state, and
//! it holds a clock rather than a flag: a fragment is spent on entry and time
//! does the rest, so there is no path where the peak is held indefinitely.

use crate::superform;
use crate::weapons::Element;

/// The only boss that yields fragments. A raid id, matching
/// `raidcon::BOSSES`.
pub const SOURCE: &str = "the_anvil";

/// Seconds a fragment holds the curve at peak.
pub const HOLD_S: f32 = 24.0;

/// Seconds before another fragment can be spent. Longer than the hold, so a
/// second fragment cannot chain straight off the first — two fragments buy two
/// moments, not one long one.
pub const COOLDOWN_S: f32 = 90.0;

/// How many cells have to be prised out before one is worth carrying home.
pub const CELLS_PER_FRAGMENT: u32 = 250;

/// Fragments earned from an Anvil run.
///
/// `cells` is what the fight actually removed from the boss. This is
/// deliberately not a roll: dispossession is the damage model, so the payout
/// IS the damage, and a caller that wants a random drop is looking at the
/// wrong boss.
pub fn taken(boss_id: &str, cells: u32) -> u32 {
    if boss_id != SOURCE {
        return 0;
    }
    cells / CELLS_PER_FRAGMENT
}

/// Whether a body is currently running on a fragment.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum State {
    /// Ordinary rules.
    Bounded,
    /// Curve held at peak; `left` seconds remain.
    Super { left: f32 },
}

/// Why a fragment could not be spent. Distinct so a UI can say which.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// None carried.
    Empty,
    /// Already running on one.
    AlreadySuper,
    /// Too soon after the last.
    Cooling,
}

/// A body's fragment inventory and current state.
#[derive(Clone, Copy, Debug)]
pub struct Charge {
    held: u32,
    state: State,
    cooldown: f32,
}

impl Default for Charge {
    fn default() -> Charge {
        Charge { held: 0, state: State::Bounded, cooldown: 0.0 }
    }
}

impl Charge {
    pub fn with(held: u32) -> Charge {
        Charge { held, ..Charge::default() }
    }
    pub fn held(&self) -> u32 {
        self.held
    }
    pub fn state(&self) -> State {
        self.state
    }
    pub fn is_super(&self) -> bool {
        matches!(self.state, State::Super { .. })
    }
    pub fn cooldown(&self) -> f32 {
        self.cooldown
    }

    /// Take fragments home from a raid.
    pub fn collect(&mut self, boss_id: &str, cells: u32) -> u32 {
        let n = taken(boss_id, cells);
        self.held += n;
        n
    }

    /// Spend one.
    pub fn spend(&mut self) -> Result<(), Refusal> {
        if self.is_super() {
            return Err(Refusal::AlreadySuper);
        }
        if self.cooldown > 0.0 {
            return Err(Refusal::Cooling);
        }
        if self.held == 0 {
            return Err(Refusal::Empty);
        }
        self.held -= 1;
        self.state = State::Super { left: HOLD_S };
        self.cooldown = COOLDOWN_S;
        Ok(())
    }

    /// Advance. Time is the ONLY way out of the peak — there is no release
    /// call, because a caller able to end it early is a caller able to hold it
    /// late, and `compact.rs`'s safety argument depends on the curve coming
    /// back down.
    pub fn tick(&mut self, dt: f32) {
        let dt = dt.max(0.0);
        self.cooldown = (self.cooldown - dt).max(0.0);
        if let State::Super { left } = self.state {
            let left = left - dt;
            self.state = if left > 0.0 { State::Super { left } } else { State::Bounded };
        }
    }

    /// The body to draw right now: the super derivation while a fragment is
    /// burning, the ordinary clone otherwise.
    pub fn body(&self, e: Element) -> Vec<glam::Vec3> {
        if self.is_super() {
            superform::model(e)
        } else {
            crate::clones::model(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_anvil_yields_fragments() {
        assert_eq!(taken(SOURCE, 1000), 4);
        assert_eq!(taken("speakerman", 1000), 0, "another boss paid out fragments");
        assert_eq!(taken(SOURCE, CELLS_PER_FRAGMENT - 1), 0, "a partial cell count paid out");
    }

    #[test]
    fn the_payout_is_the_damage_rather_than_a_roll() {
        // Dispossession as the drop table: a worse run yields strictly less,
        // and the same run always yields the same. Nothing here consults a
        // seed, which is the difference from `loot::roll`.
        let (bad, good) = (taken(SOURCE, 600), taken(SOURCE, 2400));
        assert!(bad < good);
        assert_eq!(taken(SOURCE, 2400), taken(SOURCE, 2400));
    }

    #[test]
    fn spending_needs_one_and_consumes_it() {
        let mut c = Charge::default();
        assert_eq!(c.spend(), Err(Refusal::Empty));
        c.collect(SOURCE, 500);
        assert_eq!(c.held(), 2);
        c.spend().unwrap();
        assert_eq!(c.held(), 1);
        assert!(c.is_super());
    }

    #[test]
    fn a_second_fragment_cannot_chain_off_the_first() {
        // Two fragments buy two moments, not one long one. The cooldown is
        // longer than the hold precisely so the peak cannot be sustained by
        // stockpiling.
        let mut c = Charge::with(5);
        c.spend().unwrap();
        c.tick(HOLD_S + 0.1);
        assert!(!c.is_super(), "the hold did not expire");
        assert_eq!(c.spend(), Err(Refusal::Cooling));
        c.tick(COOLDOWN_S);
        assert!(c.spend().is_ok(), "still refusing after a full cooldown");
    }

    #[test]
    fn spending_while_super_is_refused_distinctly() {
        let mut c = Charge::with(3);
        c.spend().unwrap();
        assert_eq!(c.spend(), Err(Refusal::AlreadySuper), "a UI cannot tell the player why");
    }

    #[test]
    fn time_is_the_only_way_out_of_the_peak() {
        // There is no release() on purpose: a caller able to end it early is a
        // caller able to hold it late, and compact.rs's safety argument
        // depends on the curve coming back down.
        let mut c = Charge::with(1);
        c.spend().unwrap();
        for _ in 0..1000 {
            c.tick(0.1);
        }
        assert_eq!(c.state(), State::Bounded, "the peak outlived its clock");
        assert_eq!(c.cooldown(), 0.0);
    }

    #[test]
    fn the_body_swaps_with_the_state_and_swaps_back() {
        // superform's caller, finally. The point is that the body at rest is
        // the ORDINARY one before and after — the same property compact.rs
        // needs from the fold.
        let mut c = Charge::with(1);
        let before = c.body(Element::Rock).len();
        c.spend().unwrap();
        let during = c.body(Element::Rock).len();
        c.tick(HOLD_S + 1.0);
        let after = c.body(Element::Rock).len();
        assert_ne!(before, during, "the fragment changed nothing about the body");
        assert_eq!(before, after, "the body did not come back to what it was");
    }

    #[test]
    fn a_super_body_is_still_its_own_clone_here_too() {
        // superform asserts this against profiles; asserted again at the point
        // of use, because this is where a wrong element would actually reach a
        // player.
        let mut c = Charge::with(1);
        c.spend().unwrap();
        for e in [Element::Rock, Element::Ice, Element::Fire] {
            assert_eq!(c.body(e), superform::model(e), "{e:?} drew the wrong super body");
        }
    }

    #[test]
    fn ticking_backwards_or_by_zero_does_nothing_strange() {
        let mut c = Charge::with(1);
        c.spend().unwrap();
        c.tick(-5.0);
        c.tick(0.0);
        assert!(c.is_super(), "negative time released the fragment");
    }
}
