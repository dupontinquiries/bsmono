//! Verlet integration for one axis: new = pos + (pos − prev) + accel·dt². The
//! implicit velocity is (pos − prev). Pure, deterministic.

pub fn verlet_step(pos: f32, prev_pos: f32, accel: f32, dt: f32) -> f32 {
    let t = dt.max(0.0);
    pos + (pos - prev_pos) + accel * t * t
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn at_rest_stays() { assert_eq!(verlet_step(5.0, 5.0, 0.0, 0.1), 5.0); }
    #[test] fn momentum_continues() { assert!((verlet_step(6.0, 5.0, 0.0, 0.1) - 7.0).abs() < 1e-6); }
    #[test] fn accel_moves() { assert!(verlet_step(5.0, 5.0, 10.0, 0.1) > 5.0); }
    #[test] fn dt_quadratic() {
        let e2 = verlet_step(5.0, 5.0, 10.0, 0.2) - 5.0;
        let e1 = verlet_step(5.0, 5.0, 10.0, 0.1) - 5.0;
        assert!((e2 - 4.0 * e1).abs() < 1e-5);
    }
    #[test] fn direction_sign() { assert!(verlet_step(5.0, 6.0, 0.0, 0.1) < 5.0); }
    #[test] fn deterministic() { assert_eq!(verlet_step(5.0, 5.0, 10.0, 0.1).to_bits(), verlet_step(5.0, 5.0, 10.0, 0.1).to_bits()); }
}
