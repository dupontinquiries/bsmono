//! Shader compile gate. Compiles every engine GLSL shader to SPIR-V with
//! `glslangValidator` (resolving `#include`s via `-I`) and validates the result
//! with `spirv-val`. This catches a bug class — a shader that won't compile or
//! produces invalid SPIR-V — that otherwise only surfaces on the device at
//! pipeline-creation time.
//!
//! Skips gracefully (does not fail) when the shader toolchain isn't installed,
//! so it's safe to run anywhere. Install on Debian/Ubuntu with:
//! `apt-get install glslang-tools spirv-tools`.

use std::path::PathBuf;
use std::process::Command;

fn shaders_dir() -> PathBuf {
    // crates/vk-harness -> crates/unendlich-engine/src/shaders
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../unendlich-engine/src/shaders")
}

fn tool_present(name: &str) -> bool {
    Command::new(name).arg("--version").output().map(|o| o.status.success()).unwrap_or(false)
}

#[test]
fn every_engine_shader_compiles_to_valid_spirv() {
    if !tool_present("glslangValidator") || !tool_present("spirv-val") {
        eprintln!("skipping: shader toolchain not installed (glslangValidator / spirv-val)");
        return;
    }
    let dir = shaders_dir();
    if !dir.is_dir() {
        eprintln!("skipping: shaders dir not found at {}", dir.display());
        return;
    }

    let include = format!("-I{}", dir.display());
    let out = std::env::temp_dir().join("vk_harness_shader.spv");
    let mut checked = 0;
    let mut failures: Vec<String> = Vec::new();

    let mut entries: Vec<_> = std::fs::read_dir(&dir).expect("read shaders dir")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| matches!(p.extension().and_then(|e| e.to_str()), Some("comp" | "vert" | "frag")))
        .collect();
    entries.sort();

    for path in entries {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        // glslangValidator infers the stage from the file extension.
        let compile = Command::new("glslangValidator")
            .args(["-V", path.to_str().unwrap(), &include, "-o", out.to_str().unwrap()])
            .output()
            .expect("run glslangValidator");
        if !compile.status.success() {
            failures.push(format!("{name}: glslang: {}",
                String::from_utf8_lossy(&compile.stdout).lines().find(|l| l.to_lowercase().contains("error"))
                    .unwrap_or("compile failed").trim()));
            continue;
        }
        let validate = Command::new("spirv-val").arg(&out).output().expect("run spirv-val");
        if !validate.status.success() {
            failures.push(format!("{name}: spirv-val: {}",
                String::from_utf8_lossy(&validate.stderr).lines().next().unwrap_or("invalid").trim()));
            continue;
        }
        checked += 1;
    }

    eprintln!("shaders validated: {checked}");
    assert!(checked > 0, "no shaders were checked — directory empty or unreadable");
    assert!(failures.is_empty(), "shader validation failures:\n  {}", failures.join("\n  "));
}
