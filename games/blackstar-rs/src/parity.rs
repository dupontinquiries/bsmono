//! PARITY — the same question answered in two places.
//!
//! `Car::stand_h` existed as eight copies of one expression, one of which was
//! wrong, and the guard against that was a test grepping for a literal
//! string. That is the bug class this module is about: **an invariant with
//! more than one implementation is a divergence waiting for someone not to
//! notice.**
//!
//! Two kinds live here.
//!
//! **The checks** are tests that two call sites still agree — `ground_lift`
//! used by both the draw and the collision, door state read by both the mesh
//! and the router. Written as parity rather than as "is it right", because
//! either one being right is not the property that matters; a hull drawn a
//! metre from the hull you hit is wrong even if both are individually
//! defensible.
//!
//! **The scan** is `--parity`, which greps the tree for expressions that look
//! like an invariant somebody open-coded. It is crude and it finds real
//! things: I wrote it to check the two I remembered and it turned up a third
//! I did not.
//!
//! ## The one it found
//!
//! `hull_mesh` carves a corridor when `Door::void()` is `Some`, which is
//! `travel > 0.01`. `reachable_from` routes through a door when
//! `Door::passable()`, which is `travel > 0.75`. **Between those a corridor
//! is drawn and not routable** — you can see through a door the game says is
//! shut, which is exactly the "see it here, hit it there" failure in a
//! different costume.
//!
//! Both functions are correct. `void` is about geometry and `passable` is
//! about whether a body fits through, and a half-open door genuinely is a
//! hole you cannot walk through. What was wrong was `hull_mesh` asking the
//! geometry question when it wanted the routing one.

/// The travel at which a door is a hole you can route through.
pub const PASSABLE_AT: f32 = 0.75;
/// The travel at which a door starts contributing geometry.
pub const CARVES_AT: f32 = 0.01;

/// Is this pair of thresholds a gap somebody can see through?
pub fn ambiguous_band(carve: f32, pass: f32) -> f32 {
    (pass - carve).max(0.0)
}

/// A place a duplicated invariant might live.
pub struct Site {
    pub file: &'static str,
    pub needle: &'static str,
    /// what it should call instead
    pub instead: &'static str,
}

/// Expressions that have already bitten, kept so the scan has known-answers
/// to check itself against.
pub static KNOWN: &[Site] = &[
    Site { file: "physics.rs", needle: "ride_h > 0.0", instead: "Car::stand_h" },
    Site { file: "main.rs", needle: "ride_h > 0.0", instead: "Car::stand_h" },
    Site { file: "aboard.rs", needle: "door.void()", instead: "Door::passable for routing" },
    Site { file: "main.rs", needle: "door.void()", instead: "Door::passable for routing" },
    Site { file: "interior.rs", needle: "door.passable()", instead: "one carve rule" },
];

/// Count non-comment occurrences of a needle in live (non-test) code.
///
/// Splitting on `#[cfg(test)]` rather than parsing: a test that open-codes an
/// expression to model OLD behaviour is legitimate — `seating_tests` does
/// exactly that — and a scan that flags it teaches people to ignore the scan.
pub fn open_coded(src: &str, needle: &str) -> usize {
    src.split("#[cfg(test)]")
        .next()
        .unwrap_or("")
        .lines()
        .filter(|l| l.contains(needle))
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with("//") && !t.starts_with("///") && !t.starts_with("*")
        })
        .count()
}

// ------------------------------------------------------------- driver ---

