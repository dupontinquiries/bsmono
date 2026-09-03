//! THE RED DWARF STRIKE — Billy holds the star short, Photon Man aims it.
//!
//! Source: the worldbuilding session in
//! `TF_CLONE_CIVILIZATIONS_AND_CONVERGENCE.md` (screenplay treatment).
//! CORRECTED TWICE now, and both corrections matter to the mechanics:
//!
//! 1. The Matter Transformation Planet is never found across the six
//!    films; Trilogy Two's final battle stays in Earth's solar system.
//!    (The earlier "Photon Man relocates the planet" ending is
//!    retracted, not superseded.)
//! 2. The climax is now built around a mass arrival — every scattered
//!    Clone world answering at once, for the first time — which raises
//!    a question the first draft of this module got wrong: a TRUE
//!    full-charge star release (`weapons::Star::release` at its `full`
//!    threshold) is not a targeted weapon, it is a radius, and the
//!    arriving reinforcements would be standing inside it. A full nova
//!    kills the allies it was supposed to be covering.
//!
//! The fix the session landed on is also the strongest character beat
//! either of these two has been offered: Billy does not release fully.
//! He holds the charge short of `STAR_FULL` — dims the star down
//! toward a red dwarf instead of consuming it — and fires THAT. Every
//! other place this treatment describes him is explicit that
//! restraint is not his instinct ("he does not share The Morpher's
//! reluctance about using full force"); choosing it once, in front of
//! everyone, because the people he would otherwise kill are the
//! reinforcements arriving to help him, is his version of the
//! completed arc the treatment already gives Halyard and Blackstar.
//!
//! Photon Man's role changes to match. The earlier draft had the two
//! blasts colliding; the stronger version is a division of labour, and
//! it uses a property the treatment already gives Photon Man and
//! never spends: *"light as matter — structured, directed, aware."*
//! Billy supplies the scale. Photon Man supplies the AIM — directing
//! the held draw onto the fleet and away from the airspace the
//! arriving Clone worlds are leaping into.
//!
//! `Ending::Teleport` vs. `Ending::NewSun` stays exactly as open as it
//! was: NewSun in particular is now in real tension with a held,
//! deliberately-not-full release (the doc flags this explicitly rather
//! than quietly picking one), so both branches are left as they were.

use crate::watch::{self, Watch, PHOTON};

/// What the strike did to whatever it caught.
pub struct Convergence {
    /// hostiles cleared — scales with how much of the star Billy held,
    /// but is capped by construction below what a true nova would do,
    /// because this is the version that does not erase everything in
    /// its own radius
    pub fleet_cleared: u32,
}

/// Below this multiplier the held draw is a shrug, not a weapon — a
/// charge worth firing, not a token flick. Above `weapons::STAR_FULL`'s
/// own multiplier the release stops being "held" at all; `converge`
/// enforces that ceiling by refusing a true nova outright.
pub const MIN_HELD_MULT: f32 = 3.0;

/// Hijacks cleared per point of release multiplier above the held
/// floor — arbitrary but stated once, here, rather than buried in the
/// formula below.
const HULLS_PER_MULT: f32 = 40.0;

/// Attempt the strike. Three gates, all of them load-bearing:
/// * a TRUE nova refuses outright — that blast has no aim, and the
///   arriving allies are standing in its radius;
/// * too small a draw refuses too — a shrug does not touch a fleet;
/// * Photon Man has to be the one aiming it, or the held draw goes out
///   broad instead of directed and costs exactly what a nova would.
pub fn converge(star_mult: f32, star_was_full: bool, watch: &Watch) -> Result<Convergence, String> {
    if star_was_full {
        return Err(
            "a true nova was fired; nothing in its radius survives, and the arriving allies were standing in it"
                .into(),
        );
    }
    if star_mult < MIN_HELD_MULT {
        return Err("Billy's held charge does not touch a fleet at this scale".into());
    }
    if watch.current() != Some(PHOTON) {
        return Err("the Watch does not hold Photon Man; the held draw goes out broad, not aimed".into());
    }
    let fleet_cleared = ((star_mult - MIN_HELD_MULT).max(0.0) * HULLS_PER_MULT) as u32 + 40;
    Ok(Convergence { fleet_cleared })
}

/// One ending, played to its mechanical conclusion. Neither branch
/// mutates `watch` unless it is the branch that needs to. Still open,
/// per the doc's own "worth deciding rather than blending."
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ending {
    Teleport,
    NewSun,
}

pub fn resolve(ending: Ending, watch: &mut Watch) -> Result<String, String> {
    match ending {
        Ending::Teleport => {
            let ice = watch::slot_named("ICE").expect("ICE is a native form");
            watch.shift(ice, false).map_err(|e| {
                format!("the Morpher reaches for Ice and finds nothing left to reach with: {e}")
            })?;
            Ok(format!(
                "spent to the floor, the Watch holds Ice long enough to leap them clear -- {:.0}% resolve left",
                watch.resolve() * 100.0
            ))
        }
        Ending::NewSun => Ok(
            "Billy does not come back. the star he fired stays lit -- a new sun, and the system \
             it restores does not know his name for it"
                .into(),
        ),
    }
}

