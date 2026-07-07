//! Doppler pitch-shift factor from relative radial velocity (positive = source
//! approaching). Frequency multiplier (>1 approaching). Velocity clamped below
//! the speed of sound to avoid the singularity. Pure, deterministic.

pub fn pitch_factor(relative_velocity: f32, speed_of_sound: f32) -> f32 {
    let c = speed_of_sound.max(1.0);
    let v = relative_velocity.clamp(-(c * 0.9), c * 0.9);
    (c / (c - v)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn stationary_unity() { assert!((pitch_factor(0.0, 343.0) - 1.0).abs() < 1e-6); }
    #[test] fn approaching_higher() { assert!(pitch_factor(34.0, 343.0) > 1.0); }
    #[test] fn receding_lower() { assert!(pitch_factor(-34.0, 343.0) < 1.0); }
    #[test] fn finite_near_c() { assert!(pitch_factor(100000.0, 343.0).is_finite()); }
    #[test] fn monotone_in_velocity() {
        let (mut prev, mut v) = (f32::MIN, -300.0f32);
        while v < 300.0 { let p = pitch_factor(v, 343.0); assert!(p >= prev - 1e-6); prev = p; v += 10.0; }
    }
    #[test] fn deterministic() { assert_eq!(pitch_factor(34.0, 343.0).to_bits(), pitch_factor(34.0, 343.0).to_bits()); }
}
