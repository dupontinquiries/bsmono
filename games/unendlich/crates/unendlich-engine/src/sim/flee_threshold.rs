//! Whether a mob flees: true when health falls below a courage-adjusted threshold
//! (0.3 for a coward, 0 for the fearless). Pure, deterministic.

pub fn should_flee(health_frac: f32, courage: f32) -> bool {
    let hp = health_frac.clamp(0.0, 1.0);
    let c = courage.clamp(0.0, 1.0);
    hp < 0.3 * (1.0 - c)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn healthy_no_flee() { assert!(!should_flee(1.0, 0.5)); }
    #[test] fn low_coward_flees() { assert!(should_flee(0.1, 0.0)); }
    #[test] fn fearless_never() { assert!(!should_flee(0.01, 1.0)); }
    #[test] fn braver_tolerates_more() { assert!(should_flee(0.2, 0.0) && !should_flee(0.2, 0.5)); }
    #[test] fn zero_hp_flees_unless_fearless() { assert!(should_flee(0.0, 0.5)); }
    #[test] fn deterministic() { assert_eq!(should_flee(0.1, 0.0), should_flee(0.1, 0.0)); }
}
