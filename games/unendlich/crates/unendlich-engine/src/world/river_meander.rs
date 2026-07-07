//! `river_meander` — lateral offset of a river centerline.
//!
//! Worker-03 / WORLD pure core, round 25. A river's centerline pushes sideways
//! where it bends: the offset grows with the channel's curvature and flow, takes
//! the sign of the curvature (bends deepen the way they already lean), and is
//! capped at an amplitude. A straight reach has no offset. Pairs with
//! `river_mouth` (R17) / `river_carve` (R9). Pure and deterministic.
//!
//! Self-contained for `rustc --test`.
#![allow(dead_code)]

const FLOW_K: f32 = 1.0; // flow at which the flow term is ~half
const CURV_K: f32 = 1.0; // curvature at which the bend term is ~half

/// Signed lateral offset for a centerline point with `curvature` (signed),
/// `flow`, and a max `amplitude`. Zero when straight; sign follows curvature;
/// magnitude capped at `amplitude`.
pub fn meander_offset(curvature: f32, flow: f32, amplitude: f32) -> f32 {
    let amp = amplitude.max(0.0);
    let f = flow.max(0.0);
    let flow_term = f / (f + FLOW_K); // [0,1)
    let kabs = curvature.abs();
    let curv_term = kabs / (kabs + CURV_K); // 0 at straight, →1 sharp
    let mag = amp * flow_term * curv_term;
    let signed = mag * sign(curvature);
    clamp_abs(signed, amp)
}

fn sign(v: f32) -> f32 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        0.0
    }
}
fn clamp_abs(v: f32, m: f32) -> f32 {
    if v > m {
        m
    } else if v < -m {
        -m
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_is_zero() {
        assert_eq!(meander_offset(0.0, 5.0, 10.0), 0.0);
    }

    #[test]
    fn curve_offsets() {
        assert!(meander_offset(0.5, 5.0, 10.0).abs() > 0.0);
    }

    #[test]
    fn flow_scales() {
        assert!(meander_offset(0.5, 10.0, 10.0) > meander_offset(0.5, 1.0, 10.0));
    }

    #[test]
    fn amplitude_caps() {
        let v = meander_offset(1e6, 1e6, 10.0);
        assert!(v.abs() <= 10.0 + 1e-4);
    }

    #[test]
    fn sign_follows_curvature() {
        assert!(meander_offset(0.5, 5.0, 10.0) > 0.0);
        assert!(meander_offset(-0.5, 5.0, 10.0) < 0.0);
    }

    #[test]
    fn deterministic() {
        assert_eq!(meander_offset(0.3, 4.0, 8.0), meander_offset(0.3, 4.0, 8.0));
    }
}
