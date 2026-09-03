//! GRANDEUR — the size question, by the numbers instead of by feel.
//!
//! **THE NAMES IN THIS TABLE ARE CITATIONS AND MUST NOT BE RENAMED.** Every
//! row is a measurement from real reference material — Dark of the Moon
//! production figures, Destiny lore numbers — and the label is what identifies
//! the source. A vocabulary sweep renamed "Optimus Prime" to this project's
//! own character here once, which turned a published 28 ft figure into a false
//! attribution to a fictional name, and turned "a Cybertron spire" into "a the
//! Foundry spire". Both restored. `vocab.rs` counts these as floors for the
//! same reason it keeps `AllSpark` in `compact.rs`: developer attribution is
//! not in-fiction vocabulary.
//!
//! Not `scale.rs`, which already exists and answers a different question
//! (how tall each MODEL is and where the camera goes for it). Not
//! `rescale.rs`, which prices the physics re-derivation. This is the
//! third thing: what the source material's distances ARE, in one unit,
//! next to what the game currently does.
//!
//! `docs/PRIORITIES.md` §2 prices the scale decision and stops: the
//! player is a Rocket League car with `half_h: 0.19` standing in for a
//! Transformer, the world's metres are the car's, and every distance in
//! the game is therefore some multiple of a body that is 22x too small.
//! The brief for this pass names the references the game should feel
//! like — Dark of the Moon's space and highway sequences, Interstellar's
//! sense of a tiny body against an enormous structure, Destiny's patrol
//! zones and engagement ranges — and asks for those numbers "applied to
//! what would still be relevant for robots: bigger structures, scale
//! the distance, the archways, more grand architecture because it's
//! bigger."
//!
//! This module does three things and refuses a fourth:
//!
//! 1. **Writes the references down** (`REFERENCES`), in metres, with
//!    the height of the body the audience measures them against. These
//!    are APPROXIMATE, from production notes and fan measurement; each
//!    carries a note. Treat them as the order of magnitude the source
//!    material asks for, not as canon.
//! 2. **Puts everything in one unit**: the OCCUPANT HEIGHT (`oh`). A
//!    guardian is 1 oh in Destiny; Optimus Prime is 1 oh in the films; the car
//!    is 1 oh in this engine (`scale::RL_BODY_H_M`). `docs/START_HERE.md`
//!    already says sizes are in occupant heights, not metres; this gives
//!    that unit reference values.
//! 3. **Measures where the game sits** (`report`): the field's
//!    distances, the weapon reaches, the enemy sight lines, each in oh,
//!    next to the reference they should be compared with. The ratios
//!    are the finding.
//!
//! What it refuses to do is retune anything. The measurement says the
//! field is fighting at several times Destiny's engagement distance in
//! body-heights, because every distance was authored in a world where
//! the body is a car. There are two honest fixes — make the body 22x
//! bigger (the Froude re-derivation `rescale.rs` prices) or shrink every
//! distance — and choosing between them is the day-and-a-playtest
//! decision, not something a headless module should make by editing
//! constants. `--grandeur` prints the table so the decision is made
//! against numbers. The architecture rows at the bottom are the ones to
//! author FROM, because a number in oh survives either choice.

use crate::scale::RL_BODY_H_M;

/// A body height everything else is measured against.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub name: &'static str,
    /// the height of the body the audience measures against, metres
    pub body_m: f32,
}

/// Destiny: a guardian. 1.9 m is the usual estimate from door heights.
pub const GUARDIAN: Frame = Frame { name: "Destiny guardian", body_m: 1.9 };
/// The films: a mainline Transformer. Optimus Prime is quoted at 28 ft in the
/// Bay-era production material; 8.5 m.
pub const PRIME: Frame = Frame { name: "film Transformer (Prime)", body_m: 8.5 };
/// Interstellar: a person. Its scale is always a person against a thing.
pub const PERSON: Frame = Frame { name: "Interstellar person", body_m: 1.75 };
/// This engine: the car.
pub const CAR: Frame = Frame { name: "engine car", body_m: RL_BODY_H_M };

