//! Shader contract checker — the runnable "compile-checker" for texturing.
//!
//! We can't compile GLSL→SPIR-V here, but the bugs that actually bite in this
//! kind of work are *convention drift* between the CPU and the shader: a constant
//! changed on one side only, a vertex attribute grown on one side only, or a
//! shader that samples a descriptor binding the pipeline never declared (an
//! instant device-lost / validation error on the machine). Those are all visible
//! in the shader *source* as text, so this module reads the real shader files and
//! checks them against the [`super::texture_mapping`] ruleset.
//!
//! It runs as a tool: `cargo test -p unendlich-engine --no-default-features
//! shader_contract` — identical to invoke here and on the machine — and
//! [`check_world_shaders`] returns the list of violations for a CLI to print.
//!
//! Scope today: the world block shaders (chunk.vert / chunk.frag). As texturing
//! lands, new expectations (the texture-array sampler binding, the alpha-id
//! decode line) slot into the tables below and the check enforces both sides move
//! together.

use super::texture_mapping::CHUNK_EXTENT;

/// A single contract violation, human-readable for a CLI or a failing test.
pub type Violation = String;

fn shader_src(name: &str) -> Option<String> {
    let path = format!("{}/src/shaders/{}", env!("CARGO_MANIFEST_DIR"), name);
    std::fs::read_to_string(path).ok()
}

/// Whitespace-stripped copy, so matches don't depend on GLSL formatting.
fn squeeze(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}

