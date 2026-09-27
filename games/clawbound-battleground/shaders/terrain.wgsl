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
    event: vec4<f32>,    // x flood rise, y quake shake, z fire glow, w shaft steps
    brush: array<vec4<f32>, 8>, // xyz where a cat is shouldering through, w how hard (signed)
    blast_a: array<vec4<f32>, 4>, // xyz centre, w ring radius
    blast_b: array<vec4<f32>, 4>, // x strength, y width
    cracks: array<vec4<f32>, 8>,  // x, z, radius, how open
    towers: array<vec4<f32>, 12>, // x, z, range, state (see Game::tower_rings)
    sea: vec4<f32>,               // x all under the sea, y caustic strength, z water surface y, w camera underwater
};

@group(0) @binding(0) var<uniform> frame: FrameData;

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2(123.34, 456.21));
    q = q + dot(q, q + 45.32);
    return fract(q.x * q.y);
}
// An integer hash of a cell, 0..1. `hash21` works in f32 and loses its
// low bits on cell indices in the thousands (world x 40 for the snow's
// glitter): the sparkles came out in dotted lines.
fn hash_cell(c: vec2<i32>) -> f32 {
    var h = u32(c.x) * 0x8da6b343u ^ u32(c.y) * 0xd8163841u;
    h = h ^ (h >> 16u);
    h = h * 0x7feb352du;
    h = h ^ (h >> 15u);
    h = h * 0x846ca68bu;
    h = h ^ (h >> 16u);
    return f32(h >> 8u) / 16777216.0;
}
@group(0) @binding(2) var shadow_map: texture_depth_2d;
@group(0) @binding(3) var shadow_sampler: sampler_comparison;

// Coloured point lights (ability glows, channels, lightning) and the cats'
// spheres their shadows are traced against: the smooth pipeline's own
// (smooth.wgsl). Empty outside the ultra preview.
struct Lights {
    info: vec4<f32>,
    pos: array<vec4<f32>, 8>,
    col: array<vec4<f32>, 8>,
    occ: array<vec4<f32>, 32>,
    fur: vec4<f32>,
};
@group(1) @binding(0) var<uniform> lights: Lights;

// Ultra's snowpack (snowpack.rs): the pack's depth over the whole map, and
// the marks -- the share pushed aside by paws, trenches, craters -- in a
// window round the player. `pack.w` is 0 when there is no snow to draw.
@group(1) @binding(1) var snow_pack: texture_2d<f32>;
@group(1) @binding(2) var snow_marks: texture_2d<f32>;
@group(1) @binding(3) var snow_samp: sampler;
struct SnowParams {
    pack: vec4<f32>,   // origin x, z, span, on
    marks: vec4<f32>,  // origin x, z, span, depth scale of the pack texture
};
@group(1) @binding(4) var<uniform> snowp: SnowParams;

fn snow_depth(xz: vec2<f32>) -> f32 {
    if (snowp.pack.w < 0.5) {
        return 0.0;
    }
    let uv = (xz - snowp.pack.xy) / snowp.pack.z;
    return textureSampleLevel(snow_pack, snow_samp, uv, 0.0).r * snowp.marks.w;
}

fn snow_press(xz: vec2<f32>) -> f32 {
    if (snowp.pack.w < 0.5) {
        return 0.0;
    }
    let uv = (xz - snowp.marks.xy) / snowp.marks.z;
    if (uv.x < 0.0 || uv.y < 0.0 || uv.x > 1.0 || uv.y > 1.0) {
        return 0.0;
    }
    return textureSampleLevel(snow_marks, snow_samp, uv, 0.0).r;
}

// How much of the snowpack a surface facing `ny` up carries: all on the
// flat and gentle slopes, none on steep faces or anything facing down.
// (Without it the pack's depth, sampled by x-z alone, was drawn down every
// cliff face and keel under snowy ground, in 8-bit contour bands.)
fn snow_facing(ny: f32) -> f32 {
    return smoothstep(0.55, 0.8, ny);
}

