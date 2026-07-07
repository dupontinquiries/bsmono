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
}
