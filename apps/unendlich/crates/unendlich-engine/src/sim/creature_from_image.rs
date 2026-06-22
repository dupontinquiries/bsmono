//! `sim::creature_from_image` — derive forge-creature attributes from a reference
//! image. Shared landing module for the C-threads (coordinate the surface here):
//!   * C1 — shape/proportions → body-plan genes        (adds `shape_from_image`)
//!   * C2 — palette/elements → element + forge palette  (adds `palette_from_image`)
//!   * C3 — surface detail (scales/glow) → this file's [`surface_from_image`]
//!
//! C3 scope (this contribution): read the image's *surface character* — local
//! contrast / edge density (scaly vs smooth) and bright spots (glow / eye /
//! bioluminescence placement) — and emit [`SurfaceTraits`] DATA the forge and the
//! material consume. No shaders here (that is the H-threads); this is pure
//! analysis: pixels in → traits out.
//!
//! Pure + deterministic + panic-free: no `crate::render`/GPU, no I/O, no clock,
//! no RNG, no `.unwrap()`/`.expect()` in non-test code. Release-safe (no
//! `#[cfg]`-gated helpers used in asserts).

/// One image-derived glow location, in normalized image coordinates.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GlowPoint {
    /// Horizontal position, `0.0` = left … `1.0` = right.
    pub u: f32,
    /// Vertical position, `0.0` = top … `1.0` = bottom.
    pub v: f32,
    /// Relative brightness of the spot above the glow threshold, `[0,1]`.
    pub strength: f32,
}

/// Surface character extracted from a reference image.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceTraits {
    /// Bright spots → where glow boxes / eyes / bioluminescence go (normalized).
    pub glow_points: Vec<GlowPoint>,
    /// Overall glow intensity scalar, `[0,1]` (0 when there are no bright spots).
    pub glow_strength: f32,
    /// Roughness-ish scalar `[0,1]` from edge density: low = smooth, high = scaly.
    pub roughness: f32,
}

impl SurfaceTraits {
    /// Neutral traits (no glow, mid roughness) — used for degenerate input.
    pub fn neutral() -> SurfaceTraits {
        SurfaceTraits { glow_points: Vec::new(), glow_strength: 0.0, roughness: 0.5 }
    }
}

/// Max distinct glow points returned (keeps placement "small + additive").
pub const MAX_GLOW: usize = 8;

const EDGE_THR: f32 = 0.15; // per-pixel forward-gradient magnitude that counts as an edge

#[inline]
fn lum_at(rgba: &[u8], w: usize, x: usize, y: usize) -> f32 {
    let i = (y * w + x) * 4;
    let r = rgba[i] as f32;
    let g = rgba[i + 1] as f32;
    let b = rgba[i + 2] as f32;
    (0.299 * r + 0.587 * g + 0.114 * b) / 255.0
}

