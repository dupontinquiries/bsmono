//! CPU mirror of `shaders/water_waves.glsl` — the single source of truth for
//! the water surface wave field, transcribed for the CPU so entity physics
//! (buoyancy, splashes, wakes) agrees with the rendered surface.
//!
//! KEEP IN LOCKSTEP with the GLSL (see toolkit/harness/RULESET.md §water): same
//! constants, same per-octave phase offset, same column-major rotation, same
//! squared-sine waveform, same swell. The only permitted divergence is last-ULP
//! differences between GPU and CPU `sin`/`cos`/`sqrt`, which are sub-micron for
//! wave height and irrelevant to buoyancy.
//!
//! Intentionally dependency-free (plain `f32`, positions as `(x, z)`): the field
//! is pure math, so this is a direct line-by-line transcription of the shader.

// ── constants (mirror water_waves.glsl exactly) ─────────────────────────────
pub const WW_OCTAVES: usize = 4;
pub const WW_TAU: f32 = 6.283_185_3;
pub const WW_GRAVITY: f32 = 9.8;
pub const WW_BASE_LAMBDA: f32 = 5.5;
pub const WW_LACUNARITY: f32 = 1.62;
pub const WW_PERSISTENCE: f32 = 0.55;
pub const WW_BASE_AMP: f32 = 0.09;
pub const WW_FAN: f32 = 0.5;
pub const WW_SWELL_FREQ: f32 = 0.03;
pub const WW_SWELL_SPEED: f32 = 0.25;
pub const WW_SWELL_MIN: f32 = 0.45;

/// Resolved wave inputs (mirror of the GLSL `WaveParams`).
#[derive(Clone, Copy, Debug)]
pub struct WaveParams {
    pub dir_x: f32,
    pub dir_z: f32,
    pub amp_scale: f32,
    pub speed_scale: f32,
    pub t: f32,
}

#[inline]
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    crate::util::mathx::smoothstep(e0, e1, x) // r343 dedup
}

#[inline]
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    crate::util::mathx::lerp(a, b, t) // r342 dedup
}

#[inline]
fn ww_phase_offset(i: usize) -> f32 {
    i as f32 * 2.3994
}

/// Resolve wave parameters from the engine's live fields. Mirrors
/// `water_wave_params` in the shader.
///
/// * `wind_x`, `wind_z` — wind vector (xz)
/// * `wind_mag` — wind magnitude
/// * `temp` — temperature in [0, 1] (near 0 ⇒ calm/glassy)
/// * `t` — time in seconds
pub fn water_wave_params(wind_x: f32, wind_z: f32, wind_mag: f32, temp: f32, t: f32) -> WaveParams {
    let wm = (wind_x * wind_x + wind_z * wind_z).sqrt();
    let (dir_x, dir_z) = if wm > 1e-4 {
        (wind_x / wm, wind_z / wm)
    } else {
        (0.707_106_78, 0.707_106_78)
    };
    let wind = wind_mag.clamp(0.0, 2.0);
    let thaw = smoothstep(0.05, 0.22, temp.clamp(0.0, 1.0));
    let amp_scale = (0.30 + 0.70 * smoothstep(0.0, 1.0, wind)) * lerp(0.12, 1.0, thaw);
    let speed_scale = 0.60 + 0.50 * wind;
    WaveParams { dir_x, dir_z, amp_scale, speed_scale, t }
}

#[inline]
fn ww_swell(px: f32, pz: f32, p: &WaveParams) -> f32 {
    let phase = (p.dir_x * px + p.dir_z * pz) * WW_SWELL_FREQ - p.t * WW_SWELL_SPEED;
    lerp(WW_SWELL_MIN, 1.0, 0.5 + 0.5 * phase.sin())
}

/// Wave height and its analytic XZ gradient in one pass.
/// Returns `(height, dHeight/dx, dHeight/dz)`, height in metres. Mirrors
/// `water_wave_height_grad`.
pub fn water_wave_height_grad(px: f32, pz: f32, p: &WaveParams) -> (f32, f32, f32) {
    let mut height = 0.0f32;
    let mut d_hx = 0.0f32;
    let mut d_hz = 0.0f32;

    let mut lambda = WW_BASE_LAMBDA;
    let mut amp = WW_BASE_AMP * p.amp_scale * ww_swell(px, pz, p);

    for i in 0..WW_OCTAVES {
        let k = WW_TAU / lambda;
        let w = (WW_GRAVITY * k).sqrt() * p.speed_scale;
        let ang = WW_FAN * (i as f32 - 1.5);
        // Column-major mat2(c, s, -s, c) * dir == rotate(dir, ang).
        let (c, s) = (ang.cos(), ang.sin());
        let dx = c * p.dir_x - s * p.dir_z;
        let dz = s * p.dir_x + c * p.dir_z;

        let phase = k * (dx * px + dz * pz) - w * p.t + ww_phase_offset(i);
        let sn = phase.sin();
        let g = 0.5 + 0.5 * sn;
        height += amp * g * g;
        let dh = amp * g * phase.cos();
        d_hx += dh * k * dx;
        d_hz += dh * k * dz;

        lambda /= WW_LACUNARITY;
        amp *= WW_PERSISTENCE;
    }
    (height, d_hx, d_hz)
}

