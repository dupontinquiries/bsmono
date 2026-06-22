//! texgen.rs — procedural, seamless, TILING material textures baked on the CPU (no asset files), uploaded
//! once as a GL 2D texture array and sampled triplanar in the scene shader. Same value-noise/fBm family as
//! `theme.rs`, but with a wrapped integer lattice so each tile's edges match (no seams when repeated across
//! a mesh in world space).
//!
//! Layers (must match the `mat::*` ids the shader/`Mesh3.mat` use):
//!   0 paper  1 wood/bark  2 stone  3 water  4 moss/foliage

pub const TEX_SIZE: usize = 128;
pub const LAYERS: usize = 5;

/// Material ids, as `f32` (carried per-vertex in `Mesh3.mat`, read as a texture-array layer in the shader).
pub mod mat {
    pub const PAPER: f32 = 0.0;
    pub const WOOD: f32 = 1.0;
    pub const STONE: f32 = 2.0;
    pub const WATER: f32 = 3.0;
    pub const MOSS: f32 = 4.0;
}

// value noise on a lattice that WRAPS at `period` cells → the tile is seamless when repeated.
#[inline] fn hashp(xi: i32, yi: i32, period: i32, seed: u32) -> f32 {
    let x = xi.rem_euclid(period.max(1)) as u32;
    let y = yi.rem_euclid(period.max(1)) as u32;
    let mut h = x.wrapping_mul(374_761_393).wrapping_add(y.wrapping_mul(668_265_263)).wrapping_add(seed.wrapping_mul(362_437));
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) & 0xFF_FFFF) as f32 / 0xFF_FFFF as f32
}
#[inline] fn smooth(t: f32) -> f32 { t * t * (3.0 - 2.0 * t) }
fn vnoise(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let (x0, y0) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (smooth(x - x0 as f32), smooth(y - y0 as f32));
    let a = hashp(x0, y0, period, seed);
    let b = hashp(x0 + 1, y0, period, seed);
    let c = hashp(x0, y0 + 1, period, seed);
    let d = hashp(x0 + 1, y0 + 1, period, seed);
    let ab = a + (b - a) * fx;
    let cd = c + (d - c) * fx;
    ab + (cd - ab) * fy
}
/// fBm whose octaves all tile within uv∈[0,1): octave o uses lattice period `cells·2^o`.
fn fbm(u: f32, v: f32, cells: i32, oct: u32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut p, mut norm) = (0.0f32, 0.5f32, 1.0f32, cells, 0.0f32);
    for o in 0..oct {
        sum += vnoise(u * cells as f32 * freq, v * cells as f32 * freq, p, seed + o) * amp;
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
        p *= 2;
    }
    sum / norm.max(1e-4)
}
#[inline] fn lerp3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
#[inline] fn px(out: &mut [u8], i: usize, c: [f32; 3]) {
    out[i] = (c[0].clamp(0.0, 1.0) * 255.0) as u8;
    out[i + 1] = (c[1].clamp(0.0, 1.0) * 255.0) as u8;
    out[i + 2] = (c[2].clamp(0.0, 1.0) * 255.0) as u8;
    out[i + 3] = 255;
}

/// Each material's scalar height field in [0,1] (drives both the albedo lightness and, via its gradient,
/// the normal map — so bumps line up with the texture). Uses the wrapped lattice, so it tiles.
fn height(layer: usize, u: f32, v: f32) -> f32 {
    match layer {
        0 => fbm(u, v, 6, 4, 11), // paper
        1 => { // wood/bark: vertical streaks + rings
            let streak = fbm(u * 3.0, v * 0.6, 8, 5, 21);
            let rings = (fbm(u, v, 4, 3, 23) * 7.0 + streak * 2.0).sin() * 0.5 + 0.5;
            (streak * 0.6 + rings * 0.4).clamp(0.0, 1.0)
        }
        2 => { // stone
            let base = fbm(u, v, 5, 5, 31);
            let grain = fbm(u, v, 16, 3, 37);
            (base * 0.75 + grain * 0.25).clamp(0.0, 1.0)
        }
        3 => { // water
            let r = fbm(u * 2.0, v * 2.0, 6, 4, 41);
            let weave = ((u * 6.2831 * 3.0).sin() + (v * 6.2831 * 2.0).cos()) * 0.06;
            (r * 0.5 + 0.4 + weave).clamp(0.0, 1.0)
        }
        _ => { // moss/foliage
            let base = fbm(u, v, 7, 5, 51);
            let speck = if fbm(u, v, 24, 2, 57) > 0.62 { -0.18 } else { 0.0 };
            (base + speck).clamp(0.0, 1.0)
        }
    }
}
/// Per-material relief strength (how much the height gradient tilts the normal) and base roughness (low =
/// shiny: water/wet stone glint; high = matte: bark/moss).
fn bump(layer: usize) -> f32 { match layer { 1 => 2.2, 2 => 2.0, 3 => 1.2, 4 => 1.4, _ => 0.8 } }
fn roughness(layer: usize) -> f32 { match layer { 3 => 0.12, 2 => 0.45, 1 => 0.8, 4 => 0.9, _ => 0.7 } }

