//! THE APPROACH — how you get in, judged against a clock.
//!
//! `foothold.rs` asks a good question in a Cybertronian accent: two ways into
//! a place that trade against each other, a clock that may or may not be
//! running, and no answer that is right in general. Clear an LZ — fast,
//! cheap, exposed the whole time, and the ship has to stay. Seize a fortress
//! — slow, expensive, and the sky stops mattering the moment you are inside.
//!
//! The detective campaign asks exactly the same question and none of the same
//! nouns. There is no weather, no habitability, no ship to pin. There is a
//! rival society that will arrive, a back door that takes longer than the
//! front, and a car parked somewhere it can be seen.
//!
//! So this is the question with the nouns removed, and it exists because
//! there are two real callers rather than because two callers might appear:
//! `foothold::assess` delegates its arithmetic here, and
//! `tradecraft::infiltrate` is the same decision in 1998.
//!
//! **What is generic and what is not.** Reaching the place, and whether the
//! place shelters you from THIS threat, are domain questions — a bunker stops
//! a particle front, a service corridor stops a patrol, and nothing here
//! knows the difference. The callers answer those and hand over numbers. What
//! is generic is the arithmetic that follows, and one judgement inside it:
//!
//! **THE RANKING FLIPS UNDER PRESSURE.** With a clock running, take the most
//! margin; a clock you can beat is the only thing that matters. With no
//! clock, take the CHEAPEST way in, because a fight you did not need is a
//! cost with nothing bought. `foothold`'s own comment records getting this
//! backwards once — it ranked "does not pin the ship" above everything, so
//! the fortress always won and the LZ was never worth having. Lingering costs
//! break ties; they do not decide.

/// One way in, with the domain already resolved to numbers.
#[derive(Clone, Copy, Debug)]
pub struct Opening {
    /// Seconds left on the clock once you are there, before doing anything.
    /// Negative means you cannot get there in time at all.
    pub reach_spare_s: f32,
    /// Seconds of work to make the place yours.
    pub take_s: f32,
    /// Is the whole of that work done in the open? An LZ is exposed from
    /// first boot; a fortress only on the approach.
    pub exposed_during_take: bool,
    /// Once it is yours, does it actually stop the thing the clock measures?
    /// A roof that does not stop a particle front is not shelter.
    pub shelters_after: bool,
    /// A cost that lasts beyond the opening — a grounded ship, a car parked
    /// in view. Real, and never the deciding factor.
    pub lingering_cost: bool,
    /// How expensive the way in is when nothing is chasing you.
    pub effort: f32,
    /// A cost that is NOT time, in seconds-equivalent, and that counts even
    /// under pressure.
    ///
    /// The raid domain has no use for this and passes zero: weather does not
    /// care who sees you, so time is the only currency and the fastest
    /// viable option is the best one. The detective campaign's principal
    /// cost IS being seen, and modelling it as effort was wrong — effort only
    /// applies when nothing is chasing you, so the front door could never
    /// win and could never lose. It was a dead option in a module whose whole
    /// point is that every option is right somewhere.
    pub seen_cost: f32,
}

/// The fraction of the take that happens in the open when you are not exposed
/// throughout. Not zero: you are still outside on the way in.
const SHELTERED_FRACTION: f32 = 0.25;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Verdict {
    /// Viable, with this many seconds to spare.
    Take { margin_s: f32 },
    /// You cannot even get there inside the window.
    Unreachable { margin_s: f32 },
    /// You get there, and the work outlasts the clock.
    CaughtInTheOpen { margin_s: f32 },
    /// You take it in time and it does not stop what is coming.
    NoShelter { margin_s: f32 },
}

impl Verdict {
    pub fn viable(self) -> bool {
        matches!(self, Verdict::Take { .. })
    }
    pub fn margin_s(self) -> f32 {
        match self {
            Verdict::Take { margin_s }
            | Verdict::Unreachable { margin_s }
            | Verdict::CaughtInTheOpen { margin_s }
            | Verdict::NoShelter { margin_s } => margin_s,
        }
    }
    /// Why it was rejected, in the caller's own register-free words.
    pub fn note(self) -> &'static str {
        match self {
            Verdict::Take { .. } => "",
            Verdict::Unreachable { .. } => "cannot reach it in the window",
            Verdict::CaughtInTheOpen { .. } => "the work outlasts the window",
            Verdict::NoShelter { .. } => "taken in time, and it does not stop what is coming",
        }
    }
}

/// Judge one opening. `window_s` is infinite when nothing is coming.
pub fn assess(o: &Opening, window_s: f32) -> Verdict {
    if o.reach_spare_s < 0.0 {
        return Verdict::Unreachable { margin_s: o.reach_spare_s };
    }
    let exposed = if o.exposed_during_take { o.take_s } else { o.take_s * SHELTERED_FRACTION };
    let margin_s = o.reach_spare_s - exposed;
    if margin_s < 0.0 {
        return Verdict::CaughtInTheOpen { margin_s };
    }
    // an unpressured clock cannot punish you for not sheltering from it
    if !o.shelters_after && window_s.is_finite() {
        return Verdict::NoShelter { margin_s };
    }
    Verdict::Take { margin_s }
}

