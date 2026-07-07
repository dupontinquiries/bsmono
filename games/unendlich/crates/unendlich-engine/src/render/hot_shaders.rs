//! Hot-shader asset resolution. Only compiled under the `hot-shaders` feature.
//!
//! The hot-iteration path reads `.spv` bytecode from disk at pipeline-creation
//! time so GLSL edits take effect on restart with no Rust recompile. It used to
//! read `assets/shaders/<name>` *relative to the process CWD*, which panicked
//! whenever the binary was launched from anywhere but the repo root — notably
//! when the launcher runs the cached binary from its `versions/<commit>/` dir
//! (the spawn there sets no `current_dir`, and nothing stages `assets/` beside
//! the binary).
//!
//! This resolves a stable location instead. Order, first hit wins:
//!   1. `$UNENDLICH_ASSETS/<name>` — explicit runtime override.
//!   2. `<CARGO_MANIFEST_DIR>/../../assets/shaders/<name>` — baked at build time.
//!      Points at the repo's `assets/shaders` regardless of CWD or where the
//!      binary was copied to. Reliable because `hot-shaders` is a dev-only
//!      feature: the binary is built and run on the same machine, and the build
//!      dir (where `build.rs` writes the `.spv`) doesn't move.
//!   3. `assets/shaders/<name>` — legacy CWD-relative fallback.

use std::path::PathBuf;

/// Candidate paths for a shader `.spv`, in priority order.
pub fn spv_candidates(name: &str) -> Vec<PathBuf> {
    let mut v = Vec::new();
    if let Ok(dir) = std::env::var("UNENDLICH_ASSETS") {
        v.push(PathBuf::from(dir).join(name));
    }
    // CARGO_MANIFEST_DIR is the engine crate root at build time:
    // <repo>/crates/unendlich-engine  ->  ../../assets/shaders
    v.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../assets/shaders").join(name));
    v.push(PathBuf::from("assets/shaders").join(name));
    v
}

/// Read a shader `.spv` by file name (e.g. `"chunk.vert.spv"`), trying each
/// candidate in order. Panics with the full candidate list if none is readable,
/// so a misconfiguration tells you exactly where it looked.
pub fn read_spv(name: &str) -> Vec<u8> {
    let candidates = spv_candidates(name);
    for p in &candidates {
        if let Ok(bytes) = std::fs::read(p) {
            return bytes;
        }
    }
    let tried = candidates
        .iter()
        .map(|p| format!("  - {}", p.display()))
        .collect::<Vec<_>>()
        .join("\n");
    panic!("hot-shaders: could not read shader '{}'. Tried:\n{}", name, tried);
}
