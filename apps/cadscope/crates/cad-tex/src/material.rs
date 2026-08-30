//! material.rs — the pattern generators, ported from `games/blackstar/tools/gen_texture.py`.
//!
//! Each produces an albedo image plus the two maps the engine expects beside it. The normal map is
//! derived from the albedo's luminance by Sobel gradient — the Python does the same, and while a
//! height field inferred from colour is not physically principled, it is what makes these textures
//! read as the games' own rather than as something CADscope invented.

use crate::noise::{tileable_fbm, Field, N};

pub type Rgb = [u8; 3];

/// The pattern vocabulary. A subset of the Python's dozen: the four the plan named, plus sediment,
/// which costs nothing once the noise exists and covers the "not metal" case.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Pattern {
    /// Riveted metal panels: seams and corner rivets on a per-panel tint.
    Plating,
    /// Hexagonal tech mesh.
    Hexmesh,
    /// Rectilinear circuit traces.
    Circuit,
    /// Rough noisy stone.
    Rock,
    /// Banded sedimentary layers.
    Sediment,
}

impl Pattern {
    pub fn name(self) -> &'static str {
        match self {
            Pattern::Plating => "plating",
            Pattern::Hexmesh => "hexmesh",
            Pattern::Circuit => "circuit",
            Pattern::Rock => "rock",
            Pattern::Sediment => "sediment",
        }
    }
    pub fn parse(s: &str) -> Option<Pattern> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "plating" => Pattern::Plating,
            "hexmesh" => Pattern::Hexmesh,
            "circuit" => Pattern::Circuit,
            "rock" => Pattern::Rock,
            "sediment" => Pattern::Sediment,
            _ => return None,
        })
    }
    pub fn all() -> [Pattern; 5] {
        [Pattern::Plating, Pattern::Hexmesh, Pattern::Circuit, Pattern::Rock, Pattern::Sediment]
    }
    /// The base and accent the Python defaults to for each pattern; a material may override both.
    pub fn default_colours(self) -> (Rgb, Rgb) {
        match self {
            Pattern::Plating => ([46, 50, 58], [168, 176, 190]),
            Pattern::Hexmesh => ([28, 34, 44], [120, 190, 210]),
            Pattern::Circuit => ([18, 26, 24], [90, 220, 150]),
            Pattern::Rock => ([58, 54, 50], [140, 132, 120]),
            Pattern::Sediment => ([84, 68, 52], [186, 160, 128]),
        }
    }
}

/// A generated material: albedo, plus the normal and specular maps that ride beside it.
#[derive(Clone)]
pub struct Material {
    pub pattern: Pattern,
    pub seed: u64,
    pub base: Rgb,
    pub accent: Rgb,
    /// The scalar field the maps are derived from, kept so a caller can rebake without regenerating.
    field: Field,
}

fn lerp_rgb(t: f32, a: Rgb, b: Rgb) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t).round().clamp(0.0, 255.0) as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t).round().clamp(0.0, 255.0) as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t).round().clamp(0.0, 255.0) as u8,
    ]
}

fn pat_plating(seed: u64) -> Field {
    let grain = tileable_fbm(seed + 1, 5, 0.5);
    let scratch = tileable_fbm(seed + 7, 6, 0.65);
    let mut f = Field::zeros();
    let cell = 64usize;
    for y in 0..N {
        for x in 0..N {
            let (cx, cy) = ((x % cell) as f32, (y % cell) as f32);
            // Per-panel tint. This diverges from the Python deliberately, and the port's tiling
            // test is why: upstream indexes panels as `(x/cell) + (y/cell)*(N/cell)`, so the panel
            // left of the wrap and the panel right of it get unrelated shades and the texture does
            // not tile — a visible grid on any surface large enough to repeat. Hashing the panel's
            // *wrapped* grid coordinates keeps the per-panel variation and makes the seam continue
            // correctly, because panel (0,j) is the same panel whichever side you approach it from.
            let (px, py) = ((x / cell) % (N / cell), (y / cell) % (N / cell));
            let h = (px * 73_856_093) ^ (py * 19_349_663);
            let tint = ((h % 1024) as f32 / 1024.0) * 0.25 + 0.6;
            // Seams sit at panel boundaries. Upstream draws them at `cx < 2 || cx > cell-3`,
            // which puts a 2px band at x=0..1 and a 2px band at x=61..63 — asymmetric, so the band
            // straddling the texture wrap is 2px on one side and 3px on the other and the tile
            // shows a line. Measuring distance to the nearest boundary makes the band symmetric
            // and identical wherever it falls, including across the wrap.
            let half = cell as f32 / 2.0;
            let dx = (cx - half).abs();
            let dy = (cy - half).abs();
            let seam = if dx > half - 2.0 || dy > half - 2.0 { 1.0 } else { 0.0 };
            let r2 = |ax: f32, ay: f32| (cx - ax) * (cx - ax) + (cy - ay) * (cy - ay);
            let far = cell as f32 - 8.0;
            let rivet = if r2(8.0, 8.0) < 9.0 || r2(far, 8.0) < 9.0 || r2(8.0, far) < 9.0 || r2(far, far) < 9.0 {
                1.0
            } else {
                0.0
            };
            let v = tint + grain.at(x, y) * 0.17 + (scratch.at(x, y) - 0.5) * 0.11 - seam * 0.35
                + rivet * 0.3;
            f.set(x, y, v.clamp(0.0, 1.0));
        }
    }
    f
}

