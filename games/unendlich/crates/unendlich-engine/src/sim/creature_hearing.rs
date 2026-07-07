//! `sim::creature_hearing` — whether a creature notices a sound.
//!
//! The perception counterpart to the output-side `sound_resolve`/`voice_mix`.
//! A creature detects a sound when it rises above the ambient floor within its
//! hearing range — but walls between dampen it first. This composes the orphaned
//! [`audio_occlusion`](crate::sim::audio_occlusion) (walls) with the orphaned
//! [`hearing_radius`](crate::sim::hearing_radius) (distance + ambient + range).
//!
//! Distance is handled exactly once — by `hearing_radius` — while occlusion
//! lowers the effective loudness, so the two cores compose without double-counting.
//! Pure and deterministic.

use crate::sim::audio_occlusion;
use crate::sim::hearing_radius;

/// What a creature makes of a sound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Heard {
    /// Detection strength `[0,1]` (0 = unnoticed).
    pub detection: f32,
    /// Whether it cleared the notice threshold.
    pub noticed: bool,
}

/// Resolve how a creature with `hearing_range` (and a `notice_threshold`) hears a
/// sound of `source_loudness` at `distance`, through `solid_blocks_between` walls
/// (each `per_block_db`), against an `ambient` floor. Walls cut the effective
/// loudness (`audio_occlusion`); the result is detected by `hearing_radius`. Pure.
#[allow(clippy::too_many_arguments)]
pub fn hear(
    source_loudness: f32,
    distance: f32,
    solid_blocks_between: u32,
    per_block_db: f32,
    ambient: f32,
    hearing_range: f32,
    notice_threshold: f32,
) -> Heard {
    let occ = audio_occlusion::occlusion_gain(solid_blocks_between, per_block_db);
    let effective = source_loudness.max(0.0) * occ;
    let detection = hearing_radius::detect(effective, distance, ambient, hearing_range);
    Heard {
        detection,
        noticed: detection >= notice_threshold.max(0.0) && detection > 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // loud=1.0, ambient 0.1, range 50, per-block 3 dB, threshold 0.2
    fn hear_at(dist: f32, walls: u32) -> Heard {
        hear(1.0, dist, walls, 3.0, 0.1, 50.0, 0.2)
    }

    #[test]
    fn loud_and_close_is_noticed() {
        let h = hear_at(2.0, 0);
        assert!(h.noticed && h.detection > 0.8);
    }

    #[test]
    fn walls_reduce_detection() {
        let open = hear_at(10.0, 0);
        let walled = hear_at(10.0, 4);
        assert!(walled.detection < open.detection, "walls muffle what the creature hears");
    }

    #[test]
    fn beyond_range_is_unheard() {
        let h = hear_at(60.0, 0);
        assert_eq!(h.detection, 0.0);
        assert!(!h.noticed);
    }

    #[test]
    fn below_ambient_is_unheard() {
        // very quiet source under the ambient floor
        let h = hear(0.05, 3.0, 0, 3.0, 0.2, 50.0, 0.2);
        assert_eq!(h.detection, 0.0);
        assert!(!h.noticed);
    }

    #[test]
    fn closer_and_louder_detect_more() {
        assert!(hear_at(3.0, 0).detection > hear_at(40.0, 0).detection, "closer ⇒ stronger");
        let loud = hear(1.0, 5.0, 0, 3.0, 0.1, 50.0, 0.2).detection;
        let soft = hear(0.5, 5.0, 0, 3.0, 0.1, 50.0, 0.2).detection;
        assert!(loud > soft, "louder ⇒ stronger");
    }

    #[test]
    fn threshold_gates_notice() {
        // a faint-but-nonzero detection is heard but not "noticed" under a high bar
        let faint = hear(0.4, 45.0, 0, 3.0, 0.1, 50.0, 0.9);
        assert!(faint.detection > 0.0 && !faint.noticed, "audible but below the notice bar");
    }

    #[test]
    fn deterministic() {
        assert_eq!(hear_at(7.0, 2), hear_at(7.0, 2));
    }
}
