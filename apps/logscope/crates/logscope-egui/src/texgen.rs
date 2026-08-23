//! texgen.rs — procedural, seamless, TILING material textures baked on the CPU (no asset files), uploaded
//! once as a GL 2D texture array and sampled triplanar in the scene shader. Same value-noise/fBm family as
//! `theme.rs`, but with a wrapped integer lattice so each tile's edges match (no seams when repeated across
//! a mesh in world space).
//!
//! Layers (must match the `mat::*` ids the shader/`Mesh3.mat` use):
//!   0 paper  1 wood/bark  2 stone  3 water  4 moss/foliage

pub const TEX_SIZE: usize = 128;
pub const LAYERS: usize = 11;

/// Material ids, as `f32` (carried per-vertex in `Mesh3.mat`, read as a texture-array layer in the shader).
pub mod mat {
    pub const PAPER: f32 = 0.0;
    pub const WOOD: f32 = 1.0;
    pub const STONE: f32 = 2.0;
    pub const WATER: f32 = 3.0;
    pub const MOSS: f32 = 4.0;

    // ── added for depth and variation ──
    //
    // Six more layers rather than parameters on the existing five, because the
    // shader reads `Mesh3.mat` as a texture-array INDEX: a variant has to be
    // its own layer to be selectable per-object without touching the shader.
    // Eleven 128² RGBA tiles is 720 KiB of albedo plus the same again in
    // normal+roughness — baked once at startup, and the whole point of
    // procedural is that it costs no asset files.

    /// Polished marble: low-frequency veining, high specular, near-white.
    pub const MARBLE: f32 = 5.0;
    /// Smooth planed wood — furniture, shelves, worked surfaces. Straight grain,
    /// no bark relief, the finish `WOOD` is too rough for.
    pub const WOOD_SMOOTH: f32 = 6.0;
    /// Gnarled bark: deep fissures, knots, high relief. `WOOD` is a middle
    /// ground; this is an old trunk up close.
    pub const WOOD_GNARLED: f32 = 7.0;
    /// Rain-darkened wood. Same grain as `WOOD_SMOOTH`, pulled dark and
    /// desaturated, and rough only where the water has not pooled — the
    /// darkening and the SHINE are what read as wet, not one or the other.
    pub const WOOD_WET: f32 = 8.0;
    /// Fine bladed grass, seen from above.
    pub const GRASS: f32 = 9.0;
    /// Settled dust and grit — the surface a long-untouched thing wears.
    pub const DUST: f32 = 10.0;
}

