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
    wind: vec4<f32>,     // xyz wind (u/s), w gust front 0..1
    event: vec4<f32>,    // x flood rise, y quake shake, z fire glow, w shaft steps
    brush: array<vec4<f32>, 8>, // xyz where a cat is shouldering through, w how hard (signed)
    blast_a: array<vec4<f32>, 4>, // xyz centre, w ring radius
    blast_b: array<vec4<f32>, 4>, // x strength, y width
    cracks: array<vec4<f32>, 8>,  // x, z, radius, how open
    towers: array<vec4<f32>, 12>, // (terrain.wgsl's; declared to reach `sea`)
    sea: vec4<f32>,               // x all under the sea, y caustic strength, z water surface y, w camera underwater
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

struct DrawData {
    model: mat4x4<f32>,
    tint: vec4<f32>,   // rgb tint, a = alpha multiplier
    params: vec4<f32>, // x emissive, y voxel_size, z unused, w unused (per-voxel glow: misc.z)
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

// A cat going through the flora. Same rule as the wind -- `sway` is the
// piece's weight, the base stays planted, the tips move most -- but local:
// leaves lean AWAY from each brush, and only leaves near the cat's own height,
// so a cat at the foot of a tree does not shove its canopy twelve units up.
//
// `w` is how hard. A stalking cat sends ~0.02 and nothing visibly moves; one
// at a run sends ~1.0 and a fern tip goes over by half a unit. It is signed:
// a wake (where a cat WAS) rings like a damped spring, so the leaf swings
// back through rest and leans briefly toward where the cat went.
fn brush_offset(world: vec3<f32>, local_y: f32, sway: f32) -> vec3<f32> {
    // Only what is LIGHTER than a cat gives way to one. Sway weights split
    // cleanly: bush 1.8, bamboo 2.2, vine 2.4, fern 2.6 -- and every tree at
    // 1.6 or under, whose weight covers trunk and roots too. Without this
    // gate a running cat flexed buttress roots by a fifth of a unit, which a
    // with/without diff of one frame showed as movement far outside the fern.
    let light = smoothstep(1.65, 1.8, sway);
    if (light <= 0.0) {
        return vec3(0.0);
    }
    var o = vec3(0.0);
    let h = clamp(local_y, 0.0, 2.6) * light;
    for (var i = 0; i < 8; i = i + 1) {
        let b = frame.brush[i];
        if (abs(b.w) < 0.004) {
            continue;
        }
        let d = world.xz - b.xz;
        let dist = length(d);
        let reach = 2.3;
        if (dist >= reach) {
            continue;
        }
        let near = 1.0 - dist / reach;
        let level = 1.0 - clamp(abs(world.y - (b.y + 0.7)) / 2.2, 0.0, 1.0);
        let away = d / max(dist, 0.08);
        let amt = b.w * near * near * level * h * 0.24 * min(sway, 3.0) / 2.6;
        o = o + vec3(away.x * amt, -abs(amt) * 0.4, away.y * amt);
    }
    return o;
}

// A blast front going through the forest. Unlike the brush this moves
// EVERYTHING that sways, trees included -- but by the wind's own rule, height
// squared, so trunks stay planted and crowns whip. The ring has a leading
// push and a trailing pull, so a tree leans away and then rocks back as the
// front passes it, and the wave is seen to travel.
fn blast_offset(world: vec3<f32>, local_y: f32, sway: f32) -> vec3<f32> {
    if (sway <= 0.0) {
        return vec3(0.0);
    }
    var o = vec3(0.0);
    let h = max(local_y, 0.0);
    for (var i = 0; i < 4; i = i + 1) {
        let s = frame.blast_b[i].x;
        if (s < 0.003) {
            continue;
        }
        let c = frame.blast_a[i];
        let d = world.xz - c.xz;
        let dist = length(d);
        let x = (dist - c.w) / frame.blast_b[i].y;
        if (abs(x) > 3.0) {
            continue;
        }
        // derivative-of-gaussian: out ahead of the front, back behind it
        let wave = -x * exp(-x * x) * 2.3 + exp(-x * x) * 0.6;
        let away = d / max(dist, 0.1);
        let amt = s * wave * min(h * h * 0.035, 1.6) * min(sway, 2.6);
        o = o + vec3(away.x * amt, -abs(amt) * 0.25, away.y * amt);
    }
    return o;
}

@vertex
fn vs_main(in: VSIn) -> VSOut {
    let d = draws[0]; // dynamic offset selects this draw
    let voxel = d.params.y * in.misc.x;
    let local = in.pos * voxel + in.off_ao.xyz;
    var world = d.model * vec4<f32>(local, 1.0);
    world = vec4(world.xyz + wind_offset(world.xyz, in.off_ao.y, in.misc.y) + brush_offset(world.xyz, in.off_ao.y, in.misc.y) + blast_offset(world.xyz, in.off_ao.y, in.misc.y), 1.0);
    let nrm = normalize((d.model * vec4<f32>(in.normal, 0.0)).xyz);
    var out: VSOut;
    out.clip = frame.view_proj * world;
    out.world = world.xyz;
    out.normal = nrm;
    out.color = in.color * d.tint;
    out.shadow_pos = frame.light_view_proj * world;
    out.ao = in.off_ao.w;
    // per-draw emissive, plus per-voxel glow baked in misc.z (gems in the
    // static world: they glow inside a batched chunk, no draw of their own)
    out.emissive = d.params.x + in.misc.z;
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

    // contact-hardening: crisp where a shadow meets its caster (common_shadow.wgsl)
    let shadow = sun_shadow(in.shadow_pos, 0.0032, in.clip.xy);

    // --- lighting model ----------------------------------------------------
    // Three-point-ish: a warm key (sun), a cool sky fill from above, a
    // ground bounce from below tinted by the haze, and a soft back/rim light
    // from the side opposite the sun. Shadowed areas take on the sky colour
    // instead of going grey, and the terminator warms (wrap lighting).
    let night = frame.params.w;
    let cloud = frame.sun_col.w;
    let flash = frame.weather.w;
    let sun_col = frame.sun_col.xyz;
    let V = normalize(frame.cam_pos.xyz - in.world);

    // sky + ground colours track the weather
    var sky = mix(vec3(0.50, 0.60, 0.85), vec3(0.55, 0.57, 0.62), cloud);
    sky = mix(sky, vec3(0.05, 0.07, 0.18), night);
    var ground = mix(vec3(0.42, 0.34, 0.22), frame.fog_color.xyz, 0.5) * mix(1.0, 0.7, cloud);
    ground = mix(ground, vec3(0.03, 0.03, 0.06), night);
    let up = N.y * 0.5 + 0.5;
    var hemi = mix(ground, sky, up) * 0.78;
    hemi = hemi + vec3(0.9, 0.9, 1.2) * flash * up;
    hemi = hemi + vec3(1.0, 0.45, 0.12) * frame.event.z * 0.45 * (1.0 - up * 0.5);

    // key: wrap-lit so the terminator isn't a hard grey line; warm the wrap
    let wrap = clamp((dot(N, sun) + 0.25) / 1.25, 0.0, 1.0);
    let key = sun_col * mix(wrap * wrap, ndl, 0.6) * shadow;
    // where the sun is blocked, let the sky fill in softly (no dead black)
    let fill = sky * 0.22 * (1.0 - shadow) * up;
    // rim: back light from the anti-sun direction, only at grazing view angles
    let back = normalize(vec3(-sun.x, 0.35, -sun.z));
    let rim = pow(1.0 - max(dot(N, V), 0.0), 3.0) * max(dot(N, back), 0.0) * 0.35 * mix(1.0, 0.4, night);
    var light = key + hemi + fill + sky * rim;

    // specular: a broad soft lobe on everything (voxels read as matte
    // enamel), a tighter one when wet
    let Hv = normalize(V + sun);
    let nh = max(dot(N, Hv), 0.0);
    let spec = pow(nh, 14.0) * 0.10 + pow(nh, 48.0) * frame.weather.z * 0.45;
    light = light + sun_col * spec * shadow;

    // contact darkening: things near the ground lose sky light
    let contact = smoothstep(0.0, 1.2, in.world.y) * 0.25 + 0.75;
    light = light * contact;

    // distance fog — melts into warm golden jungle haze
    let dist = distance(in.world, frame.cam_pos.xyz);
    let fog = 1.0 - exp(-dist * frame.params.y);
    let albedo = in.color.rgb;
    // baked AO: stronger in the crevices, let the key light punch through it a little
    let ao = mix(in.ao, 1.0, 0.18 * ndl * shadow);
    var lit = albedo * light * ao;
    // caustics on a sea-floor map (Atlantis): the halls' stone and statues
    // ripple with light from above, most on their tops
    if (frame.sea.x > 0.5 && frame.sea.y > 0.001) {
        let amount = frame.sea.y * (0.2 + 0.8 * max(N.y, 0.0)) * (1.0 - smoothstep(60.0, 140.0, dist));
        lit = lit + albedo * sun_col * caustic(in.world.xz + vec2(in.world.y * 0.35), frame.params.x) * amount * (0.35 + 0.65 * shadow) * ao;
    }
    let lum = dot(albedo, vec3(0.299, 0.587, 0.114));
    let haze = frame.fog_color.xyz * (0.45 + 0.55 * lum);
    lit = mix(lit, haze, fog * 0.9);
    lit = lit + frame.sun_col.xyz * shaft_light(in.world) * 0.42 * (1.0 - frame.params.w);
    // emissive glow (crystals, projectiles, VFX)
    lit = lit + albedo * in.emissive * (1.0 - fog * 0.4);

    // cheap tonemap here is skipped for HDR path; alpha handling:
    return vec4(lit, in.color.a);
}