// Loose snow standing here: the pack less what has been pushed aside.
fn snow_surface(xz: vec2<f32>) -> f32 {
    return snow_depth(xz) * (1.0 - snow_press(xz));
}

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

// The point lights on the ground, each shadow ray-traced against the cats:
// a lightning strike throws every cat's shadow across the ground.
fn point_lights(world: vec3<f32>, N: vec3<f32>) -> vec3<f32> {
    var sum = vec3(0.0);
    let nl = i32(lights.info.x);
    let no = i32(lights.info.y);
    for (var i = 0; i < 8; i = i + 1) {
        if (i >= nl) {
            break;
        }
        let lp = lights.pos[i];
        let to_l = lp.xyz - world;
        let dist = length(to_l);
        if (dist >= lp.w) {
            continue;
        }
        let L = to_l / dist;
        let win = clamp(1.0 - pow(dist / lp.w, 4.0), 0.0, 1.0);
        let atten = win * win / (dist * dist * 0.25 + 1.0);
        var vis = 1.0;
        for (var k = 0; k < 32; k = k + 1) {
            if (k >= no) {
                break;
            }
            vis = vis * sphere_soft_shadow(world + N * 0.05, L, lights.occ[k], 8.0, dist);
        }
        sum = sum + lights.col[i].rgb * atten * vis * max(dot(N, L), 0.0);
    }
    return sum;
}

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

// Triplanar detail: the pattern projected along each axis, blended by how
// much the surface faces it. `detail` alone samples world xz, which on a
// cliff smears one texel down the whole face -- the vertical streaks on
// every canyon wall. The side projections put world HEIGHT in the pattern's
// second coordinate, so rock strata become horizontal bands on a cliff.
// Returns (brightness mul, bump as a world-space vec3).
fn tri_detail(w: vec3<f32>, n: vec3<f32>, m: f32, dist: f32) -> vec4<f32> {
    let top = detail(w.xz, m, dist);
    var bw = pow(abs(n), vec3(4.0));
    bw = bw / max(bw.x + bw.y + bw.z, 1e-4);
    if (bw.y > 0.96) {
        // flat ground (almost all of it): one projection, as before
        return vec4(top.x, top.y, 0.0, top.z);
    }
    let sx = detail(w.zy, m, dist); // faces looking along x
    let sz = detail(w.xy, m, dist); // faces looking along z
    let mul = top.x * bw.y + sx.x * bw.x + sz.x * bw.z;
    let bump = vec3(top.y, 0.0, top.z) * bw.y + vec3(0.0, sx.z, sx.y) * bw.x + vec3(sz.y, sz.z, 0.0) * bw.z;
    return vec4(mul, bump);
}

@vertex
fn vs_main(in: VSIn) -> VSOut {
    var out: VSOut;
    // the snowpack lifts the ground it lies on -- only ground that faces
    // up: the pack is a 2D field, and a cliff face or an island's keel
    // under snowy ground must not be lifted (or painted) by it
    let w = vec4<f32>(in.pos + vec3(0.0, snow_surface(in.pos.xz) * snow_facing(in.normal.y), 0.0), 1.0);
    out.clip = frame.view_proj * w;
    out.world = w.xyz;
    out.normal = in.normal;
    out.color = in.color;
    out.shadow_pos = frame.light_view_proj * w;
    out.mat = in.mat;
    return out;
}

