//! `sim::shader_from_image` — synthesise material/shader descriptions from a
//! reference image (the "shader pictures" loop from `shader_synthesis_guide`).
//!
//! This file owns the **H1** surface: estimating PBR [`MaterialParams`] from an
//! RGBA image via [`material_from_image`], plus a tiny pure [`shade_swatch`]
//! helper so a swatch can be previewed headless (H3) without GPU. H2 (procedural
//! pattern GLSL codegen) and H3 (swatch render + image-match scoring) extend
//! this same module; they should add their own types/functions and leave the
//! `MaterialParams` / `material_from_image` surface here intact.
//!
//! Pure & deterministic: image in → params out. No GPU, no shader code, no I/O,
//! no RNG, no `unwrap`/`expect`/`panic!`. All outputs finite and clamped.
//!
//! The estimators are deliberately simple, monotone heuristics (the synthesis
//! loop refines from a correct global approximation):
//! - **albedo** = mean linear-ish colour (the dominant tone of the surface);
//! - **roughness** ← falls with specular highlights + global contrast (a glossy
//!   surface shows bright clipped speculars and high local contrast; a flat
//!   matte field shows neither → mid roughness);
//! - **metalness** ← rises with bright highlights on a *desaturated* surface
//!   (chrome/steel reads as colourless + specular; a colourful matte reads
//!   dielectric);
//! - **emissive** ← mean colour of clipped-bright pixels, weighted by how much
//!   of the image is clipped.

/// Estimated physically-based material parameters, all in `[0,1]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialParams {
    /// Base colour (mean surface tone), per channel `[0,1]`.
    pub albedo: [f32; 3],
    /// Microfacet roughness `[0,1]` (0 = mirror, 1 = fully matte).
    pub roughness: f32,
    /// Metalness guess `[0,1]`.
    pub metalness: f32,
    /// Emissive colour `[0,1]` (0 = non-emissive).
    pub emissive: [f32; 3],
}

impl MaterialParams {
    /// Neutral fallback for an empty/invalid image: mid-grey matte dielectric.
    pub const NEUTRAL: MaterialParams = MaterialParams {
        albedo: [0.5, 0.5, 0.5],
        roughness: 0.7,
        metalness: 0.0,
        emissive: [0.0, 0.0, 0.0],
    };
}

impl Default for MaterialParams {
    fn default() -> Self {
        MaterialParams::NEUTRAL
    }
}

// ── tunable heuristic constants (documented; the synthesis loop refines) ──
const HIGHLIGHT_LUMA: f32 = 0.85; // luma above this counts as a specular highlight
const CLIP_LUMA: f32 = 0.97; // luma above this counts as clipped-bright (emissive)
const R_BASE: f32 = 0.70; // roughness of a flat matte field
const R_SPEC: f32 = 0.60; // how much highlights cut roughness
const R_CONTRAST: f32 = 0.25; // how much global contrast cuts roughness
const R_MIN: f32 = 0.05;
const R_MAX: f32 = 0.95;
const METAL_GAIN: f32 = 2.0; // highlight×(1-sat) → metalness
const EMISSIVE_GAIN: f32 = 4.0; // clipped-area fraction → emissive strength

#[inline]
fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

