//! `sim::forge_lod` — pick a forge-dragon voxel resolution by viewer distance.
//!
//! Overspawn can put many dragons on screen; far ones don't need 256³ detail.
//! [`forge_res_for_distance`] is the LOD ladder; [`next_res`] adds hysteresis so a
//! dragon hovering on a tier boundary doesn't rebuild every frame. The bin keeps
//! each dragon's current res, calls `next_res` as the camera moves, and on a
//! change rebuilds via `spawn::spawn_dragon_with_genome_res` and swaps the
//! `Arc<ForgeModel>`. Pure, deterministic, panic-free.

/// LOD ladder: full detail up close, halving by distance tier, floored at 8.
/// Non-increasing in `dist`.
pub fn forge_res_for_distance(dist: f32, base_res: u32) -> u32 {
    let b = base_res.max(8);
    let r = if dist < 30.0 {
        b
    } else if dist < 60.0 {
        b / 2
    } else if dist < 120.0 {
        b / 4
    } else if dist < 240.0 {
        b / 8
    } else {
        b / 16
    };
    r.max(8)
}

/// Hysteretic update: keep `current_res` unless the distance is *clearly* in a
/// different tier (±10% margin), so a dragon sitting on a boundary stays put.
/// Returns the resolution to use this frame.
pub fn next_res(current_res: u32, dist: f32, base_res: u32) -> u32 {
    let d = dist.max(0.0);
    let want_more = forge_res_for_distance(d * 1.1, base_res); // solidly closer
    let want_less = forge_res_for_distance(d * 0.9, base_res); // solidly farther
    if want_more > current_res {
        want_more // moved in: add detail
    } else if want_less < current_res {
        want_less // moved out: shed detail
    } else {
        current_res // in the dead-band: no rebuild
    }
}

/// Whether a rebuild is needed (the bin only rebuilds when this is true).
pub fn should_rebuild(current_res: u32, desired_res: u32) -> bool {
    current_res != desired_res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ladder_non_increasing() {
        let mut prev = u32::MAX;
        for d in [0.0, 10.0, 29.0, 31.0, 59.0, 61.0, 119.0, 121.0, 239.0, 241.0, 1000.0] {
            let r = forge_res_for_distance(d, 256);
            assert!(r <= prev, "resolution must not rise with distance (d={d})");
            assert!(r >= 8);
            prev = r;
        }
        assert_eq!(forge_res_for_distance(5.0, 256), 256, "full detail up close");
        assert_eq!(forge_res_for_distance(1e6, 256), 16, "far floor");
    }

    #[test]
    fn lower_base_still_floors_at_8() {
        assert_eq!(forge_res_for_distance(1e6, 64), 8);
        assert!(forge_res_for_distance(0.0, 64) <= 64);
    }

    #[test]
    fn hysteresis_holds_on_boundary() {
        // exactly on the 60.0 boundary, starting at the near tier (128)
        let base = 256;
        let near_tier = forge_res_for_distance(45.0, base); // 128
        let held = next_res(near_tier, 60.0, base);
        assert_eq!(held, near_tier, "sitting on the boundary keeps the current res");
    }

    #[test]
    fn moving_in_and_out_switches() {
        let base = 256;
        // start far (low res), move clearly close -> upgrades
        let far = forge_res_for_distance(300.0, base); // 16
        let closer = next_res(far, 10.0, base);
        assert!(closer > far, "moving in adds detail");
        // start close (high res), move clearly far -> downgrades
        let close = forge_res_for_distance(10.0, base); // 256
        let farther = next_res(close, 300.0, base);
        assert!(farther < close, "moving out sheds detail");
    }

    #[test]
    fn should_rebuild_only_on_change() {
        assert!(!should_rebuild(128, 128));
        assert!(should_rebuild(256, 128));
    }
}
