//! Body-part mesh LOD: far ⇒ coarse (higher index), damaged/just-hit ⇒ fine
//! (lower index; always finest while under attack) (handoff/BODY_PART_MESHING.md).

/// Returns LOD 0..=max_lod, where 0 = finest. `damage` in [0,1].
pub fn part_lod(distance: f32, damage: f32, recently_hit: bool, max_lod: u8) -> u8 {
    if max_lod == 0 {
        return 0;
    }
    if recently_hit {
        return 0;
    }
    let dmg = damage.clamp(0.0, 1.0);
    let dist_level = (distance.max(0.0) / 32.0) as u32;
    let dmg_relief = (dmg * max_lod as f32) as u32;
    dist_level.saturating_sub(dmg_relief).min(max_lod as u32) as u8
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn far_is_coarser() { assert!(part_lod(200.0, 0.0, false, 4) > part_lod(10.0, 0.0, false, 4)); }
    #[test] fn damaged_is_finer() { assert!(part_lod(200.0, 0.9, false, 4) < part_lod(200.0, 0.0, false, 4)); }
    #[test] fn hit_forces_finest() { assert_eq!(part_lod(500.0, 0.0, true, 4), 0); }
    #[test] fn bounded_by_max() {
        let mut d = 0.0f32;
        while d < 10000.0 { assert!(part_lod(d, 0.0, false, 3) <= 3); d += 100.0; }
    }
    #[test] fn monotone_in_distance() {
        let (mut prev, mut d) = (0u8, 0.0f32);
        while d < 5000.0 { let l = part_lod(d, 0.2, false, 6); assert!(l >= prev); prev = l; d += 50.0; }
    }
    #[test] fn deterministic() { assert_eq!(part_lod(128.0, 0.3, false, 4), part_lod(128.0, 0.3, false, 4)); }
}
