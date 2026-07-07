//! imprint.rs — turn a generated/attached image into a LOW-RES mosaic the 3D scene can render as geometry.
//!
//! The glow scene shader samples procedural triplanar textures by world position — there are no per-object
//! UVs and no image-texture path — so we don't paste a real texture onto the pearl or a node. Instead we
//! downsample the image to a small grid of average colours (the deliberately low-res "cool effect") and draw
//! each cell as a tiny glowing facet. The SAME grid drives the pearl imprint and image graph-nodes; full-
//! resolution display (hover, message thumbnails) is handled separately with egui textures.
//!
//! Everything here is pure and dependency-free (plain `[f32; 3]` math, RGBA slices), so it unit-tests without
//! the GPU, a window, or an image decoder — the decoder is a thin seam that hands us `(rgba, w, h)`.

pub type Rgb = [u8; 3];

/// Box-average downsample an RGBA8 buffer (`w*h*4` bytes, row-major; alpha ignored) to a `cols`×`rows` grid
/// of average RGB, row-major (grid row 0 = image top). Each output cell averages the source pixels in its
/// region; a grid finer than the source falls back to nearest-pixel. Empty/mismatched input → all black.
pub fn downsample(rgba: &[u8], w: usize, h: usize, cols: usize, rows: usize) -> Vec<Rgb> {
    let cols = cols.max(1);
    let rows = rows.max(1);
    let mut out = vec![[0u8; 3]; cols * rows];
    if w == 0 || h == 0 || rgba.len() < w * h * 4 {
        return out;
    }
    for gy in 0..rows {
        let y0 = gy * h / rows;
        let y1 = (((gy + 1) * h / rows).max(y0 + 1)).min(h);
        for gx in 0..cols {
            let x0 = gx * w / cols;
            let x1 = (((gx + 1) * w / cols).max(x0 + 1)).min(w);
            let (mut r, mut g, mut b, mut n) = (0u64, 0u64, 0u64, 0u64);
            for y in y0..y1 {
                for x in x0..x1 {
                    let i = (y * w + x) * 4;
                    r += rgba[i] as u64;
                    g += rgba[i + 1] as u64;
                    b += rgba[i + 2] as u64;
                    n += 1;
                }
            }
            if n > 0 {
                out[gy * cols + gx] = [(r / n) as u8, (g / n) as u8, (b / n) as u8];
            }
        }
    }
    out
}

/// The average colour of a grid — a cheap single swatch for a distant image node or a fallback tint.
pub fn average_color(grid: &[Rgb]) -> Rgb {
    if grid.is_empty() {
        return [0, 0, 0];
    }
    let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
    for c in grid {
        r += c[0] as u64;
        g += c[1] as u64;
        b += c[2] as u64;
    }
    let n = grid.len() as u64;
    [(r / n) as u8, (g / n) as u8, (b / n) as u8]
}

/// One imprint cell placed in 3D: a point on a surface + the colour to glow there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Facet {
    pub pos: [f32; 3],
    pub color: Rgb,
}

fn norm(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / l, v[1] / l, v[2] / l]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

