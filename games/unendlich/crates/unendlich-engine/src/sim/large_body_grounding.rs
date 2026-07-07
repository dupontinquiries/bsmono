//! large_body_grounding — keep big creatures *on* the world. A single-point ground
//! probe makes a long dragon teeter and float when one sample lands on a lip; this
//! samples the terrain under several footprint points, fits a contact plane to
//! them for the lean, rests the body on the highest contact (so no foot clips
//! under or floats over), and clamps the lean by body size (a 1-block step barely
//! tilts something huge). Pure + deterministic.

/// A terrain sample under the footprint: body-local `(x, z)` offset and the ground
/// height there.
#[derive(Clone, Copy, Debug)]
pub struct FootSample {
    pub x: f32,
    pub z: f32,
    pub ground: f32,
}

/// The grounding solution.
#[derive(Clone, Copy, Debug)]
pub struct Grounding {
    /// Height to rest the body at — the highest contact under the footprint.
    pub height: f32,
    /// Forward tilt (radians), size-clamped.
    pub pitch: f32,
    /// Sideways tilt (radians), size-clamped.
    pub roll: f32,
}

/// Fit `ground ≈ A·x + B·z + C` by least squares over the samples, returning the
/// slopes `(A, B)`; `(0, 0)` if the system is degenerate (collinear / too few).
fn fit_slopes(samples: &[FootSample]) -> (f32, f32) {
    let n = samples.len() as f32;
    if samples.len() < 3 {
        return (0.0, 0.0);
    }
    let (mut sx, mut sz, mut sh) = (0.0, 0.0, 0.0);
    let (mut sxx, mut szz, mut sxz, mut sxh, mut szh) = (0.0, 0.0, 0.0, 0.0, 0.0);
    for s in samples {
        sx += s.x;
        sz += s.z;
        sh += s.ground;
        sxx += s.x * s.x;
        szz += s.z * s.z;
        sxz += s.x * s.z;
        sxh += s.x * s.ground;
        szh += s.z * s.ground;
    }
    // Solve the 3×3 normal equations for (A, B, C) via the upper 2×2 in centered
    // form: cov * [A, B] = [cov_xh, cov_zh].
    let cxx = sxx - sx * sx / n;
    let czz = szz - sz * sz / n;
    let cxz = sxz - sx * sz / n;
    let cxh = sxh - sx * sh / n;
    let czh = szh - sz * sh / n;
    let det = cxx * czz - cxz * cxz;
    if det.abs() < 1e-6 {
        return (0.0, 0.0);
    }
    let a = (cxh * czz - czh * cxz) / det;
    let b = (czh * cxx - cxh * cxz) / det;
    (a, b)
}

/// Solve grounding for a footprint. `size` is the body scale (1 = baseline);
/// larger bodies tilt less for the same slope. `max_lean` caps |pitch|/|roll|.
pub fn solve(samples: &[FootSample], size: f32, max_lean: f32) -> Grounding {
    if samples.is_empty() {
        return Grounding { height: 0.0, pitch: 0.0, roll: 0.0 };
    }
    // Rest on the highest contact — never float over or clip under a foot.
    let mut height = samples[0].ground;
    for s in samples {
        if s.ground > height {
            height = s.ground;
        }
    }
    let (a, b) = fit_slopes(samples);
    let s = size.max(1.0);
    let cap = max_lean.abs();
    let pitch = (a.atan() / s).clamp(-cap, cap);
    let roll = (b.atan() / s).clamp(-cap, cap);
    Grounding { height, pitch, roll }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }
    fn square(h: f32) -> Vec<FootSample> {
        vec![
            FootSample { x: -1.0, z: -1.0, ground: h },
            FootSample { x: 1.0, z: -1.0, ground: h },
            FootSample { x: -1.0, z: 1.0, ground: h },
            FootSample { x: 1.0, z: 1.0, ground: h },
        ]
    }

    #[test]
    fn flat_ground_gives_zero_lean() {
        let g = solve(&square(64.0), 1.0, 0.6);
        assert!(g.pitch.abs() < 1e-5 && g.roll.abs() < 1e-5, "flat → level: {} {}", g.pitch, g.roll);
        assert!((g.height - 64.0).abs() < 1e-5, "rests on the ground");
    }

    #[test]
    fn rests_on_the_highest_contact_no_float() {
        let mut s = square(64.0);
        s[0].ground = 66.0; // one foot on a higher lip
        let g = solve(&s, 1.0, 1.5);
        assert!((g.height - 66.0).abs() < 1e-5, "height = max sample (no foot underground): {}", g.height);
    }

    #[test]
    fn a_slope_tilts_but_a_bigger_body_tilts_less() {
        // ground rises 2 over the 2-wide footprint in +x → a real slope
        let s = vec![
            FootSample { x: -1.0, z: -1.0, ground: 0.0 },
            FootSample { x: 1.0, z: -1.0, ground: 2.0 },
            FootSample { x: -1.0, z: 1.0, ground: 0.0 },
            FootSample { x: 1.0, z: 1.0, ground: 2.0 },
        ];
        let small = solve(&s, 1.0, 1.5);
        let big = solve(&s, 3.0, 1.5);
        assert!(small.pitch.abs() > 0.1, "small body leans on the slope");
        assert!(big.pitch.abs() < small.pitch.abs(), "bigger body leans less: {} vs {}", big.pitch, small.pitch);
    }

    #[test]
    fn prop_lean_bounded_height_is_max_and_finite() {
        let mut seed = 0x6105_0DEDu32;
        for _ in 0..3000 {
            let n = 3 + (lcg(&mut seed) * 6.0) as usize;
            let mut s = Vec::new();
            let mut hmax = f32::MIN;
            for _ in 0..n {
                let g = lcg(&mut seed) * 80.0;
                hmax = hmax.max(g);
                s.push(FootSample { x: lcg(&mut seed) * 4.0 - 2.0, z: lcg(&mut seed) * 4.0 - 2.0, ground: g });
            }
            let size = 1.0 + lcg(&mut seed) * 4.0;
            let cap = 0.2 + lcg(&mut seed);
            let g = solve(&s, size, cap);
            assert!(g.pitch.is_finite() && g.roll.is_finite() && g.height.is_finite(), "finite");
            assert!(g.pitch.abs() <= cap + 1e-6 && g.roll.abs() <= cap + 1e-6, "lean clamped");
            assert!((g.height - hmax).abs() < 1e-4, "height is the max contact");
        }
    }
}
