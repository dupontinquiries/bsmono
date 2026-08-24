//! HOW A CHASSIS MOVES, decided from its own geometry.
//!
//! `physics.rs` has had three locomotion modes for a long time: wheeled
//! (grip, the default), `Car::hover` (a four-point cushion, a pulse
//! instead of a jump), and `Car::flight` (never grounded, lift and nose
//! thrust). Only ONE of them was ever assigned from data — the five
//! hand-written hovercars set `Model::hover` at load — so every chassis
//! that arrived as a pose strip drove on invisible wheels no matter what
//! it was shaped like. A hovertank sank onto its skirt and a jet drove.
//!
//! `partdetect` already answers the question that decides this, per prim:
//! is this thing a wheel, a thruster, a leg, a wing? And `cockpit` signs
//! the forward axis so "under the body" and "out the back" mean something.
//! This module is the decider that puts them together — the seam between
//! measurement and the physics mode, so the pipeline reads:
//!
//!   cloud -> orient (up/forward) -> cockpit (bow) -> partdetect (parts)
//!         -> locomotion::decide (mode) -> physics (Car::hover / flight)
//!
//! WHAT IT MEASURES, and what each measurement is worth:
//!
//! * **Wheels** — round, small, low, paired left/right. Wheels are the
//!   strongest signal in either direction: something with four of them
//!   rolls, and something with none does not, whatever else it has.
//! * **Thrusters** — tapered nozzles clustered aft. Thrust with no wheels
//!   is a flyer. `partdetect`'s own doc refuses to claim a nozzle's
//!   DIRECTION from taper, and this module respects that: it counts and
//!   locates nozzles, it does not infer which way they point.
//! * **Legs** — elongated, low, ground-touching. Legs plus no wheels is a
//!   walker, which this engine models as wheeled-with-grip today (the
//!   gait is `loco.rs`'s pose problem, not a physics mode) — so legs vote
//!   AGAINST hover, and nothing else.
//! * **A skirt** — a broad flat underside with no wheels and no legs is a
//!   cushion. This is the hovertank case, and it is the one the roster
//!   was missing entirely.
//!
//! HONESTY, up front: the thresholds below are reasoned from shapes, not
//! fitted to a labelled set — the same caveat `partdetect::classify`
//! writes about itself, inherited rather than laundered. What this module
//! adds over guessing is that the decision is one function with one set of
//! numbers, it explains itself (`Decision::why`), and a chassis manifest
//! can always override it (`decide_with_override`) — data beats detection
//! whenever data exists, which is the whole reason the five hand-written
//! hovercars keep their hand-written cushions.

use glam::Vec3;

use crate::physics::Hover;

/// How something gets around.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// tyres or feet: grip, grounded, jumps
    Wheeled,
    /// a cushion: no grip, a pulse instead of a jump
    Hover,
    /// never grounded: lift and nose thrust
    Flight,
}

/// What was measured and what it decided, so a driver can print the
/// reasoning instead of the verdict alone.
#[derive(Clone, Copy, Debug)]
pub struct Decision {
    pub mode: Mode,
    pub wheels: usize,
    pub thrusters: usize,
    pub legs: usize,
    /// how flat and broad the underside is, 0..1 — the skirt signal
    pub skirt: f32,
    /// how far aft the thruster cluster sits, -1 (bow) .. 1 (stern)
    pub thrust_aft: f32,
    pub why: &'static str,
}

/// A cushion sized for a body of this length. The five hand-written
/// hovercars keep their own tuned numbers; a DETECTED cushion gets these,
/// scaled off the hull so a tank and a scout do not share a ride height.
pub fn cushion_for(half_h: f32, half_len: f32) -> Hover {
    Hover {
        // ride the same fraction of body height regardless of size: a
        // cushion is a gap, and a gap that does not scale is a bug the
        // interior clearances already taught this codebase once
        ride: (half_h * 0.55).clamp(0.18, 1.10),
        // heavier hulls need a stiffer spring to hold the same gap
        stiff: (30.0 + half_len * 6.0).clamp(28.0, 70.0),
        // a skirt holds SOME line — less than a tyre, more than ice
        hold: 0.18,
        field_r: (half_len * 1.4).clamp(2.0, 5.0),
        field_k: 24.0,
        ..Hover::default()
    }
}

