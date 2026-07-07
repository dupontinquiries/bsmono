//! Centripetal g-load in a banked turn: a = v²/r, expressed in g (÷9.81). Faster
//! or tighter turns pull more g. Pure, deterministic.

pub fn g_force(speed: f32, turn_radius: f32) -> f32 {
    let v = speed.max(0.0);
    (v * v / turn_radius.max(1e-4)) / 9.81
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn still_zero() { assert_eq!(g_force(0.0, 50.0), 0.0); }
    #[test] fn faster_more() { assert!(g_force(40.0, 50.0) > g_force(20.0, 50.0)); }
    #[test] fn tighter_more() { assert!(g_force(30.0, 20.0) > g_force(30.0, 80.0)); }
    #[test] fn quadratic_in_speed() { assert!((g_force(40.0, 50.0) - 4.0 * g_force(20.0, 50.0)).abs() < 1e-4); }
    #[test] fn nonneg() { assert!(g_force(30.0, 50.0) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(g_force(30.0, 50.0).to_bits(), g_force(30.0, 50.0).to_bits()); }
}
