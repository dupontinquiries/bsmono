// Offline WGSL validation: every shader must parse and pass naga's validator
// with the same naga wgpu 0.19 uses, so a shader typo fails `cargo test`
// instead of panicking at pipeline creation on the player's machine.
use naga::valid::{Capabilities, ValidationFlags, Validator};

fn check(name: &str, src: &str) {
    let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{name}: parse error:\n{}", e.emit_to_string(src)));
    let mut v = Validator::new(ValidationFlags::all(), Capabilities::all());
    v.validate(&module).unwrap_or_else(|e| panic!("{name}: validation error: {e:?}"));
}

#[test]
fn all_shaders_validate() {
    check("voxel", include_str!("../shaders/voxel.wgsl"));
    check("shadow", include_str!("../shaders/shadow.wgsl"));
    check("terrain", include_str!("../shaders/terrain.wgsl"));
    check("water", include_str!("../shaders/water.wgsl"));
    check("sky", include_str!("../shaders/sky.wgsl"));
    check("post", include_str!("../shaders/post.wgsl"));
}

/// The frame uniform must be exactly the size WGSL expects (3 mat4 + 8 vec4).
#[test]
fn frame_uniform_matches_wgsl_layout() {
    assert_eq!(std::mem::size_of::<clawbound::render::FrameUniform>(), 3 * 64 + 8 * 16);
    assert_eq!(std::mem::size_of::<clawbound::render::DrawData>(), 64 + 32);
}
