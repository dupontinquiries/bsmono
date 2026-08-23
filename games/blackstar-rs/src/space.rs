//! SPACE — what kuiper needs to stop being a demo map.
//!
//! `KUIPER` is skydribble's asteroid field, still wired in: eighteen anchored
//! worlds of radius 7–38 m, free bodies with real gravity wells, and one of
//! them collapses into a black hole every five minutes. It is the best map in
//! the set and it has two things wrong with it — the worlds are the size of
//! buildings, and there is a **floor** under them (`MapStyle::amp = 0.15`
//! gives it a terrain grid like every grounded map).
//!
//! Making the planets bigger is not one change. It is three coupled ones, and
//! this module is the arithmetic for all three before anything touches
//! worldgen — the same order that worked for the near plane: measure, then
//! wire.
//!
//! ## 1. LOD has to be angular, not radial
//!
//! A distance-based LOD is correct when everything is the same size. In this
//! field the radii already span 5.4x, and scaling up widens it. What decides
//! how much detail a body needs is **how big it looks**, which is
//! `2·atan(r/d)` — so a giant at four hundred metres and a dwarf at eighty
//! deserve the same treatment, and a distance rule gives them opposite ones.
//!
//! `--space lod` measures that error rather than asserting it.
//!
//! ## 2. Gravity does not survive a rescale — and is wrong before you start
//!
//! What follows was written before `gravity.rs` measured the field itself,
//! and it analyses the CLAMP on `node.g` while missing three larger errors:
//! the falloff is flat out to twice the surface, surface pull does not scale
//! with radius, and the far field relaxes to 1g rather than to nothing. Read
//! `--gravity` first; the rescale problem below is real and it is the
//! smallest of the four.
//!
//! `world.rs` tunes the wells as `(r/15)^1.3`, clamped to 0.35–1.6, with a
//! comment saying the exponent was pulled down from 1.6 "so the field no
//! longer swallows a car that drifts past". That tuning is written around a
//! 15 m reference radius. Multiply every radius by ten and every well clamps
//! to its ceiling — the field becomes uniformly maximal, which is the same as
//! having no size variation at all.
//!
//! The fix is not a new exponent. It is that the reference radius has to
//! scale with the field, and `well_strength` states that explicitly.
//!
//! ## 3. Bigger worlds mean a longer view, and the depth buffer notices
//!
//! `depth.rs` already found that the shipped standard-Z projection resolves
//! 82 mm at 343 m. A space map wants a far plane several times further, and
//! this reports what that costs.

use crate::depth;

/// The field as it ships: eighteen anchored worlds.
pub const CURRENT_MIN_R: f32 = 7.0;
pub const CURRENT_MAX_R: f32 = 38.0;
/// How far out the anchored ring reaches, at world scale 1.
pub const CURRENT_REACH_M: f32 = 210.0;
/// The reference radius `world.rs` tunes gravity around.
pub const GRAV_REF_R: f32 = 15.0;
pub const GRAV_EXP: f32 = 1.3;
pub const GRAV_MIN: f32 = 0.35;
pub const GRAV_MAX: f32 = 1.6;

/// How large a body of radius `r` looks from `d`, in degrees.
pub fn angular_size_deg(r: f32, d: f32) -> f32 {
    if d <= r {
        return 180.0;
    }
    2.0 * (r / d).atan().to_degrees()
}

/// Detail tiers, chosen by how much of the screen a body occupies.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Tier {
    /// smaller than a pixel's worth of arc: do not draw it
    Cull,
    /// a dot
    Impostor,
    Low,
    Medium,
    /// close enough to land on
    Full,
}

/// Degrees of arc at which each tier begins.
pub const TIER_DEG: [(Tier, f32); 5] = [
    (Tier::Full, 25.0),
    (Tier::Medium, 8.0),
    (Tier::Low, 2.5),
    (Tier::Impostor, 0.35),
    (Tier::Cull, 0.0),
];

pub fn tier_for(r: f32, d: f32) -> Tier {
    let a = angular_size_deg(r, d);
    for (t, deg) in TIER_DEG {
        if a >= deg {
            return t;
        }
    }
    Tier::Cull
}

