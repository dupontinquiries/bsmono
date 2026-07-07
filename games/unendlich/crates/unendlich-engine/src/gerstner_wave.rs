//! Single Gerstner wave contribution at `phase` (radians): returns
//! (horizontal_offset, vertical_height). Steepness in [0,1] sets the crest pinch.

pub fn wave(phase: f32, steepness: f32, amplitude: f32) -> (f32, f32) {
    let q = steepness.clamp(0.0, 1.0);
    let a = amplitude.max(0.0);
    (q * a * phase.cos(), a * phase.sin())
}

// ---- r335-water-visual: multi-octave directional Gerstner swell ------------
// Builds a richer swell by summing several directional Gerstner octaves, each
// evaluated through the single-wave [`wave`] above (so per-octave behaviour is
// identical to the reference). Pure, deterministic, NaN-safe.

use std::f32::consts::TAU;

/// One directional Gerstner octave. `dir` is the horizontal travel direction
/// (normalized on use; a zero vector is treated as +x); `wavelength` the crest
/// spacing; `steepness` the crest pinch `[0,1]`; `amplitude` the height; `speed`
/// the phase advance per unit time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveOctave {
    pub dir: [f32; 2],
    pub wavelength: f32,
    pub steepness: f32,
    pub amplitude: f32,
    pub speed: f32,
}

#[inline]
fn norm2(d: [f32; 2]) -> [f32; 2] {
    let l = (d[0] * d[0] + d[1] * d[1]).sqrt();
    if l < 1e-6 { [1.0, 0.0] } else { [d[0] / l, d[1] / l] }
}

/// The phase of one octave at world-plane position `pos` and time `t`:
/// `k·(dir·pos) − speed·k·t`, `k = 2π/wavelength`.
fn octave_phase(pos: [f32; 2], t: f32, oct: &WaveOctave) -> f32 {
    let k = TAU / oct.wavelength.max(1e-3);
    let dir = norm2(oct.dir);
    let along = dir[0] * pos[0] + dir[1] * pos[1];
    k * along - oct.speed * k * t
}

/// Summed vertical height of all octaves at `pos`, time `t`. Bounded by the sum
/// of octave amplitudes; finite for any input (zero wavelengths are guarded).
pub fn multi_octave_height(pos: [f32; 2], t: f32, octaves: &[WaveOctave]) -> f32 {
    octaves.iter().map(|o| wave(octave_phase(pos, t, o), o.steepness, o.amplitude).1).sum()
}

/// Full Gerstner displacement `[dx, dy, dz]` of the surface point at `pos`:
/// horizontal crest pinch along each octave's direction plus the summed height
/// in `dy`. This is what a vertex shader adds to the flat water grid.
pub fn multi_octave_displacement(pos: [f32; 2], t: f32, octaves: &[WaveOctave]) -> [f32; 3] {
    let mut out = [0.0f32; 3];
    for o in octaves {
        let (offset, height) = wave(octave_phase(pos, t, o), o.steepness, o.amplitude);
        let dir = norm2(o.dir);
        out[0] += dir[0] * offset;
        out[2] += dir[1] * offset;
        out[1] += height;
    }
    out
}

