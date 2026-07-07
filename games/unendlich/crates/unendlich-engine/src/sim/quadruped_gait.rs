//! Quadruped gait — per-foot placement for a four-legged creature.
//!
//! Each foot is solved independently so a dragon crossing broken ground steps
//! onto whatever height is under *that* foot (cliff stepping), while the reach
//! to the hip is length-bounded so a leg never stretches past its limit (it
//! plants on the edge instead of dropping into a chasm). The four legs run a
//! diagonal trot (FL+RR, then FR+RL) with a lift curve during each swing.
//!
//! Pure and deterministic: world transforms + terrain heights in, foot
//! placements out. No GPU, no time source (the caller passes the phase).

/// Front-left, front-right, rear-left, rear-right.
pub const NUM_LEGS: usize = 4;

/// One leg's fixed parameters.
#[derive(Clone, Copy, Debug)]
pub struct Leg {
    /// Body-local rest position of the foot (X forward, Y up, Z right).
    pub rest_offset: [f32; 3],
    /// Length bound: the foot may never be farther than this from its hip.
    pub max_reach: f32,
}

/// Where one foot ends up this frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FootPlacement {
    /// World-space foot position.
    pub pos: [f32; 3],
    /// Height above the planted ground (0 = planted, >0 = mid-swing lift).
    pub lift: f32,
    /// True while the foot bears weight (stance), false mid-swing.
    pub planted: bool,
}

/// Diagonal-trot phase offset for a leg, in `[0,1)`. FL & RR share a phase;
/// FR & RL share the opposite phase.
pub fn leg_phase(leg: usize) -> f32 {
    match leg & 3 {
        0 => 0.0, // FL
        1 => 0.5, // FR
        2 => 0.5, // RL
        _ => 0.0, // RR
    }
}

fn rot_y(local: [f32; 3], yaw: f32) -> [f32; 3] {
    let (s, c) = yaw.sin_cos();
    [
        local[0] * c - local[2] * s,
        local[1],
        local[0] * s + local[2] * c,
    ]
}

/// Place one foot.
///
/// * `body_pos`, `yaw` — creature world transform (yaw 0 = facing +X).
/// * `leg`, `l` — which leg and its parameters.
/// * `gait_phase01` — global walk-cycle phase `0..1` (advance by distance).
/// * `speed01` — `0` = standing (feet planted, no lift), `1` = full stride.
/// * `ground_h` — terrain height directly under *this* foot's plant spot.
/// * `step_len` — stride length at full speed.
///
/// The hip is the rest offset rotated into world; the foot tracks a plant point
/// on `ground_h` that slides fore/aft over the cycle, lifting on the swing
/// half. The result is clamped so its distance from the hip never exceeds
/// `max_reach` (length-bounded traversal).
pub fn place_foot(
    body_pos: [f32; 3],
    yaw: f32,
    leg: usize,
    l: &Leg,
    gait_phase01: f32,
    speed01: f32,
    ground_h: f32,
    step_len: f32,
) -> FootPlacement {
    let hip = {
        let r = rot_y(l.rest_offset, yaw);
        [body_pos[0] + r[0], body_pos[1] + r[1], body_pos[2] + r[2]]
    };
    let speed = speed01.clamp(0.0, 1.0);
    // Per-leg phase within [0,1).
    let mut ph = gait_phase01 + leg_phase(leg);
    ph -= ph.floor();

    // Foot's resting ground point: directly under the hip, on this foot's ground.
    let base = [hip[0], ground_h, hip[2]];

    // Stance for the first half of the cycle, swing for the second.
    let (planted, fore, lift) = if speed < 1e-3 {
        (true, 0.0, 0.0)
    } else if ph < 0.5 {
        // Stance: foot fixed, sliding backward relative to travel.
        let s = ph / 0.5; // 0..1
        (true, (0.5 - s) * step_len * speed, 0.0)
    } else {
        // Swing: foot lifts and swings forward.
        let s = (ph - 0.5) / 0.5; // 0..1
        let lift = (s * core_pi()).sin() * step_len * 0.35 * speed;
        (false, (s - 0.5) * step_len * speed, lift)
    };

    // Forward is the creature's local +X in world.
    let fwd = rot_y([1.0, 0.0, 0.0], yaw);
    let mut pos = [
        base[0] + fwd[0] * fore,
        base[1] + lift,
        base[2] + fwd[2] * fore,
    ];

    // Length bound: clamp the foot inside max_reach of the hip.
    let d = [pos[0] - hip[0], pos[1] - hip[1], pos[2] - hip[2]];
    let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    if dist > l.max_reach && dist > 1e-6 {
        let k = l.max_reach / dist;
        pos = [hip[0] + d[0] * k, hip[1] + d[1] * k, hip[2] + d[2] * k];
    }

    FootPlacement { pos, lift, planted }
}

