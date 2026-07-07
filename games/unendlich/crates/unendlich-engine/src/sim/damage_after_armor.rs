//! Damage after armor mitigation: raw·k/(armor+k). More armor ⇒ more reduction
//! with diminishing returns; never negative, never exceeds raw. Pure.

pub fn mitigated(raw: f32, armor: f32, k: f32) -> f32 {
    let r = raw.max(0.0);
    let a = armor.max(0.0);
    let kk = k.max(1e-4);
    r * kk / (a + kk)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_armor_full() { assert_eq!(mitigated(100.0, 0.0, 50.0), 100.0); }
    #[test] fn more_armor_less() { assert!(mitigated(100.0, 100.0, 50.0) < mitigated(100.0, 20.0, 50.0)); }
    #[test] fn nonneg() { assert!(mitigated(100.0, 50.0, 50.0) >= 0.0); }
    #[test] fn bounded_by_raw() { assert!(mitigated(100.0, 50.0, 50.0) <= 100.0); }
    #[test] fn diminishing() {
        let early = mitigated(100.0, 0.0, 50.0) - mitigated(100.0, 50.0, 50.0);
        let late = mitigated(100.0, 50.0, 50.0) - mitigated(100.0, 100.0, 50.0);
        assert!(early > late);
    }
    #[test] fn deterministic() { assert_eq!(mitigated(100.0, 50.0, 50.0).to_bits(), mitigated(100.0, 50.0, 50.0).to_bits()); }
}