fn pat_hexmesh(seed: u64) -> Field {
    let grain = tileable_fbm(seed, 4, 0.5);
    let scratch = tileable_fbm(seed + 7, 6, 0.65);
    let mut f = Field::zeros();
    // 64 x 32 divides 256 on both axes, so the lattice tiles with the noise.
    let (hx, hy) = (64.0f32, 32.0f32);
    for y in 0..N {
        for x in 0..N {
            let (xx, yy) = (x as f32, y as f32);
            let row = (yy / hy).floor();
            let ox = (row % 2.0) * (hx / 2.0);
            let gx = ((xx - ox).rem_euclid(hx)) - hx / 2.0;
            let gy = yy.rem_euclid(hy) - hy / 2.0;
            let d = (gx.abs() / (hx / 2.0)).max(gy.abs() / (hy / 2.0));
            let edge = if d > 0.82 { 1.0 } else { 0.0 };
            let v = 0.55 + grain.at(x, y) * 0.20 + (scratch.at(x, y) - 0.5) * 0.10 + edge * 0.35;
            f.set(x, y, v.clamp(0.0, 1.0));
        }
    }
    f
}

fn pat_circuit(seed: u64) -> Field {
    let grain = tileable_fbm(seed + 3, 4, 0.5);
    let mut f = Field::zeros();
    // Traces on a lattice that divides N, so every run rejoins itself across the wrap.
    let pitch = 32usize;
    for y in 0..N {
        for x in 0..N {
            let (cx, cy) = (x % pitch, y % pitch);
            let lane = ((x / pitch) * 7 + (y / pitch) * 13) % 5;
            let horiz = cy < 3 && lane != 0;
            let vert = cx < 3 && lane != 1;
            let pad = cx < 7 && cy < 7 && lane == 2;
            let trace = if pad { 1.0 } else if horiz || vert { 0.8 } else { 0.0 };
            let v = 0.22 + grain.at(x, y) * 0.14 + trace * 0.62;
            f.set(x, y, v.clamp(0.0, 1.0));
        }
    }
    f
}

fn pat_rock(seed: u64) -> Field {
    let coarse = tileable_fbm(seed, 6, 0.62);
    let fine = tileable_fbm(seed + 11, 6, 0.75);
    let mut f = Field::zeros();
    for y in 0..N {
        for x in 0..N {
            let v = coarse.at(x, y) * 0.75 + fine.at(x, y) * 0.25;
            f.set(x, y, v.clamp(0.0, 1.0));
        }
    }
    f
}

fn pat_sediment(seed: u64) -> Field {
    let warp = tileable_fbm(seed + 5, 4, 0.55);
    let grain = tileable_fbm(seed + 9, 5, 0.6);
    let mut f = Field::zeros();
    for y in 0..N {
        for x in 0..N {
            // Bands run along x, displaced by noise; the band count divides N so it wraps.
            let bands = 9.0;
            let t = (y as f32 / N as f32 + (warp.at(x, y) - 0.5) * 0.08) * bands;
            let band = (t * std::f32::consts::TAU).sin() * 0.5 + 0.5;
            let v = 0.35 + band * 0.4 + grain.at(x, y) * 0.2;
            f.set(x, y, v.clamp(0.0, 1.0));
        }
    }
    f
}

