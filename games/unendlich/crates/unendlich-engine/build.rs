//! Build script that compiles GLSL shaders to SPIR-V at build time.
//!
//! Behaviour:
//! * Computes a SHA-256 of the GLSL source. If a matching `.spv.hash`
//!   sidecar file exists next to the SPIR-V, skip recompilation.
//! * Otherwise, try `glslc` and then `glslangValidator`. On success, write
//!   the new `.spv` AND the hash sidecar.
//! * If neither compiler is installed, emit one combined warning instead
//!   of one per shader. The crate ships pre-compiled SPIR-V so the build
//!   still succeeds — BUT the user MUST commit any updated SPV when GLSL
//!   changes ship.
//!
//! ### Why content hash, not mtime
//!
//! This module used to compare GLSL mtime vs SPV mtime. That check
//! returned "no recompile needed" whenever the two files had equal
//! mtimes, which is exactly the case after `unzip` (which preserves
//! archive mtimes and frequently produces identical timestamps for
//! files generated in the same `zip` run). Result: a ship that bundles
//! both updated GLSL and stale SPV would leave the stale SPV in place
//! after a fresh unzip + `cargo build`, and the user would run with
//! mismatched shaders. The v0.24.20 vertex-pack ship hit exactly this:
//! pre-pack `.spv` survived against post-pack `.glsl`, the pipeline
//! delivered packed 8-byte data to a shader that read 24-byte
//! unpacked vertices, every vertex decoded into `[0,1]³` near origin,
//! and the entire world rendered as an invisible speck below the player.
//!
//! Content hashing is bullet-proof against this. SPV size + bytes are
//! deterministic given the same GLSL + same compiler version; a hash
//! match means the SPV definitely came from the current GLSL.

use std::path::Path;
use std::process::Command;
use std::fs;

// Shared with src/shader_artifacts.rs so the build pipeline and the freshness
// test hash identically (transitive #include closure). See that file.
include!("shader_hash_shared.rs");

const SHADERS: &[&str] = &[
    "src/shaders/chunk_generation.comp",
    "src/shaders/chunk_mesh.comp",
    "src/shaders/chunk_mesh_sparse.comp",
    "src/shaders/chunk_mesh_runs.comp",
    "src/shaders/chunk_cull.comp",
    "src/shaders/view_compute.comp",
    "src/shaders/cascade_fit.comp",
    "src/shaders/triangle.vert",
    "src/shaders/triangle.frag",
    "src/shaders/chunk.vert",
    "src/shaders/chunk.frag",
    // r305: neural G-buffer producer (MRT variant of chunk.frag; reuses chunk.vert).
    "src/shaders/chunk_gbuffer.frag",
    "src/shaders/chunk_shadow.vert",
    "src/shaders/froxel_fill.comp",
    // These four ship a committed .spv but were previously absent from this
    // list, so build.rs never hash-checked them — editing the source would
    // silently leave a STALE .spv behind (the exact mechanism behind the old
    // worldgen striping). Now hash-managed like the rest.
    "src/shaders/chunk_mesh_octree.comp",
    "src/shaders/octree_probe.comp",
    // r301: neural deferred shading (forward-ported from r67 integration).
    "src/shaders/neural_feat_octree.comp",
    "src/shaders/neural_shade.comp",
    // r303: lean GEMM ship-path shaders (linear_q8_0 GEMM + silu + 14-feat lean pass).
    "src/shaders/linear_q8_0.comp",
    "src/shaders/silu.comp",
    "src/shaders/neural_feat_lean.comp",
    // r307: image-G-buffer feature pass (samples the MRT G-buffer images).
    "src/shaders/neural_feat_gbuf.comp",
    "src/shaders/entity.vert",
    "src/shaders/entity.frag",
    "src/shaders/ui_downsample.comp",
    "src/shaders/ui_blur.comp",
    "src/shaders/egui.vert",
    "src/shaders/egui.frag",
    // R32 GPU offload of the weather/moisture field step (sim::field_diffuse_ref).
    "src/shaders/field_diffuse.comp",
    // r68-05: post stack (tonemap/bloom) was hash-unmanaged — editing tonemap.glsl /
    // present.frag / bloom_*.frag left their committed .spv stale. Now hash-managed.
    "src/shaders/present.vert",
    "src/shaders/present.frag",
    "src/shaders/bloom_prefilter.frag",
    "src/shaders/bloom_blur.frag",
    // r68-07: analytic atmospheric sky pass (fullscreen triangle on the far plane).
    "src/shaders/sky.vert",
    "src/shaders/sky.frag",
];