/// Pick the best way in, as an index into `openings`.
///
/// See the module doc: the ranking flips under pressure, and lingering costs
/// only break ties.
pub fn choose(openings: &[Opening], window_s: f32) -> Option<usize> {
    let pressured = window_s.is_finite();
    openings
        .iter()
        .enumerate()
        .filter(|(_, o)| assess(o, window_s).viable())
        .max_by(|(_, a), (_, b)| {
            let key = |o: &Opening| {
                let free = if o.lingering_cost { 0.0 } else { 1.0 };
                if pressured {
                    // margin NET of what it costs you to be noticed. With
                    // seen_cost zero this is the raid's original ranking
                    // exactly, which is why foothold's tests are unchanged.
                    (assess(o, window_s).margin_s() - o.seen_cost, free)
                } else {
                    (-o.effort, free)
                }
            };
            key(a).partial_cmp(&key(b)).unwrap()
        })
        .map(|(i, _)| i)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn open(reach: f32, take: f32, exposed: bool) -> Opening {
        Opening {
            reach_spare_s: reach,
            take_s: take,
            exposed_during_take: exposed,
            shelters_after: true,
            lingering_cost: false,
            effort: take,
            seen_cost: 0.0,
        }
    }

    #[test]
    fn shelter_during_the_take_is_the_real_difference() {
        // 100 s of clock, 90 s of work. Exposed throughout it is close;
        // sheltered after the door it is comfortable. This is the trade the
        // whole module exists to express.
        let exposed = assess(&open(100.0, 90.0, true), 100.0);
        let inside = assess(&open(100.0, 90.0, false), 100.0);
        assert!(exposed.viable() && inside.viable());
        assert!(inside.margin_s() > exposed.margin_s());
        // and past the line, only the sheltered one survives
        assert!(!assess(&open(100.0, 130.0, true), 100.0).viable());
        assert!(assess(&open(100.0, 130.0, false), 100.0).viable());
    }

    #[test]
    fn the_ways_to_fail_are_told_apart() {
        assert!(matches!(
            assess(&open(-5.0, 10.0, true), 60.0),
            Verdict::Unreachable { .. }
        ));
        assert!(matches!(
            assess(&open(10.0, 900.0, true), 60.0),
            Verdict::CaughtInTheOpen { .. }
        ));
        let mut o = open(100.0, 10.0, false);
        o.shelters_after = false;
        assert!(matches!(assess(&o, 60.0), Verdict::NoShelter { .. }));
        // ... and with nothing coming, not sheltering is not a problem
        assert!(assess(&o, f32::INFINITY).viable());
        for v in [
            assess(&open(-5.0, 10.0, true), 60.0),
            assess(&open(10.0, 900.0, true), 60.0),
            assess(&o, 60.0),
        ] {
            assert!(!v.note().is_empty(), "a rejection must say why");
        }
    }

    #[test]
    fn under_pressure_margin_wins_and_otherwise_cheapness_does() {
        // the flip, and the bug foothold's comment records: ranking the
        // lingering cost first made the expensive option always win
        let cheap_tight = Opening { effort: 10.0, ..open(100.0, 95.0, true) };
        let dear_roomy = Opening { effort: 900.0, ..open(100.0, 20.0, false) };
        let set = [cheap_tight, dear_roomy];

        // clock running: take the one with room
        assert_eq!(choose(&set, 100.0), Some(1));
        // nothing coming: take the one that costs least
        assert_eq!(choose(&set, f32::INFINITY), Some(0));
    }

    #[test]
    fn a_lingering_cost_breaks_ties_and_never_decides() {
        let free = Opening { lingering_cost: false, ..open(100.0, 20.0, false) };
        let pinned = Opening { lingering_cost: true, ..open(100.0, 20.0, false) };
        // identical otherwise: the free one wins
        assert_eq!(choose(&[pinned, free], 100.0), Some(1));
        // but a pinned option with real margin beats a free one without
        let free_tight = Opening { lingering_cost: false, ..open(100.0, 99.0, true) };
        assert_eq!(choose(&[free_tight, pinned], 100.0), Some(1));
    }

    #[test]
    fn being_seen_costs_even_when_the_clock_is_running() {
        // the axis the raid domain does not have. Both fit the window; the
        // fast one is noticed and the slow one is not, and with enough room
        // the slow one is right.
        // reach_spare tracks the window, or the fixture is testing a
        // building you can stand outside of forever
        let pair = |w: f32| {
            [
                Opening { seen_cost: 300.0, ..open(w, 20.0, true) },
                Opening { seen_cost: 0.0, ..open(w, 150.0, false) },
            ]
        };
        assert_eq!(choose(&pair(600.0), 600.0), Some(1), "with room, take the quiet one");

        // squeeze until only the loud one still fits, and it wins by being
        // the only thing left — an option that is never correct is an option
        // that should not be in the list
        assert_eq!(choose(&pair(30.0), 30.0), Some(0));
    }

    #[test]
    fn a_zero_seen_cost_is_the_raids_original_ranking() {
        // foothold passes zero for every site, so this must be a no-op there
        let a = Opening { seen_cost: 0.0, ..open(100.0, 20.0, false) };
        let b = Opening { seen_cost: 0.0, ..open(100.0, 60.0, false) };
        assert_eq!(choose(&[a, b], 100.0), Some(0), "most margin still wins");
    }

    #[test]
    fn nothing_viable_is_no_answer_rather_than_a_bad_one() {
        let hopeless = [open(5.0, 500.0, true), open(-1.0, 1.0, true)];
        assert_eq!(choose(&hopeless, 30.0), None);
        assert_eq!(choose(&[], 30.0), None);
    }
}
