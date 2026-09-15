//! deform — squash & stretch, the skin-warping that gives a box creature weight
//! and snap. The rule is **volume preservation**: stretching a part along one
//! axis thins the other two, so a limb keeps its mass while reading as elastic —
//! a body lengthens as it leaps and flattens as it lands, never inflating. Pure;
//! the renderer applies the returned half-extents to a part's box before
//! `push_box_oriented`.

/// Elastic clamp on the stretch factor (0.5 = squashed to half, 2.0 = doubled).
pub const MIN_STRETCH: f32 = 0.5;
pub const MAX_STRETCH: f32 = 2.0;

/// Scale a box's `half` extents by stretching `axis` (0=x, 1=y, 2=z) by `stretch`
/// (>1 longer, <1 shorter), shrinking the other two axes by `1/√stretch` so the
/// box's volume is unchanged. `stretch` is clamped to the elastic range.
pub fn stretch_axis(half: [f32; 3], axis: usize, stretch: f32) -> [f32; 3] {
    let s = stretch.clamp(MIN_STRETCH, MAX_STRETCH);
    let perp = 1.0 / s.sqrt();
    let mut h = [half[0] * perp, half[1] * perp, half[2] * perp];
    h[axis.min(2)] = half[axis.min(2)] * s;
    h
}

/// Landing squash from a downward impact in `[0, 1]` (0 = none, 1 = hardest): the
/// box flattens vertically and bulges sideways, volume kept. A cheap version of
/// the weight settling onto contact.
pub fn landing_squash(half: [f32; 3], impact: f32) -> [f32; 3] {
    let i = impact.clamp(0.0, 1.0);
    // impact 0 → stretch 1.0 (none); impact 1 → 0.6 (squashed to 60% height).
    stretch_axis(half, 1, 1.0 - 0.4 * i)
}

/// Motion stretch: forward `speed` (relative to `max_speed`) lengthens the body
/// along its local +X (forward), up to +40% at full speed.
pub fn motion_stretch(half: [f32; 3], speed: f32, max_speed: f32) -> [f32; 3] {
    let t = if max_speed > 1e-3 { (speed / max_speed).clamp(0.0, 1.0) } else { 0.0 };
    stretch_axis(half, 0, 1.0 + 0.4 * t)
}

/// Volume of a box from its half-extents (for tests/diagnostics).
pub fn volume(half: [f32; 3]) -> f32 {
    8.0 * half[0] * half[1] * half[2]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn stretch_preserves_volume() {
        let h = [0.4, 0.6, 0.5];
        for &(axis, s) in &[(0usize, 1.6f32), (1, 0.7), (2, 1.3)] {
            let d = stretch_axis(h, axis, s);
            assert!(approx(volume(d), volume(h)), "axis {axis} s {s}: vol {} vs {}", volume(d), volume(h));
        }
    }

    #[test]
    fn stretch_lengthens_the_chosen_axis_and_thins_others() {
        let h = [0.5, 0.5, 0.5];
        let d = stretch_axis(h, 1, 1.6);
        assert!(d[1] > h[1], "Y should grow");
        assert!(d[0] < h[0] && d[2] < h[2], "X/Z should thin");
    }

    #[test]
    fn unit_stretch_is_identity() {
        let h = [0.3, 0.7, 0.4];
        let d = stretch_axis(h, 0, 1.0);
        assert!(approx(d[0], h[0]) && approx(d[1], h[1]) && approx(d[2], h[2]));
    }

    #[test]
    fn stretch_is_clamped() {
        let h = [0.5, 0.5, 0.5];
        let huge = stretch_axis(h, 0, 100.0);
        assert!(approx(huge[0], h[0] * MAX_STRETCH), "clamped to MAX_STRETCH");
    }

    #[test]
    fn landing_squash_flattens_and_bulges() {
        let h = [0.5, 0.5, 0.5];
        let s = landing_squash(h, 1.0);
        assert!(s[1] < h[1], "Y squashed");
        assert!(s[0] > h[0] && s[2] > h[2], "X/Z bulge");
        assert!(approx(volume(s), volume(h)), "volume kept");
        // No impact = no change.
        let none = landing_squash(h, 0.0);
        assert!(approx(none[1], h[1]));
    }

    #[test]
    fn motion_stretch_lengthens_forward_with_speed() {
        let h = [0.5, 0.5, 0.5];
        let fast = motion_stretch(h, 10.0, 10.0);
        let slow = motion_stretch(h, 1.0, 10.0);
        assert!(fast[0] > slow[0], "faster → longer");
        assert!(approx(volume(fast), volume(h)));
        assert!(approx(motion_stretch(h, 0.0, 10.0)[0], h[0]), "at rest, no stretch");
    }
}
