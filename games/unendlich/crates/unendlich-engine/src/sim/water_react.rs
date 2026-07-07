//! `water_react` — two pure rules the water sim needs but that deserve their own tests:
//!   1. lava ↔ water contact (operator: "lava against water" tests) — molten rock meeting water
//!      solidifies and flashes steam, the classic voxel interaction;
//!   2. body ↔ water displacement (operator: "push back on the water and create waves with a mass
//!      function") — a moving body shoves water aside, and the wave it raises scales with its mass,
//!      speed, and how much cross-section it presents.
//! Both are host-agnostic; the bin maps its BlockIds and player/mob/falling-block AABBs onto these.

/// What a lava/water contact produces. Obsidian where a deep/source lava face meets water (hard,
/// permanent); Stone where thin/flowing lava meets water; plus a steam flag for the puff.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LavaWaterResult {
    /// the lava cell solidifies to this (true = obsidian, false = stone)
    pub obsidian: bool,
    /// the touching water cell is consumed (flashed to steam)
    pub water_consumed: bool,
    /// emit a steam puff at the contact
    pub steam: bool,
}

/// Resolve a face contact between a lava cell and a water cell. `lava_is_source` distinguishes a
/// deep/source lava body (→ obsidian, water survives) from thin flowing lava (→ stone, water flash-
/// boils). Returns None if this isn't a lava/water contact (caller passes the two booleans).
pub fn lava_water_contact(lava_present: bool, water_present: bool, lava_is_source: bool) -> Option<LavaWaterResult> {
    if !(lava_present && water_present) {
        return None;
    }
    Some(LavaWaterResult {
        obsidian: lava_is_source,
        water_consumed: !lava_is_source, // flowing lava boils the water away; a source doesn't
        steam: true,
    })
}

/// Volume of water (in cells) displaced by a body submerged by `submerged_fraction` in [0,1] with
/// bounding-box footprint `w`×`h` (the operator's 1×2 player box) and horizontal `depth_cells`
/// (how many column-cells the box spans). Archimedes: displaced volume = submerged volume.
pub fn displaced_volume(w: f32, h: f32, depth_cells: f32, submerged_fraction: f32) -> f32 {
    (w * h * depth_cells * submerged_fraction.clamp(0.0, 1.0)).max(0.0)
}

/// Wave amplitude (in cells) raised when a body of `mass` moving at `speed` shoves into water,
/// presenting cross-sectional `area`. Heavier + faster + blunter ⇒ bigger wave; `gain` tunes the
/// whole thing; `max_amp` caps it so a cannonball doesn't make a tsunami. A still body (speed 0)
/// makes no wave. This is the "mass function" the push-back uses.
pub fn wave_amplitude(mass: f32, speed: f32, area: f32, gain: f32, max_amp: f32) -> f32 {
    if mass <= 0.0 || speed <= 0.0 || area <= 0.0 {
        return 0.0;
    }
    // momentum per unit frontal area → height, softened by a sqrt so it grows sub-linearly
    let drive = (mass * speed / area) * gain;
    let amp = drive.sqrt();
    amp.clamp(0.0, max_amp.max(0.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_contact_when_not_both_present() {
        assert_eq!(lava_water_contact(true, false, true), None);
        assert_eq!(lava_water_contact(false, true, false), None);
    }

    #[test]
    fn source_lava_makes_obsidian_water_survives() {
        let r = lava_water_contact(true, true, true).unwrap();
        assert!(r.obsidian);
        assert!(!r.water_consumed);
        assert!(r.steam);
    }

    #[test]
    fn flowing_lava_makes_stone_and_boils_water() {
        let r = lava_water_contact(true, true, false).unwrap();
        assert!(!r.obsidian);
        assert!(r.water_consumed);
        assert!(r.steam);
    }

    #[test]
    fn displacement_follows_submersion() {
        // 1x2 box, spanning 1 column, fully submerged → 2 cells displaced
        assert_eq!(displaced_volume(1.0, 2.0, 1.0, 1.0), 2.0);
        // half submerged → half the volume
        assert_eq!(displaced_volume(1.0, 2.0, 1.0, 0.5), 1.0);
        // out of water → nothing
        assert_eq!(displaced_volume(1.0, 2.0, 1.0, 0.0), 0.0);
    }

    #[test]
    fn still_body_makes_no_wave() {
        assert_eq!(wave_amplitude(80.0, 0.0, 1.0, 1.0, 5.0), 0.0);
    }

    #[test]
    fn heavier_faster_makes_bigger_wave() {
        let light = wave_amplitude(40.0, 2.0, 1.0, 0.01, 100.0);
        let heavy = wave_amplitude(160.0, 2.0, 1.0, 0.01, 100.0);
        assert!(heavy > light);
        let slow = wave_amplitude(80.0, 1.0, 1.0, 0.01, 100.0);
        let fast = wave_amplitude(80.0, 4.0, 1.0, 0.01, 100.0);
        assert!(fast > slow);
    }

    #[test]
    fn wave_is_capped() {
        let amp = wave_amplitude(1e6, 1e3, 0.1, 1.0, 3.0);
        assert!(amp <= 3.0 + 1e-6);
    }
}
