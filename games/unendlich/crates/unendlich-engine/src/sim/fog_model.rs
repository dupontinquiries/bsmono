//! `sim::fog_model` — a clean-room, CPU-side model of the physics-based fog
//! system: Beer-Lambert transmittance, height-stratified density, the standard
//! scattering phase functions, the closed-form (non-raymarched) airmass
//! integral, and the underwater absorption model.
//!
//! This is derived independently from published atmospheric-scattering and
//! radiative-transfer results (Rayleigh/Mie phase functions, Henyey-Greenstein,
//! Cornette-Shanks, Beer-Lambert) — textbook mathematics, no shader source
//! reproduced. It exists so the engine has a reference the GPU shader can be
//! validated against, an analytic fallback for the CPU/low-end path, and — most
//! usefully for gameplay — a fog-limited **visibility distance** (how far a mob
//! or player can see before fog swallows a target).
//!
//! Conventions: coefficients σ are per world-unit extinction/scattering;
//! distances and heights are world units; `cos_theta` is the cosine of the angle
//! between the incident and scattered directions. All functions are pure and
//! deterministic (`f32` math, std only).

/// Rayleigh volume-scattering coefficients baked to the R/G/B primaries
/// (680/550/440 nm), scaled to world units. Blue scatters far more than red,
/// which is why distant fog and sky trend blue. (Reference magnitudes from the
/// standard-atmosphere derivation, ×10⁻⁶ m⁻¹, then world-scaled by the caller.)
pub const RAYLEIGH_RGB: [f32; 3] = [5.802e-6, 13.558e-6, 33.100e-6];

/// Example underwater absorption per unit depth (R/G/B): red is absorbed first,
/// giving the blue-green shift at depth.
pub const WATER_ABSORB_RGB: [f32; 3] = [0.45, 0.06, 0.02];
/// Wavelength-independent underwater scattering approximation, per unit depth.
pub const WATER_SCATTER: f32 = 0.07;

const FOUR_PI: f32 = 4.0 * std::f32::consts::PI;
const LN2: f32 = std::f32::consts::LN_2;

// ---------------------------------------------------------------------------
// Beer-Lambert extinction
// ---------------------------------------------------------------------------

/// Optical depth τ over a homogeneous segment: `σ_t · d`.
pub fn optical_depth(sigma_t: f32, distance: f32) -> f32 {
    sigma_t.max(0.0) * distance.max(0.0)
}

/// Beer-Lambert transmittance over a homogeneous segment: `exp(-σ_t · d)`.
/// In `(0, 1]`: 1 at zero distance/zero extinction, → 0 as either grows.
pub fn transmittance(sigma_t: f32, distance: f32) -> f32 {
    (-optical_depth(sigma_t, distance)).exp()
}

/// Transmittance from a precomputed optical depth (e.g. the analytic airmass ×
/// σ_t). `exp(-τ)`.
pub fn transmittance_from_tau(tau: f32) -> f32 {
    (-tau.max(0.0)).exp()
}

/// Distance at which transmittance falls to `threshold` in a homogeneous medium
/// of extinction `sigma_t`: `-ln(threshold) / σ_t`. This is the practical
/// "visibility distance" — how far you can see a target before fog hides it.
/// Returns `f32::INFINITY` when there is effectively no fog. `threshold` is
/// clamped to `(0, 1)`.
pub fn visibility_distance(sigma_t: f32, threshold: f32) -> f32 {
    let st = sigma_t.max(0.0);
    if st <= 0.0 {
        return f32::INFINITY;
    }
    let thr = threshold.clamp(1.0e-6, 1.0 - 1.0e-6);
    -thr.ln() / st
}

// ---------------------------------------------------------------------------
// Height-stratified density
// ---------------------------------------------------------------------------

