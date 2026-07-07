//! Encumbrance speed factor in [0.1,1]: full speed under capacity (= strength·10),
//! slowing as load exceeds it, floored at 0.1. Pure, deterministic.

pub fn speed_factor(items_weight: f32, strength: f32) -> f32 {
    let w = items_weight.max(0.0);
    let cap = (strength.max(0.0) * 10.0).max(1e-6);
    if w <= cap {
        1.0
    } else {
        (cap / w).clamp(0.1, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn under_capacity_full() { assert_eq!(speed_factor(50.0, 10.0), 1.0); }
    #[test] fn over_slows() { assert!(speed_factor(200.0, 10.0) < 1.0); }
    #[test] fn floored() { assert!(speed_factor(100000.0, 10.0) >= 0.1); }
    #[test] fn heavier_slower() { assert!(speed_factor(300.0, 10.0) < speed_factor(150.0, 10.0)); }
    #[test] fn stronger_less_penalty() { assert!(speed_factor(200.0, 20.0) > speed_factor(200.0, 10.0)); }
    #[test] fn deterministic() { assert_eq!(speed_factor(200.0, 10.0).to_bits(), speed_factor(200.0, 10.0).to_bits()); }
}