/// One reference distance or size from the source material.
#[derive(Clone, Copy, Debug)]
pub struct Reference {
    pub what: &'static str,
    pub frame: Frame,
    pub metres: f32,
    /// what kind of thing it is, so like is compared with like
    pub kind: Kind,
    /// where the number comes from, and how much to trust it
    pub note: &'static str,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// the distance at which fighting starts
    Engagement,
    /// how far a weapon is worth firing
    WeaponReach,
    /// how far a body notices another
    Sight,
    /// how fast a body moves on foot
    FootSpeed,
    /// how fast a vehicle moves
    VehicleSpeed,
    /// a doorway or a console: built for one body
    Intimate,
    /// an archway, a corridor, a bridge: built for a body to pass grandly
    Grand,
    /// a hall, a hangar, a ship: a structure the body is small in
    Monumental,
    /// a canyon, a wave, a machine you cannot see the end of
    Planetary,
    /// a playable area
    Zone,
}

impl Reference {
    /// The number in occupant heights of its own frame.
    pub fn oh(&self) -> f32 {
        self.metres / self.frame.body_m
    }
}

/// THE REFERENCES. Approximate; see each note.
pub const REFERENCES: &[Reference] = &[
    // ---- Destiny, by the numbers ----
    Reference { what: "engagement opens (a red bar notices you)", frame: GUARDIAN, metres: 40.0, kind: Kind::Engagement, note: "typical aggro range on patrol; fan-measured, +/-10 m" },
    Reference { what: "auto rifle effective range", frame: GUARDIAN, metres: 30.0, kind: Kind::WeaponReach, note: "damage falloff start, mid-range archetype" },
    Reference { what: "sniper effective range", frame: GUARDIAN, metres: 80.0, kind: Kind::WeaponReach, note: "practical; the sightline rarely allows more" },
    Reference { what: "a red bar sees you at", frame: GUARDIAN, metres: 60.0, kind: Kind::Sight, note: "fan-measured; past this they idle" },
    Reference { what: "sprint", frame: GUARDIAN, metres: 8.0, kind: Kind::FootSpeed, note: "m/s, fan-timed" },
    Reference { what: "sparrow", frame: GUARDIAN, metres: 44.0, kind: Kind::VehicleSpeed, note: "m/s; ~160 km/h quoted" },
    Reference { what: "a doorway", frame: GUARDIAN, metres: 3.0, kind: Kind::Intimate, note: "Tower doors" },
    Reference { what: "a patrol zone across", frame: GUARDIAN, metres: 1000.0, kind: Kind::Zone, note: "Cosmodrome/EDZ sub-zones, +/-300 m" },
    Reference { what: "the Traveler", frame: GUARDIAN, metres: 14_000.0, kind: Kind::Planetary, note: "diameter, lore figure" },
    // ---- Dark of the Moon ----
    Reference { what: "Optimus Prime", frame: PRIME, metres: 8.5, kind: Kind::Intimate, note: "28 ft, production material" },
    Reference { what: "Sentinel Prime", frame: PRIME, metres: 9.8, kind: Kind::Intimate, note: "32 ft" },
    Reference { what: "the Driller", frame: PRIME, metres: 400.0, kind: Kind::Planetary, note: "length, production estimate; the thing that ate a skyscraper" },
    Reference { what: "the Chicago set piece", frame: PRIME, metres: 2000.0, kind: Kind::Zone, note: "the fought-over stretch of downtown, +/-500 m" },
    Reference { what: "a highway lane (the chase)", frame: PRIME, metres: 3.7, kind: Kind::Intimate, note: "a real lane; the cars are small in it, the robots are not" },
    Reference { what: "a Cybertron spire", frame: PRIME, metres: 300.0, kind: Kind::Monumental, note: "the space sequence's towers, eyeballed against the Ark" },
    Reference { what: "the Ark", frame: PRIME, metres: 300.0, kind: Kind::Monumental, note: "the ship on the moon, +/-100 m" },
    // ---- Interstellar ----
    Reference { what: "the Endurance ring", frame: PERSON, metres: 64.0, kind: Kind::Monumental, note: "diameter, production figure" },
    Reference { what: "the Ranger", frame: PERSON, metres: 20.0, kind: Kind::Grand, note: "length, production figure" },
    Reference { what: "Miller's wave", frame: PERSON, metres: 1200.0, kind: Kind::Planetary, note: "height; the number the film quotes" },
    Reference { what: "Cooper Station", frame: PERSON, metres: 8000.0, kind: Kind::Planetary, note: "length, an O'Neill cylinder" },
    // ---- the film-scale ARCHITECTURE the brief asks for, derived ----
    Reference { what: "an archway a Transformer walks under grandly", frame: PRIME, metres: 8.5 * 4.0, kind: Kind::Grand, note: "DERIVED: 4 oh - a cathedral door is ~3 person-heights; grand is a little more" },
    Reference { what: "a corridor two abreast", frame: PRIME, metres: 8.5 * 2.5, kind: Kind::Grand, note: "DERIVED: 2.5 oh wide" },
    Reference { what: "a hangar", frame: PRIME, metres: 8.5 * 20.0, kind: Kind::Monumental, note: "DERIVED: 20 oh - the Ark's interior" },
    Reference { what: "a fought-over district", frame: PRIME, metres: 8.5 * 250.0, kind: Kind::Zone, note: "DERIVED: 250 oh, between Chicago (235) and a patrol zone (526)" },
];