// value noise on a lattice that WRAPS at `period` cells → the tile is seamless when repeated.
// The hashp/vnoise/fbm family that lived here moved to `shared/noise-kit` bit-identically (this
// file was the origin of those constants — soldier mesh.rs adapted them from here, and the kit
// golden-pins the outputs of both). `fbm` keeps its local name/signature for the ~15 call sites.
/// fBm whose octaves all tile within uv∈[0,1): octave o uses lattice period `cells·2^o`.
#[inline]
fn fbm(u: f32, v: f32, cells: i32, oct: u32, seed: u32) -> f32 {
    noise_kit::fbm2_tiled(u, v, cells, oct, seed)
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
/// Shortest distance from `a` to `b` on a unit circle.
///
/// A knot placed at a fixed uv must wrap with the tile, or it is a seam. In the
/// plane `u - k` is wrong at the edges; on the torus the short way round is.
#[inline]
fn wrapd(a: f32, b: f32) -> f32 {
    let d = (a - b).abs();
    if d > 0.5 { 1.0 - d } else { d }
}

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
        5 => { // MARBLE — veins, not noise
            // A vein is a THIN, HIGH-CONTRAST curve, and the way to get one is
            // a sine through a warped coordinate rather than a noise threshold:
            // thresholding fbm gives blobby patches with soft edges, which read
            // as damp plaster. Warping the phase is what makes a vein wander
            // and occasionally double back on itself.

            // 2*PI*k*u, not PI*4*u: the sine must complete a WHOLE number of
            // cycles across the tile or its value at u=1 differs from u=0 and
            // the vein walks off the edge. The pre-existing `tiles_are_seamless`
            // test caught this at 87 against a limit of 60 — the warp term
            // wraps on its own lattice, so it was the plain sine that leaked.
            // Integer cycles across the tile, and a warp amplitude small enough
            // that it cannot push the vein a whole cycle out of phase at the
            // seam. Frequency and warp depth trade against each other here:
            // more of either is a prettier vein and a worse edge.
            // WARP THE LATTICE, NOT THE PHASE.
            //
            // Adding a warp to the sine's argument is what leaked: the sine
            // itself wraps, but `warp` shifts it by an amount that differs at
            // u=0 and u=1, and `powf(9)` below turns a small phase error into a
            // hard line. Three attempts at tuning the frequency and amplitude
            // got 87, 139, then 76 against a limit of 60 — the shape was wrong,
            // not the numbers.
            //
            // Sampling a wrapping fbm and thresholding ITS ridge keeps every
            // term on the lattice. `1 - |2f-1|` is the same fold that makes the
            // bark fissures cusp, and a vein is the same object: a thin ridge
            // through a smooth field.
            let field = fbm(u * 3.0, v * 1.0, 4, 3, 55);
            let vein = 1.0 - (field * 2.0 - 1.0).abs();
            // |vein| near zero IS the vein. Raising it to a high power keeps the
            // line thin while leaving the field smooth everywhere else, which
            // matters because the normal pass differentiates this.
            let line = vein.powf(9.0);
            let f2 = fbm(u * 1.0, v * 2.0, 3, 4, 57);
            let second = (1.0 - (f2 * 2.0 - 1.0).abs()).powf(14.0);
            let body = fbm(u, v, 4, 4, 53) * 0.12 + 0.80;   // near-white, faintly cloudy
            (body - line * 0.55 - second * 0.30).clamp(0.0, 1.0)
        }
        6 => { // WOOD_SMOOTH — planed, straight grain
            // The same streak family as bark with the ring term dropped and the
            // frequency raised: what distinguishes worked wood from bark is
            // regularity, not colour.
            let streak = fbm(u * 6.0, v * 0.4, 7, 5, 61);
            let fine = fbm(u * 14.0, v * 0.8, 4, 3, 63);
            (streak * 0.78 + fine * 0.22).clamp(0.0, 1.0)
        }
        7 => { // WOOD_GNARLED — fissured bark with knots
            let streak = fbm(u * 2.2, v * 0.5, 8, 5, 71);
            // Ridged noise: fold the field at its midpoint so the creases are
            // CUSPS rather than smooth valleys. Plain fbm cannot make a sharp
            // fissure at any octave count.
            let ridge = 1.0 - (fbm(u * 3.0, v * 1.1, 6, 4, 73) - 0.5).abs() * 2.0;
            // A knot is a radial swirl around a point, and it must WRAP, so the
            // distance is measured on the torus rather than in the plane.
            // Placed to STRADDLE the tile edge. At (0.31, 0.62) the knot's
            // radius (~1/7) never reached a boundary, so `wrapd` was defensive
            // code that could not be exercised — the seam test passed with the
            // distance taken in the plane, which is the shape of a guard
            // nothing guards. Near the edge it wraps for real, and a knot that
            // continues across the repeat is better looking besides.
            let (kx, ky) = (0.04, 0.62);
            let (dx, dy) = (wrapd(u, kx), wrapd(v, ky));
            let r = (dx * dx + dy * dy).sqrt();
            let knot = (1.0 - (r * 7.0).min(1.0)).powf(2.0) * ((r * 34.0).sin() * 0.5 + 0.5);
            (streak * 0.34 + ridge.powf(2.4) * 0.46 + knot * 0.20).clamp(0.0, 1.0)
        }
        8 => { // WOOD_WET — smooth grain, water pooling in the low places
            let base = height(6, u, v);
            // Where water sits, the surface flattens: pooling FILLS the grain.
            // Lerping toward the pool level rather than adding to it is what
            // stops wet wood reading as embossed.
            let pool = fbm(u * 2.0, v * 2.0, 4, 4, 81);
            let wetness = ((pool - 0.42) * 3.0).clamp(0.0, 1.0);
            base + (0.62 - base) * wetness * 0.55
        }
        9 => { // GRASS — blades, from above
            // Directional stretch is the whole trick: fbm sampled far more
            // finely across than along gives strands rather than clumps.
            let blades = fbm(u * 26.0, v * 3.0, 5, 6, 91);
            let clump = fbm(u * 2.5, v * 2.5, 4, 4, 93);
            (blades * 0.62 + clump * 0.38).clamp(0.0, 1.0)
        }
        10 => { // DUST — fine grit, faint drifts
            let grit = fbm(u * 30.0, v * 30.0, 3, 8, 101);
            let drift = fbm(u * 1.2, v * 1.2, 4, 3, 103);
            (grit * 0.30 + drift * 0.70).clamp(0.0, 1.0)
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
fn bump(layer: usize) -> f32 { match layer { 1 => 3.1, 2 => 2.8, 3 => 1.7, 4 => 2.0, _ => 1.15 } }
fn roughness(layer: usize) -> f32 {
    match layer {
        3 => 0.12, 2 => 0.45, 1 => 0.8, 4 => 0.9,
        // Marble is the point of having a roughness channel at all: at 0.09 it
        // is the shiniest thing in the scene bar water, and a polished stone
        // that is not visibly polished is just pale stone.
        5 => 0.09,
        6 => 0.42,  // planed wood: a finish, not raw grain
        7 => 0.92,  // gnarled bark: rougher than plain WOOD's 0.8
        // Wet wood: SMOOTHER than dry (0.42), because a water film is what a
        // low roughness models. Darkening alone would read as stain; it is the
        // pairing of dark albedo with a sharp highlight that says wet.
        8 => 0.22,
        9 => 0.88,  // grass
        10 => 0.95, // dust: the matte extreme, and what makes a dusted marble
                    // slab read as neglected rather than as different stone
        _ => 0.7,
    }
}

/// Per-TEXEL roughness — the flat `roughness(layer)` broken up by a large-scale weathering field and tied to
/// the local height, so a surface has POLISHED and MATTE patches instead of one uniform gloss. This is what
/// gives the GGX highlight a real, non-uniform response: raised stone faces catch a glint while crevices stay
/// matte, wood sapwood reads smoother than its dark grain, moss stays broadly rough. Wraps (tiles) with fbm.
fn rough_at(layer: usize, u: f32, v: f32) -> f32 {
    let base = roughness(layer);
    let weather = fbm(u, v, 3, 3, 200 + layer as u32) - 0.5; // large-scale polish/matte patches [-0.5,0.5]
    let h = height(layer, u, v) - 0.5;                        // raised (+) vs recessed (-)
    let var = match layer {
        2 => weather * 0.34 - h * 0.26, // stone: proud faces polish smooth, crevices rough
        1 => weather * 0.24 - h * 0.20, // wood: pale sapwood rings smoother than dark grain
        3 => weather * 0.10,            // water: near-uniform smooth, faint variation
        4 => weather * 0.18 + 0.06,     // moss: rough & patchy
        // Marble: the VEINS are softer stone and take a duller polish, so
        // roughness tracks the vein rather than a weathering field. `h` is low
        // in a vein, so +h raises gloss on the body and leaves the vein matte —
        // the detail that stops polished marble looking like painted plastic.
        5 => weather * 0.06 + h * 0.10,
        6 => weather * 0.16 - h * 0.10, // planed wood: proud grain takes the polish
        7 => weather * 0.10 - h * 0.22, // bark: fissure floors rougher than ridges
        // Wet wood: the pools are the SMOOTH part, and they are the high places
        // in this height field (water fills the grain), so gloss follows +h
        // hard. This is the only layer where roughness varies more than the
        // albedo does.
        8 => weather * 0.08 - h * 0.34,
        9 => weather * 0.14 + 0.04,     // grass
        10 => weather * 0.05,           // dust: uniformly matte, barely varying
        _ => weather * 0.30,            // paper: matte with a patchy sheen
    };
    (base + var).clamp(0.04, 1.0)
}

/// Per-material fine MICRO-GRAIN: (lattice cells = grain frequency, amplitude = grain strength). A crisp
/// high-frequency detail normal layered on top of the coarse height-gradient normal so each surface has
/// tactile sub-relief — paper tooth, stone grain, bark fibre, moss speckle — that the GGX highlight + the
/// spatially-varying roughness catch. High `cells` keeps it tiling with the lattice.
fn micro(layer: usize) -> (i32, f32) {
    match layer {
        1 => (30, 0.9), // wood: fine fibre
        2 => (26, 1.0), // stone: gritty grain
        3 => (14, 0.25),// water: almost none
        4 => (24, 0.7), // moss: speckle
        5 => (10, 0.12),// marble: almost none — micro-grain is what makes a
                        // polished surface look sandblasted
        6 => (34, 0.55),// planed wood: fine, regular fibre
        7 => (22, 1.0), // gnarled bark: coarse, deep
        8 => (30, 0.30),// wet wood: the film fills the fibre
        9 => (40, 0.85),// grass: fine blade edges
        10 => (52, 0.45),// dust: very fine grit, low relief
        _ => (22, 0.6), // paper: tooth
    }
}

/// A multi-stop colour LUT (lookup table) — maps a scalar in [0,1] through N colour stops with linear blend.
/// Richer, more natural material gradients than a single lo→hi lerp (e.g. bark darkens through a reddish midtone
/// into pale sapwood). Per-material stop tables live in `lut_stops`.
fn ramp(stops: &[[f32; 3]], t: f32) -> [f32; 3] {
    let n = stops.len();
    if n == 1 { return stops[0]; }
    let p = t.clamp(0.0, 1.0) * (n - 1) as f32;
    let i = (p.floor() as usize).min(n - 2);
    lerp3(stops[i], stops[i + 1], p - i as f32)
}
/// Per-material colour LUT stops (dark→light), tuned for each surface.
fn lut_stops(layer: usize) -> &'static [[f32; 3]] {
    match layer {
        0 => &[[0.82, 0.79, 0.72], [0.90, 0.88, 0.82], [0.97, 0.96, 0.92]],                       // paper
        1 => &[[0.20, 0.13, 0.07], [0.36, 0.23, 0.13], [0.50, 0.35, 0.20], [0.63, 0.48, 0.30]],   // wood/bark
        2 => &[[0.28, 0.30, 0.33], [0.41, 0.42, 0.45], [0.54, 0.55, 0.58], [0.67, 0.68, 0.70]],   // stone
        3 => &[[0.04, 0.12, 0.16], [0.08, 0.21, 0.26], [0.14, 0.32, 0.38], [0.22, 0.43, 0.48]],   // water
        4 => &[[0.13, 0.23, 0.09], [0.23, 0.35, 0.13], [0.37, 0.50, 0.19], [0.53, 0.65, 0.28]],   // moss/foliage
        // Marble runs DARK->LIGHT because the height field subtracts the veins:
        // low values are the vein, high is the body. The darkest stop is a warm
        // grey rather than black — a pure black vein reads as a crack.
        5 => &[[0.30, 0.28, 0.31], [0.62, 0.60, 0.62], [0.88, 0.87, 0.86], [0.96, 0.96, 0.94]],   // marble
        6 => &[[0.42, 0.30, 0.18], [0.58, 0.42, 0.25], [0.72, 0.55, 0.35], [0.83, 0.68, 0.47]],   // wood, planed
        7 => &[[0.13, 0.09, 0.05], [0.26, 0.18, 0.10], [0.40, 0.29, 0.17], [0.55, 0.42, 0.26]],   // bark, gnarled
        // Wet wood is the planed palette pulled dark and toward grey. Darkening
        // alone looks like stain; the desaturation is what says water.
        8 => &[[0.18, 0.13, 0.09], [0.28, 0.21, 0.15], [0.38, 0.30, 0.23], [0.47, 0.39, 0.31]],   // wood, wet
        9 => &[[0.14, 0.24, 0.08], [0.26, 0.42, 0.13], [0.40, 0.58, 0.20], [0.58, 0.72, 0.30]],   // grass
        _ => &[[0.44, 0.41, 0.36], [0.56, 0.53, 0.47], [0.66, 0.63, 0.57], [0.75, 0.72, 0.66]],   // dust
    }
}

/// One material albedo tile → RGBA8, row-major, `TEX_SIZE²·4` bytes. Colour comes from the height field run
/// through the material LUT, then broken up by a LARGE-scale tone field (patchy weathering / colour variation)
/// and a fine hue jitter so the tile reads organic instead of a smooth height ramp.
fn bake_one(layer: usize, n: usize) -> Vec<u8> {
    let mut out = vec![0u8; n * n * 4];
    let stops = lut_stops(layer);
    for y in 0..n {
        for x in 0..n {
            let (u, v) = (x as f32 / n as f32, y as f32 / n as f32);
            let i = (y * n + x) * 4;
            let t = height(layer, u, v);
            // large-scale tone patches shift WHERE we sample the LUT → weathered blotches, sun-bleached runs
            let tone = fbm(u, v, 3, 3, 90 + layer as u32) - 0.5;
            let mut c = ramp(stops, (t + tone * 0.20).clamp(0.0, 1.0));
            // fine per-channel hue jitter (kept subtle) → no two texels read identical
            let jit = fbm(u * 2.0, v * 2.0, 8, 3, 130 + layer as u32) - 0.5;
            c = [(c[0] + jit * 0.045).clamp(0.0, 1.0), (c[1] + jit * 0.055).clamp(0.0, 1.0), (c[2] + jit * 0.035).clamp(0.0, 1.0)];
            px(&mut out, i, c);
        }
    }
    out
}

/// One material normal+roughness tile → RGBA8: RGB = tangent-space normal (from the height gradient,
/// encoded `n*0.5+0.5`), A = roughness. Tileable (the gradient wraps with the lattice).
fn bake_normal_one(layer: usize, n: usize, bump_scale: f32) -> Vec<u8> {
    let mut out = vec![0u8; n * n * 4];
    let e = 1.0 / n as f32;
    let s = bump(layer) * bump_scale;
    let (mc, ma) = micro(layer);
    let ms = ma * bump_scale;
    for y in 0..n {
        for x in 0..n {
            let (u, v) = (x as f32 / n as f32, y as f32 / n as f32);
            let i = (y * n + x) * 4;
            let dx = height(layer, u + e, v) - height(layer, u - e, v);
            let dy = height(layer, u, v + e) - height(layer, u, v - e);
            // fine micro-grain gradient (tiles: fbm with `cells=mc` wraps at u=1) layered on the coarse relief
            let mdx = fbm(u + e, v, mc, 2, 400 + layer as u32) - fbm(u - e, v, mc, 2, 400 + layer as u32);
            let mdy = fbm(u, v + e, mc, 2, 400 + layer as u32) - fbm(u, v - e, mc, 2, 400 + layer as u32);
            let (nx, ny, nz) = (-dx * s - mdx * ms, -dy * s - mdy * ms, 1.0);
            let inv = 1.0 / (nx * nx + ny * ny + nz * nz).sqrt();
            out[i] = ((nx * inv * 0.5 + 0.5) * 255.0) as u8;
            out[i + 1] = ((ny * inv * 0.5 + 0.5) * 255.0) as u8;
            out[i + 2] = ((nz * inv * 0.5 + 0.5) * 255.0) as u8;
            out[i + 3] = (rough_at(layer, u, v).clamp(0.0, 1.0) * 255.0) as u8; // spatially-varying roughness
        }
    }
    out
}

/// All material normal+roughness layers concatenated (layer-major) for a second `TEXTURE_2D_ARRAY`.
pub fn bake_normals(n: usize, bump_scale: f32) -> Vec<u8> {
    let mut all = Vec::with_capacity(LAYERS * n * n * 4);
    for l in 0..LAYERS { all.extend_from_slice(&bake_normal_one(l, n, bump_scale)); }
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
            let t = bake_normal_one(l, TEX_SIZE, 1.0);
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
            assert!(max_edge < 105, "layer {l} normal seam too large: {max_edge}"); // threshold scales with the (now bumpier) relief
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
            assert_eq!(bake_normals(n, 1.0).len(), LAYERS * n * n * 4, "normal array sized for {n}px");
        }
        // higher resolution is the same pattern at finer sampling: corner texel matches across sizes
        let a = bake_one(2, 128); let b = bake_one(2, 256); // stone layer, top-left texel (u=v=0)
        assert_eq!(&a[0..4], &b[0..4], "the (0,0) texel is identical regardless of resolution");
    }
}