/// Extract [`SurfaceTraits`] from an RGBA8 image (`rgba.len() >= w*h*4`,
/// row-major, 4 bytes/pixel). Degenerate input returns [`SurfaceTraits::neutral`].
pub fn surface_from_image(rgba: &[u8], w: usize, h: usize) -> SurfaceTraits {
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return SurfaceTraits::neutral();
    }

    // ── roughness from edge density (fraction of interior pixels that are edges) ──
    let mut edge_n = 0u32;
    let mut interior = 0u32;
    let mut lum_sum = 0.0f32;
    let mut lum_max = 0.0f32;
    for y in 0..h {
        for x in 0..w {
            let l = lum_at(rgba, w, x, y);
            lum_sum += l;
            if l > lum_max {
                lum_max = l;
            }
            if x + 1 < w && y + 1 < h {
                // forward differences detect high-frequency detail that central
                // differences alias away (e.g. a 1px checker → scales).
                let gx = lum_at(rgba, w, x + 1, y) - l;
                let gy = lum_at(rgba, w, x, y + 1) - l;
                if (gx * gx + gy * gy).sqrt() > EDGE_THR {
                    edge_n += 1;
                }
                interior += 1;
            }
        }
    }
    let mean_lum = lum_sum / (w * h) as f32;
    let roughness = if interior == 0 {
        0.5
    } else {
        (edge_n as f32 / interior as f32).clamp(0.0, 1.0)
    };

    // ── bright spots → glow points (threshold + greedy non-max suppression) ──
    let thr = (mean_lum + 0.45 * (lum_max - mean_lum)).clamp(0.40, 0.95);
    // collect candidates above threshold
    let mut cand: Vec<(f32, usize, usize)> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            let l = lum_at(rgba, w, x, y);
            if l >= thr {
                cand.push((l, x, y));
            }
        }
    }
    // brightest first; deterministic tie-break by position
    cand.sort_by(|a, b| b.0.total_cmp(&a.0).then((a.2, a.1).cmp(&(b.2, b.1))));

    let min_dist = 0.07 * (w.max(h) as f32);
    let min_dist2 = min_dist * min_dist;
    let denom = (1.0 - thr).max(1e-3);
    let (uw, vh) = (((w - 1).max(1)) as f32, ((h - 1).max(1)) as f32);

    let mut glow_points: Vec<GlowPoint> = Vec::new();
    let mut centers: Vec<(f32, f32)> = Vec::new(); // pixel-space accepted centers
    for &(l, x, y) in &cand {
        if glow_points.len() >= MAX_GLOW {
            break;
        }
        let (fx, fy) = (x as f32, y as f32);
        let mut ok = true;
        for &(cxp, cyp) in &centers {
            let dx = fx - cxp;
            let dy = fy - cyp;
            if dx * dx + dy * dy < min_dist2 {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }
        centers.push((fx, fy));
        glow_points.push(GlowPoint {
            u: (fx / uw).clamp(0.0, 1.0),
            v: (fy / vh).clamp(0.0, 1.0),
            strength: ((l - thr) / denom).clamp(0.0, 1.0),
        });
    }

    let glow_strength = if glow_points.is_empty() {
        0.0
    } else {
        let s: f32 = glow_points.iter().map(|p| p.strength).sum();
        (s / glow_points.len() as f32).clamp(0.0, 1.0)
    };

    SurfaceTraits { glow_points, glow_strength, roughness }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Build an RGBA buffer from a per-pixel grey closure.
    fn grey_img(w: usize, h: usize, f: impl Fn(usize, usize) -> u8) -> Vec<u8> {
        let mut v = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let g = f(x, y);
                let i = (y * w + x) * 4;
                v[i] = g;
                v[i + 1] = g;
                v[i + 2] = g;
                v[i + 3] = 255;
            }
        }
        v
    }

    fn near(p: &GlowPoint, u: f32, v: f32, tol: f32) -> bool {
        (p.u - u).abs() <= tol && (p.v - v).abs() <= tol
    }

    #[test]
    fn bright_spots_become_glow_points_near_those_spots() {
        let (w, h) = (64usize, 64usize);
        // dark field with two bright 5x5 squares
        let spots = [(16usize, 16usize), (48usize, 40usize)];
        let img = grey_img(w, h, |x, y| {
            for &(sx, sy) in &spots {
                if x.abs_diff(sx) <= 2 && y.abs_diff(sy) <= 2 {
                    return 255;
                }
            }
            30
        });
        let t = surface_from_image(&img, w, h);
        assert!(t.glow_points.len() >= 2, "found {} glow points", t.glow_points.len());
        for &(sx, sy) in &spots {
            let (eu, ev) = (sx as f32 / 63.0, sy as f32 / 63.0);
            assert!(t.glow_points.iter().any(|p| near(p, eu, ev, 0.10)), "no glow point near spot ({sx},{sy})");
        }
        assert!(t.glow_strength > 0.0);
    }

    #[test]
    fn flat_image_has_no_glow() {
        let (w, h) = (32usize, 32usize);
        let img = grey_img(w, h, |_, _| 90);
        let t = surface_from_image(&img, w, h);
        assert!(t.glow_points.is_empty(), "flat mid-grey should yield no glow, got {:?}", t.glow_points);
        assert_eq!(t.glow_strength, 0.0);
    }

    #[test]
    fn roughness_higher_for_scaly_than_smooth() {
        let (w, h) = (32usize, 32usize);
        let smooth = grey_img(w, h, |x, _| ((x as f32 / (w - 1) as f32) * 255.0) as u8); // ramp
        let scaly = grey_img(w, h, |x, y| if (x + y) % 2 == 0 { 20 } else { 235 }); // checker
        let rs = surface_from_image(&smooth, w, h).roughness;
        let rc = surface_from_image(&scaly, w, h).roughness;
        assert!(rc > rs + 0.2, "scaly {rc} should exceed smooth {rs} clearly");
        assert!((0.0..=1.0).contains(&rs) && (0.0..=1.0).contains(&rc));
    }

    #[test]
    fn outputs_in_range() {
        let (w, h) = (40usize, 24usize);
        let img = grey_img(w, h, |x, y| ((x * 7 + y * 13) % 256) as u8);
        let t = surface_from_image(&img, w, h);
        assert!((0.0..=1.0).contains(&t.roughness));
        assert!((0.0..=1.0).contains(&t.glow_strength));
        for p in &t.glow_points {
            assert!((0.0..=1.0).contains(&p.u) && (0.0..=1.0).contains(&p.v) && (0.0..=1.0).contains(&p.strength));
        }
        assert!(t.glow_points.len() <= MAX_GLOW);
    }

    #[test]
    fn many_spots_capped_and_separated() {
        let (w, h) = (96usize, 96usize);
        // a dense grid of bright dots → must cap at MAX_GLOW and stay separated
        let img = grey_img(w, h, |x, y| if x % 8 == 0 && y % 8 == 0 { 255 } else { 25 });
        let t = surface_from_image(&img, w, h);
        assert!(t.glow_points.len() <= MAX_GLOW, "capped at {MAX_GLOW}, got {}", t.glow_points.len());
        // pairwise separation (normalized) respected
        for i in 0..t.glow_points.len() {
            for j in (i + 1)..t.glow_points.len() {
                let (a, b) = (&t.glow_points[i], &t.glow_points[j]);
                let d = ((a.u - b.u).powi(2) + (a.v - b.v).powi(2)).sqrt();
                assert!(d > 0.05, "glow points {i},{j} too close: {d}");
            }
        }
    }

    #[test]
    fn deterministic() {
        let (w, h) = (48usize, 48usize);
        let img = grey_img(w, h, |x, y| if x.abs_diff(24) <= 3 && y.abs_diff(20) <= 3 { 250 } else { 40 });
        assert_eq!(surface_from_image(&img, w, h), surface_from_image(&img, w, h));
    }

    #[test]
    fn degenerate_input_is_neutral_and_safe() {
        assert_eq!(surface_from_image(&[], 0, 0), SurfaceTraits::neutral());
        assert_eq!(surface_from_image(&[0, 0, 0, 255], 10, 10), SurfaceTraits::neutral()); // buffer too short
    }
}
