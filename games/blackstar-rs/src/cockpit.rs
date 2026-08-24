//! COCKPIT DETECTION — which end of a flyer is the front, from its shape.
//!
//! `orient.rs` guesses up and forward for a whole body, and its own doc
//! admits the hard part: fore-aft SIGN. A fuselage is nearly symmetric
//! end to end, so a forward axis derived from elongation alone flips a
//! coin — and a flipped flyer puts the nose camera at the tail and would
//! hand a procedural interior a bridge facing the engines. The
//! disambiguator every aircraft carries is the COCKPIT: pilots sit high
//! and toward the bow, and `procars` builds exactly that (a canopy sunk
//! into the body, forward of centre).
//!
//! This module finds it from the model's own point cloud — the
//! `Model.cloud` every chassis already carries for the reflection bake,
//! so detection needs no new data. The method is measurement, in
//! `partdetect`'s spirit: take the TOP BAND of the cloud (a canopy sits
//! proud of the fuselage), split it fore and aft of the centroid along
//! the candidate forward axis, and the side with the taller, tighter
//! bump is the bow. `signed_forward` then returns the forward axis with
//! its sign fixed, or the axis unchanged (and `confident = false`) when
//! the shape refuses to say — a symmetric hull keeps orient's coin flip
//! rather than trading it for mine.
//!
//! Consumers today: the nose camera anchors at the DETECTED cockpit of
//! the active flyer instead of a fixed fraction of the hull. Named next:
//! `interior.rs` orienting its room graph bow-ward off the same guess,
//! and per-prim `partdetect::classify` once prim vertex groups survive
//! upload.

use glam::Vec3;

/// What the cloud said.
#[derive(Clone, Copy, Debug)]
pub struct CockpitGuess {
    /// canopy centroid, model-local
    pub pos: Vec3,
    /// forward with its fore-aft sign fixed toward the cockpit
    pub forward: Vec3,
    /// false when the shape was too symmetric to call
    pub confident: bool,
}

/// The top band: points in the upper quarter of the body's height along
/// `up`. A canopy sits proud; a belly pod does not vote.
const TOP_BAND: f32 = 0.25;
/// How much taller one end's bump must be than the other's to call it,
/// as a fraction of body height. Below this the hull is symmetric and we
/// say so instead of guessing.
const CALL_MARGIN: f32 = 0.04;

/// Find the cockpit and sign the forward axis with it.
///
/// `up` and `forward` are the axes `orient::guess` (or the model's own
/// convention) proposes; only forward's SIGN is in question here.
pub fn signed_forward(cloud: &[Vec3], up: Vec3, forward: Vec3) -> CockpitGuess {
    let unsure = CockpitGuess { pos: Vec3::ZERO, forward, confident: false };
    if cloud.len() < 16 {
        return unsure;
    }
    let n = cloud.len() as f32;
    let centroid = cloud.iter().copied().sum::<Vec3>() / n;
    let h = |p: &Vec3| (*p - centroid).dot(up);
    let f = |p: &Vec3| (*p - centroid).dot(forward);
    let (mut lo, mut hi) = (f32::INFINITY, f32::NEG_INFINITY);
    for p in cloud {
        let y = h(p);
        lo = lo.min(y);
        hi = hi.max(y);
    }
    let height = hi - lo;
    if height < 1e-3 {
        return unsure;
    }
    let band = hi - height * TOP_BAND;
    // the tallest thing forward of centre vs the tallest thing aft:
    // whichever end carries the higher crest carries the canopy
    let (mut crest_fore, mut crest_aft) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    let (mut sum_fore, mut n_fore) = (Vec3::ZERO, 0.0f32);
    for p in cloud {
        let y = h(p);
        if y < band {
            continue;
        }
        if f(p) > 0.0 {
            crest_fore = crest_fore.max(y);
            sum_fore += *p;
            n_fore += 1.0;
        } else {
            crest_aft = crest_aft.max(y);
        }
    }
    let (crest_fore, crest_aft) = (
        if crest_fore.is_finite() { crest_fore } else { lo },
        if crest_aft.is_finite() { crest_aft } else { lo },
    );
    let margin = (crest_fore - crest_aft) / height;
    if margin.abs() < CALL_MARGIN {
        return unsure;
    }
    let fwd = if margin > 0.0 { forward } else { -forward };
    // re-collect the canopy centroid on the CHOSEN bow side
    let (mut sum, mut count) = (Vec3::ZERO, 0.0f32);
    for p in cloud {
        if h(p) >= band && (*p - centroid).dot(fwd) > 0.0 {
            sum += *p;
            count += 1.0;
        }
    }
    let pos = if count > 0.0 { sum / count } else { sum_fore / n_fore.max(1.0) };
    CockpitGuess { pos, forward: fwd, confident: true }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A synthetic jet: a long fuselage of points on the -Z..+Z line, a
    /// canopy bump forward-top (+Z side), flat wings amidships.
    fn jet(canopy_at: f32) -> Vec<Vec3> {
        let mut c = Vec::new();
        for i in 0..60 {
            let z = -3.0 + i as f32 * 0.1;
            c.push(Vec3::new(0.0, 0.0, z));
            c.push(Vec3::new(0.3, 0.1, z));
            c.push(Vec3::new(-0.3, 0.1, z));
        }
        for i in 0..12 {
            let x = -1.8 + i as f32 * 0.3;
            c.push(Vec3::new(x, 0.05, 0.0));
        }
        for i in 0..10 {
            let t = i as f32 * 0.08;
            c.push(Vec3::new(0.0, 0.45 + 0.1 * (1.0 - t), canopy_at + t));
        }
        c
    }

    #[test]
    fn the_canopy_names_the_bow_whatever_sign_the_guess_arrived_with() {
        // canopy forward: +Z proposal keeps its sign
        let g = signed_forward(&jet(1.4), Vec3::Y, Vec3::Z);
        assert!(g.confident);
        assert!(g.forward.z > 0.9, "{:?}", g.forward);
        assert!(g.pos.z > 0.5, "the canopy centroid is not at the bow: {:?}", g.pos);
        // same jet, the proposal arrives FLIPPED — detection must unflip it
        let g = signed_forward(&jet(1.4), Vec3::Y, -Vec3::Z);
        assert!(g.confident, "a flipped proposal broke the call");
        assert!(g.forward.z > 0.9, "the flip survived: {:?}", g.forward);
        // and a tail-sitter canopy (aft) flips a +Z proposal the other way
        let g = signed_forward(&jet(-1.4), Vec3::Y, Vec3::Z);
        assert!(g.confident);
        assert!(g.forward.z < -0.9, "{:?}", g.forward);
    }

    #[test]
    fn a_symmetric_hull_refuses_to_call_it() {
        // fuselage with identical bumps both ends: keep orient's answer,
        // say so via `confident`
        let mut c = jet(1.4);
        for i in 0..10 {
            let t = i as f32 * 0.08;
            c.push(Vec3::new(0.0, 0.45 + 0.1 * (1.0 - t), -1.4 - t));
        }
        let g = signed_forward(&c, Vec3::Y, Vec3::Z);
        assert!(!g.confident, "a symmetric hull got a confident call");
        assert_eq!(g.forward, Vec3::Z, "an unsure call must not touch the axis");
    }

    #[test]
    fn a_handful_of_points_is_not_a_shape() {
        let g = signed_forward(&[Vec3::ZERO, Vec3::Y, Vec3::X], Vec3::Y, Vec3::Z);
        assert!(!g.confident);
    }
}
