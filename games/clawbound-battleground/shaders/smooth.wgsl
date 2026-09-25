// Ultra preview: characters as smooth surfaces (smoothvox), shaded with
// physically based lighting, furred with shells, lit by coloured point lights
// whose shadows are RAY TRACED against sphere proxies of the cats.
//
// It keeps the game's lighting model (voxel.wgsl: warm wrap-lit sun, sky and
// ground hemisphere that follows weather and night, rim, golden haze) so an
// ultra cat stands in the same world as everything else; what it adds is
// GGX specular, fur, coloured lights and traced shadows.

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
    event: vec4<f32>,
    brush: array<vec4<f32>, 8>,
    blast_a: array<vec4<f32>, 4>,
    blast_b: array<vec4<f32>, 4>,
    cracks: array<vec4<f32>, 8>,
};

struct DrawData {
    model: mat4x4<f32>,
    tint: vec4<f32>,
    params: vec4<f32>,
};

// Coloured point lights and the spheres their shadows are traced against.
struct Lights {
    info: vec4<f32>,               // x lights, y occluders, z fur shells, w fur density
    pos: array<vec4<f32>, 8>,      // xyz, w radius
    col: array<vec4<f32>, 8>,      // rgb * intensity
    occ: array<vec4<f32>, 32>,     // xyz centre, w radius
};

@group(0) @binding(0) var<uniform> frame: FrameData;
@group(0) @binding(1) var<storage, read> draws: array<DrawData>;
@group(0) @binding(2) var shadow_map: texture_depth_2d;
@group(0) @binding(3) var shadow_sampler: sampler_comparison;
@group(1) @binding(0) var<uniform> lights: Lights;

struct VSIn {
    @location(0) pos: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) fur: f32,
    @builtin(instance_index) shell: u32,
};

struct VSOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) shadow_pos: vec4<f32>,
    @location(4) local: vec3<f32>,
    @location(5) h: f32,
    @location(6) fur: f32,
};

@vertex
fn vs_main(in: VSIn) -> VSOut {
    let d = draws[0];
    // shell 0 is the skin; shells 1..N stand off along the normal, the fur
    // drooping a little under its own weight toward the tips
    let shells = max(lights.info.z, 1.0);
    let h = f32(in.shell) / shells;
    // STYLIZED locks, combed: each shell steps out along the normal bent
    // back toward the tail and a little down (model space faces +z), so a
    // lock leans the way fur lies instead of bristling straight out
    let comb = normalize(vec3(0.0, -0.55, -1.0));
    let dir = normalize(in.normal + comb * 0.9 * h);
    let local = in.pos + dir * in.fur * h;
    var world = (d.model * vec4(local, 1.0)).xyz;
    world = world - vec3(0.0, in.fur * h * h * 0.6, 0.0);
    // the wind combs the tips
    world = world + frame.wind.xyz * (in.fur * h * h * 0.08);
    var out: VSOut;
    out.clip = frame.view_proj * vec4(world, 1.0);
    out.world = world;
    out.normal = normalize((d.model * vec4(in.normal, 0.0)).xyz);
    out.color = in.color * d.tint;
    out.shadow_pos = frame.light_view_proj * vec4(world, 1.0);
    out.local = in.pos;
    out.h = h;
    out.fur = in.fur;
    return out;
}

fn hash3(p: vec3<f32>) -> f32 {
    var q = fract(p * vec3(0.1031, 0.1030, 0.0973));
    q = q + dot(q, q.yxz + 33.33);
    return fract((q.x + q.y) * q.z);
}

fn shadow_edge(uv: vec2<f32>) -> f32 {
    let e = min(min(uv.x, 1.0 - uv.x), min(uv.y, 1.0 - uv.y));
    return 1.0 - smoothstep(0.0, 0.08, e);
}

