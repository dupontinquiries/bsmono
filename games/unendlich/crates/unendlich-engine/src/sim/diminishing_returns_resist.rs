//! CC effectiveness under diminishing returns: each successive application of the
//! same crowd-control is less effective. effectiveness = dr_factor^applications,
//! in (0,1] (e.g. dr 0.5 → 100%, 50%, 25%…). Pure, deterministic.

pub fn effectiveness(applications: u32, dr_factor: f32) -> f32 {
    dr_factor.clamp(0.0, 1.0).powi(applications as i32)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn first_full() { assert_eq!(effectiveness(0, 0.5), 1.0); }
    #[test] fn second_reduced() { assert_eq!(effectiveness(1, 0.5), 0.5); }
    #[test] fn third_less() { assert_eq!(effectiveness(2, 0.5), 0.25); }
    #[test] fn no_dr_stays_full() { assert_eq!(effectiveness(5, 1.0), 1.0); }
    #[test] fn bounded() { for a in 0..10u32 { let v = effectiveness(a, 0.6); assert!(v > 0.0 && v <= 1.0); } }
    #[test] fn deterministic() { assert_eq!(effectiveness(3, 0.5).to_bits(), effectiveness(3, 0.5).to_bits()); }
}
