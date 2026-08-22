// Side-view scene shader: procedural ground/sky, a car silhouette
// (body/cabin/wheels, shaped and colored per CarKind), a boss with a
// telegraph flash, and a grit/dust/grain post pass — composited in one
// fullscreen fragment shader.
//
// Confirmed working: run through the real naga/wgpu pipeline (Vulkan via
// lavapipe under Xvfb) repeatedly this session, screenshotted at every step.
// Two real bugs were caught this way and are fixed: a sky/ground vertical
// flip from misjudging wgpu's Y-up NDC convention, and a uniform-buffer
// size mismatch from using vec3<f32> for padding (see the Uniforms struct
// comment below for why that specifically breaks).

struct Uniforms {
    car_x: f32,       // world-space car position, normalized to [-1,1] view space by CPU side
    car_y: f32,
    car_rotation: f32,
    time: f32,
    resolution: vec2<f32>,
    boss_relative_x: f32,
    boss_present: f32,
    boss_telegraphing: f32,
    // car_kind selects body color/silhouette (0=Behemoth, 1=SixWheeler,
    // 2=RedSedan, 3=Pillager). The other two are still plain scalar padding
    // — not vec3<f32>, which has a 16-byte alignment rule in WGSL's uniform
    // address space that silently inflates the struct's required size
    // (confirmed the hard way: wgpu's validator rejected a 48-byte buffer
    // because the shader, with a trailing vec3 pad, actually expected 64).
    car_kind: f32,
    _pad1: f32,
    _pad2: f32,
};

@group(0) @binding(0)
var<uniform> u: Uniforms;