// Soft shadow of a sphere along a ray, closed form (after Inigo Quilez):
// how nearly the ray passes the sphere, relative to how far along it the
// pass is, gives the penumbra. 1 = clear, 0 = fully blocked. Nothing is cast
// by a sphere behind the point or beyond the light.
fn sphere_soft_shadow(ro: vec3<f32>, rd: vec3<f32>, sph: vec4<f32>, k: f32, t_max: f32) -> f32 {
    let oc = ro - sph.xyz;
    let b = dot(oc, rd);
    let c = dot(oc, oc) - sph.w * sph.w;
    let h = b * b - c;
    if (b > 0.0 || -b - sph.w > t_max) {
        return 1.0;
    }
    return smoothstep(0.0, 1.0, h * k / b);
}

const PI: f32 = 3.14159265;

// GGX / Smith / Schlick: the standard microfacet specular.
fn ggx(N: vec3<f32>, V: vec3<f32>, L: vec3<f32>, rough: f32, f0: vec3<f32>) -> vec3<f32> {
    let H = normalize(V + L);
    let nh = max(dot(N, H), 0.0);
    let nv = max(dot(N, V), 0.001);
    let nl = max(dot(N, L), 0.0);
    let a = rough * rough;
    let a2 = a * a;
    let dd = nh * nh * (a2 - 1.0) + 1.0;
    let D = a2 / (PI * dd * dd);
    let k = (rough + 1.0) * (rough + 1.0) / 8.0;
    let G = (nv / (nv * (1.0 - k) + k)) * (nl / (nl * (1.0 - k) + k));
    let F = f0 + (1.0 - f0) * pow(1.0 - max(dot(H, V), 0.0), 5.0);
    return D * G * F / (4.0 * nv * max(nl, 0.001)) * nl;
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    // fur: shell h keeps only the strands that reach it, thinner toward the
    // tips (a strand is a cell of object space; the skin keeps everything)
    if (in.h > 0.0) {
        // STYLIZED LOCKS, not hairs: a few chunky cones, each filling most of
        // its cell at the root and coming to a point -- they echo the voxels
        // the cats are made of. Longer fur makes bigger locks (the mane).
        // Fine realistic strands read as uncanny on a voxel character.
        // no fur, no shells: shells collapsed onto bare skin (the face)
        // showed their lighter tips as dots
        if (in.fur < 0.012) {
            discard;
        }
        let lock = clamp(in.fur / 0.05, 1.0, 3.0);
        let q = in.local * lights.info.w / lock;
        let cell = floor(q);
        let seed = hash3(cell);
        let centre = vec3(hash3(cell + 7.1), hash3(cell + 13.7), hash3(cell + 29.3)) * 0.3 + 0.35;
        // wide roots that overlap their neighbours, so the fur reads as
        // clumps parting toward their tips -- separate thin cones seen
        // head-on read as polka dots
        let r = 0.86 * pow(1.0 - in.h, 0.9) * (0.85 + 0.15 * seed);
        let reach = 0.65 + 0.35 * seed; // this lock's length, of the fur's
        if (length(fract(q) - centre) > r || in.h > reach) {
            discard;
        }
    }
    var N = normalize(in.normal);
    let V = normalize(frame.cam_pos.xyz - in.world);
    let sun = normalize(frame.sun_dir.xyz);
    let ndl = max(dot(N, sun), 0.0);

    // the sun's shadow map, as the voxels read it
    var sp = in.shadow_pos;
    sp = sp / sp.w;
    let uv = sp.xy * vec2(0.5, -0.5) + vec2(0.5);
    var shadow = 1.0;
    if (uv.x > 0.002 && uv.x < 0.998 && uv.y > 0.002 && uv.y < 0.998 && sp.z <= 1.0) {
        var s = 0.0;
        for (var i: i32 = -1; i <= 1; i = i + 1) {
            for (var j: i32 = -1; j <= 1; j = j + 1) {
                s = s + textureSampleCompare(shadow_map, shadow_sampler, uv + vec2(f32(i), f32(j)) * 0.0012, sp.z - 0.0032);
            }
        }
        shadow = mix(s / 9.0, 1.0, shadow_edge(uv));
    }

    // the game's sky and ground (voxel.wgsl), so ultra cats share its world
    let night = frame.params.w;
    let cloud = frame.sun_col.w;
    let sun_col = frame.sun_col.xyz;
    var sky = mix(vec3(0.50, 0.60, 0.85), vec3(0.55, 0.57, 0.62), cloud);
    sky = mix(sky, vec3(0.05, 0.07, 0.18), night);
    var ground = mix(vec3(0.42, 0.34, 0.22), frame.fog_color.xyz, 0.5) * mix(1.0, 0.7, cloud);
    ground = mix(ground, vec3(0.03, 0.03, 0.06), night);
    let up = N.y * 0.5 + 0.5;
    let hemi = mix(ground, sky, up) * 0.78;

    // material: fur is rough and dielectric; the skin a little less rough;
    // the fur darkens toward the root (self-shadowing among the strands)
    // two-tone locks: darker at the root, lighter at the tip -- the
    // stylized gradient that reads at a distance
    let albedo = in.color.rgb * select(1.0, mix(0.84, 1.12, in.h), in.h > 0.0);
    // the darkest features (eyes, nose) are glossy: they catch a highlight
    // and read as alive, not as holes
    let feat = 1.0 - smoothstep(0.06, 0.16, dot(albedo, vec3(0.2126, 0.7152, 0.0722)));
    let rough = mix(mix(0.55, 0.85, in.h), 0.18, feat * select(0.0, 1.0, in.h == 0.0));
    let f0 = vec3(0.04);
    let fur_ao = mix(0.55, 1.0, in.h) * select(1.0, 0.85, in.h == 0.0);

    // the sun: wrap-lit diffuse, GGX specular
    let wrap = clamp((dot(N, sun) + 0.25) / 1.25, 0.0, 1.0);
    var diffuse = sun_col * mix(wrap * wrap, ndl, 0.6) * shadow;
    var spec = sun_col * ggx(N, V, sun, rough, f0) * shadow;
    let back = normalize(vec3(-sun.x, 0.35, -sun.z));
    // fur sheen: light caught at the silhouette (strands scatter it forward)
    let sheen = pow(1.0 - max(dot(N, V), 0.0), 3.0) * max(dot(N, back), 0.0) * mix(0.35, 0.6, in.h) * mix(1.0, 0.4, night);
    diffuse = diffuse + hemi + sky * sheen + sky * 0.22 * (1.0 - shadow) * up;

    // coloured point lights (abilities, gems), each shadow ray-traced
    // against the cats' spheres
    let nl_count = i32(lights.info.x);
    let no_count = i32(lights.info.y);
    for (var i = 0; i < 8; i = i + 1) {
        if (i >= nl_count) {
            break;
        }
        let lp = lights.pos[i];
        let to_l = lp.xyz - in.world;
        let dist = length(to_l);
        if (dist >= lp.w) {
            continue;
        }
        let L = to_l / dist;
        let win = clamp(1.0 - pow(dist / lp.w, 4.0), 0.0, 1.0);
        let atten = win * win / (dist * dist * 0.25 + 1.0);
        var vis = 1.0;
        for (var k = 0; k < 32; k = k + 1) {
            if (k >= no_count) {
                break;
            }
            let o = lights.occ[k];
            // skip the cat this surface belongs to
            if (length(in.world - o.xyz) < o.w * 1.15) {
                continue;
            }
            vis = vis * sphere_soft_shadow(in.world + N * 0.02, L, o, 6.0, dist);
        }
        let c = lights.col[i].rgb * atten * vis;
        diffuse = diffuse + c * max(dot(N, L), 0.0);
        spec = spec + c * ggx(N, V, L, rough, f0);
    }

    var lit = albedo * diffuse * fur_ao + spec;
    // the same haze as the world
    let fdist = distance(in.world, frame.cam_pos.xyz);
    let fog = 1.0 - exp(-fdist * frame.params.y);
    let lum = dot(albedo, vec3(0.299, 0.587, 0.114));
    lit = mix(lit, frame.fog_color.xyz * (0.45 + 0.55 * lum), fog * 0.9);
    return vec4(lit, 1.0);
}