// Light shafts THROUGH the canopy. The post pass already smears bright pixels
// toward the sun, which only works with the sun in frame and knows nothing of
// the trees. This marches the view ray through the shadow map instead: air in
// a gap is lit, air under a crown is not, and the difference is the shaft.
// No new bindings -- the shadow map and the light matrix are already here.
// `frame.event.w` is the step count; 0 is off, for GPUs that cannot afford it.
fn shaft_light(world: vec3<f32>) -> f32 {
    let steps = i32(frame.event.w);
    if (steps <= 0) {
        return 0.0;
    }
    let cam = frame.cam_pos.xyz;
    let seg = world - cam;
    let full = length(seg);
    let len = min(full, 70.0);
    let dir = seg / max(full, 0.001);
    // Dither the start so eight steps band as noise, not as stripes.
    let jitter = hash21(world.xz * 3.7 + vec2(frame.params.x * 5.0, world.y));
    var lit = 0.0;
    for (var i = 0; i < steps; i = i + 1) {
        let p = cam + dir * ((f32(i) + jitter) / f32(steps) * len);
        let sp = frame.light_view_proj * vec4(p, 1.0);
        let uv = sp.xy * vec2(0.5, -0.5) + vec2(0.5);
        var s = 1.0; // outside the map is open sky
        if (uv.x > 0.0 && uv.x < 1.0 && uv.y > 0.0 && uv.y < 1.0 && sp.z < 1.0) {
            s = textureSampleCompareLevel(shadow_map, shadow_sampler, uv, sp.z - 0.003);
        }
        lit = lit + s;
    }
    // Forward scattering: shafts are brightest looking toward the sun.
    let toward = max(dot(dir, normalize(frame.sun_dir.xyz)), 0.0);
    let phase = 0.25 + 0.75 * toward * toward * toward;
    // A shaft is CONTRAST, not light. Returning the lit fraction itself added
    // glow to every sunlit column of air, which is haze: a with/without split
    // of one frame showed the whole far field gone milky. Open air (f = 1) and
    // deep shade (f = 0) both contribute nothing; a column the canopy breaks
    // up contributes most.
    let f = lit / f32(steps);
    return 4.0 * f * (1.0 - f) * (len / 70.0) * phase;
}

// The fitted shadow box ends somewhere. Fade to "lit" over its last tenth so
// the edge is a gradient on the ground and not a line.
// shadow_edge, sun_shadow: common_shadow.wgsl

// Shattered ground: the edges of a cell pattern, strongest at the impact and
// fading to its rim. `open` runs 1 -> 0 as it heals: the cracks NARROW first
// (soil swelling shut, fastest in rain -- see groundfx.rs), and what is left
// behind is darker, damper earth that fades last.
fn cell_edge(p: vec2<f32>) -> f32 {
    let ip = floor(p);
    let fp = fract(p);
    var d1 = 8.0;
    var d2 = 8.0;
    for (var j = -1; j <= 1; j = j + 1) {
        for (var i = -1; i <= 1; i = i + 1) {
            let g = vec2(f32(i), f32(j));
            let o = vec2(hash21(ip + g), hash21(ip + g + vec2(17.3, 5.1)));
            let d = length(g + o - fp);
            if (d < d1) {
                d2 = d1;
                d1 = d;
            } else if (d < d2) {
                d2 = d;
            }
        }
    }
    return d2 - d1; // ~0 on an edge between two cells
}

fn cracked(p: vec2<f32>) -> vec2<f32> {
    var dark = 0.0;
    var damp = 0.0;
    for (var i = 0; i < 8; i = i + 1) {
        let c = frame.cracks[i];
        if (c.w <= 0.002) {
            continue;
        }
        let d = distance(p, c.xy);
        if (d >= c.z) {
            continue;
        }
        let fall = 1.0 - d / c.z;
        let e = cell_edge(p * 0.85 + c.xy * 0.31);
        let width = (0.015 + 0.11 * c.w) * (0.4 + 0.6 * fall);
        let line = 1.0 - smoothstep(0.0, width, e);
        dark = max(dark, line * smoothstep(0.0, 0.3, c.w) * smoothstep(0.0, 0.2, fall));
        damp = max(damp, (1.0 - c.w) * fall * 0.35 + c.w * fall * fall * 0.25);
    }
    return vec2(dark, damp);
}

