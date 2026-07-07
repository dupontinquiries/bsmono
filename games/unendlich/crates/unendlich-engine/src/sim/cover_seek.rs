//! How strongly to move toward terrain cover to shield the weak side
//! (handoff/COMBAT_STANCE.md open list). More exposure + nearer cover ⇒ stronger
//! pull, in [0,1]. Cover past `max_range` is useless.

pub fn seek_strength(weak_exposure: f32, cover_distance: f32, max_range: f32) -> f32 {
    let e = weak_exposure.clamp(0.0, 1.0);
    let mr = max_range.max(1e-6);
    let d = cover_distance.max(0.0);
    if d >= mr {
        return 0.0;
    }
    (e * (1.0 - d / mr)).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn exposed_and_near_is_strong() { assert!(seek_strength(1.0, 1.0, 20.0) > 0.9); }
    #[test] fn beyond_range_zero() { assert_eq!(seek_strength(1.0, 25.0, 20.0), 0.0); }
    #[test] fn no_exposure_zero() { assert_eq!(seek_strength(0.0, 1.0, 20.0), 0.0); }
    #[test] fn nearer_is_stronger() { assert!(seek_strength(0.8, 2.0, 20.0) > seek_strength(0.8, 15.0, 20.0)); }
    #[test] fn bounded() {
        let mut d = 0.0f32;
        while d < 25.0 { let s = seek_strength(0.7, d, 20.0); assert!((0.0..=1.0).contains(&s)); d += 1.0; }
    }
    #[test] fn deterministic() { assert_eq!(seek_strength(0.6, 5.0, 20.0).to_bits(), seek_strength(0.6, 5.0, 20.0).to_bits()); }
}
