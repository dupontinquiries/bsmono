//! Should a body part stay voxel-fine? (handoff/BODY_PART_MESHING.md)
//! Fine when its HP fraction is below threshold OR it was hit within `cooldown`
//! ticks (so it doesn't pop back to coarse mid-fight). Pure, deterministic.

pub fn needs_fine(limb_hp: f32, max_hp: f32, ticks_since_hit: u32, cooldown: u32, threshold_frac: f32) -> bool {
    if ticks_since_hit < cooldown {
        return true;
    }
    let max = max_hp.max(1e-6);
    let frac = (limb_hp / max).clamp(0.0, 1.0);
    frac < threshold_frac.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn low_hp_is_fine() { assert!(needs_fine(10.0, 100.0, 999, 30, 0.3)); }
    #[test] fn recent_hit_is_fine_even_if_healthy() { assert!(needs_fine(100.0, 100.0, 5, 30, 0.3)); }
    #[test] fn healthy_and_old_is_coarse() { assert!(!needs_fine(100.0, 100.0, 999, 30, 0.3)); }
    #[test] fn cooldown_boundary() { assert!(!needs_fine(100.0, 100.0, 30, 30, 0.3)); }
    #[test] fn threshold_respected() {
        assert!(needs_fine(40.0, 100.0, 999, 30, 0.5));
        assert!(!needs_fine(60.0, 100.0, 999, 30, 0.5));
    }
    #[test] fn deterministic() { assert_eq!(needs_fine(50.0, 100.0, 10, 30, 0.4), needs_fine(50.0, 100.0, 10, 30, 0.4)); }
}