// Where a tower's line is, and what it means to YOU right now.
//   1 yours            thin, calm, team-cool
//   2 theirs           dashed amber: a line you have not crossed
//   3 you are inside   solid red, with a soft band inside it
//   4 aiming at you    the same, pulsing fast
fn tower_ring(p: vec2<f32>) -> vec4<f32> {
    var col = vec3(0.0);
    var a = 0.0;
    for (var i = 0; i < 12; i = i + 1) {
        let t = frame.towers[i];
        if (t.w < 0.5) {
            continue;
        }
        let e = distance(p, t.xy) - t.z; // < 0 inside
        if (e > 0.4 || e < -3.0) {
            continue;
        }
        let line = 1.0 - smoothstep(0.10, 0.24, abs(e));
        let ang = atan2(p.y - t.y, p.x - t.x);
        let dash = step(0.45, fract(ang * t.z / 2.4));
        var c = vec3(0.55, 0.80, 1.0);
        var k = line * 0.24; // yours: present, never loud
        if (t.w > 1.5) {
            c = vec3(1.0, 0.72, 0.20);
            k = line * dash * 0.75;
        }
        if (t.w > 2.5) {
            let fast = select(3.0, 11.0, t.w > 3.5);
            let pulse = 0.75 + 0.25 * sin(frame.params.x * fast);
            c = vec3(1.0, 0.16, 0.10);
            let band = smoothstep(-3.0, 0.0, e) * step(e, 0.0) * 0.22;
            k = max(line, band) * pulse;
        }
        if (k > a) {
            a = k;
            col = c;
        }
    }
    return vec4(col, clamp(a, 0.0, 1.0));
}

