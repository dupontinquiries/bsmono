//! Every shader clawbound ships parses and type-checks.
//!
//! ## Why this is a test and not a build step
//!
//! `cargo check` does not compile WGSL. A malformed shader is a valid `&str`
//! until `create_shader_module`, so it surfaces as a panic on the first frame
//! that dispatches it — which for this crate means launching the game, on a
//! machine with a GPU, and getting far enough in to draw.
//!
//! That gap is not hypothetical here. The AAA post-processing fix
//! (`2912cde3`) edited `POST_WGSL` and `cargo check` had nothing to say about
//! it; the only way to know the edit was even syntactically sound was to
//! extract the string by hand and run naga over it. This makes that automatic.
//!
//! ## Why it reads main.rs instead of importing the consts
//!
//! `SCENE_WGSL` and `POST_WGSL` are private to `src/main.rs`, and an
//! integration test cannot see a binary's private items. Making them `pub` in
//! the library purely to test them would move production code to suit a test.
//!
//! Reading the source is the smaller lie, and it has a real advantage: it
//! validates the bytes that are actually in the file, so it cannot be fooled by
//! a const that compiles but is no longer the one the pipeline uses.
//!
//! The extraction is structural — `const NAME: &str = r#"` through `"#;` — and
//! the test asserts it found BOTH shaders. A scanner that silently matches
//! nothing passes every check ever written against it, which is the specific
//! way a gate like this rots.

/// Pull every `const NAME: &str = r#"..."#;` out of a Rust source.
fn raw_string_consts(src: &str) -> Vec<(String, String)> {
    const OPEN: &str = ": &str = r#\"";
    let mut out = Vec::new();
    let mut rest = src;
    let mut consumed = 0usize;
    while let Some(i) = rest.find(OPEN) {
        let name = src[..consumed + i]
            .rsplit("const ").next().unwrap_or("?").trim().to_string();
        let after = &rest[i + OPEN.len()..];
        let Some(end) = after.find("\"#;") else { break };
        out.push((name, after[..end].to_string()));
        consumed += i + OPEN.len() + end;
        rest = &after[end..];
    }
    out
}

#[test]
fn every_shader_in_main_is_valid_wgsl() {
    let src = include_str!("../src/main.rs");
    let consts = raw_string_consts(src);

    let shaders: Vec<_> = consts
        .into_iter()
        .filter(|(_, body)| {
            // Only sources with a shader entry point. A crate may keep other
            // things in raw strings, and handing those to naga reports noise
            // as failure.
            body.contains("@compute") || body.contains("@fragment") || body.contains("@vertex")
        })
        .collect();

    let names: Vec<&str> = shaders.iter().map(|(n, _)| n.as_str()).collect();
    assert!(
        names.contains(&"SCENE_WGSL") && names.contains(&"POST_WGSL"),
        "expected SCENE_WGSL and POST_WGSL, found {names:?} — the scan regressed, \
and a scan that finds nothing passes every assertion below it"
    );

    let mut bad = Vec::new();
    for (name, body) in &shaders {
        let module = match naga::front::wgsl::parse_str(body) {
            Ok(m) => m,
            Err(e) => {
                bad.push(format!("{name} — parse:\n{}", e.emit_to_string(body)));
                continue;
            }
        };
        let mut v = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        if let Err(e) = v.validate(&module) {
            bad.push(format!("{name} — validate:\n{e:?}"));
        }
    }

    assert!(
        bad.is_empty(),
        "{} of {} shader(s) failed naga validation:\n\n{}",
        bad.len(), shaders.len(), bad.join("\n\n")
    );
}

/// The post shader's outline taps must be in OUTPUT pixels.
///
/// Pins the AAA fix. `fx.screen.xy` is the SUPERSAMPLED texel (1/scene_w), so
/// at the 1.85x AAA scale those taps step 0.54 of an output pixel and the
/// outline filter runs sub-pixel — measured by `examples/post_probe` as 8.99%
/// of adjacent pixels jumping a quantisation step, against 0.31% when correct.
///
/// Cheap to reintroduce: `fx.screen.xy` reads like the obvious texel size, and
/// it IS the right value for sampling the scene texture — just not for a filter
/// whose output is one pixel wide.
#[test]
fn the_post_outline_taps_are_output_pixels_not_source_texels() {
    let src = include_str!("../src/main.rs");
    let post = raw_string_consts(src)
        .into_iter()
        .find(|(n, _)| n == "POST_WGSL")
        .map(|(_, b)| b)
        .expect("POST_WGSL");

    assert!(
        post.contains("let texel = vec2<f32>(1.0, 1.0) / fx.screen.zw;"),
        "the outline tap is not derived from the OUTPUT size (fx.screen.zw). If it is \
`fx.screen.xy` again, AAA will lay down a dot lattice — see examples/post_probe.rs, \
which measures it."
    );
}
