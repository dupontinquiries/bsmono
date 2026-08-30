//! GRAVITY — triple-checking a formula that has four separate things wrong.
//!
//! The complaint was that a tiny platform pulls as hard as a planet at close
//! range. It does, and measuring why turned up two more problems, one of
//! which matters more than the original.
//!
//! ## What the engine does
//!
//! `World::gravity_field` finds the nearest node and blends its multiplier
//! toward 1.0:
//!
//! ```text
//! t = d / node.r          where node.r = planet_radius * 2.6
//! w = clamp((1.5 - t) / 0.75, 0, 1)
//! g = 1.0 + (node.g - 1.0) * w
//! ```
//!
//! and `node.g = clamp((r/15)^1.3, 0.35, 1.6)`.
//!
//! ## Defect 1 — the falloff is normalised by the world's own size
//!
//! `t` is distance in units of the node's own radius, so **every world's
//! field has the same shape**; only its extent changes. A 2 m platform holds
//! full strength out to 3.9 m and a 38 m world out to 74 m, and at their own
//! surfaces they pull 0.35 and 1.60 — a ratio of 4.6 across a 19:1 range of
//! radius. Constant density says surface gravity is proportional to radius,
//! so the ratio should be 19.
//!
//! ## Defect 2 — there is no falloff near the surface at all
//!
//! `w` is pinned at 1.0 for all `t < 0.75`, which is `d < 1.95 · radius`.
//! Gravity is FLAT from the centre out to nearly twice the surface, then
//! decays linearly. Real gravity peaks exactly at the surface and drops as
//! `1/d²` above it. Measured from a 38 m world: the engine gives 1.60 at both
//! 38 m and 60 m; the truth falls to 0.40 over that span.
//!
//! ## Defect 3 — and this is the one that matters for a space map
//!
//! The far field relaxes to **1.0, not 0**. Beyond every well you still get a
//! full gravity pointing at world -Y. On a grounded map that is correct and
//! deliberate — it is the ground's own gravity. In `kuiper` it means **the
//! void pulls downward**, which is why an asteroid field reads as a set of
//! rocks hanging over an invisible floor even before you notice the actual
//! floor.
//!
//! That is the same finding `space.rs` reached from the other end, and it is
//! worse than it looked there: I analysed the CLAMP on `node.g` and missed
//! that the falloff shape and the ambient floor are the larger errors.
//!
//! ## What is here
//!
//! The correct field for a uniform sphere, the engine's field, and the
//! arithmetic to compare them. Defects 1 and 2 (the falloff shape) are
//! still unpatched — `world.rs`'s constants are load-bearing for eighteen
//! grounded maps. Defect 3 IS patched: `World::gravity_field` now takes
//! its far field from [`far_field`] per map — `Ambient::Void` on kuiper,
//! `Ambient::Grounded` everywhere else — so the void no longer pulls.

/// `GravNode.r` is this multiple of the planet's drawn radius.
pub const REACH_MULT: f32 = 2.6;
/// The reference radius `world.rs` tunes around.
pub const REF_R: f32 = 15.0;

/// The engine's `node.g` for a world of drawn radius `r`.
pub fn engine_mul(r: f32) -> f32 {
    (r / REF_R).powf(1.3).clamp(0.35, 1.6)
}

/// The engine's gravity multiplier at distance `d` from a world of radius `r`.
pub fn engine_field(d: f32, r: f32) -> f32 {
    let reach = (r * REACH_MULT).max(0.01);
    let t = d / reach;
    let w = ((1.5 - t) / 0.75).clamp(0.0, 1.0);
    1.0 + (engine_mul(r) - 1.0) * w
}

/// Surface gravity of a uniform sphere, relative to the reference world.
///
/// `g = GM/r²` and `M ∝ r³`, so at constant density **surface gravity is
/// proportional to radius**. Nothing exotic; it is the thing the engine's
/// `^1.3` exponent and its clamps are approximating badly.
pub fn true_surface_g(r: f32, ref_r: f32) -> f32 {
    r / ref_r.max(0.01)
}

