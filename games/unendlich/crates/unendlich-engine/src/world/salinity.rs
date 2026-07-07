//! Water salinity from freshwater inflow (dilutes) and evaporation (concentrates),
//! about an ocean base of 0.5. In [0,1] (1 = brine). Pure, deterministic.

pub fn salinity(freshwater_in: f32, evaporation: f32) -> f32 {
    let fw = freshwater_in.max(0.0);
    let ev = evaporation.max(0.0);
    (0.5 - 0.3 * fw + 0.3 * ev).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn more_fresh_lower() { assert!(salinity(1.0, 0.0) < salinity(0.0, 0.0)); }
    #[test] fn more_evap_higher() { assert!(salinity(0.0, 1.0) > salinity(0.0, 0.0)); }
    #[test] fn balanced_is_base() { assert!((salinity(0.0, 0.0) - 0.5).abs() < 1e-6); }
    #[test] fn bounded() {
        for &(fw, ev) in &[(0.0f32, 0.0f32), (5.0, 0.0), (0.0, 5.0), (1.0, 1.0)] { let v = salinity(fw, ev); assert!((0.0..=1.0).contains(&v)); }
    }
    #[test] fn monotone_in_fresh() { assert!(salinity(0.5, 0.2) < salinity(0.1, 0.2)); }
    #[test] fn deterministic() { assert_eq!(salinity(0.3, 0.2).to_bits(), salinity(0.3, 0.2).to_bits()); }
}
