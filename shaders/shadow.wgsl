// Depth-only voxel pass for the sun shadow map.

struct FrameData {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    light_view_proj: mat4x4<f32>,
    sun_dir: vec4<f32>,
    cam_pos: vec4<f32>,
    fog_color: vec4<f32>,
    params: vec4<f32>,   // x time, y fog_density, z exposure, w night
    sun_col: vec4<f32>,  // rgb sun colour*intensity, w cloud cover
    weather: vec4<f32>,  // x rain, y snow, z wetness, w lightning
    wind: vec4<f32>,
};

struct DrawData {
    model: mat4x4<f32>,
    tint: vec4<f32>,
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: FrameData;
@group(0) @binding(1) var<storage, read> draws: array<DrawData>;

struct VSIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) off_ao: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) misc: vec4<f32>,
};

@vertex
fn vs_main(in: VSIn) -> @builtin(position) vec4<f32> {
    let d = draws[0];
    let voxel = d.params.y * in.misc.x;
    let local = in.pos * voxel + in.off_ao.xyz;
    let world = d.model * vec4<f32>(local, 1.0);
    return frame.light_view_proj * world;
}