/// Wave height at a world XZ position (what buoyancy queries).
#[inline]
pub fn water_wave_height(px: f32, pz: f32, p: &WaveParams) -> f32 {
    water_wave_height_grad(px, pz, p).0
}

/// World-space up-facing surface normal `[x, y, z]`. `grazing` in [0, 1] damps
/// the slope at grazing angles (mirrors `water_wave_normal`).
pub fn water_wave_normal(px: f32, pz: f32, p: &WaveParams, grazing: f32) -> [f32; 3] {
    let (_, dhx, dhz) = water_wave_height_grad(px, pz, p);
    let infl = lerp(0.15, 1.0, grazing);
    let mut n = [-dhx * infl, 1.0, -dhz * infl];
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    n[0] /= len;
    n[1] /= len;
    n[2] /= len;
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, tol: f32) -> bool {
        (a - b).abs() <= tol
    }

    #[test]
    fn field_is_deterministic_and_bounded() {
        let p = water_wave_params(1.0, 0.0, 0.8, 0.6, 12.0);
        let h1 = water_wave_height(10.0, 20.0, &p);
        let h2 = water_wave_height(10.0, 20.0, &p);
        assert_eq!(h1, h2, "field must be deterministic");
        // Amplitudes sum to a bounded crest; never a runaway height.
        assert!(h1.abs() < 0.5, "height {h1} out of expected range");
    }

    #[test]
    fn normal_is_unit_and_up_facing() {
        let p = water_wave_params(0.3, 0.95, 1.2, 0.7, 4.0);
        let n = water_wave_normal(13.0, -5.0, &p, 1.0);
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        assert!(approx(len, 1.0, 1e-5), "normal not unit length: {len}");
        assert!(n[1] > 0.5, "water normal should face up: {n:?}");
    }

    #[test]
    fn analytic_gradient_matches_finite_difference() {
        // Validates the core optimization: the normal comes from the analytic
        // gradient (1 eval) rather than a 3-tap finite difference. They must
        // agree. (The ~200 m swell's own slope is intentionally omitted from the
        // analytic normal, so a small tolerance covers that low-frequency term.)
        let p = water_wave_params(0.6, 0.8, 1.0, 0.7, 3.5);
        let (px, pz) = (4.0, -7.0);
        let eps = 0.001;
        let (_, ax, az) = water_wave_height_grad(px, pz, &p);
        let fx = (water_wave_height(px + eps, pz, &p) - water_wave_height(px - eps, pz, &p))
            / (2.0 * eps);
        let fz = (water_wave_height(px, pz + eps, &p) - water_wave_height(px, pz - eps, &p))
            / (2.0 * eps);
        assert!(approx(ax, fx, 2e-2), "dH/dx analytic {ax} vs finite {fx}");
        assert!(approx(az, fz, 2e-2), "dH/dz analytic {az} vs finite {fz}");
    }

    #[test]
    fn physics_drives_amplitude() {
        // Near-freezing water is much calmer than warm water.
        let cold = water_wave_params(1.0, 0.0, 0.8, 0.02, 5.0);
        let warm = water_wave_params(1.0, 0.0, 0.8, 0.90, 5.0);
        assert!(
            cold.amp_scale < warm.amp_scale * 0.5,
            "cold {} should be far calmer than warm {}",
            cold.amp_scale,
            warm.amp_scale
        );
        // Still air is calmer than a stiff breeze.
        let calm = water_wave_params(0.0, 0.0, 0.0, 0.90, 5.0);
        let windy = water_wave_params(1.0, 0.0, 1.5, 0.90, 5.0);
        assert!(
            calm.amp_scale < windy.amp_scale,
            "calm {} should be < windy {}",
            calm.amp_scale,
            windy.amp_scale
        );
        // Wind sets propagation direction (normalized).
        let p = water_wave_params(3.0, 4.0, 1.0, 0.8, 0.0);
        assert!(approx((p.dir_x * p.dir_x + p.dir_z * p.dir_z).sqrt(), 1.0, 1e-5));
        assert!(approx(p.dir_x, 0.6, 1e-4) && approx(p.dir_z, 0.8, 1e-4));
    }

    #[test]
    fn swell_modulates_amplitude_across_space() {
        // The swell makes local wave energy vary across space (calm vs choppy
        // patches). Compare mean |height| in a swell-crest region (quarter
        // wavelength along wind, swell≈1.0) vs a swell-trough region
        // (three-quarter wavelength, swell≈0.45).
        let p = water_wave_params(1.0, 0.0, 1.0, 0.9, 0.0);
        let wl = WW_TAU / WW_SWELL_FREQ; // swell wavelength along wind
        let energy = |x0: f32| {
            let mut e = 0.0;
            for j in 0..32 {
                e += water_wave_height(x0 + j as f32 * 0.37, 0.0, &p).abs();
            }
            e / 32.0
        };
        let crest = energy(wl * 0.25); // swell ≈ 1.0 (choppy)
        let trough = energy(wl * 0.75); // swell ≈ 0.45 (calm)
        assert!(
            crest > trough * 1.2,
            "swell should make the crest region choppier than the trough: {crest} vs {trough}"
        );
    }
}
