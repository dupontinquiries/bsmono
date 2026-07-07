//! Terminal falling velocity where gravity balances drag: sqrt(2·m·g/(ρ·A)) with
//! ρ≈1.225 folded in. Heavier ⇒ faster terminal; more drag area ⇒ slower. Pure.

pub fn terminal(mass: f32, drag_area: f32, gravity: f32) -> f32 {
    let m = mass.max(0.0);
    let a = drag_area.max(1e-4);
    let g = gravity.max(0.0);
    (2.0 * m * g / (1.225 * a)).max(0.0).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn heavier_faster() { assert!(terminal(100.0, 1.0, 9.8) > terminal(50.0, 1.0, 9.8)); }
    #[test] fn more_area_slower() { assert!(terminal(100.0, 5.0, 9.8) < terminal(100.0, 1.0, 9.8)); }
    #[test] fn zero_gravity_zero() { assert_eq!(terminal(100.0, 1.0, 0.0), 0.0); }
    #[test] fn zero_mass_zero() { assert_eq!(terminal(0.0, 1.0, 9.8), 0.0); }
    #[test] fn monotone_in_mass() {
        let mut prev = f32::MIN;
        for m in [0.0f32, 10.0, 50.0, 100.0] { let v = terminal(m, 1.0, 9.8); assert!(v >= prev); prev = v; }
    }
    #[test] fn deterministic() { assert_eq!(terminal(100.0, 1.0, 9.8).to_bits(), terminal(100.0, 1.0, 9.8).to_bits()); }
}
