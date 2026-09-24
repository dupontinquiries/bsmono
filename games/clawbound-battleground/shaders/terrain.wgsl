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
    let w = vec4<f32>(in.pos, 1.0);
    out.clip = frame.view_proj * w;
    out.world = in.pos;
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
fn shadow_edge(uv: vec2<f32>) -> f32 {
    let m = max(abs(uv.x - 0.5), abs(uv.y - 0.5)) * 2.0;
    return smoothstep(0.88, 0.99, m);
}

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
    let det = mix(tri_detail(in.world, n0, 0.0, cam_d), tri_detail(in.world, n0, mat_id, cam_d), purity);
    let N = normalize(in.normal + det.yzw);
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
        shadow = mix(s / 9.0, 1.0, shadow_edge(uv));
    }

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
    let wet = frame.weather.z;
    let pud = smoothstep(0.55, 0.8, hash21(floor(in.world.xz * 0.35)) * 0.5 + 0.5 * hash21(floor(in.world.xz * 0.9 + 3.1)));
    let wetness = wet * (0.55 + 0.45 * pud);
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
    var albedo = in.color * det.x * (ground + scuff) * (1.0 - 0.32 * wetness);
    // a little hue drift with the broad noise: sun-bleached vs damp
    albedo = albedo * mix(vec3(1.05, 1.0, 0.90), vec3(0.93, 1.0, 1.04), broad);
    let broken = cracked(in.world.xz);
    albedo = albedo * (1.0 - 0.45 * broken.y) * (1.0 - 0.9 * broken.x);
    // snow cover settles on up-facing ground, patchy by the same noise as the puddles
    let snow = frame.weather.y;
    let snow_cov = snow * smoothstep(0.35, 0.9, N.y) * (0.6 + 0.4 * (1.0 - pud));
    albedo = mix(albedo, vec3(0.88, 0.92, 0.98), snow_cov * 0.85);
    var lit = albedo * (sun_col * mix(wrap * wrap, ndl, 0.7) * shadow + hemi * 0.9 + sky * 0.2 * (1.0 - shadow));
    let V = normalize(frame.cam_pos.xyz - in.world);
    let Hv = normalize(V + sun);
    let fres = pow(1.0 - max(dot(N, V), 0.0), 3.0);
    let glint = pow(max(dot(N, Hv), 0.0), 90.0) * shadow;
    lit = lit + (hemi * fres * 0.6 + sun_col * glint * 0.9) * wetness;
    let lum = dot(in.color, vec3(0.299, 0.587, 0.114));
    let haze = frame.fog_color.xyz * (0.45 + 0.55 * lum);
    lit = mix(lit, haze, fog * 0.9);
    lit = lit + frame.sun_col.xyz * shaft_light(in.world) * 0.42 * (1.0 - frame.params.w);
    // Tower lines, last, so neither fog nor night hides them: this is
    // information, not scenery.
    let ring = tower_ring(in.world.xz);
    lit = mix(lit, ring.rgb, ring.a);
    return vec4(lit, 1.0);
}
