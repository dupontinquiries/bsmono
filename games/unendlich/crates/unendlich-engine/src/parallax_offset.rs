//! Parallax-mapping UV offset: shift along the view direction proportional to
//! surface height and scale, clamped so the texture doesn't swim.

pub fn offset(view_xy: [f32; 2], height: f32, scale: f32) -> [f32; 2] {
    let h = height.clamp(0.0, 1.0);
    let s = scale.max(0.0);
    [
        (view_xy[0] * h * s).clamp(-0.25, 0.25),
        (view_xy[1] * h * s).clamp(-0.25, 0.25),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_height_no_offset() { assert_eq!(offset([1.0, 1.0], 0.0, 0.1), [0.0, 0.0]); }
    #[test] fn more_height_more() { assert!(offset([1.0, 0.0], 0.8, 0.1)[0] > offset([1.0, 0.0], 0.2, 0.1)[0]); }
    #[test] fn scale_scales() { assert!(offset([1.0, 0.0], 0.5, 0.2)[0] > offset([1.0, 0.0], 0.5, 0.05)[0]); }
    #[test] fn bounded() {
        let o = offset([100.0, -100.0], 1.0, 10.0);
        assert!(o[0].abs() <= 0.25 && o[1].abs() <= 0.25);
    }
    #[test] fn sign_follows_view() { assert!((offset([0.5, 0.0], 0.4, 0.1)[0] + offset([-0.5, 0.0], 0.4, 0.1)[0]).abs() < 1e-6); }
    #[test] fn deterministic() { assert_eq!(offset([0.5, 0.3], 0.4, 0.1), offset([0.5, 0.3], 0.4, 0.1)); }
}