/// Project a `cols`×`rows` colour grid onto a spherical CAP facing `dir` — the front of the pearl. Returns
/// one [`Facet`] per cell: its position on the sphere surface (distance `radius` from `center`) and colour.
/// `arc` is the FULL angular width of the cap in radians — how much of the pearl the imprint wraps (≈1.1 is
/// a bold front patch). Gnomonic-style: a flat grid pushed out onto the sphere, so it curves with the shell.
/// Grid row 0 maps to the TOP of the cap so the image isn't upside-down.
pub fn sphere_cap_facets(
    grid: &[Rgb],
    cols: usize,
    rows: usize,
    center: [f32; 3],
    radius: f32,
    dir: [f32; 3],
    up_hint: [f32; 3],
    arc: f32,
) -> Vec<Facet> {
    let cols = cols.max(1);
    let rows = rows.max(1);
    let fwd = norm(dir);
    let mut right = cross(up_hint, fwd);
    if right[0].abs() + right[1].abs() + right[2].abs() < 1e-4 {
        right = cross([1.0, 0.0, 0.0], fwd); // up_hint parallel to dir → pick any perpendicular
    }
    let right = norm(right);
    let up = cross(fwd, right); // unit (fwd ⟂ right)
    let half = (arc * 0.5).tan();
    let mut facets = Vec::with_capacity(cols * rows);
    for r in 0..rows {
        for c in 0..cols {
            let u = ((c as f32 + 0.5) / cols as f32 - 0.5) * 2.0 * half; // [-half, half]
            let v = (0.5 - (r as f32 + 0.5) / rows as f32) * 2.0 * half; // row 0 → +up (top)
            let d = norm([
                fwd[0] + right[0] * u + up[0] * v,
                fwd[1] + right[1] * u + up[1] * v,
                fwd[2] + right[2] * u + up[2] * v,
            ]);
            let color = grid.get(r * cols + c).copied().unwrap_or([0, 0, 0]);
            facets.push(Facet {
                pos: [center[0] + d[0] * radius, center[1] + d[1] * radius, center[2] + d[2] * radius],
                color,
            });
        }
    }
    facets
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, c: Rgb) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 4);
        for _ in 0..w * h {
            v.extend_from_slice(&[c[0], c[1], c[2], 255]);
        }
        v
    }

    #[test]
    fn downsample_solid_is_uniform() {
        let img = solid(16, 16, [200, 100, 50]);
        let g = downsample(&img, 16, 16, 4, 4);
        assert_eq!(g.len(), 16);
        assert!(g.iter().all(|&c| c == [200, 100, 50]));
    }

    #[test]
    fn downsample_averages_two_halves() {
        // left half red, right half blue, 4 wide → a 2×1 grid should read red then blue
        let (w, h) = (4usize, 2usize);
        let mut img = vec![0u8; w * h * 4];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 4;
                let c = if x < w / 2 { [255, 0, 0] } else { [0, 0, 255] };
                img[i] = c[0];
                img[i + 1] = c[1];
                img[i + 2] = c[2];
                img[i + 3] = 255;
            }
        }
        let g = downsample(&img, w, h, 2, 1);
        assert_eq!(g, vec![[255, 0, 0], [0, 0, 255]]);
    }

    #[test]
    fn downsample_rejects_bad_input() {
        assert!(downsample(&[], 0, 0, 3, 3).iter().all(|&c| c == [0, 0, 0]));
        assert!(downsample(&[1, 2, 3], 10, 10, 2, 2).iter().all(|&c| c == [0, 0, 0])); // buffer too small
    }

    #[test]
    fn average_color_is_the_mean() {
        assert_eq!(average_color(&[[100, 0, 0], [0, 100, 0], [0, 0, 100], [100, 100, 100]]), [50, 50, 50]);
        assert_eq!(average_color(&[]), [0, 0, 0]);
    }

    #[test]
    fn cap_facets_sit_on_the_sphere_and_face_forward() {
        let grid = vec![[10, 20, 30]; 9];
        let center = [1.0, 2.0, 3.0];
        let radius = 0.5;
        let dir = [0.0, 0.0, 1.0];
        let f = sphere_cap_facets(&grid, 3, 3, center, radius, dir, [0.0, 1.0, 0.0], 1.0);
        assert_eq!(f.len(), 9);
        for facet in &f {
            let d = [facet.pos[0] - center[0], facet.pos[1] - center[1], facet.pos[2] - center[2]];
            let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            assert!((dist - radius).abs() < 1e-4, "facet must lie on the sphere surface");
            // front-facing: component along dir is positive
            assert!(d[2] > 0.0, "every facet is on the cap facing `dir`");
        }
        // the centre cell of a 3×3 grid sits dead-centre on the cap: center + dir*radius
        let mid = f[4].pos;
        assert!((mid[0] - 1.0).abs() < 1e-4 && (mid[1] - 2.0).abs() < 1e-4 && (mid[2] - 3.5).abs() < 1e-4);
    }

    #[test]
    fn cap_top_row_is_higher_than_bottom_row() {
        let grid = vec![[0, 0, 0]; 4];
        let f = sphere_cap_facets(&grid, 2, 2, [0.0, 0.0, 0.0], 1.0, [0.0, 0.0, 1.0], [0.0, 1.0, 0.0], 1.2);
        // row 0 (top) should have greater y than row 1 (bottom), so images aren't upside-down
        assert!(f[0].pos[1] > f[2].pos[1]);
        assert!(f[1].pos[1] > f[3].pos[1]);
    }
}