/// One material albedo tile → RGBA8, row-major, `TEX_SIZE²·4` bytes (colour from the height field).
fn bake_one(layer: usize, n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n * n * 4];
    for y in 0..n {
        for x in 0..n {
            let (u, v) = (x as f32 / n as f32, y as f32 / n as f32);
            let i = (y * n + x) * 4;
            let t = height(layer, u, v);
            let c = match layer {
                0 => lerp3([0.86, 0.83, 0.76], [0.97, 0.95, 0.90], t),
                1 => lerp3([0.30, 0.20, 0.11], [0.55, 0.40, 0.24], t),
                2 => lerp3([0.34, 0.34, 0.37], [0.62, 0.63, 0.66], t),
                3 => lerp3([0.06, 0.16, 0.19], [0.16, 0.34, 0.38], t),
                _ => lerp3([0.18, 0.30, 0.12], [0.46, 0.58, 0.22], t),
            };
            px(&mut out, i, c);
        }
    }
    out
}

/// One material normal+roughness tile → RGBA8: RGB = tangent-space normal (from the height gradient,
/// encoded `n*0.5+0.5`), A = roughness. Tileable (the gradient wraps with the lattice).
fn bake_normal_one(layer: usize, n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n * n * 4];
    let e = 1.0 / n as f32;
    let (s, r) = (bump(layer), roughness(layer));
    for y in 0..n {
        for x in 0..n {
            let (u, v) = (x as f32 / n as f32, y as f32 / n as f32);
            let i = (y * n + x) * 4;
            let dx = height(layer, u + e, v) - height(layer, u - e, v);
            let dy = height(layer, u, v + e) - height(layer, u, v - e);
            let (nx, ny, nz) = (-dx * s, -dy * s, 1.0);
            let inv = 1.0 / (nx * nx + ny * ny + nz * nz).sqrt();
            out[i] = ((nx * inv * 0.5 + 0.5) * 255.0) as u8;
            out[i + 1] = ((ny * inv * 0.5 + 0.5) * 255.0) as u8;
            out[i + 2] = ((nz * inv * 0.5 + 0.5) * 255.0) as u8;
            out[i + 3] = (r.clamp(0.0, 1.0) * 255.0) as u8;
        }
    }
    out
}

/// All material normal+roughness layers concatenated (layer-major) for a second `TEXTURE_2D_ARRAY`.
pub fn bake_normals(n: usize) -> Vec<u8> {
    let mut all = Vec::with_capacity(LAYERS * n * n * 4);
    for l in 0..LAYERS { all.extend_from_slice(&bake_normal_one(l, n)); }
    all
}

/// All material layers concatenated (layer-major) for a `TEXTURE_2D_ARRAY` upload: `LAYERS·TEX_SIZE²·4` bytes.
pub fn bake_layers(n: usize) -> Vec<u8> {
    let mut all = Vec::with_capacity(LAYERS * n * n * 4);
    for l in 0..LAYERS { all.extend_from_slice(&bake_one(l, n)); }
    all
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bakes_all_layers_sized_and_opaque() {
        let data = bake_layers(TEX_SIZE);
        assert_eq!(data.len(), LAYERS * TEX_SIZE * TEX_SIZE * 4);
        assert!(data.chunks(4).all(|p| p[3] == 255), "all texels opaque");
        // deterministic
        assert_eq!(data, bake_layers(TEX_SIZE));
    }

    #[test]
    fn normal_maps_point_up_and_tile() {
        for l in 0..LAYERS {
            let t = bake_normal_one(l, TEX_SIZE);
            let n = TEX_SIZE;
            // average normal should point up (+Z): blue channel is the largest on average
            let (mut r, mut g, mut b) = (0u64, 0u64, 0u64);
            for p in t.chunks(4) { r += p[0] as u64; g += p[1] as u64; b += p[2] as u64; }
            let np = (n * n) as u64;
            assert!(b / np > r / np && b / np > g / np, "layer {l} normals face up: r={} g={} b={}", r / np, g / np, b / np);
            // tileable: left edge ≈ right edge
            let mut max_edge = 0i32;
            for y in 0..n {
                for ch in 0..3 {
                    max_edge = max_edge.max((t[(y * n) * 4 + ch] as i32 - t[(y * n + n - 1) * 4 + ch] as i32).abs());
                }
            }
            assert!(max_edge < 70, "layer {l} normal seam too large: {max_edge}");
        }
    }

    #[test]
    fn tiles_are_seamless() {
        // left edge ≈ right edge, top edge ≈ bottom edge (wrapped lattice) for every layer
        for l in 0..LAYERS {
            let t = bake_one(l, TEX_SIZE);
            let n = TEX_SIZE;
            let mut max_edge = 0i32;
            for y in 0..n {
                let left = (y * n) * 4;
                let right = (y * n + (n - 1)) * 4;
                for c in 0..3 { max_edge = max_edge.max((t[left + c] as i32 - t[right + c] as i32).abs()); }
            }
            for x in 0..n {
                let top = x * 4;
                let bot = ((n - 1) * n + x) * 4;
                for c in 0..3 { max_edge = max_edge.max((t[top + c] as i32 - t[bot + c] as i32).abs()); }
            }
            // edges won't be identical (one lattice cell apart) but must be close → no visible seam
            assert!(max_edge < 60, "layer {l} edge discontinuity too large: {max_edge}");
        }
    }
}

#[cfg(test)]
mod resolution_tests {
    use super::*;
    #[test]
    fn bakes_at_any_resolution() {
        for n in [64usize, 128, 256, 512] {
            assert_eq!(bake_layers(n).len(), LAYERS * n * n * 4, "albedo array sized for {n}px");
            assert_eq!(bake_normals(n).len(), LAYERS * n * n * 4, "normal array sized for {n}px");
        }
        // higher resolution is the same pattern at finer sampling: corner texel matches across sizes
        let a = bake_one(2, 128); let b = bake_one(2, 256); // stone layer, top-left texel (u=v=0)
        assert_eq!(&a[0..4], &b[0..4], "the (0,0) texel is identical regardless of resolution");
    }
}
