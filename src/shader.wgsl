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
    hmap: vec4<f32>,       // x half extent, y texels, zw = screen w,h in px
    gcol: vec4<f32>,       // rgb terrain base color, w shape count
    cine: vec4<f32>,       // x exposure, y vignette, z grain, w filmic tonemap
    post: vec4<f32>,       // x bloom strength, y bloom threshold, zw spare
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
    n: u32,
    id: array<u32, 16>,
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
    if ((*c).n < KMAX) {
        (*c).id[(*c).n] = i;
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

fn map_d(p: vec3<f32>, c: ptr<function, Cands>, skip_ter: bool) -> f32 {
    var d = 1e6;
    var db = 1e6; // fillet group: tubes + junction rooms smooth-union
    if (!skip_ter) {
        d = p.y - terrain_h(p.x, p.z);
    }
    for (var j = 0u; j < (*c).n; j = j + 1u) {
        let id = (*c).id[j];
        let sd = shape_sdf(id, p);
        if (RTS[id].e.y >= 16.0) {
            db = smin(db, sd, 2.2);
        } else {
            d = min(d, sd);
        }
    }
    d = min(d, db);
    return d;
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
    let steps = i32(mix(20.0 + 14.0 * G.rt.y, 9.0, lod));
    let skip_ter = p.y > 17.0 && l.y > 0.05;
    for (var i = 0; i < steps; i = i + 1) {
        let d = map_d(p + l * t, &c, skip_ter);
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
    var occ = 0.0;
    var w = 1.0;
    for (var i = 1; i <= 4; i = i + 1) {
        let h = 0.15 + 0.5 * f32(i);
        let d = map_d(p + n * h, &c, false);
        occ = occ + (h - d) * w;
        w = w * 0.6;
    }
    return clamp(1.0 - occ * 0.55, 0.25, 1.0);
}

/// One real reflection bounce: march the world, shade the hit with the same
/// sun/fog/emissive model, fall back to the sky.
fn trace_reflection(ro: vec3<f32>, rd: vec3<f32>, skip: i32, base_dist: f32) -> vec3<f32> {
    let qn = clamp(G.rt.y, 0.0, 1.0);
    if (qn < 0.35 && base_dist > 70.0) { return sky_color(rd); }
    let lod = clamp(base_dist / 150.0, 0.0, 1.0);
    let tmax = mix(260.0, 90.0, lod) * mix(0.4, 1.0, qn);
    var c = gather(ro, rd, tmax, skip);
    var t = 0.06;
    var hit = false;
    let steps = i32(mix(30.0 + 18.0 * G.rt.y, 14.0, lod) * mix(0.5, 1.0, qn));
    let skip_ter = ro.y > 17.0 && rd.y > 0.05;
    for (var i = 0; i < steps; i = i + 1) {
        let p = ro + rd * t;
        let d = map_d(p, &c, skip_ter);
        if (d < 0.004 * t + 0.006) { hit = true; break; }
        t = t + d * 0.96;
        if (t > tmax) { break; }
    }
    if (!hit) {
        return sky_color(rd);
    }
    let p = ro + rd * t;
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
    params: vec4<f32>,     // x mode (0 plain, 1 terrain grid), y emissive, z spec, w rt self id + 1
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

    let dif = max(dot(n, l), 0.0);
    // hemisphere ambient: cool sky from above, warm ground bounce from below
    let amb = mix(vec3(0.42, 0.37, 0.34), vec3(0.55, 0.62, 0.74), n.y * 0.5 + 0.5);
    let h = normalize(l + v);
    let spec = pow(max(dot(n, h), 0.0), 42.0) * (0.18 + O.params.z * 0.6);

    var col = base * (amb * ao + vec3(1.0, 0.97, 0.90) * dif * 0.95 * shadow)
        + vec3(spec) * (0.25 + 0.75 * shadow);
    col = mix(col, base * 1.7 + vec3(0.12), clamp(O.params.y, 0.0, 1.0)); // emissive pop

    // real mirror reflections where the material (or a wet map floor) wants
    // them - and only inside reflection range (fog owns the far field)
    if (rays && O.params.y < 0.35 && dist0 < 150.0) {
            var f0 = O.params.z * 0.55;
        if (is_terrain) { f0 = G.rt.w; }
        // low-spec objects trade the full reflection march for a sky-tinted
        // sheen: visually a whisper, computationally half the glass bill
        let cheap_refl = !is_terrain && f0 < 0.14;
        let fres = f0 + (1.0 - f0) * pow(1.0 - max(dot(n, v), 0.0), 5.0);
        let amount = clamp(fres, 0.0, 0.85) * step(0.06, f0);
        if (amount > 0.03) {
            let rr = reflect(-v, n);
            if (cheap_refl) {
                col = mix(col, sky_color(rr), amount * 0.8);
            } else {
                let rc = trace_reflection(in.wpos + n * 0.05, rr, self_id, dist0);
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