/// The distance-based rule this replaces, for comparison. One set of
/// thresholds for every body, whatever its size.
pub fn tier_by_distance(d: f32, reach: f32) -> Tier {
    let f = d / reach.max(1.0);
    if f < 0.15 {
        Tier::Full
    } else if f < 0.35 {
        Tier::Medium
    } else if f < 0.7 {
        Tier::Low
    } else if f < 1.6 {
        Tier::Impostor
    } else {
        Tier::Cull
    }
}

/// Gravity for a world of radius `r`, given the field's reference radius.
///
/// `world.rs` hardcodes `GRAV_REF_R`. Passing it in is the whole fix: the
/// tuning is a statement about a world's size RELATIVE TO ITS NEIGHBOURS, and
/// a rescale that leaves the reference behind turns every well into the same
/// well.
pub fn well_strength(r: f32, reference_r: f32) -> f32 {
    (r / reference_r.max(0.01)).powf(GRAV_EXP).clamp(GRAV_MIN, GRAV_MAX)
}

/// How much variety is left in a field's wells, 0..1. One means the smallest
/// and largest differ by the full clamp range; zero means they are identical.
pub fn well_spread(min_r: f32, max_r: f32, reference_r: f32) -> f32 {
    let lo = well_strength(min_r, reference_r);
    let hi = well_strength(max_r, reference_r);
    (hi - lo) / (GRAV_MAX - GRAV_MIN)
}

/// The far plane needed to see a field that reaches `reach` metres, with a
/// margin so the furthest body is not clipped as you approach the edge.
pub fn far_plane_for(reach_m: f32) -> f32 {
    reach_m * 2.5
}

// ------------------------------------------------------------- driver ---

pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--space").unwrap_or(0);
    match argv.get(i + 1).map(String::as_str).unwrap_or("all") {
        "lod" => lod(),
        "gravity" => gravity(),
        "depth" => depth_cost(),
        "all" => {
            lod();
            println!();
            gravity();
            println!();
            depth_cost();
            println!();
            floor();
        }
        other => println!("unknown: --space {other}\ntry: lod | gravity | depth | all"),
    }
}

fn lod() {
    println!("ANGULAR LOD — a giant far away looks like a dwarf up close\n");
    let bodies: [(&str, f32); 3] = [("dwarf", 7.0), ("mid", 20.0), ("giant", 38.0)];
    let dists = [40.0f32, 90.0, 200.0, 450.0, 1200.0];
    print!("  {:<8}", "");
    for d in dists {
        print!(" {:>10}", format!("{d:.0} m"));
    }
    println!();
    for (n, r) in bodies {
        print!("  {n:<8}");
        for d in dists {
            print!(" {:>10}", format!("{:?}", tier_for(r, d)));
        }
        println!();
    }
    println!("\n  the same table by DISTANCE alone, which is what a one-size rule does:\n");
    print!("  {:<8}", "");
    for d in dists {
        print!(" {:>10}", format!("{d:.0} m"));
    }
    println!();
    for (n, _) in bodies {
        print!("  {n:<8}");
        for d in dists {
            print!(" {:>10}", format!("{:?}", tier_by_distance(d, CURRENT_REACH_M)));
        }
        println!();
    }
    // measure the disagreement rather than asserting it
    let mut wrong = 0;
    let mut total = 0;
    for (_, r) in bodies {
        for d in [40.0f32, 60.0, 90.0, 140.0, 200.0, 300.0, 450.0, 700.0, 1200.0] {
            total += 1;
            if tier_for(r, d) != tier_by_distance(d, CURRENT_REACH_M) {
                wrong += 1;
            }
        }
    }
    println!(
        "\n  they disagree on {wrong} of {total} cases — because a distance rule cannot"
    );
    println!("  see the {:.1}x radius spread the field already has.", CURRENT_MAX_R / CURRENT_MIN_R);
}