/// The threshold a wheel count has to clear to say "this rolls".
const ROLLS_AT: usize = 3;
/// Thruster count that says "this is propelled" absent wheels.
const FLIES_AT: usize = 2;
/// How flat-and-broad an underside must read to be a skirt.
const SKIRT_AT: f32 = 0.55;
/// How far aft thrusters must cluster to read as propulsion rather than
/// as attitude jets scattered over a hull.
const AFT_AT: f32 = 0.15;

/// Decide from counted parts and two measured shapes.
///
/// Kept separate from the cloud pass below so the decision is testable
/// with hand-written inputs — the counts come from `partdetect`, and this
/// is the part that turns them into a physics mode.
/// Treads are running gear. A tracked hull has no round things at the
/// corners, so `split_wheels` finds nothing and the skirt test would call
/// a tank a hovercraft — which is how a tank ends up floating. `turret`
/// detects the treads; this is where that answer has to arrive, and it
/// arrives as WHEELS because grip-on-the-ground is what treads are to
/// this physics.
pub fn decide_tracked(tracked: bool, wheels: usize, thrusters: usize, legs: usize, skirt: f32, aft: f32) -> Decision {
    if tracked {
        let mut d = decide(ROLLS_AT.max(wheels), thrusters, legs, skirt, aft);
        d.mode = Mode::Wheeled;
        d.why = "treads: it grips the ground";
        return d;
    }
    decide(wheels, thrusters, legs, skirt, aft)
}

pub fn decide(wheels: usize, thrusters: usize, legs: usize, skirt: f32, thrust_aft: f32) -> Decision {
    let d = |mode, why| Decision { mode, wheels, thrusters, legs, skirt, thrust_aft, why };
    // WHEELS WIN. Something with a full set of them rolls, whatever else
    // is bolted on — a jet with landing gear is still a jet in the air,
    // but this engine has one mode per chassis and the ground is where
    // the player spends the time.
    if wheels >= ROLLS_AT {
        return d(Mode::Wheeled, "a full set of wheels: it rolls");
    }
    // THRUST, AFT, AND NOTHING TOUCHING THE GROUND. All three: nozzles
    // scattered over a hull are attitude jets, and thrust on a legged
    // walker is a jetpack, not a flight model.
    if thrusters >= FLIES_AT && thrust_aft > AFT_AT && legs == 0 && wheels == 0 {
        return d(Mode::Flight, "aft thrust and nothing to stand on: it flies");
    }
    // A BROAD FLAT UNDERSIDE with nothing to roll or walk on is a skirt.
    // This is the hovertank, and it is what the roster never had a way to
    // say.
    if wheels == 0 && legs == 0 && skirt > SKIRT_AT {
        return d(Mode::Hover, "a skirt and no running gear: it floats");
    }
    // legs, one lonely wheel, or nothing legible: grip and a gait
    if legs > 0 {
        return d(Mode::Wheeled, "legs: it walks, which this engine grips");
    }
    d(Mode::Wheeled, "nothing decisive: the default rolls")
}