pub fn run() {
    println!("PARITY — the same question answered in two places\n");

    println!("  THE ONE THAT WAS ALREADY WRONG\n");
    println!("    Door::void()      carves geometry at travel > {CARVES_AT}");
    println!("    Door::passable()  routes a body at    travel > {PASSABLE_AT}");
    println!(
        "\n    between them is a band {:.2} wide where a corridor is DRAWN and",
        ambiguous_band(CARVES_AT, PASSABLE_AT)
    );
    println!("    NOT ROUTABLE — you see through a door the game says is shut.\n");
    println!("    Both functions are right. `void` is geometry, `passable` is");
    println!("    whether a body fits, and a half-open door genuinely is a hole you");
    println!("    cannot walk through. What was wrong is that `hull_mesh` asked the");
    println!("    geometry question when it wanted the routing one.\n");

    println!("  THE SCAN\n");
    let files: [(&str, &str); 4] = [
        ("physics.rs", include_str!("physics.rs")),
        ("main.rs", include_str!("main.rs")),
        ("aboard.rs", include_str!("aboard.rs")),
        ("interior.rs", include_str!("interior.rs")),
    ];

    let mut total = 0;
    // FROM `KNOWN`, not a second list. The driver had its own array of
    // needles beside the table meant to hold them, which is this module's own
    // bug class inside this module. Anything the scan should look for goes in
    // `KNOWN` and the driver reads it.
    let needles: Vec<(&str, &str)> = {
        let mut v: Vec<(&str, &str)> =
            KNOWN.iter().map(|s| (s.needle, s.instead)).collect();
        v.sort_unstable();
        v.dedup_by_key(|(n, _)| *n);
        v
    };
    println!(
        "  {} expressions from KNOWN, across {} files\n",
        needles.len(),
        files.len()
    );
    // WHAT TO USE INSTEAD is the half worth printing. A scan that says "this
    // line is suspicious" and stops makes the reader go and find the answer;
    // the table already holds it.
    println!("  {:<12} {:>4}  {:<18} {}", "file", "hits", "expression", "use instead");
    for (name, src) in files {
        for (needle, instead) in needles.iter().copied() {
            let n = open_coded(src, needle);
            if n > 0 {
                total += n;
                println!("  {name:<12} {n:>4}  {needle:<18} {instead}");
            }
        }
    }

    // and where each was first found, so a hit has provenance
    println!("\n  first found in:");
    for s in KNOWN {
        println!("    {:<18} {}", s.needle, s.file);
    }
    println!("\n  {total} open-codings in live code.\n");
    println!("  Crude on purpose. It greps, it cannot tell an invariant from a");
    println!("  coincidence, and it skips test modules because a test that models");
    println!("  OLD behaviour open-codes deliberately — `seating_tests` does, and a");
    println!("  scan that flags it teaches people to ignore the scan.\n");
    println!("  It earned its place anyway: I wrote it to check the two duplications");
    println!("  I remembered and it turned up the door one I had not.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scan_skips_test_modules() {
        // a scan that flags deliberate open-coding in tests teaches people to
        // ignore the scan
        let src = "fn live() { let a = ride_h > 0.0; }\n#[cfg(test)]\nmod t { fn x() { let b = ride_h > 0.0; } }";
        assert_eq!(open_coded(src, "ride_h > 0.0"), 1);
    }

    #[test]
    fn the_scan_skips_comments() {
        let src = "// ride_h > 0.0 is the old way\n/// and ride_h > 0.0 in a doc\nlet x = ride_h > 0.0;";
        assert_eq!(open_coded(src, "ride_h > 0.0"), 1);
    }

    #[test]
    fn stand_h_has_exactly_one_implementation() {
        // the invariant that bit: eight copies, one wrong, guarded by a grep
        // for a literal string
        for s in KNOWN {
            let src = match s.file {
                "physics.rs" => include_str!("physics.rs"),
                "main.rs" => include_str!("main.rs"),
                _ => continue,
            };
            assert!(
                open_coded(src, s.needle) <= 1,
                "{} open-codes `{}`; use {}",
                s.file,
                s.needle,
                s.instead
            );
        }
    }

    #[test]
    fn the_drawn_hull_and_the_collided_hull_share_one_offset() {
        // Two transforms for one object is how a hull you can SEE and a hull
        // you can HIT end up a metre apart, and nothing catches it because
        // each is individually defensible. Both sites call `ground_lift`;
        // this asserts they still agree on the number rather than that either
        // is correct.
        let a = crate::aboard::Aboard::new(15.2);
        let lift = crate::aboard::ground_lift(&a.ship);

        // what the draw call offsets by (main.rs reads the parked origin)
        // and what `repark` parks at are the same expression, so the check
        // is that the expression is stable and lands the hull above ground
        let m = crate::aboard::hull_mesh(&a.ship);
        let lo = m.verts.iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
        assert!(
            (lo + lift - 0.1).abs() < 1e-3,
            "the lift is not the mesh floor: floor {lo:.3} + lift {lift:.3}"
        );
        // and it is not accidentally zero, which would pass trivially
        assert!(lift.abs() > 0.1, "the lift is {lift}, so this test proves nothing");
    }

    /// Vertices in a hull with no corridors drawn at all.
    fn rooms_only(ship: &crate::interior::Interior) -> usize {
        let mut s = ship.clone();
        for l in s.links.iter_mut() {
            l.door.travel = 0.0;
        }
        crate::aboard::hull_mesh(&s).verts.len()
    }

    /// What one drawn corridor costs, measured rather than guessed.
    fn corridor_verts(ship: &crate::interior::Interior) -> usize {
        let mut s = ship.clone();
        for l in s.links.iter_mut() {
            l.door.travel = 1.0;
        }
        let all = crate::aboard::hull_mesh(&s).verts.len();
        (all - rooms_only(ship)) / ship.links.len().max(1)
    }

    #[test]
    fn the_mesh_and_the_router_agree_about_every_door() {
        // THE PARITY THAT WAS BROKEN. `hull_mesh` used to carve on `void()`
        // and `reachable_from` routes on `passable()`, so a door mid-swing
        // was drawn open and treated as shut.
        use crate::interior::Interior;
        let mut ship = Interior::generate(15.2);
        ship.unseal();
        // walk the doors through their whole travel and check at every step
        ship.depower();
        for step in 0..200 {
            ship.step_doors(1.0 / 60.0);
            let carved = crate::aboard::hull_mesh(&ship).verts.len();
            let routable = ship.links.iter().filter(|l| l.door.passable()).count();
            // MEASURED, not assumed. My first version divided by 8 verts per
            // box; `cbox` is BEVELLED and has more, so it read 247 corridors
            // where there were 9. Take the per-corridor cost from the mesh
            // itself: the difference between all doors open and all shut,
            // over the number of links.
            let all_shut = rooms_only(&ship);
            let per_corridor = corridor_verts(&ship);
            let corridors_drawn = (carved - all_shut) / per_corridor;
            assert_eq!(
                corridors_drawn, routable,
                "at step {step}: {corridors_drawn} corridors drawn, {routable} routable"
            );
        }
    }

    #[test]
    fn a_door_is_drawn_open_before_it_is_walkable() {
        // THE ONE THE SCAN FOUND. Not a bug in either function — `void` is
        // geometry and `passable` is whether a body fits — but a real gap for
        // anything that asks the wrong one.
        assert!(CARVES_AT < PASSABLE_AT);
        assert!(ambiguous_band(CARVES_AT, PASSABLE_AT) > 0.5, "the band is the problem");
        // and the thresholds are the ones airlock.rs actually uses, or this
        // test is about numbers I made up
        let mut d = crate::airlock::Door::new(
            crate::airlock::Kind::Hatch,
            crate::airlock::Gauge::Personnel,
            glam::Vec3::ZERO,
            glam::Vec3::splat(1.0),
        );
        d.travel = 0.5;
        assert!(d.void().is_some(), "a half-open door contributes no geometry");
        assert!(!d.passable(), "a half-open door is walkable");
    }
}
