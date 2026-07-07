//! `render::dynamic_lights` — dynamic point lights from emissive sources (S1).
//!
//! Forge glow-boxes, eyes, and fire are emissive point lights. This is the pure
//! CPU-side math: a [`PointLight`] list, [`gather_nearest`] to pick the N nearest
//! to a fragment region, and [`apply_lights`] — the CPU REFERENCE for the
//! lighting sum (inverse-square attenuation × a smooth range window × clamp).
//! [`dynamic_lights_frag_glsl`] emits a `.frag` that applies the SAME math on the
//! GPU; the test checks the CPU reference against that formula and validates the
//! GLSL. NO descriptor/pass wiring — codegen + reference only (SHARED_RULES §1/§2).
//! Mirrors `sim::light_physics` attenuation; kept render-local & dependency-free.

/// An emissive point light.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointLight {
    pub pos: [f32; 3],
    pub color: [f32; 3],
    pub intensity: f32,
    /// Influence radius; contribution is exactly 0 at and beyond it.
    pub radius: f32,
}

fn dist2(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

/// Indices of the up-to-`n` lights nearest `region_center`, nearest first.
/// Deterministic: ties break toward the lower index. (This is the per-region
/// light cull the GPU would consume.)
pub fn gather_nearest(region_center: [f32; 3], lights: &[PointLight], n: usize) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..lights.len()).collect();
    // sort by (distance, index) — total order, no NaN surprises (dist2 finite for finite input)
    idx.sort_by(|&a, &b| {
        let da = dist2(lights[a].pos, region_center);
        let db = dist2(lights[b].pos, region_center);
        da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal).then(a.cmp(&b))
    });
    idx.truncate(n);
    idx
}

/// Smooth windowed inverse-square attenuation for one light at `point`.
/// `1/(1+d²)` (finite at d=0) × `(1 − d²/r²)²` (→ 0 at the radius). `0` at/beyond
/// the radius. Always finite, `>= 0`.
pub fn light_attenuation(l: &PointLight, point: [f32; 3]) -> f32 {
    let r = l.radius.max(1e-4);
    let d2 = dist2(l.pos, point);
    let r2 = r * r;
    if !(d2 < r2) {
        return 0.0; // at/beyond radius (also covers NaN)
    }
    let window = 1.0 - d2 / r2;
    let window = (window * window).max(0.0);
    (1.0 / (1.0 + d2)) * window
}

/// CPU REFERENCE: accumulate the `lights` at `point`, clamped to `[0,1]` per
/// channel. The emitted `.frag` computes the identical value. Finite & bounded.
pub fn apply_lights(point: [f32; 3], lights: &[PointLight]) -> [f32; 3] {
    let mut acc = [0.0f32; 3];
    for l in lights {
        let a = light_attenuation(l, point) * l.intensity.max(0.0);
        for c in 0..3 {
            acc[c] += l.color[c].max(0.0) * a;
        }
    }
    [acc[0].min(1.0), acc[1].min(1.0), acc[2].min(1.0)]
}

fn glf(x: f32) -> String {
    let v = if x.is_finite() { x } else { 0.0 };
    format!("{:.6}", v)
}

