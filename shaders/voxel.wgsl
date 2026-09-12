// Instanced microvoxel rendering: lambert + baked AO + PCF shadows + fog.

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
    tint: vec4<f32>,   // rgb tint, a = alpha multiplier
    params: vec4<f32>, // x emissive, y voxel_size, z unused, w unused
};

@group(0) @binding(0) var<uniform> frame: FrameData;
@group(0) @binding(1) var<storage, read> draws: array<DrawData>;
@group(0) @binding(2) var shadow_map: texture_depth_2d;
@group(0) @binding(3) var shadow_sampler: sampler_comparison;

struct VSIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) off_ao: vec4<f32>,
    @location(3) color: vec4<f32>,
    @location(4) misc: vec4<f32>,
};

struct VSOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) shadow_pos: vec4<f32>,
    @location(4) ao: f32,
    @location(5) emissive: f32,
};

@vertex
fn vs_main(in: VSIn) -> VSOut {
    let d = draws[0]; // dynamic offset selects this draw
    let voxel = d.params.y * in.misc.x;
    let local = in.pos * voxel + in.off_ao.xyz;
    let world = d.model * vec4<f32>(local, 1.0);
    let nrm = normalize((d.model * vec4<f32>(in.normal, 0.0)).xyz);
    var out: VSOut;
    out.clip = frame.view_proj * world;
    out.world = world.xyz;
    out.normal = nrm;
    out.color = in.color * d.tint;
    out.shadow_pos = frame.light_view_proj * world;
    out.ao = in.off_ao.w;
    out.emissive = d.params.x;
    return out;
}

fn aces(x: vec3<f32>) -> vec3<f32> {
    let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3(0.0), vec3(1.0));
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let N = normalize(in.normal);
    let sun = normalize(frame.sun_dir.xyz);
    let ndl = max(dot(N, sun), 0.0);

    var sp = in.shadow_pos;
    sp = sp / sp.w;
    let uv = sp.xy * vec2(0.5, -0.5) + vec2(0.5);
    var shadow = 1.0;
    if (uv.x > 0.002 && uv.x < 0.998 && uv.y > 0.002 && uv.y < 0.998 && sp.z <= 1.0) {
        let bias = 0.0032;
        var s = 0.0;
        for (var i: i32 = -1; i <= 1; i = i + 1) {
            for (var j: i32 = -1; j <= 1; j = j + 1) {
                s = s + textureSampleCompare(shadow_map, shadow_sampler,
                    uv + vec2(f32(i), f32(j)) * 0.0012, sp.z - bias);
            }
        }
        shadow = s / 9.0;
    }

    // hemisphere ambient: dims and cools under cloud, goes indigo at night;
    // a lightning flash floods it white for a frame or two
    let night = frame.params.w;
    let cloud = frame.sun_col.w;
    var hemi = mix(vec3(0.34, 0.36, 0.46), vec3(0.62, 0.66, 0.80), N.y * 0.5 + 0.5);
    hemi = hemi * mix(1.0, 0.72, cloud);
    hemi = mix(hemi, vec3(0.06, 0.07, 0.16), night);
    let flash = frame.weather.w;
    hemi = hemi + vec3(0.9, 0.9, 1.2) * flash * (N.y * 0.5 + 0.5);
    let sun_col = frame.sun_col.xyz;
    var light = sun_col * ndl * shadow + hemi * 0.66;

    // wet sheen: a tight Blinn lobe that grows with ground wetness
    let V = normalize(frame.cam_pos.xyz - in.world);
    let Hv = normalize(V + sun);
    let spec = pow(max(dot(N, Hv), 0.0), 26.0) * frame.weather.z * 0.35 * shadow;
    light = light + sun_col * spec;

    // distance fog — melts into warm golden jungle haze
    let dist = distance(in.world, frame.cam_pos.xyz);
    let fog = 1.0 - exp(-dist * frame.params.y);
    let albedo = in.color.rgb;
    var lit = albedo * light * in.ao;
    let lum = dot(albedo, vec3(0.299, 0.587, 0.114));
    let haze = frame.fog_color.xyz * (0.45 + 0.55 * lum);
    lit = mix(lit, haze, fog * 0.9);
    // emissive glow (crystals, projectiles, VFX)
    lit = lit + albedo * in.emissive * (1.0 - fog * 0.4);

    // cheap tonemap here is skipped for HDR path; alpha handling:
    return vec4(lit, in.color.a);
}