fn gravity() {
    println!("GRAVITY DOES NOT SURVIVE A RESCALE\n");
    println!("  world.rs tunes wells as (r/{GRAV_REF_R:.0})^{GRAV_EXP} clamped to {GRAV_MIN}..{GRAV_MAX}\n");
    println!("  {:>6} {:>10} {:>10} {:>12}", "scale", "dwarf", "giant", "variety left");
    for mult in [1.0f32, 2.0, 5.0, 10.0, 25.0] {
        let (lo, hi) = (CURRENT_MIN_R * mult, CURRENT_MAX_R * mult);
        println!(
            "  {:>5.0}x {:>10.2} {:>10.2} {:>11.0}%",
            mult,
            well_strength(lo, GRAV_REF_R),
            well_strength(hi, GRAV_REF_R),
            well_spread(lo, hi, GRAV_REF_R) * 100.0
        );
    }
    println!("\n  past about five times, every well clamps to the ceiling and the field");
    println!("  has no size variation left at all — which is the opposite of what");
    println!("  making the planets bigger was FOR.\n");
    println!("  with the reference radius scaled alongside the field:\n");
    println!("  {:>6} {:>10} {:>10} {:>12}", "scale", "dwarf", "giant", "variety left");
    for mult in [1.0f32, 2.0, 5.0, 10.0, 25.0] {
        let (lo, hi) = (CURRENT_MIN_R * mult, CURRENT_MAX_R * mult);
        println!(
            "  {:>5.0}x {:>10.2} {:>10.2} {:>11.0}%",
            mult,
            well_strength(lo, GRAV_REF_R * mult),
            well_strength(hi, GRAV_REF_R * mult),
            well_spread(lo, hi, GRAV_REF_R * mult) * 100.0
        );
    }
    println!("\n  unchanged, at any scale. the tuning was always a statement about a");
    println!("  world's size RELATIVE TO ITS NEIGHBOURS.");
}

fn depth_cost() {
    println!("A LONGER VIEW COSTS DEPTH PRECISION\n");
    println!("  {:>6} {:>10} {:>12} {:>14}", "scale", "reach", "far plane", "standard Z");
    for mult in [1.0f32, 5.0, 10.0, 25.0] {
        let reach = CURRENT_REACH_M * mult;
        let far = far_plane_for(reach) as f64;
        let res = depth::resolution_m(reach as f64, depth::NEAR_M, far, false);
        println!(
            "  {:>5.0}x {:>9.0}m {:>11.0}m {:>13.3}m",
            mult, reach, far, res
        );
    }
    println!("\n  at ten times, standard Z cannot resolve a metre at the far side of");
    println!("  the field. `--terrain depth` has the fix and it is free: reversed Z,");
    println!("  or a near plane that scales with the shot.");
    let reach = CURRENT_REACH_M * 10.0;
    let far = far_plane_for(reach) as f64;
    println!(
        "\n  same field, reversed Z: {:.5} m",
        depth::resolution_m(reach as f64, depth::NEAR_M, far, true)
    );
}

