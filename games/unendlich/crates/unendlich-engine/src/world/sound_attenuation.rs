//! Sound volume attenuation by distance: full within the reference distance, then
//! inverse rolloff beyond it. In [0,1]. Pure, deterministic.

pub fn attenuation(distance: f32, reference: f32, rolloff: f32) -> f32 {
    let d = distance.max(0.0);
    let r = reference.max(1e-6);
    let k = rolloff.max(0.0);
    if d <= r {
        1.0
    } else {
        (r / (r + k * (d - r))).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn within_reference_full() { assert_eq!(attenuation(5.0, 10.0, 1.0), 1.0); }
    #[test] fn far_quieter() { assert!(attenuation(100.0, 10.0, 1.0) < attenuation(20.0, 10.0, 1.0)); }
    #[test] fn more_rolloff_quieter() { assert!(attenuation(50.0, 10.0, 2.0) < attenuation(50.0, 10.0, 0.5)); }
    #[test] fn bounded() {
        let mut d = 0.0f32;
        while d < 200.0 { assert!((0.0..=1.0).contains(&attenuation(d, 10.0, 1.0))); d += 5.0; }
    }
    #[test] fn monotone_decreasing() {
        let (mut prev, mut d) = (f32::MAX, 10.0f32);
        while d < 200.0 { let a = attenuation(d, 10.0, 1.0); assert!(a <= prev + 1e-6); prev = a; d += 5.0; }
    }
    #[test] fn deterministic() { assert_eq!(attenuation(50.0, 10.0, 1.0).to_bits(), attenuation(50.0, 10.0, 1.0).to_bits()); }
}