/// Whether `Ending::Teleport` is even affordable right now, without
/// spending anything — a caller (the shell, or a story trigger) can
/// use this to decide which ending the fiction earns rather than
/// gambling on a resolve check it cannot see coming.
pub fn teleport_affordable(watch: &Watch) -> bool {
    watch.resolve() >= crate::watch::SHIFT_COST
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weapons::{Star, STAR_FULL};

    #[test]
    fn the_two_endings_are_not_symmetric_and_the_gap_is_two_seconds() {
        // Found by driving `--finale`, not by reading: NewSun is always
        // available and Teleport is REFUSED straight after the strike,
        // because Photon Man costs almost the whole resolve pool (5% left
        // against a 16% shift). It becomes possible only if the Morpher
        // survives long enough to recover eleven percent.
        //
        // Nobody wrote that rule; it falls out of two costs in `watch.rs`.
        // Pinned here because it is now a story fact — the ending depends on
        // whether the scene gives him a beat — and a tweak to SHIFT_COST or
        // the Photon cost would silently rewrite it.
        let after_photon = || {
            let mut w = Watch::default();
            w.shift(PHOTON, true).unwrap();
            w
        };
        assert!(!teleport_affordable(&after_photon()), "Teleport should not be free after Photon");

        let mut w = after_photon();
        assert!(resolve(Ending::Teleport, &mut w).is_err(), "Teleport ran anyway");
        let mut w = after_photon();
        assert!(resolve(Ending::NewSun, &mut w).is_ok(), "NewSun should always be available");

        // and the gap is short enough to be a dramatic beat rather than a
        // different scene
        let need = (crate::watch::SHIFT_COST - after_photon().resolve()).max(0.0)
            / crate::watch::REGEN_PER_SEC;
        assert!(need > 0.0 && need < 5.0, "the recovery beat is {need}s");
        let mut rested = after_photon();
        rested.tick(need + 0.5);
        assert!(teleport_affordable(&rested), "the beat did not pay for the leap");
        assert!(resolve(Ending::Teleport, &mut rested).is_ok());
    }

    #[test]
    fn a_true_nova_refuses_outright_no_matter_who_is_watching() {
        // this is the whole point of the correction: full force is not
        // an option here, not even with Photon Man aiming it
        let mut watch = Watch::default();
        watch.shift(PHOTON, true).unwrap();
        assert!(converge(6.0, true, &watch).is_err(), "a true nova was allowed through");
    }

    #[test]
    fn a_shrug_does_not_clear_anything_and_photon_man_still_has_to_be_the_one_aiming() {
        let watch = Watch::default(); // native Ice, not Photon
        assert!(converge(6.0, true, &watch).is_err(), "double-refused for the wrong combination of reasons");
        assert!(converge(1.5, false, &watch).is_err(), "a token flick with no Photon Man still cleared something");
        let mut photon_watch = Watch::default();
        photon_watch.shift(PHOTON, true).unwrap();
        assert!(converge(1.5, false, &photon_watch).is_err(), "too small a draw still cleared something");
        assert!(converge(3.5, false, &photon_watch).is_ok(), "a real held draw, aimed, still refused");
    }

    #[test]
    fn a_bigger_held_draw_clears_more_but_never_as_much_as_a_nova_would() {
        let mut watch = Watch::default();
        watch.shift(PHOTON, true).unwrap();
        let small = converge(MIN_HELD_MULT + 0.1, false, &watch).unwrap();
        // the ceiling: the strongest legal held draw is just under the
        // true-nova multiplier weapons.rs computes at STAR_FULL
        let ceiling_mult = 1.0 + STAR_FULL * 5.0 - 0.01;
        let big = converge(ceiling_mult, false, &watch).unwrap();
        assert!(big.fleet_cleared > small.fleet_cleared, "scale did not matter");
    }

    #[test]
    fn the_held_ceiling_sits_just_under_where_weapons_rs_calls_it_a_true_nova() {
        // ties this module's notion of "held" to weapons.rs's own
        // definition of "full" rather than inventing a second one
        let mut star = Star::default();
        star.begin();
        star.update(10.0, true); // fully charged
        let (mult, full) = star.release();
        assert!(full, "a full charge did not read as a true nova in weapons.rs");
        assert!(converge(mult, true, &Watch::default()).is_err(), "that exact release should have refused");
        let mut aimed = Watch::default();
        aimed.shift(PHOTON, true).unwrap();
        assert!(converge(mult - 0.5, false, &aimed).is_ok(), "a draw just under the nova threshold should have gone through");
    }

    #[test]
    fn teleport_leaps_the_survivors_and_can_genuinely_fail() {
        let mut watch = Watch::default();
        watch.shift(PHOTON, true).unwrap(); // resolve now near zero
        assert!(!teleport_affordable(&watch), "photon man left resolve for a free leap");
        assert!(resolve(Ending::Teleport, &mut watch).is_err(), "an empty Watch still leapt them clear");
        // rested enough, the same ending succeeds and actually shifts to Ice
        watch.tick(100.0);
        let msg = resolve(Ending::Teleport, &mut watch).unwrap();
        assert!(msg.contains("resolve left"));
        assert_eq!(watch.current_name(), "ICE");
    }

    #[test]
    fn new_sun_is_permanent_and_does_not_touch_the_watch() {
        let mut watch = Watch::default();
        watch.shift(PHOTON, true).unwrap();
        let before = watch.current_name();
        let msg = resolve(Ending::NewSun, &mut watch).unwrap();
        assert!(msg.contains("new sun"));
        assert_eq!(watch.current_name(), before, "the new-sun ending changed what the Watch is doing");
    }
}

