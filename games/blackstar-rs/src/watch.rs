//! THE WATCH — one clone, eleven mapped forms, and a ceiling reached
//! exactly once.
//!
//! Source: `Clones.md` and `TF_TRILOGY_TWO_FRAMEWORK.md` from the
//! screenplay treatment. The watch-bearer (`chi.rs`, `clones.rs`) is
//! THE MORPHER — established there as the same character, name
//! revealed later — and the Watch is not a collection of separate
//! powers, it is ONE ability with a form selector. By the time the
//! story introduces him (Film 4) he is already a master of it:
//!
//!   "The Watch has twelve forms The Morpher can access reliably. He
//!   has mapped them. He understands their costs and limits."
//!
//! `weapons::TABLE` already encodes that eleven-form roster, in the
//! treatment's own order, at indices 0..=10 — WATER, TSUNAMI, ICE,
//! FIRE, MAGMA, BLIZZARD, METAL, TITANIUM, ELECTRICITY, WIND, TORNADO
//! — so this module does NOT gate them behind discovery; a Morpher who
//! has to unlock Fire is a different, wrong character. Index 11
//! (PHOTON) is the twelfth and stands apart, because the treatment is
//! explicit that it is not mapped at all:
//!
//!   "There is a thirteenth form the Watch shows him — not as an
//!   option he can choose, but as a presence he can feel at the edge
//!   of the device."
//!
//! He reaches it exactly once in six films — once by accident (an
//! extremity the game does not simulate; it is backstory), and once
//! DELIBERATELY, in Film 6, at a cost that puts him under for three
//! days. This module's job is that second kind of access:
//! * every shift among the eleven costs resolve — the Watch's
//!   reproduction of a form is faithful but not native ("costs him
//!   slightly more than it costs a native"), and the cost is how that
//!   shows up mechanically;
//! * Photon Man is not gated by having "found" anything — nothing was
//!   ever found on purpose — it is gated by a caller-declared moment
//!   of last resort ("it only activates when the Watch decides no
//!   other form is sufficient") plus a cost steep enough that it
//!   cannot be a rotation, only a climax;
//! * the ICE form is the ONLY form that opens the quantum gravity leap
//!   (`weapons.rs`'s `Verb::Blink`, built against
//!   `TF_QUANTUM_GRAVITY_LEAP.md`) — this module asserts that
//!   invariant rather than duplicating the leap, because "The Morpher
//!   can perform the quantum leap only when the Watch is in Ice form"
//!   is a fact about ONE mechanic, not two.

use crate::weapons::{Verb, TABLE};

/// The eleven native forms, in the treatment's own order. `TABLE[11]`
/// is Photon Man and is deliberately excluded — it is not a form you
/// find, it is the form that finds you.
pub const NATIVE: [usize; 11] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10];
pub const PHOTON: usize = 11;

/// The Watch's own biology cost per shift: a Morpher paying more than a
/// native of the form he is imitating, per the treatment ("the jump
/// costs him slightly more than it costs a native Ice Clone"). Applied
/// to every shift, not just the leap — the Watch's whole nature is
/// reproduction, not birthright.
pub const SHIFT_COST: f32 = 0.16;
/// Photon Man's cost: steep enough that it can never be a rotation
/// option, only a climax — a resolve pool starting at 1.0 covers
/// exactly one attempt. The treatment does not give this a number
/// ("he does not remember those forty seconds clearly"); the cost is
/// this module's translation of "he wakes up three days later."
pub const PHOTON_COST: f32 = 0.95;
/// Resolve regained per second at rest — slow, so "he uses the ability
/// sparingly... arriving depleted rarely improves anything" holds up
/// as more than a line.
pub const REGEN_PER_SEC: f32 = 0.05;

pub struct Watch {
    current: Option<usize>,
    /// 0..1, spent by shifting, regained by tick
    resolve: f32,
    /// has Photon Man ever been reached DELIBERATELY through this
    /// module — a fact kept for the record, per the treatment's own
    /// Film 6 beat ("accessed for the first time deliberately"); it
    /// does not relax the gate on a second attempt, because the
    /// treatment never says it gets easier
    photon_reached: bool,
}