/// Every `binding = N` mentioned in the source (set 0 is the only set the world
/// shaders use). Returns them sorted/deduped.
fn bindings_used(src: &str) -> Vec<u32> {
    let sq = squeeze(src);
    let mut out: Vec<u32> = Vec::new();
    for part in sq.split("binding=").skip(1) {
        let digits: String = part.chars().take_while(|c| c.is_ascii_digit()).collect();
        if let Ok(n) = digits.parse::<u32>() {
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    out.sort_unstable();
    out
}

/// `(location, name)` for each *vertex input* (`... in <type> <name>`) declared in
/// a vertex shader. Excludes inter-stage `out`/`in` varyings by requiring the
/// `layout(location=N) in <type> <name>;` shape with a recognizable attribute
/// name; we match the two world-vertex attribute names explicitly.
fn vertex_inputs(vert_src: &str) -> Vec<(u32, String)> {
    let mut out = Vec::new();
    for line in vert_src.lines() {
        let l = line.trim();
        if !(l.starts_with("layout(location") && l.contains(" in ")) {
            continue;
        }
        let sq = squeeze(l);
        // ...location=N)invec4 name;
        let n = sq
            .split("location=")
            .nth(1)
            .and_then(|p| p.chars().take_while(|c| c.is_ascii_digit()).collect::<String>().parse::<u32>().ok());
        // name is the last identifier before the ';'
        let name = l
            .trim_end_matches(';')
            .rsplit(|c: char| c.is_whitespace())
            .next()
            .unwrap_or("")
            .to_string();
        if let Some(n) = n {
            out.push((n, name));
        }
    }
    out.sort_by_key(|(n, _)| *n);
    out
}

/// The checks, as data, so a CLI and the tests share one source of truth.
///
/// `expected_bindings`: the set 0 descriptor bindings the renderer declares; a
/// shader using a binding outside this set is a crash waiting to happen.
const EXPECTED_BINDINGS: &[u32] = &[0, 1, 2, 3];
/// `expected_vertex_inputs`: the packed vertex attributes. The alpha-id texturing
/// path deliberately does NOT add an attribute, so this stays at two.
const EXPECTED_VERTEX_INPUTS: &[(u32, &str)] = &[(0, "in_pos_packed"), (1, "in_color_unorm")]; // r349: R32_UINT manual unpack
/// Face-UV reconstruction (texturing increment 1): the fragment shader must derive
/// per-face UVs on the SAME face-tangent axes as `texture_mapping::face_uv`, so the
/// shader and the CPU reference can't drift on which world axes map to U/V. These
/// are the whitespace-stripped expressions the frag must contain.
const FACE_UV_EXPRS: &[&str] = &[
    "fract(vec2(v_world_pos.z,v_world_pos.y))", // X faces -> tangents Z,Y
    "fract(vec2(v_world_pos.x,v_world_pos.z))", // Y faces -> tangents X,Z
    "fract(vec2(v_world_pos.x,v_world_pos.y))", // Z faces -> tangents X,Y
];

/// Run every check against the world block shaders. Empty result == contract holds.
pub fn check_world_shaders() -> Vec<Violation> {
    let mut v = Vec::new();

    let vert = match shader_src("chunk.vert") {
        Some(s) => s,
        None => {
            v.push("chunk.vert: could not read source".into());
            return v;
        }
    };
    let frag = match shader_src("chunk.frag") {
        Some(s) => s,
        None => {
            v.push("chunk.frag: could not read source".into());
            return v;
        }
    };

    // 1. Shared constant: CHUNK_EXTENT must match the ruleset.
    let want = format!("CHUNK_EXTENT={:.1}", CHUNK_EXTENT);
    if !squeeze(&vert).contains(&want) {
        v.push(format!("chunk.vert: missing or mismatched constant ({want})"));
    }

    // 2. Vertex inputs must match the declared packed-vertex layout exactly.
    let got = vertex_inputs(&vert);
    let want: Vec<(u32, String)> =
        EXPECTED_VERTEX_INPUTS.iter().map(|(n, s)| (*n, s.to_string())).collect();
    if got != want {
        v.push(format!("chunk.vert: vertex inputs {got:?} != expected {want:?}"));
    }

    // 3. No shader may sample a descriptor binding the pipeline never declared.
    for src_name in ["chunk.vert", "chunk.frag"] {
        let src = if src_name == "chunk.vert" { &vert } else { &frag };
        for b in bindings_used(src) {
            if !EXPECTED_BINDINGS.contains(&b) {
                v.push(format!(
                    "{src_name}: uses descriptor binding {b} not in the declared set {EXPECTED_BINDINGS:?}"
                ));
            }
        }
    }

    // 4. Face-UV reconstruction must use the ruleset's face-tangent axes.
    let frag_sq = squeeze(&frag);
    for expr in FACE_UV_EXPRS {
        if !frag_sq.contains(expr) {
            v.push(format!(
                "chunk.frag: missing face-UV reconstruction `{expr}` (must match texture_mapping::face_uv axes)"
            ));
        }
    }

    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn current_world_shaders_satisfy_the_contract() {
        let violations = check_world_shaders();
        assert!(violations.is_empty(), "contract violations: {violations:#?}");
    }

    #[test]
    fn parser_reads_expected_bindings_from_chunk_frag() {
        // chunk.frag binds the camera UBO (0), shadow sampler (1), froxel (2).
        let frag = shader_src("chunk.frag").expect("chunk.frag present");
        let b = bindings_used(&frag);
        for expect in [0u32, 1, 2] {
            assert!(b.contains(&expect), "expected binding {expect} in {b:?}");
        }
    }

    #[test]
    fn parser_reads_the_two_vertex_inputs() {
        let vert = shader_src("chunk.vert").expect("chunk.vert present");
        let inputs = vertex_inputs(&vert);
        assert_eq!(
            inputs,
            vec![(0u32, "in_pos_packed".to_string()), (1u32, "in_color_unorm".to_string())]
        );
    }

    // The checker must actually CATCH drift, not just pass on good input. These
    // exercise the detection logic on synthetic sources.
    #[test]
    fn detects_an_undeclared_binding() {
        let bogus = "layout(set=0, binding = 7) uniform sampler2D tex;";
        let used = bindings_used(bogus);
        assert_eq!(used, vec![7]);
        assert!(!EXPECTED_BINDINGS.contains(&7), "7 would be flagged as undeclared");
    }

    #[test]
    fn detects_a_grown_or_renamed_vertex_input() {
        let grown = "layout(location = 0) in vec4 in_pos_unorm;\n\
                     layout(location = 1) in vec4 in_color_unorm;\n\
                     layout(location = 2) in vec2 in_uv;\n";
        let got = vertex_inputs(grown);
        let want: Vec<(u32, String)> =
            EXPECTED_VERTEX_INPUTS.iter().map(|(n, s)| (*n, s.to_string())).collect();
        assert_ne!(got, want, "a third attribute must be detected as drift");
    }

    #[test]
    fn frag_reconstructs_face_uv_on_ruleset_axes() {
        // The increment-1 frag must derive UVs on the same axes as the ruleset.
        let frag = shader_src("chunk.frag").expect("chunk.frag present");
        let sq = squeeze(&frag);
        for expr in FACE_UV_EXPRS {
            assert!(sq.contains(expr), "chunk.frag missing face-UV expr {expr}");
        }
    }

    #[test]
    fn squeeze_makes_constant_match_formatting_independent() {
        assert!(squeeze("const float CHUNK_EXTENT = 32.0 ;").contains("CHUNK_EXTENT=32.0"));
    }
}
