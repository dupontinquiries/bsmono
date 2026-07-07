//! Scree/talus depth accumulating at a slope's base: material above the angle of
//! repose slides down. More over-steepness + supply ⇒ more talus, capped at supply.

pub fn scree(slope: f32, repose: f32, supply: f32) -> f32 {
    let over = (slope - repose).max(0.0);
    let s = supply.max(0.0);
    (over * s).min(s)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn below_repose_no_scree() { assert_eq!(scree(0.3, 0.6, 5.0), 0.0); }
    #[test] fn steeper_more() { assert!(scree(1.0, 0.6, 5.0) > scree(0.7, 0.6, 5.0)); }
    #[test] fn more_supply_more() { assert!(scree(1.0, 0.6, 10.0) >= scree(1.0, 0.6, 5.0)); }
    #[test] fn capped_at_supply() { assert_eq!(scree(100.0, 0.0, 5.0), 5.0); }
    #[test] fn bounded() {
        let mut sl = 0.0f32;
        while sl < 3.0 { let v = scree(sl, 0.6, 5.0); assert!((0.0..=5.0).contains(&v)); sl += 0.1; }
    }
    #[test] fn deterministic() { assert_eq!(scree(1.0, 0.6, 5.0).to_bits(), scree(1.0, 0.6, 5.0).to_bits()); }
}
