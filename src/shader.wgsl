// skydribble shaders — sky / scene / ui, one module, three pipelines.
//
// v1.2 "rays": the scene fragment shader ray-marches the SAME analytic SDF
// world the physics uses (shapes come in through a storage buffer, terrain
// through a baked heightmap), giving real ray-traced soft shadows, SDF
// ambient occlusion and mirror reflections — no RT hardware, just ALU. This
// is the classic software-RT workaround and it thrives on Apple-silicon-class
// GPUs (built for M2 Ultra / M4 Max: wide, unified-memory, bandwidth-rich).

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam: vec4<f32>,        // xyz cam pos, w time
    sun: vec4<f32>,        // xyz sun dir (towards sun), w unused
    fog: vec4<f32>,        // rgb fog/horizon color, w fog density
    misc: vec4<f32>,       // xyz zenith color (per map style), w spare
    rt: vec4<f32>,         // x rays on, y quality, z spare, w ground wetness
    ray2: vec4<f32>,       // x ray distance, y AO probes, z/w spare
    hmap: vec4<f32>,       // x half extent, y texels, zw = screen w,h in px
    gcol: vec4<f32>,       // rgb terrain base color, w shape count
    cine: vec4<f32>,       // x exposure, y vignette, z grain, w filmic tonemap
    post: vec4<f32>,       // x bloom strength, y bloom threshold, zw spare
    pcount: vec4<f32>,     // x = live portal count (max 4)
    portals: array<vec4<f32>, 16>, // per portal: [pos.xyz, r], then face quat
};
@group(0) @binding(0) var<uniform> G: Globals;

// Ray-marchable shape mirror of world::Shape (built each frame, includes
// dynamic proxies for the ball, cars and live pad orbs).
struct RtShape {
    a: vec4<f32>,          // xyz pos, w bounding radius
    b: vec4<f32>,          // kind data: sphere r / torus big,tube / capsule r,half / rbox half+round(w)
    c: vec4<f32>,          // inverse rotation quaternion
    d: vec4<f32>,          // rgb albedo, w emissive
    e: vec4<f32>,          // x spec, y kind (0 sph 1 tor 2 cap 3 rbox), zw spare
};
@group(0) @binding(1) var<storage, read> RTS: array<RtShape>;
@group(0) @binding(2) var t_hmap: texture_2d<f32>;
@group(0) @binding(3) var<storage, read> RCELLS: array<u32>;
@group(0) @binding(4) var t_csdf: texture_3d<f32>;
@group(0) @binding(5) var s_csdf: sampler;

const CSDF_SLAB: f32 = 32.0;
const CSDF_D: f32 = 384.0; // SLAB x 12 models

// baked candidate grid (must match render.rs RC_* consts)
const RC_DIM: u32 = 40u;
const RC_STRIDE: u32 = 32u;
const RC_WORLD_HALF: f32 = 1300.0;

fn rcell_base(p: vec3<f32>) -> u32 {
    let cell = (RC_WORLD_HALF * 2.0) / f32(RC_DIM);
    let cx = clamp(u32((p.x + RC_WORLD_HALF) / cell), 0u, RC_DIM - 1u);
    let cz = clamp(u32((p.z + RC_WORLD_HALF) / cell), 0u, RC_DIM - 1u);
    return (cz * RC_DIM + cx) * RC_STRIDE;
}

// ----------------------------------------------------------- cinematic ----
fn aces(x: vec3<f32>) -> vec3<f32> {
    // Narkowicz ACES filmic approximation
    let a = 2.51; let b = 0.03; let c = 2.43; let d = 0.59; let e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), vec3(0.0), vec3(1.0));
}