#[inline]
fn core_pi() -> f32 {
    core::f32::consts::PI
}

#[cfg(test)]
mod tests {
    use super::*;

    fn leg() -> Leg {
        Leg { rest_offset: [1.0, -1.2, 1.0], max_reach: 3.0 }
    }

    #[test]
    fn standing_feet_are_planted_and_flat() {
        for g in [0.0, 0.25, 0.5, 0.75] {
            let fp = place_foot([0.0, 5.0, 0.0], 0.0, 0, &leg(), g, 0.0, 0.0, 2.0);
            assert!(fp.planted && fp.lift == 0.0);
        }
    }

    #[test]
    fn each_foot_uses_its_own_ground_height() {
        // Same hip transform, different ground under each foot → different Y.
        let high = place_foot([0.0, 5.0, 0.0], 0.0, 0, &leg(), 0.1, 1.0, 4.0, 2.0);
        let low = place_foot([0.0, 5.0, 0.0], 0.0, 0, &leg(), 0.1, 1.0, 2.0, 2.0);
        assert!(high.pos[1] > low.pos[1], "cliff stepping: higher ground → higher foot");
    }

    #[test]
    fn foot_lifts_during_swing() {
        let stance = place_foot([0.0, 5.0, 0.0], 0.0, 0, &leg(), 0.2, 1.0, 0.0, 2.0);
        let swing = place_foot([0.0, 5.0, 0.0], 0.0, 0, &leg(), 0.75, 1.0, 0.0, 2.0);
        assert!(stance.planted && stance.lift == 0.0);
        assert!(!swing.planted && swing.lift > 0.0);
    }

    #[test]
    fn reach_is_length_bounded() {
        // Ground far below the hip → foot clamps to max_reach, never reaching it.
        let l = leg();
        let fp = place_foot([0.0, 50.0, 0.0], 0.0, 0, &l, 0.0, 0.0, -100.0, 2.0);
        let hip = [0.0 + 1.0, 50.0 - 1.2, 0.0 + 1.0];
        let d = [fp.pos[0] - hip[0], fp.pos[1] - hip[1], fp.pos[2] - hip[2]];
        let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        assert!(dist <= l.max_reach + 1e-3, "foot must stay within reach: {dist}");
    }

    #[test]
    fn diagonal_pairs_share_phase() {
        assert_eq!(leg_phase(0), leg_phase(3)); // FL == RR
        assert_eq!(leg_phase(1), leg_phase(2)); // FR == RL
        assert!(leg_phase(0) != leg_phase(1));
    }

    #[test]
    fn deterministic() {
        let a = place_foot([1.0, 5.0, 2.0], 0.7, 2, &leg(), 0.3, 0.8, 3.0, 1.5);
        let b = place_foot([1.0, 5.0, 2.0], 0.7, 2, &leg(), 0.3, 0.8, 3.0, 1.5);
        assert_eq!(a, b);
    }
}