fn floor() {
    println!("AND THERE IS A FLOOR UNDER IT\n");
    println!("  `MapStyle` KUIPER carries amp 0.15, so the asteroid field is");
    println!("  generated over a terrain grid exactly like every grounded map.");
    println!("  That is the demo tell: worlds the size of buildings, hanging above");
    println!("  a hill.");
    println!("\n  Removing it is one field, and it is the one change here that is NOT");
    println!("  just arithmetic — `ground_h` feeds seating, spawn clearance and the");
    println!("  camera's terrain clamp, so a map with no floor needs those to agree");
    println!("  that there is nothing to stand on. Worth doing, worth doing on");
    println!("  purpose.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn angular_size_is_what_it_says() {
        // a body as far away as it is wide subtends about 53 degrees
        assert!((angular_size_deg(1.0, 2.0) - 53.13).abs() < 0.1);
        // twice as far, half the arc, near enough at small angles
        let a = angular_size_deg(1.0, 100.0);
        let b = angular_size_deg(1.0, 200.0);
        assert!((a / b - 2.0).abs() < 0.01);
        // inside it, you are inside it
        assert_eq!(angular_size_deg(10.0, 5.0), 180.0);
    }

    #[test]
    fn a_giant_far_away_matches_a_dwarf_up_close() {
        // THE WHOLE POINT. Same apparent size, so the same detail — and a
        // distance rule gets these two exactly backwards.
        let giant = (38.0f32, 400.0f32);
        let dwarf = (7.0f32, 400.0 * 7.0 / 38.0);
        assert!(
            (angular_size_deg(giant.0, giant.1) - angular_size_deg(dwarf.0, dwarf.1)).abs()
                < 0.01
        );
        assert_eq!(tier_for(giant.0, giant.1), tier_for(dwarf.0, dwarf.1));
        // the distance rule disagrees, because it can only see the distance
        assert_ne!(
            tier_by_distance(giant.1, CURRENT_REACH_M),
            tier_by_distance(dwarf.1, CURRENT_REACH_M)
        );
    }

    #[test]
    fn tiers_are_monotone_in_both_arguments() {
        // further is never more detail; bigger is never less
        for r in [7.0f32, 20.0, 38.0] {
            let mut prev = Tier::Full;
            for d in [30.0f32, 60.0, 120.0, 300.0, 900.0, 3000.0] {
                let t = tier_for(r, d);
                assert!(t <= prev, "detail rose with distance at r={r} d={d}");
                prev = t;
            }
        }
        for d in [60.0f32, 300.0] {
            assert!(tier_for(38.0, d) >= tier_for(7.0, d), "a giant got less detail");
        }
    }

    #[test]
    fn the_two_rules_disagree_enough_to_matter() {
        let bodies = [7.0f32, 20.0, 38.0];
        let dists = [40.0f32, 60.0, 90.0, 140.0, 200.0, 300.0, 450.0, 700.0, 1200.0];
        let mut wrong = 0;
        let mut total = 0;
        for r in bodies {
            for d in dists {
                total += 1;
                if tier_for(r, d) != tier_by_distance(d, CURRENT_REACH_M) {
                    wrong += 1;
                }
            }
        }
        assert!(
            wrong * 3 > total,
            "only {wrong} of {total} differ; a distance rule would be fine"
        );
    }

    #[test]
    fn the_shipped_gravity_tuning_collapses_when_you_scale_up() {
        // the coupling that makes "just make them bigger" wrong
        let at_one = well_spread(CURRENT_MIN_R, CURRENT_MAX_R, GRAV_REF_R);
        assert!(at_one > 0.2, "the field should start with real variety: {at_one}");
        let at_ten =
            well_spread(CURRENT_MIN_R * 10.0, CURRENT_MAX_R * 10.0, GRAV_REF_R);
        assert!(
            at_ten < 0.01,
            "scaling ten times should flatten every well: {at_ten}"
        );
        // both ends pinned to the ceiling
        assert_eq!(well_strength(CURRENT_MIN_R * 10.0, GRAV_REF_R), GRAV_MAX);
    }

    #[test]
    fn scaling_the_reference_keeps_the_field_intact() {
        // and the fix, which is not a new exponent
        let base = well_spread(CURRENT_MIN_R, CURRENT_MAX_R, GRAV_REF_R);
        for mult in [2.0f32, 5.0, 10.0, 25.0, 100.0] {
            let s = well_spread(
                CURRENT_MIN_R * mult,
                CURRENT_MAX_R * mult,
                GRAV_REF_R * mult,
            );
            assert!(
                (s - base).abs() < 1e-4,
                "variety moved at {mult}x: {s} vs {base}"
            );
        }
    }

    #[test]
    fn a_bigger_field_needs_the_depth_fix_that_already_exists() {
        // composes with `depth.rs` rather than restating it
        let reach = (CURRENT_REACH_M * 10.0) as f64;
        let far = far_plane_for(reach as f32) as f64;
        let std = depth::resolution_m(reach, depth::NEAR_M, far, false);
        let rev = depth::resolution_m(reach, depth::NEAR_M, far, true);
        assert!(std > 0.5, "standard Z is unexpectedly fine out there: {std}");
        assert!(rev < std / 100.0, "reversed Z should rescue it: {rev}");
    }

    #[test]
    fn the_far_plane_leaves_room_to_reach_the_edge() {
        // a far plane exactly at the reach clips the furthest body the moment
        // you move away from the centre
        let reach = 210.0f32;
        assert!(far_plane_for(reach) > reach * 2.0);
    }

    #[test]
    fn kuiper_still_has_a_floor() {
        // The demo tell, asserted so that removing it is a deliberate change
        // with a failing test to greet it rather than something that quietly
        // never happens. `amp` above zero means a terrain grid.
        let k = crate::world::styles()
            .iter()
            .find(|s| s.name == "KUIPER")
            .expect("the kuiper style");
        assert!(
            k.amp > 0.0,
            "kuiper's floor is gone — good, now update this test and the docs"
        );
    }
}
