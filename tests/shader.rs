//! Validates shader.wgsl offline with naga — the exact frontend wgpu uses at
//! runtime — so reserved-keyword or type errors fail `cargo test` instead of
//! panicking on the player's machine.

#[test]
fn wgsl_parses_and_validates() {
    let src = include_str!("../src/shader.wgsl");
    let module = naga::front::wgsl::parse_str(src)
        .unwrap_or_else(|e| panic!("WGSL parse error:\n{}", e.emit_to_string(src)));
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .expect("WGSL validation failed");
}