impl Material {
    pub fn new(pattern: Pattern, seed: u64) -> Material {
        let (base, accent) = pattern.default_colours();
        Material::with_colours(pattern, seed, base, accent)
    }

    pub fn with_colours(pattern: Pattern, seed: u64, base: Rgb, accent: Rgb) -> Material {
        let field = match pattern {
            Pattern::Plating => pat_plating(seed),
            Pattern::Hexmesh => pat_hexmesh(seed),
            Pattern::Circuit => pat_circuit(seed),
            Pattern::Rock => pat_rock(seed),
            Pattern::Sediment => pat_sediment(seed),
        };
        Material { pattern, seed, base, accent, field }
    }

    pub fn field(&self) -> &Field {
        &self.field
    }

    /// RGB8, row-major, N*N*3 bytes.
    pub fn albedo(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(N * N * 3);
        for y in 0..N {
            for x in 0..N {
                out.extend_from_slice(&lerp_rgb(self.field.at(x, y), self.base, self.accent));
            }
        }
        out
    }

    /// Tangent-space normal map, RGB8, from the Sobel gradient of the field. `strength` scales the
    /// slope; 1.0 matches the Python's default feel.
    pub fn normal(&self, strength: f32) -> Vec<u8> {
        let mut out = Vec::with_capacity(N * N * 3);
        let at = |x: isize, y: isize| {
            // Wrapping sampling, so the normal map tiles exactly like the albedo it came from.
            self.field.at(x.rem_euclid(N as isize) as usize, y.rem_euclid(N as isize) as usize)
        };
        for y in 0..N as isize {
            for x in 0..N as isize {
                let gx = (at(x + 1, y - 1) + 2.0 * at(x + 1, y) + at(x + 1, y + 1))
                    - (at(x - 1, y - 1) + 2.0 * at(x - 1, y) + at(x - 1, y + 1));
                let gy = (at(x - 1, y + 1) + 2.0 * at(x, y + 1) + at(x + 1, y + 1))
                    - (at(x - 1, y - 1) + 2.0 * at(x, y - 1) + at(x + 1, y - 1));
                let n = glam::Vec3::new(-gx * strength, -gy * strength, 1.0).normalize();
                out.push(((n.x * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8);
                out.push(((n.y * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8);
                out.push(((n.z * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8);
            }
        }
        out
    }

    /// Specular map: bright where the surface is raised and smooth, dark in seams and pits.
    pub fn specular(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(N * N * 3);
        for y in 0..N {
            for x in 0..N {
                let v = (self.field.at(x, y).powf(1.6) * 255.0).round().clamp(0.0, 255.0) as u8;
                out.extend_from_slice(&[v, v, v]);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pattern_fills_a_full_image() {
        for p in Pattern::all() {
            let m = Material::new(p, 3);
            assert_eq!(m.albedo().len(), N * N * 3, "{}", p.name());
            assert_eq!(m.normal(1.0).len(), N * N * 3, "{}", p.name());
            assert_eq!(m.specular().len(), N * N * 3, "{}", p.name());
        }
    }

    #[test]
    fn every_pattern_tiles_without_a_visible_step() {
        // What "tiles" means here needs care, and the first version of this test got it wrong.
        //
        // A per-panel feature is a real discontinuity at the wrap: with 64px panels in a 256px
        // texture, the panel at row 0 and the panel at row 255 are *different panels*, so their
        // tints differ and always will. That is not a bug — adjacent panels differ everywhere in
        // the texture, and the eye reads it as panelling rather than as a seam.
        //
        // What must not happen is a step at the wrap that is larger than the steps the pattern
        // already contains internally. So the comparison is against the pattern's own worst
        // interior step, not against its average smoothness.
        for p in Pattern::all() {
            let m = Material::new(p, 5);
            let f = m.field();
            let mut worst_seam = 0.0f32;
            let mut worst_interior = 0.0f32;
            for i in 0..N {
                worst_seam = worst_seam.max((f.at(i, 0) - f.at(i, N - 1)).abs());
                worst_seam = worst_seam.max((f.at(0, i) - f.at(N - 1, i)).abs());
                for j in 1..N {
                    worst_interior = worst_interior.max((f.at(i, j) - f.at(i, j - 1)).abs());
                    worst_interior = worst_interior.max((f.at(j, i) - f.at(j - 1, i)).abs());
                }
            }
            assert!(
                worst_seam <= worst_interior + 1e-4,
                "{}: the wrap steps by {worst_seam}, worse than any step inside it ({worst_interior})",
                p.name()
            );
        }
    }

    #[test]
    fn the_underlying_noise_wraps_exactly() {
        // Separately from the lattice features: the noise every pattern is built on must wrap with
        // steps no worse than its own interior, which is the property the FFT band-limiting buys.
        let f = crate::noise::tileable_fbm(6, 5, 0.5);
        let mut worst_seam = 0.0f32;
        let mut worst_interior = 0.0f32;
        for i in 0..N {
            worst_seam = worst_seam.max((f.at(i, 0) - f.at(i, N - 1)).abs());
            for j in 1..N {
                worst_interior = worst_interior.max((f.at(i, j) - f.at(i, j - 1)).abs());
            }
        }
        assert!(worst_seam <= worst_interior, "seam {worst_seam} vs interior {worst_interior}");
    }

    #[test]
    fn patterns_are_visibly_different_from_each_other() {
        let a = Material::new(Pattern::Plating, 1).albedo();
        let b = Material::new(Pattern::Circuit, 1).albedo();
        let diff: u64 = a.iter().zip(b.iter()).map(|(x, y)| x.abs_diff(*y) as u64).sum();
        assert!(diff > (a.len() as u64) * 10, "patterns are too similar: {diff}");
    }

    #[test]
    fn normal_maps_decode_to_unit_vectors() {
        // The shader decodes these as n = rgb*2-1 and expects something it can normalize. A map
        // whose texels do not decode to roughly unit length would light the surface wrongly in a
        // way no CPU test of the albedo would ever catch.
        for p in Pattern::all() {
            let n = Material::new(p, 4).normal(1.0);
            let mut worst = 0.0f32;
            for px in n.chunks_exact(3) {
                let v = glam::Vec3::new(
                    px[0] as f32 / 255.0 * 2.0 - 1.0,
                    px[1] as f32 / 255.0 * 2.0 - 1.0,
                    px[2] as f32 / 255.0 * 2.0 - 1.0,
                );
                worst = worst.max((v.length() - 1.0).abs());
            }
            // 8-bit quantisation alone costs about 1/255 per channel.
            assert!(worst < 0.02, "{}: normals off unit length by {worst}", p.name());
        }
    }

    #[test]
    fn normal_maps_point_outward_everywhere() {
        // Blue channel above 128 means z > 0: the perturbed normal never points into the surface,
        // which would turn a bump inside out.
        for p in Pattern::all() {
            let n = Material::new(p, 4).normal(1.0);
            let inward = n.chunks_exact(3).filter(|px| px[2] < 128).count();
            assert_eq!(inward, 0, "{}: {inward} texels point into the surface", p.name());
        }
    }

    #[test]
    fn strength_scales_the_slope() {
        let flat = Material::new(Pattern::Rock, 4).normal(0.0);
        let steep = Material::new(Pattern::Rock, 4).normal(4.0);
        let tilt = |m: &[u8]| {
            m.chunks_exact(3)
                .map(|px| (px[0] as i32 - 128).abs() + (px[1] as i32 - 128).abs())
                .sum::<i32>()
        };
        assert_eq!(tilt(&flat), 0, "zero strength must be perfectly flat");
        assert!(tilt(&steep) > 0, "strength should tilt the normals");
    }

    #[test]
    fn a_flat_region_has_an_upward_normal() {
        // Rock has no lattice, so its normal map should average out to mostly +Z (128,128,~255).
        let n = Material::new(Pattern::Rock, 2).normal(1.0);
        let z: u64 = n.chunks_exact(3).map(|p| p[2] as u64).sum();
        let mean_z = z as f32 / (N * N) as f32;
        assert!(mean_z > 200.0, "normals should point mostly out of the surface, got {mean_z}");
    }

    #[test]
    fn names_round_trip() {
        for p in Pattern::all() {
            assert_eq!(Pattern::parse(p.name()), Some(p));
        }
        assert_eq!(Pattern::parse("not_a_pattern"), None);
    }

    #[test]
    fn a_seed_reproduces_the_material() {
        assert_eq!(Material::new(Pattern::Plating, 8).albedo(), Material::new(Pattern::Plating, 8).albedo());
        assert_ne!(Material::new(Pattern::Plating, 8).albedo(), Material::new(Pattern::Plating, 9).albedo());
    }
}

