//! `sim::water_tint` (r459) — water surface colour from environmental fields.
//!
//! Shifts the base water albedo by salinity (turquoise/teal), temperature (warm green vs cold blue),
//! suspended sediment from the erosion field (murky brown-green), and depth (darkening). The render
//! shader `chunk.frag` mirrors this same formula on the water fragment. Pure + tested.

#[inline]
fn clamp01(x: f32) -> f32 { x.max(0.0).min(1.0) }
#[inline]
fn mix(a: f32, b: f32, t: f32) -> f32 { a + (b - a) * t }

/// Water albedo from `base` colour given depth (blocks), temperature, salinity and erosion (all the
/// climate inputs `0..1` except depth).
pub fn water_albedo(
    base: [f32; 3],
    depth_blocks: f32,
    temp01: f32,
    salinity01: f32,
    erosion01: f32,
) -> [f32; 3] {
    let t = clamp01(temp01);
    let s = clamp01(salinity01);
    let e = clamp01(erosion01);
    let mut c = base;
    c[1] += s * 0.10; // salinity → teal
    c[2] += s * 0.06;
    c[1] += (t - 0.5) * 0.08; // warm greener
    c[2] += (0.5 - t) * 0.10; // cold bluer
    let murk = [0.34, 0.33, 0.22];
    c[0] = mix(c[0], murk[0], e * 0.5);
    c[1] = mix(c[1], murk[1], e * 0.4);
    c[2] = mix(c[2], murk[2], e * 0.5);
    let d = clamp01(depth_blocks.max(0.0) / 12.0);
    let dark = 1.0 - d * 0.5;
    [clamp01(c[0] * dark), clamp01(c[1] * dark), clamp01(c[2] * dark)]
}

#[cfg(test)]
mod tests {
    use super::*;
    const BASE: [f32; 3] = [0.20, 0.40, 0.85];
    #[test] fn salty_more_teal() {
        let salty = water_albedo(BASE, 1.0, 0.5, 1.0, 0.0);
        let fresh = water_albedo(BASE, 1.0, 0.5, 0.0, 0.0);
        assert!(salty[1] > fresh[1] && salty[2] > fresh[2]);
    }
    #[test] fn cold_bluer_warm_greener() {
        let cold = water_albedo(BASE, 1.0, 0.0, 0.3, 0.0);
        let warm = water_albedo(BASE, 1.0, 1.0, 0.3, 0.0);
        assert!(cold[2] > warm[2] && warm[1] > cold[1]);
    }
    #[test] fn erosion_murky() {
        let clear = water_albedo(BASE, 1.0, 0.5, 0.3, 0.0);
        let murky = water_albedo(BASE, 1.0, 0.5, 0.3, 1.0);
        assert!(murky[0] > clear[0] && murky[2] < clear[2]);
    }
    #[test] fn deeper_darker() {
        assert!(water_albedo(BASE, 12.0, 0.5, 0.3, 0.0)[2] < water_albedo(BASE, 0.0, 0.5, 0.3, 0.0)[2]);
    }
}
