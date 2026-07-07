//! Temperature across a weather front: smoothstep interpolation from the warm
//! air-mass temp to the cold air-mass temp over the front's width. Pure.

pub fn temperature(warm_temp: f32, cold_temp: f32, position: f32, front_width: f32) -> f32 {
    let t = (position / front_width.max(1e-4)).clamp(0.0, 1.0);
    let s = t * t * (3.0 - 2.0 * t);
    warm_temp + (cold_temp - warm_temp) * s
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn warm_side() { assert_eq!(temperature(30.0, -10.0, 0.0, 100.0), 30.0); }
    #[test] fn cold_side() { assert_eq!(temperature(30.0, -10.0, 100.0, 100.0), -10.0); }
    #[test] fn mid_between() { let v = temperature(30.0, -10.0, 50.0, 100.0); assert!(v > -10.0 && v < 30.0); }
    #[test] fn monotone() { assert!(temperature(30.0, -10.0, 70.0, 100.0) < temperature(30.0, -10.0, 30.0, 100.0)); }
    #[test] fn bounded() { let mut p = 0.0f32; while p <= 100.0 { let v = temperature(30.0, -10.0, p, 100.0); assert!((-10.0..=30.0).contains(&v)); p += 5.0; } }
    #[test] fn deterministic() { assert_eq!(temperature(30.0, -10.0, 50.0, 100.0).to_bits(), temperature(30.0, -10.0, 50.0, 100.0).to_bits()); }
}