/// Emit a fragment shader applying up to `n` point lights with the SAME math as
/// [`apply_lights`] (inverse-square × windowed radius × clamp). Lights arrive in
/// a uniform block and the shaded world point in `frag_world.xyz` (`.w` = active
/// count). This is codegen for validation/reference — NOT wired to a live pass.
/// Floats fixed-point ⇒ validates as SPIR-V.
pub fn dynamic_lights_frag_glsl(n: usize) -> String {
    let n = n.clamp(1, 64);
    let mut s = String::new();
    s.push_str("#version 450\n");
    s.push_str("// render::dynamic_lights::dynamic_lights_frag_glsl (S1) — emissive point lights.\n");
    s.push_str("layout(location = 0) in vec2 v_uv;\n");
    s.push_str("layout(location = 0) out vec4 out_color;\n");
    s.push_str(&format!("const int N = {};\n", n));
    s.push_str("layout(set = 0, binding = 0) uniform Lights {\n");
    s.push_str("    vec4 pos_radius[N];      // xyz = world pos, w = radius\n");
    s.push_str("    vec4 color_intensity[N]; // rgb = colour, a = intensity\n");
    s.push_str("    vec4 frag_world;         // xyz = shaded point, w = active count\n");
    s.push_str("} L;\n\n");
    s.push_str("void main() {\n");
    s.push_str("    vec3 P = L.frag_world.xyz;\n");
    s.push_str("    int count = int(L.frag_world.w);\n");
    s.push_str("    vec3 acc = vec3(0.0);\n");
    s.push_str("    for (int i = 0; i < N; i++) {\n");
    s.push_str("        if (i >= count) { break; }\n");
    s.push_str("        vec3 d = L.pos_radius[i].xyz - P;\n");
    s.push_str(&format!("        float r = max(L.pos_radius[i].w, {});\n", glf(1e-4)));
    s.push_str("        float d2 = dot(d, d);\n");
    s.push_str("        float r2 = r * r;\n");
    s.push_str("        float window = clamp(1.0 - d2 / r2, 0.0, 1.0);\n");
    s.push_str("        window = window * window;\n");
    s.push_str("        float a = (1.0 / (1.0 + d2)) * window * max(L.color_intensity[i].a, 0.0);\n");
    s.push_str("        acc += max(L.color_intensity[i].rgb, vec3(0.0)) * a;\n");
    s.push_str("    }\n");
    s.push_str("    acc = clamp(acc, 0.0, 1.0);\n");
    s.push_str("    out_color = vec4(acc, 1.0);\n");
    s.push_str("}\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn pl(pos: [f32; 3], col: [f32; 3], i: f32, r: f32) -> PointLight {
        PointLight { pos, color: col, intensity: i, radius: r }
    }

    #[test]
    fn gather_picks_nearest_n_deterministically() {
        let lights = [
            pl([10.0, 0.0, 0.0], [1.0; 3], 1.0, 5.0),
            pl([1.0, 0.0, 0.0], [1.0; 3], 1.0, 5.0),
            pl([3.0, 0.0, 0.0], [1.0; 3], 1.0, 5.0),
            pl([2.0, 0.0, 0.0], [1.0; 3], 1.0, 5.0),
        ];
        let got = gather_nearest([0.0, 0.0, 0.0], &lights, 2);
        assert_eq!(got, vec![1, 3], "two nearest are idx 1 (d=1) then 3 (d=2)");
        // n larger than the set just returns all, nearest-first
        assert_eq!(gather_nearest([0.0; 3], &lights, 99), vec![1, 3, 2, 0]);
    }

    #[test]
    fn attenuation_falls_off_zero_beyond_radius_finite() {
        let l = pl([0.0, 0.0, 0.0], [1.0; 3], 1.0, 10.0);
        let near = light_attenuation(&l, [1.0, 0.0, 0.0]);
        let far = light_attenuation(&l, [5.0, 0.0, 0.0]);
        assert!(near > far && far > 0.0, "closer is brighter");
        assert_eq!(light_attenuation(&l, [10.0, 0.0, 0.0]), 0.0, "zero at radius");
        assert_eq!(light_attenuation(&l, [50.0, 0.0, 0.0]), 0.0, "zero beyond radius");
        // d=0 finite (== 1 * window(=1))
        assert!((light_attenuation(&l, [0.0; 3]) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn apply_lights_matches_reference_formula_and_clamps() {
        // independently recompute the documented formula and compare
        let lights = [
            pl([0.0, 0.0, 0.0], [1.0, 0.2, 0.1], 2.0, 20.0),
            pl([4.0, 0.0, 0.0], [0.1, 0.3, 1.0], 1.5, 15.0),
        ];
        let p = [1.0, 0.5, 0.0];
        let got = apply_lights(p, &lights);
        let mut want = [0.0f32; 3];
        for l in &lights {
            let r2 = l.radius * l.radius;
            let d = [l.pos[0] - p[0], l.pos[1] - p[1], l.pos[2] - p[2]];
            let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            let w = (1.0f32 - d2 / r2).max(0.0);
            let a = (1.0 / (1.0 + d2)) * (w * w) * l.intensity;
            for c in 0..3 {
                want[c] += l.color[c] * a;
            }
        }
        for c in 0..3 {
            want[c] = want[c].min(1.0);
            assert!((got[c] - want[c]).abs() < 1e-5, "channel {} {} vs {}", c, got[c], want[c]);
            assert!((0.0..=1.0).contains(&got[c]));
        }
        // a brighter near light dominates the accumulation
        let near_only = apply_lights([0.2, 0.0, 0.0], &[lights[0]]);
        assert!(near_only[0] >= near_only[2], "warm near light dominates red over blue");
    }

    #[test]
    fn apply_lights_finite_on_hostile_input() {
        let lights = [
            pl([f32::NAN, 0.0, 0.0], [2.0, -1.0, 0.5], f32::INFINITY, -3.0),
            pl([0.0; 3], [1.0; 3], 1.0, 0.0),
        ];
        let got = apply_lights([0.0; 3], &lights);
        assert!(got.iter().all(|c| c.is_finite() && (0.0..=1.0).contains(c)));
    }

    fn tool(name: &str) -> Option<String> {
        if Command::new(name).arg("--version").output().is_ok() {
            return Some(name.to_string());
        }
        let p = format!("/usr/bin/{}", name);
        if std::path::Path::new(&p).exists() {
            Some(p)
        } else {
            None
        }
    }

    #[test]
    fn dynamic_lights_frag_validates_for_several_n() {
        let glslang = match tool("glslangValidator") {
            Some(g) => g,
            None => {
                eprintln!("glslangValidator absent — skipping SPIR-V check");
                return;
            }
        };
        let spirv_val = tool("spirv-val");
        let dir = std::env::temp_dir();
        for n in [1usize, 4, 8, 16] {
            let src = dynamic_lights_frag_glsl(n);
            let frag = dir.join(format!("s1_lights_{}.frag", n));
            let spv = dir.join(format!("s1_lights_{}.spv", n));
            std::fs::write(&frag, &src).expect("write");
            let out = Command::new(&glslang).arg("-V").arg(&frag).arg("-o").arg(&spv).output().expect("glslang");
            assert!(out.status.success(), "lights frag N={} rejected:\n{}\n{}", n, String::from_utf8_lossy(&out.stdout), src);
            if let Some(ref sv) = spirv_val {
                let v = Command::new(sv).arg(&spv).output().expect("spirv-val");
                assert!(v.status.success(), "spirv-val N={}: {}", n, String::from_utf8_lossy(&v.stderr));
            }
        }
    }
}
