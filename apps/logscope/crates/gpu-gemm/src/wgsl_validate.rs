//! wgsl_validate.rs — the in-process WGSL fitness gate.
//!
//! A shader artifact is validated with naga (parse → validate) entirely in-process: no GPU, no subprocess,
//! no toolchain to install, runs in milliseconds and anywhere. This is the cheapest, most ruthless cull in
//! the whole cascade — a branch that emits malformed WGSL dies instantly, before a single token is spent
//! judging it. And because kernels can be checked against a CPU reference (see `gpu-gemm`'s existing kernel
//! validation), WGSL is the swarm's dream fitness target: objective, cheap, deterministic.

/// Parse + validate a WGSL source. `Ok(())` means it's a well-formed, type-correct shader module; `Err`
/// carries a human-readable diagnostic (with source spans) suitable to feed straight back to the model as
/// the retry signal.
pub fn validate_wgsl(src: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(src)
        .map_err(|e| format!("WGSL parse error:\n{}", e.emit_to_string(src)))?;
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    validator
        .validate(&module)
        .map_err(|e| format!("WGSL validation error:\n{}", e.emit_to_string(src)))?;
    Ok(())
}

/// Convenience: validate and collapse to pass/fail + message, matching the shape verifiers report.
pub fn check_wgsl(src: &str) -> (bool, String) {
    match validate_wgsl(src) {
        Ok(()) => (true, String::new()),
        Err(e) => (false, e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
        @group(0) @binding(0) var<storage, read> input: array<f32>;
        @group(0) @binding(1) var<storage, read_write> output: array<f32>;
        @compute @workgroup_size(64)
        fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
            let i = gid.x;
            if (i < arrayLength(&input)) {
                output[i] = input[i] * 2.0;
            }
        }
    "#;

    #[test]
    fn accepts_a_valid_compute_shader() {
        assert!(validate_wgsl(GOOD).is_ok(), "{:?}", validate_wgsl(GOOD));
        assert_eq!(check_wgsl(GOOD), (true, String::new()));
    }

    #[test]
    fn rejects_a_syntax_error_with_a_diagnostic() {
        // missing semicolon / bad token
        let bad = "@compute @workgroup_size(64) fn main() { let x = ; }";
        let (ok, msg) = check_wgsl(bad);
        assert!(!ok, "syntax error should fail the gate");
        assert!(msg.contains("WGSL parse error"), "diagnostic mentions the parse phase: {msg}");
    }

    #[test]
    fn rejects_a_type_error_that_parses_but_wont_validate() {
        // parses fine, but assigns a vec to a scalar — caught at validation, not parse
        let bad = r#"
            @compute @workgroup_size(1)
            fn main() {
                var x: f32 = vec2<f32>(1.0, 2.0);
            }
        "#;
        let (ok, msg) = check_wgsl(bad);
        assert!(!ok, "type mismatch should fail validation");
        assert!(!msg.is_empty());
    }

    /// Every shader this crate ships parses and type-checks.
    ///
    /// `cargo check` does NOT compile WGSL. A malformed shader is a perfectly
    /// good `&str` right up to `create_shader_module`, so it surfaces as a
    /// panic on the first frame that dispatches it — which, for a kernel behind
    /// a rarely-taken branch, can be a long way from the edit.
    ///
    /// This crate has had `validate_wgsl` all along and never pointed it at its
    /// own shaders: its three tests check the validator, not the payload. Naga
    /// runs in milliseconds with no GPU, so there is no reason for the gap.
    ///
    /// Covers both populations, because they are different risks:
    ///   * the inline `r#"..."#` consts in lib.rs — what the engine ACTUALLY
    ///     dispatches today;
    ///   * `src/kernels/*.wgsl` — the 2026-07-26 extraction, one of which lib.rs
    ///     already `include_str!`s directly, and the rest of which become live
    ///     the moment that extraction is completed. A syntax error parked there
    ///     would surface as a regression in whichever commit finishes the job.
    #[test]
    fn every_shader_this_crate_ships_is_valid_wgsl() {
        let mut checked = 0usize;
        let mut bad: Vec<String> = Vec::new();

        // ── inline consts in lib.rs ──
        //
        // Matched structurally rather than by name: `const NAME: &str = r#"`
        // through the closing `"#;`. Only sources carrying a shader entry point
        // are validated — this crate also keeps help text and JSON in raw
        // strings, and feeding those to naga would report noise as failure.
        let lib = include_str!("lib.rs");
        let mut rest = lib;
        while let Some(i) = rest.find(": &str = r#\"") {
            let after = &rest[i + 12..];
            let Some(end) = after.find("\"#;") else { break };
            let body = &after[..end];
            // name, for the diagnostic: walk back to `const `
            let head = &rest[..i];
            let name = head.rsplit("const ").next().unwrap_or("?").trim().to_string();
            rest = &after[end..];

            let is_shader = body.contains("@compute") || body.contains("@fragment")
                || body.contains("@vertex");
            if !is_shader { continue }
            checked += 1;
            if let Err(e) = validate_wgsl(body) {
                bad.push(format!("lib.rs const {name}:\n{e}"));
            }
        }
        assert!(checked > 20, "only found {checked} inline shaders; the scan regressed");

        // ── the extracted .wgsl files ──
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/kernels");
        let mut files = 0usize;
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let path = e.path();
                if path.extension().and_then(|x| x.to_str()) != Some("wgsl") { continue }
                let Ok(src) = std::fs::read_to_string(&path) else { continue };
                files += 1;
                if let Err(err) = validate_wgsl(&src) {
                    bad.push(format!("{}:\n{err}", path.file_name().unwrap().to_string_lossy()));
                }
            }
        }
        assert!(files > 50, "only found {files} .wgsl files; expected ~90");

        assert!(
            bad.is_empty(),
            "{} of {} shader(s) failed naga validation:\n\n{}",
            bad.len(), checked + files, bad.join("\n\n")
        );
        eprintln!("validated {checked} inline + {files} file shaders");
    }

}