@fragment
fn fs_main(in: VSOut) -> @location(0) vec4<f32> {
    let cam_d = distance(in.world, frame.cam_pos.xyz);
    // `mat` is a CATEGORY (1 lane, 2 river pebbles, 6 moss...) and the GPU
    // interpolates it like any other number. Between a lane vertex and a
    // moss vertex it sweeps through 2, which painted a strip of river
    // pebbles along every road edge. So: snap to the nearest real id, and
    // fade that id's pattern out toward plain ground wherever the value is
    // in between -- which is exactly where two materials meet.
    let mat_id = round(in.mat);
    let purity = clamp(1.0 - 2.0 * abs(in.mat - mat_id), 0.0, 1.0);
    let n0 = normalize(in.normal);
    // Materials meet by HEIGHT, not by a fade: each side has a height (the
    // surface field, the material's own pattern raising its stones), and
    // where they meet the higher wins, with a thin transition -- a ragged,
    // natural border (stones poking through, moss creeping over a lane's
    // edge) instead of a smudge.
    let det_a = tri_detail(in.world, n0, 0.0, cam_d);
    let det_b = tri_detail(in.world, n0, mat_id, cam_d);
    let h_a = surface_height(in.world.xz + vec2(17.3, 5.1));
    let h_b = surface_height(in.world.xz) * 0.7 + det_b.x * 0.3;
    let wb = smoothstep(-0.06, 0.06, (purity - 0.5) * 1.6 + (h_b - h_a));
    let det = mix(det_a, det_b, wb);
    // the seam itself darkens a little, so the edge reads as an edge
    let seam = 1.0 - 0.14 * (1.0 - abs(wb * 2.0 - 1.0)) * (1.0 - purity);
    let broken = cracked(in.world.xz);
    // Standing water in the lows of the height field -- the cracks deepest
    // -- flat, dark and glossy; the high ground only damp. Replaces blocky
    // hash-cell puddles over a ground at least 55% wet everywhere: an even
    // gloss over everything.
    let wet = frame.weather.z;
    let h_wet = surface_height(in.world.xz) - broken.x * 0.5 - broken.y * 0.2;
    let pud = water_fill(h_wet, wet, n0.y);
    var N = normalize(in.normal + det.yzw);
    N = normalize(mix(N, vec3(0.0, 1.0, 0.0), pud * 0.9));
    // The snowpack: its surface's own normal, from the fine marks, so a
    // paw print or a trench is a dent the light falls into. `lying` is
    // how much snow is here at all; `loose` how much is still standing.
    let sd = snow_depth(in.world.xz) * snow_facing(normalize(in.normal).y);
    let spress = snow_press(in.world.xz);
    let lying = smoothstep(0.02, 0.08, sd);
    let loose = smoothstep(0.0, 0.05, sd * (1.0 - spress));
    if (lying > 0.0) {
        // the surface is depth x (1 - press): the pack's slope over a wide
        // step (its texels are ~2.6 u, and a narrow difference showed their
        // bilinear seams as straight creases across the snow), the marks'
        // over a narrow one (a print is a few of their 0.2 u cells)
        let xz = in.world.xz;
        let E = 1.3;
        let e = 0.1;
        let fy = snow_facing(normalize(in.normal).y);
        let dx = (snow_depth(xz + vec2(E, 0.0)) - snow_depth(xz - vec2(E, 0.0))) / (2.0 * E) * fy;
        let dz = (snow_depth(xz + vec2(0.0, E)) - snow_depth(xz - vec2(0.0, E))) / (2.0 * E) * fy;
        let px = (snow_press(xz + vec2(e, 0.0)) - snow_press(xz - vec2(e, 0.0))) / (2.0 * e);
        let pz = (snow_press(xz + vec2(0.0, e)) - snow_press(xz - vec2(0.0, e))) / (2.0 * e);
        let hx = dx * (1.0 - spress) - sd * px;
        let hz = dz * (1.0 - spress) - sd * pz;
        let ns = normalize(vec3(-hx, 1.0, -hz));
        N = normalize(mix(N, ns, lying));
    }
    let sun = normalize(frame.sun_dir.xyz);
    let ndl = max(dot(N, sun), 0.0);

    // contact-hardening: crisp where a shadow meets its caster (common_shadow.wgsl)
    let shadow = sun_shadow(in.shadow_pos, 0.0030, in.clip.xy);

    let night = frame.params.w;
    let cloud = frame.sun_col.w;
    var sky = mix(vec3(0.50, 0.60, 0.85), vec3(0.55, 0.57, 0.62), cloud);
    sky = mix(sky, vec3(0.05, 0.07, 0.18), night);
    var hemi = sky * (0.55 + 0.35 * (N.y * 0.5 + 0.5));
    let flash = frame.weather.w;
    hemi = hemi + vec3(0.9, 0.9, 1.2) * flash;
    // a nearby wildfire throws orange light from below the canopy
    hemi = hemi + vec3(1.0, 0.45, 0.12) * frame.event.z * 0.5;
    // sky fills the shadows so they read blue, not grey; slopes facing away
    // from the sun get a touch of the anti-sun sky
    let sun_col = frame.sun_col.xyz * 1.02;
    let wrap = clamp((dot(N, sun) + 0.2) / 1.2, 0.0, 1.0);
    let dist = distance(in.world, frame.cam_pos.xyz);
    let fog = 1.0 - exp(-dist * frame.params.y);

    // wet ground: darken albedo, add a broad sky reflection + sun glint;
    // puddles pool in a low-frequency noise pattern so it reads as water
    // damp everywhere a little, water where it pools
    let wetness = wet * 0.18 + pud * 0.82;
    // Ground noise, three scales: broad patches of richer and poorer ground,
    // a mid scale that breaks up the vertex grid, and grain up close. Faded
    // with distance so far terrain does not shimmer.
    let gp = in.world.xz;
    let broad = noise2(gp * 0.045) * 0.6 + noise2(gp * 0.11 + vec2(31.0, 7.0)) * 0.4;
    let mid = noise2(gp * 0.42 + vec2(5.0, 19.0));
    let grain = noise2(gp * 2.3) * (1.0 - smoothstep(18.0, 55.0, cam_d));
    let ground = 0.84 + 0.20 * broad + 0.10 * (mid - 0.5) + 0.09 * (grain - 0.5);
    // Where materials meet, let the mid noise decide per pixel which side
    // wins, so the join is scuffed and patchy rather than a gradient.
    let scuff = (1.0 - purity) * (mid - 0.5) * 0.45;
    var albedo = in.color * det.x * (ground + scuff) * seam * (1.0 - 0.18 * wet - 0.3 * pud);
    // a little hue drift with the broad noise: sun-bleached vs damp
    albedo = albedo * mix(vec3(1.05, 1.0, 0.90), vec3(0.93, 1.0, 1.04), broad);
    albedo = albedo * (1.0 - 0.45 * broken.y) * (1.0 - 0.9 * broken.x);
    // snow cover settles on up-facing ground, patchy by the same noise as the puddles
    // (not where the snowpack draws the snow itself)
    let snow = frame.weather.y * (1.0 - step(0.5, snowp.pack.w));
    let snow_cov = snow * smoothstep(0.35, 0.9, N.y) * (0.6 + 0.4 * (1.0 - pud));
    albedo = mix(albedo, vec3(0.88, 0.92, 0.98), snow_cov * 0.85);
    // the snowpack: fresh snow white; where it has been pushed aside,
    // packed snow, bluer and a little darker (the floor of a print or a
    // trench, cold in its own shade); a glitter of crystals on the loose
    // The floor of a print: packed snow if the snow was deep enough to
    // pack, the ground itself where a paw brushed thin powder aside.
    let floor_col = mix(albedo * vec3(0.62, 0.70, 0.88), vec3(0.66, 0.76, 0.95), smoothstep(0.15, 0.35, sd));
    albedo = mix(albedo, mix(floor_col, vec3(0.93, 0.95, 1.0), loose), lying);
    // a glitter of crystals, fine and sparse, near the camera only
    let glit = step(0.996, hash_cell(vec2<i32>(floor(in.world.xz * 40.0)) + vec2<i32>(i32(floor(frame.params.x * 0.7)) * 7919, 0))) * loose * (1.0 - smoothstep(3.0, 16.0, cam_d));
    var lit = albedo * (sun_col * mix(wrap * wrap, ndl, 0.7) * shadow + hemi * 0.9 + sky * 0.2 * (1.0 - shadow));
    lit = lit + albedo * point_lights(in.world, N);
    // caustics: the sun through moving water. Under a river's surface (the
    // shallows most, fading with depth), or everywhere on a sea-floor map;
    // strongest on ground facing up at the light
    if (frame.sea.y > 0.001) {
        let depth = frame.sea.z - in.world.y;
        let river = smoothstep(0.03, 0.25, depth) * exp(-max(depth, 0.0) * 0.35);
        let amount = max(river, frame.sea.x) * frame.sea.y * (0.25 + 0.75 * max(N.y, 0.0)) * (1.0 - smoothstep(60.0, 140.0, cam_d));
        if (amount > 0.001) {
            lit = lit + albedo * sun_col * caustic(in.world.xz, frame.params.x) * amount * 3.0 * (0.3 + 0.7 * shadow) * (1.0 - night);
        }
    }
    lit = lit + vec3(1.4, 1.5, 1.7) * glit * (0.4 + shadow * 0.6);
    let V = normalize(frame.cam_pos.xyz - in.world);
    let Hv = normalize(V + sun);
    let fres = pow(1.0 - max(dot(N, V), 0.0), 3.0);
    let glint = pow(max(dot(N, Hv), 0.0), 90.0) * shadow;
    // (not under water: a river bed does not glint -- the surface above
    // it does, in water.wgsl; the bed's glints read as white blotches
    // through it)
    let submerged = smoothstep(0.02, 0.12, frame.sea.z - in.world.y) * (1.0 - frame.sea.x);
    lit = lit + (hemi * fres * 0.6 + sun_col * glint * 0.9) * wetness * (1.0 - submerged);
    let lum = dot(in.color, vec3(0.299, 0.587, 0.114));
    let haze = frame.fog_color.xyz * (0.45 + 0.55 * lum);
    lit = mix(lit, haze, fog * 0.9);
    lit = lit + frame.sun_col.xyz * shaft_light(in.world) * 0.42 * (1.0 - frame.params.w);
    // Tower lines, last, so neither fog nor night hides them: this is
    // information, not scenery.
    let ring = tower_ring(in.world.xz);
    // the towers stand on the ground layer: their lines are not drawn on
    // Starwell's Aerie 20 u above (they were, as a tinted patch)
    lit = mix(lit, ring.rgb, ring.a * (1.0 - smoothstep(6.0, 10.0, in.world.y)));
    return vec4(lit, 1.0);
}