struct VertexOut {
    @builtin(position) clip_position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> VertexOut {
    // Fullscreen triangle, no vertex buffer needed.
    var positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    var out: VertexOut;
    let p = positions[idx];
    out.clip_position = vec4<f32>(p, 0.0, 1.0);
    out.uv = p * 0.5 + vec2<f32>(0.5, 0.5);
    return out;
}

fn hash(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 = p3 + dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn noise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let a = hash(i);
    let b = hash(i + vec2<f32>(1.0, 0.0));
    let c = hash(i + vec2<f32>(0.0, 1.0));
    let d = hash(i + vec2<f32>(1.0, 1.0));
    let u = f * f * (3.0 - 2.0 * f);
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

// Layered dust streaks scrolling opposite the car's motion direction,
// standing in for kicked-up road dust.
fn dust(uv: vec2<f32>, scroll: f32) -> f32 {
    var v = 0.0;
    var amp = 0.5;
    var p = uv * vec2<f32>(3.0, 8.0) + vec2<f32>(scroll, 0.0);
    for (var i = 0; i < 4; i = i + 1) {
        v = v + noise(p) * amp;
        p = p * 2.0 + vec2<f32>(0.0, 1.7);
        amp = amp * 0.5;
    }
    return v;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let aspect = u.resolution.x / max(u.resolution.y, 1.0);

    // --- base scene: ground/sky split ---
    // uv.y is 0 at the bottom of the screen, 1 at the top (wgpu's NDC is
    // Y-up, same as OpenGL) — ground belongs in the low-uv.y branch, sky in
    // the high one. Confirmed by actually running this: the first version
    // had these swapped, putting sky at the bottom of the screen.
    let horizon = 0.62;
    var color: vec3<f32>;
    if (uv.y < horizon) {
        let ground_t = uv.y / horizon;
        let dirt_noise = noise(uv * vec2<f32>(40.0 * aspect, 40.0) + vec2<f32>(u.car_x * 2.0, 0.0));
        let base = mix(vec3<f32>(0.20, 0.15, 0.11), vec3<f32>(0.36, 0.28, 0.20), ground_t);
        color = base + vec3<f32>(dirt_noise * 0.06);
    } else {
        let sky_t = (uv.y - horizon) / (1.0 - horizon);
        color = mix(vec3<f32>(0.82, 0.80, 0.74), vec3<f32>(0.55, 0.62, 0.68), sky_t);
    }

    // --- car: body + cabin + wheels, shaped/colored per car kind ---
    // The car stays put on screen; the world should scroll underneath it
    // (dirt noise and dust already key off u.car_x for that). A car_screen.x
    // tied directly to car_x was a real bug caught by actually driving
    // across a full-length track: the car ran off the right edge of the
    // screen well before reaching even a third of an 88-unit track, because
    // nothing was translating world position into a bounded screen offset.
    let car_screen = vec2<f32>(0.32, horizon - 0.05 + u.car_y * 0.02);
    let c = cos(-u.car_rotation);
    let s = sin(-u.car_rotation);
    let rel = uv - car_screen;
    // rel.x is pre-multiplied by aspect before rotating, so the rotation
    // itself happens in a uniform (non-stretched) space — this is what
    // makes the wheel circles below render as true circles instead of
    // ellipses, and rotations look like real rotations instead of shears.
    let rel_rot = vec2<f32>(rel.x * aspect * c - rel.y * s, rel.x * aspect * s + rel.y * c);

    // Behemoth is visibly bigger (heavy/armored), RedSedan slightly smaller
    // and sleeker (fast, light) — matches the stat differences in vehicle.rs.
    var size_scale = 1.0;
    if (u.car_kind < 0.5) { size_scale = 1.25; }       // Behemoth
    else if (u.car_kind > 1.5 && u.car_kind < 2.5) { size_scale = 0.85; } // RedSedan

    let body_half = vec2<f32>(0.055, 0.022) * size_scale;
    let cabin_half = vec2<f32>(0.024, 0.016) * size_scale;
    let cabin_offset = vec2<f32>(-0.006, body_half.y + cabin_half.y * 0.7);
    let wheel_radius = 0.013 * size_scale;
    let wheel_y = -body_half.y - wheel_radius * 0.5;
    let wheel_front_x = body_half.x * 0.55;
    let wheel_rear_x = -body_half.x * 0.55;
    let is_six_wheeler = u.car_kind > 0.5 && u.car_kind < 1.5;

    let in_body = abs(rel_rot.x) < body_half.x && abs(rel_rot.y) < body_half.y;
    let cabin_rel = rel_rot - cabin_offset;
    let in_cabin = abs(cabin_rel.x) < cabin_half.x && abs(cabin_rel.y) < cabin_half.y;
    let dist_front_wheel = distance(rel_rot, vec2<f32>(wheel_front_x, wheel_y));
    let dist_rear_wheel = distance(rel_rot, vec2<f32>(wheel_rear_x, wheel_y));
    // A third, centered wheel — the six-wheeler's namesake feature, and the
    // one visual detail that's actually specific to a single car archetype
    // rather than just a size/color tweak.
    let dist_mid_wheel = distance(rel_rot, vec2<f32>(0.0, wheel_y));
    let in_wheel = dist_front_wheel < wheel_radius
        || dist_rear_wheel < wheel_radius
        || (is_six_wheeler && dist_mid_wheel < wheel_radius);

    if (in_body || in_cabin || in_wheel) {
        if (in_wheel) {
            color = vec3<f32>(0.05, 0.05, 0.05);
        } else if (u.car_kind < 0.5) {
            color = vec3<f32>(0.45, 0.42, 0.40); // Behemoth: gunmetal grey
        } else if (u.car_kind < 1.5) {
            color = vec3<f32>(0.35, 0.50, 0.30); // SixWheeler: olive drab
        } else if (u.car_kind < 2.5) {
            color = vec3<f32>(0.75, 0.12, 0.10); // RedSedan
        } else {
            color = vec3<f32>(0.85, 0.65, 0.15); // Pillager: yellow-gold
        }
    }

    // --- boss: appears ahead of the car, scaled from world-space distance ---
    // Distance-to-screen scale is tuned so the boss is off-screen until
    // reasonably close, then comes into view as the car approaches it —
    // there's no real depth/perspective here, just a linear proxy for it.
    if (u.boss_present > 0.5) {
        let boss_distance_scale = 0.025;
        let boss_screen = vec2<f32>(car_screen.x + u.boss_relative_x * boss_distance_scale, horizon - 0.09);
        let boss_half_size = vec2<f32>(0.05, 0.07); // bigger than the car -- reads as a threat
        let boss_rel = uv - boss_screen;
        if (abs(boss_rel.x) < boss_half_size.x && abs(boss_rel.y) < boss_half_size.y) {
            if (u.boss_telegraphing > 0.5) {
                // Flash between near-white and a saturated amber, not red —
                // confirmed via an actual screenshot that a red/near-white
                // flash was close enough to the car's own red body color to
                // be hard to tell apart at a glance, defeating the point of
                // a telegraph. Amber has no overlap with the car's palette.
                let flash = step(0.5, fract(u.time * 8.0));
                color = mix(vec3<f32>(0.95, 0.55, 0.05), vec3<f32>(0.98, 0.92, 0.75), flash);
            } else {
                color = vec3<f32>(0.22, 0.14, 0.30); // muted purple -- idle, distinct from both car and ground
            }
        }
    }

    // --- dust kicked up behind the car ---
    let dust_amt = dust(uv, u.time * 0.6 - u.car_x * 0.3);
    let dust_mask = smoothstep(0.55, 0.0, distance(uv, car_screen) * vec2<f32>(1.0, 2.0).x);
    color = mix(color, vec3<f32>(0.6, 0.55, 0.45), dust_amt * dust_mask * 0.35);

    // --- grain ---
    let grain = hash(uv * u.resolution + vec2<f32>(u.time * 60.0, 0.0));
    color = color + (grain - 0.5) * 0.05;

    // --- vignette ---
    let d = distance(uv, vec2<f32>(0.5, 0.5));
    color = color * (1.0 - smoothstep(0.4, 0.9, d) * 0.5);

    return vec4<f32>(clamp(color, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
