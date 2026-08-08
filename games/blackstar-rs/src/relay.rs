//! RELAY — one carrier, several receivers, and everyone else running
//! reduced.
//!
//! The homage is Download Play: one cartridge in one machine, everybody else
//! booting a cut-down copy over the air. It was a compromise forced by
//! hardware and it produced a genuinely good asymmetry — the host had the
//! content and the guests had enough of it to play, and the game was
//! designed around that gap rather than apologising for it.
//!
//! Translated: one squad member carries the ARRAY — the scanning suite that
//! reads the raid — and shares a degraded picture with everybody else. The
//! gap is the mechanic.
//!
//! **It works because degradation here is not a debuff, it is a different
//! view.** The carrier sees range and identity; a receiver at distance sees
//! bearing and motion but not what the thing is. So the carrier is not
//! "better", they are the only one who can answer a specific question — and
//! they are also slow, loud and worth killing.
//!
//! It also fits the architecture the design note describes: the array does
//! not know what a raid is. It is handed a distance and a link budget and it
//! returns a fidelity, and everything above it decides what that means.

/// What a member can see, given how far they are from the carrier.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Fidelity {
    /// nothing but your own eyes
    Dark,
    /// something is out there, and roughly where
    Bearing,
    /// where, and how fast it is moving
    Track,
    /// where, how fast, and WHAT
    Identity,
    /// everything the array has, including what it is about to do
    Full,
}

impl Fidelity {
    pub fn label(self) -> &'static str {
        match self {
            Fidelity::Dark => "DARK",
            Fidelity::Bearing => "BEARING ONLY",
            Fidelity::Track => "TRACKING",
            Fidelity::Identity => "IDENTIFIED",
            Fidelity::Full => "FULL ARRAY",
        }
    }
    /// Can this view answer "what is it"? The question the carrier exists
    /// for, and the reason a squad splits up reluctantly.
    pub fn identifies(self) -> bool {
        self >= Fidelity::Identity
    }
}

/// The carrier's suite. Range is in metres; a bigger array reaches further
/// and costs more to carry.
#[derive(Clone, Copy, Debug)]
pub struct Array {
    /// metres at which the link falls to Bearing
    pub range_m: f32,
    /// how much the carrier is slowed, 0 none .. 1 halted
    pub burden: f32,
    /// how much louder it makes them to anything listening, 1 = normal
    pub signature: f32,
}

impl Default for Array {
    fn default() -> Array {
        Array { range_m: 260.0, burden: 0.28, signature: 2.4 }
    }
}

/// THE LINK BUDGET, which is the only thing this layer knows.
///
/// Signal falls with the square of distance, so fidelity does not degrade
/// linearly — it holds up well and then collapses, which is what makes a
/// squad's spread a real decision rather than a slider. Obstruction is a
/// straight multiplier: rock between you and the carrier costs the same as
/// distance.
pub fn fidelity(a: &Array, distance_m: f32, obstruction: f32) -> Fidelity {
    if distance_m < 0.0 {
        return Fidelity::Dark;
    }
    // TOTAL OBSTRUCTION IS TOTAL. The floor of 0.02 exists so the maths does
    // not divide by zero, and it let a member five metres behind solid rock
    // keep a Track — the floor was doing the work of a physical claim it
    // should not make. Fully blocked is Dark at any range.
    if obstruction >= 1.0 {
        return Fidelity::Dark;
    }
    let clear = (1.0 - obstruction.clamp(0.0, 1.0)).max(0.02);
    let d = distance_m / (a.range_m.max(1.0) * clear);
    // inverse square, normalised so d = 1 is the edge of Bearing
    let strength = 1.0 / (1.0 + d * d);
    if strength > 0.90 {
        Fidelity::Full
    } else if strength > 0.62 {
        Fidelity::Identity
    } else if strength > 0.30 {
        Fidelity::Track
    } else if strength > 0.11 {
        Fidelity::Bearing
    } else {
        Fidelity::Dark
    }
}

/// What carrying it costs.
pub fn carrier_speed(a: &Array, base_ms: f32) -> f32 {
    base_ms * (1.0 - a.burden.clamp(0.0, 0.9))
}

/// How far away a listener can hear the carrier, against a baseline.
pub fn heard_at_m(a: &Array, base_m: f32) -> f32 {
    base_m * a.signature.max(0.1)
}

/// A squad's coverage: the worst view anybody has.
///
/// The minimum rather than the mean, for the reason the combiner taught —
/// a squad is as informed as its blindest member, and averaging would let
/// three people standing on the carrier hide one person in the dark.
pub fn squad_coverage(a: &Array, distances_m: &[f32], obstruction: f32) -> Fidelity {
    distances_m
        .iter()
        .map(|d| fidelity(a, *d, obstruction))
        .min()
        .unwrap_or(Fidelity::Dark)
}