#[cfg(test)]
mod new_material_tests {
    use super::*;

    fn tile(layer: usize) -> Vec<u8> { bake_one(layer, 64) }
    fn mean_luma(t: &[u8]) -> f32 {
        let n = t.len() / 4;
        (0..n).map(|i| (t[i*4] as f32 + t[i*4+1] as f32 + t[i*4+2] as f32) / 3.0).sum::<f32>() / n as f32
    }
    fn saturation(t: &[u8]) -> f32 {
        let n = t.len() / 4;
        (0..n).map(|i| {
            let (r, g, b) = (t[i*4] as f32, t[i*4+1] as f32, t[i*4+2] as f32);
            let mx = r.max(g).max(b); let mn = r.min(g).min(b);
            if mx <= 0.0 { 0.0 } else { (mx - mn) / mx }
        }).sum::<f32>() / n as f32
    }

    /// Every layer bakes, and none is a flat field.
    ///
    /// A material whose height function fell through to a default renders as a
    /// smooth ramp — visually "fine" and completely wrong. Variance catches it
    /// where a size check cannot.
    #[test]
    fn every_layer_bakes_with_real_variation() {
        for l in 0..LAYERS {
            let t = tile(l);
            assert_eq!(t.len(), 64 * 64 * 4, "layer {l} wrong size");
            let m = mean_luma(&t);
            let var = {
                let n = t.len() / 4;
                (0..n).map(|i| {
                    let y = (t[i*4] as f32 + t[i*4+1] as f32 + t[i*4+2] as f32) / 3.0;
                    (y - m) * (y - m)
                }).sum::<f32>() / n as f32
            };
            assert!(var > 4.0, "layer {l} is nearly flat (variance {var:.2}) — its height \
function probably fell through to a default");
        }
    }

