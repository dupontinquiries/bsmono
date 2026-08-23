//! The shader this crate ships parses and type-checks.
//!
//! `cargo check` does NOT compile WGSL. A malformed shader is a valid `&str`
//! until `create_shader_module`, so it surfaces as a panic on the first frame
//! that dispatches it — on a machine with a GPU, far from the edit.
//!
//! Validated with naga 0.19, the version this crate's own wgpu resolves. That
//! pinning is the reason this test lives here instead of in a single
//! `maintenance/tools/` checker: one was written and deleted after it reported
//! 14 failures in gpu-gemm shaders that run perfectly, because naga 29 accepts
//! `fn f16` and naga 22 rejects it as a reserved word. This tree runs four naga
//! versions at once. A shader must be judged by the parser that will actually
//! compile it, so the duplication is load-bearing rather than sloppy.
#[test]
fn the_shader_parses_and_validates() {
    let src = include_str!("../src/phase_corr.wgsl");
    let module = naga::front::wgsl::parse_str(src)
        .unwrap_or_else(|e| panic!("WGSL parse error:\n{}", e.emit_to_string(src)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .unwrap_or_else(|e| panic!("WGSL validation error: {e:?}"));
}
