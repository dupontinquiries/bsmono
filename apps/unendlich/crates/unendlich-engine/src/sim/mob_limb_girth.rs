//! `sim::mob_limb_girth` — a generalized "are the legs too fat while walking?"
//! check for voxel mobs. The animation rig may inflate limb boxes to bridge seams;
//! this guards against over-thickening: it samples each leg box's lateral radii
//! (the two axes perpendicular to the limb's long/bone axis) in the animated pose
//! and compares them to the rest pose. If any radius grows past a threshold (legs
//! visibly too wide) it flags. Pure; works on any `[ForgeBox]` + a leg predicate,
//! so dragons (or any future fauna) plug straight in.

use crate::sim::dragon_forge::ForgeBox;

/// The default "too wide" ratio: a walking leg may not exceed 1.1× its rest radius.
pub const GIRTH_THRESHOLD: f32 = 1.1;

// The two axes perpendicular to a box's longest (bone) axis — its lateral radii.
fn lateral_axes(rest_half: [f32; 3]) -> (usize, usize) {
    // longest rest half = the limb's length/bone axis; the other two are radii
    let long = if rest_half[0] >= rest_half[1] && rest_half[0] >= rest_half[2] {
        0
    } else if rest_half[1] >= rest_half[2] {
        1
    } else {
        2
    };
    match long {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    }
}

/// Largest lateral-radius growth ratio across all matched leg boxes. 1.0 = no
/// fattening; >1.0 = legs wider when animated than at rest. `rest` and `anim` must
/// be the same model's boxes (same order); `is_leg` selects which joints count.
pub fn leg_girth_ratio(rest: &[ForgeBox], anim: &[ForgeBox], is_leg: impl Fn(u8) -> bool) -> f32 {
    let mut worst = 1.0f32;
    for (r, a) in rest.iter().zip(anim.iter()) {
        if !is_leg(r.joint) {
            continue;
        }
        let (u, v) = lateral_axes(r.half);
        for &k in &[u, v] {
            let rest_r = r.half[k];
            if rest_r > 1e-4 {
                let ratio = a.half[k] / rest_r;
                if ratio.is_finite() && ratio > worst {
                    worst = ratio;
                }
            }
        }
    }
    worst
}

/// True if any leg's lateral radius exceeds `threshold`× its rest radius.
pub fn exceeds_girth(rest: &[ForgeBox], anim: &[ForgeBox], is_leg: impl Fn(u8) -> bool, threshold: f32) -> bool {
    leg_girth_ratio(rest, anim, is_leg) > threshold
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::dragon_archetype::Genome;
    use crate::sim::dragon_forge::{build, is_leg_joint, skin, ForgeAnim};

    #[test]
    fn legs_do_not_fatten_past_threshold_across_the_walk() {
        // sample the gait sweep every 5 "ticks" (gait phases) plus anim_time; the
        // legs' lateral radii must stay within GIRTH_THRESHOLD of rest the whole walk.
        for seed in [1u64, 7, 13, 21] {
            let m = build(&Genome::from_seed(seed), 64);
            let rest = m.boxes.clone();
            let mut tick = 0;
            while tick <= 60 {
                let gait = tick as f32 * 0.21; // a couple radii checked every 5 ticks
                let t = tick as f32 * 0.05;
                let anim = skin(&m, &ForgeAnim { gait_phase: gait, anim_time: t, ..Default::default() });
                let ratio = leg_girth_ratio(&rest, &anim, is_leg_joint);
                assert!(
                    ratio <= GIRTH_THRESHOLD,
                    "seed {seed} tick {tick}: legs {ratio:.3}× rest radius (> {GIRTH_THRESHOLD}) — too wide"
                );
                tick += 5;
            }
        }
    }

    #[test]
    fn detects_a_deliberately_fattened_leg() {
        let m = build(&Genome::from_seed(3), 64);
        let rest = m.boxes.clone();
        let mut fat = rest.clone();
        for b in fat.iter_mut() {
            if is_leg_joint(b.joint) {
                b.half = [b.half[0] * 1.5, b.half[1], b.half[2] * 1.5];
            }
        }
        assert!(exceeds_girth(&rest, &fat, is_leg_joint, GIRTH_THRESHOLD), "a 1.5× leg trips the check");
        assert!(!exceeds_girth(&rest, &rest, is_leg_joint, GIRTH_THRESHOLD), "rest vs rest is 1.0");
    }
}