fn main() {
    // Track each shader file individually. A directory-only
    // `rerun-if-changed` watches the dir entry, not the contents, on
    // some filesystems — being explicit about every file removes that
    // ambiguity.
    for src in SHADERS {
        println!("cargo:rerun-if-changed={}", src);
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=UNENDLICH_STRICT_SHADERS");

    let out_dir = Path::new("../../assets/shaders");
    fs::create_dir_all(out_dir).ok();

    let mut tried_to_compile = false;
    let mut compiler_missing = false;
    let mut compiles_failed: Vec<String> = Vec::new();
    let mut stale_present: Vec<String> = Vec::new();

    for src in SHADERS {
        let src_path = Path::new(src);
        if !src_path.exists() {
            println!("cargo:warning=Shader source missing: {}", src);
            continue;
        }
        let out_name = format!("{}.spv", src_path.file_name().unwrap().to_string_lossy());
        let out_path = out_dir.join(&out_name);
        let hash_path = out_dir.join(format!("{}.hash", out_name));

        // Compute current GLSL content hash.
        let _src_bytes = match fs::read(src_path) {
            Ok(b) => b,
            Err(e) => {
                println!("cargo:warning=Failed to read {}: {}", src, e);
                continue;
            }
        };
        let want_hash = transitive_source_hash(src_path);

        // If sidecar hash matches current GLSL AND the SPV exists, skip.
        let have_hash = fs::read_to_string(&hash_path).ok().map(|s| s.trim().to_string());
        let spv_ok = out_path.exists();
        if spv_ok && have_hash.as_deref() == Some(want_hash.as_str()) {
            continue;
        }

        tried_to_compile = true;
        match try_compile(src, out_path.to_str().unwrap()) {
            CompileResult::Ok => {
                // Write the hash sidecar so the next build is a no-op.
                if let Err(e) = fs::write(&hash_path, &want_hash) {
                    println!("cargo:warning=Failed to write hash sidecar {}: {}",
                        hash_path.display(), e);
                }
            }
            CompileResult::Failed(msg) => {
                compiles_failed.push(format!("{}: {}", src, msg));
            }
            CompileResult::NoCompiler => {
                compiler_missing = true;
                // If the shipped SPV is present but its hash doesn't
                // match the current GLSL, we're definitely stale — flag
                // it so the user can install a compiler or accept the
                // risk knowingly. If the SPV is simply missing, that's
                // the existing "compiler needed" case.
                if spv_ok {
                    stale_present.push(out_name);
                }
                break; // No point trying the rest.
            }
        }
    }

    if compiler_missing && tried_to_compile {
        if !stale_present.is_empty() {
            // STALE-SPV WARNING: the shipped SPV does not match the
            // shipped GLSL. Running with these binaries WILL produce
            // wrong rendering (see v0.24.20-vertex-pack's "every
            // vertex at origin" failure mode). Install a compiler and
            // rebuild.
            println!("cargo:warning=╔════════════════════════════════════════════════════════════╗");
            println!("cargo:warning=║  SHIPPED SPIR-V DOES NOT MATCH SHIPPED GLSL  (build.rs)    ║");
            println!("cargo:warning=╠════════════════════════════════════════════════════════════╣");
            println!("cargo:warning=║  Stale SPVs detected. Running the binary as-is will give   ║");
            println!("cargo:warning=║  wrong rendering (e.g. blank screen if vertex/cull SPVs    ║");
            println!("cargo:warning=║  are stale). Install a shader compiler and rebuild:        ║");
            println!("cargo:warning=║    Fedora / Asahi:  sudo dnf install glslang spirv-tools   ║");
            println!("cargo:warning=║    Ubuntu/Debian:   sudo apt install glslang-tools         ║");
            println!("cargo:warning=║    macOS:           brew install glslang                   ║");
            println!("cargo:warning=╚════════════════════════════════════════════════════════════╝");
            for name in &stale_present {
                println!("cargo:warning=  stale: {}", name);
            }
            // Opt-in CI/dev guard: refuse to build with stale SPIR-V rather than
            // shipping a binary that renders wrong (the worldgen-striping class).
            // Off by default so the no-compiler ship-spv path still works.
            if std::env::var("UNENDLICH_STRICT_SHADERS").is_ok() {
                panic!(
                    "UNENDLICH_STRICT_SHADERS set and {} shader(s) have a stale .spv \
                     (source hash != sidecar). Install glslang/spirv-tools and rebuild.",
                    stale_present.len()
                );
            }
        } else if any_spv_missing(out_dir) {
            println!("cargo:warning=GLSL shaders need compiling but no shader compiler is installed.");
            println!("cargo:warning=  Fedora / Asahi:  sudo dnf install glslang spirv-tools");
            println!("cargo:warning=  Ubuntu/Debian:   sudo apt install glslang-tools spirv-tools");
            println!("cargo:warning=  macOS:           brew install glslang");
        }
        // else: pre-compiled SPIR-V exists, compiler missing — silent.
    }

    for fail in compiles_failed {
        println!("cargo:warning=Shader compile failed: {}", fail);
    }
}

/// SHA-256 of input bytes, hex-encoded. We pull this in via a tiny
/// hand-rolled implementation rather than adding a crate dep — build
/// scripts get their own dependency tree and we want this script to
/// stay fast (single-file, no compile of a hash crate).

fn any_spv_missing(out_dir: &Path) -> bool {
    SHADERS.iter().any(|src| {
        let name = format!("{}.spv", Path::new(src).file_name().unwrap().to_string_lossy());
        !out_dir.join(name).exists()
    })
}

enum CompileResult {
    Ok,
    Failed(String),
    NoCompiler,
}

fn try_compile(src: &str, out: &str) -> CompileResult {
    // Try glslc first.
    match Command::new("glslc").args(&["--target-env=vulkan1.2", src, "-o", out]).output() {
        Ok(o) if o.status.success() => return CompileResult::Ok,
        Ok(o) => {
            // glslc ran but rejected the shader.
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            return CompileResult::Failed(stderr);
        }
        Err(_) => { /* not installed — try fallback */ }
    }

    match Command::new("glslangValidator").args(&["-V", "--target-env", "vulkan1.2", src, "-o", out]).output() {
        Ok(o) if o.status.success() => CompileResult::Ok,
        Ok(o) => {
            let stderr = String::from_utf8_lossy(&o.stderr).to_string();
            CompileResult::Failed(stderr)
        }
        Err(_) => CompileResult::NoCompiler,
    }
}