/// The correct field at distance `d` from the centre of a uniform sphere.
///
/// Two regimes, and the engine has neither: **inside** the body gravity falls
/// linearly to zero at the centre (only the mass beneath you pulls); **above
/// the surface** it falls as the inverse square.
pub fn true_field(d: f32, r: f32, ref_r: f32) -> f32 {
    let gs = true_surface_g(r, ref_r);
    if d <= 0.0 {
        return 0.0;
    }
    if d < r {
        gs * (d / r) // shell theorem: the shell above you cancels
    } else {
        gs * (r / d).powi(2)
    }
}

/// What every well at once adds up to, which is what a field of them needs.
///
/// The engine takes the NEAREST node only. That is cheap and it is wrong
/// between two worlds, where the real field is the sum and can point
/// somewhere neither node does.
pub fn true_field_sum(d_and_r: &[(f32, f32)], ref_r: f32) -> f32 {
    d_and_r.iter().map(|(d, r)| true_field(*d, *r, ref_r)).sum()
}

/// The engine's tube profile: `0.50 + 0.95 * (1 - d/(r/1.45))`.
///
/// Maximum at the axis, falling toward the wall.
pub fn engine_tube(d_from_axis: f32, node_r: f32) -> f32 {
    let f = (d_from_axis / (node_r / 1.45).max(0.01)).clamp(0.0, 1.0);
    0.50 + 0.95 * (1.0 - f)
}

/// What a spinning tube actually does: `a = ω²r`.
///
/// **Zero at the axis, maximum at the wall** — the exact inverse of the
/// engine's profile. It is also the same shape as the interior of a solid
/// sphere (`true_field` below its surface), for the same reason: in both
/// cases only what is outside your radius contributes nothing.
///
/// The engine's comment history records both symptoms of having it backwards
/// and neither diagnosis. "A car flung into a wide pipe simply flew up the
/// middle at constant speed, never touching anything" — that is CORRECT for
/// spin gravity, you are weightless on the axis, and it was read as a bug.
/// The fix made the axis the strongest point, and then: "at 1.7x it pinned".
/// Of course it did. It was flinging you outward hardest from the one place
/// that should be still.
pub fn true_tube(d_from_axis: f32, bore_r: f32) -> f32 {
    (d_from_axis / bore_r.max(0.01)).clamp(0.0, 1.0)
}

/// Does this map have gravity where no world does?
///
/// A grounded map does — that is the planet you are standing on, and the
/// nodes are local overrides on top of it. **A space map does not**, and this
/// is the single most consequential difference between kuiper and everything
/// else in the style table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ambient {
    /// beyond every well, fall toward world -Y at 1g
    Grounded,
    /// beyond every well, fall toward nothing
    Void,
}

/// The engine's far-field value under each convention.
pub fn far_field(a: Ambient) -> f32 {
    match a {
        Ambient::Grounded => 1.0,
        Ambient::Void => 0.0,
    }
}

/// Where the engine's field crosses the truth, for one world. Below this the
/// engine over-pulls; above it, under-pulls.
pub fn crossover_m(r: f32, ref_r: f32) -> Option<f32> {
    let mut lo = r * 0.05;
    let mut hi = r * REACH_MULT * 1.5;
    let f = |d: f32| engine_field(d, r) - true_field(d, r, ref_r);
    if f(lo) * f(hi) > 0.0 {
        return None;
    }
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if f(lo) * f(mid) <= 0.0 {
            hi = mid;
        } else {
            lo = mid;
        }
    }
    Some(0.5 * (lo + hi))
}

// ------------------------------------------------------------- driver ---

