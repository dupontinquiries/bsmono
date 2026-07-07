//! Creature spawn density from biome suitability and a day/night activity factor
//! (nocturnal creatures peak at night). In [0,1]. Pure, deterministic.

pub fn density(suitability: f32, nocturnality: f32, is_night: bool) -> f32 {
    let s = suitability.clamp(0.0, 1.0);
    let noc = nocturnality.clamp(0.0, 1.0);
    let activity = if is_night { noc } else { 1.0 - noc };
    (s * (0.3 + 0.7 * activity)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_suitability_zero() { assert_eq!(density(0.0, 0.5, true), 0.0); }
    #[test] fn nocturnal_more_at_night() { assert!(density(1.0, 1.0, true) > density(1.0, 1.0, false)); }
    #[test] fn diurnal_more_by_day() { assert!(density(1.0, 0.0, false) > density(1.0, 0.0, true)); }
    #[test] fn baseline_activity() { assert!(density(1.0, 0.0, true) >= 0.3 - 1e-6); }
    #[test] fn bounded() { for &nn in &[true, false] { for i in 0..11 { let v = density(0.9, i as f32 / 10.0, nn); assert!((0.0..=1.0).contains(&v)); } } }
    #[test] fn deterministic() { assert_eq!(density(0.9, 0.5, true).to_bits(), density(0.9, 0.5, true).to_bits()); }
}
