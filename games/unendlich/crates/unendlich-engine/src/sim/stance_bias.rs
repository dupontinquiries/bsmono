//! Aggressive↔defensive lean from overall health (handoff/COMBAT_STANCE.md).
//! +1 = aggressive (full HP, close & square), -1 = defensive (hurt, distant & side-on).

pub fn bias(overall_hp: f32, max_hp: f32) -> f32 {
    let max = max_hp.max(1e-6);
    let frac = (overall_hp / max).clamp(0.0, 1.0);
    (2.0 * frac - 1.0).clamp(-1.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn full_hp_aggressive() { assert!((bias(100.0, 100.0) - 1.0).abs() < 1e-6); }
    #[test] fn no_hp_defensive() { assert!((bias(0.0, 100.0) + 1.0).abs() < 1e-6); }
    #[test] fn half_hp_neutral() { assert!(bias(50.0, 100.0).abs() < 1e-6); }
    #[test] fn bounded() {
        let mut h = -50.0f32;
        while h < 150.0 { let b = bias(h, 100.0); assert!((-1.0..=1.0).contains(&b)); h += 7.0; }
    }
    #[test] fn monotone() {
        let (mut prev, mut h) = (f32::MIN, 0.0f32);
        while h <= 100.0 { let b = bias(h, 100.0); assert!(b >= prev); prev = b; h += 5.0; }
    }
    #[test] fn deterministic() { assert_eq!(bias(70.0, 100.0).to_bits(), bias(70.0, 100.0).to_bits()); }
}