/// One of the game's own numbers, for the table.
#[derive(Clone, Copy, Debug)]
pub struct Measured {
    pub what: &'static str,
    pub metres: f32,
    pub kind: Kind,
}

impl Measured {
    pub fn oh(&self) -> f32 {
        self.metres / CAR.body_m
    }
}

/// The game's numbers as they stand, in engine metres.
pub fn measured() -> Vec<Measured> {
    use crate::field;
    vec![
        Measured { what: "field: a foe folds and drives beyond", metres: field::FOLD_OUT_M, kind: Kind::Engagement },
        Measured { what: "field: the flank point beside you", metres: field::FLANK_M, kind: Kind::Engagement },
        Measured { what: "field: the wave musters past the LZ", metres: field::SITE_AHEAD_M, kind: Kind::Zone },
        Measured { what: "field: a rail rank reaches for you up to", metres: field::CABLE_MAX_M, kind: Kind::WeaponReach },
        Measured { what: "field: your hook reaches", metres: field::HOOK_REACH_M, kind: Kind::WeaponReach },
        Measured { what: "enemy: a grunt sees you at", metres: 140.0, kind: Kind::Sight },
        Measured { what: "enemy: a grunt opens fire at", metres: 90.0, kind: Kind::Engagement },
        Measured { what: "enemy: a grunt walks at", metres: 4.0, kind: Kind::FootSpeed },
        Measured { what: "enemy: a grunt drives at", metres: 4.0 * field::VEH_MUL, kind: Kind::VehicleSpeed },
        Measured { what: "weapon: TITANIUM reach", metres: 70.0, kind: Kind::WeaponReach },
        Measured { what: "weapon: PHOTON reach", metres: 120.0, kind: Kind::WeaponReach },
        Measured { what: "weapon: FIRE reach", metres: 40.0, kind: Kind::WeaponReach },
        Measured { what: "car: the body (2 x half_h)", metres: CAR.body_m, kind: Kind::Intimate },
    ]
}

/// The reference a measured number should be read against: the first
/// reference of the same kind, in oh.
pub fn reference_oh(kind: Kind) -> Option<(f32, &'static str)> {
    REFERENCES.iter().find(|r| r.kind == kind).map(|r| (r.oh(), r.what))
}

/// The headline ratio: the median, over engagement and weapon reach, of
/// the game's number over the reference, both in oh.
pub fn headline() -> f32 {
    let mut ratios: Vec<f32> = measured()
        .into_iter()
        .filter(|m| matches!(m.kind, Kind::Engagement | Kind::WeaponReach))
        .filter_map(|m| reference_oh(m.kind).map(|(r, _)| m.oh() / r))
        .filter(|r| r.is_finite())
        .collect();
    ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    ratios.get(ratios.len() / 2).copied().unwrap_or(f32::NAN)
}