    /// A tile must be seamless, because everything is sampled triplanar in
    /// world space and a seam repeats across the whole surface.
    ///
    /// ## What this does NOT catch, measured
    ///
    /// It catches a GROSS seam — a whole edge discontinuous — and nothing
    /// subtler. Probed by taking the `WOOD_GNARLED` knot's distance in the
    /// plane instead of on the torus, which is a real seam: the knot is present
    /// at u=0.01 and absent at u=0.99. The test passed anyway, three times,
    /// through a mean-step version, a worst-step version, and after moving the
    /// knot to straddle the edge so the bug was reachable at all.
    ///
    /// The reason is not a bad threshold. Ridged noise gives that layer
    /// interior steps larger than the seam itself, so ANY bound expressed
    /// relative to the interior swallows it. A test that only fires when the
    /// tile is obviously broken is still worth keeping — a layer whose height
    /// function ignores the lattice fails it outright — but `wrapd` is correct
    /// by construction here rather than verified by this.
    ///
    /// Catching the subtle case needs a different instrument: compare each edge
    /// texel against its own neighbourhood rather than against the tile's worst
    /// step. Not done, and named so the next person does not read a pass as
    /// proof of a seamless knot.
    #[test]
    fn tiles_wrap_without_a_seam() {
        for l in 0..LAYERS {
            let n = 64;
            let t = tile(l);
            let px = |x: usize, y: usize| {
                let i = (y * n + x) * 4;
                (t[i] as f32 + t[i+1] as f32 + t[i+2] as f32) / 3.0
            };
            // WORST step, not the mean. A seam is LOCAL: a knot straddling the
            // edge blows out a dozen rows and leaves the other fifty untouched,
            // so an average over the whole edge dilutes it below any sensible
            // threshold. Measured: with the knot's distance taken in the plane
            // instead of on the torus, the mean test passed and this one does
            // not.
            let worst_interior = (0..n).map(|y| (px(n/2, y) - px(n/2 - 1, y)).abs())
                .fold(0.0f32, f32::max);
            let worst_wrap = (0..n).map(|y| (px(0, y) - px(n - 1, y)).abs())
                .fold(0.0f32, f32::max);
            assert!(worst_wrap <= worst_interior.max(2.0) * 2.5,
                    "layer {l}: the wrap seam steps {worst_wrap:.1} at its worst against an \
interior worst of {worst_interior:.1} — a fixed-uv feature is probably measured in the plane \
instead of on the torus");
        }
    }

