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
    @location(3) mat: f32,
};

struct VSOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec3<f32>,
    @location(3) shadow_pos: vec4<f32>,
    @location(4) mat: f32,
};

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

// Per-material sub-metre detail. Returns an albedo multiplier (around 1)
// and a small normal perturbation so the detail catches the sun.
// mat: 0 grass, 1 dirt lane, 2 riverbed, 3 sand, 4 outcrop, 5 mountain, 6 moss
fn detail(p: vec2<f32>, m: f32, dist: f32) -> vec4<f32> {
    // fade the detail out with distance so it never shimmers
    let fade = smoothstep(120.0, 20.0, dist);
    var mul = 1.0;
    var bump = vec2(0.0);
    if (m < 0.5 || m > 5.5) {
        // grass / moss: fine blade speckle + broad tuft variation
        let n1 = noise2(p * 3.1);
        let n2 = noise2(p * 11.0 + 4.0);
        let n3 = noise2(p * 0.55 + 9.0);
        mul = 0.86 + n1 * 0.16 + (n2 - 0.5) * 0.14 + (n3 - 0.5) * 0.12;
        bump = vec2(n2 - 0.5, noise2(p * 11.0 + 21.0) - 0.5) * 0.10;
    } else if (m < 1.5) {
        // dirt: cracks (thin dark lines from a cellular-ish noise), pebbles, ruts
        let n1 = noise2(p * 1.7);
        let crack = smoothstep(0.02, 0.0, abs(n1 - 0.5) - 0.01) * 0.35;
        let peb = step(0.965, hash21(floor(p * 6.0))) * 0.22;
        let rut = (noise2(vec2(p.x * 0.9, p.y * 0.2) + 3.0) - 0.5) * 0.14;
        mul = 0.94 - crack + peb + rut + (noise2(p * 5.0) - 0.5) * 0.10;
        bump = vec2(noise2(p * 5.0 + 1.0) - 0.5, noise2(p * 5.0 + 2.0) - 0.5) * 0.12 + vec2(peb, peb) * 0.3;
    } else if (m < 2.5) {
        // riverbed: rounded pebbles, darker between, a green algae film
        let cell = floor(p * 2.6);
        let cc = (cell + 0.5 + vec2(hash21(cell), hash21(cell + 7.0)) * 0.5 - 0.25) / 2.6;
        let dd = distance(p, cc);
        let pebble = smoothstep(0.19, 0.11, dd);
        let algae = smoothstep(0.55, 0.8, noise2(p * 0.8 + 13.0)) * 0.3;
        mul = 0.78 + pebble * 0.42 - algae;
        bump = normalize(p - cc + vec2(0.0001)) * pebble * 0.35;
    } else if (m < 3.5) {
        // sand: ripples along the bank + grain
        let rip = sin(p.x * 5.0 + noise2(p * 0.7) * 6.0) * 0.5 + 0.5;
        mul = 0.92 + rip * 0.12 + (noise2(p * 14.0) - 0.5) * 0.10;
        bump = vec2(cos(p.x * 5.0) * 0.08, 0.0);
    } else {
        // rock: strata + chips
        let strata = sin(p.y * 2.0 + noise2(p * 0.9) * 4.0) * 0.5 + 0.5;
        let chip = step(0.94, hash21(floor(p * 4.0))) * 0.25;
        mul = 0.85 + strata * 0.2 + chip + (noise2(p * 7.0) - 0.5) * 0.12;
        bump = vec2(noise2(p * 7.0 + 5.0) - 0.5, noise2(p * 7.0 + 6.0) - 0.5) * 0.2;
    }
    return vec4(mix(1.0, mul, fade), bump * fade, 0.0);
}

@vertex
fn vs_main(in: VSIn) -> VSOut {
    var out: VSOut;
    let w = vec4<f32>(in.pos, 1.0);
    out.clip = frame.view_proj * w;
    out.world = in.pos;
    out.normal = in.normal;
    out.color = in.color;
    out.shadow_pos = frame.light_view_proj * w;
    out.mat = in.mat;
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let cam_d = distance(in.world, frame.cam_pos.xyz);
    let det = detail(in.world.xz, in.mat, cam_d);
    let N = normalize(in.normal + vec3(det.y, 0.0, det.z));
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
    var albedo = in.color * det.x * (1.0 - 0.32 * wetness);
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
