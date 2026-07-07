//! Architectural checks for the shader build artifacts (rendering / meshing /
//! octree pipelines). The engine ships pre-compiled SPIR-V in `assets/shaders`
//! with a `<name>.spv.hash` sidecar holding the hash of the GLSL source closure.
//! `build.rs` recompiles only when the sidecar disagrees; on a machine without a
//! shader compiler it keeps the committed `.spv` and warns "Stale SPVs detected"
//! — which silently ships WRONG rendering (the build script's own words).
//!
//! These tests turn that footgun into a hard failure: committed SPIR-V must be
//! fresh against its *transitive* source (shader + every `#include`), every
//! source shader must be tracked by build.rs, and every core (non-UI) shader
//! must ship a pre-compiled binary so a compiler-less build is correct.

#![cfg(test)]

use std::path::PathBuf;

// Identical hashing to build.rs (transitive #include closure). One file, two
// include sites, so the build pipeline and this check can never disagree.
include!("../shader_hash_shared.rs");

fn shaders_src_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/shaders")
}
fn spv_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/shaders")
}

/// Source shaders that intentionally ship NO committed SPIR-V (UI-overlay /
/// post-process; only built where a compiler is guaranteed present). A new
/// CORE shader missing its binary must NOT land here silently.
const NO_COMMITTED_SPV_ALLOWED: &[&str] =
    &["egui.vert", "egui.frag", "ui_blur.comp", "ui_downsample.comp"];

fn list_source_shaders() -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(shaders_src_dir()).unwrap()
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| matches!(p.extension().and_then(|x| x.to_str()), Some("comp" | "vert" | "frag")))
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

#[test]
fn committed_spirv_is_fresh_against_transitive_source() {
    let spv = spv_dir();
    let mut stale = Vec::new();
    let mut checked = 0;
    for entry in std::fs::read_dir(&spv).expect("assets/shaders missing") {
        let path = entry.unwrap().path();
        let fname = path.file_name().unwrap().to_string_lossy().into_owned();
        let Some(stripped) = fname.strip_suffix(".spv.hash") else { continue };
        let src = shaders_src_dir().join(stripped);
        assert!(src.exists(), "sidecar {fname} has no source {}", src.display());
        let want = transitive_source_hash(&src);
        let have = std::fs::read_to_string(&path).unwrap().trim().to_string();
        if want != have {
            stale.push(format!("{stripped} (sidecar {}… vs source {}…)",
                &have[..12.min(have.len())], &want[..12]));
        }
        checked += 1;
    }
    assert!(checked > 0, "no .spv.hash sidecars found");
    assert!(stale.is_empty(),
        "stale committed SPIR-V (source or an #include edited, .spv not recompiled) — a \
         compiler-less build will render wrong:\n  {}", stale.join("\n  "));
}

#[test]
fn build_list_tracks_every_source_shader() {
    let build_rs = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("build.rs"))
        .expect("build.rs");
    for name in list_source_shaders() {
        let needle = format!("src/shaders/{name}");
        assert!(build_rs.contains(&needle),
            "{name} exists in src/shaders but is not in build.rs SHADERS[] — it will never be \
             (re)compiled and can silently ship stale");
    }
}

#[test]
fn core_shaders_ship_precompiled_spirv() {
    let spv = spv_dir();
    for name in list_source_shaders() {
        if NO_COMMITTED_SPV_ALLOWED.contains(&name.as_str()) {
            continue;
        }
        let bin = spv.join(format!("{name}.spv"));
        let hash = spv.join(format!("{name}.spv.hash"));
        assert!(bin.exists(),
            "core shader {name} ships no committed {name}.spv — a compiler-less build has no \
             pipeline for it (add it, or allowlist if intentionally UI-only)");
        assert!(hash.exists(), "core shader {name} has a .spv but no .spv.hash sidecar");
    }
}

#[test]
fn transitive_hash_follows_real_includes_only() {
    let dir = shaders_src_dir();
    let mesher = dir.join("chunk_mesh_octree.comp");
    let names: Vec<String> = collect_shader_includes(&mesher).iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert!(names.contains(&"octree_types.glsl".to_string()));
    assert!(names.contains(&"octree_sample.glsl".to_string()));

    // Folding includes in changes the hash vs the top file alone — so editing an
    // include now invalidates the dependent shader's sidecar (the gap this closes).
    let top_only = sha256_hex(&std::fs::read(&mesher).unwrap());
    assert_ne!(transitive_source_hash(&mesher), top_only);

    // A no-include shader hashes identically either way (scheme is backward-compatible).
    let plain = dir.join("triangle.frag");
    assert_eq!(transitive_source_hash(&plain), sha256_hex(&std::fs::read(&plain).unwrap()));

    // Commented example `#include` lines inside the .glsl files must be ignored.
    assert!(collect_shader_includes(&dir.join("octree_sample.glsl")).is_empty(),
        "commented example includes must not be followed");
}