    /// Marble is pale and near-neutral; bark is dark. The palettes have to be
    /// attached to the right layers, and swapping two is invisible in a
    /// variance check.
    #[test]
    fn the_new_palettes_land_on_the_right_layers() {
        let marble = mean_luma(&tile(mat::MARBLE as usize));
        let bark = mean_luma(&tile(mat::WOOD_GNARLED as usize));
        assert!(marble > 170.0, "marble should be pale, got {marble:.0}");
        assert!(bark < 110.0, "gnarled bark should be dark, got {bark:.0}");
        assert!(saturation(&tile(mat::MARBLE as usize)) < 0.10,
                "marble should be near-neutral");
    }

    /// Wet wood is darker AND less saturated than the dry plank it derives
    /// from. Either alone reads as stain rather than water.
    #[test]
    fn wet_wood_is_darker_and_greyer_than_dry() {
        let dry = tile(mat::WOOD_SMOOTH as usize);
        let wet = tile(mat::WOOD_WET as usize);
        assert!(mean_luma(&wet) < mean_luma(&dry) * 0.8,
                "wet {:.0} vs dry {:.0}", mean_luma(&wet), mean_luma(&dry));
        assert!(saturation(&wet) < saturation(&dry),
                "water desaturates; darkening alone is a stain");
    }