/// Height density with an exponential half-life profile: density is `1` at (and
/// below) `y_ref` and **halves every `half_life` world units of altitude gain**,
/// clamped to `[0, 1]`. `2^(-(y - y_ref) / H)`.
///
/// (This is the standard half-life form the spec targets — density that hugs the
/// ground and thins with altitude — expressed directly so the "hugs the ground"
/// intent is exact.)
pub fn height_density(y: f32, y_ref: f32, half_life: f32) -> f32 {
    if half_life <= 0.0 {
        return if y <= y_ref { 1.0 } else { 0.0 };
    }
    let d = exp2(-(y - y_ref) / half_life);
    d.clamp(0.0, 1.0)
}

/// Linear fade applied below the fog volume so density goes to zero under the
/// sea floor rather than growing. `clamp((y - vol_bottom)/(sea_level - vol_bottom), 0, 1)`.
pub fn sea_level_fade(y: f32, vol_bottom: f32, sea_level: f32) -> f32 {
    let span = sea_level - vol_bottom;
    if span.abs() < 1.0e-6 {
        return if y >= sea_level { 1.0 } else { 0.0 };
    }
    ((y - vol_bottom) / span).clamp(0.0, 1.0)
}

/// Closed-form airmass (integrated density) of the exponential profile along a
/// ray segment — the analytic, non-raymarched path. Integrates
/// `ρ(y) = 2^(-(y - y_ref)/H)` from the origin height `y0` along vertical
/// direction `dir_y` for `ray_len` units. Result is in density·distance
/// (multiply by a species' σ to get optical depth). Clamped to
/// `[0, ray_len]` for a unit-max density.
///
/// For a near-horizontal ray (`|dir_y|` ~ 0) the height barely changes, so the
/// integral collapses to `ρ(y0) · ray_len`.
pub fn airmass_exponential(y0: f32, dir_y: f32, ray_len: f32, y_ref: f32, half_life: f32) -> f32 {
    let ray_len = ray_len.max(0.0);
    if half_life <= 0.0 {
        // step profile: full density where y <= y_ref
        return if y0 <= y_ref { ray_len } else { 0.0 };
    }
    // ρ(y) = exp(a * (y - y_ref)), a = -ln2 / H  (negative → thins upward)
    let a = -LN2 / half_life;
    let rho0 = (a * (y0 - y_ref)).exp();
    let c1 = a * dir_y; // d/ds of the exponent
    let mass = if c1.abs() < 1.0e-6 {
        // horizontal-ish: constant density along the segment
        rho0 * ray_len
    } else {
        let rho_end = (a * (y0 + dir_y * ray_len - y_ref)).exp();
        (rho_end - rho0) / c1
    };
    mass.clamp(0.0, ray_len)
}

// ---------------------------------------------------------------------------
// Phase functions (angular scattering distributions; each integrates to 1 over
// the sphere)
// ---------------------------------------------------------------------------

/// Rayleigh phase function `(3/16π)(1 + cos²θ)`. Symmetric — equal forward and
/// backward scattering, peaked at both poles.
pub fn rayleigh_phase(cos_theta: f32) -> f32 {
    (3.0 / (4.0 * FOUR_PI)) * (1.0 + cos_theta * cos_theta)
}

/// Henyey-Greenstein phase function. `g ∈ (-1, 1)` is the asymmetry: `g > 0`
/// forward-scatters (fog, water droplets ≈ 0.5–0.85), `g = 0` is isotropic
/// (`1/4π`), `g < 0` back-scatters.
pub fn hg_phase(cos_theta: f32, g: f32) -> f32 {
    let g = g.clamp(-0.999, 0.999);
    let g2 = g * g;
    let denom = (1.0 + g2 - 2.0 * g * cos_theta).max(1.0e-6).powf(1.5);
    (1.0 / FOUR_PI) * (1.0 - g2) / denom
}

