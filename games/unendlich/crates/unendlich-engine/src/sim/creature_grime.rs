//! `sim::creature_grime` — dirt/grime accumulation on creatures (S1).
//!
//! Pure, deterministic data: a creature's recent biome/activity history feeds a
//! [`GrimeState`] that produces a grime factor in `0..1` plus a mud tint, which
//! the forge skin or material layer can consume later (DATA ONLY — no live
//! wiring here). Grime sticks more to DOWN-facing surfaces (belly, feet) than to
//! the back, via [`belly_weight`]. A companion fragment shader
//! ([`grime_frag_glsl`]) darkens and roughens the lower surfaces; it validates as
//! SPIR-V (see tests). Panic-free, no IO/RNG in non-test code (SHARED_RULES §1).

/// One slice of recent history (a tick's worth of context).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrimeSample {
    /// How dirtying the ground underfoot is (`0` clean rock/snow … `1` deep mud).
    pub soil_grime01: f32,
    /// Wetness from water/rain this tick (`1` washes grime off).
    pub wetness01: f32,
    /// Locomotion activity (`0` still … `1` running) — stirs up & rubs on dirt.
    pub activity01: f32,
    /// Ground/mud colour to tint toward when dirtying, `[r,g,b]` in `0..1`.
    pub soil_tint: [f32; 3],
    /// Seconds this sample covers.
    pub dt: f32,
}

impl Default for GrimeSample {
    fn default() -> GrimeSample {
        GrimeSample { soil_grime01: 0.0, wetness01: 0.0, activity01: 0.0, soil_tint: [0.40, 0.30, 0.20], dt: 1.0 / 60.0 }
    }
}

/// Accumulated grime state for one creature.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GrimeState {
    grime01: f32,
    tint: [f32; 3],
}

const GAIN_RATE: f32 = 0.6; // grime/sec at full soil×activity
const WASH_RATE: f32 = 0.9; // grime removed/sec at full wetness
const TINT_BLEND: f32 = 2.0; // tint EMA speed toward the active soil colour
/// Even a clean back keeps a little grime; the belly gets the full amount.
const BACK_FLOOR: f32 = 0.25;

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

impl Default for GrimeState {
    fn default() -> GrimeState {
        GrimeState::clean()
    }
}

impl GrimeState {
    /// A freshly-cleaned creature.
    pub fn clean() -> GrimeState {
        GrimeState { grime01: 0.0, tint: [0.40, 0.30, 0.20] }
    }

    pub fn grime_factor01(&self) -> f32 {
        self.grime01
    }
    pub fn tint(&self) -> [f32; 3] {
        self.tint
    }

    /// Fold one history sample in. Active movement over dirty ground accrues
    /// grime; wetness washes it off; both are rate-limited and clamped. The tint
    /// drifts toward the soil colour in proportion to how much grime is gained.
    pub fn accumulate(&mut self, s: &GrimeSample) {
        let dt = if s.dt.is_finite() { s.dt.max(0.0) } else { 0.0 };
        let soil = clamp01(s.soil_grime01);
        let wet = clamp01(s.wetness01);
        let act = clamp01(s.activity01);

        let gain = soil * act * GAIN_RATE * dt;
        let wash = wet * WASH_RATE * dt;
        self.grime01 = clamp01(self.grime01 + gain - wash);

        // tint drifts toward soil colour, weighted by grime actually gained
        let blend = (gain * TINT_BLEND).min(1.0);
        for c in 0..3 {
            let target = clamp01(s.soil_tint[c]);
            self.tint[c] = self.tint[c] + (target - self.tint[c]) * blend;
        }
    }
}

/// How strongly grime applies to a surface whose world normal has vertical
/// component `normal_y` (`-1` straight down = belly/feet → full; `+1` up = back →
/// only [`BACK_FLOOR`]). Always in `[BACK_FLOOR, 1]`.
pub fn belly_weight(normal_y: f32) -> f32 {
    let down = clamp01(-normal_y); // 1 when facing straight down
    BACK_FLOOR + (1.0 - BACK_FLOOR) * down
}

/// CPU reference the swatch PNG uses and the `.frag` mirrors: composite `base`
/// material colour with grime at a surface of vertical normal `normal_y`.
pub fn apply_grime_color(base: [f32; 3], normal_y: f32, st: &GrimeState) -> [f32; 3] {
    let applied = clamp01(st.grime01 * belly_weight(normal_y));
    let mud = st.tint;
    let mut out = [0.0f32; 3];
    for c in 0..3 {
        // darken toward 60% of the mud tint as grime rises
        out[c] = base[c] + (mud[c] * 0.6 - base[c]) * applied;
    }
    out
}

fn glf(x: f32) -> String {
    let v = if x.is_finite() { x } else { 0.0 };
    format!("{:.6}", v)
}

