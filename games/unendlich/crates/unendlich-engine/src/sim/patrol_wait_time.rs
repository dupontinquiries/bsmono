//! Pause time at a patrol waypoint: alert mobs move on quickly, relaxed mobs
//! linger. Returns seconds in [min_wait, max_wait]. Pure, deterministic.

pub fn wait_time(alertness: f32, min_wait: f32, max_wait: f32) -> f32 {
    let a = alertness.clamp(0.0, 1.0);
    let mn = min_wait.max(0.0);
    let mx = max_wait.max(mn);
    mx - (mx - mn) * a
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn relaxed_max() { assert_eq!(wait_time(0.0, 1.0, 5.0), 5.0); }
    #[test] fn alert_min() { assert_eq!(wait_time(1.0, 1.0, 5.0), 1.0); }
    #[test] fn between() { let w = wait_time(0.5, 1.0, 5.0); assert!(w > 1.0 && w < 5.0); }
    #[test] fn bounded() { let mut a = 0.0f32; while a <= 1.0 { let w = wait_time(a, 1.0, 5.0); assert!((1.0..=5.0).contains(&w)); a += 0.1; } }
    #[test] fn monotone_decreasing() { assert!(wait_time(0.8, 1.0, 5.0) < wait_time(0.2, 1.0, 5.0)); }
    #[test] fn deterministic() { assert_eq!(wait_time(0.5, 1.0, 5.0).to_bits(), wait_time(0.5, 1.0, 5.0).to_bits()); }
}