/// Cornette-Shanks phase function — a more physically-motivated forward-peak
/// approximation that matches Mie theory better at high `g`.
pub fn cornette_shanks_phase(cos_theta: f32, g: f32) -> f32 {
    let g = g.clamp(-0.999, 0.999);
    let g2 = g * g;
    let num = 1.0 + cos_theta * cos_theta;
    let denom = (1.0 + g2 - 2.0 * g * cos_theta).max(1.0e-6).powf(1.5);
    (1.5 / FOUR_PI) * ((1.0 - g2) / (2.0 + g2)) * (num / denom)
}

/// Double-lobe Henyey-Greenstein: a weighted blend of a forward lobe (`g1`) and
/// a weaker backward lobe (`g2`), capturing the real aerosol shape.
/// `f · HG(g1) + (1-f) · HG(g2)`.
pub fn double_lobe_hg(cos_theta: f32, f: f32, g1: f32, g2: f32) -> f32 {
    let f = f.clamp(0.0, 1.0);
    f * hg_phase(cos_theta, g1) + (1.0 - f) * hg_phase(cos_theta, g2)
}

/// Approximate multiple scattering as a geometric series of single-scatter
/// evaluations: `Σ_{k=0}^{n} c^k · HG(cosθ, g · b^k)`. Each order dims by `c` and
/// loses anisotropy by `b`, brightening shadowed fog. Good defaults `c = 0.5`,
/// `b = 0.7`, `n = 3..4`.
pub fn multi_scatter_hg(cos_theta: f32, g: f32, c: f32, b: f32, n: u32) -> f32 {
    let c = c.clamp(0.0, 0.999);
    let b = b.clamp(0.0, 1.0);
    let mut sum = 0.0;
    let mut c_pow = 1.0;
    let mut g_scale = 1.0;
    for _ in 0..=n {
        sum += c_pow * hg_phase(cos_theta, g * g_scale);
        c_pow *= c;
        g_scale *= b;
    }
    sum
}

// ---------------------------------------------------------------------------
// Underwater fog
// ---------------------------------------------------------------------------

/// Single-scattering albedo `ω₀ = σ_s / σ_t`. `0` = pure absorber, `1` = pure
/// scatterer.
pub fn single_scatter_albedo(sigma_s: f32, sigma_t: f32) -> f32 {
    if sigma_t <= 0.0 {
        return 0.0;
    }
    (sigma_s / sigma_t).clamp(0.0, 1.0)
}

/// Underwater multiple-scattering energy gain from the single-scattering albedo
/// (`ms_factor = 0.84·ω₀`, `gain = ms_factor/(1-ms_factor)`). Total radiance is
/// `L_single · (1 + gain)` per channel; see [`apply_water_multiscatter`].
pub fn water_multiscatter_gain(albedo: f32) -> f32 {
    let ms = (0.84 * albedo.clamp(0.0, 1.0)).min(0.999);
    ms / (1.0 - ms)
}

/// Apply the underwater multiple-scattering brightening to a per-channel single
/// scatter radiance: `L_single · (1 + gain(ω₀))`.
pub fn apply_water_multiscatter(l_single: f32, sigma_s: f32, sigma_t: f32) -> f32 {
    let gain = water_multiscatter_gain(single_scatter_albedo(sigma_s, sigma_t));
    l_single * (1.0 + gain)
}

// ---------------------------------------------------------------------------
// Renderer alignment
// ---------------------------------------------------------------------------