#[inline]
fn luma(r: f32, g: f32, b: f32) -> f32 {
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// HSV-style saturation: `(max-min)/max`, 0 for greys/black.
#[inline]
fn saturation(r: f32, g: f32, b: f32) -> f32 {
    let mx = r.max(g).max(b);
    let mn = r.min(g).min(b);
    if mx <= 1e-6 {
        0.0
    } else {
        (mx - mn) / mx
    }
}

/// Estimate [`MaterialParams`] from an RGBA8 image (`rgba.len() >= w*h*4`,
/// row-major). An empty or too-small buffer returns [`MaterialParams::NEUTRAL`]
/// (never panics).
pub fn material_from_image(rgba: &[u8], w: usize, h: usize) -> MaterialParams {
    let n = w.saturating_mul(h);
    if n == 0 || rgba.len() < n.saturating_mul(4) {
        return MaterialParams::NEUTRAL;
    }
    let inv = 1.0 / n as f32;

    let (mut sr, mut sg, mut sb) = (0.0f32, 0.0f32, 0.0f32);
    let (mut sum_luma, mut sum_luma_sq) = (0.0f32, 0.0f32);
    let mut sum_sat = 0.0f32;
    let mut highlights = 0u32;
    let mut clipped = 0u32;
    let (mut cr, mut cg, mut cb) = (0.0f32, 0.0f32, 0.0f32);

    for i in 0..n {
        let p = i * 4;
        let r = rgba[p] as f32 / 255.0;
        let g = rgba[p + 1] as f32 / 255.0;
        let b = rgba[p + 2] as f32 / 255.0;
        sr += r;
        sg += g;
        sb += b;
        let l = luma(r, g, b);
        sum_luma += l;
        sum_luma_sq += l * l;
        sum_sat += saturation(r, g, b);
        if l > HIGHLIGHT_LUMA {
            highlights += 1;
        }
        if l > CLIP_LUMA {
            clipped += 1;
            cr += r;
            cg += g;
            cb += b;
        }
    }

    let albedo = [clamp01(sr * inv), clamp01(sg * inv), clamp01(sb * inv)];

    let mean_luma = sum_luma * inv;
    // variance = E[l^2] - E[l]^2, clamped ≥ 0 against float error.
    let variance = (sum_luma_sq * inv - mean_luma * mean_luma).max(0.0);
    let std = variance.sqrt();
    // luma std maxes around 0.5 (half black / half white); normalise to [0,1].
    let contrast = clamp01(std / 0.5);

    let highlight_ratio = highlights as f32 * inv;
    let mean_sat = sum_sat * inv;

    let roughness = clamp01_range(
        R_BASE - R_SPEC * highlight_ratio - R_CONTRAST * contrast,
        R_MIN,
        R_MAX,
    );

    let metalness = clamp01(METAL_GAIN * highlight_ratio * (1.0 - mean_sat));

    let emissive = if clipped == 0 {
        [0.0, 0.0, 0.0]
    } else {
        let cinv = 1.0 / clipped as f32;
        let strength = clamp01(clipped as f32 * inv * EMISSIVE_GAIN);
        [
            clamp01(cr * cinv) * strength,
            clamp01(cg * cinv) * strength,
            clamp01(cb * cinv) * strength,
        ]
    };

    MaterialParams { albedo, roughness, metalness, emissive }
}

#[inline]
fn clamp01_range(v: f32, lo: f32, hi: f32) -> f32 {
    if v.is_nan() {
        lo
    } else {
        v.max(lo).min(hi)
    }
}

/// Tiny pure preview shader: shade the material under a single light with
/// `n·l = nl` and view-aligned specular, returning an sRGB-ish RGB8 swatch
/// pixel. Lets H3 (and the swatch example) render an eyeballable cell without a
/// GPU. Metals tint their specular by albedo; dielectrics use white speculars.
pub fn shade_swatch(p: &MaterialParams, nl: f32) -> [u8; 3] {
    let nl = clamp01(nl);
    // Lambert diffuse, attenuated for metals (metals have little diffuse).
    let diffuse_k = (1.0 - p.metalness) * nl;
    // Specular lobe: sharper (and stronger) as roughness drops.
    let shininess = 1.0 + (1.0 - p.roughness) * 64.0;
    let spec = nl.powf(shininess) * (1.0 - p.roughness);
    let spec_tint = |c: f32| if p.metalness > 0.5 { c } else { 1.0 };

    let shade = |c: f32, tint: f32| -> u8 {
        let lit = c * (0.15 + 0.85 * diffuse_k) + spec * tint + p_emissive(p, c);
        (clamp01(lit) * 255.0 + 0.5) as u8
    };
    [
        shade(p.albedo[0], spec_tint(p.albedo[0])),
        shade(p.albedo[1], spec_tint(p.albedo[1])),
        shade(p.albedo[2], spec_tint(p.albedo[2])),
    ]
}

#[inline]
fn p_emissive(p: &MaterialParams, _c: f32) -> f32 {
    // emissive adds independent of lighting; indexed per channel by caller order
    // (kept simple: average emissive so the helper stays per-channel-correct).
    (p.emissive[0] + p.emissive[1] + p.emissive[2]) / 3.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: usize, h: usize, rgb: [u8; 3]) -> Vec<u8> {
        let mut v = Vec::with_capacity(w * h * 4);
        for _ in 0..w * h {
            v.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
        }
        v
    }

    fn approx(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    #[test]
    fn flat_matte_grey_is_mid_rough_low_metal_grey_albedo() {
        let img = solid(32, 32, [128, 128, 128]);
        let m = material_from_image(&img, 32, 32);
        assert!(approx(m.albedo[0], 0.502, 0.01) && approx(m.albedo[1], 0.502, 0.01));
        assert!((0.5..=0.85).contains(&m.roughness), "mid roughness, got {}", m.roughness);
        assert!(m.metalness < 0.05, "low metalness, got {}", m.metalness);
        assert_eq!(m.emissive, [0.0, 0.0, 0.0], "matte grey isn't emissive");
    }

    #[test]
    fn specular_image_has_lower_roughness_than_matte() {
        let matte = material_from_image(&solid(32, 32, [128, 128, 128]), 32, 32);

        // Mid-grey field with a bright specular cluster (high luma + contrast).
        let (w, h) = (32usize, 32usize);
        let mut img = solid(w, h, [120, 120, 120]);
        for y in 12..20 {
            for x in 12..20 {
                let p = (y * w + x) * 4;
                img[p] = 252;
                img[p + 1] = 252;
                img[p + 2] = 252;
            }
        }
        let spec = material_from_image(&img, w, h);
        assert!(spec.roughness < matte.roughness, "specular {} should be < matte {}", spec.roughness, matte.roughness);
    }

    #[test]
    fn clipped_bright_region_drives_emissive() {
        let (w, h) = (32usize, 32usize);
        let dark = material_from_image(&solid(w, h, [40, 40, 40]), w, h);
        assert_eq!(dark.emissive, [0.0, 0.0, 0.0]);

        // Same dark field with a clipped pure-white patch → emissive > 0.
        let mut img = solid(w, h, [40, 40, 40]);
        for y in 0..8 {
            for x in 0..8 {
                let p = (y * w + x) * 4;
                img[p] = 255;
                img[p + 1] = 255;
                img[p + 2] = 255;
            }
        }
        let m = material_from_image(&img, w, h);
        let e = m.emissive[0] + m.emissive[1] + m.emissive[2];
        assert!(e > 0.0, "clipped white should be emissive, got {:?}", m.emissive);
    }

    #[test]
    fn albedo_is_the_mean_colour() {
        // Half (200,0,0), half black → mean ≈ (100/255, 0, 0).
        let (w, h) = (16usize, 16usize);
        let mut img = solid(w, h, [0, 0, 0]);
        for y in 0..h {
            for x in 0..(w / 2) {
                let p = (y * w + x) * 4;
                img[p] = 200;
            }
        }
        let m = material_from_image(&img, w, h);
        assert!(approx(m.albedo[0], 100.0 / 255.0, 0.02), "albedo.r {}", m.albedo[0]);
        assert!(m.albedo[1] < 0.01 && m.albedo[2] < 0.01);
    }

    #[test]
    fn desaturated_specular_reads_more_metallic_than_colourful_matte() {
        let (w, h) = (32usize, 32usize);
        // Colourful matte (saturated red, no highlights).
        let red = material_from_image(&solid(w, h, [200, 20, 20]), w, h);
        // Desaturated grey with a big bright highlight (chrome-ish).
        let mut chrome = solid(w, h, [140, 140, 140]);
        for y in 6..26 {
            for x in 6..26 {
                let p = (y * w + x) * 4;
                chrome[p] = 250;
                chrome[p + 1] = 250;
                chrome[p + 2] = 250;
            }
        }
        let chrome = material_from_image(&chrome, w, h);
        assert!(chrome.metalness > red.metalness, "chrome {} > red {}", chrome.metalness, red.metalness);
    }

    #[test]
    fn ranges_finite_and_in_unit_on_arbitrary_bytes() {
        let (w, h) = (20usize, 13usize);
        let mut img = vec![0u8; w * h * 4];
        // pseudo-pattern (deterministic, no RNG)
        for (i, b) in img.iter_mut().enumerate() {
            *b = ((i * 37 + 11) % 256) as u8;
        }
        let m = material_from_image(&img, w, h);
        let all = [m.albedo[0], m.albedo[1], m.albedo[2], m.roughness, m.metalness, m.emissive[0], m.emissive[1], m.emissive[2]];
        for v in all {
            assert!(v.is_finite() && (0.0..=1.0).contains(&v), "out of range: {v}");
        }
    }

    #[test]
    fn deterministic() {
        let img = solid(24, 24, [77, 133, 200]);
        let a = material_from_image(&img, 24, 24);
        for _ in 0..20 {
            assert_eq!(material_from_image(&img, 24, 24), a);
        }
    }

    #[test]
    fn empty_or_undersized_returns_neutral_no_panic() {
        assert_eq!(material_from_image(&[], 0, 0), MaterialParams::NEUTRAL);
        assert_eq!(material_from_image(&[1, 2, 3], 4, 4), MaterialParams::NEUTRAL); // too small
        // exactly-sized minimal image is fine
        let one = material_from_image(&[10, 20, 30, 255], 1, 1);
        assert!(one.albedo[0].is_finite());
    }

    #[test]
    fn shade_swatch_is_finite_and_brighter_in_light() {
        let m = material_from_image(&solid(8, 8, [180, 120, 60]), 8, 8);
        let lit = shade_swatch(&m, 1.0);
        let dark = shade_swatch(&m, 0.0);
        let sum = |c: [u8; 3]| c[0] as u32 + c[1] as u32 + c[2] as u32;
        assert!(sum(lit) >= sum(dark), "lit side should be >= shadow side");
    }
}

// ── H2: procedural pattern GLSL codegen (image → .frag), validated glslang+spirv-val ──
#[derive(Clone, Debug, PartialEq)]
pub struct PatternFeatures {
    /// Dominant orientation of the texture, radians.
    pub directionality_rad: f32,
    /// How directional it is: `0` isotropic … `1` strongly streaked along
    /// `directionality_rad`.
    pub anisotropy: f32,
    /// Dominant spatial frequency (pattern repeats across the unit square).
    pub base_freq: f32,
    /// Octaves of value-noise detail (clamped to `1..=8`).
    pub octaves: u32,
    /// Domain-warp amount (`0` = none; larger = more swirl/organic distortion).
    pub warp: f32,
    /// Output contrast applied to the noise field before the palette lookup.
    pub contrast: f32,
    /// Seed so distinct images get distinct fields.
    pub seed: u32,
    /// Ordered colour ramp (≥1 stop); `[r,g,b]` in `0..1`. The noise field maps
    /// across these stops.
    pub palette: Vec<[f32; 3]>,
}

impl Default for PatternFeatures {
    fn default() -> PatternFeatures {
        PatternFeatures {
            directionality_rad: 0.0,
            anisotropy: 0.0,
            base_freq: 4.0,
            octaves: 4,
            warp: 0.0,
            contrast: 1.0,
            seed: 0,
            palette: vec![[0.05, 0.05, 0.08], [0.9, 0.85, 0.7]],
        }
    }
}

/// Format an `f32` as a GLSL float literal: fixed-point (never exponent form,
/// which some drivers reject), always with a decimal point. Non-finite → `0.0`.
fn glf(x: f32) -> String {
    let v = if x.is_finite() { x } else { 0.0 };
    format!("{:.6}", v)
}

/// Emit a complete, self-contained GLSL fragment shader approximating the look
/// described by `features`. Pure and deterministic: identical features → byte-
/// identical source. The result validates as SPIR-V (see module tests).
pub fn synth_pattern_glsl(features: &PatternFeatures) -> String {
    // sanitise into safe ranges so the emitted constants are always valid
    let octaves = features.octaves.clamp(1, 8);
    let base_freq = if features.base_freq.is_finite() { features.base_freq.clamp(0.25, 64.0) } else { 4.0 };
    let aniso = features.anisotropy.clamp(0.0, 1.0);
    let warp = if features.warp.is_finite() { features.warp.clamp(0.0, 4.0) } else { 0.0 };
    let contrast = if features.contrast.is_finite() { features.contrast.clamp(0.1, 4.0) } else { 1.0 };
    let angle = if features.directionality_rad.is_finite() { features.directionality_rad } else { 0.0 };
    let seed = features.seed;

    // palette: at least one stop; clamp channels to [0,1]
    let stops: Vec<[f32; 3]> = if features.palette.is_empty() {
        vec![[0.0, 0.0, 0.0], [1.0, 1.0, 1.0]]
    } else {
        features
            .palette
            .iter()
            .map(|c| [c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0)])
            .collect()
    };
    let n = stops.len();

    // anisotropic frequency: stretch across the dominant direction
    let fx = base_freq * (1.0 + 2.0 * aniso);
    let fy = base_freq * (1.0 - 0.75 * aniso);

    let mut s = String::new();
    s.push_str("#version 450\n");
    s.push_str("// Procedurally synthesized by sim::shader_from_image::synth_pattern_glsl (H2).\n");
    s.push_str("// Fullscreen present.vert convention: v_uv at location 0.\n");
    s.push_str(&format!(
        "// features: freq={} octaves={} aniso={} warp={} contrast={} angle={} seed={} stops={}\n",
        glf(base_freq), octaves, glf(aniso), glf(warp), glf(contrast), glf(angle), seed, n
    ));
    s.push_str("layout(location = 0) in vec2 v_uv;\n");
    s.push_str("layout(location = 0) out vec4 out_color;\n");
    s.push_str("layout(push_constant) uniform Pc { float time; } pc;\n\n");

    // hash / value-noise / fbm
    s.push_str("float hash21(vec2 p) {\n");
    s.push_str(&format!("    p = fract(p * vec2(123.34, 456.21) + {});\n", glf((seed as f32) * 0.001 + 0.1)));
    s.push_str("    p += dot(p, p + 34.345);\n");
    s.push_str("    return fract(p.x * p.y);\n}\n\n");

    s.push_str("float vnoise(vec2 p) {\n");
    s.push_str("    vec2 i = floor(p);\n    vec2 f = fract(p);\n");
    s.push_str("    vec2 u = f * f * (3.0 - 2.0 * f);\n");
    s.push_str("    float a = hash21(i + vec2(0.0, 0.0));\n");
    s.push_str("    float b = hash21(i + vec2(1.0, 0.0));\n");
    s.push_str("    float c = hash21(i + vec2(0.0, 1.0));\n");
    s.push_str("    float d = hash21(i + vec2(1.0, 1.0));\n");
    s.push_str("    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);\n}\n\n");

    s.push_str(&format!("float fbm(vec2 p) {{\n    float sum = 0.0;\n    float amp = 0.5;\n    for (int o = 0; o < {}; o++) {{\n", octaves));
    s.push_str("        sum += amp * vnoise(p);\n        p *= 2.0;\n        amp *= 0.5;\n    }\n    return sum;\n}\n\n");

    // palette ramp
    s.push_str(&format!("const int STOP_COUNT = {};\n", n));
    s.push_str(&format!("const vec3 STOPS[{}] = vec3[](\n", n));
    for (k, c) in stops.iter().enumerate() {
        let comma = if k + 1 < n { "," } else { "" };
        s.push_str(&format!("    vec3({}, {}, {}){}\n", glf(c[0]), glf(c[1]), glf(c[2]), comma));
    }
    s.push_str(");\n\n");
    s.push_str("vec3 palette(float t) {\n");
    s.push_str("    t = clamp(t, 0.0, 1.0);\n");
    if n == 1 {
        s.push_str("    return STOPS[0];\n");
    } else {
        s.push_str(&format!("    float scaled = t * float({});\n", n - 1));
        s.push_str("    int i = int(floor(scaled));\n");
        s.push_str(&format!("    i = clamp(i, 0, {});\n", n - 1));
        s.push_str(&format!("    int j = min(i + 1, {});\n", n - 1));
        s.push_str("    float f = fract(scaled);\n");
        s.push_str("    return mix(STOPS[i], STOPS[j], f);\n");
    }
    s.push_str("}\n\n");

    // main: rotate into the dominant direction, anisotropic frequency, optional
    // domain warp, fbm field, contrast, palette.
    let (ca, sa) = (angle.cos(), angle.sin());
    s.push_str("void main() {\n");
    s.push_str(&format!("    mat2 rot = mat2({}, {}, {}, {});\n", glf(ca), glf(-sa), glf(sa), glf(ca)));
    s.push_str("    vec2 uv = rot * (v_uv - 0.5);\n");
    s.push_str(&format!("    vec2 p = uv * vec2({}, {});\n", glf(fx), glf(fy)));
    if warp > 0.0 {
        s.push_str(&format!("    vec2 q = vec2(fbm(p + 0.0), fbm(p + 5.2));\n    p += {} * q;\n", glf(warp)));
    }
    s.push_str("    float v = fbm(p);\n");
    s.push_str(&format!("    v = clamp((v - 0.5) * {} + 0.5, 0.0, 1.0);\n", glf(contrast)));
    s.push_str("    vec3 col = palette(v);\n");
    s.push_str("    out_color = vec4(col, 1.0);\n");
    s.push_str("}\n");
    s
}