/// Emit a fragment shader (fullscreen `present.vert` convention) that darkens and
/// roughens a creature's lower surfaces by a grime amount. `grime`/`tint` arrive
/// via a push-constant; `v_uv.y` stands in for surface down-ness (bottom = belly)
/// in the swatch. Floats are fixed-point (no inf/nan/exponent) so it validates.
pub fn grime_frag_glsl() -> String {
    let mut s = String::new();
    s.push_str("#version 450\n");
    s.push_str("// sim::creature_grime::grime_frag_glsl (S1) — belly/feet dirt darkening.\n");
    s.push_str("layout(location = 0) in vec2 v_uv;\n");
    s.push_str("layout(location = 0) out vec4 out_color;\n");
    s.push_str("layout(push_constant) uniform Pc { float grime; vec4 tint; } pc;\n");
    s.push_str(&format!("const vec3 BASE = vec3({}, {}, {});\n", glf(0.55), glf(0.50), glf(0.45)));
    s.push_str(&format!("const float BACK_FLOOR = {};\n", glf(BACK_FLOOR)));
    s.push_str("void main() {\n");
    s.push_str("    float downness = clamp(v_uv.y, 0.0, 1.0); // bottom of swatch = belly/feet\n");
    s.push_str("    float applied = clamp(pc.grime, 0.0, 1.0) * mix(BACK_FLOOR, 1.0, downness);\n");
    s.push_str("    vec3 dirty = mix(BASE, pc.tint.rgb * 0.6, applied);\n");
    s.push_str("    float rough = mix(0.30, 0.90, applied); // roughness for the material layer\n");
    s.push_str("    out_color = vec4(dirty, rough);\n");
    s.push_str("}\n");
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn grime_rises_with_mud_activity_and_washes_with_water() {
        let mut g = GrimeState::clean();
        let muddy = GrimeSample { soil_grime01: 1.0, activity01: 1.0, wetness01: 0.0, dt: 0.5, ..Default::default() };
        for _ in 0..10 {
            g.accumulate(&muddy);
        }
        let dirty = g.grime_factor01();
        assert!(dirty > 0.5, "should get dirty tromping through mud: {}", dirty);
        // now wade through water
        let wet = GrimeSample { soil_grime01: 0.0, activity01: 0.2, wetness01: 1.0, dt: 0.5, ..Default::default() };
        for _ in 0..10 {
            g.accumulate(&wet);
        }
        assert!(g.grime_factor01() < dirty, "water should wash grime off");
    }

    #[test]
    fn grime_is_bounded_and_nan_safe() {
        let mut g = GrimeState::clean();
        let hostile = GrimeSample {
            soil_grime01: 9.0,
            activity01: f32::NAN,
            wetness01: -3.0,
            soil_tint: [2.0, -1.0, f32::INFINITY],
            dt: f32::NAN,
        };
        for _ in 0..100 {
            g.accumulate(&hostile);
        }
        assert!((0.0..=1.0).contains(&g.grime_factor01()));
        assert!(g.tint().iter().all(|c| c.is_finite() && (0.0..=1.0).contains(c)));
    }

    #[test]
    fn belly_gets_more_than_back() {
        assert!(belly_weight(-1.0) > belly_weight(1.0), "belly (down) > back (up)");
        assert!((belly_weight(-1.0) - 1.0).abs() < 1e-6, "straight-down = full grime");
        assert!((belly_weight(1.0) - BACK_FLOOR).abs() < 1e-6, "straight-up = back floor");
        for ny in [-1.0f32, -0.5, 0.0, 0.5, 1.0] {
            let w = belly_weight(ny);
            assert!((BACK_FLOOR..=1.0).contains(&w));
        }
    }

    #[test]
    fn apply_grime_darkens_belly_more_than_back() {
        let mut g = GrimeState::clean();
        for _ in 0..30 {
            g.accumulate(&GrimeSample { soil_grime01: 1.0, activity01: 1.0, dt: 0.5, soil_tint: [0.3, 0.2, 0.1], ..Default::default() });
        }
        let base = [0.7, 0.65, 0.6];
        let belly = apply_grime_color(base, -1.0, &g);
        let back = apply_grime_color(base, 1.0, &g);
        let lum = |c: [f32; 3]| c[0] + c[1] + c[2];
        assert!(lum(belly) < lum(back), "belly should be darker (dirtier) than back");
        // a clean creature is unchanged
        let clean = GrimeState::clean();
        assert_eq!(apply_grime_color(base, -1.0, &clean), base);
    }

    #[test]
    fn deterministic() {
        let mut a = GrimeState::clean();
        let mut b = GrimeState::clean();
        let s = GrimeSample { soil_grime01: 0.7, activity01: 0.8, dt: 0.1, ..Default::default() };
        for _ in 0..50 {
            a.accumulate(&s);
            b.accumulate(&s);
        }
        assert_eq!(a, b);
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
    fn grime_frag_validates_as_spirv() {
        let glslang = match tool("glslangValidator") {
            Some(g) => g,
            None => {
                eprintln!("glslangValidator absent — skipping SPIR-V check");
                return;
            }
        };
        let dir = std::env::temp_dir();
        let frag = dir.join("s1_grime.frag");
        let spv = dir.join("s1_grime.spv");
        std::fs::write(&frag, grime_frag_glsl()).expect("write");
        let out = Command::new(&glslang).arg("-V").arg(&frag).arg("-o").arg(&spv).output().expect("glslang");
        assert!(out.status.success(), "grime frag rejected:\n{}\n{}", String::from_utf8_lossy(&out.stdout), grime_frag_glsl());
        if let Some(sv) = tool("spirv-val") {
            let v = Command::new(sv).arg(&spv).output().expect("spirv-val");
            assert!(v.status.success(), "spirv-val: {}", String::from_utf8_lossy(&v.stderr));
        }
    }
}