/// THE HANDOFF. The carrier can pass the array to somebody closer to the
/// problem, and it takes time — during which NOBODY has it.
///
/// That gap is the point: a handoff under pressure is a real cost, and it is
/// the moment the squad is most exposed.
pub const HANDOFF_S: f32 = 3.5;

/// Is a handoff worth it? Only if the receiver's position covers the squad
/// better than the current one does.
pub fn handoff_improves(
    a: &Array,
    from_distances: &[f32],
    to_distances: &[f32],
    obstruction: f32,
) -> bool {
    squad_coverage(a, to_distances, obstruction) > squad_coverage(a, from_distances, obstruction)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standing_on_the_carrier_gives_you_everything() {
        let a = Array::default();
        assert_eq!(fidelity(&a, 0.0, 0.0), Fidelity::Full);
        assert!(fidelity(&a, 0.0, 0.0).identifies());
    }

    #[test]
    fn fidelity_collapses_rather_than_fading() {
        // inverse square, so it holds up and then goes — which is what makes
        // a squad's spread a decision rather than a slider
        let a = Array::default();
        let near = fidelity(&a, 60.0, 0.0);
        let mid = fidelity(&a, 240.0, 0.0);
        let far = fidelity(&a, 900.0, 0.0);
        assert!(near > mid && mid > far);
        assert!(near.identifies(), "60 m should still identify");
        assert!(!mid.identifies(), "240 m should not");
        assert_eq!(far, Fidelity::Dark);
    }

    #[test]
    fn rock_costs_the_same_as_distance() {
        let a = Array::default();
        let clear = fidelity(&a, 120.0, 0.0);
        let blocked = fidelity(&a, 120.0, 0.75);
        assert!(blocked < clear, "clear {clear:?} blocked {blocked:?}");
        // and total obstruction is dark whatever the range
        assert_eq!(fidelity(&a, 5.0, 1.0), Fidelity::Dark);
    }

    #[test]
    fn carrying_it_is_a_real_cost() {
        // the carrier is not "better", they are slow and loud and worth
        // killing, which is what stops this being a free upgrade
        let a = Array::default();
        assert!(carrier_speed(&a, 14.0) < 14.0 * 0.8);
        assert!(heard_at_m(&a, 100.0) > 200.0);
        // a smaller array costs less and reaches less
        let light = Array { range_m: 90.0, burden: 0.08, signature: 1.3 };
        assert!(carrier_speed(&light, 14.0) > carrier_speed(&a, 14.0));
        assert!(fidelity(&light, 200.0, 0.0) < fidelity(&a, 200.0, 0.0));
    }

    #[test]
    fn a_squad_is_as_informed_as_its_blindest_member() {
        // the minimum, for the reason the combiner taught: averaging would
        // let three people standing on the carrier hide one in the dark
        let a = Array::default();
        let huddled = [10.0, 20.0, 15.0, 30.0];
        let one_lost = [10.0, 20.0, 15.0, 1_200.0];
        assert_eq!(squad_coverage(&a, &huddled, 0.0), Fidelity::Full);
        assert_eq!(squad_coverage(&a, &one_lost, 0.0), Fidelity::Dark);
    }

    #[test]
    fn a_handoff_is_only_worth_it_if_it_covers_the_squad_better() {
        let a = Array::default();
        // carrier at the back: the forward pair is out of range
        let from = [15.0, 20.0, 700.0, 750.0];
        // carrier in the middle: everybody is inside it
        let to = [200.0, 190.0, 210.0, 205.0];
        assert!(handoff_improves(&a, &from, &to, 0.0));
        // and moving it somewhere worse is refused
        assert!(!handoff_improves(&a, &to, &from, 0.0));
        // the gap during the pass is the cost, and it is not zero
        assert!(HANDOFF_S > 2.0);
    }

    #[test]
    fn the_layer_knows_nothing_about_raids() {
        // the architecture point from the design note: this is handed a
        // distance and a link budget and returns a fidelity. It does not
        // know what is being scanned, who is scanning, or why — everything
        // above it decides what a Bearing means.
        let a = Array { range_m: 1.0, burden: 0.0, signature: 1.0 };
        for d in [0.0, 0.5, 5.0, 5_000.0] {
            let f = fidelity(&a, d, 0.0);
            assert!(f >= Fidelity::Dark && f <= Fidelity::Full, "{d} gave {f:?}");
        }
        // nonsense in, defined answer out
        assert_eq!(fidelity(&a, -5.0, 0.0), Fidelity::Dark);
        assert_eq!(fidelity(&a, 0.0, 2.0), fidelity(&a, 0.0, 1.0));
    }
}
