//! 3D spatial music mixer. The track-separated stems (music / bass / drums) sit at
//! points in space; the listener's position fades between them, so the menu
//! fly-through (or the player in-world) naturally crossfades the mix by *where* it
//! is. This module is the PURE gain math — opus decode + rodio playback is the
//! compile-on-end shell (see handoff/AUDIO_SPATIAL_MIXER.md), gated behind an
//! `audio` feature so the default build needs no audio system libs.

/// One stem placed in space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StemSource {
    pub name: &'static str,
    pub pos: [f32; 3],
    /// Mix balance baked in (e.g. drums sit slightly under the theme).
    pub base_gain: f32,
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    dx * dx + dy * dy + dz * dz
}

/// Smooth distance gain: 1.0 at the source, falling toward 0 with distance.
/// `falloff` is the distance at which the gain is ≈ 0.5 (inverse-square-ish).
pub fn distance_gain(listener: [f32; 3], src: [f32; 3], falloff: f32) -> f32 {
    let f = falloff.max(1e-3);
    1.0 / (1.0 + dist2(listener, src) / (f * f))
}

/// Per-stem playback gains for a listener position. Walking toward the "drums" point
/// brings the drums up and the others down — a spatial crossfade, not a slider.
pub fn mix_gains(listener: [f32; 3], stems: &[StemSource], falloff: f32) -> Vec<f32> {
    stems
        .iter()
        .map(|s| (s.base_gain * distance_gain(listener, s.pos, falloff)).clamp(0.0, 1.0))
        .collect()
}

/// Default menu layout: the three stems around the title so the title→section
/// fly-through crossfades them as it travels.
pub fn menu_stem_layout() -> [StemSource; 3] {
    [
        StemSource { name: "music", pos: [0.0, 0.0, 0.0], base_gain: 1.00 }, // theme, centre
        StemSource { name: "bass", pos: [-8.0, 0.0, 4.0], base_gain: 0.90 }, // left
        StemSource { name: "drums", pos: [8.0, 0.0, 4.0], base_gain: 0.85 }, // right
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gain_is_one_at_source_and_half_at_falloff() {
        let g0 = distance_gain([0.0, 0.0, 0.0], [0.0, 0.0, 0.0], 5.0);
        assert!((g0 - 1.0).abs() < 1e-6, "full gain at the source");
        let ghalf = distance_gain([5.0, 0.0, 0.0], [0.0, 0.0, 0.0], 5.0);
        assert!((ghalf - 0.5).abs() < 1e-6, "≈half gain at the falloff distance");
        let gfar = distance_gain([100.0, 0.0, 0.0], [0.0, 0.0, 0.0], 5.0);
        assert!(gfar < 0.01, "near-silent far away");
    }

    #[test]
    fn listener_position_crossfades_the_stems() {
        let stems = menu_stem_layout(); // music@0, bass@-8, drums@+8
        let falloff = 6.0;
        // stand on the drums point → drums is the loudest stem
        let near_drums = mix_gains([8.0, 0.0, 4.0], &stems, falloff);
        let drums_i = 2;
        let bass_i = 1;
        assert!(
            near_drums[drums_i] > near_drums[bass_i],
            "by the drums, drums beat bass ({} vs {})",
            near_drums[drums_i],
            near_drums[bass_i]
        );
        // move to the bass point → the ordering flips
        let near_bass = mix_gains([-8.0, 0.0, 4.0], &stems, falloff);
        assert!(near_bass[bass_i] > near_bass[drums_i], "by the bass, bass beats drums");
        // centre favours the theme
        let centre = mix_gains([0.0, 0.0, 0.0], &stems, falloff);
        assert!(centre[0] >= centre[1] && centre[0] >= centre[2], "centre favours the theme");
    }
}
