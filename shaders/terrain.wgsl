// Terrain: colored ground grid with lambert + shadows + fog.

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

@group(0) @binding(0) var<uniform> frame: FrameData;

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2(123.34, 456.21));
    q = q + dot(q, q + 45.32);
    return fract(q.x * q.y);
}
@group(0) @binding(2) var shadow_map: texture_depth_2d;
@group(0) @binding(3) var shadow_sampler: sampler_comparison;

struct VSIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
};

struct VSOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) shadow_pos: vec4<f32>,
};

@vertex
fn vs_main(in: VSIn) -> VSOut {
    var out: VSOut;
    let w = vec4<f32>(in.pos, 1.0);
    out.clip = frame.view_proj * w;
    out.world = in.pos;
    out.normal = in.normal;
    out.color = in.color;
    out.shadow_pos = frame.light_view_proj * w;
    return out;
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
        let bias = 0.0030;
        var s = 0.0;
        for (var i: i32 = -1; i <= 1; i = i + 1) {
            for (var j: i32 = -1; j <= 1; j = j + 1) {
                s = s + textureSampleCompare(shadow_map, shadow_sampler,
                    uv + vec2(f32(i), f32(j)) * 0.0010, sp.z - bias);
            }
        }
        shadow = s / 9.0;
    }

    let night = frame.params.w;
    let cloud = frame.sun_col.w;
    var hemi = vec3(0.58, 0.62, 0.76) * mix(1.0, 0.72, cloud);
    hemi = mix(hemi, vec3(0.06, 0.07, 0.16), night);
    let flash = frame.weather.w;
    hemi = hemi + vec3(0.9, 0.9, 1.2) * flash;
    let sun_col = frame.sun_col.xyz * 1.02;
    let dist = distance(in.world, frame.cam_pos.xyz);
    let fog = 1.0 - exp(-dist * frame.params.y);

    // wet ground: darken albedo, add a broad sky reflection + sun glint;
    // puddles pool in a low-frequency noise pattern so it reads as water
    let wet = frame.weather.z;
    let pud = smoothstep(0.55, 0.8, hash21(floor(in.world.xz * 0.35)) * 0.5 + 0.5 * hash21(floor(in.world.xz * 0.9 + 3.1)));
    let wetness = wet * (0.55 + 0.45 * pud);
    var albedo = in.color * (1.0 - 0.32 * wetness);
    // snow cover settles on up-facing ground, patchy by the same noise as the puddles
    let snow = frame.weather.y;
    let snow_cov = snow * smoothstep(0.35, 0.9, N.y) * (0.6 + 0.4 * (1.0 - pud));
    albedo = mix(albedo, vec3(0.88, 0.92, 0.98), snow_cov * 0.85);
    var lit = albedo * (sun_col * ndl * shadow + hemi * 0.9);
    let V = normalize(frame.cam_pos.xyz - in.world);
    let Hv = normalize(V + sun);
    let fres = pow(1.0 - max(dot(N, V), 0.0), 3.0);
    let glint = pow(max(dot(N, Hv), 0.0), 90.0) * shadow;
    lit = lit + (hemi * fres * 0.6 + sun_col * glint * 0.9) * wetness;
    let lum = dot(in.color, vec3(0.299, 0.587, 0.114));
    let haze = frame.fog_color.xyz * (0.45 + 0.55 * lum);
    lit = mix(lit, haze, fog * 0.9);
    return vec4(lit, 1.0);
}
