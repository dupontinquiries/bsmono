//! Breath-weapon coverage: intensity in [0,1] for a target at `angle_to_target`
//! from the breath axis — full near the axis, falling to 0 at the cone edge. Pure.

pub fn intensity(angle_to_target: f32, cone_half_angle: f32) -> f32 {
    let a = angle_to_target.abs();
    let h = cone_half_angle.max(1e-6);
    if a >= h {
        0.0
    } else {
        let t = 1.0 - a / h;
        (t * t).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn on_axis_full() { assert!((intensity(0.0, 0.5) - 1.0).abs() < 1e-6); }
    #[test] fn edge_zero() { assert_eq!(intensity(0.5, 0.5), 0.0); }
    #[test] fn beyond_zero() { assert_eq!(intensity(1.0, 0.5), 0.0); }
    #[test] fn symmetric() { assert_eq!(intensity(0.2, 0.5), intensity(-0.2, 0.5)); }
    #[test] fn monotone_toward_axis() {
        let (mut prev, mut a) = (f32::MIN, 0.5f32);
        while a > 0.0 { let v = intensity(a, 0.5); assert!(v >= prev - 1e-6); prev = v; a -= 0.02; }
    }
    #[test] fn deterministic() { assert_eq!(intensity(0.2, 0.5).to_bits(), intensity(0.2, 0.5).to_bits()); }
}
