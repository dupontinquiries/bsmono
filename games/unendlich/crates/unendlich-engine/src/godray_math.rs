//! godray_math.rs — pure radial in-scatter weight toward the sun.
//!
//! A normalized light-shaft (god-ray) accumulation: march in screen space from a
//! fragment toward the sun, weighting each sample by its proximity to the sun
//! and a per-step decay. The result is a weighted average in `[0,1]`, so it
//! never blows up regardless of sample count. Pure, deterministic, panic-free
//! (R1/R11/R21).

/// Radial in-scatter weight at `frag_uv` toward `sun_uv` (both in screen `[0,1]`
/// space). `density` scales the march step length, `decay` (clamped `[0,1]`)
/// attenuates later (nearer-the-sun) samples so a higher decay yields *less*
/// shaft, and `samples` is the march count. Returns a value in `[0,1]`,
/// maximized when the fragment sits on the sun.
pub fn godray(sun_uv: [f32; 2], frag_uv: [f32; 2], density: f32, decay: f32, samples: u32) -> f32 {
    if samples == 0 {
        return 0.0;
    }
    let n = samples as f32;
    let d = decay.max(0.0).min(1.0);
    // step toward the sun
    let dx = (sun_uv[0] - frag_uv[0]) / n * density;
    let dy = (sun_uv[1] - frag_uv[1]) / n * density;

    let mut px = frag_uv[0];
    let mut py = frag_uv[1];
    let mut illum = 1.0f32;
    let mut acc = 0.0f32;
    let mut norm = 0.0f32;

    for _ in 0..samples {
        // proximity to the sun at this sample: 1 on the sun, 0 at distance >= 1
        let ddx = px - sun_uv[0];
        let ddy = py - sun_uv[1];
        let dist = (ddx * ddx + ddy * ddy).sqrt();
        let s = (1.0 - dist).max(0.0).min(1.0);

        acc += s * illum;
        norm += illum;
        illum *= 1.0 - d; // attenuate; higher decay -> faster falloff -> less shaft

        px += dx;
        py += dy;
    }

    if norm > 0.0 {
        (acc / norm).max(0.0).min(1.0)
    } else {
        0.0
    }
}

// ── r334-fog ────────────────────────────────────────────────────────────────
// A sun-angle godray weight to modulate the screen-space [`godray`] shafts:
// peaks when the view points at the sun, fades off-axis, and is night-gated to
// zero. Pure, finite, panic-free. (Standalone-gateable: scalars in/out.)

/// Godray strength multiplier in `[0,1]` from geometry alone.
/// `view_dot_sun` is `dot(view_dir, sun_dir)` (1 = looking straight at the
/// sun); `sun_elev_sin` is `sun_dir.y` (night gate, same ramp as in-scatter).
/// `sharpness >= 1` tightens the forward lobe. Maximised at the sun, 0 at night
/// or when looking away from the sun.
pub fn godray_sun_weight(view_dot_sun: f32, sun_elev_sin: f32, sharpness: f32) -> f32 {
    let forward = view_dot_sun.clamp(0.0, 1.0); // only the sun-facing hemisphere
    let s = sharpness.max(1.0);
    let lobe = forward.powf(s);
    let day = (sun_elev_sin / 0.1).clamp(0.0, 1.0);
    (lobe * day).clamp(0.0, 1.0)
}

#[cfg(test)]
mod r334_fog_tests {
    use super::*;

    #[test]
    fn peaks_toward_the_sun() {
        // Looking straight at a high sun → full weight.
        assert!((godray_sun_weight(1.0, 1.0, 4.0) - 1.0).abs() < 1e-6);
        // Off-axis is weaker; away from the sun is zero.
        assert!(godray_sun_weight(0.5, 1.0, 4.0) < godray_sun_weight(0.95, 1.0, 4.0));
        assert_eq!(godray_sun_weight(-0.3, 1.0, 4.0), 0.0);
    }

