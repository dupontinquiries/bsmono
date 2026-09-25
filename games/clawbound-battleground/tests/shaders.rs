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
    check("voxel", clawbound::render::shader_src::VOXEL);
    check("shadow", include_str!("../shaders/shadow.wgsl"));
    check("terrain", clawbound::render::shader_src::TERRAIN);
    check("water", include_str!("../shaders/water.wgsl"));
    check("sky", include_str!("../shaders/sky.wgsl"));
    check("post", clawbound::render::shader_src::POST);
    check("smooth", clawbound::render::shader_src::SMOOTH);
}

/// The frame uniform must be exactly the size WGSL expects (3 mat4 + 8 vec4).
#[test]
fn frame_uniform_matches_wgsl_layout() {
    // + 8 brushes of vec4. APPENDED: only voxel.wgsl declares them, and the
    // other shaders bind the same buffer through the shorter struct.
    // ...+ 4 blasts x 2 vec4 + 8 cracks, declared by voxel.wgsl and terrain.wgsl.
    assert_eq!(std::mem::size_of::<clawbound::render::FrameUniform>(), 3 * 64 + 8 * 16 + 8 * 16 + 8 * 16 + 8 * 16 + 12 * 16);
    assert_eq!(clawbound::render::MAX_TOWER_RINGS, 12, "terrain.wgsl hard-codes array<vec4<f32>, 12>");
    assert_eq!((clawbound::groundfx::MAX_BLASTS, clawbound::groundfx::MAX_CRACKS, clawbound::groundfx::MAX_RINGS), (4, 8, 8), "the shaders hard-code these");
    assert_eq!(clawbound::puppet::MAX_BRUSHES, 8, "voxel.wgsl hard-codes array<vec4<f32>, 8>");
    assert_eq!(std::mem::size_of::<clawbound::render::DrawData>(), 64 + 32);
}
