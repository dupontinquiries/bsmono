//! `current_force` — the physics that flow *causes*: a moving current pushes a submerged body
//! (force) and wears away the bed it runs over (erosion). Both are pure functions of the local
//! flow vector so they are fully testable; the bin samples the water flow field at a body/voxel
//! and feeds it here. Force is wired into the player's swim; erosion is groundwork the world sim
//! can use to carve channels where fast flow runs over soft substrate.

/// Horizontal drift velocity (blocks/s) imparted to a submerged body by a current `flow`
/// (the water flow field vector). `strength` scales flow→speed; the result is capped at
/// `max_speed`; `submersion` in [0,1] scales the push (a body barely in the water is pushed less
/// than one fully under). A still current imparts nothing.
pub fn drift_velocity(flow: [f32; 2], strength: f32, max_speed: f32, submersion: f32) -> [f32; 2] {
    let s = submersion.clamp(0.0, 1.0) * strength.max(0.0);
    let mut v = [flow[0] * s, flow[1] * s];
    let mag = (v[0] * v[0] + v[1] * v[1]).sqrt();
    if max_speed > 0.0 && mag > max_speed {
        let k = max_speed / mag;
        v[0] *= k;
        v[1] *= k;
    }
    v
}

/// Hardness of a substrate for erosion purposes: 0 = won't erode (bedrock/stone), 1 = erodes
/// readily (sand/silt). A convenience the bin can map block ids through.
pub fn substrate_softness(soft: bool) -> f32 {
    if soft {
        1.0
    } else {
        0.0
    }
}

/// Erosion rate (blocks of bed worn per second, before budgeting) for a current of speed
/// `flow_speed` (magnitude of the flow vector) running over a bed of `softness` in [0,1]. No
/// erosion until the flow exceeds `threshold` (slow water doesn't carve); above it, erosion grows
/// linearly with the excess speed, scaled by `gain` and the bed softness. Always >= 0.
pub fn erosion_rate(flow_speed: f32, threshold: f32, softness: f32, gain: f32) -> f32 {
    let excess = flow_speed - threshold;
    if excess <= 0.0 {
        return 0.0;
    }
    (excess * gain * softness.clamp(0.0, 1.0)).max(0.0)
}

/// Magnitude of a flow vector — small helper so callers don't re-derive it.
pub fn speed(flow: [f32; 2]) -> f32 {
    (flow[0] * flow[0] + flow[1] * flow[1]).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    #[test]
    fn still_water_no_push() {
        assert_eq!(drift_velocity([0.0, 0.0], 1.0, 5.0, 1.0), [0.0, 0.0]);
    }

    #[test]
    fn drift_follows_flow_direction() {
        let v = drift_velocity([2.0, 0.0], 0.5, 10.0, 1.0);
        assert!(v[0] > 0.0 && approx(v[1], 0.0));
    }

    #[test]
    fn drift_capped_at_max_speed() {
        let v = drift_velocity([100.0, 0.0], 1.0, 3.0, 1.0);
        assert!(approx(speed(v), 3.0));
    }

    #[test]
    fn submersion_scales_push() {
        let full = drift_velocity([4.0, 0.0], 1.0, 100.0, 1.0);
        let half = drift_velocity([4.0, 0.0], 1.0, 100.0, 0.5);
        assert!(approx(half[0], full[0] * 0.5));
    }

    #[test]
    fn submersion_clamped() {
        let over = drift_velocity([4.0, 0.0], 1.0, 100.0, 5.0);
        let one = drift_velocity([4.0, 0.0], 1.0, 100.0, 1.0);
        assert!(approx(over[0], one[0]));
    }

    #[test]
    fn slow_flow_does_not_erode() {
        assert_eq!(erosion_rate(0.3, 0.5, 1.0, 1.0), 0.0);
    }

    #[test]
    fn fast_flow_erodes_soft_bed() {
        let r = erosion_rate(2.0, 0.5, 1.0, 0.1);
        assert!(r > 0.0);
        // linear in excess: doubling the excess doubles the rate
        let r2 = erosion_rate(3.5, 0.5, 1.0, 0.1);
        assert!(approx(r2, r * 2.0));
    }

    #[test]
    fn hard_bed_does_not_erode() {
        assert_eq!(erosion_rate(10.0, 0.5, 0.0, 1.0), 0.0);
    }

    #[test]
    fn softness_helper() {
        assert_eq!(substrate_softness(true), 1.0);
        assert_eq!(substrate_softness(false), 0.0);
    }
}