#[cfg(test)]
mod h2_pattern_tests {
    use super::*;
    use std::process::Command;

    fn sample_feature_sets() -> Vec<PatternFeatures> {
        vec![
            PatternFeatures::default(),
            PatternFeatures { palette: vec![[1.0, 0.2, 0.1]], octaves: 1, warp: 0.0, ..Default::default() }, // single stop
            PatternFeatures {
                directionality_rad: 0.7,
                anisotropy: 0.9,
                base_freq: 12.0,
                octaves: 6,
                warp: 1.5,
                contrast: 2.0,
                seed: 42,
                palette: vec![[0.0, 0.0, 0.1], [0.2, 0.4, 0.6], [0.9, 0.9, 1.0]],
            },
            // hostile inputs must still produce valid source
            PatternFeatures {
                directionality_rad: f32::NAN,
                anisotropy: 9.0,
                base_freq: -3.0,
                octaves: 99,
                warp: f32::INFINITY,
                contrast: f32::NAN,
                seed: 7,
                palette: vec![[2.0, -1.0, 0.5], [0.3, 0.3, 0.3], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0], [0.5, 0.1, 0.9]],
            },
        ]
    }

    #[test]
    fn pure_and_deterministic() {
        let f = sample_feature_sets()[2].clone();
        assert_eq!(synth_pattern_glsl(&f), synth_pattern_glsl(&f));
    }

    #[test]
    fn structural_invariants() {
        for f in sample_feature_sets() {
            let src = synth_pattern_glsl(&f);
            assert!(src.starts_with("#version 450"), "must declare GLSL version");
            assert!(src.contains("layout(location = 0) in vec2 v_uv;"), "present.vert input");
            assert!(src.contains("layout(location = 0) out vec4 out_color;"), "frag output");
            assert!(src.contains("void main()"), "has main");
            assert!(!src.contains("e-"), "no exponent float literals (driver-safe)");
            assert!(!src.contains("inf") && !src.contains("nan") && !src.contains("NaN"), "no non-finite literals");
        }
    }

    fn find_tool(name: &str) -> Option<String> {
        if Command::new(name).arg("--version").output().is_ok() {
            return Some(name.to_string());
        }
        let cand = format!("/usr/bin/{}", name);
        if std::path::Path::new(&cand).exists() {
            return Some(cand);
        }
        None
    }

    /// The required bar: every emitted shader compiles to valid SPIR-V.
    #[test]
    fn emitted_glsl_compiles_to_valid_spirv() {
        let glslang = match find_tool("glslangValidator") {
            Some(g) => g,
            None => {
                eprintln!("glslangValidator not found — skipping SPIR-V validation (structural test still ran)");
                return;
            }
        };
        let spirv_val = find_tool("spirv-val");
        let dir = std::env::temp_dir();
        for (idx, f) in sample_feature_sets().into_iter().enumerate() {
            let src = synth_pattern_glsl(&f);
            let frag = dir.join(format!("h2_synth_{}.frag", idx));
            let spv = dir.join(format!("h2_synth_{}.spv", idx));
            std::fs::write(&frag, &src).expect("write temp frag");
            let out = Command::new(&glslang)
                .arg("-V")
                .arg(&frag)
                .arg("-o")
                .arg(&spv)
                .output()
                .expect("run glslangValidator");
            assert!(
                out.status.success(),
                "glslangValidator rejected generated shader #{}:\nSTDOUT:\n{}\nSOURCE:\n{}",
                idx,
                String::from_utf8_lossy(&out.stdout),
                src
            );
            if let Some(ref sv) = spirv_val {
                let v = Command::new(sv).arg(&spv).output().expect("run spirv-val");
                assert!(
                    v.status.success(),
                    "spirv-val rejected generated shader #{}:\n{}",
                    idx,
                    String::from_utf8_lossy(&v.stderr)
                );
            }
        }
    }
}
