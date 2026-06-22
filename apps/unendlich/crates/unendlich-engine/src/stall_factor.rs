//! Lift-retention factor [0,1] vs angle of attack: ~1 up to the stall angle, then
//! collapsing linearly to 0 within one further stall_angle. Pure, deterministic.

pub fn stall_factor(angle_of_attack: f32, stall_angle: f32) -> f32 {
    let aoa = angle_of_attack.abs();
    let sa = stall_angle.max(1e-4);
    if aoa <= sa {
        1.0
    } else {
        (1.0 - (aoa - sa) / sa).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn below_stall_full() { assert_eq!(stall_factor(0.2, 0.5), 1.0); }
    #[test] fn at_stall_full() { assert_eq!(stall_factor(0.5, 0.5), 1.0); }
    #[test] fn past_stall_reduced() { assert!(stall_factor(0.7, 0.5) < 1.0); }
    #[test] fn deep_stall_zero() { assert_eq!(stall_factor(1.5, 0.5), 0.0); }
    #[test] fn symmetric() { assert_eq!(stall_factor(-0.7, 0.5), stall_factor(0.7, 0.5)); }
    #[test] fn deterministic() { assert_eq!(stall_factor(0.7, 0.5).to_bits(), stall_factor(0.7, 0.5).to_bits()); }
}