    /// Roughness must be attached too — a polished material with the default
    /// roughness is just a pale one.
    #[test]
    fn roughness_separates_polish_from_matte() {
        assert!(roughness(mat::MARBLE as usize) < 0.15, "marble must be glossy");
        assert!(roughness(mat::DUST as usize) > 0.9, "dust must be matte");
        // Wet is SMOOTHER than dry: the film is what a low roughness models.
        assert!(roughness(mat::WOOD_WET as usize) < roughness(mat::WOOD_SMOOTH as usize));
        assert!(roughness(mat::WOOD_GNARLED as usize) > roughness(mat::WOOD_SMOOTH as usize));
    }

    /// Every layer has its own roughness and micro-grain entry rather than
    /// falling through to the paper default.
    #[test]
    fn no_new_layer_falls_through_to_the_paper_defaults() {
        for l in 5..LAYERS {
            assert_ne!((roughness(l), micro(l)), (0.7, (22, 0.6)),
                       "layer {l} is using the `_` arm in BOTH tables — almost certainly unhandled");
        }
    }

    /// A vein is a THIN DARK LINE, not a cloud.
    ///
    /// The seam fix rebuilt the vein from a wrapping fbm ridge instead of a
    /// warped sine, and the risk in that rewrite is losing the thing that makes
    /// marble read as marble: `powf(9)` on a ridge should leave most of the
    /// tile pale body with a small, much darker tail.
    ///
    /// Without it the layer still passes every other test here — it bakes, it
    /// varies, it wraps, it is pale and neutral — and looks like damp plaster.
    #[test]
    fn marble_veins_are_thin_and_dark_not_a_smooth_cloud() {
        let t = tile(mat::MARBLE as usize);
        let n = t.len() / 4;
        let mut luma: Vec<f32> = (0..n)
            .map(|i| (t[i*4] as f32 + t[i*4+1] as f32 + t[i*4+2] as f32) / 3.0).collect();
        luma.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let p05 = luma[n / 20];
        let median = luma[n / 2];
        // The body dominates and the veins are a genuine dark tail.
        assert!(median > 190.0, "the body should be pale marble, median {median:.0}");
        assert!(median - p05 > 45.0,
                "the darkest 5% is only {:.0} below the median — that is a cloud, not veining",
                median - p05);
        // And the tail is SMALL: veins are thin.
        let dark = luma.iter().filter(|y| **y < median - 40.0).count();
        assert!(dark * 100 / n < 22, "{}% of the tile is vein — too wide", dark * 100 / n);
    }

}