/// The table, as text. Ratios over 1 mean the game's number is bigger
/// in body-heights than the reference.
pub fn report() -> String {
    let mut out = String::new();
    out.push_str("GRANDEUR - the game's numbers in occupant heights (oh), against the references\n\n");
    out.push_str(&format!(
        "frames: {} {:.2} m | {} {:.1} m | {} {:.1} m | {} {:.2} m\n\n",
        CAR.name, CAR.body_m, PRIME.name, PRIME.body_m, GUARDIAN.name, GUARDIAN.body_m, PERSON.name, PERSON.body_m
    ));
    out.push_str("REFERENCES (approximate; see notes)\n");
    for r in REFERENCES {
        out.push_str(&format!("  {:<48} {:>9.1} m  {:>8.1} oh  [{}]  {}\n", r.what, r.metres, r.oh(), r.frame.name, r.note));
    }
    out.push_str("\nMEASURED (engine metres; the body is the car)\n");
    for m in measured() {
        let (ref_oh, ref_what) = reference_oh(m.kind).unwrap_or((f32::NAN, "-"));
        let ratio = m.oh() / ref_oh;
        out.push_str(&format!(
            "  {:<48} {:>9.1} m  {:>8.1} oh   vs {:>7.1} oh ({})  x{:.2}\n",
            m.what, m.metres, m.oh(), ref_oh, ref_what, ratio
        ));
    }
    out.push_str(&format!("\nHEADLINE: engagement and reach sit at x{:.1} the reference, in body-heights.\n", headline()));
    out.push_str("Two honest fixes, neither taken here: grow the body (Froude, rescale.rs) or shrink the distances.\n");
    out.push_str("Either way, author the ARCHITECTURE in oh from the DERIVED rows, so it survives the choice.\n");
    out
}

/// `--grandeur`: print the table.
pub fn run() {
    print!("{}", report());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_reference_is_a_positive_number_in_a_named_frame_with_provenance() {
        for r in REFERENCES {
            assert!(r.metres > 0.0, "{}", r.what);
            assert!(r.frame.body_m > 0.0);
            assert!(r.oh() > 0.0);
            assert!(!r.note.is_empty(), "{} has no provenance", r.what);
        }
    }

    #[test]
    fn the_unit_is_the_body_so_a_body_is_one() {
        let car = measured().into_iter().find(|m| m.what.starts_with("car:")).unwrap();
        assert!((car.oh() - 1.0).abs() < 1e-6);
        let prime = REFERENCES.iter().find(|r| r.what == "Optimus Prime").unwrap();
        assert!((prime.oh() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn every_measured_kind_has_a_reference_to_read_against() {
        for m in measured() {
            assert!(reference_oh(m.kind).is_some(), "no reference for {:?} ({})", m.kind, m.what);
        }
    }

    #[test]
    fn the_headline_is_a_finding_not_a_pass() {
        // the report exists to say the game fights too far out in body
        // heights; if this ever reads as ~1 the scale decision was made,
        // and this test should be rewritten to pin it
        let h = headline();
        assert!(h.is_finite());
        assert!(h > 2.0, "the field now fights at x{h:.1} the reference - the scale was decided; pin it here");
        assert!(report().contains("HEADLINE"));
    }

    #[test]
    fn the_derived_architecture_is_grander_than_intimate_and_smaller_than_planetary() {
        let grand: Vec<f32> = REFERENCES.iter().filter(|r| r.kind == Kind::Grand).map(|r| r.oh()).collect();
        let intimate_max = REFERENCES.iter().filter(|r| r.kind == Kind::Intimate).map(|r| r.oh()).fold(0.0, f32::max);
        let planetary_min = REFERENCES.iter().filter(|r| r.kind == Kind::Planetary).map(|r| r.oh()).fold(f32::MAX, f32::min);
        for g in grand {
            assert!(g > intimate_max && g < planetary_min, "grand {g:.1} oh is not between intimate {intimate_max:.1} and planetary {planetary_min:.1}");
        }
    }
}
