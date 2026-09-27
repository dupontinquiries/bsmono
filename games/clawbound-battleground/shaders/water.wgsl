// Water: stylized, liquid, readable. Vertex-displaced wave crests that ride
// the river's flow, depth-shaded from clear turquoise shallows to a dark
// teal channel, foam licking the banks, a moving sun-glint field, rain
// ripples, and a wet Fresnel toward the sky colour. Alpha-blended over the
// pebbled bed with read-only depth; the gelatin blobs (vfx.rs) sit on top.

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
    event: vec4<f32>,    // x flood rise, y quake shake, z fire glow, w unused
    brush: array<vec4<f32>, 8>,
    blast_a: array<vec4<f32>, 4>,
    blast_b: array<vec4<f32>, 4>,
    cracks: array<vec4<f32>, 8>,
    towers: array<vec4<f32>, 12>,
    sea: vec4<f32>,               // y: caustic strength (render.rs FrameUniform)
};

@group(0) @binding(0) var<uniform> frame: FrameData;

fn frame_caustics() -> f32 {
    return frame.sea.y;
}
@group(0) @binding(2) var shadow_map: texture_depth_2d;
@group(0) @binding(3) var shadow_sampler: sampler_comparison;

struct VSIn {
    @location(0) pos: vec3<f32>,
    @location(1) bed: f32,
    @location(2) flow: vec2<f32>,
    @location(3) uv: vec2<f32>,
};

struct VSOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) depth: f32,
    @location(2) flow: vec2<f32>,
    @location(3) uv: vec2<f32>,
    @location(4) wave_n: vec3<f32>,
};

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

// two crossing wave trains along the flow + a slow swell
fn wave_h(p: vec2<f32>, flow: vec2<f32>, t: f32) -> f32 {
    let along = dot(p, flow);
    let across = dot(p, vec2(-flow.y, flow.x));
    let w1 = sin(along * 1.6 - t * 2.6) * 0.055;
    let w2 = sin(along * 0.7 + across * 1.3 - t * 1.7) * 0.04;
    let w3 = sin(across * 2.4 + t * 1.1 + along * 0.4) * 0.025;
    let swell = (noise2(p * 0.18 + vec2(0.0, -t * 0.25)) - 0.5) * 0.09;
    return w1 + w2 + w3 + swell;
}

