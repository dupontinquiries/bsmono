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
    wind: vec4<f32>,     // xyz wind (u/s), w gust front 0..1
    event: vec4<f32>,    // x flood rise, y quake shake, z fire glow, w unused
};

struct DrawData {
    model: mat4x4<f32>,
    tint: vec4<f32>,
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: FrameData;
@group(0) @binding(1) var<storage, read> draws: array<DrawData>;

// Same deformation as voxel.wgsl so shadows follow the swaying canopy.
fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2(123.34, 456.21));
    q = q + dot(q, q + 45.32);
    return fract(q.x * q.y);
}
fn noise2(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash21(i);
    let b = hash21(i + vec2(1.0, 0.0));
    let c = hash21(i + vec2(0.0, 1.0));
    let d = hash21(i + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Wind deformation for flora. misc.y is the piece's sway weight (baked at
// chunk merge; 0 for everything rigid). Bend grows with height above the
// piece's base (off.y is model-space, so the trunk stays planted), rides a
// travelling gust wave across the map, and flutters at the tips.
fn wind_offset(world: vec3<f32>, local_y: f32, sway: f32) -> vec3<f32> {
    if (sway <= 0.0) {
        return vec3(0.0);
    }
    let t = frame.params.x;
    let w = frame.wind.xyz;
    let ws = length(w);
    let dir = select(vec3(1.0, 0.0, 0.0), w / max(ws, 0.001), ws > 0.001);
    let gust = frame.wind.w;
    // gust front travelling downwind: a broad wave + a finer ripple
    let along = dot(world.xz, dir.xz);
    let front = 0.5 + 0.5 * sin(along * 0.08 - t * 2.4);
    let ripple = noise2(world.xz * 0.25 + vec2(t * 0.9, -t * 0.7)) - 0.5;
    let strength = ws * 0.012 + gust * 0.11 * (0.55 + 0.45 * front);
    let h = max(local_y, 0.0);
    let bend = h * h * 0.06 * sway * strength;
    // lean downwind, sag a little, flutter side-to-side at the tips
    let side = vec3(-dir.z, 0.0, dir.x);
    return dir * bend + side * (ripple * h * 0.05 * sway * (0.4 + gust)) - vec3(0.0, bend * 0.35, 0.0);
}



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
    var world = d.model * vec4<f32>(local, 1.0);
    world = vec4(world.xyz + wind_offset(world.xyz, in.off_ao.y, in.misc.y), 1.0);
    return frame.light_view_proj * world;
}
