//! Number of steps to climb a vertical `rise` at a given `step_height`, rounded up
//! so the top is reached. Pure, deterministic.

pub fn steps(rise: f32, step_height: f32) -> u32 {
    let r = rise.max(0.0);
    let h = step_height.max(1e-4);
    (r / h).ceil() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_rise_zero() { assert_eq!(steps(0.0, 0.2), 0); }
    #[test] fn exact_multiple() { assert_eq!(steps(2.0, 0.5), 4); }
    #[test] fn rounds_up_partial() { assert_eq!(steps(2.1, 0.5), 5); }
    #[test] fn taller_steps_fewer() { assert!(steps(2.0, 1.0) < steps(2.0, 0.5)); }
    #[test] fn monotone_in_rise() {
        let mut prev = 0u32;
        for r in [0.0f32, 1.0, 2.0, 5.0] { let s = steps(r, 0.5); assert!(s >= prev); prev = s; }
    }
    #[test] fn deterministic() { assert_eq!(steps(2.1, 0.5), steps(2.1, 0.5)); }
}