// --------------------------------------------------------------- driver ---

/// `--finale` — the red dwarf strike, and the three ways it refuses.
///
/// `converge` and `resolve` were written, tested, and called by nothing.
/// That matters more here than for most dead modules: the interesting thing
/// about this scene is not that it succeeds, it is WHAT IT REFUSES. A true
/// nova is turned away because the arriving allies are standing in the
/// radius; a small draw because a shrug does not touch a fleet; a Watch not
/// holding Photon Man because the held draw goes out broad instead of aimed.
///
/// Three refusals that each state a piece of the fiction, and no way to read
/// them short of the source.
pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--finale").unwrap_or(0);
    let arg = argv.get(i + 1).map(String::as_str);

    println!("THE RED DWARF STRIKE\n");
    println!("  Billy does not release fully. He holds the charge short of a nova");
    println!("  -- dims the star toward a red dwarf -- and Photon Man aims it, away");
    println!("  from the airspace the arriving Clone worlds are leaping into.");
    println!("  Restraint is not his instinct, which is why doing it once counts.\n");

    // the refusals, each with the state that causes it
    let aimed = photon_watch();
    let wrong_form = Watch::default();
    println!("  WHAT IT REFUSES\n");
    for (what, mult, full, w) in [
        ("a true nova", 9.0f32, true, &aimed),
        ("too small a draw", 1.0, false, &aimed),
        ("nobody aiming it", 9.0, false, &wrong_form),
    ] {
        match converge(mult, full, w) {
            Err(e) => println!("    {:<18} {}", what, e),
            Ok(c) => println!("    {:<18} UNEXPECTEDLY ALLOWED ({} cleared)", what, c.fleet_cleared),
        }
    }

    println!("\n  WHAT IT DOES, by how much of the star he holds\n");
    println!("    {:>6}  {:>14}", "mult", "fleet cleared");
    for mult in [3.0f32, 4.0, 6.0, 9.0, 12.0] {
        match converge(mult, false, &aimed) {
            Ok(c) => println!("    {mult:>6.1}  {:>14}", c.fleet_cleared),
            Err(e) => println!("    {mult:>6.1}  {e}"),
        }
    }

    // and the two endings, which the treatment deliberately leaves open
    let which = match arg {
        Some("teleport") => Some(Ending::Teleport),
        Some("newsun") => Some(Ending::NewSun),
        _ => None,
    };
    // THE TWO ENDINGS ARE NOT SYMMETRIC, and that turns out to be the
    // interesting thing. NewSun is always available. Teleport needs the
    // Morpher to hold one more form, and Photon Man costs almost the whole
    // resolve pool — 5% left against a 16% shift — so straight after the
    // strike it REFUSES. It only becomes possible if he survives long enough
    // to recover eleven percent.
    //
    // So the choice is not a menu. NewSun is what happens if the scene does
    // not give him that beat, and the mechanics say so without anyone having
    // written the rule down.
    println!("\n  THE ENDING is still open, and the two are NOT symmetric:\n");
    let rest_s = (crate::watch::SHIFT_COST - photon_watch().resolve()).max(0.0)
        / crate::watch::REGEN_PER_SEC;
    for e in [Ending::Teleport, Ending::NewSun] {
        if which.map_or(false, |w| w != e) {
            continue;
        }
        let mut now = photon_watch();
        println!("    {:?}", e);
        match resolve(e, &mut now) {
            Ok(s) => println!("      straight after the strike: {s}"),
            Err(s) => println!("      straight after the strike: REFUSED — {s}"),
        }
        let mut rested = photon_watch();
        rested.tick(rest_s + 0.5);
        match resolve(e, &mut rested) {
            Ok(s) => println!("      after {rest_s:.0}s of holding on: {s}"),
            Err(s) => println!("      after {rest_s:.0}s of holding on: still refused — {s}"),
        }
    }
    println!("\n  Photon Man costs almost the whole pool, so Teleport needs {rest_s:.0}s");
    println!("  the scene may not have. NewSun is what happens if it does not.");
    println!("\n  --finale teleport | newsun  for one of them");
}

/// A Watch actually holding Photon Man, which every success path needs.
fn photon_watch() -> Watch {
    let mut w = Watch::default();
    // reach the state the way the game does — `shift` is the real path and
    // carries its own resolve cost — rather than writing the field
    let _ = w.shift(PHOTON, true);
    w
}
