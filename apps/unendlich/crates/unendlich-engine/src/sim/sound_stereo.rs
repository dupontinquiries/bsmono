//! `sim::sound_stereo` — split a resolved mono gain into left/right.
//!
//! [`sound_resolve`](crate::sim::sound_resolve) gives the gain a sound reaches
//! the listener with; this places it in the stereo field from the source's
//! direction relative to where the listener is facing. Constant-power pan: a
//! source hard to one side plays entirely in that ear, dead ahead (or behind)
//! sits centred, and total power is preserved (`left² + right² == gain²`) so a
//! sound doesn't get louder or quieter as it sweeps across. Pure and
//! deterministic; completes the output chain `sound_resolve` → `voice_mix` →
//! `sound_stereo`.

use std::f32::consts::FRAC_PI_4;

/// Left/right gains for one voice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StereoGains {
    pub left: f32,
    pub right: f32,
}

#[inline]
fn norm3(v: [f32; 3]) -> Option<[f32; 3]> {
    let m = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if m > 1e-6 {
        Some([v[0] / m, v[1] / m, v[2] / m])
    } else {
        None
    }
}

/// Pan `gain` for a source at `source_pos` heard by a listener at `listener_pos`
/// facing `listener_forward` (world up is +Y). The pan is the source direction's
/// component along the listener's right vector, mapped constant-power. Degenerate
/// geometry (source on the listener, or looking straight up/down) → centred. The
/// returned gains are non-negative and satisfy `left² + right² == gain²`.
pub fn stereo(
    gain: f32,
    listener_pos: [f32; 3],
    listener_forward: [f32; 3],
    source_pos: [f32; 3],
) -> StereoGains {
    let g = gain.max(0.0);

    // Listener right = forward × up, normalised.
    let f = norm3(listener_forward);
    let dir = norm3([
        source_pos[0] - listener_pos[0],
        source_pos[1] - listener_pos[1],
        source_pos[2] - listener_pos[2],
    ]);

    let pan = match (f, dir) {
        (Some(f), Some(d)) => {
            // right = f × up,  up = (0,1,0)  ⇒  (f.z, 0, -f.x)
            let right = [f[2], 0.0, -f[0]];
            match norm3(right) {
                Some(r) => (d[0] * r[0] + d[1] * r[1] + d[2] * r[2]).clamp(-1.0, 1.0),
                None => 0.0, // forward parallel to up ⇒ no stereo axis
            }
        }
        _ => 0.0,
    };

    // Constant-power: theta in [0, pi/2]; -1 ⇒ full left, +1 ⇒ full right.
    let theta = (pan + 1.0) * FRAC_PI_4;
    StereoGains { left: g * theta.cos(), right: g * theta.sin() }
}

#[cfg(test)]
mod tests {
    use super::*;

    // listener at origin, facing +Z (so right is −X... verify with the math).
    fn forward() -> [f32; 3] {
        [0.0, 0.0, 1.0]
    }

    fn power(s: StereoGains) -> f32 {
        s.left * s.left + s.right * s.right
    }

    #[test]
    fn source_ahead_is_centred() {
        let s = stereo(1.0, [0.0; 3], forward(), [0.0, 0.0, 10.0]);
        assert!((s.left - s.right).abs() < 1e-5, "dead ahead ⇒ centred");
    }

    #[test]
    fn left_and_right_separate() {
        // facing +Z, right vector = (f.z,0,-f.x) = (1,0,0) = +X.
        let right_src = stereo(1.0, [0.0; 3], forward(), [10.0, 0.0, 0.0]); // +X
        let left_src = stereo(1.0, [0.0; 3], forward(), [-10.0, 0.0, 0.0]); // -X
        assert!(right_src.right > right_src.left, "+X source favours the right ear");
        assert!(left_src.left > left_src.right, "-X source favours the left ear");
    }

    #[test]
    fn hard_pan_is_one_sided() {
        let r = stereo(1.0, [0.0; 3], forward(), [100.0, 0.0, 0.0]);
        assert!(r.left < 1e-3, "a hard-right source is silent on the left");
        assert!((r.right - 1.0).abs() < 1e-3);
    }

    #[test]
    fn constant_power_across_the_sweep() {
        for &x in &[-100.0f32, -10.0, -1.0, 0.0, 1.0, 10.0, 100.0] {
            let s = stereo(1.0, [0.0; 3], forward(), [x, 0.0, 5.0]);
            assert!((power(s) - 1.0).abs() < 1e-4, "total power preserved as it pans");
        }
    }

    #[test]
    fn gain_scales_and_degenerate_is_centred() {
        let s = stereo(0.5, [0.0; 3], forward(), [3.0, 0.0, 0.0]);
        assert!((power(s) - 0.25).abs() < 1e-4, "power scales with gain²");
        // source on the listener ⇒ centred, finite
        let c = stereo(1.0, [0.0; 3], forward(), [0.0; 3]);
        assert!((c.left - c.right).abs() < 1e-5 && c.left.is_finite());
        // looking straight up ⇒ no stereo axis ⇒ centred
        let up = stereo(1.0, [0.0; 3], [0.0, 1.0, 0.0], [5.0, 0.0, 0.0]);
        assert!((up.left - up.right).abs() < 1e-5);
    }

    #[test]
    fn deterministic() {
        assert_eq!(
            stereo(0.8, [1.0, 2.0, 3.0], [1.0, 0.0, 0.0], [4.0, 2.0, 9.0]),
            stereo(0.8, [1.0, 2.0, 3.0], [1.0, 0.0, 0.0], [4.0, 2.0, 9.0])
        );
    }
}