fn hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3(p.x, p.y, p.x) * 0.1031);
    p3 = p3 + dot(p3, vec3(p3.y, p3.z, p3.x) + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

/// Exposure -> tonemap -> vignette -> grain. frag = pixel coords.
fn screen_fx(col_in: vec3<f32>, frag: vec2<f32>) -> vec3<f32> {
    var col = col_in * G.cine.x;
    if (G.cine.w > 0.5) {
        col = aces(col);
    }
    let res = max(G.hmap.zw, vec2(1.0));
    let uv = frag / res * 2.0 - vec2(1.0);
    let r = length(uv * vec2(1.0, 0.85));
    col *= 1.0 - G.cine.y * smoothstep(0.62, 1.55, r);
    let g = (hash12(frag + vec2(fract(G.cam.w * 61.7) * 977.0)) - 0.5) * G.cine.z * 0.10;
    return max(col + vec3(g), vec3(0.0));
}

// ---------------------------------------------------------------- sky ----
struct SkyOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_sky(@builtin(vertex_index) vi: u32) -> SkyOut {
    // Fullscreen triangle.
    var p = array<vec2<f32>, 3>(vec2(-1.0, -3.0), vec2(3.0, 1.0), vec2(-1.0, 1.0));
    var out: SkyOut;
    out.clip = vec4(p[vi], 1.0, 1.0);
    out.ndc = p[vi];
    return out;
}

fn sky_color(dir: vec3<f32>) -> vec3<f32> {
    let zen = G.misc.xyz;                      // zenith color, per map style
    let hor = G.fog.rgb;                       // pink-cream horizon
    let dusk = vec3(1.00, 0.78, 0.62);         // warm band above horizon
    let up = clamp(dir.y, -1.0, 1.0);
    var col = mix(hor, zen, pow(clamp(up, 0.0, 1.0), 0.55));
    col = mix(col, dusk, exp(-abs(up) * 9.0) * 0.35);
    if (up < 0.0) {                            // below horizon: soft haze floor
        col = mix(hor, hor * 0.82, clamp(-up * 3.0, 0.0, 1.0));
    }
    let s = max(dot(dir, normalize(G.sun.xyz)), 0.0);
    col += vec3(1.0, 0.95, 0.80) * pow(s, 900.0) * 1.6;   // sun disc
    col += vec3(1.0, 0.90, 0.70) * pow(s, 12.0) * 0.14;   // glow
    return col;
}

@fragment
fn fs_sky(in: SkyOut) -> @location(0) vec4<f32> {
    let near = G.inv_view_proj * vec4(in.ndc, 0.05, 1.0);
    let far  = G.inv_view_proj * vec4(in.ndc, 0.95, 1.0);
    let dir = normalize(far.xyz / far.w - near.xyz / near.w);
    return vec4(sky_color(dir), 1.0); // raw HDR; the composite pass grades it
}

// ------------------------------------------------------- rt primitives ----
fn qrot(q: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    return v + 2.0 * cross(q.xyz, q.w * v + cross(q.xyz, v));
}

/// Terrain height from the baked map, manual bilinear (no filterable-f32
/// feature needed, works everywhere Metal/Vulkan/GL).
fn terrain_h(x: f32, z: f32) -> f32 {
    let half_ext = G.hmap.x;
    let n = G.hmap.y;
    let u = clamp((x / half_ext * 0.5 + 0.5) * (n - 1.0), 0.0, n - 1.001);
    let v = clamp((z / half_ext * 0.5 + 0.5) * (n - 1.0), 0.0, n - 1.001);
    let i = vec2<i32>(i32(u), i32(v));
    let f = vec2(fract(u), fract(v));
    let h00 = textureLoad(t_hmap, i, 0).r;
    let h10 = textureLoad(t_hmap, i + vec2(1, 0), 0).r;
    let h01 = textureLoad(t_hmap, i + vec2(0, 1), 0).r;
    let h11 = textureLoad(t_hmap, i + vec2(1, 1), 0).r;
    return mix(mix(h00, h10, f.x), mix(h01, h11, f.x), f.y);
}

fn smin(a: f32, b: f32, k: f32) -> f32 {
    let h = clamp(0.5 + 0.5 * (b - a) / k, 0.0, 1.0);
    return b * (1.0 - h) + a * h - k * h * (1.0 - h);
}

fn shape_sdf(i: u32, p: vec3<f32>) -> f32 {
    let s = RTS[i];
    let q = qrot(s.c, p - s.a.xyz);
    let kind = i32(s.e.y) % 16; // +16 = fillet-group flag, stripped here
    if (kind == 0) {                                    // sphere
        return length(q) - s.b.x;
    } else if (kind == 1) {                             // torus
        let ring = vec2(length(q.xz) - s.b.x, q.y);
        return length(ring) - s.b.y;
    } else if (kind == 2) {                             // capsule (y axis)
        let y = clamp(q.y, -s.b.y, s.b.y);
        return length(vec3(q.x, q.y - y, q.z)) - s.b.x;
    } else if (kind == 5) {                             // hollow tube shell
        let ring = abs(length(q.xz) - s.b.x) - s.b.z;
        return max(ring, abs(q.y) - s.b.y);
    } else if (kind == 6) {                             // hexagonal prism
        let k6 = vec3(-0.8660254, 0.5, 0.57735);
        var a6 = abs(q);
        let d6 = 2.0 * min(k6.x * a6.x + k6.y * a6.z, 0.0);
        a6 = vec3(a6.x - d6 * k6.x, a6.y, a6.z - d6 * k6.y);
        let cl6 = clamp(a6.x, -k6.z * s.b.x, k6.z * s.b.x);
        let dxz = length(vec2(a6.x - cl6, a6.z - s.b.x)) * sign(a6.z - s.b.x);
        let dy6 = a6.y - s.b.y;
        return min(max(dxz, dy6), 0.0) + length(max(vec2(dxz, dy6), vec2(0.0)));
    } else if (kind == 10) {                            // flat ring deck
        let radial = abs(length(q.xz) - s.b.x) - s.b.y;
        let vert = abs(q.y) - s.b.z;
        return min(max(radial, vert), 0.0) + length(vec2(max(radial, 0.0), max(vert, 0.0))) - 0.12;
    } else if (kind == 9) {                             // hollow torus (loop)
        let ring = vec2(length(q.xz) - s.b.x, q.y);
        return abs(length(ring) - s.b.y) - s.b.z;
    } else if (kind == 8) {                             // EXACT car model SDF
        let h = s.b.xyz;
        let a8 = abs(q) - h;
        let outside = length(max(a8, vec3(0.0)));
        if (outside > 0.03) {
            return outside + 0.02; // conservative: true d >= box d
        }
        let uvw = clamp(q / (2.0 * h) + vec3(0.5), vec3(0.02), vec3(0.98));
        let w = (s.b.w * CSDF_SLAB + 0.5 + uvw.z * (CSDF_SLAB - 1.0)) / CSDF_D;
        let enc = textureSampleLevel(t_csdf, s_csdf, vec3(uvw.x, uvw.y, w), 0.0).r;
        return (enc * 2.4 - 0.6) * max(s.e.z, 0.05);
    }
    let a = abs(q) - s.b.xyz + vec3(s.b.w);             // rounded box
    return length(max(a, vec3(0.0))) + min(max(a.x, max(a.y, a.z)), 0.0) - s.b.w;
}

// Per-ray candidate set: bounding-sphere cull every shape once, march only
// the survivors. 16 nearest is plenty for a playfield this sparse.
const KMAX: u32 = 16u;
struct Cands {
    id: array<u32, KMAX>,
    // where the ray enters and leaves each candidate's bounding sphere:
    // entry orders the list, exit lets the march retire candidates it has
    // already gone past
    t0: array<f32, KMAX>,
    t1: array<f32, KMAX>,
    n: u32,
};

fn gather_try(c: ptr<function, Cands>, i: u32, ro: vec3<f32>, rd: vec3<f32>, tmax: f32, skip: i32) {
    if (i32(i) == skip) { return; }
    let s = RTS[i];
    let oc = ro - s.a.xyz;
    let b = dot(oc, rd);
    let cc = dot(oc, oc) - s.a.w * s.a.w;
    let disc = b * b - cc;
    if (disc <= 0.0) { return; }
    let sq = sqrt(disc);
    if (-b + sq < 0.0 || -b - sq > tmax) { return; }
    // low quality gathers fewer candidates: the marcher's cost is
    // (candidates x steps), so this is the cheapest knob there is
    let kcap = max(4u, u32(4.0 + 12.0 * clamp(G.rt.y, 0.0, 1.0)));
    let cap = min(KMAX, kcap);
    let e0 = -b - sq;
    let e1 = -b + sq;
    // KEEP THE NEAREST, NOT THE FIRST.
    //
    // A full list used to drop everything that arrived afterwards, so which
    // shapes a ray saw depended on the order the cell happened to store
    // them. That is wrong as well as slow: a nearer shape found late was
    // discarded in favour of a farther one found early, and on a busy cell
    // the ray could miss the very thing it was about to hit.
    //
    // Now a full list rejects on a SINGLE COMPARE when the newcomer is
    // farther than the current worst — which is the common case, so the
    // insertion sort below only runs when it is actually going to change
    // the answer.
    if ((*c).n >= cap) {
        if (e0 >= (*c).t0[cap - 1u]) { return; }
        (*c).n = cap - 1u; // drop the worst, then insert normally
    }
    {
        var k = (*c).n;
        loop {
            if (k == 0u) { break; }
            if ((*c).t0[k - 1u] <= e0) { break; }
            (*c).id[k] = (*c).id[k - 1u];
            (*c).t0[k] = (*c).t0[k - 1u];
            (*c).t1[k] = (*c).t1[k - 1u];
            k = k - 1u;
        }
        (*c).id[k] = i;
        (*c).t0[k] = e0;
        (*c).t1[k] = e1;
        (*c).n = (*c).n + 1u;
    }
}

// BAKE + PROJECT: statics come from the fragment's precomputed cell list
// (RCELLS), dynamics (car, ball, bots — RTS[static_n..total]) are scanned
// live. Shadow, AO and reflection all share this projection instead of
// each walking the whole shape list.
fn gather(ro: vec3<f32>, rd: vec3<f32>, tmax: f32, skip: i32) -> Cands {
    var c: Cands;
    c.n = 0u;
    let total = min(u32(G.gcol.w), arrayLength(&RTS));
    let static_n = min(u32(G.rt.z), total);
    let base = rcell_base(ro);
    let bn = min(RCELLS[base], RC_STRIDE - 1u);
    for (var j = 0u; j < bn; j = j + 1u) {
        let i = RCELLS[base + 1u + j];
        if (i < static_n) {
            gather_try(&c, i, ro, rd, tmax, skip);
        }
    }
    for (var i = static_n; i < total; i = i + 1u) {
        gather_try(&c, i, ro, rd, tmax, skip);
    }
    return c;
}

fn qconj(q: vec4<f32>) -> vec4<f32> {
    return vec4(-q.x, -q.y, -q.z, q.w);
}

/// Did the segment a->b cross a portal face from the front? If so, return
/// the teleported point and direction. This is what lets a ray SEE THROUGH
/// a portal: it does not stop at the face, it comes out of the partner and
/// keeps marching, so reflections and shadows carry through the pair too.
fn portal_hop(a: vec3<f32>, b: vec3<f32>, rd: vec3<f32>, out_p: ptr<function, vec3<f32>>, out_d: ptr<function, vec3<f32>>) -> bool {
    let n = i32(G.pcount.x);
    if (n == 0) { return false; }
    for (var i = 0; i < n; i = i + 1) {
        let base = u32(i) * 2u;
        let pa = G.portals[base].xyz;
        let ra = G.portals[base].w;
        let qa = G.portals[base + 1u];
        // partner is the paired index: 0<->1, 2<->3
        let j = select(i - 1, i + 1, (i % 2) == 0);
        if (j < 0 || j >= n) { continue; }
        let pb = G.portals[u32(j) * 2u].xyz;
        let qb = G.portals[u32(j) * 2u + 1u];
        let na = qrot(qa, vec3(0.0, 0.0, 1.0));
        let d0 = dot(a - pa, na);
        let d1 = dot(b - pa, na);
        if (d0 <= 0.0 || d1 > 0.0) { continue; }
        let t = clamp(d0 / (d0 - d1), 0.0, 1.0);
        let hit = mix(a, b, t);
        if (length(hit - pa) > ra) { continue; }
        // local = inverse(qa) * (hit - pa); half turn about the portal up;
        // then out through the partner's frame
        var loc = qrot(qconj(qa), hit - pa);
        var ldir = qrot(qconj(qa), rd);
        loc = vec3(-loc.x, loc.y, -loc.z);
        ldir = vec3(-ldir.x, ldir.y, -ldir.z);
        *out_p = pb + qrot(qb, loc) + qrot(qb, vec3(0.0, 0.0, 1.0)) * 0.05;
        *out_d = normalize(qrot(qb, ldir));
        return true;
    }
    return false;
}

/// ---- SKINS -------------------------------------------------------------
/// A finish gets the surface point, its normal, the view direction, the
/// clock and the car's speed, and returns a modified albedo plus an
/// emissive boost and a spec boost. Because it sees the view direction it
/// can respond to HOW YOU ARE LOOKING at the car, which is what separates
/// these from a scrolling texture — and because spec feeds the reflection
/// march, a skin can hand itself over to the ray tracer entirely.
struct SkinOut {
    albedo: vec3<f32>,
    emissive: f32,
    spec: f32,
}

fn hue_shift(c: vec3<f32>, a: f32) -> vec3<f32> {
    // rotate around the grey axis: cheap, and keeps luminance sane
    let k = vec3(0.57735, 0.57735, 0.57735);
    let cosa = cos(a);
    return c * cosa + cross(k, c) * sin(a) + k * dot(k, c) * (1.0 - cosa);
}

fn apply_skin(id: i32, base: vec3<f32>, p: vec3<f32>, pw: vec3<f32>, n: vec3<f32>, v: vec3<f32>, t: f32, speed: f32, phase: f32, ball: vec3<f32>, boost: f32, hit: vec4<f32>, combo: f32) -> SkinOut {
    var o: SkinOut;
    o.albedo = base;
    o.emissive = 0.0;
    o.spec = 0.0;
    let facing = clamp(dot(n, v), 0.0, 1.0);
    let graze = 1.0 - facing;
    if (id == 1) {
        // IRIDESCENT: hue follows the viewing angle, so a curved flank shows
        // a whole spectrum at once and it MOVES as you orbit
        o.albedo = hue_shift(base, graze * 4.2 + t * 0.35 + phase);
        o.spec = 0.35 * graze;
        o.emissive = 0.10 * graze;
    } else if (id == 2) {
        // ENERGY FLOW: bands running nose to tail along the body's own axis
        let band = sin(p.z * 6.0 - t * 5.0 + phase);
        let hot = smoothstep(0.55, 1.0, band);
        o.albedo = mix(base, vec3(0.55, 0.95, 1.0), hot * 0.7);
        o.emissive = hot * 0.9;
    } else if (id == 3) {
        // BREATHING: a slow cycle, with the seams (steep normals) brightest
        // on the rise — a machine idling rather than a decal
        let breath = 0.5 + 0.5 * sin(t * 1.15 + phase);
        let seam = pow(graze, 3.0);
        o.emissive = breath * (0.25 + seam * 1.5);
        o.albedo = mix(base, base * 1.35, breath * 0.5);
    } else if (id == 4) {
        // LIQUID CHROME: almost no colour of its own, all reflection
        o.albedo = base * 0.10 + vec3(0.06);
        o.spec = 0.95;
    } else if (id == 5) {
        // SCANLINE: a sweep that traces the bodywork, brightest where the
        // surface turns away — so the sweep draws the car's shape
        let sweep = fract(p.y * 3.0 - t * 0.9 + phase);
        let line = smoothstep(0.94, 1.0, sweep) + smoothstep(0.04, 0.0, sweep);
        o.emissive = line * (0.4 + graze);
        o.albedo = mix(base, vec3(0.7, 0.9, 1.0), line * 0.5);
        o.spec = 0.25;
    } else if (id == 6) {
        // PLASMA: drifting cell noise in the BODY's space, so the slick
        // belongs to the car instead of sliding over it
        let q = p * 5.0 + vec3(t * 0.35, t * 0.2, -t * 0.3);
        let a1 = sin(q.x) * sin(q.y * 1.3) * sin(q.z * 0.8);
        let a2 = sin(q.x * 1.7 + 1.3) * sin(q.z * 1.1);
        let m = 0.5 + 0.5 * (a1 * 0.6 + a2 * 0.4);
        o.albedo = hue_shift(base, m * 3.0 + phase);
        o.emissive = pow(m, 3.0) * 0.6;
        o.spec = 0.4;
    } else if (id == 7) {
        // CIRCUIT: a lattice of traces with pulses travelling along them
        let g = abs(fract(p.xz * 7.0) - 0.5);
        let trace = smoothstep(0.46, 0.5, max(g.x, g.y));
        let pulse = smoothstep(0.7, 1.0, sin(p.x * 4.0 + p.z * 3.0 - t * 4.0 + phase));
        o.albedo = mix(base * 0.55, vec3(0.25, 1.0, 0.75), trace * 0.8);
        o.emissive = trace * (0.25 + pulse * 1.3);
    } else if (id == 8) {
        // FORGE: heat driven by SPEED rather than time — the leading edges
        // glow the harder you drive
        let lead = clamp(-dot(n, vec3(0.0, 0.0, 1.0)), 0.0, 1.0);
        let heat = clamp(speed * (0.35 + lead), 0.0, 1.0);
        o.albedo = mix(base * 0.5, vec3(1.0, 0.42, 0.10), heat);
        o.emissive = heat * heat * 1.6;
    } else if (id == 10) {
        // HUNTER: brightest on the side facing the ball. A finish that
        // knows where the play is — the car glows toward it as you turn,
        // which no texture can do because a texture has no idea.
        let toward = clamp(dot(n, normalize(ball)), 0.0, 1.0);
        let hot = pow(toward, 2.5);
        o.albedo = mix(base * 0.6, vec3(1.0, 0.35, 0.15), hot);
        o.emissive = hot * 1.4;
        o.spec = 0.3;
    } else if (id == 11) {
        // AFTERBURNER: dark at rest, and the panels light from the tail
        // forward as boost comes on — the car itself is the boost meter
        let along = clamp((p.z + 0.6) * 0.9, 0.0, 1.0);
        let lit = clamp(boost * 1.6 - along, 0.0, 1.0);
        o.albedo = mix(base * 0.45, vec3(0.30, 0.75, 1.0), lit);
        o.emissive = lit * 1.8;
        o.spec = 0.2 + 0.4 * lit;
    } else if (id == 12) {
        // GHOST: solid when you are working, translucent when you coast.
        // The fresnel keeps the silhouette readable, so it fades without
        // disappearing — you can still tell where the car is.
        let effort = clamp(max(boost, speed), 0.0, 1.0);
        let rim = pow(graze, 2.0);
        o.albedo = mix(vec3(0.55, 0.85, 1.0) * (0.25 + rim), base, effort);
        o.emissive = rim * (1.1 - effort * 0.7);
        o.spec = 0.55;
    } else if (id == 13) {
        // THERMAL: false colour by how square the surface is to the way you
        // are travelling — the parts pushing air go white, the rest stay
        // cold. Reads the car's own aerodynamics.
        let bite = clamp(dot(n, normalize(vec3(0.0, 0.0, -1.0))), 0.0, 1.0) * speed;
        let cold = vec3(0.05, 0.10, 0.35);
        let warm = vec3(1.0, 0.45, 0.05);
        let hot = vec3(1.0, 1.0, 0.92);
        var c = mix(cold, warm, clamp(bite * 1.8, 0.0, 1.0));
        c = mix(c, hot, clamp(bite * 2.6 - 1.0, 0.0, 1.0));
        o.albedo = c;
        o.emissive = clamp(bite * 1.4 - 0.2, 0.0, 1.0);
    } else if (id == 14) {
        // AURORA: curtains that hang in the air rather than on the paint —
        // driven by world height, so they drift past as the car rises
        let curtain = sin(pw.y * 2.2 + t * 0.8 + sin(pw.x * 0.5 + t * 0.3) * 1.6);
        let band = smoothstep(0.2, 1.0, curtain);
        o.albedo = mix(base * 0.5, hue_shift(vec3(0.2, 1.0, 0.6), pw.y * 0.4 + t * 0.2), band * 0.85);
        o.emissive = band * 0.9;
        o.spec = 0.25;
    } else if (id == 15) {
        // CRYSTAL: the surface refracts. The reflection is offset along the
        // normal's own tangent, so the world seen in the paint is displaced
        // from the world behind it — a lens rather than a mirror.
        o.albedo = base * 0.25 + vec3(0.05, 0.06, 0.09);
        o.spec = 0.85;
        let facet = sin(p.x * 14.0) * sin(p.y * 13.0) * sin(p.z * 12.0);
        o.emissive = smoothstep(0.75, 1.0, abs(facet)) * 0.5;
    } else if (id == 16) {
        // MOMENTUM: the paint keeps score. `phase2` carries the combo, so
        // the car gets hotter the longer you keep the ball alive and cools
        // the moment you drop it — a finish that is a running commentary.
        let heat = clamp(combo, 0.0, 1.0);
        o.albedo = mix(base * 0.7, hue_shift(vec3(1.0, 0.55, 0.1), heat * 2.4), heat);
        o.emissive = heat * heat * 1.5;
        o.spec = 0.2 + 0.4 * heat;
    } else if (id == 17) {
        // IMPACT: rings spreading from wherever the car last hit something.
        // The wave travels outward in BODY space from the contact point, so
        // it rolls across the panels that actually took the blow.
        let d = length(p - hit.xyz);
        let age = hit.w;
        let front = age * 3.2;
        let ring = smoothstep(0.25, 0.0, abs(d - front)) * clamp(1.0 - age * 0.8, 0.0, 1.0);
        o.albedo = mix(base, vec3(1.0, 0.95, 0.85), ring * 0.8);
        o.emissive = ring * 2.0;
        o.spec = 0.3;
    } else if (id == 18) {
        // CONSTELLATION: stars fixed in the WORLD, not on the paint. The
        // body moves through the field, so points appear and pass over it
        // like light through a window rather than a texture riding along.
        let g = fract(pw * 1.7) - 0.5;
        let star = smoothstep(0.42, 0.5, 0.5 - length(g));
        let twinkle = 0.6 + 0.4 * sin(t * 3.0 + pw.x * 4.0 + pw.z * 3.0);
        o.albedo = mix(base * 0.35, vec3(0.85, 0.90, 1.0), star * twinkle);
        o.emissive = star * twinkle * 1.6;
        o.spec = 0.35;
    } else if (id == 19) {
        // MIRAGE: the paint will not hold still. The surface normal is bent
        // by a travelling wave before anything is shaded with it, so the
        // reflection swims and the highlights slide across the panels.
        let w1 = sin(p.z * 9.0 - t * 2.2) * sin(p.y * 7.0 + t * 1.3);
        o.albedo = hue_shift(base, w1 * 0.7 + graze * 1.2);
        o.emissive = smoothstep(0.6, 1.0, abs(w1)) * 0.35;
        o.spec = 0.45 + 0.35 * graze;
    } else if (id == 9) {
        // MOIRE: two lattices at slightly different scales, so the
        // interference pattern shifts with both angle and time
        let s1 = sin(p.x * 22.0 + t * 0.7);
        let s2 = sin(p.x * 23.4 - t * 0.5 + graze * 6.0);
        let m = 0.5 + 0.5 * s1 * s2;
        o.albedo = hue_shift(base, m * 2.2 + graze * 2.0);
        o.emissive = pow(m, 6.0) * 0.8;
        o.spec = 0.5 * m;
    }
    return o;
}

fn map_d_from(p: vec3<f32>, c: ptr<function, Cands>, skip_ter: bool, first: u32) -> f32 {
    var d = 1e6;
    var db = 1e6; // fillet group: tubes + junction rooms smooth-union
    if (!skip_ter) {
        d = p.y - terrain_h(p.x, p.z);
    }
    for (var j = first; j < (*c).n; j = j + 1u) {
        let id = (*c).id[j];
        let s = RTS[id];
        // BOUNDING-SPHERE REJECTION. a.w is the shape's bounding radius, so
        // this is a true lower bound on its distance — and it costs one
        // length() against a full rotate-and-evaluate. A candidate that
        // cannot beat the running minimum is skipped outright, and because
        // `d` shrinks as the loop proceeds, later candidates are rejected
        // more and more often.
        let bd = length(p - s.a.xyz) - s.a.w;
        let blended = s.e.y >= 16.0;
        if (blended) {
            // the smooth-min still pulls from a little beyond the surface
            if (bd >= db + 1.8) { continue; }
        } else if (bd >= d + 0.25) {
            continue;
        }
        let sd = shape_sdf(id, p);
        if (blended) {
            db = smin(db, sd, 1.8);
        } else {
            d = min(d, sd);
        }
    }
    d = smin(d, db, 0.25); // match physics: the raster has no fillet // built form fillets into the terrain as well
    return d;
}

fn map_d(p: vec3<f32>, c: ptr<function, Cands>, skip_ter: bool) -> f32 {
    return map_d_from(p, c, skip_ter, 0u);
}

/// Which surface is at p (after a hit): -1 terrain, else shape index.
fn map_id(p: vec3<f32>, c: ptr<function, Cands>) -> i32 {
    var best = p.y - terrain_h(p.x, p.z);
    var id = -1;
    for (var j = 0u; j < (*c).n; j = j + 1u) {
        let d = shape_sdf((*c).id[j], p);
        if (d < best) {
            best = d;
            id = i32((*c).id[j]);
        }
    }
    return id;
}

fn map_normal(p: vec3<f32>, c: ptr<function, Cands>) -> vec3<f32> {
    let e = 0.05;
    let k0 = vec3(1.0, -1.0, -1.0);
    let k1 = vec3(-1.0, -1.0, 1.0);
    let k2 = vec3(-1.0, 1.0, -1.0);
    let k3 = vec3(1.0, 1.0, 1.0);
    return normalize(
        k0 * map_d(p + k0 * e, c, false) + k1 * map_d(p + k1 * e, c, false) +
        k2 * map_d(p + k2 * e, c, false) + k3 * map_d(p + k3 * e, c, false));
}

/// Ray-traced soft sun shadow: cone march toward the light through the same
/// SDF the ball bounces off. k sets penumbra width.
fn soft_shadow(p: vec3<f32>, l: vec3<f32>, pdist: f32) -> f32 {
    let reach = mix(80.0, 230.0, clamp(G.rt.y, 0.0, 1.0));
    if (pdist > reach) { return 1.0; }             // fog owns it out there
    let tmax = mix(60.0, 130.0, clamp(G.rt.y, 0.0, 1.0));
    var c = gather(p, l, tmax, -1);
    if (c.n == 0u && p.y > terrain_h(p.x, p.z) + 40.0) { return 1.0; } // high sky: clear
    var res = 1.0;
    var t = 0.12;
    let lod = clamp(pdist / 150.0, 0.0, 1.0);
    // the dial scales the STEP budget, which is what actually costs:
    // measured, shortening rays to a quarter of their range saved only 10%
    // of the evaluations, because a ray terminates on a HIT long before it
    // reaches tmax. A control that moved only tmax would change what you see
    // and not what you pay for it.
    let steps = max(6, i32(mix(20.0 + 14.0 * G.rt.y, 9.0, lod) * clamp(G.ray2.x, 0.25, 1.4)));
    let skip_ter = p.y > 17.0 && l.y > 0.05;
    // The shadow march is monotonic in t, exactly like the reflection, so it
    // can retire candidates it has already gone past. It never did — the
    // machinery was written for reflections and the other two ray systems
    // were left walking the whole list at every step.
    var first = 0u;
    for (var i = 0; i < steps; i = i + 1) {
        loop {
            if (first >= c.n || c.t1[first] > t) { break; }
            first = first + 1u;
        }
        let d = map_d_from(p + l * t, &c, skip_ter, first);
        res = min(res, 9.0 * d / t);
        t = t + clamp(d, 0.06 + lod * 0.2, 8.0);
        if (res < 0.015 || t > tmax) { break; }
    }
    return clamp(res, 0.0, 1.0);
}

/// Short-range SDF ambient occlusion.
fn calc_ao(p: vec3<f32>, n: vec3<f32>, pdist: f32) -> f32 {
    // AO is the first ray system to go at low quality
    if (pdist > 120.0 || G.rt.y < 0.45) { return 1.0; }
    var c = gather(p, n, 3.0, -1);
    // nothing within reach and well clear of the ground: nothing to occlude
    // by, so skip four map_d calls that can only return "no occlusion"
    if (c.n == 0u && p.y > terrain_h(p.x, p.z) + 3.0) { return 1.0; }
    var occ = 0.0;
    var w = 1.0;
    // FEWER RAYS AT LOW QUALITY. Each probe is a whole ray, so dropping to
    // two halves this system's cost outright — unlike a step-count change,
    // which only makes each ray cheaper.
    let probes = i32(clamp(G.ray2.y, 2.0, 4.0));
    // The probes are weighted 1, 0.6, 0.36, 0.216 — so two of them collect
    // only 62% of the occlusion four would. Without correcting for that,
    // turning quality down would visibly BRIGHTEN every contact shadow
    // instead of simply approximating it more coarsely.
    var wsum = 0.0;
    for (var i = 1; i <= probes; i = i + 1) {
        let h = 0.15 + 0.5 * f32(i);
        let d = map_d(p + n * h, &c, false);
        occ = occ + (h - d) * w;
        wsum = wsum + w;
        w = w * 0.6;
    }
    let full = 1.0 + 0.6 + 0.36 + 0.216;
    occ = occ * (full / max(wsum, 0.001));
    return clamp(1.0 - occ * 0.55, 0.25, 1.0);
}

/// One real reflection bounce: march the world, shade the hit with the same
/// sun/fog/emissive model, fall back to the sky.
fn trace_reflection(ro: vec3<f32>, rd: vec3<f32>, skip: i32, base_dist: f32) -> vec3<f32> {
    return trace_reflection_w(ro, rd, skip, base_dist, 1.0);
}

/// `weight` is how much of the result will actually survive the blend. A
/// reflection contributing a tenth of the pixel does not deserve a full
/// 260 m march — the far end of it is fog-tinted sky either way.
fn trace_reflection_w(ro: vec3<f32>, rd: vec3<f32>, skip: i32, base_dist: f32, weight: f32) -> vec3<f32> {
    let qn = clamp(G.rt.y, 0.0, 1.0);
    if (qn < 0.35 && base_dist > 70.0) { return sky_color(rd); }
    let lod = clamp(base_dist / 150.0, 0.0, 1.0);
    // RAY DISTANCE is a direct control, not a quality tier: the march costs
    // candidates x steps and steps scale with how far a ray may travel, so
    // this is the most immediate performance dial in the renderer. Turning
    // it down shortens what reflections can SEE — distant geometry fades to
    // sky — rather than making everything blurrier.
    let tmax = mix(260.0, 90.0, lod)
        * mix(0.4, 1.0, qn)
        * mix(0.35, 1.0, clamp(weight * 2.2, 0.0, 1.0))
        * clamp(G.ray2.x, 0.2, 2.0);
    // PORTAL-AWARE MARCH: the ray walks in its own frame (ro2/rd2) so that
    // when a step crosses a portal face the ray is teleported and simply
    // keeps going — the far side is then genuinely visible THROUGH the
    // portal in reflections and in the portal's own surface. Two hops, so a
    // pair facing each other terminates instead of recursing forever.
    var ro2 = ro;
    var rd2 = rd;
    // one bounds test up front: if no portal mouth is within reach of this
    // ray at all, the per-step hop check is skipped entirely. That test was
    // eight quaternion transforms EVERY march step on portal maps.
    var near_portal = false;
    {
        let np = i32(G.pcount.x);
        for (var i = 0; i < np; i = i + 1) {
            let pp = G.portals[u32(i) * 2u];
            let to = pp.xyz - ro;
            let along = clamp(dot(to, rd), 0.0, tmax);
            if (length(to - rd * along) < pp.w + 3.0) {
                near_portal = true;
                break;
            }
        }
    }
    var c = gather(ro2, rd2, tmax, skip);
    var t = 0.06;
    var hit = false;
    var hops = 0;
    // the reflection march takes the dial as well — it is the expensive one
    let steps = max(8, i32(mix(30.0 + 18.0 * G.rt.y, 14.0, lod) * mix(0.5, 1.0, qn) * clamp(G.ray2.x, 0.25, 1.4)));
    let skip_ter = ro.y > 17.0 && rd.y > 0.05;
    var first = 0u;
    for (var i = 0; i < steps; i = i + 1) {
        let p = ro2 + rd2 * t;
        // retire candidates the ray has already gone past: their bounding
        // spheres are behind us and can never be hit again
        loop {
            if (first >= c.n || c.t1[first] > t) { break; }
            first = first + 1u;
        }
        let d = map_d_from(p, &c, skip_ter, first);
        if (d < 0.004 * t + 0.006) { hit = true; break; }
        let nt = t + d * 0.96;
        if (hops < 2 && near_portal) {
            var np = vec3(0.0);
            var nd = vec3(0.0);
            if (portal_hop(p, ro2 + rd2 * nt, rd2, &np, &nd)) {
                ro2 = np;
                rd2 = nd;
                t = 0.06;
                hops = hops + 1;
                c = gather(ro2, rd2, tmax, skip);
                first = 0u;
                continue;
            }
        }
        t = nt;
        if (t > tmax) { break; }
    }
    if (!hit) {
        return sky_color(rd2);
    }
    let p = ro2 + rd2 * t;
    let id = map_id(p, &c);
    var alb = G.gcol.rgb;
    var em = 0.0;
    if (id >= 0) {
        alb = RTS[id].d.rgb;
        em = RTS[id].d.w;
        if (u32(RTS[id].e.w) == 4u) {
            // the ball: rotate into ball space and paint its panel bands so
            // reflections show a real spinning ball, not a gray marble
            let s4 = RTS[id];
            let iq = s4.c;
            let ld = normalize(qrot(iq, p - s4.a.xyz));
            let bands = sin(ld.x * 9.0) * sin(ld.y * 9.0) * sin(ld.z * 9.0);
            let hexish = smoothstep(0.06, 0.22, abs(bands));
            alb = mix(vec3(0.95, 0.45, 0.12), alb, hexish);
        }
    }
    let n = map_normal(p, &c);
    let l = normalize(G.sun.xyz);
    let dif = max(dot(n, l), 0.0);
    let amb = mix(vec3(0.40, 0.36, 0.33), vec3(0.52, 0.59, 0.71), n.y * 0.5 + 0.5);
    var col = alb * (amb + vec3(1.0, 0.97, 0.90) * dif * 0.9);
    col = mix(col, alb * 1.7 + vec3(0.12), clamp(em, 0.0, 1.0));
    // fog over the whole light path (eye → surface → hit)
    let f = 1.0 - exp(-(base_dist + t) * G.fog.w);
    return mix(col, G.fog.rgb, clamp(f, 0.0, 0.94));
}

// -------------------------------------------------------------- scene ----
struct Object {
    model: mat4x4<f32>,
    nrm: mat4x4<f32>,
    color: vec4<f32>,      // base color factor (linear), a = alpha
    params: vec4<f32>,
    // x mode (0 plain, 1 terrain grid), y emissive, z spec, w rt self id + 1
    skin: vec4<f32>,       // x id, y phase, z intensity, w speed 0..1
    skin2: vec4<f32>,      // xyz ball direction in body space, w boost 0..1
    skin3: vec4<f32>,      // xyz last contact in body space, w seconds since
};
@group(1) @binding(0) var<uniform> O: Object;
@group(2) @binding(0) var t_col: texture_2d<f32>;
@group(2) @binding(1) var s_col: sampler;

struct VsIn {
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) uv: vec2<f32>,
};
struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) wpos: vec3<f32>,
    @location(1) wnrm: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

