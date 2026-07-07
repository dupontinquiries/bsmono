//! phys::acoustics — sound-propagation physics (r201). Models how sound
//! travels and decays in the world, complementing the existing perceptual
//! pieces (sim::sound_masking dB/occlusion, sim::sound_mix attenuation/pan,
//! world::reverb_wetness dry/wet amount). This module adds the PHYSICAL
//! quantities none of those compute:
//!   - propagation delay (speed of sound) — distant events are heard late;
//!   - distance + air absorption (high frequencies fade faster with range);
//!   - Sabine reverberation time (RT60) — how long a room's tail rings,
//!     from its volume and total surface absorption.
//!
//! Pure f32, deterministic, std-only — replay/rollback-safe and unit-
//! tested against the textbook formulas.

/// Speed of sound in air, blocks-per-second (1 block ≈ 1 m). ~343 m/s at
/// 20°C. Used for propagation delay; the engine treats 1 block = 1 m.
pub const SPEED_OF_SOUND: f32 = 343.0;

/// Time in seconds for sound to travel `distance` blocks. A thunderclap
/// 343 blocks away is heard ~1 s after the flash.
#[inline]
pub fn propagation_delay(distance: f32) -> f32 {
    distance.max(0.0) / SPEED_OF_SOUND
}

/// Inverse-distance (spherical spreading) gain, normalized to 1.0 at
/// `ref_dist`. Energy spreads over a sphere, so pressure ~ 1/r. Clamped
/// near zero distance so it stays finite. This is the geometric term;
/// air absorption below multiplies on top.
#[inline]
pub fn spreading_gain(distance: f32, ref_dist: f32) -> f32 {
    let d = distance.max(ref_dist.max(1e-3));
    ref_dist.max(1e-3) / d
}

/// Frequency-dependent air absorption gain over `distance` blocks.
/// `absorption_per_block` is the linear attenuation coefficient (1/blocks)
/// for the band of interest — higher for treble, near zero for bass —
/// giving exp(-a*d). This is why distant sound is muffled (the highs are
/// gone), not merely quieter.
#[inline]
pub fn air_absorption_gain(distance: f32, absorption_per_block: f32) -> f32 {
    (-absorption_per_block.max(0.0) * distance.max(0.0)).exp()
}

/// Combined linear gain at `distance`: spherical spreading × air
/// absorption. Multiply a source's reference amplitude by this.
#[inline]
pub fn distance_gain(distance: f32, ref_dist: f32, absorption_per_block: f32) -> f32 {
    spreading_gain(distance, ref_dist) * air_absorption_gain(distance, absorption_per_block)
}

/// Sabine reverberation time (RT60) in seconds: how long for the tail to
/// drop 60 dB. RT60 = 0.161 * V / A, where V is room volume (blocks³) and
/// A is the total absorption (Σ surface_area × absorption_coeff, in
/// "sabins"). Bigger rooms ring longer; more absorptive surfaces ring
/// shorter. Returns 0 for a fully-open/zero-volume space.
pub fn sabine_rt60(volume: f32, total_absorption: f32) -> f32 {
    if volume <= 0.0 || total_absorption <= 1e-6 {
        return 0.0;
    }
    0.161 * volume / total_absorption
}

/// Convenience: RT60 for a rectangular room of given interior dimensions
/// (blocks) whose six faces share one average absorption coefficient
/// `avg_absorption` in 0..1 (0 = perfect reflector → infinite tail;
/// 1 = anechoic → no tail). Clamps the coefficient off the singular ends.
pub fn box_room_rt60(dx: f32, dy: f32, dz: f32, avg_absorption: f32) -> f32 {
    let v = dx.max(0.0) * dy.max(0.0) * dz.max(0.0);
    let surface = 2.0 * (dx * dy + dy * dz + dx * dz);
    let a = avg_absorption.clamp(0.01, 1.0);
    sabine_rt60(v, surface * a)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delay_matches_speed_of_sound() {
        // 343 blocks -> ~1 second.
        assert!((propagation_delay(343.0) - 1.0).abs() < 1e-3);
        assert_eq!(propagation_delay(0.0), 0.0);
    }

    #[test]
    fn spreading_is_inverse_distance() {
        // double the distance -> half the pressure gain.
        let near = spreading_gain(1.0, 1.0);
        let far = spreading_gain(2.0, 1.0);
        assert!((near - 1.0).abs() < 1e-5);
        assert!((far - 0.5).abs() < 1e-5);
    }

    #[test]
    fn air_absorption_muffles_with_range() {
        // a treble-ish coefficient drops far gain a lot; zero coeff = no loss.
        assert!(air_absorption_gain(100.0, 0.02) < 0.2);
        assert_eq!(air_absorption_gain(100.0, 0.0), 1.0);
        // monotonic decay
        assert!(air_absorption_gain(10.0, 0.05) > air_absorption_gain(20.0, 0.05));
    }

    #[test]
    fn sabine_bigger_room_rings_longer() {
        // same absorption coefficient, bigger box -> longer RT60.
        let small = box_room_rt60(4.0, 3.0, 4.0, 0.2);
        let big = box_room_rt60(20.0, 8.0, 20.0, 0.2);
        assert!(big > small, "big {big} should exceed small {small}");
        // more absorptive -> shorter tail.
        let reflective = box_room_rt60(10.0, 5.0, 10.0, 0.05);
        let dead = box_room_rt60(10.0, 5.0, 10.0, 0.9);
        assert!(reflective > dead);
    }

    #[test]
    fn open_space_has_no_tail() {
        assert_eq!(sabine_rt60(0.0, 5.0), 0.0);
        assert_eq!(sabine_rt60(100.0, 0.0), 0.0);
    }

    #[test]
    fn deterministic() {
        assert_eq!(distance_gain(17.0, 1.0, 0.01).to_bits(),
                   distance_gain(17.0, 1.0, 0.01).to_bits());
    }
}