@vertex
fn vs_main(in: VSIn) -> VSOut {
    let t = frame.params.x;
    let rise = frame.event.x;
    let depth = max(in.pos.y + rise - in.bed, 0.0);
    // shallow water barely moves; the channel swells
    let amp = smoothstep(0.0, 0.8, depth);
    let h = wave_h(in.pos.xz, in.flow, t) * amp;
    var p = in.pos;
    p.y = p.y + h + rise;
    // finite-difference normal of the wave field
    let e = 0.35;
    let hx = wave_h(in.pos.xz + vec2(e, 0.0), in.flow, t) * amp;
    let hz = wave_h(in.pos.xz + vec2(0.0, e), in.flow, t) * amp;
    let n = normalize(vec3(h - hx, e * 1.6, h - hz));
    var out: VSOut;
    out.clip = frame.view_proj * vec4(p, 1.0);
    out.world = p;
    out.depth = depth;
    out.flow = in.flow;
    out.uv = in.uv;
    out.wave_n = n;
    return out;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let t = frame.params.x;
    let night = frame.params.w;
    let rain = frame.weather.x;
    let flash = frame.weather.w;
    let sun = normalize(frame.sun_dir.xyz);
    let V = normalize(frame.cam_pos.xyz - in.world);

    // --- surface normal: waves + a fine scrolling ripple texture + rain rings
    var N = normalize(in.wave_n);
    let p = in.world.xz;
    let drift = in.flow * t * 1.4;
    let r1 = noise2(p * 2.2 - drift);
    let r2 = noise2(p * 4.7 - drift * 1.6 + vec2(3.1, 7.7));
    var ripple = vec3((r1 - 0.5) * 0.35 + (r2 - 0.5) * 0.18, 0.0, (r2 - 0.5) * 0.35 + (r1 - 0.5) * 0.18);
    if (rain > 0.02) {
        // expanding rings: cells with a phase each; a ring is a thin band in radius
        let cell = floor(p * 1.5);
        let ph = hash21(cell);
        let c = (cell + 0.5 + vec2(ph, hash21(cell + 9.0)) * 0.4) / 1.5;
        let rr = fract(t * 1.3 + ph * 7.0);
        let d = distance(p, c);
        let ring = smoothstep(0.06, 0.0, abs(d - rr * 0.6)) * (1.0 - rr);
        ripple = ripple + normalize(vec3(p.x - c.x, 0.0, p.y - c.y) + vec3(0.0001)) * ring * 0.5 * rain;
    }
    N = normalize(N + ripple * 0.5);

    // --- depth shading: clear shallows -> teal channel
    let d = in.depth;
    // a flood runs brown: silt in the water
    let silt = clamp(frame.event.x / 0.9, 0.0, 1.0);
    let shallow = mix(vec3(0.30, 0.62, 0.62), vec3(0.42, 0.38, 0.26), silt);
    let deep = mix(vec3(0.03, 0.18, 0.24), vec3(0.16, 0.13, 0.08), silt);
    var col = mix(shallow, deep, smoothstep(0.0, 1.6, d));
    // the bed shows through: alpha by depth (Beer), and more of it looking
    // straight down than across -- the light's path through the water is
    // shorter (the bed, and its caustics, show under a steep view)
    let steep = max(V.y, 0.05);
    var alpha = 1.0 - exp(-d * 0.45 / steep) * 0.95;
    alpha = clamp(alpha + 0.25, 0.0, 0.82);

    // --- lighting
    let ndl = max(dot(N, sun), 0.0);
    let sun_col = frame.sun_col.xyz;
    let sky = mix(vec3(0.62, 0.66, 0.80), vec3(0.06, 0.07, 0.16), night) * mix(1.0, 0.75, frame.sun_col.w);
    // Schlick's Fresnel for water (F0 = 0.02): almost clear looking down,
    // a mirror at a glance
    // (on the swell's normal with a third of the ripple: the full ripple
    // tipped whole patches to a grazing angle and they read as white
    // blotches, not water)
    let Nf = normalize(mix(normalize(in.wave_n), N, 0.35));
    let cosv = max(dot(Nf, V), 0.0);
    let fres = 0.02 + 0.98 * pow(1.0 - cosv, 5.0);
    // what the surface reflects: the sky as it is in that direction -- the
    // haze at the horizon, the deeper blue overhead, and the sun's glow
    let R = reflect(-V, Nf);
    let zen = sky * vec3(0.78, 0.9, 1.15);
    let hor = mix(frame.fog_color.xyz * 1.1, sky, 0.35);
    var refl = mix(hor, zen, smoothstep(-0.05, 0.6, R.y));
    refl = refl + sun_col * (pow(max(dot(R, sun), 0.0), 12.0) * 0.35) * (1.0 - frame.sun_col.w * 0.8);
    // light scattered up out of the body of the water, greenest where the
    // sun comes through a wave toward the eye
    let through = pow(max(dot(-V, sun) * 0.5 + 0.5, 0.0), 3.0) * (1.0 - cosv);
    var lit = col * (sun_col * (0.25 + 0.45 * ndl) + sky * 0.38) + col * sun_col * through * 0.6;
    // the caustic web on the bed, seen through the water's body: the bed
    // itself takes it (terrain.wgsl), and the water in front of it glows
    // with the same pattern, bent a little by the surface
    let bent = p + N.xz * d * 0.6;
    lit = lit + sun_col * vec3(0.85, 1.0, 0.95) * caustic(bent, t) * 0.55 * exp(-d * 0.5) * (1.0 - night) * frame_caustics();
    lit = mix(lit, refl, fres);
    alpha = max(alpha, fres);
    // sun glint: tight lobe broken up by the ripple field so it sparkles
    let Hv = normalize(V + sun);
    let spec = pow(max(dot(N, Hv), 0.0), 220.0) * 2.4;
    lit = lit + sun_col * spec * (1.0 - frame.sun_col.w * 0.7);
    // lightning + moon
    lit = lit + vec3(0.7, 0.75, 1.0) * flash * 0.6;

    // --- foam: banks, wave crests, and the wakes of rain
    // (only the true edge: at 0.32 u every shallow rise mid-river foamed,
    // and read as white blotches on the water)
    let bank = smoothstep(0.12, 0.0, d);
    let crest_n = noise2(p * 3.0 - drift * 2.0 + vec2(t * 0.3, 0.0));
    let crest = smoothstep(0.74, 0.92, crest_n) * smoothstep(0.4, 1.2, d) * 0.18;
    let foam = clamp(bank * (0.5 + 0.5 * noise2(p * 5.0 - drift * 3.0)) + crest, 0.0, 1.0);
    lit = mix(lit, vec3(0.9, 0.95, 0.95) * (sun_col.g * 0.7 + 0.3), foam * 0.85);
    alpha = max(alpha, foam * 0.9);

    // --- distance fog
    let dist = distance(in.world, frame.cam_pos.xyz);
    let fog = 1.0 - exp(-dist * frame.params.y);
    let lum = dot(lit, vec3(0.299, 0.587, 0.114));
    let haze = frame.fog_color.xyz * (0.45 + 0.55 * lum);
    lit = mix(lit, haze, fog * 0.9);

    return vec4(lit, alpha);
}