pub fn run() {
    println!("GRAVITY — four things wrong, measured\n");

    println!("  1. SURFACE PULL DOES NOT SCALE WITH THE WORLD\n");
    println!("  {:>8} {:>12} {:>12} {:>10}", "radius", "engine", "uniform sphere", "error");
    for r in [2.0f32, 7.0, 15.0, 26.0, 38.0] {
        let e = engine_field(r, r);
        let t = true_surface_g(r, REF_R);
        println!("  {r:>7.0}m {e:>12.2} {t:>14.2} {:>9.1}x", e / t);
    }
    println!("\n     a 19:1 spread of radius becomes a 4.6:1 spread of pull, and a two");
    println!("     metre platform pulls nearly three times what it should. that is the");
    println!("     complaint: at close range everything is a planet.\n");

    println!("  2. THERE IS NO FALLOFF UNTIL TWICE THE SURFACE\n");
    println!("  {:>8} {:>10} {:>14}", "from 38m", "engine", "inverse square");
    for d in [38.0f32, 50.0, 60.0, 74.0, 99.0, 148.0, 250.0] {
        println!(
            "  {d:>7.0}m {:>10.2} {:>14.2}",
            engine_field(d, 38.0),
            true_field(d, 38.0, REF_R)
        );
    }
    println!("\n     `w` is pinned at 1 while d < 1.95r, so the field is FLAT from the");
    println!("     centre out past the surface, then decays linearly. Real gravity");
    println!("     peaks at the surface and drops as 1/d² above it.\n");

    println!("  3. THE VOID PULLS\n");
    println!("     beyond every well the engine returns {:.1}, pointing at world -Y.", far_field(Ambient::Grounded));
    println!("     On a grounded map that IS the planet you are standing on, and the");
    println!("     nodes are local overrides. In kuiper it means an asteroid field");
    println!("     hangs over an invisible floor — which it also literally does,");
    println!("     since KUIPER carries amp 0.15.\n");
    println!("     A space map wants {:.1}.", far_field(Ambient::Void));

    // the fourth thing, which only shows up in a FIELD of worlds
    println!("  4. AND IT ONLY EVER LOOKS AT THE NEAREST WORLD\n");
    let r = 20.0f32;
    println!("     between two {r:.0} m worlds 240 m apart:\n");
    println!("     {:>10} {:>12} {:>12} {:>10}", "position", "engine sees", "real sum", "missing");
    for frac in [0.5f32, 0.35, 0.2, 0.08] {
        let (da, db) = (240.0 * frac, 240.0 * (1.0 - frac));
        let nearest = da.min(db);
        let eng = engine_field(nearest, r);
        let sum = true_field_sum(&[(da, r), (db, r)], REF_R);
        let one = true_field(nearest, r, REF_R);
        println!(
            "     {:>9.0}m {:>12.3} {:>12.3} {:>9.0}%",
            da,
            eng,
            sum,
            (sum / one.max(1e-6) - 1.0) * 100.0
        );
    }
    println!("\n     the last column is how much of the real pull the far world");
    println!("     contributes and the engine discards. At the midpoint it is the");
    println!("     other HALF of the field, and the two pull opposite ways — so the");
    println!("     true answer there is nearly weightless and the engine says 1g.");
    println!("     A Lagrange point is a thing an asteroid field should have.\n");

    println!("  WHERE THE TWO CROSS\n");
    for r in [7.0f32, 15.0, 38.0] {
        match crossover_m(r, REF_R) {
            Some(d) => println!("    a {r:.0} m world: engine and truth agree at {d:.1} m ({:.2}r)", d / r),
            None => println!("    a {r:.0} m world: they never agree"),
        }
    }
    println!("\n  NOTHING HERE IS PATCHED. `world.rs`'s constants are load-bearing for");
    println!("  eighteen grounded maps where the flat near-field is what makes a");
    println!("  carved deck driveable. The fix is a per-map ambient setting plus a");
    println!("  real falloff, and that is a decision, not a better exponent.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tiny_platform_pulls_far_too_hard() {
        // the reported complaint, as a number
        let e = engine_field(2.0, 2.0);
        let t = true_surface_g(2.0, REF_R);
        assert!(e / t > 2.0, "a 2 m platform pulls {:.1}x what it should", e / t);
        // and the clamp is what does it: anything under about 8 m gets the floor
        assert_eq!(engine_mul(2.0), 0.35);
        assert_eq!(engine_mul(0.5), 0.35);
    }

    #[test]
    fn the_engine_compresses_the_whole_size_range() {
        // 19:1 of radius becomes 4.6:1 of pull, because both clamps bite
        let (small, big) = (2.0f32, 38.0f32);
        let engine_ratio = engine_field(big, big) / engine_field(small, small);
        let true_ratio = true_surface_g(big, REF_R) / true_surface_g(small, REF_R);
        assert!(
            true_ratio > engine_ratio * 3.0,
            "engine {engine_ratio:.1}x vs truth {true_ratio:.1}x"
        );
        assert_eq!(engine_mul(big), 1.6, "the giant is clamped at the ceiling");
    }

    #[test]
    fn there_is_no_falloff_until_nearly_twice_the_surface() {
        // the second defect: flat from the centre out past the surface
        let r = 38.0;
        assert_eq!(engine_field(0.0, r), engine_field(r, r), "flat through the interior");
        assert_eq!(engine_field(r, r), engine_field(r * 1.9, r), "flat past the surface");
        // the truth does not do either of those
        assert!(
            true_field(r * 0.5, r, REF_R) < true_field(r, r, REF_R),
            "inside a body, gravity falls toward the centre"
        );
        assert!(true_field(r * 1.9, r, REF_R) < true_field(r, r, REF_R) * 0.35);
    }

    #[test]
    fn the_true_field_is_the_shell_theorem() {
        let r = 20.0;
        // zero at the centre, peak at the surface
        assert_eq!(true_field(0.0, r, REF_R), 0.0);
        let peak = true_field(r, r, REF_R);
        for d in [1.0f32, 5.0, 10.0, 19.0] {
            assert!(true_field(d, r, REF_R) < peak, "interior exceeded the surface at {d}");
        }
        // inverse square above it
        let a = true_field(r * 2.0, r, REF_R);
        let b = true_field(r * 4.0, r, REF_R);
        assert!((a / b - 4.0).abs() < 1e-4, "not inverse square: {a} {b}");
        // and continuous across the surface
        assert!((true_field(r - 0.001, r, REF_R) - true_field(r + 0.001, r, REF_R)).abs() < 1e-3);
    }

    #[test]
    fn surface_gravity_is_proportional_to_radius() {
        // constant density: g = GM/r^2 with M proportional to r^3
        for (a, b) in [(5.0f32, 10.0f32), (7.0, 38.0), (2.0, 26.0)] {
            let ratio = true_surface_g(b, REF_R) / true_surface_g(a, REF_R);
            assert!((ratio - b / a).abs() < 1e-4, "{a} to {b} gave {ratio}");
        }
    }

    #[test]
    fn the_void_pulls_and_that_is_the_worst_of_the_three() {
        // beyond every well the engine returns full gravity toward -Y
        let far = engine_field(10_000.0, 38.0);
        assert!((far - 1.0).abs() < 1e-6, "the far field is {far}, not 1.0");
        assert_eq!(far_field(Ambient::Grounded), 1.0);
        assert_eq!(far_field(Ambient::Void), 0.0);
        // on a grounded map that is correct and deliberate; in a space map it
        // is an invisible floor
        assert_ne!(far_field(Ambient::Grounded), far_field(Ambient::Void));
    }

    #[test]
    fn the_nearest_node_is_not_the_field_between_two_worlds() {
        // halfway between two equal worlds the real field is the SUM, and it
        // nearly cancels in direction while the engine simply picks one
        let r = 20.0;
        let midpoint = [(120.0f32, r), (120.0f32, r)];
        let sum = true_field_sum(&midpoint, REF_R);
        let one = true_field(120.0, r, REF_R);
        assert!((sum - one * 2.0).abs() < 1e-5, "the sum is not a sum");
        // and near one of them the sum is dominated by it, so the engine's
        // shortcut is fine THERE and only there
        let near = [(25.0f32, r), (215.0f32, r)];
        let dominant = true_field(25.0, r, REF_R);
        assert!(true_field_sum(&near, REF_R) < dominant * 1.1);
    }

    #[test]
    fn the_two_models_cross_somewhere_useful() {
        // below the crossover the engine over-pulls, above it under-pulls;
        // knowing where is what makes a retune possible rather than a guess
        for r in [7.0f32, 15.0, 38.0] {
            let Some(d) = crossover_m(r, REF_R) else { continue };
            assert!(d > 0.0);
            let below = engine_field(d * 0.5, r) - true_field(d * 0.5, r, REF_R);
            let above = engine_field(d * 1.6, r) - true_field(d * 1.6, r, REF_R);
            assert!(
                below * above < 0.0,
                "r={r}: no sign change around {d} ({below}, {above})"
            );
        }
    }

    #[test]
    fn the_reach_is_the_only_thing_size_actually_changes() {
        // the root of defect one, stated: the field SHAPE is identical for
        // every world because `t` is measured in units of that world's reach
        let shape = |r: f32| {
            (0..10).map(|i| {
                let d = r * REACH_MULT * (i as f32 / 10.0);
                let g = engine_field(d, r);
                (g - 1.0) / (engine_mul(r) - 1.0).abs().max(1e-6)
            }).collect::<Vec<f32>>()
        };
        let a = shape(7.0);
        let b = shape(38.0);
        for (x, y) in a.iter().zip(b.iter()) {
            assert!((x.abs() - y.abs()).abs() < 1e-3, "shapes differ: {x} {y}");
        }
    }
}