@vertex
fn vs_scene(in: VsIn) -> VsOut {
    var out: VsOut;
    let wp = O.model * vec4(in.pos, 1.0);
    out.wpos = wp.xyz;
    out.clip = G.view_proj * wp;
    out.wnrm = normalize((O.nrm * vec4(in.nrm, 0.0)).xyz);
    out.uv = in.uv;
    return out;
}

fn grid_shade(p: vec2<f32>, step: f32, width: f32) -> f32 {
    let g = abs(fract(p / step - 0.5) - 0.5) * step;
    let d = min(g.x, g.y);
    return 1.0 - smoothstep(width, width * 2.2, d);
}

@fragment
fn fs_scene(in: VsOut) -> @location(0) vec4<f32> {
    var n = normalize(in.wnrm);
    let v = normalize(G.cam.xyz - in.wpos);
    if (dot(n, v) < 0.0) { n = -n; }           // two-sided safety for thin bits
    let l = normalize(G.sun.xyz);

    var base = O.color.rgb * textureSample(t_col, s_col, in.uv).rgb;

    let is_terrain = O.params.x > 0.5;
    if (is_terrain) {                          // terrain: painterly grid + tone patches
        let big = grid_shade(in.wpos.xz, 24.0, 0.10);
        let small = grid_shade(in.wpos.xz, 4.0, 0.035);
        base *= 1.0 - small * 0.07 - big * 0.12;
        let mottle = sin(in.wpos.x * 0.021 + 4.7) * sin(in.wpos.z * 0.017 + 1.3);
        base *= 1.0 + mottle * 0.05;
    }

    // ---- the ray-traced bits (soft shadow + AO + reflection) ------------
    let rays = G.rt.x > 0.5;
    let self_id = i32(O.params.w) - 1;
    let dist0 = length(G.cam.xyz - in.wpos);
    var shadow = 1.0;
    var ao = 1.0;
    if (rays) {
        let sp = in.wpos + n * 0.06;
        // shadow only ever scales the diffuse term, so faces pointing away
        // from the sun (roughly half of everything) skip the march for an
        // IDENTICAL image — the honest high-quality win
        if (O.params.y < 0.5 && dot(n, l) > 0.02) {
            shadow = soft_shadow(sp, l, dist0);
        }
        // glowing surfaces (hoops, launch rings, orbs) skip occlusion:
        // emission dominates their shading and the rays are pure cost
        if (O.params.y < 0.5) {
            ao = calc_ao(in.wpos, n, dist0);
        }
    }

    // ---- SKIN ----------------------------------------------------------
    // Applied before lighting, so a finish changes what the light falls on
    // rather than being painted over the top of it. Skins may also raise the
    // spec, which feeds the reflection march below — that is how LIQUID
    // CHROME becomes nothing but what the ray tracer sees.
    var skin_em = 0.0;
    var skin_spec = 0.0;
    let skin_id = i32(O.skin.x);
    if (skin_id > 0) {
        // in the BODY's own space, so a moving car carries its finish
        let lp = (in.wpos - O.model[3].xyz);
        let sk = apply_skin(skin_id, base, lp, in.wpos, n, v, G.cam.w, O.skin.w, O.skin.y, O.skin2.xyz, O.skin2.w, O.skin3, O.skin.z);
        base = sk.albedo;
        skin_em = sk.emissive * O.skin.z;
        skin_spec = sk.spec;
    }

    let dif = max(dot(n, l), 0.0);
    // hemisphere ambient: cool sky from above, warm ground bounce from below
    let amb = mix(vec3(0.42, 0.37, 0.34), vec3(0.55, 0.62, 0.74), n.y * 0.5 + 0.5);
    let h = normalize(l + v);
    let spec = pow(max(dot(n, h), 0.0), 42.0) * (0.18 + max(O.params.z, skin_spec) * 0.6);

    var col = base * (amb * ao + vec3(1.0, 0.97, 0.90) * dif * 0.95 * shadow)
        + vec3(spec) * (0.25 + 0.75 * shadow);
    col = mix(col, base * 1.7 + vec3(0.12), clamp(max(O.params.y, skin_em), 0.0, 1.0)); // emissive pop

    // real mirror reflections where the material (or a wet map floor) wants
    // them - and only inside reflection range (fog owns the far field)
    if (rays && O.params.y < 0.35 && dist0 < 150.0) {
            var f0 = max(O.params.z, skin_spec) * 0.55;
        if (is_terrain) { f0 = G.rt.w; }
        // ROUGH GLASS (tubes, junction rooms): a strong grazing reflection
        // that scatters. The mirror-smooth version read as coloured film;
        // jittering the reflected ray per-fragment and lifting the fresnel
        // is what makes it look like thick, slightly frosted glass.
        let glassy = O.params.x > 0.5 && O.params.x < 1.5;
    // ---- THE VIEW THROUGH A PORTAL ----------------------------------
    // A portal face shows what is on the far side, live: take the view ray,
    // carry it through the pair transform, and march the world from the
    // partner's mouth. Cars and the ball are in the marcher's shape list,
    // so they appear through it too — which is the point, you can read an
    // attack coming before it arrives. Three jittered samples soften the
    // result just enough to read as a portal rather than a window.
    if (O.params.x >= 1.5 && rays) {
        let pi = u32(O.params.x - 2.0) * 2u;
        let pa = G.portals[pi].xyz;
        let qa = G.portals[pi + 1u];
        let j = select(i32(pi / 2u) - 1, i32(pi / 2u) + 1, (pi / 2u) % 2u == 0u);
        let jb = u32(max(j, 0)) * 2u;
        let pb = G.portals[jb].xyz;
        let qb = G.portals[jb + 1u];
        let vd = normalize(in.wpos - G.cam.xyz);
        var loc = qrot(qconj(qa), in.wpos - pa);
        var ldir = qrot(qconj(qa), vd);
        loc = vec3(-loc.x, loc.y, -loc.z);
        ldir = vec3(-ldir.x, ldir.y, -ldir.z);
        let ro2 = pb + qrot(qb, loc) + qrot(qb, vec3(0.0, 0.0, 1.0)) * 0.25;
        let rd0 = normalize(qrot(qb, ldir));
        var acc3 = vec3(0.0);
        for (var k = 0; k < 2; k = k + 1) {
            // a smooth offset per sample rather than a hash: two rays that
            // differ by a fixed small angle soften the image without the
            // salt-and-pepper a random direction gives
            let ang = f32(k) * 2.399 + in.wpos.x * 0.7 + in.wpos.z * 0.5;
            let jit = vec3(cos(ang), sin(ang), cos(ang * 1.7)) * 0.022;
            // base_dist 0 so the low-quality early-out cannot turn the view
            // into flat sky — a portal you cannot see through is just a wall
            acc3 = acc3 + trace_reflection(ro2, normalize(rd0 + jit), -1, 0.0);
        }
        // lift it: the far side is lit by its own scene, and a portal reads
        // as a hole in the world only if it is at least as bright as one
        var pcol = (acc3 / 2.0) * 1.35;
        pcol = mix(pcol, O.color.rgb, 0.09); // just enough tint to tell pairs apart
        let edge = pow(1.0 - max(dot(n, v), 0.0), 3.0);
        pcol = pcol + O.color.rgb * edge * 0.55;
        return vec4(pcol, 1.0);
    }
        if (glassy) {
            // Glass raised f0 as high as 0.92, which sent nearly every glass
            // fragment down the FULL reflection march — and on a map like
            // PERPLEXUS half the screen is glass. Grazing angles still get a
            // strong reflection, but the ceiling comes down and the falloff
            // is gentler, so a face-on pane takes the cheap path.
            let fres = pow(1.0 - max(dot(n, v), 0.0), 4.0);
            f0 = mix(0.10, 0.72, fres);
        }
        // low-spec objects trade the full reflection march for a sky-tinted
        // sheen: visually a whisper, computationally half the glass bill
        let cheap_refl = !is_terrain && f0 < 0.14;
        let fres = f0 + (1.0 - f0) * pow(1.0 - max(dot(n, v), 0.0), 5.0);
        let amount = clamp(fres, 0.0, 0.85) * step(0.06, f0);
        if (amount > 0.03) {
            var rr = reflect(-v, n);
            if (glassy) {
                // FROSTED, NOT TILED. This used to hash floor(wpos * 7),
                // which quantises the world into 14 cm cells — every
                // fragment inside a cell got the SAME jitter, so the glass
                // came out in visible blocks. A hash cannot be the answer
                // here anyway: without temporal accumulation a per-pixel
                // random direction is just static.
                //
                // A continuous ripple is both cheaper and correct: three
                // sines across the surface wobble the reflection smoothly,
                // which is what slightly imperfect glass actually does.
                let w = in.wpos * 2.9;
                let rip = vec3(
                    sin(w.y * 2.3 + w.z * 1.7),
                    sin(w.z * 2.1 + w.x * 1.9),
                    sin(w.x * 2.7 + w.y * 1.3)
                );
                rr = normalize(rr + rip * 0.055);
            }
            if (cheap_refl) {
                col = mix(col, sky_color(rr), amount * 0.8);
            } else {
                let rc_raw = trace_reflection_w(in.wpos + n * 0.05, rr, self_id, dist0, amount);
                // DENOISE BY CURVATURE.
                //
                // One reflection ray per pixel is exact where the surface is
                // flat and noisy where it is not: on a curved panel adjacent
                // pixels shoot in visibly different directions and the result
                // sparkles. Screen-space derivatives measure exactly that —
                // how fast the reflected direction is changing ACROSS the
                // pixel — so where it changes fast, fall back toward the
                // smooth sky term rather than trusting one sample.
                //
                // It costs two derivative instructions and no extra rays,
                // which is the point: the cheapest denoiser is the one that
                // knows when a sample is not worth believing.
                let spread = length(dpdx(rr)) + length(dpdy(rr));
                let trust = 1.0 - clamp(spread * 5.5, 0.0, 0.75);
                let rc = mix(sky_color(rr), rc_raw, trust);
                col = mix(col, rc, amount);
            }
        }
    }

    // distance fog toward horizon color
    let f = 1.0 - exp(-dist0 * G.fog.w);
    // fade fog with view height a touch so high aerials stay clear
    let fh = f * mix(1.0, 0.55, clamp(G.cam.y / 60.0, 0.0, 1.0));
    col = mix(col, G.fog.rgb, clamp(fh, 0.0, 0.94));

    return vec4(col, O.color.a); // raw HDR; the composite pass grades it
}