/// The effective fog extinction the **fragment shader** renders with, so the CPU
/// (vision, culling) and the GPU agree on exactly how dense the fog is at the
/// camera/creature. Mirrors `chunk.frag`: `base · mix(0.6, 2.2, humidity^1.6)` —
/// humidity thickens the fog on a curved response. Pass the same `base_density`
/// and `humidity` the renderer puts in `atmos_params`.
///
/// This is the linchpin of the integration: feed this σ_t into
/// [`visibility_distance`] / [`EyeProfile::in_fog`](crate::sim::eyes::EyeProfile::in_fog)
/// and a creature can see exactly as far as the player can before the fog closes
/// in — no more shooting things through a white wall.
pub fn shader_fog_density(base_density: f32, humidity: f32) -> f32 {
    let h = humidity.clamp(0.0, 1.0);
    let curve = h.powf(1.6);
    base_density.max(0.0) * lerp(0.6, 2.2, curve)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// `2^x` (exp2), via the natural exponent so the module is std-only.
fn exp2(x: f32) -> f32 {
    (x * LN2).exp()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn transmittance_is_bounded_and_monotone() {
        assert!(approx(transmittance(0.1, 0.0), 1.0, 1e-6), "no distance → full");
        assert!(approx(transmittance(0.0, 100.0), 1.0, 1e-6), "no fog → full");
        let a = transmittance(0.1, 10.0);
        let b = transmittance(0.1, 20.0);
        assert!(a > b && b > 0.0 && a < 1.0, "decreases with distance, stays in (0,1)");
    }

    #[test]
    fn visibility_distance_inverts_beer_lambert() {
        let sigma = 0.05;
        let thr = 0.1;
        let d = visibility_distance(sigma, thr);
        // transmittance at that distance should equal the threshold
        assert!(approx(transmittance(sigma, d), thr, 1e-4));
        assert_eq!(visibility_distance(0.0, 0.1), f32::INFINITY, "clear air → see forever");
        // denser fog → shorter sight
        assert!(visibility_distance(0.2, thr) < visibility_distance(0.05, thr));
    }

    #[test]
    fn height_density_halves_each_half_life() {
        let h = 12.0;
        assert!(approx(height_density(0.0, 0.0, h), 1.0, 1e-4), "at ref → 1");
        assert!(approx(height_density(h, 0.0, h), 0.5, 1e-4), "one half-life up → 0.5");
        assert!(approx(height_density(2.0 * h, 0.0, h), 0.25, 1e-4), "two → 0.25");
        assert!(approx(height_density(-50.0, 0.0, h), 1.0, 1e-6), "below ref clamps to 1");
        // hugs the ground: lower is always denser
        assert!(height_density(5.0, 0.0, h) > height_density(40.0, 0.0, h));
    }

    #[test]
    fn sea_level_fade_ramps() {
        assert_eq!(sea_level_fade(0.0, 0.0, 10.0), 0.0);
        assert_eq!(sea_level_fade(10.0, 0.0, 10.0), 1.0);
        assert!(approx(sea_level_fade(5.0, 0.0, 10.0), 0.5, 1e-6));
        assert_eq!(sea_level_fade(-3.0, 0.0, 10.0), 0.0, "below bottom → 0");
    }

    #[test]
    fn analytic_airmass_matches_numerical_integration() {
        // The spec's acceptance test 12.1: analytic and raymarched density
        // integrals must agree. March the exponential density and compare.
        let (y0, dir_y, len, y_ref, h) = (4.0f32, 0.6f32, 50.0f32, 0.0f32, 12.0f32);
        let analytic = airmass_exponential(y0, dir_y, len, y_ref, h);
        let steps = 4000;
        let ds = len / steps as f32;
        let mut numeric = 0.0;
        for i in 0..steps {
            let s = (i as f32 + 0.5) * ds;
            let y = y0 + dir_y * s;
            numeric += height_density(y, y_ref, h) * ds;
        }
        assert!(approx(analytic, numeric, 0.05), "analytic {analytic} vs numeric {numeric}");
    }

    #[test]
    fn analytic_airmass_horizontal_is_constant_density() {
        // near-horizontal ray → mass ≈ ρ(y0) · len
        let mass = airmass_exponential(6.0, 0.0, 30.0, 0.0, 12.0);
        let expect = height_density(6.0, 0.0, 12.0) * 30.0;
        assert!(approx(mass, expect, 1e-3));
    }

    #[test]
    fn hg_isotropic_at_zero_g() {
        let iso = 1.0 / FOUR_PI;
        for &c in &[-1.0, -0.3, 0.0, 0.5, 1.0] {
            assert!(approx(hg_phase(c, 0.0), iso, 1e-6), "g=0 is isotropic");
        }
    }

    #[test]
    fn hg_forward_scatters_when_g_positive() {
        let fwd = hg_phase(1.0, 0.7); // toward = strong
        let back = hg_phase(-1.0, 0.7);
        assert!(fwd > back, "positive g peaks forward");
        assert!(hg_phase(-1.0, -0.7) > hg_phase(1.0, -0.7), "negative g peaks backward");
    }

    #[test]
    fn rayleigh_is_symmetric() {
        for &c in &[0.2, 0.5, 0.9] {
            assert!(approx(rayleigh_phase(c), rayleigh_phase(-c), 1e-6), "symmetric");
        }
        assert!(approx(rayleigh_phase(0.0), 3.0 / (4.0 * FOUR_PI), 1e-6));
    }

    #[test]
    fn phase_functions_integrate_to_unity() {
        // numerically integrate p(cosθ) over the sphere: ∫ p dω = 2π ∫_{-1}^{1} p dμ
        let integ = |f: &dyn Fn(f32) -> f32| {
            let n = 20000;
            let mut s = 0.0;
            for i in 0..n {
                let mu = -1.0 + 2.0 * (i as f32 + 0.5) / n as f32;
                s += f(mu) * (2.0 / n as f32);
            }
            s * 2.0 * std::f32::consts::PI
        };
        assert!(approx(integ(&|m| hg_phase(m, 0.6)), 1.0, 2e-2), "HG normalized");
        assert!(approx(integ(&|m| rayleigh_phase(m)), 1.0, 2e-2), "Rayleigh normalized");
        assert!(approx(integ(&|m| cornette_shanks_phase(m, 0.5)), 1.0, 3e-2), "CS ~normalized");
    }

    #[test]
    fn multi_scatter_brightens_and_converges() {
        let single = hg_phase(0.3, 0.6);
        let multi = multi_scatter_hg(0.3, 0.6, 0.5, 0.7, 4);
        assert!(multi > single, "adds scattered-in light");
        assert!(multi.is_finite() && multi < single * 3.0, "series converges quickly");
    }

    #[test]
    fn water_absorption_shifts_blue_green() {
        // red is absorbed fastest → lowest transmittance at depth
        let d = 5.0;
        let tr = transmittance(WATER_ABSORB_RGB[0] + WATER_SCATTER, d);
        let tg = transmittance(WATER_ABSORB_RGB[1] + WATER_SCATTER, d);
        let tb = transmittance(WATER_ABSORB_RGB[2] + WATER_SCATTER, d);
        assert!(tr < tg && tg < tb, "red gone first, blue-green survives");
    }

    #[test]
    fn shader_fog_density_matches_frag_curve() {
        // reproduces chunk.frag: base * mix(0.6, 2.2, humidity^1.6)
        let base = 0.008;
        assert!(approx(shader_fog_density(base, 0.0), base * 0.6, 1e-6), "dry → 0.6x");
        assert!(approx(shader_fog_density(base, 1.0), base * 2.2, 1e-6), "saturated → 2.2x");
        // humid air is foggier than dry air → shorter sight
        let dry = shader_fog_density(base, 0.2);
        let wet = shader_fog_density(base, 0.9);
        assert!(wet > dry);
        assert!(visibility_distance(wet, 0.05) < visibility_distance(dry, 0.05));
    }

    #[test]
    fn water_multiscatter_gain_grows_with_albedo() {
        let low = water_multiscatter_gain(single_scatter_albedo(0.07, 0.5));
        let high = water_multiscatter_gain(single_scatter_albedo(0.07, 0.1));
        assert!(high > low && low >= 0.0, "more scattering-dominated → more bounce light");
        assert!(apply_water_multiscatter(1.0, 0.07, 0.14) > 1.0, "brightens single scatter");
    }
}