    #[test]
    fn zero_at_night() {
        assert_eq!(godray_sun_weight(1.0, 0.0, 4.0), 0.0);
        assert_eq!(godray_sun_weight(1.0, -0.5, 4.0), 0.0);
    }

    #[test]
    fn sharper_lobe_is_tighter() {
        // Higher sharpness → less weight off-axis (faster falloff).
        let soft = godray_sun_weight(0.7, 1.0, 1.0);
        let sharp = godray_sun_weight(0.7, 1.0, 8.0);
        assert!(sharp < soft);
    }

    #[test]
    fn bounded_and_sharpness_floored() {
        // sharpness < 1 is floored to 1 (no blow-up), result stays in [0,1].
        for &dot in &[-1.0f32, 0.0, 0.4, 1.0] {
            let w = godray_sun_weight(dot, 0.5, 0.0);
            assert!((0.0..=1.0).contains(&w));
        }
    }

    #[test]
    fn dawn_ramps() {
        assert!(godray_sun_weight(1.0, 0.02, 4.0) < godray_sun_weight(1.0, 0.2, 4.0));
    }

    #[test]
    fn deterministic() {
        assert_eq!(
            godray_sun_weight(0.8, 0.5, 4.0).to_bits(),
            godray_sun_weight(0.8, 0.5, 4.0).to_bits()
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / ((1u32 << 24) as f32)
    }

    #[test]
    fn max_at_the_sun() {
        let sun = [0.5f32, 0.5];
        let on_sun = godray(sun, sun, 1.0, 0.2, 32);
        assert!((on_sun - 1.0).abs() < 1e-5, "not max on the sun: {}", on_sun);
        // anywhere else is <= the on-sun value
        let mut seed = 0x6004_0001u32;
        for _ in 0..200 {
            let frag = [lcg(&mut seed), lcg(&mut seed)];
            assert!(godray(sun, frag, 1.0, 0.2, 32) <= on_sun + 1e-6);
        }
    }

    #[test]
    fn falls_off_with_distance() {
        let sun = [0.5f32, 0.5];
        let near = godray(sun, [0.55, 0.5], 1.0, 0.2, 32);
        let mid = godray(sun, [0.75, 0.5], 1.0, 0.2, 32);
        let far = godray(sun, [0.95, 0.5], 1.0, 0.2, 32);
        assert!(near > mid && mid > far, "not falling off: {} {} {}", near, mid, far);
    }

    #[test]
    fn higher_decay_gives_less() {
        let sun = [0.5f32, 0.5];
        let frag = [0.8, 0.5];
        let lo = godray(sun, frag, 1.0, 0.1, 64);
        let hi = godray(sun, frag, 1.0, 0.9, 64);
        assert!(hi < lo, "higher decay not less: {} !< {}", hi, lo);
    }

    #[test]
    fn result_in_unit_range() {
        let mut seed = 0x6004_0002u32;
        for _ in 0..2000 {
            let sun = [lcg(&mut seed), lcg(&mut seed)];
            let frag = [lcg(&mut seed), lcg(&mut seed)];
            let dens = lcg(&mut seed) * 3.0;
            let dec = lcg(&mut seed);
            let smp = (lcg(&mut seed) * 128.0) as u32;
            let r = godray(sun, frag, dens, dec, smp);
            assert!((0.0..=1.0).contains(&r), "out of range: {}", r);
        }
    }

    #[test]
    fn more_samples_stays_bounded() {
        let sun = [0.5f32, 0.5];
        let frag = [0.7, 0.3];
        for &n in &[1u32, 8, 64, 512, 4096] {
            let r = godray(sun, frag, 1.0, 0.3, n);
            assert!((0.0..=1.0).contains(&r), "blow-up at {} samples: {}", n, r);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(
            godray([0.2, 0.8], [0.6, 0.4], 1.2, 0.25, 48),
            godray([0.2, 0.8], [0.6, 0.4], 1.2, 0.25, 48)
        );
    }
}