/// Measure a model's point cloud and decide.
///
/// The cloud is what every chassis already carries for the reflection
/// bake, so this needs no new export. Parts are found by CLUSTERING
/// rather than by prim, because prims do not survive upload as separate
/// vertex groups — a real per-prim pass through `partdetect::classify` is
/// the named upgrade, and it belongs here when the geometry does.
pub fn from_cloud(cloud: &[Vec3], up: Vec3, forward: Vec3, half_h: f32) -> Decision {
    if cloud.len() < 24 {
        return decide(0, 0, 0, 0.0, 0.0);
    }
    let n = cloud.len() as f32;
    let centroid = cloud.iter().copied().sum::<Vec3>() / n;
    let side = forward.cross(up).normalize_or(Vec3::X);
    let h = |p: &Vec3| (*p - centroid).dot(up);
    let f = |p: &Vec3| (*p - centroid).dot(forward);
    let x = |p: &Vec3| (*p - centroid).dot(side);

    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    let (mut zlo, mut zhi) = (f32::INFINITY, f32::NEG_INFINITY);
    for p in cloud {
        lo = lo.min(h(p));
        hi = hi.max(h(p));
        zlo = zlo.min(f(p));
        zhi = zhi.max(f(p));
    }
    let height = (hi - lo).max(1e-3);
    let length = (zhi - zlo).max(1e-3);

    // WHEELS: low, outboard, and in fore/aft PAIRS. Counting "low outboard
    // points" alone called a skirt four wheels, which is exactly the
    // mistake that would keep a hovertank on tyres — so the test is
    // whether the low band CLUSTERS into corners with gaps between them,
    // the thing a wheel has and a continuous skirt does not.
    let low_band = lo + height * 0.30;
    let mut quad = [0usize; 4];
    let mut low_n = 0usize;
    let mut skirt_span = 0.0f32;
    for p in cloud {
        if h(p) > low_band {
            continue;
        }
        low_n += 1;
        let q = (if f(p) > 0.0 { 1 } else { 0 }) + (if x(p) > 0.0 { 2 } else { 0 });
        quad[q] += 1;
        skirt_span = skirt_span.max(x(p).abs());
    }
    // a corner counts as running gear only if it is a real cluster
    let need = (low_n as f32 * 0.08).max(3.0) as usize;
    let corners = quad.iter().filter(|c| **c >= need).count();
    // GAPS: a wheeled underside is empty between the corners; a skirt is
    // not. Sample the mid-length band and see how much of it is filled.
    let mut mid_low = 0usize;
    for p in cloud {
        if h(p) <= low_band && f(p).abs() < length * 0.18 {
            mid_low += 1;
        }
    }
    let filled = if low_n > 0 { mid_low as f32 / low_n as f32 } else { 0.0 };
    // four corners with a hollow middle reads as wheels; four corners with
    // a full middle reads as a skirt
    let wheels = if corners >= 3 && filled < 0.22 { corners } else { 0 };
    let skirt = if wheels == 0 {
        // broad and flat: how much of the width the low band spans, times
        // how shallow it is
        let breadth = (skirt_span / (height * 0.9 + 1e-3)).clamp(0.0, 1.0);
        (breadth * 0.6 + filled * 0.8).clamp(0.0, 1.0)
    } else {
        0.0
    };

    // THRUSTERS: clusters standing off the stern. Counted in the aft
    // fifth, banded by height so a single wide exhaust does not read as
    // six nozzles.
    let aft = zhi - length * 0.20;
    let mut nozzles = [0usize; 4];
    let mut aft_sum = 0.0f32;
    let mut aft_n = 0.0f32;
    for p in cloud {
        if f(p) < aft {
            continue;
        }
        let b = (if h(p) > 0.0 { 1 } else { 0 }) + (if x(p) > 0.0 { 2 } else { 0 });
        nozzles[b] += 1;
        aft_sum += f(p) / (length * 0.5);
        aft_n += 1.0;
    }
    let nneed = (aft_n * 0.12).max(3.0) as usize;
    let thrusters = nozzles.iter().filter(|c| **c >= nneed).count();
    let thrust_aft = if aft_n > 0.0 { (aft_sum / aft_n).clamp(-1.0, 1.0) } else { 0.0 };

    // LEGS: tall narrow columns reaching the bottom, near the spine rather
    // than outboard — the shape a wheel is not.
    let mut legs = 0usize;
    for s in [-1.0f32, 1.0] {
        let mut col = 0usize;
        for p in cloud {
            if h(p) < lo + height * 0.45 && x(p) * s > 0.0 && x(p).abs() < skirt_span * 0.55 {
                col += 1;
            }
        }
        if col as f32 > n * 0.04 {
            legs += 1;
        }
    }
    // a legged body is TALL for its footprint; a low broad one is a skirt
    if height < half_h.max(1e-3) * 0.9 {
        legs = 0;
    }

    decide(wheels, thrusters, legs, skirt, thrust_aft)
}