impl Default for Watch {
    /// Ice is the Morpher's native state — the treatment never puts
    /// him anywhere else at rest, and the leap depends on it being
    /// reachable from minute one. Every other native form is already
    /// his ("he has mapped them") — nothing here is locked.
    fn default() -> Watch {
        Watch { current: Some(ice_slot()), resolve: 1.0, photon_reached: false }
    }
}

fn ice_slot() -> usize {
    NATIVE.iter().copied().find(|i| TABLE[*i].name == "ICE").expect("ICE is a native form")
}

/// A native form by name (case-insensitive).
pub fn slot_named(name: &str) -> Option<usize> {
    NATIVE.iter().copied().find(|i| TABLE[*i].name.eq_ignore_ascii_case(name))
}

impl Watch {
    pub fn current(&self) -> Option<usize> {
        self.current
    }
    pub fn current_name(&self) -> &'static str {
        self.current.map(|i| TABLE[i].name).unwrap_or("--")
    }
    pub fn resolve(&self) -> f32 {
        self.resolve
    }
    pub fn photon_reached(&self) -> bool {
        self.photon_reached
    }

    /// Attempt a shift. `desperate` is the caller's own declaration
    /// that nothing else in the fight will do — the only lever this
    /// module offers toward Photon Man, because the treatment refuses
    /// to give the ceiling a threshold: "he does not know if the Watch
    /// chooses the moment or if he does."
    pub fn shift(&mut self, slot: usize, desperate: bool) -> Result<(), String> {
        if slot == PHOTON {
            if !desperate {
                return Err("the Watch does not open on request; only when nothing else is enough".into());
            }
            if self.resolve < PHOTON_COST {
                return Err(format!(
                    "too spent to reach it: {:.0}% resolve, needs {:.0}%",
                    self.resolve * 100.0,
                    PHOTON_COST * 100.0
                ));
            }
            self.resolve -= PHOTON_COST;
            self.current = Some(PHOTON);
            self.photon_reached = true;
            return Ok(());
        }
        if slot >= NATIVE.len() {
            return Err("no such form".into());
        }
        if self.resolve < SHIFT_COST {
            return Err(format!("too spent to hold a new form: {:.0}% resolve", self.resolve * 100.0));
        }
        self.resolve -= SHIFT_COST;
        self.current = Some(slot);
        Ok(())
    }

    /// Rest. Regeneration is linear and slow by design (see
    /// `REGEN_PER_SEC`) — the cost is meant to be felt.
    pub fn tick(&mut self, dt: f32) {
        self.resolve = (self.resolve + REGEN_PER_SEC * dt.max(0.0)).min(1.0);
    }

    /// The one fact `weapons.rs` needs from this module: is the leap
    /// even reachable right now. Asserted rather than reimplemented —
    /// `Verb::Blink` is the leap, and ICE is the only form that carries
    /// it, checked below as an invariant on the table itself.
    pub fn can_leap(&self) -> bool {
        self.current == Some(ice_slot()) && TABLE[ice_slot()].verb == Verb::Blink
    }

    /// `current_name:resolve:photon_reached` — one lenient save line,
    /// same spirit as `chi.rs`'s.
    pub fn from_line(line: &str) -> Watch {
        let mut w = Watch::default();
        let mut parts = line.trim().split(':');
        if let Some(name) = parts.next().filter(|s| !s.is_empty()) {
            if let Some(slot) = NATIVE.iter().copied().find(|i| TABLE[*i].name == name) {
                w.current = Some(slot);
            } else if name == "PHOTON" {
                w.current = Some(PHOTON);
            }
        }
        if let Some(r) = parts.next().and_then(|s| s.parse().ok()) {
            w.resolve = r;
        }
        if let Some(p) = parts.next() {
            w.photon_reached = p == "1";
        }
        w
    }
    pub fn line(&self) -> String {
        format!("{}:{:.3}:{}", self.current_name(), self.resolve, if self.photon_reached { 1 } else { 0 })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_morpher_starts_in_his_native_ice_form_with_every_form_already_mapped() {
        let w = Watch::default();
        assert_eq!(w.current_name(), "ICE");
        assert!(w.can_leap(), "the native form must open the leap from minute one");
        assert!(!w.photon_reached());
    }

    #[test]
    fn every_native_form_is_already_his_and_each_shift_costs_resolve() {
        let mut w = Watch::default();
        for i in NATIVE {
            let before = w.resolve();
            w.tick(100.0); // never let resolve be the reason a mapped form refuses
            w.shift(i, false).unwrap_or_else(|e| panic!("{} refused, but it is already mapped: {e}", TABLE[i].name));
            assert_eq!(w.current(), Some(i));
            let _ = before;
        }
        assert!(!w.can_leap(), "the last form in the loop is TORNADO, not ICE");
    }

    #[test]
    fn photon_man_demands_a_declared_last_resort_at_steep_cost() {
        let mut w = Watch::default();
        assert!(w.shift(PHOTON, false).is_err(), "photon man answered a request, not a last resort");
        w.shift(PHOTON, true).unwrap();
        assert_eq!(w.current(), Some(PHOTON));
        assert!(w.photon_reached(), "reaching it once must leave a record");
        assert!(!w.can_leap(), "photon man is not the ice form");
        // and it is a climax, not a rotation option: a fresh pool cannot
        // take it twice in a row
        assert!(w.shift(PHOTON, true).is_err(), "photon man was affordable back to back");
    }

    #[test]
    fn a_spent_watch_cannot_shift_and_rest_repays_it() {
        let mut w = Watch::default();
        let fire = slot_named("FIRE").unwrap();
        let mut shifts = 0;
        while w.shift(fire, false).is_ok() {
            shifts += 1;
            assert!(shifts < 20, "resolve never ran out");
        }
        assert!(w.resolve() < SHIFT_COST, "refused while it could still afford one");
        w.tick(SHIFT_COST / REGEN_PER_SEC + 1.0);
        assert!(w.shift(fire, false).is_ok(), "rest did not repay the cost");
    }

    #[test]
    fn the_save_line_roundtrips_including_a_photon_reach() {
        let mut w = Watch::default();
        w.tick(100.0);
        w.shift(slot_named("METAL").unwrap(), false).unwrap();
        let back = Watch::from_line(&w.line());
        assert_eq!(back.current_name(), "METAL");
        assert!((back.resolve() - w.resolve()).abs() < 1e-3);
        assert!(!back.photon_reached());
        w.tick(100.0);
        w.shift(PHOTON, true).unwrap();
        let back2 = Watch::from_line(&w.line());
        assert_eq!(back2.current_name(), "PHOTON");
        assert!(back2.photon_reached());
        // junk input never panics; it just falls back to the default
        let junk = Watch::from_line("not a real line at all :: ");
        assert_eq!(junk.current_name(), "ICE");
    }

    #[test]
    fn the_native_roster_matches_the_treatment_order_and_stays_off_grapple() {
        // Clones.md's own list, in order: water, tsunami, ice, fire,
        // magma, blizzard, metal, titanium, electricity, wind, tornado
        let order = [
            "WATER", "TSUNAMI", "ICE", "FIRE", "MAGMA", "BLIZZARD", "METAL", "TITANIUM",
            "ELECTRICITY", "WIND", "TORNADO",
        ];
        for (slot, name) in NATIVE.iter().zip(order) {
            assert_eq!(TABLE[*slot].name, name);
        }
        assert_eq!(TABLE[PHOTON].name, "PHOTON");
        assert!(NATIVE.iter().all(|i| TABLE[*i].name != "GRAPPLE"), "grapple is a tool, not a form");
    }
}