/// Sum of octave amplitudes — the height bound `|multi_octave_height| <= this`.
pub fn amplitude_bound(octaves: &[WaveOctave]) -> f32 {
    octaves.iter().map(|o| o.amplitude.max(0.0)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;
    #[test] fn zero_amplitude_flat() { assert_eq!(wave(1.0, 0.5, 0.0), (0.0, 0.0)); }
    #[test] fn no_steepness_no_offset() { assert_eq!(wave(1.0, 0.0, 2.0).0, 0.0); }
    #[test] fn height_bounded() {
        let mut p = 0.0f32;
        while p < 7.0 { assert!(wave(p, 0.5, 2.0).1.abs() <= 2.0 + 1e-6); p += 0.2; }
    }
    #[test] fn periodic() { assert!((wave(0.3, 0.5, 2.0).1 - wave(0.3 + 2.0 * PI, 0.5, 2.0).1).abs() < 1e-3); }
    #[test] fn amplitude_scales() { assert!((wave(1.0, 0.5, 4.0).1 - 2.0 * wave(1.0, 0.5, 2.0).1).abs() < 1e-5); }
    #[test] fn deterministic() { let a = wave(1.0, 0.5, 2.0); let b = wave(1.0, 0.5, 2.0); assert_eq!(a.0.to_bits(), b.0.to_bits()); }

    // ---- r335-water-visual: multi-octave swell tests ----
    fn swell() -> Vec<WaveOctave> {
        vec![
            WaveOctave { dir: [1.0, 0.0], wavelength: 12.0, steepness: 0.6, amplitude: 0.8, speed: 1.0 },
            WaveOctave { dir: [0.6, 0.8], wavelength: 5.0,  steepness: 0.5, amplitude: 0.3, speed: 1.6 },
            WaveOctave { dir: [-0.3, 1.0], wavelength: 2.3, steepness: 0.4, amplitude: 0.12, speed: 2.2 },
        ]
    }

    #[test]
    fn multi_octave_finite_over_fuzz_incl_degenerate() {
        let mut octs = swell();
        // add a degenerate octave (zero wavelength + zero dir) — must be guarded.
        octs.push(WaveOctave { dir: [0.0, 0.0], wavelength: 0.0, steepness: 0.5, amplitude: 0.2, speed: 1.0 });
        let mut seed = 0x5151_5151u32;
        let mut lcg = || { seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223); (seed >> 8) as f32 / ((1u32<<24) as f32) };
        for _ in 0..3000 {
            let pos = [lcg() * 200.0 - 100.0, lcg() * 200.0 - 100.0];
            let t = lcg() * 100.0;
            let h = multi_octave_height(pos, t, &octs);
            let d = multi_octave_displacement(pos, t, &octs);
            assert!(h.is_finite(), "height NaN/inf");
            assert!(d.iter().all(|v| v.is_finite()), "displacement NaN/inf");
        }
    }

    #[test]
    fn empty_octaves_are_flat() {
        assert_eq!(multi_octave_height([3.0, 4.0], 1.0, &[]), 0.0);
        assert_eq!(multi_octave_displacement([3.0, 4.0], 1.0, &[]), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn height_bounded_by_amplitude_sum() {
        let octs = swell();
        let bound = amplitude_bound(&octs) + 1e-5;
        let mut p = 0.0f32;
        while p < 40.0 {
            let h = multi_octave_height([p, p * 0.5], p * 0.3, &octs);
            assert!(h.abs() <= bound, "height {h} exceeds amplitude bound {bound}");
            p += 0.25;
        }
    }

    #[test]
    fn single_octave_matches_reference_wave() {
        let o = WaveOctave { dir: [1.0, 0.0], wavelength: 8.0, steepness: 0.5, amplitude: 1.3, speed: 1.0 };
        let pos = [2.0, 0.0];
        let t = 0.7;
        let k = TAU / o.wavelength;
        let phase = k * pos[0] - o.speed * k * t;
        let (_, h_ref) = wave(phase, o.steepness, o.amplitude);
        assert!((multi_octave_height(pos, t, &[o]) - h_ref).abs() < 1e-6, "single octave must equal the reference wave height");
    }

    #[test]
    fn more_octaves_add_detail() {
        // adding a fine octave increases spatial variation along a scanline.
        let base = vec![swell()[0]];
        let rich = swell();
        let tv = |octs: &[WaveOctave]| {
            let (mut tv, mut prev) = (0.0f32, multi_octave_height([0.0, 0.0], 0.0, octs));
            for i in 1..=400 { let u = i as f32 * 0.1; let cur = multi_octave_height([u, 0.0], 0.0, octs); tv += (cur-prev).abs(); prev = cur; }
            tv
        };
        assert!(tv(&rich) > tv(&base), "more octaves should add surface detail");
    }

    #[test]
    fn multi_octave_deterministic() {
        let octs = swell();
        assert_eq!(multi_octave_height([1.5, 2.5], 3.0, &octs).to_bits(), multi_octave_height([1.5, 2.5], 3.0, &octs).to_bits());
        assert_eq!(multi_octave_displacement([1.5, 2.5], 3.0, &octs), multi_octave_displacement([1.5, 2.5], 3.0, &octs));
    }
}