#[cfg(test)]
mod tube_tests {
    use super::*;

    #[test]
    fn the_tube_profile_is_inverted() {
        // spin gravity is zero on the axis and maximum at the wall; the
        // engine is maximum on the axis and weakest at the wall
        let r = 20.0;
        let (axis, wall) = (0.0, r / 1.45);
        assert!(engine_tube(axis, r) > engine_tube(wall, r), "engine peaks at the axis");
        assert!(true_tube(axis, r) < true_tube(wall, r), "spin peaks at the wall");
        // and they are close to exact mirrors across the bore
        let mid = wall * 0.5;
        assert!((engine_tube(mid, r) - (0.50 + 0.95 * 0.5)).abs() < 0.01);
        assert!((true_tube(mid, r * 0.69) - 0.5).abs() < 0.02);
    }

    #[test]
    fn weightless_on_the_axis_is_correct_and_was_read_as_a_bug() {
        // "a car flung into a wide pipe simply flew up the middle at constant
        // speed, never touching anything" — that is what a spinning habitat
        // does. The comment records it as the failure that motivated making
        // the axis strongest, which is how the profile got inverted.
        assert_eq!(true_tube(0.0, 20.0), 0.0);
        assert!(engine_tube(0.0, 20.0) > 1.0, "the engine flings you off the axis hardest");
    }

    #[test]
    fn spin_gravity_and_a_sphere_interior_are_the_same_shape() {
        // both are linear in radius, for the same reason: nothing outside
        // your radius contributes
        for f in [0.1f32, 0.4, 0.75, 1.0] {
            let tube = true_tube(f * 10.0, 10.0);
            let inside = true_field(f * 10.0, 10.0, 10.0);
            assert!((tube - inside).abs() < 1e-5, "at {f}: {tube} vs {inside}");
        }
    }
}