// ----------------------------------------------------------------- ui ----
struct UiIn {
    @location(0) pos: vec2<f32>,
    @location(1) color: vec4<f32>,
};
struct UiOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_ui(in: UiIn) -> UiOut {
    var out: UiOut;
    out.clip = vec4(in.pos, 0.0, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_ui(in: UiOut) -> @location(0) vec4<f32> {
    return in.color;
}

// ------------------------------------------------------------- post fx ----
// Bloom + final grade. The scene resolves into an HDR target; a bright pass
// pulls everything over the threshold into a half-res buffer, a separable
// gaussian softens it, and the composite adds it back, then runs the whole
// screen_fx grade (exposure -> ACES -> vignette -> grain) exactly once.

@group(1) @binding(5) var t_post: texture_2d<f32>;
@group(1) @binding(6) var s_post: sampler;
struct PostP {
    v: vec4<f32>, // xy = texel size, zw = blur direction
};
@group(1) @binding(7) var<uniform> P: PostP;
@group(1) @binding(8) var t_post2: texture_2d<f32>;

struct FullOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_full(@builtin(vertex_index) i: u32) -> FullOut {
    var o: FullOut;
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    o.clip = vec4(xy * 2.0 - 1.0, 0.0, 1.0);
    o.uv = vec2(xy.x, 1.0 - xy.y);
    return o;
}

@fragment
fn fs_bright(in: FullOut) -> @location(0) vec4<f32> {
    let c = textureSample(t_post, s_post, in.uv).rgb;
    let l = max(max(c.r, c.g), c.b);
    let m = max(l - G.post.y, 0.0);
    let w = m / (m + 0.6); // soft knee
    return vec4(c * w, 1.0);
}

@fragment
fn fs_blur(in: FullOut) -> @location(0) vec4<f32> {
    let step = P.v.zw * P.v.xy * 1.5;
    var wts = array<f32, 4>(0.1945946, 0.1216216, 0.054054, 0.016216);
    var acc = textureSample(t_post, s_post, in.uv).rgb * 0.227027;
    for (var k: i32 = 1; k <= 4; k = k + 1) {
        let o = step * f32(k);
        acc = acc + textureSample(t_post, s_post, in.uv + o).rgb * wts[k - 1];
        acc = acc + textureSample(t_post, s_post, in.uv - o).rgb * wts[k - 1];
    }
    return vec4(acc, 1.0);
}

@fragment
fn fs_composite(in: FullOut) -> @location(0) vec4<f32> {
    let hdr = textureSample(t_post, s_post, in.uv).rgb;
    let bloom = textureSample(t_post2, s_post, in.uv).rgb;
    var col = hdr + bloom * G.post.x;
    // subtle screen-door: checker dim, configurable (post.z)
    let m = f32((u32(in.clip.x) + u32(in.clip.y)) & 1u);
    col *= 1.0 - G.post.z * 0.5 * m; // halved: the checker read too heavy on glass
    return vec4(screen_fx(col, in.clip.xy), 1.0);
}