/// Detection, unless the chassis said otherwise.
///
/// DATA BEATS DETECTION: the five hand-written hovercars carry tuned
/// cushions and must keep them, and a manifest that names a mode is a
/// statement by somebody who looked at the model. Detection is for the
/// hundred chassis nobody will ever hand-tune.
pub fn decide_with_override(declared: Option<Mode>, measured: Decision) -> Decision {
    match declared {
        Some(m) => Decision { mode: m, why: "declared by the chassis", ..measured },
        None => measured,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A boxy body plus whatever running gear the caller wants.
    fn body(w: f32, hgt: f32, len: f32) -> Vec<Vec3> {
        let mut c = Vec::new();
        for i in 0..14 {
            for j in 0..10 {
                let t = -1.0 + i as f32 * (2.0 / 13.0);
                let u = -1.0 + j as f32 * (2.0 / 9.0);
                c.push(Vec3::new(u * w, hgt, t * len));
                c.push(Vec3::new(u * w, hgt * 0.3, t * len));
            }
        }
        c
    }

    fn with_wheels(mut c: Vec<Vec3>, w: f32, len: f32) -> Vec<Vec3> {
        for (sx, sz) in [(-1.0f32, -1.0f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            for k in 0..16 {
                let a = k as f32 * 0.4;
                c.push(Vec3::new(sx * w, -0.3 + 0.25 * a.sin(), sz * len * 0.7 + 0.25 * a.cos()));
            }
        }
        c
    }

    #[test]
    fn four_wheels_roll() {
        let c = with_wheels(body(1.0, 0.6, 2.2), 1.0, 2.2);
        let d = from_cloud(&c, Vec3::Y, Vec3::NEG_Z, 0.6);
        assert_eq!(d.mode, Mode::Wheeled, "{d:?}");
    }

    #[test]
    fn a_flat_skirt_with_no_running_gear_floats() {
        // wide, low, continuous underside: the hovertank the roster never
        // had a way to describe
        let mut c = body(1.6, 0.35, 2.4);
        for i in 0..24 {
            for j in 0..12 {
                let t = -1.0 + i as f32 * (2.0 / 23.0);
                let u = -1.0 + j as f32 * (2.0 / 11.0);
                c.push(Vec3::new(u * 1.6, -0.30, t * 2.4));
            }
        }
        let d = from_cloud(&c, Vec3::Y, Vec3::NEG_Z, 0.35);
        assert_eq!(d.mode, Mode::Hover, "{d:?}");
        assert_eq!(d.wheels, 0, "a skirt was counted as wheels: {d:?}");
    }

    #[test]
    fn the_decision_table_is_the_one_that_decides() {
        // pure table, no geometry: these are the rules in one place
        assert_eq!(decide(4, 2, 0, 0.9, 0.9).mode, Mode::Wheeled);
        assert_eq!(decide(0, 4, 0, 0.1, 0.8).mode, Mode::Flight);
        assert_eq!(decide(0, 0, 0, 0.9, 0.0).mode, Mode::Hover);
        // thrust scattered over the hull is attitude jets, not propulsion
        assert_eq!(decide(0, 4, 0, 0.1, 0.0).mode, Mode::Wheeled);
        // a walker with a jetpack still walks
        assert_eq!(decide(0, 3, 2, 0.1, 0.9).mode, Mode::Wheeled);
    }

    #[test]
    fn treads_grip_rather_than_float() {
        // the tank bug: a flat wide underside with no round corners reads
        // as a skirt, so a tank hovers
        let skirted = decide(0, 0, 0, 0.9, 0.0);
        assert_eq!(skirted.mode, Mode::Hover, "the fixture does not reproduce the bug");
        let tracked = decide_tracked(true, 0, 0, 0, 0.9, 0.0);
        assert_eq!(tracked.mode, Mode::Wheeled, "a tank still floats: {tracked:?}");
        assert!(tracked.why.contains("treads"));
    }

    #[test]
    fn a_declaration_beats_the_measurement() {
        let m = from_cloud(&with_wheels(body(1.0, 0.6, 2.2), 1.0, 2.2), Vec3::Y, Vec3::NEG_Z, 0.6);
        assert_eq!(m.mode, Mode::Wheeled);
        let d = decide_with_override(Some(Mode::Hover), m);
        assert_eq!(d.mode, Mode::Hover);
        assert!(d.why.contains("declared"));
        // and the measurements survive the override, so a driver can print
        // what the geometry said even when it was overruled
        assert_eq!(d.wheels, m.wheels);
    }

    #[test]
    fn a_detected_cushion_scales_with_the_hull() {
        let small = cushion_for(0.35, 2.0);
        let big = cushion_for(1.4, 6.0);
        assert!(big.ride > small.ride, "a tank rides the same height as a scout");
        assert!(big.stiff > small.stiff, "a heavier hull got a softer spring");
        assert!(small.ride >= 0.18, "the gap collapsed on a small hull");
    }
}
